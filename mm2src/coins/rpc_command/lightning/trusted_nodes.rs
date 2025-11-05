use crate::lightning::ln_serialization::PublicKeyForRPC;
use crate::lightning::ln_storage::LightningStorage;
use crate::{lp_coinfind_or_err, CoinFindError, MmCoinEnum};
use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;

type TrustedNodeResult<T> = Result<T, MmError<TrustedNodeError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum TrustedNodeError {
    #[display(fmt = "Lightning network is not supported for {_0}")]
    UnsupportedCoin(String),
    #[display(fmt = "No such coin {_0}")]
    NoSuchCoin(String),
    #[display(fmt = "I/O error {_0}")]
    IOError(String),
}

impl HttpStatusCode for TrustedNodeError {
    fn status_code(&self) -> StatusCode {
        match self {
            TrustedNodeError::UnsupportedCoin(_) => StatusCode::BAD_REQUEST,
            TrustedNodeError::NoSuchCoin(_) => StatusCode::NOT_FOUND,
            TrustedNodeError::IOError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CoinFindError> for TrustedNodeError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => TrustedNodeError::NoSuchCoin(coin),
        }
    }
}

impl From<std::io::Error> for TrustedNodeError {
    fn from(err: std::io::Error) -> TrustedNodeError {
        TrustedNodeError::IOError(err.to_string())
    }
}

#[derive(Deserialize)]
pub struct AddTrustedNodeReq {
    pub coin: String,
    pub node_id: PublicKeyForRPC,
}

#[derive(Serialize)]
pub struct AddTrustedNodeResponse {
    pub added_node: PublicKeyForRPC,
}

pub async fn add_trusted_node(ctx: MmArc, req: AddTrustedNodeReq) -> TrustedNodeResult<AddTrustedNodeResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(TrustedNodeError::UnsupportedCoin(e.ticker().to_string())),
    };

    if ln_coin.trusted_nodes.lock().insert(req.node_id.clone().into()) {
        ln_coin.persister.save_trusted_nodes(ln_coin.trusted_nodes).await?;
    }

    Ok(AddTrustedNodeResponse {
        added_node: req.node_id,
    })
}

#[derive(Deserialize)]
pub struct RemoveTrustedNodeReq {
    pub coin: String,
    pub node_id: PublicKeyForRPC,
}

#[derive(Serialize)]
pub struct RemoveTrustedNodeResponse {
    pub removed_node: PublicKeyForRPC,
}

pub async fn remove_trusted_node(
    ctx: MmArc,
    req: RemoveTrustedNodeReq,
) -> TrustedNodeResult<RemoveTrustedNodeResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(TrustedNodeError::UnsupportedCoin(e.ticker().to_string())),
    };

    if ln_coin.trusted_nodes.lock().remove(&req.node_id.clone().into()) {
        ln_coin.persister.save_trusted_nodes(ln_coin.trusted_nodes).await?;
    }

    Ok(RemoveTrustedNodeResponse {
        removed_node: req.node_id,
    })
}

#[derive(Deserialize)]
pub struct ListTrustedNodesReq {
    pub coin: String,
}

#[derive(Serialize)]
pub struct ListTrustedNodesResponse {
    trusted_nodes: Vec<PublicKeyForRPC>,
}

pub async fn list_trusted_nodes(ctx: MmArc, req: ListTrustedNodesReq) -> TrustedNodeResult<ListTrustedNodesResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(TrustedNodeError::UnsupportedCoin(e.ticker().to_string())),
    };

    let trusted_nodes = ln_coin.trusted_nodes.lock().clone();

    Ok(ListTrustedNodesResponse {
        trusted_nodes: trusted_nodes.into_iter().map(PublicKeyForRPC).collect(),
    })
}
