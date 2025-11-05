use crate::context::CoinsActivationContext;
use crate::l2::{
    InitL2ActivationOps, InitL2Error, InitL2InitialStatus, InitL2TaskHandleShared, InitL2TaskManagerShared,
    L2ProtocolParams,
};
use crate::prelude::*;
use async_trait::async_trait;
use coins::coin_errors::MyAddressError;
use coins::lightning::ln_conf::{LightningCoinConf, LightningProtocolConf};
use coins::lightning::ln_errors::{EnableLightningError, EnableLightningResult};
use coins::lightning::ln_events::{init_abortable_events, LightningEventHandler};
use coins::lightning::ln_p2p::{connect_to_ln_nodes_loop, init_peer_manager, ln_node_announcement_loop};
use coins::lightning::ln_platform::Platform;
use coins::lightning::ln_storage::LightningStorage;
use coins::lightning::ln_utils::{
    get_open_channels_nodes_addresses, init_channel_manager, init_db, init_keys_manager, init_persister,
    PAYMENT_RETRY_ATTEMPTS,
};
use coins::lightning::{InvoicePayer, LightningCoin};
use coins::utxo::utxo_standard::UtxoStandardCoin;
use coins::utxo::UtxoCommonOps;
use coins::{BalanceError, CoinBalance, CoinProtocol, MarketCoinOps, MmCoinEnum, RegisterCoinError};
use common::executor::{SpawnFuture, Timer};
use crypto::hw_rpc_task::{HwRpcTaskAwaitingStatus, HwRpcTaskUserAction};
use derive_more::Display;
use futures::compat::Future01CompatExt;
use lightning::chain::keysinterface::{KeysInterface, Recipient};
use lightning::chain::Access;
use lightning::routing::gossip;
use lightning::routing::router::DefaultRouter;
use lightning_background_processor::{BackgroundProcessor, GossipSync};
use lightning_invoice::payment;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use parking_lot::Mutex as PaMutex;
use secp256k1::Secp256k1;
use ser_error_derive::SerializeErrorType;
use serde_derive::{Deserialize, Serialize};
use serde_json::{self as json, Value as Json};
use std::sync::Arc;

const DEFAULT_LISTENING_PORT: u16 = 9735;

pub type LightningTaskManagerShared = InitL2TaskManagerShared<LightningCoin>;
pub type LightningRpcTaskHandleShared = InitL2TaskHandleShared<LightningCoin>;
pub type LightningAwaitingStatus = HwRpcTaskAwaitingStatus;
pub type LightningUserAction = HwRpcTaskUserAction;

#[derive(Clone, PartialEq, Serialize)]
pub enum LightningInProgressStatus {
    ActivatingCoin,
    GettingFeesFromRPC,
    ReadingNetworkGraphFromFile,
    InitializingChannelManager,
    InitializingPeerManager,
    ReadingScorerFromFile,
    InitializingBackgroundProcessor,
    ReadingChannelsAddressesFromFile,
    Finished,
    /// This status doesn't require the user to send `UserAction`,
    /// but it tells the user that he should confirm/decline an address on his device.
    WaitingForTrezorToConnect,
    WaitingForUserToConfirmPubkey,
}

impl InitL2InitialStatus for LightningInProgressStatus {
    fn initial_status() -> Self {
        LightningInProgressStatus::ActivatingCoin
    }
}

impl TryPlatformCoinFromMmCoinEnum for UtxoStandardCoin {
    fn try_from_mm_coin(coin: MmCoinEnum) -> Option<Self>
    where
        Self: Sized,
    {
        match coin {
            MmCoinEnum::UtxoCoin(coin) => Some(coin),
            _ => None,
        }
    }
}

impl TryFromCoinProtocol for LightningProtocolConf {
    fn try_from_coin_protocol(proto: CoinProtocol) -> Result<Self, MmError<CoinProtocol>>
    where
        Self: Sized,
    {
        match proto {
            CoinProtocol::LIGHTNING {
                platform,
                network,
                confirmation_targets,
            } => Ok(LightningProtocolConf {
                platform_coin_ticker: platform,
                network,
                confirmation_targets,
            }),
            proto => MmError::err(proto),
        }
    }
}

