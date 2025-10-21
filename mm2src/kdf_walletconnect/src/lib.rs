pub mod chain;
mod connection_handler;
#[allow(unused)]
pub mod error;
pub mod inbound_message;
mod metadata;
#[allow(unused)]
mod pairing;
pub mod session;
mod storage;

// Re-export `Topic` as it is used within KDF to identify which sessions a coin is running on.
pub use relay_rpc::domain::Topic as WcTopic;

use crate::connection_handler::{Handler, MAX_BACKOFF};
use crate::session::rpc::propose::send_session_proposal_request;
use chain::{WcChainId, WcRequestMethods, SUPPORTED_PROTOCOL};
use common::custom_futures::timeout::FutureTimerExt;
use common::executor::abortable_queue::AbortableQueue;
use common::executor::{AbortableSystem, SpawnFuture, Timer};
use common::log::{debug, error, info, LogOnError};
use error::WalletConnectError;
use futures::channel::mpsc::{unbounded, UnboundedReceiver};
use futures::StreamExt;
use inbound_message::{process_inbound_request, process_inbound_response, SessionMessageType};
use metadata::{generate_metadata, AUTH_TOKEN_DURATION, AUTH_TOKEN_SUB, PROJECT_ID, RELAY_ADDRESS};
use mm2_core::mm_ctx::{from_ctx, MmArc};
use mm2_err_handle::prelude::*;
use pairing_api::PairingClient;
use relay_client::websocket::{connection_event_loop as client_event_loop, Client, PublishedMessage};
use relay_client::{ConnectionOptions, MessageIdGenerator};
use relay_rpc::auth::{ed25519_dalek::SigningKey, AuthToken};
use relay_rpc::domain::{MessageId, Topic};
use relay_rpc::rpc::params::session::{Namespace, ProposeNamespaces};
use relay_rpc::rpc::params::session_request::SessionRequestRequest;
use relay_rpc::rpc::params::{
    session_request::Request as SessionRequest, IrnMetadata, Metadata, Relay, RelayProtocolMetadata, RequestParams,
    ResponseParamsError, ResponseParamsSuccess,
};
use relay_rpc::rpc::{ErrorResponse, Payload, Request, Response, SuccessfulResponse};
use serde::de::DeserializeOwned;
use session::rpc::delete::send_session_delete_request;
use session::{key::SymKeyPair, SessionManager};
use session::{EncodingAlgo, Session, SessionProperties, FIVE_MINUTES};
use std::collections::BTreeSet;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use storage::SessionStorageDb;
use storage::WalletConnectStorageOps;
use timed_map::TimedMap;
use tokio::sync::{oneshot, watch};
use wc_common::{decode_and_decrypt_type0, encrypt_and_encode, EnvelopeType, SymKey};

const PUBLISH_TIMEOUT_SECS: f64 = 6.;
const CONNECTION_TIMEOUT_S: f64 = 30.;

/// The necessary data to establish a new WalletConnect connection to a newly
/// established pairing by KDF (via [`WalletConnectCtxImpl::new_connection`]).
pub struct NewConnection {
    pub url: String,
    pub pairing_topic: Topic,
}

/// Broadcast by the lifecycle task so every RPC can cheaply await connectivity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
}

#[async_trait::async_trait]
pub trait WalletConnectOps {
    type Error;
    type Params<'a>;
    type SignTxData;
    type SendTxData;

    /// Unique chain_id associated with an activated/supported coin.
    async fn wc_chain_id(&self, ctx: &WalletConnectCtx) -> Result<WcChainId, Self::Error>;

    /// Send sign transaction request to WalletConnect Wallet.
    async fn wc_sign_tx<'a>(
        &self,
        wc: &WalletConnectCtx,
        params: Self::Params<'a>,
    ) -> Result<Self::SignTxData, Self::Error>;

    /// Send sign and send/broadcast transaction request to WalletConnect Wallet.
    async fn wc_send_tx<'a>(
        &self,
        wc: &WalletConnectCtx,
        params: Self::Params<'a>,
    ) -> Result<Self::SendTxData, Self::Error>;

    /// Session topic used to activate this.
    fn session_topic(&self) -> Result<&Topic, Self::Error>;
}

