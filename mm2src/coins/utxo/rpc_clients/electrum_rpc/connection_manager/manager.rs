use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak};

use super::super::client::{ElectrumClient, ElectrumClientImpl};
use super::super::connection::{ElectrumConnection, ElectrumConnectionErr, ElectrumConnectionSettings};
use super::super::constants::{BACKGROUND_TASK_WAIT_TIMEOUT, PING_INTERVAL};
use super::connection_context::ConnectionContext;

use crate::utxo::rpc_clients::UtxoRpcClientOps;
use common::executor::abortable_queue::AbortableQueue;
use common::executor::{AbortableSystem, SpawnFuture, Timer};
use common::log::{debug, error, LogOnError};
use common::notifier::{Notifiee, Notifier};
use common::now_ms;
use derive_more::Display;
use keys::Address;

use futures::compat::Future01CompatExt;
use futures::FutureExt;

/// A macro to unwrap an option and *execute* some code if the option is None.
macro_rules! unwrap_or_else {
    ($option:expr, $($action:tt)*) => {{
        match $option {
            Some(some_val) => some_val,
            None => { $($action)* }
        }
    }};
}

macro_rules! unwrap_or_continue {
    ($option:expr) => {
        unwrap_or_else!($option, continue)
    };
}

macro_rules! unwrap_or_return {
    ($option:expr, $ret:expr) => {
        unwrap_or_else!($option, return $ret)
    };
    ($option:expr) => {
        unwrap_or_else!($option, return)
    };
}

/// The ID of a connection (and also its priority, lower is better).
type ID = u32;

#[derive(Debug, Display)]
pub enum ConnectionManagerErr {
    #[display(fmt = "Unknown server address")]
    UnknownAddress,
    #[display(fmt = "Failed to connect to the server due to {_0:?}")]
    ConnectingError(ElectrumConnectionErr),
    #[display(fmt = "No client found, connection manager isn't initialized properly")]
    NoClient,
    #[display(fmt = "Connection manager is already initialized")]
    AlreadyInitialized,
}

/// The configuration parameter for a connection manager.
#[derive(Debug)]
pub struct ManagerConfig {
    /// A flag to spawn a ping loop task for active connections.
    pub spawn_ping: bool,
    /// The minimum number of connections that should be connected at all times.
    pub min_connected: usize,
    /// The maximum number of connections that can be connected at any given time.
    pub max_connected: usize,
}

#[derive(Debug)]
/// A connection manager that maintains a set of connections to electrum servers and
/// handles reconnecting, address subscription distribution, etc...
struct ConnectionManagerImpl {
    /// The configuration for the connection manager.
    config: ManagerConfig,
    /// The set of addresses that are currently connected.
    ///
    /// This set's size should satisfy: `min_connected <= maintained_connections.len() <= max_connected`.
    ///
    /// It is actually represented as a sorted map from connection ID (u32, also represents connection priority)
    /// to address so we can easily/cheaply pop low priority connections and add high priority ones.
    maintained_connections: RwLock<BTreeMap<ID, String>>,
    /// A map for server addresses to their corresponding connections.
    connections: RwLock<HashMap<String, ConnectionContext>>,
    /// A weak reference to the electrum client that owns this connection manager.
    /// It is used to send electrum requests during connection establishment (version querying).
    // TODO: This field might not be necessary if [`ElectrumConnection`] object be used to send
    // electrum requests on its own, i.e. implement [`JsonRpcClient`] & [`UtxoRpcClientOps`].
    electrum_client: RwLock<Option<Weak<ElectrumClientImpl>>>,
    /// A notification sender to notify the background task when we have less than `min_connected` connections.
    below_min_connected_notifier: Notifier,
    /// A notification receiver to be used by the background task to receive notifications of when
    /// we have less than `min_connected` maintained connections.
    ///
    /// Wrapped inside a Mutex<Option< to be taken out when the background task is spawned.
    below_min_connected_notifiee: Mutex<Option<Notifiee>>,
}

#[derive(Clone, Debug)]
pub struct ConnectionManager(Arc<ConnectionManagerImpl>);

