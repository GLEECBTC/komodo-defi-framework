use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use crate::streamer::spawn;
use crate::{Event, EventStreamer, StreamerId};
use common::executor::abortable_queue::WeakSpawner;
use common::log::{error, LogOnError};

use common::on_drop_callback::OnDropCallback;
use futures::channel::mpsc::UnboundedSender;
use futures::channel::oneshot;
use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use tokio::sync::mpsc;

/// The errors that could originate from the streaming manager.
#[derive(Debug)]
pub enum StreamingManagerError {
    /// There is no streamer with the given ID.
    StreamerNotFound,
    /// Couldn't send the data to the streamer.
    SendError(String),
    /// The streamer doesn't accept an input.
    NoDataIn,
    /// Couldn't spawn the streamer.
    SpawnError(String),
    /// The client is not known/registered.
    UnknownClient,
    /// A client with the same ID already exists.
    ClientExists,
    /// The client is already listening to the streamer.
    ClientAlreadyListening,
}

#[derive(Debug)]
struct StreamerInfo {
    /// The communication channel to the streamer.
    data_in: Option<UnboundedSender<Box<dyn Any + Send>>>,
    /// Clients the streamer is serving for.
    clients: HashSet<u64>,
    /// The shutdown handle of the streamer.
    shutdown: oneshot::Sender<()>,
}

impl StreamerInfo {
    fn new(data_in: Option<UnboundedSender<Box<dyn Any + Send>>>, shutdown: oneshot::Sender<()>) -> Self {
        Self {
            data_in,
            clients: HashSet::new(),
            shutdown,
        }
    }

    fn add_client(&mut self, client_id: u64) {
        self.clients.insert(client_id);
    }

    fn remove_client(&mut self, client_id: &u64) {
        self.clients.remove(client_id);
    }

    fn is_down(&self) -> bool {
        self.shutdown.is_canceled()
    }
}

#[derive(Debug)]
struct ClientInfo {
    /// The streamers the client is listening to.
    listening_to: HashSet<StreamerId>,
    /// The communication/stream-out channel to the client.
    // NOTE: Here we are using `tokio`'s `mpsc` because the one in `futures` have some extra feature
    // (ref: https://users.rust-lang.org/t/why-does-try-send-from-crate-futures-require-mut-self/100389).
    // This feature is aimed towards the multi-producer case (which we don't use) and requires a mutable
    // reference on `try_send` calls. This will require us to put the channel in a mutex and degrade the
    // broadcasting performance.
    channel: mpsc::Sender<Arc<Event>>,
}

impl ClientInfo {
    fn new(channel: mpsc::Sender<Arc<Event>>) -> Self {
        Self {
            listening_to: HashSet::new(),
            channel,
        }
    }

    fn add_streamer(&mut self, streamer_id: StreamerId) {
        self.listening_to.insert(streamer_id);
    }

    fn remove_streamer(&mut self, streamer_id: &StreamerId) {
        self.listening_to.remove(streamer_id);
    }

    fn listens_to(&self, streamer_id: &StreamerId) -> bool {
        self.listening_to.contains(streamer_id)
    }

    fn send_event(&self, event: Arc<Event>) {
        // Only `try_send` here. If the channel is full (client is slow), the message
        // will be dropped and the client won't receive it.
        // This avoids blocking the broadcast to other receivers.
        self.channel.try_send(event).error_log();
    }
}

#[derive(Default, Debug)]
struct StreamingManagerInner {
    /// A map from streamer IDs to their communication channels (if present) and shutdown handles.
    streamers: HashMap<StreamerId, StreamerInfo>,
    /// An inverse map from client IDs to the streamers they are listening to and the communication channel with the client.
    clients: HashMap<u64, ClientInfo>,
}

#[derive(Clone, Default, Debug)]
pub struct StreamingManager(Arc<RwLock<StreamingManagerInner>>);