/// Implements the WalletConnect context, providing functionality for
/// establishing and managing wallet connections.
/// This struct contains the necessary state and methods to handle
/// wallet connection sessions, signing requests, and connection events.
pub struct WalletConnectCtxImpl {
    pub(crate) client: Client,
    pub(crate) pairing: PairingClient,
    pub(crate) key_pair: SymKeyPair,
    pub session_manager: SessionManager,
    relay: Relay,
    metadata: Metadata,
    message_id_generator: MessageIdGenerator,
    pending_requests: Mutex<TimedMap<MessageId, oneshot::Sender<SessionMessageType>>>,
    abortable_system: AbortableQueue,
    connection_state_rx: watch::Receiver<ConnectionState>,
}

/// A newtype wrapper around a thread-safe reference to `WalletConnectCtxImpl`.
/// Provides shared access to wallet connection functionality through an Arc pointer.
pub struct WalletConnectCtx(pub Arc<WalletConnectCtxImpl>);
impl Deref for WalletConnectCtx {
    type Target = WalletConnectCtxImpl;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl WalletConnectCtx {
    /// Attempt to initialize a new WalletConnect context.
    pub fn try_init(ctx: &MmArc) -> MmResult<Self, WalletConnectError> {
        let abortable_system = ctx
            .abortable_system
            .create_subsystem::<AbortableQueue>()
            .map_to_mm(|err| WalletConnectError::InternalError(err.to_string()))?;
        let storage = SessionStorageDb::new(ctx)?;
        let pairing = PairingClient::new();
        let relay = Relay {
            protocol: SUPPORTED_PROTOCOL.to_string(),
            data: None,
        };
        let (inbound_message_tx, inbound_message_rx) = unbounded();
        let (conn_live_sender, conn_live_receiver) = unbounded();
        let (connection_state_tx, connection_state_rx) = watch::channel(ConnectionState::Disconnected);
        let (client, _) = Client::new_with_callback(
            Handler::new("KDF", inbound_message_tx, conn_live_sender),
            |receiver, handler| {
                abortable_system
                    .weak_spawner()
                    .spawn(client_event_loop(receiver, handler))
            },
        );

        let message_id_generator = MessageIdGenerator::new();
        let context = Arc::new(WalletConnectCtxImpl {
            client,
            pairing,
            relay,
            metadata: generate_metadata(),
            key_pair: SymKeyPair::new(),
            session_manager: SessionManager::new(storage),
            pending_requests: Default::default(),
            message_id_generator,
            abortable_system,
            connection_state_rx,
        });

        // Spawn the relayer connection lifecycle task.
        context.abortable_system.weak_spawner().spawn(
            context
                .clone()
                .connection_lifecycle_task(conn_live_receiver, connection_state_tx),
        );

        // spawn message handler event loop
        context
            .abortable_system
            .weak_spawner()
            .spawn(context.clone().spawn_published_message_fut(inbound_message_rx));

        Ok(Self(context))
    }

    pub fn from_ctx(ctx: &MmArc) -> MmResult<Arc<WalletConnectCtx>, WalletConnectError> {
        from_ctx(&ctx.wallet_connect, move || {
            Self::try_init(ctx).map_err(|err| err.to_string())
        })
        .map_to_mm(WalletConnectError::InternalError)
    }
}

impl WalletConnectCtxImpl {
    /// Centralised task owning **all** connection logic (connect → monitor → reconnect).
    async fn connection_lifecycle_task(
        self: Arc<Self>,
        mut conn_status_rx: UnboundedReceiver<Option<String>>,
        connection_state_tx: watch::Sender<ConnectionState>,
    ) {
        if let Err(err) = self.session_manager.storage().init().await {
            error!("Failed to initialize WalletConnect storage, shutting down: {err:?}");
            connection_state_tx.send(ConnectionState::Disconnected).error_log();
            self.abortable_system.abort_all().error_log();
            return;
        }

        if let Err(err) = self.load_sessions_from_storage().await {
            error!("Failed to load sessions from storage, shutting down: {err:?}");
            connection_state_tx.send(ConnectionState::Disconnected).error_log();
            self.abortable_system.abort_all().error_log();
            return;
        }

        loop {
            connection_state_tx.send(ConnectionState::Connecting).error_log();
            info!("WalletConnect: connecting…");

            let mut backoff = 1;
            while let Err(e) = self.connect_and_subscribe().await {
                error!("Connection attempt failed: {e:?}; retrying in {backoff}s");
                Timer::sleep(backoff as f64).await;
                backoff = std::cmp::min(backoff * 2, MAX_BACKOFF);
            }

            connection_state_tx.send(ConnectionState::Connected).error_log();
            info!("WalletConnect: online.");

            if let Some(msg) = conn_status_rx.next().await {
                info!("WalletConnect: disconnected with message: {msg:?}, will reconnect.");
                connection_state_tx.send(ConnectionState::Disconnected).error_log();
            } else {
                connection_state_tx.send(ConnectionState::Disconnected).error_log();
                self.abortable_system.abort_all().error_log();
                break;
            }
        }
    }