// Public interface.
impl ConnectionManager {
    pub fn try_new(
        servers: Vec<ElectrumConnectionSettings>,
        spawn_ping: bool,
        (min_connected, max_connected): (usize, usize),
        abortable_system: &AbortableQueue,
    ) -> Result<Self, String> {
        let mut connections = HashMap::with_capacity(servers.len());
        // Priority is assumed to be the order of the servers in the list as they appear.
        for (priority, connection_settings) in servers.into_iter().enumerate() {
            let subsystem = abortable_system.create_subsystem().map_err(|e| {
                ERRL!(
                    "Failed to create abortable subsystem for connection: {}, error: {:?}",
                    connection_settings.url,
                    e
                )
            })?;
            let connection = ElectrumConnection::new(connection_settings, subsystem);
            connections.insert(
                connection.address().to_string(),
                ConnectionContext::new(connection, priority as u32),
            );
        }

        if min_connected == 0 {
            return Err(ERRL!("min_connected should be greater than 0"));
        }
        if min_connected > max_connected {
            return Err(ERRL!(
                "min_connected ({}) must be <= max_connected ({})",
                min_connected,
                max_connected
            ));
        }

        let (notifier, notifiee) = Notifier::new();
        Ok(ConnectionManager(Arc::new(ConnectionManagerImpl {
            config: ManagerConfig {
                spawn_ping,
                min_connected,
                max_connected,
            },
            connections: RwLock::new(connections),
            maintained_connections: RwLock::new(BTreeMap::new()),
            electrum_client: RwLock::new(None),
            below_min_connected_notifier: notifier,
            below_min_connected_notifiee: Mutex::new(Some(notifiee)),
        })))
    }

    /// Initializes the connection manager by connecting the electrum connections.
    /// This must be called and only be called once to have a functioning connection manager.
    pub fn initialize(&self, weak_client: Weak<ElectrumClientImpl>) -> Result<(), ConnectionManagerErr> {
        // Disallow reusing the same manager with another client.
        if self.weak_client().read().unwrap().is_some() {
            return Err(ConnectionManagerErr::AlreadyInitialized);
        }

        let electrum_client = unwrap_or_return!(weak_client.upgrade(), Err(ConnectionManagerErr::NoClient));

        // Store the (weak) electrum client.
        *self.weak_client().write().unwrap() = Some(weak_client);

        // Use the client's spawner to spawn the connection manager's background task.
        electrum_client.weak_spawner().spawn(self.clone().background_task());

        if self.config().spawn_ping {
            // Use the client's spawner to spawn the connection manager's ping task.
            electrum_client.weak_spawner().spawn(self.clone().ping_task());
        }

        Ok(())
    }

    /// Returns all the server addresses.
    pub fn get_all_server_addresses(&self) -> Vec<String> {
        self.read_connections().keys().cloned().collect()
    }

    /// Returns all the connections.
    pub fn get_all_connections(&self) -> Vec<Arc<ElectrumConnection>> {
        self.read_connections()
            .values()
            .map(|conn_ctx| conn_ctx.connection.clone())
            .collect()
    }

    /// Retrieve a specific electrum connection by its address.
    /// The connection will be forcibly established if it's disconnected.
    pub async fn get_connection_by_address(
        &self,
        server_address: &str,
        force_connect: bool,
    ) -> Result<Arc<ElectrumConnection>, ConnectionManagerErr> {
        let connection = self
            .get_connection(server_address)
            .ok_or(ConnectionManagerErr::UnknownAddress)?;

        if force_connect {
            let client = unwrap_or_return!(self.get_client(), Err(ConnectionManagerErr::NoClient));
            // Make sure the connection is connected.
            connection
                .establish_connection_loop(client)
                .await
                .map_err(ConnectionManagerErr::ConnectingError)?;
        }

        Ok(connection)
    }

    /// Returns a list of active/maintained connections.
    pub fn get_active_connections(&self) -> Vec<Arc<ElectrumConnection>> {
        self.read_maintained_connections()
            .iter()
            .filter_map(|(_id, address)| self.get_connection(address))
            .collect()
    }

    /// Returns a boolean `true` if the connection pool is empty, `false` otherwise.
    pub fn is_connections_pool_empty(&self) -> bool {
        self.read_connections().is_empty()
    }

