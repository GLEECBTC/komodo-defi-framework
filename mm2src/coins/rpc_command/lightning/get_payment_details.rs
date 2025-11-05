use crate::lightning::ln_db::LightningDB;
use crate::lightning::ln_serialization::PaymentInfoForRPC;
use crate::{lp_coinfind_or_err, CoinFindError, H256Json, MmCoinEnum};
use common::HttpStatusCode;
use db_common::sqlite::rusqlite::Error as SqlError;
use derive_more::Display;
use http::StatusCode;
use lightning::ln::PaymentHash;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;

type GetPaymentDetailsResult<T> = Result<T, MmError<GetPaymentDetailsError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum GetPaymentDetailsError {
    #[display(fmt = "Lightning network is not supported for {_0}")]
    UnsupportedCoin(String),
    #[display(fmt = "No such coin {_0}")]
    NoSuchCoin(String),
    #[display(fmt = "Payment with hash: {_0:?} is not found")]
    NoSuchPayment(H256Json),
    #[display(fmt = "DB error {_0}")]
    DbError(String),
}

impl HttpStatusCode for GetPaymentDetailsError {
    fn status_code(&self) -> StatusCode {
        match self {
            GetPaymentDetailsError::UnsupportedCoin(_) => StatusCode::BAD_REQUEST,
            GetPaymentDetailsError::NoSuchCoin(_) => StatusCode::NOT_FOUND,
            GetPaymentDetailsError::NoSuchPayment(_) => StatusCode::NOT_FOUND,
            GetPaymentDetailsError::DbError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CoinFindError> for GetPaymentDetailsError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => GetPaymentDetailsError::NoSuchCoin(coin),
        }
    }
}

impl From<SqlError> for GetPaymentDetailsError {
    fn from(err: SqlError) -> GetPaymentDetailsError {
        GetPaymentDetailsError::DbError(err.to_string())
    }
}

#[derive(Deserialize)]
pub struct GetPaymentDetailsRequest {
    pub coin: String,
    pub payment_hash: H256Json,
}

#[derive(Serialize)]
pub struct GetPaymentDetailsResponse {
    payment_details: PaymentInfoForRPC,
}

pub async fn get_payment_details(
    ctx: MmArc,
    req: GetPaymentDetailsRequest,
) -> GetPaymentDetailsResult<GetPaymentDetailsResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(GetPaymentDetailsError::UnsupportedCoin(e.ticker().to_string())),
    };

    if let Some(payment_info) = ln_coin.db.get_payment_from_db(PaymentHash(req.payment_hash.0)).await? {
        return Ok(GetPaymentDetailsResponse {
            payment_details: payment_info.into(),
        });
    }

    MmError::err(GetPaymentDetailsError::NoSuchPayment(req.payment_hash))
}