    /// Waits until the current state is `Connected`.
    async fn await_connection(&self) -> MmResult<(), WalletConnectError> {
        let mut rx = self.connection_state_rx.clone();

        let wait_for_connected = async move {
            loop {
                if *rx.borrow() == ConnectionState::Connected {
                    return Ok(());
                }

                if rx.changed().await.is_err() {
                    let last_state = *rx.borrow();
                    return MmError::err(WalletConnectError::InternalError(format!(
                        "Connection task dropped, last state was: {last_state:?}"
                    )));
                }
            }
        };

        Box::pin(wait_for_connected)
            .timeout_secs(CONNECTION_TIMEOUT_S)
            .await
            .map_to_mm(|_timeout_err| WalletConnectError::TimeoutError)?
    }

    /// Attempt to connect to a wallet connection relay server.
    pub async fn connect_client(&self) -> MmResult<(), WalletConnectError> {
        let auth = {
            let key = SigningKey::generate(&mut rand::thread_rng());
            AuthToken::new(AUTH_TOKEN_SUB)
                .aud(RELAY_ADDRESS)
                .ttl(AUTH_TOKEN_DURATION)
                .as_jwt(&key)
                .map_to_mm(|err| WalletConnectError::InternalError(err.to_string()))?
        };
        let opts = ConnectionOptions::new(PROJECT_ID, auth).with_address(RELAY_ADDRESS);
        self.client.connect(&opts).await?;

        Ok(())
    }

    /// Connects to WalletConnect relayer and re-subscribes to previously active session topics if it's a reconnection.
    pub(crate) async fn connect_and_subscribe(&self) -> MmResult<(), WalletConnectError> {
        self.connect_client().await?;
        let sessions = self
            .session_manager
            .get_sessions()
            .flat_map(|s| vec![s.topic, s.pairing_topic])
            .collect::<Vec<_>>();

        if !sessions.is_empty() {
            self.client.batch_subscribe(sessions).await?;
        }

        Ok(())
    }

    /// Create a WalletConnect pairing connection url.
    pub async fn new_connection(
        &self,
        required_namespaces: serde_json::Value,
        optional_namespaces: Option<serde_json::Value>,
    ) -> MmResult<NewConnection, WalletConnectError> {
        self.await_connection().await?;

        let required_namespaces = serde_json::from_value(required_namespaces)?;
        let optional_namespaces = match optional_namespaces {
            Some(value) => serde_json::from_value(value)?,
            None => ProposeNamespaces::default(),
        };
        let (topic, url) = self.pairing.create(self.metadata.clone(), None)?;

        info!("[{topic}] Subscribing to topic");

        self.client
            .subscribe(topic.clone())
            .timeout_secs(PUBLISH_TIMEOUT_SECS)
            .await
            .map_to_mm(|_| WalletConnectError::TimeoutError)?
            .map_to_mm(|e| e.into())?;

        info!("[{topic}] Subscribed to topic");
        // Note that the creation of pairing doesn't have to do anything with the session proposal but we choose
        // to do them on one go.
        // TODO: We probably want to separate creating the pairing (done above) and then using the pairing
        //       to propose a session into two separate steps/functions. This aligns more with WalletConnect spec
        //       here and is easier to follow (have a clear boundary between a pairing and sessions instantiated using it).
        //       ref. https://specs.walletconnect.com/2.0/specs/clients/sign#context
        send_session_proposal_request(self, &topic, required_namespaces, optional_namespaces).await?;

        Ok(NewConnection {
            url,
            pairing_topic: topic,
        })
    }

