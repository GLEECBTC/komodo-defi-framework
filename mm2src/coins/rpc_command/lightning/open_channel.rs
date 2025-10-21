use crate::lightning::ln_conf::{ChannelOptions, OurChannelsConfigs};
use crate::lightning::ln_db::{DBChannelDetails, LightningDB};
use crate::lightning::ln_p2p::{connect_to_ln_node, ConnectionError};
use crate::lightning::ln_serialization::NodeAddress;
use crate::lightning::ln_storage::LightningStorage;
use crate::utxo::utxo_common::UtxoTxBuilder;
use crate::utxo::{sat_from_big_decimal, FeePolicy, GetUtxoListOps, UtxoTxGenerationOps};
use crate::{
    lp_coinfind_or_err, BalanceError, CoinFindError, GenerateTxError, MmCoinEnum, NumConversError,
    UnexpectedDerivationMethod, UtxoRpcError,
};
use chain::TransactionOutput;
use common::log::error;
use common::{async_blocking, new_uuid, HttpStatusCode};
use db_common::sqlite::rusqlite::Error as SqlError;
use derive_more::Display;
use http::StatusCode;
use keys::AddressHashEnum;
use lightning::util::config::UserConfig;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::BigDecimal;
use script::Builder;
use uuid::Uuid;

type OpenChannelResult<T> = Result<T, MmError<OpenChannelError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum OpenChannelError {
    #[display(fmt = "Lightning network is not supported for {_0}")]
    UnsupportedCoin(String),
    #[display(fmt = "Balance Error {_0}")]
    BalanceError(String),
    #[display(fmt = "Invalid path: {_0}")]
    InvalidPath(String),
    #[display(fmt = "Failure to open channel with node {_0}: {_1}")]
    FailureToOpenChannel(String, String),
    #[display(fmt = "RPC error {_0}")]
    RpcError(String),
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
    #[display(fmt = "I/O error {_0}")]
    IOError(String),
    #[display(fmt = "DB error {_0}")]
    DbError(String),
    ConnectToNodeError(String),
    #[display(fmt = "No such coin {_0}")]
    NoSuchCoin(String),
    #[display(fmt = "Generate Tx Error {_0}")]
    GenerateTxErr(String),
}

impl HttpStatusCode for OpenChannelError {
    fn status_code(&self) -> StatusCode {
        match self {
            OpenChannelError::UnsupportedCoin(_) | OpenChannelError::RpcError(_) => StatusCode::BAD_REQUEST,
            OpenChannelError::FailureToOpenChannel(_, _)
            | OpenChannelError::ConnectToNodeError(_)
            | OpenChannelError::InternalError(_)
            | OpenChannelError::GenerateTxErr(_)
            | OpenChannelError::IOError(_)
            | OpenChannelError::DbError(_)
            | OpenChannelError::InvalidPath(_)
            | OpenChannelError::BalanceError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OpenChannelError::NoSuchCoin(_) => StatusCode::NOT_FOUND,
        }
    }
}

impl From<ConnectionError> for OpenChannelError {
    fn from(err: ConnectionError) -> OpenChannelError {
        OpenChannelError::ConnectToNodeError(err.to_string())
    }
}

impl From<CoinFindError> for OpenChannelError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => OpenChannelError::NoSuchCoin(coin),
        }
    }
}

impl From<BalanceError> for OpenChannelError {
    fn from(e: BalanceError) -> Self {
        OpenChannelError::BalanceError(e.to_string())
    }
}

impl From<NumConversError> for OpenChannelError {
    fn from(e: NumConversError) -> Self {
        OpenChannelError::InternalError(e.to_string())
    }
}

impl From<GenerateTxError> for OpenChannelError {
    fn from(e: GenerateTxError) -> Self {
        OpenChannelError::GenerateTxErr(e.to_string())
    }
}

impl From<UtxoRpcError> for OpenChannelError {
    fn from(e: UtxoRpcError) -> Self {
        OpenChannelError::RpcError(e.to_string())
    }
}

impl From<UnexpectedDerivationMethod> for OpenChannelError {
    fn from(e: UnexpectedDerivationMethod) -> Self {
        OpenChannelError::InternalError(e.to_string())
    }
}

impl From<std::io::Error> for OpenChannelError {
    fn from(err: std::io::Error) -> OpenChannelError {
        OpenChannelError::IOError(err.to_string())
    }
}