impl StreamingManager {
    /// Returns a read guard over the streaming manager.
    fn read(&self) -> RwLockReadGuard<'_, StreamingManagerInner> {
        self.0.read()
    }

    /// Returns a write guard over the streaming manager.
    fn write(&self) -> RwLockWriteGuard<'_, StreamingManagerInner> {
        self.0.write()
    }

    /// Spawns and adds a new streamer `streamer` to the manager.
    pub async fn add(
        &self,
        client_id: u64,
        streamer: impl EventStreamer,
        spawner: WeakSpawner,
    ) -> Result<StreamerId, StreamingManagerError> {
        let streamer_id = streamer.streamer_id();
        // Remove the streamer if it died for some reason.
        self.remove_streamer_if_down(&streamer_id);

        // Pre-checks before spawning the streamer. Inside another scope to drop the lock early.
        {
            let mut this = self.write();
            match this.clients.get(&client_id) {
                // We don't know that client. We don't have a connection to it.
                None => return Err(StreamingManagerError::UnknownClient),
                // The client is already listening to that streamer.
                Some(client_info) if client_info.listens_to(&streamer_id) => {
                    return Err(StreamingManagerError::ClientAlreadyListening);
                },
                _ => (),
            }

            // If a streamer is already up and running, we won't spawn another one.
            if let Some(streamer_info) = this.streamers.get_mut(&streamer_id) {
                // Register the client as a listener to the streamer.
                streamer_info.add_client(client_id);
                // Register the streamer as listened-to by the client.
                if let Some(client_info) = this.clients.get_mut(&client_id) {
                    client_info.add_streamer(streamer_id.clone());
                }
                return Ok(streamer_id);
            }
        }

        // Spawn a new streamer.
        let (shutdown, data_in) = spawn(streamer, spawner, self.clone())
            .await
            .map_err(StreamingManagerError::SpawnError)?;
        let streamer_info = StreamerInfo::new(data_in, shutdown);

        // Note that we didn't hold the lock while spawning the streamer (potentially a long operation).
        // This means we can't assume either that the client still exists at this point or
        // that the streamer still doesn't exist.
        let mut this = self.write();
        if let Some(client_info) = this.clients.get_mut(&client_id) {
            client_info.add_streamer(streamer_id.clone());
            this.streamers
                .entry(streamer_id.clone())
                .or_insert(streamer_info)
                .add_client(client_id);
        } else {
            // The client was removed while we were spawning the streamer.
            // We no longer have a connection for it.
            return Err(StreamingManagerError::UnknownClient);
        }
        Ok(streamer_id)
    }

    /// Sends data to a streamer with `streamer_id`.
    pub fn send<T: Send + 'static>(&self, streamer_id: &StreamerId, data: T) -> Result<(), StreamingManagerError> {
        let this = self.read();
        let streamer_info = this
            .streamers
            .get(streamer_id)
            .ok_or(StreamingManagerError::StreamerNotFound)?;
        let data_in = streamer_info.data_in.as_ref().ok_or(StreamingManagerError::NoDataIn)?;
        data_in
            .unbounded_send(Box::new(data))
            .map_err(|e| StreamingManagerError::SendError(e.to_string()))
    }

    /// Same as `StreamingManager::send`, but computes that data to send to a streamer using a closure,
    /// thus avoiding computations & cloning if the intended streamer isn't running (more like the
    /// laziness of `*_or_else()` functions).
    ///
    /// `data_fn` will only be evaluated if the streamer is found and accepts an input.
    pub fn send_fn<T: Send + 'static>(
        &self,
        streamer_id: &StreamerId,
        data_fn: impl FnOnce() -> T,
    ) -> Result<(), StreamingManagerError> {
        let this = self.read();
        let streamer_info = this
            .streamers
            .get(streamer_id)
            .ok_or(StreamingManagerError::StreamerNotFound)?;
        let data_in = streamer_info.data_in.as_ref().ok_or(StreamingManagerError::NoDataIn)?;
        data_in
            .unbounded_send(Box::new(data_fn()))
            .map_err(|e| StreamingManagerError::SendError(e.to_string()))
    }

    /// Stops streaming from the streamer with `streamer_id` to the client with `client_id`.
    pub fn stop(&self, client_id: u64, streamer_id: &StreamerId) -> Result<(), StreamingManagerError> {
        let mut this = self.write();
        let client_info = this
            .clients
            .get_mut(&client_id)
            .ok_or(StreamingManagerError::UnknownClient)?;
        client_info.remove_streamer(streamer_id);

        this.streamers
            .get_mut(streamer_id)
            .ok_or(StreamingManagerError::StreamerNotFound)?
            .remove_client(&client_id);

        // If there are no more listening clients, terminate the streamer.
        if this.streamers.get(streamer_id).map(|info| info.clients.len()) == Some(0) {
            this.streamers.remove(streamer_id);
        }
        Ok(())
    }

    /// Broadcasts some event to clients listening to it.
    ///
    /// In contrast to `StreamingManager::send`, which sends some data to a streamer,
    /// this method broadcasts an event to the listening *clients* directly, independently
    /// of any streamer (i.e. bypassing any streamer).
    pub fn broadcast(&self, event: Event) {
        let event = Arc::new(event);
        let this = self.read();
        if let Some(client_ids) = this.streamers.get(event.origin()).map(|info| &info.clients) {
            client_ids.iter().for_each(|client_id| {
                if let Some(info) = this.clients.get(client_id) {
                    info.send_event(event.clone());
                }
            });
        };
    }

    /// Broadcasts (actually just *sends* in this case) some event to a specific client.
    ///
    /// Could be used in case we have a single known client and don't want to spawn up a streamer just for that.
    pub fn broadcast_to(&self, event: Event, client_id: u64) -> Result<(), StreamingManagerError> {
        let event = Arc::new(event);
        self.read()
            .clients
            .get(&client_id)
            .map(|info| info.send_event(event))
            .ok_or(StreamingManagerError::UnknownClient)
    }

    /// Forcefully broadcasts an event to all known clients even if they are not listening for such an event.
    pub fn broadcast_all(&self, event: Event) {
        let event = Arc::new(event);
        self.read().clients.values().for_each(|info| {
            info.send_event(event.clone());
        });
    }

    /// Creates a new client and returns the event receiver for this client.
    pub fn new_client(&self, client_id: u64) -> Result<ClientHandle, StreamingManagerError> {
        let mut this = self.write();
        if this.clients.contains_key(&client_id) {
            return Err(StreamingManagerError::ClientExists);
        }
        // Note that events queued in the channel are `Arc<` shared.
        // So a 1024 long buffer isn't actually heavy on memory.
        let (tx, rx) = mpsc::channel(1024);
        let client_info = ClientInfo::new(tx);
        this.clients.insert(client_id, client_info);
        let manager = self.clone();
        Ok(ClientHandle {
            rx,
            _on_drop_callback: OnDropCallback::new(move || {
                manager.remove_client(client_id).ok();
            }),
        })
    }

    /// Removes a client from the manager.
    pub fn remove_client(&self, client_id: u64) -> Result<(), StreamingManagerError> {
        let mut this = self.write();
        // Remove the client from our known-clients map.
        let client_info = this
            .clients
            .remove(&client_id)
            .ok_or(StreamingManagerError::UnknownClient)?;
        // Remove the client from all the streamers it was listening to.
        for streamer_id in client_info.listening_to {
            if let Some(streamer_info) = this.streamers.get_mut(&streamer_id) {
                streamer_info.remove_client(&client_id);
            } else {
                error!("Client {client_id} was listening to a non-existent streamer {streamer_id}. This is a bug!");
            }
            // If there are no more listening clients, terminate the streamer.
            if this.streamers.get(&streamer_id).map(|info| info.clients.len()) == Some(0) {
                this.streamers.remove(&streamer_id);
            }
        }
        Ok(())
    }

    /// Removes a streamer if it is no longer running.
    ///
    /// Aside from us shutting down a streamer when all its clients are disconnected,
    /// the streamer might die by itself (e.g. the spawner it was spawned with aborted).
    /// In this case, we need to remove the streamer and de-list it from all clients.
    fn remove_streamer_if_down(&self, streamer_id: &StreamerId) {
        let mut this = self.write();
        let Some(streamer_info) = this.streamers.get(streamer_id) else {
            return;
        };
        if !streamer_info.is_down() {
            return;
        }
        // Remove the streamer from our registry.
        let Some(streamer_info) = this.streamers.remove(streamer_id) else {
            return;
        };
        // And remove the streamer from all clients listening to it.
        for client_id in streamer_info.clients {
            if let Some(info) = this.clients.get_mut(&client_id) {
                info.remove_streamer(streamer_id);
            }
        }
    }
}