    /// Get symmetric key associated with a for `topic`.
    fn sym_key(&self, topic: &Topic) -> MmResult<SymKey, WalletConnectError> {
        self.session_manager
            .sym_key(topic)
            .or_else(|| self.pairing.sym_key(topic).ok())
            .ok_or_else(|| {
                error!("Failed to find sym_key for topic: {topic}");
                MmError::new(WalletConnectError::InternalError(format!(
                    "topic sym_key not found: {topic}"
                )))
            })
    }

    /// Handles an inbound published message by decrypting, decoding, and processing it.
    async fn handle_published_message(&self, msg: PublishedMessage) -> MmResult<(), WalletConnectError> {
        let message = {
            let key = self.sym_key(&msg.topic)?;
            decode_and_decrypt_type0(msg.message.as_bytes(), &key)?
        };

        info!("[{}] Inbound message payload={message}", msg.topic);

        match serde_json::from_str(&message)? {
            Payload::Request(request) => process_inbound_request(self, request, &msg.topic).await?,
            Payload::Response(response) => process_inbound_response(self, response, &msg.topic).await,
        }

        debug!("[{}] Inbound message was handled successfully", msg.topic);

        Ok(())
    }

    /// Spawns a task that continuously processes published messages from inbound message channel.
    async fn spawn_published_message_fut(self: Arc<Self>, mut recv: UnboundedReceiver<PublishedMessage>) {
        while let Some(msg) = recv.next().await {
            self.handle_published_message(msg)
                .await
                .error_log_with_msg("Error processing message");
        }
    }

    /// Loads sessions from storage, activates valid ones, and deletes expired.
    async fn load_sessions_from_storage(&self) -> MmResult<(), WalletConnectError> {
        info!("Loading WalletConnect session from storage");
        let now = chrono::Utc::now().timestamp() as u64;
        let sessions = self
            .session_manager
            .storage()
            .get_all_sessions()
            .await
            .mm_err(|err| WalletConnectError::StorageError(err.to_string()))?;

        // bring most recent active session to the back.
        for session in sessions.into_iter().rev() {
            // delete expired session
            if now > session.expiry {
                debug!("Session {} expired, trying to delete from storage", session.topic);
                self.session_manager
                    .storage()
                    .delete_session(&session.topic)
                    .await
                    .error_log_with_msg(&format!("[{}] Unable to delete session from storage", session.topic));
                continue;
            };

            debug!("[{}] Session found! activating", session.topic);
            self.session_manager.add_session(session);
        }

        info!("Loaded WalletConnect session from storage");

        Ok(())
    }

    pub fn encode<T: AsRef<[u8]>>(&self, session_topic: &Topic, data: T) -> String {
        let algo = self
            .session_manager
            .get_session(session_topic)
            .map(|session| session.encoding_algo.unwrap_or(EncodingAlgo::Hex))
            .unwrap_or(EncodingAlgo::Hex);

        algo.encode(data)
    }

    /// Private function to publish a WC request.
    pub(crate) async fn publish_request(
        &self,
        topic: &Topic,
        param: RequestParams,
    ) -> MmResult<(oneshot::Receiver<SessionMessageType>, Duration), WalletConnectError> {
        let irn_metadata = param.irn_metadata();
        let ttl = irn_metadata.ttl;
        let message_id = self.message_id_generator.next();
        let request = Request::new(message_id, param.into());

        self.publish_payload(topic, irn_metadata, Payload::Request(request))
            .await?;

        let (tx, rx) = oneshot::channel();
        // insert request to map with a reasonable expiration time of 5 minutes
        self.pending_requests
            .lock()
            .unwrap()
            .insert_expirable(message_id, tx, Duration::from_secs(FIVE_MINUTES));

        Ok((rx, Duration::from_secs(ttl)))
    }

    /// Private function to publish a success WC request response.
    pub(crate) async fn publish_response_ok(
        &self,
        topic: &Topic,
        result: ResponseParamsSuccess,
        message_id: &MessageId,
    ) -> MmResult<(), WalletConnectError> {
        let irn_metadata = result.irn_metadata();
        let value = serde_json::to_value(result)?;
        let response = Response::Success(SuccessfulResponse::new(*message_id, value));

        self.publish_payload(topic, irn_metadata, Payload::Response(response))
            .await
    }

