use super::eth_fee_events::EstimatorType;
use super::ser::FeePerGasEstimated;
use crate::{lp_coinfind, MmCoinEnum};
use common::HttpStatusCode;
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::mm_error::{MmError, MmResult};

use http::StatusCode;
use std::convert::TryFrom;

#[derive(Deserialize)]
pub struct GetFeeEstimationRequest {
    coin: String,
    estimator_type: EstimatorType,
}

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum GetFeeEstimationRequestError {
    CoinNotFound,
    Internal(String),
    CoinNotSupported,
}

impl HttpStatusCode for GetFeeEstimationRequestError {
    fn status_code(&self) -> StatusCode {
        match self {
            GetFeeEstimationRequestError::CoinNotFound => StatusCode::NOT_FOUND,
            GetFeeEstimationRequestError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            GetFeeEstimationRequestError::CoinNotSupported => StatusCode::NOT_IMPLEMENTED,
        }
    }
}

pub async fn get_eth_estimated_fee_per_gas(
    ctx: MmArc,
    req: GetFeeEstimationRequest,
) -> MmResult<FeePerGasEstimated, GetFeeEstimationRequestError> {
    match lp_coinfind(&ctx, &req.coin).await {
        Ok(Some(MmCoinEnum::EthCoinVariant(coin))) => {
            let use_simple = matches!(req.estimator_type, EstimatorType::Simple);
            let fee = coin
                .get_eip1559_gas_fee(use_simple)
                .await
                .map_err(|e| GetFeeEstimationRequestError::Internal(e.to_string()))?;
            let ser_fee =
                FeePerGasEstimated::try_from(fee).map_err(|e| GetFeeEstimationRequestError::Internal(e.to_string()))?;
            Ok(ser_fee)
        },
        Ok(Some(_)) => MmError::err(GetFeeEstimationRequestError::CoinNotSupported),
        Ok(None) => MmError::err(GetFeeEstimationRequestError::CoinNotFound),
        Err(e) => MmError::err(GetFeeEstimationRequestError::Internal(e)),
    }
}
