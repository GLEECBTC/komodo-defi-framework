use crate::lightning::ln_db::LightningDB;
use crate::lightning::ln_serialization::{PaymentInfoForRPC, PaymentsFilterForRPC};
use crate::{lp_coinfind_or_err, CoinFindError, H256Json, MmCoinEnum};
use common::{calc_total_pages, ten, HttpStatusCode, PagingOptionsEnum};
use db_common::sqlite::rusqlite::Error as SqlError;
use derive_more::Display;
use http::StatusCode;
use lightning::ln::PaymentHash;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;

type ListPaymentsResult<T> = Result<T, MmError<ListPaymentsError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum ListPaymentsError {
    #[display(fmt = "Lightning network is not supported for {_0}")]
    UnsupportedCoin(String),
    #[display(fmt = "No such coin {_0}")]
    NoSuchCoin(String),
    #[display(fmt = "DB error {_0}")]
    DbError(String),
}

impl HttpStatusCode for ListPaymentsError {
    fn status_code(&self) -> StatusCode {
        match self {
            ListPaymentsError::UnsupportedCoin(_) => StatusCode::BAD_REQUEST,
            ListPaymentsError::NoSuchCoin(_) => StatusCode::NOT_FOUND,
            ListPaymentsError::DbError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CoinFindError> for ListPaymentsError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => ListPaymentsError::NoSuchCoin(coin),
        }
    }
}

impl From<SqlError> for ListPaymentsError {
    fn from(err: SqlError) -> ListPaymentsError {
        ListPaymentsError::DbError(err.to_string())
    }
}

#[derive(Deserialize)]
pub struct ListPaymentsReq {
    pub coin: String,
    pub filter: Option<PaymentsFilterForRPC>,
    #[serde(default = "ten")]
    limit: usize,
    #[serde(default)]
    paging_options: PagingOptionsEnum<H256Json>,
}

#[derive(Serialize)]
pub struct ListPaymentsResponse {
    payments: Vec<PaymentInfoForRPC>,
    limit: usize,
    skipped: usize,
    total: usize,
    total_pages: usize,
    paging_options: PagingOptionsEnum<H256Json>,
}

pub async fn list_payments_by_filter(ctx: MmArc, req: ListPaymentsReq) -> ListPaymentsResult<ListPaymentsResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(ListPaymentsError::UnsupportedCoin(e.ticker().to_string())),
    };
    let get_payments_res = ln_coin
        .db
        .get_payments_by_filter(
            req.filter.map(From::from),
            req.paging_options.clone().map(|h| PaymentHash(h.0)),
            req.limit,
        )
        .await?;

    Ok(ListPaymentsResponse {
        payments: get_payments_res.payments.into_iter().map(From::from).collect(),
        limit: req.limit,
        skipped: get_payments_res.skipped,
        total: get_payments_res.total,
        total_pages: calc_total_pages(get_payments_res.total, req.limit),
        paging_options: req.paging_options,
    })
}