    /// Subscribe the list of addresses to our active connections.
    ///
    /// There is a bit of indirection here. We register the abandoned addresses on `on_disconnected` with
    /// the client to queue them for `utxo_balance_events` which in turn calls this method back to re-subscribe
    /// the abandoned addresses. We could have instead directly re-subscribed the addresses here in the connection
    /// manager without sending them to `utxo_balance_events`. However, we don't do that so that `utxo_balance_events`
    /// knows about all the added addresses. If it doesn't know about them, it won't be able to retrieve the triggered
    /// address when its script hash is notified.
    pub async fn add_subscriptions(&self, addresses: &HashMap<String, Address>) {
        for (scripthash, address) in addresses.iter() {
            // For a single address/scripthash, keep trying to subscribe it until we succeed.
            'single_address_sub: loop {
                let client = unwrap_or_return!(self.get_client());
                let connections = self.get_active_connections();
                if connections.is_empty() {
                    // If there are no active connections, wait for a connection to be established.
                    Timer::sleep(1.).await;
                    continue;
                }
                // Try to subscribe the address to any connection we have.
                for connection in connections {
                    if client
                        .blockchain_scripthash_subscribe_using(connection.address(), scripthash.clone())
                        .compat()
                        .await
                        .is_ok()
                    {
                        let all_connections = self.read_connections();
                        let connection_ctx = unwrap_or_continue!(all_connections.get(connection.address()));
                        connection_ctx.add_sub(address.clone());
                        break 'single_address_sub;
                    }
                }
            }
        }
    }

    /// Handles the connection event.
    pub fn on_connected(&self, server_address: &str) {
        let all_connections = self.read_connections();
        let connection_ctx = unwrap_or_return!(all_connections.get(server_address));

        // Reset the suspend time & disconnection time.
        connection_ctx.connected();
    }

    /// Handles the disconnection event from an Electrum server.
    pub fn on_disconnected(&self, server_address: &str) {
        debug!("Electrum server disconnected: {}", server_address);
        let all_connections = self.read_connections();
        let connection_ctx = unwrap_or_return!(all_connections.get(server_address));

        self.unmaintain(connection_ctx.id);

        let abandoned_subs = connection_ctx.disconnected();
        // Re-subscribe the abandoned addresses using the client.
        let client = unwrap_or_return!(self.get_client());
        client.subscribe_addresses(abandoned_subs).error_log();
    }

    /// A method that should be called after using a specific server for some request.
    ///
    /// Instead of disconnecting the connection right away, this method will only disconnect it
    /// if it's not in the maintained connections set.
    pub fn not_needed(&self, server_address: &str) {
        let (id, connection) = {
            let all_connections = self.read_connections();
            let connection_ctx = unwrap_or_return!(all_connections.get(server_address));
            (connection_ctx.id, connection_ctx.connection.clone())
        };
        if !self.read_maintained_connections().contains_key(&id) {
            connection.disconnect(Some(ElectrumConnectionErr::Temporary("Not needed anymore".to_string())));
            self.on_disconnected(connection.address());
        }
    }

    /// Remove a connection from the connection manager by its address.
    // TODO(feat): Add the ability to add a connection during runtime.
    pub fn remove_connection(&self, server_address: &str) -> Result<Arc<ElectrumConnection>, ConnectionManagerErr> {
        let connection = self
            .get_connection(server_address)
            .ok_or(ConnectionManagerErr::UnknownAddress)?;
        // Make sure this connection is disconnected.
        connection.disconnect(Some(ElectrumConnectionErr::Irrecoverable(
            "Forcefully disconnected & removed".to_string(),
        )));
        // Run the on-disconnection hook, this will also make sure the connection is removed from the maintained set.
        self.on_disconnected(connection.address());
        // Remove the connection from the manager.
        self.write_connections().remove(server_address);
        Ok(connection)
    }
}

// Background tasks.
impl ConnectionManager {
    /// A forever-lived task that pings active/maintained connections periodically.
    async fn ping_task(self) {
        loop {
            let client = unwrap_or_return!(self.get_client());
            // This will ping all the active/maintained connections, which will keep these connections alive.
            client.server_ping().compat().await.ok();
            Timer::sleep(PING_INTERVAL).await;
        }
    }

