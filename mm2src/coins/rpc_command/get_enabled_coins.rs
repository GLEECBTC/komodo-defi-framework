use crate::CoinsContext;
use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::MmResult;
use mm2_err_handle::prelude::*;

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum GetEnabledCoinsError {
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl HttpStatusCode for GetEnabledCoinsError {
    fn status_code(&self) -> StatusCode {
        match self {
            GetEnabledCoinsError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Deserialize)]
pub struct GetEnabledCoinsRequest {}

#[derive(Debug, Serialize)]
pub struct GetEnabledCoinsResponse {
    pub coins: Vec<EnabledCoinV2>,
}

#[derive(Debug, Serialize)]
pub struct EnabledCoinV2 {
    ticker: String,
}

pub async fn get_enabled_coins_rpc(
    ctx: MmArc,
    _req: Option<GetEnabledCoinsRequest>,
) -> MmResult<GetEnabledCoinsResponse, GetEnabledCoinsError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(GetEnabledCoinsError::Internal)?;
    let coins_map = coins_ctx.coins.lock().await;

    let coins = coins_map
        .keys()
        .map(|ticker| EnabledCoinV2 { ticker: ticker.clone() })
        .collect();
    Ok(GetEnabledCoinsResponse { coins })
}