    /// Private function to publish an error WC request response.
    pub(crate) async fn publish_response_err(
        &self,
        topic: &Topic,
        error_data: ResponseParamsError,
        message_id: &MessageId,
    ) -> MmResult<(), WalletConnectError> {
        let error = error_data.error();
        let irn_metadata = error_data.irn_metadata();
        let response = Response::Error(ErrorResponse::new(*message_id, error));

        self.publish_payload(topic, irn_metadata, Payload::Response(response))
            .await
    }

    /// Private function to publish a WC payload.
    pub(crate) async fn publish_payload(
        &self,
        topic: &Topic,
        irn_metadata: IrnMetadata,
        payload: Payload,
    ) -> MmResult<(), WalletConnectError> {
        self.await_connection().await?;

        info!("[{topic}] Publishing message={payload:?}");
        let message = {
            let sym_key = self.sym_key(topic)?;
            let payload = serde_json::to_string(&payload)?;
            encrypt_and_encode(EnvelopeType::Type0, payload, &sym_key)?
        };

        self.client
            .publish(
                topic.clone(),
                &*message,
                None,
                irn_metadata.tag,
                Duration::from_secs(irn_metadata.ttl),
                irn_metadata.prompt,
            )
            .timeout_secs(PUBLISH_TIMEOUT_SECS)
            .await
            .map_to_mm(|_| WalletConnectError::TimeoutError)?
            .map_to_mm(|e| e.into())?;

        info!("[{topic}] Message published successfully");
        Ok(())
    }

    /// Checks if the current session is connected to a Ledger device.
    /// NOTE: for COSMOS chains only.
    pub fn is_ledger_connection(&self, session_topic: &Topic) -> bool {
        self.session_manager
            .get_session(session_topic)
            .and_then(|session| session.session_properties)
            .and_then(|props| props.keys.as_ref().cloned())
            // TODO: This is flaky. ref. https://github.com/KomodoPlatform/komodo-defi-framework/pull/2499#discussion_r2174531817
            .and_then(|keys| keys.first().cloned())
            .map(|key| key.is_nano_ledger)
            .unwrap_or(false)
    }

    /// Checks if the current session is connected via Keplr wallet.
    /// NOTE: for COSMOS chains only.
    pub fn is_keplr_connection(&self, session_topic: &Topic) -> bool {
        self.session_manager
            .get_session(session_topic)
            .map(|session| session.controller.metadata.name == "Keplr")
            .unwrap_or_default()
    }

    /// Checks if a given chain ID is supported.
    pub(crate) fn validate_chain_id(
        &self,
        session: &Session,
        chain_id: &WcChainId,
    ) -> MmResult<(), WalletConnectError> {
        if let Some(Namespace { chains, .. }) = session.namespaces.get(chain_id.chain.as_ref()) {
            match chains {
                Some(chains) => {
                    if chains.contains(&chain_id.to_string()) {
                        return Ok(());
                    }
                },
                None => {
                    // TODO: Please re-check the correctness of this logic. This doesn't seem to be part of the spec. And the link provided
                    //       doesn't have anything to do with sessionProperties.
                    // https://specs.walletconnect.com/2.0/specs/clients/sign/namespaces#13-chains-might-be-omitted-if-the-caip-2-is-defined-in-the-index
                    if let Some(SessionProperties { keys: Some(keys) }) = &session.session_properties {
                        if keys.iter().any(|k| k.chain_id == chain_id.id) {
                            return Ok(());
                        }
                    }
                },
            };
        }

        // https://specs.walletconnect.com/2.0/specs/clients/sign/namespaces#13-chains-might-be-omitted-if-the-caip-2-is-defined-in-the-index
        if session.namespaces.contains_key(&chain_id.to_string()) {
            return Ok(());
        }

        MmError::err(WalletConnectError::ChainIdNotSupported(chain_id.to_string()))
    }