impl L2ProtocolParams for LightningProtocolConf {
    fn platform_coin_ticker(&self) -> &str {
        &self.platform_coin_ticker
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct LightningActivationParams {
    // The listening port for the p2p LN node
    pub listening_port: Option<u16>,
    // Printable human-readable string to describe this node to other users.
    pub name: String,
    // Node's HEX color. This is used for showing the node in a network graph with the desired color.
    pub color: Option<String>,
    // The number of payment retries that should be done before considering a payment failed or partially failed.
    pub payment_retries: Option<usize>,
    // Node's backup path for channels and other data that requires backup.
    pub backup_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LightningValidatedParams {
    // The listening port for the p2p LN node
    pub listening_port: u16,
    // Printable human-readable string to describe this node to other users.
    pub node_name: [u8; 32],
    // Node's RGB color. This is used for showing the node in a network graph with the desired color.
    pub node_color: [u8; 3],
    // Invoice Payer is initialized while starting the lightning node, and it requires the number of payment retries that
    // it should do before considering a payment failed or partially failed. If not provided the number of retries will be 5
    // as this is a good default value.
    pub payment_retries: Option<usize>,
    // Node's backup path for channels and other data that requires backup.
    pub backup_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum LightningValidationErr {
    #[display(fmt = "Platform coin {_0} activated in {_1} mode")]
    UnexpectedMethod(String, String),
    #[display(fmt = "{_0} is only supported in {_1} mode")]
    UnsupportedMode(String, String),
    #[display(fmt = "Invalid request: {_0}")]
    InvalidRequest(String),
    #[display(fmt = "Invalid address: {_0}")]
    InvalidAddress(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct LightningActivationResult {
    platform_coin: String,
    address: String,
    balance: CoinBalance,
}

#[derive(Clone, Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum LightningInitError {
    CoinIsAlreadyActivated {
        ticker: String,
    },
    InvalidConfiguration(String),
    #[display(fmt = "Error while validating {platform_coin_ticker} configuration: {err}")]
    InvalidPlatformConfiguration {
        platform_coin_ticker: String,
        err: String,
    },
    EnableLightningError(EnableLightningError),
    LightningValidationErr(LightningValidationErr),
    MyBalanceError(BalanceError),
    MyAddressError(String),
    Internal(String),
}

impl From<MyAddressError> for LightningInitError {
    fn from(err: MyAddressError) -> Self {
        Self::MyAddressError(err.to_string())
    }
}

impl From<LightningInitError> for InitL2Error {
    fn from(err: LightningInitError) -> Self {
        match err {
            LightningInitError::CoinIsAlreadyActivated { ticker } => InitL2Error::L2IsAlreadyActivated(ticker),
            LightningInitError::InvalidConfiguration(err) => InitL2Error::L2ConfigParseError(err),
            LightningInitError::InvalidPlatformConfiguration {
                platform_coin_ticker,
                err,
            } => InitL2Error::InvalidPlatformConfiguration {
                platform_coin_ticker,
                err,
            },
            LightningInitError::EnableLightningError(enable_err) => match enable_err {
                EnableLightningError::RpcError(rpc_err) => InitL2Error::Transport(rpc_err),
                enable_error => InitL2Error::Internal(enable_error.to_string()),
            },
            LightningInitError::LightningValidationErr(req_err) => InitL2Error::Internal(req_err.to_string()),
            LightningInitError::MyBalanceError(balance_err) => match balance_err {
                BalanceError::Transport(e) => InitL2Error::Transport(e),
                balance_error => InitL2Error::Internal(balance_error.to_string()),
            },
            LightningInitError::MyAddressError(e) => InitL2Error::Internal(e),
            LightningInitError::Internal(e) => InitL2Error::Internal(e),
        }
    }
}

impl From<EnableLightningError> for LightningInitError {
    fn from(err: EnableLightningError) -> Self {
        LightningInitError::EnableLightningError(err)
    }
}

impl From<LightningValidationErr> for LightningInitError {
    fn from(err: LightningValidationErr) -> Self {
        LightningInitError::LightningValidationErr(err)
    }
}

impl From<RegisterCoinError> for LightningInitError {
    fn from(reg_err: RegisterCoinError) -> LightningInitError {
        match reg_err {
            RegisterCoinError::CoinIsInitializedAlready { coin } => {
                LightningInitError::CoinIsAlreadyActivated { ticker: coin }
            },
            RegisterCoinError::Internal(internal) => LightningInitError::Internal(internal),
        }
    }
}

#[async_trait]
impl InitL2ActivationOps for LightningCoin {
    type PlatformCoin = UtxoStandardCoin;
    type ActivationParams = LightningActivationParams;
    type ProtocolInfo = LightningProtocolConf;
    type ValidatedParams = LightningValidatedParams;
    type CoinConf = LightningCoinConf;
    type ActivationResult = LightningActivationResult;
    type ActivationError = LightningInitError;
    type InProgressStatus = LightningInProgressStatus;
    type AwaitingStatus = LightningAwaitingStatus;
    type UserAction = LightningUserAction;

    fn rpc_task_manager(activation_ctx: &CoinsActivationContext) -> &LightningTaskManagerShared {
        &activation_ctx.init_lightning_task_manager
    }

    fn coin_conf_from_json(json: Json) -> Result<Self::CoinConf, MmError<Self::ActivationError>> {
        json::from_value::<LightningCoinConf>(json)
            .map_to_mm(|e| LightningInitError::InvalidConfiguration(e.to_string()))
    }

    fn validate_platform_configuration(
        platform_coin: &Self::PlatformCoin,
    ) -> Result<(), MmError<Self::ActivationError>> {
        // Channel funding transactions need to spend segwit outputs
        // and while the witness script can be generated from pubkey and be used
        // it's better for the coin to be enabled in segwit to check if balance is enough for funding transaction, etc...
        if !platform_coin.addr_format().is_segwit() {
            return MmError::err(
                LightningValidationErr::UnsupportedMode("Lightning network".into(), "segwit".into()).into(),
            );
        }
        if platform_coin.as_ref().conf.avg_blocktime.is_none() {
            return MmError::err(LightningInitError::InvalidPlatformConfiguration {
                platform_coin_ticker: platform_coin.ticker().to_string(),
                err: "'avg_blocktime' field is not found in platform coin config".into(),
            });
        }
        Ok(())
    }

    fn validate_activation_params(
        activation_params: Self::ActivationParams,
    ) -> Result<Self::ValidatedParams, MmError<Self::ActivationError>> {
        if activation_params.name.len() > 32 {
            return MmError::err(
                LightningValidationErr::InvalidRequest("Node name length can't be more than 32 characters".into())
                    .into(),
            );
        }
        let mut node_name = [b' '; 32];
        node_name[0..activation_params.name.len()].copy_from_slice(activation_params.name.as_bytes());

        let mut node_color = [0u8; 3];
        hex::decode_to_slice(
            activation_params.color.unwrap_or_else(|| "000000".into()),
            &mut node_color as &mut [u8],
        )
        .map_to_mm(|_| LightningValidationErr::InvalidRequest("Invalid Hex Color".into()))
        .map_mm_err()?;

        let listening_port = activation_params.listening_port.unwrap_or(DEFAULT_LISTENING_PORT);

        Ok(LightningValidatedParams {
            listening_port,
            node_name,
            node_color,
            payment_retries: activation_params.payment_retries,
            backup_path: activation_params.backup_path,
        })
    }

    async fn init_l2(
        ctx: &MmArc,
        platform_coin: Self::PlatformCoin,
        validated_params: Self::ValidatedParams,
        protocol_conf: Self::ProtocolInfo,
        coin_conf: Self::CoinConf,
        task_handle: LightningRpcTaskHandleShared,
    ) -> Result<(Self, Self::ActivationResult), MmError<Self::ActivationError>> {
        let lightning_coin = start_lightning(
            ctx,
            platform_coin.clone(),
            protocol_conf,
            coin_conf,
            validated_params,
            task_handle,
        )
        .await
        .map_mm_err()?;
        Timer::sleep(10.).await;

        let address = lightning_coin.my_address().map_mm_err()?;
        let balance = lightning_coin
            .my_balance()
            .compat()
            .await
            .mm_err(LightningInitError::MyBalanceError)?;
        let init_result = LightningActivationResult {
            platform_coin: platform_coin.ticker().into(),
            address,
            balance,
        };
        Ok((lightning_coin, init_result))
    }
}

async fn start_lightning(
    ctx: &MmArc,
    platform_coin: UtxoStandardCoin,
    protocol_conf: LightningProtocolConf,
    conf: LightningCoinConf,
    params: LightningValidatedParams,
    task_handle: LightningRpcTaskHandleShared,
) -> EnableLightningResult<LightningCoin> {
    // Todo: add support for Hardware wallets for funding transactions and spending spendable outputs (channel closing transactions)
    if let coins::DerivationMethod::HDWallet(_) = platform_coin.as_ref().derivation_method {
        return MmError::err(EnableLightningError::UnsupportedMode(
            "'start_lightning'".into(),
            "iguana".into(),
        ));
    }

    let platform = Arc::new(Platform::new(
        platform_coin.clone(),
        protocol_conf.network.clone(),
        protocol_conf.confirmation_targets,
    )?);
    task_handle
        .update_in_progress_status(LightningInProgressStatus::GettingFeesFromRPC)
        .map_mm_err()?;
    platform.set_latest_fees().await.map_mm_err()?;

    // Initialize the Logger
    let logger = ctx.log.0.clone();

    // Initialize the KeysManager
    let keys_manager = init_keys_manager(&platform)?;
    let node_id = keys_manager
        .get_node_secret(Recipient::Node)
        .map_err(|e| EnableLightningError::Internal(format!("Error while getting node id: {e:?}")))?
        .public_key(&Secp256k1::new());
    let node_id = node_id.to_string();

    // Initialize Persister
    let persister = init_persister(ctx, &node_id, conf.ticker.clone(), params.backup_path).await?;

    // Initialize the P2PGossipSync. This is used for providing routes to send payments over
    task_handle
        .update_in_progress_status(LightningInProgressStatus::ReadingNetworkGraphFromFile)
        .map_mm_err()?;
    let network_graph = Arc::new(
        persister
            .get_network_graph(protocol_conf.network.into(), logger.clone())
            .await?,
    );

    let gossip_sync = Arc::new(gossip::P2PGossipSync::new(
        network_graph.clone(),
        None::<Arc<dyn Access + Send + Sync>>,
        logger.clone(),
    ));

    // Initialize DB
    let db = init_db(ctx, &node_id, conf.ticker.clone()).await?;

    // Initialize the ChannelManager
    task_handle
        .update_in_progress_status(LightningInProgressStatus::InitializingChannelManager)
        .map_mm_err()?;
    let (chain_monitor, channel_manager) = init_channel_manager(
        platform.clone(),
        logger.clone(),
        persister.clone(),
        db.clone(),
        keys_manager.clone(),
        conf.clone().into(),
    )
    .await?;

    // Initialize the PeerManager
    task_handle
        .update_in_progress_status(LightningInProgressStatus::InitializingPeerManager)
        .map_mm_err()?;
    let peer_manager = init_peer_manager(
        ctx.clone(),
        &platform,
        params.listening_port,
        channel_manager.clone(),
        keys_manager.clone(),
        gossip_sync.clone(),
        logger.clone(),
    )
    .await?;

    let trusted_nodes = Arc::new(PaMutex::new(persister.get_trusted_nodes().await?));

    init_abortable_events(platform.clone(), db.clone()).await?;

    // Initialize the event handler
    let event_handler = Arc::new(LightningEventHandler::new(
        platform.clone(),
        channel_manager.clone(),
        keys_manager.clone(),
        db.clone(),
        trusted_nodes.clone(),
    ));

    // Initialize routing Scorer
    task_handle
        .update_in_progress_status(LightningInProgressStatus::ReadingScorerFromFile)
        .map_mm_err()?;
    // status_notifier
    //     .try_send(LightningInProgressStatus::ReadingScorerFromFile)
    //     .debug_log_with_msg("No one seems interested in LightningInProgressStatus");
    let scorer = Arc::new(persister.get_scorer(network_graph.clone(), logger.clone()).await?);

    // Create InvoicePayer
    // random_seed_bytes are additional random seed to improve privacy by adding a random CLTV expiry offset to each path's final hop.
    // This helps obscure the intended recipient from adversarial intermediate hops. The seed is also used to randomize candidate paths during route selection.
    // TODO: random_seed_bytes should be taken in consideration when implementing swaps because they change the payment lock-time.
    // https://github.com/lightningdevkit/rust-lightning/issues/158
    // https://github.com/lightningdevkit/rust-lightning/pull/1286
    // https://github.com/lightningdevkit/rust-lightning/pull/1359
    let router_random_seed_bytes = keys_manager.get_secure_random_bytes();
    let router = DefaultRouter::new(
        network_graph.clone(),
        logger.clone(),
        router_random_seed_bytes,
        scorer.clone(),
    );
    let invoice_payer = Arc::new(InvoicePayer::new(
        channel_manager.clone(),
        router,
        logger.clone(),
        event_handler,
        // Todo: Add option for choosing payment::Retry::Timeout instead of Attempts in LightningParams
        payment::Retry::Attempts(params.payment_retries.unwrap_or(PAYMENT_RETRY_ATTEMPTS)),
    ));

    // Start Background Processing. Runs tasks periodically in the background to keep LN node operational.
    // InvoicePayer will act as our event handler as it handles some of the payments related events before
    // delegating it to LightningEventHandler.
    // note: background_processor stops automatically when dropped since BackgroundProcessor implements the Drop trait.
    task_handle
        .update_in_progress_status(LightningInProgressStatus::InitializingBackgroundProcessor)
        .map_mm_err()?;
    let background_processor = Arc::new(BackgroundProcessor::start(
        persister.clone(),
        invoice_payer.clone(),
        chain_monitor.clone(),
        channel_manager.clone(),
        GossipSync::p2p(gossip_sync),
        peer_manager.clone(),
        logger.clone(),
        Some(scorer.clone()),
    ));

    // If channel_nodes_data file exists, read channels nodes data from disk and reconnect to channel nodes/peers if possible.
    task_handle
        .update_in_progress_status(LightningInProgressStatus::ReadingChannelsAddressesFromFile)
        .map_mm_err()?;
    let open_channels_nodes = Arc::new(PaMutex::new(
        get_open_channels_nodes_addresses(persister.clone(), channel_manager.clone()).await?,
    ));

    platform.spawner().spawn(connect_to_ln_nodes_loop(
        open_channels_nodes.clone(),
        peer_manager.clone(),
    ));

    // Broadcast Node Announcement
    platform.spawner().spawn(ln_node_announcement_loop(
        peer_manager.clone(),
        params.node_name,
        params.node_color,
        params.listening_port,
    ));

    Ok(LightningCoin {
        platform,
        conf,
        background_processor,
        peer_manager,
        channel_manager,
        chain_monitor,
        keys_manager,
        invoice_payer,
        persister,
        db,
        open_channels_nodes,
        trusted_nodes,
        router: Arc::new(DefaultRouter::new(
            network_graph,
            logger.clone(),
            router_random_seed_bytes,
            scorer,
        )),
        logger,
    })
}