    /// A forever-lived task that does the house keeping tasks of the connection manager:
    ///     - Maintaining the right number of active connections.
    ///     - Establishing new connections if needed.
    ///     - Replacing low priority connections with high priority ones periodically.
    ///     - etc...
    async fn background_task(self) {
        // Take out the min_connected notifiee from the manager.
        let mut min_connected_notification = unwrap_or_return!(self.extract_below_min_connected_notifiee());
        // A flag to indicate whether to log connection establishment errors or not. We should not log them if we
        // are in panic mode (i.e. we are below the `min_connected` threshold) as this will flood the error log.
        let mut log_errors = true;
        loop {
            // Get the candidate connections that we will consider maintaining.
            let (candidate_connections, will_never_get_min_connected) = self.get_candidate_connections();
            // Establish the connections to the selected candidates and alter the maintained connections set accordingly.
            self.establish_best_connections(candidate_connections, log_errors).await;
            // Only sleep if we successfully acquired the minimum number of connections,
            // or if we know we can never maintain `min_connected` connections; there is no point of infinite non-wait looping then.
            if self.read_maintained_connections().len() >= self.config().min_connected || will_never_get_min_connected {
                // Wait for a timeout or a below `min_connected` notification before doing another round of house keeping.
                futures::select! {
                    _ = Timer::sleep(BACKGROUND_TASK_WAIT_TIMEOUT).fuse() => (),
                    _ = min_connected_notification.wait().fuse() => (),
                }
                log_errors = true;
            } else {
                // Never sleeping can result in busy waiting, which is problematic as it might not
                // give a chance to other tasks to make progress, especially in single threaded environments.
                // Yield the execution to the executor to give a chance to other tasks to run.
                // TODO: `yield` keyword is not supported in the current rust version, using a short sleep for now.
                Timer::sleep(1.).await;
                log_errors = false;
            }
        }
    }

    /// Returns a list of candidate connections that aren't maintained and could be considered for maintaining.
    ///
    /// Also returns a flag indicating whether covering `min_connected` connections is even possible: not possible when
    /// `min_connected` is greater than the number of connections we have.
    fn get_candidate_connections(&self) -> (Vec<(Arc<ElectrumConnection>, u32)>, bool) {
        let all_connections = self.read_connections();
        let maintained_connections = self.read_maintained_connections();
        // The number of connections we need to add as maintained to reach the `min_connected` threshold.
        let connections_needed = self.config().min_connected.saturating_sub(maintained_connections.len());
        // The connections that we can consider (all connections - candidate connections).
        let all_candidate_connections: Vec<_> = all_connections
            .iter()
            .filter(|&(_, conn_ctx)| !maintained_connections.contains_key(&conn_ctx.id))
            .map(|(_, conn_ctx)| (conn_ctx.connection.clone(), conn_ctx.id))
            .collect();
        // The candidate connections from above, but further filtered by whether they are suspended or not.
        let non_suspended_candidate_connections: Vec<_> = all_candidate_connections
            .iter()
            .filter(|(connection, _)| {
                all_connections
                    .get(connection.address())
                    .is_some_and(|conn_ctx| now_ms() > conn_ctx.suspended_till())
            })
            .cloned()
            .collect();
        // Decide which candidate connections to consider (all or only non-suspended).
        if connections_needed > non_suspended_candidate_connections.len() {
            if connections_needed > all_candidate_connections.len() {
                // Not enough connections to cover the `min_connected` threshold.
                // This means we will never be able to maintain `min_connected` active connections.
                (all_candidate_connections, true)
            } else {
                // If we consider all candidate connection (but some are suspended), we can cover the needed connections.
                // We will consider the suspended ones since if we don't we will stay below `min_connected` threshold.
                (all_candidate_connections, false)
            }
        } else {
            // Non suspended candidates are enough to cover the needed connections.
            (non_suspended_candidate_connections, false)
        }
    }