    /// Validate and send update active chain to WC if needed.
    pub async fn validate_update_active_chain_id(
        &self,
        session_topic: &Topic,
        chain_id: &WcChainId,
    ) -> MmResult<(), WalletConnectError> {
        let session =
            self.session_manager
                .get_session(session_topic)
                .ok_or(MmError::new(WalletConnectError::SessionError(
                    "No active WalletConnect session found".to_string(),
                )))?;

        self.validate_chain_id(&session, chain_id)?;

        // TODO: uncomment when WalletConnect wallets start listening to chainChanged event
        // if WcChain::Eip155 != chain_id.chain {
        //     return Ok(());
        // };
        //
        // if let Some(active_chain_id) = session.get_active_chain_id().await {
        //     if chain_id == active_chain_id {
        //         return Ok(());
        //     }
        // };
        //
        // let event = SessionEventRequest {
        //     event: Event {
        //         name: "chainChanged".to_string(),
        //         data: serde_json::to_value(&chain_id.id)?,
        //     },
        //     chain_id: chain_id.to_string(),
        // };
        // self.publish_request(&session.topic, RequestParams::SessionEvent(event))
        //     .await?;
        //
        // let wait_duration = Duration::from_secs(60);
        // if let Ok(Some(resp)) = self.message_rx.lock().await.next().timeout(wait_duration).await {
        //     let result = resp.mm_err(WalletConnectError::InternalError)?;
        //     if let ResponseParamsSuccess::SessionEvent(data) = result.data {
        //         if !data {
        //             return MmError::err(WalletConnectError::PayloadError(
        //                 "Please approve chain id change".to_owned(),
        //             ));
        //         }
        //
        //         self.session
        //             .get_session_mut(&session.topic)
        //             .ok_or(MmError::new(WalletConnectError::SessionError(
        //                 "No active WalletConnect session found".to_string(),
        //             )))?
        //             .set_active_chain_id(chain_id.clone())
        //             .await;
        //     }
        // }

        Ok(())
    }

    /// Get available account for a given chain ID.
    pub fn get_account_and_properties_for_chain_id(
        &self,
        session_topic: &Topic,
        chain_id: &WcChainId,
    ) -> MmResult<(String, Option<SessionProperties>), WalletConnectError> {
        let session =
            self.session_manager
                .get_session(session_topic)
                .ok_or(MmError::new(WalletConnectError::SessionError(
                    "No active WalletConnect session found".to_string(),
                )))?;

        if let Some(Namespace {
            accounts: Some(accounts),
            ..
        }) = &session.namespaces.get(chain_id.chain.as_ref())
        {
            if let Some(account) = find_account_in_namespace(accounts, &chain_id.id) {
                return Ok((account, session.session_properties));
            }
        };

        MmError::err(WalletConnectError::NoAccountFound(chain_id.to_string()))
    }

    /// Waits for and handles a WalletConnect session response with arbitrary data.
    /// https://specs.walletconnect.com/2.0/specs/clients/sign/session-events#session_request
    pub async fn send_session_request_and_wait<R>(
        &self,
        session_topic: &Topic,
        chain_id: &WcChainId,
        method: WcRequestMethods,
        params: serde_json::Value,
    ) -> MmResult<R, WalletConnectError>
    where
        R: DeserializeOwned,
    {
        self.session_manager.validate_session_exists(session_topic)?;

        let request = SessionRequestRequest {
            chain_id: chain_id.to_string(),
            request: SessionRequest {
                method: method.as_ref().to_string(),
                expiry: None,
                params,
            },
        };
        let (rx, ttl) = self
            .publish_request(session_topic, RequestParams::SessionRequest(request))
            .await?;

        let response = rx
            .timeout(ttl)
            .await
            .map_to_mm(|_| WalletConnectError::TimeoutError)?
            .map_to_mm(|err| WalletConnectError::InternalError(err.to_string()))??;
        match response.data {
            ResponseParamsSuccess::Arbitrary(data) => Ok(serde_json::from_value::<R>(data)?),
            _ => MmError::err(WalletConnectError::PayloadError("Unexpected response type".to_string())),
        }
    }

    // Destroy WC session.
    pub async fn drop_session(&self, topic: &Topic) -> MmResult<(), WalletConnectError> {
        send_session_delete_request(self, topic).await
    }
}

fn find_account_in_namespace<'a>(accounts: &'a BTreeSet<String>, chain_id: &'a str) -> Option<String> {
    accounts.iter().find_map(move |account_name| {
        let parts: Vec<&str> = account_name.split(':').collect();
        if parts.len() >= 3 && parts[1] == chain_id {
            Some(parts[2].to_string())
        } else {
            None
        }
    })
}