/// A handle that is returned on [`StreamingManager::new_client`] calls that will auto remove
/// the client when dropped.
/// So this handle must live as long as the client is connected.
pub struct ClientHandle {
    rx: mpsc::Receiver<Arc<Event>>,
    _on_drop_callback: OnDropCallback,
}

/// Deref the handle to the receiver inside for ease of use.
impl Deref for ClientHandle {
    type Target = mpsc::Receiver<Arc<Event>>;
    fn deref(&self) -> &Self::Target {
        &self.rx
    }
}

/// Also DerefMut since the receiver inside is mutated when consumed.
impl DerefMut for ClientHandle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.rx
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use crate::streamer::test_utils::{InitErrorStreamer, PeriodicStreamer, ReactiveStreamer};

    use common::executor::{abortable_queue::AbortableQueue, AbortableSystem, Timer};
    use common::{cfg_wasm32, cross_test};
    use serde_json::json;
    cfg_wasm32! {
        use wasm_bindgen_test::*;
        wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
    }

    cross_test!(test_add_remove_client, {
        let manager = StreamingManager::default();
        let client_id1 = 1;
        let client_id2 = 2;
        let client_id3 = 3;

        let c1_handle = manager.new_client(client_id1);
        assert!(matches!(c1_handle, Ok(..)));
        // Adding the same client again should fail.
        assert!(matches!(
            manager.new_client(client_id1),
            Err(StreamingManagerError::ClientExists)
        ));
        // Adding a different new client should be OK.
        let c2_handle = manager.new_client(client_id2);
        assert!(matches!(c2_handle, Ok(..)));

        assert!(matches!(manager.remove_client(client_id1), Ok(())));
        // Removing a removed client should fail.
        assert!(matches!(
            manager.remove_client(client_id1),
            Err(StreamingManagerError::UnknownClient)
        ));
        // Same as removing a non-existent client.
        assert!(matches!(
            manager.remove_client(client_id3),
            Err(StreamingManagerError::UnknownClient)
        ));
    });

    cross_test!(test_broadcast_all, {
        // Create a manager and add register two clients with it.
        let manager = StreamingManager::default();
        let mut client1 = manager.new_client(1).unwrap();
        let mut client2 = manager.new_client(2).unwrap();
        let event = Event::new(
            StreamerId::ForTesting {
                test_streamer: "test".to_string(),
            },
            json!("test"),
        );

        // Broadcast the event to all clients.
        manager.broadcast_all(event.clone());

        // The clients should receive the events.
        assert_eq!(*client1.try_recv().unwrap(), event);
        assert_eq!(*client2.try_recv().unwrap(), event);

        // Remove the clients.
        manager.remove_client(1).unwrap();
        manager.remove_client(2).unwrap();

        // `recv` shouldn't work at this point since the client is removed.
        assert!(client1.try_recv().is_err());
        assert!(client2.try_recv().is_err());
    });

    // https://github.com/KomodoPlatform/komodo-defi-framework/issues/1712#issuecomment-2669924113
    cross_test!(
        test_periodic_streamer,
        {
            let manager = StreamingManager::default();
            let system = AbortableQueue::default();
            let (client_id1, client_id2) = (1, 2);
            // Register a new client with the manager.
            let mut client1 = manager.new_client(client_id1).unwrap();
            // Another client whom we won't have it subscribe to the streamer.
            let mut client2 = manager.new_client(client_id2).unwrap();
            // Subscribe the new client to PeriodicStreamer.
            let streamer_id = manager
                .add(client_id1, PeriodicStreamer, system.weak_spawner())
                .await
                .unwrap();

            // We should be hooked now. try to receive some events from the streamer.
            for _ in 0..3 {
                // The streamer should send an event every 0.1s. Wait for 0.15s for safety.
                Timer::sleep(0.15).await;
                let event = client1.try_recv().unwrap();
                assert_eq!(event.origin(), &streamer_id);
            }

            // The other client shouldn't have received any events.
            assert!(client2.try_recv().is_err());
        },
        target_os = "linux",
        target_os = "windows"
    );

    cross_test!(test_reactive_streamer, {
        let manager = StreamingManager::default();
        let system = AbortableQueue::default();
        let (client_id1, client_id2) = (1, 2);
        // Register a new client with the manager.
        let mut client1 = manager.new_client(client_id1).unwrap();
        // Another client whom we won't have it subscribe to the streamer.
        let mut client2 = manager.new_client(client_id2).unwrap();
        // Subscribe the new client to ReactiveStreamer.
        let streamer_id = manager
            .add(client_id1, ReactiveStreamer, system.weak_spawner())
            .await
            .unwrap();

        // We should be hooked now. try to receive some events from the streamer.
        for i in 1..=3 {
            let msg = format!("send{i}");
            manager.send(&streamer_id, msg.clone()).unwrap();
            // Wait for a little bit to make sure the streamer received the data we sent.
            Timer::sleep(0.1).await;
            // The streamer should broadcast some event to the subscribed clients.
            let event = client1.try_recv().unwrap();
            assert_eq!(event.origin(), &streamer_id);
            // It's an echo streamer, so the message should be the same.
            assert_eq!(event.get().1, &json!(msg));
        }

        // If we send the wrong datatype (void here instead of String), the streamer should ignore it.
        manager.send(&streamer_id, ()).unwrap();
        Timer::sleep(0.1).await;
        assert!(client1.try_recv().is_err());

        // The other client shouldn't have received any events.
        assert!(client2.try_recv().is_err());
    });

    cross_test!(test_erroring_streamer, {
        let manager = StreamingManager::default();
        let system = AbortableQueue::default();
        let client_id = 1;
        // Register a new client with the manager.
        let _client = manager.new_client(client_id).unwrap();
        // Subscribe the new client to InitErrorStreamer.
        let error = manager
            .add(client_id, InitErrorStreamer, system.weak_spawner())
            .await
            .unwrap_err();

        assert!(matches!(error, StreamingManagerError::SpawnError(..)));
    });

    cross_test!(test_remove_streamer_if_down, {
        let manager = StreamingManager::default();
        let system = AbortableQueue::default();
        let client_id = 1;
        // Register a new client with the manager.
        let _client = manager.new_client(client_id).unwrap();
        // Subscribe the new client to PeriodicStreamer.
        let streamer_id = manager
            .add(client_id, PeriodicStreamer, system.weak_spawner())
            .await
            .unwrap();

        // The streamer is up and streaming to `client_id`.
        assert!(manager
            .0
            .read()
            .streamers
            .get(&streamer_id)
            .unwrap()
            .clients
            .contains(&client_id));

        // The client should be registered and listening to `streamer_id`.
        assert!(manager
            .0
            .read()
            .clients
            .get(&client_id)
            .unwrap()
            .listens_to(&streamer_id));

        // Abort the system to kill the streamer.
        system.abort_all().unwrap();
        // Wait a little bit since the abortion doesn't take effect immediately (the aborted task needs to yield first).
        Timer::sleep(0.1).await;

        manager.remove_streamer_if_down(&streamer_id);

        // The streamer should be removed.
        assert!(!manager.read().streamers.contains_key(&streamer_id));
        // And the client is no more listening to it.
        assert!(!manager
            .0
            .read()
            .clients
            .get(&client_id)
            .unwrap()
            .listens_to(&streamer_id));
    });
}
