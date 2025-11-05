use crate::lightning::ln_errors::EnableLightningError;
use crate::lightning::ln_p2p::{connect_to_ln_node, ConnectToNodeRes, ConnectionError};
use crate::lightning::ln_serialization::NodeAddress;
use crate::lightning::ln_storage::LightningStorage;
use crate::{lp_coinfind_or_err, CoinFindError, MmCoinEnum};
use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use std::collections::hash_map::Entry;

type ConnectToNodeResult<T> = Result<T, MmError<ConnectToNodeError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum ConnectToNodeError {
    #[display(fmt = "Parse error: {_0}")]
    ParseError(String),
    #[display(fmt = "Error connecting to node: {_0}")]
    ConnectionError(String),
    #[display(fmt = "I/O error {_0}")]
    IOError(String),
    #[display(fmt = "Lightning network is not supported for {_0}")]
    UnsupportedCoin(String),
    #[display(fmt = "No such coin {_0}")]
    NoSuchCoin(String),
}

impl HttpStatusCode for ConnectToNodeError {
    fn status_code(&self) -> StatusCode {
        match self {
            ConnectToNodeError::UnsupportedCoin(_) => StatusCode::BAD_REQUEST,
            ConnectToNodeError::ParseError(_)
            | ConnectToNodeError::IOError(_)
            | ConnectToNodeError::ConnectionError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ConnectToNodeError::NoSuchCoin(_) => StatusCode::NOT_FOUND,
        }
    }
}

impl From<ConnectToNodeError> for EnableLightningError {
    fn from(err: ConnectToNodeError) -> EnableLightningError {
        EnableLightningError::ConnectToNodeError(err.to_string())
    }
}

impl From<CoinFindError> for ConnectToNodeError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => ConnectToNodeError::NoSuchCoin(coin),
        }
    }
}

impl From<std::io::Error> for ConnectToNodeError {
    fn from(err: std::io::Error) -> ConnectToNodeError {
        ConnectToNodeError::IOError(err.to_string())
    }
}

impl From<ConnectionError> for ConnectToNodeError {
    fn from(err: ConnectionError) -> ConnectToNodeError {
        ConnectToNodeError::ConnectionError(err.to_string())
    }
}

#[derive(Deserialize)]
pub struct ConnectToNodeRequest {
    pub coin: String,
    pub node_address: NodeAddress,
}

/// Connect to a certain node on the lightning network.
pub async fn connect_to_node(ctx: MmArc, req: ConnectToNodeRequest) -> ConnectToNodeResult<String> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(ConnectToNodeError::UnsupportedCoin(e.ticker().to_string())),
    };

    let node_pubkey = req.node_address.pubkey;
    let node_addr = req.node_address.addr;
    let res = connect_to_ln_node(node_pubkey, node_addr, ln_coin.peer_manager.clone()).await?;

    // If a node that we have an open channel with changed it's address, "connect_to_node"
    // can be used to reconnect to the new address while saving this new address for reconnections.
    if let ConnectToNodeRes::ConnectedSuccessfully { .. } = res {
        if let Entry::Occupied(mut entry) = ln_coin.open_channels_nodes.lock().entry(node_pubkey) {
            entry.insert(node_addr);
        }
        ln_coin
            .persister
            .save_nodes_addresses(ln_coin.open_channels_nodes)
            .await?;
    }

    Ok(res.to_string())
}