    /// Establishes the best connections (based on priority) using the candidate connections
    /// till we can't establish no more (hit the `max_connected` threshold).
    async fn establish_best_connections(
        &self,
        mut candidate_connections: Vec<(Arc<ElectrumConnection>, u32)>,
        log_errors: bool,
    ) {
        let client = unwrap_or_return!(self.get_client());
        // Sort the candidate connections by their priority/ID.
        candidate_connections.sort_by_key(|(_, priority)| *priority);
        for (connection, connection_id) in candidate_connections {
            let address = connection.address().to_string();
            let (maintained_connections_size, lowest_priority_connection_id) = {
                let maintained_connections = self.read_maintained_connections();
                let maintained_connections_size = maintained_connections.len();
                let lowest_priority_connection_id = *maintained_connections.keys().next_back().unwrap_or(&u32::MAX);
                (maintained_connections_size, lowest_priority_connection_id)
            };

            // We can only try to add the connection if:
            //     1- We haven't reached the `max_connected` threshold.
            //     2- We have reached the `max_connected` threshold but the connection has a higher priority than the lowest priority connection.
            if maintained_connections_size < self.config().max_connected
                || connection_id < lowest_priority_connection_id
            {
                // Now that we know the connection is good to be inserted, try to establish it.
                if let Err(e) = connection.establish_connection_loop(client.clone()).await {
                    if log_errors {
                        error!("Failed to establish connection to {address} due to error: {e:?}");
                    }
                    // Remove the connection if it's not recoverable.
                    if !e.is_recoverable() {
                        self.remove_connection(&address).ok();
                    }
                    continue;
                }
                self.maintain(connection_id, address);
            } else {
                // If any of the two conditions on the `if` statement above are not met, there is nothing to do.
                // At this point we have already collected `max_connected` connections and also the current connection
                // in the candidate list has a lower priority than the lowest priority maintained connection, and the next
                // candidate connections as well since they are sorted by priority.
                break;
            }
        }
    }
}

// Abstractions over the accesses of the inner fields of the connection manager.
impl ConnectionManager {
    #[inline]
    pub fn config(&self) -> &ManagerConfig {
        &self.0.config
    }

    #[inline]
    fn read_connections(&self) -> RwLockReadGuard<'_, HashMap<String, ConnectionContext>> {
        self.0.connections.read().unwrap()
    }

    #[inline]
    fn write_connections(&self) -> RwLockWriteGuard<'_, HashMap<String, ConnectionContext>> {
        self.0.connections.write().unwrap()
    }

    #[inline]
    fn get_connection(&self, server_address: &str) -> Option<Arc<ElectrumConnection>> {
        self.read_connections()
            .get(server_address)
            .map(|connection_ctx| connection_ctx.connection.clone())
    }

    #[inline]
    fn read_maintained_connections(&self) -> RwLockReadGuard<'_, BTreeMap<ID, String>> {
        self.0.maintained_connections.read().unwrap()
    }

    #[inline]
    fn maintain(&self, id: ID, server_address: String) {
        let mut maintained_connections = self.0.maintained_connections.write().unwrap();
        maintained_connections.insert(id, server_address);
        // If we have reached the `max_connected` threshold then remove the lowest priority connection.
        if maintained_connections.len() > self.config().max_connected {
            let lowest_priority_connection_id = *maintained_connections.keys().next_back().unwrap_or(&u32::MAX);
            maintained_connections.remove(&lowest_priority_connection_id);
        }
    }

    #[inline]
    fn unmaintain(&self, id: ID) {
        // To avoid write locking the maintained connections, just make sure the connection is actually maintained first.
        let is_maintained = self.read_maintained_connections().contains_key(&id);
        if is_maintained {
            // If the connection was maintained, remove it from the maintained connections.
            let mut maintained_connections = self.0.maintained_connections.write().unwrap();
            maintained_connections.remove(&id);
            // And notify the background task if we fell below the `min_connected` threshold.
            if maintained_connections.len() < self.config().min_connected {
                self.notify_below_min_connected()
            }
        }
    }

    #[inline]
    fn notify_below_min_connected(&self) {
        self.0.below_min_connected_notifier.notify().ok();
    }

    #[inline]
    fn extract_below_min_connected_notifiee(&self) -> Option<Notifiee> {
        self.0.below_min_connected_notifiee.lock().unwrap().take()
    }

    #[inline]
    fn weak_client(&self) -> &RwLock<Option<Weak<ElectrumClientImpl>>> {
        &self.0.electrum_client
    }

    #[inline]
    fn get_client(&self) -> Option<ElectrumClient> {
        self.weak_client()
            .read()
            .unwrap()
            .as_ref() // None here = client was never initialized.
            .and_then(|weak| weak.upgrade().map(ElectrumClient)) // None here = client was dropped.
    }
}