impl From<SqlError> for OpenChannelError {
    fn from(err: SqlError) -> OpenChannelError {
        OpenChannelError::DbError(err.to_string())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value")]
pub enum ChannelOpenAmount {
    Exact(BigDecimal),
    Max,
}

#[derive(Deserialize)]
pub struct OpenChannelRequest {
    pub coin: String,
    pub node_address: NodeAddress,
    pub amount: ChannelOpenAmount,
    /// The amount to push to the counterparty as part of the open, in milli-satoshi. Creates inbound liquidity for the channel.
    /// By setting push_msat to a value, opening channel request will be equivalent to opening a channel then sending a payment with
    /// the push_msat amount.
    #[serde(default)]
    pub push_msat: u64,
    pub channel_options: Option<ChannelOptions>,
    pub channel_configs: Option<OurChannelsConfigs>,
}

#[derive(Serialize)]
pub struct OpenChannelResponse {
    uuid: Uuid,
    node_address: NodeAddress,
}

/// Opens a channel on the lightning network.
pub async fn open_channel(ctx: MmArc, req: OpenChannelRequest) -> OpenChannelResult<OpenChannelResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(OpenChannelError::UnsupportedCoin(e.ticker().to_string())),
    };

    // Making sure that the node data is correct and that we can connect to it before doing more operations
    let node_pubkey = req.node_address.pubkey;
    let node_addr = req.node_address.addr;
    connect_to_ln_node(node_pubkey, node_addr, ln_coin.peer_manager.clone()).await?;

    let platform_coin = ln_coin.platform_coin().clone();
    let decimals = platform_coin.as_ref().decimals;
    let my_address = platform_coin
        .as_ref()
        .derivation_method
        .single_addr_or_err()
        .await
        .map_mm_err()?;
    let (unspents, _) = platform_coin.get_unspent_ordered_list(&my_address).await.map_mm_err()?;
    let (value, fee_policy) = match req.amount.clone() {
        ChannelOpenAmount::Max => (
            unspents.iter().fold(0, |sum, unspent| sum + unspent.value),
            FeePolicy::DeductFromOutput(0),
        ),
        ChannelOpenAmount::Exact(v) => {
            let value = sat_from_big_decimal(&v, decimals).map_mm_err()?;
            (value, FeePolicy::SendExact)
        },
    };

    // The actual script_pubkey will replace this before signing the transaction after receiving the required
    // output script from the other node when the channel is accepted
    let script_pubkey = match Builder::build_p2wsh(&AddressHashEnum::WitnessScriptHash(Default::default())) {
        Ok(script) => script.to_bytes(),
        Err(err) => return MmError::err(OpenChannelError::InternalError(err.to_string())),
    };
    let outputs = vec![TransactionOutput { value, script_pubkey }];

    let mut tx_builder = UtxoTxBuilder::new(&platform_coin)
        .await
        .add_available_inputs(unspents)
        .add_outputs(outputs)
        .with_fee_policy(fee_policy);

    let fee = platform_coin
        .get_fee_rate()
        .await
        .map_err(|e| OpenChannelError::RpcError(e.to_string()))?;
    tx_builder = tx_builder.with_fee(fee);

    let (unsigned, _) = tx_builder.build().await.map_mm_err()?;

    let amount_in_sat = unsigned.outputs[0].value;
    let push_msat = req.push_msat;
    let channel_manager = ln_coin.channel_manager.clone();

    let mut conf = ln_coin.conf.clone();
    if let Some(options) = req.channel_options {
        match conf.channel_options.as_mut() {
            Some(o) => o.update_according_to(options),
            None => conf.channel_options = Some(options),
        }
    }
    if let Some(configs) = req.channel_configs {
        match conf.our_channels_configs.as_mut() {
            Some(o) => o.update_according_to(configs),
            None => conf.our_channels_configs = Some(configs),
        }
    }
    drop_mutability!(conf);
    let user_config: UserConfig = conf.into();

    let uuid = new_uuid();
    let temp_channel_id = async_blocking(move || {
        channel_manager
            .create_channel(node_pubkey, amount_in_sat, push_msat, uuid.as_u128(), Some(user_config))
            .map_to_mm(|e| OpenChannelError::FailureToOpenChannel(node_pubkey.to_string(), format!("{e:?}")))
    })
    .await?;

    {
        let mut unsigned_funding_txs = ln_coin.platform.unsigned_funding_txs.lock();
        unsigned_funding_txs.insert(uuid, unsigned);
    }

    let pending_channel_details = DBChannelDetails::new(
        uuid,
        temp_channel_id,
        node_pubkey,
        true,
        user_config.channel_handshake_config.announced_channel,
    );

    // Saving node data to reconnect to it on restart
    ln_coin.open_channels_nodes.lock().insert(node_pubkey, node_addr);
    ln_coin
        .persister
        .save_nodes_addresses(ln_coin.open_channels_nodes)
        .await?;

    if let Err(e) = ln_coin.db.add_channel_to_db(&pending_channel_details).await {
        error!("Unable to add new outbound channel {} to db: {}", uuid, e);
    }

    Ok(OpenChannelResponse {
        uuid,
        node_address: req.node_address,
    })
}
