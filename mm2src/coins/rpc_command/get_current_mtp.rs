use common::{HttpStatusCode, StatusCode};
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;

use crate::{
    lp_coinfind_or_err,
    utxo::{rpc_clients::UtxoRpcError, UtxoCommonOps},
    CoinFindError, MmCoinEnum,
};

pub type GetCurrentMtpRpcResult<T> = Result<T, MmError<GetCurrentMtpError>>;

#[derive(Deserialize)]
pub struct GetCurrentMtpRequest {
    coin: String,
}

#[derive(Serialize)]
pub struct GetCurrentMtpResponse {
    mtp: u32,
}

#[derive(Serialize, Display, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum GetCurrentMtpError {
    NoSuchCoin(String),
    #[display(fmt = "Requested coin: {_0}; is not supported for this action.")]
    NotSupportedCoin(String),
    RpcError(String),
}

impl HttpStatusCode for GetCurrentMtpError {
    fn status_code(&self) -> StatusCode {
        match self {
            GetCurrentMtpError::NoSuchCoin(_) => StatusCode::NOT_FOUND,
            GetCurrentMtpError::NotSupportedCoin(_) => StatusCode::BAD_REQUEST,
            GetCurrentMtpError::RpcError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<UtxoRpcError> for GetCurrentMtpError {
    fn from(err: UtxoRpcError) -> Self {
        Self::RpcError(err.to_string())
    }
}

impl From<CoinFindError> for GetCurrentMtpError {
    fn from(err: CoinFindError) -> Self {
        Self::NoSuchCoin(err.to_string())
    }
}

pub async fn get_current_mtp_rpc(
    ctx: MmArc,
    req: GetCurrentMtpRequest,
) -> GetCurrentMtpRpcResult<GetCurrentMtpResponse> {
    match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::UtxoCoin(utxo) => Ok(GetCurrentMtpResponse {
            mtp: utxo.get_current_mtp().await.map_mm_err()?,
        }),
        MmCoinEnum::QtumCoin(qtum) => Ok(GetCurrentMtpResponse {
            mtp: qtum.get_current_mtp().await.map_mm_err()?,
        }),
        MmCoinEnum::Qrc20Coin(qrc) => Ok(GetCurrentMtpResponse {
            mtp: qrc.get_current_mtp().await.map_mm_err()?,
        }),
        #[cfg(not(target_arch = "wasm32"))]
        MmCoinEnum::ZCoin(zcoin) => Ok(GetCurrentMtpResponse {
            mtp: zcoin.get_current_mtp().await.map_mm_err()?,
        }),
        MmCoinEnum::Bch(bch) => Ok(GetCurrentMtpResponse {
            mtp: bch.get_current_mtp().await.map_mm_err()?,
        }),
        _ => Err(MmError::new(GetCurrentMtpError::NotSupportedCoin(req.coin))),
    }
}
