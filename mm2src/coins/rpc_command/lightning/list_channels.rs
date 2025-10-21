use crate::lightning::ln_db::{ClosedChannelsFilter, DBChannelDetails, LightningDB};
use crate::lightning::ln_serialization::ChannelDetailsForRPC;
use crate::lightning::OpenChannelsFilter;
use crate::{lp_coinfind_or_err, CoinFindError, MmCoinEnum};
use common::{calc_total_pages, ten, HttpStatusCode, PagingOptionsEnum};
use db_common::sqlite::rusqlite::Error as SqlError;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use uuid::Uuid;

type ListChannelsResult<T> = Result<T, MmError<ListChannelsError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum ListChannelsError {
    #[display(fmt = "Lightning network is not supported for {_0}")]
    UnsupportedCoin(String),
    #[display(fmt = "No such coin {_0}")]
    NoSuchCoin(String),
    #[display(fmt = "DB error {_0}")]
    DbError(String),
}

impl HttpStatusCode for ListChannelsError {
    fn status_code(&self) -> StatusCode {
        match self {
            ListChannelsError::UnsupportedCoin(_) => StatusCode::BAD_REQUEST,
            ListChannelsError::NoSuchCoin(_) => StatusCode::NOT_FOUND,
            ListChannelsError::DbError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CoinFindError> for ListChannelsError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => ListChannelsError::NoSuchCoin(coin),
        }
    }
}

impl From<SqlError> for ListChannelsError {
    fn from(err: SqlError) -> ListChannelsError {
        ListChannelsError::DbError(err.to_string())
    }
}

#[derive(Deserialize)]
pub struct ListOpenChannelsRequest {
    pub coin: String,
    pub filter: Option<OpenChannelsFilter>,
    #[serde(default = "ten")]
    limit: usize,
    #[serde(default)]
    paging_options: PagingOptionsEnum<Uuid>,
}

#[derive(Serialize)]
pub struct ListOpenChannelsResponse {
    open_channels: Vec<ChannelDetailsForRPC>,
    limit: usize,
    skipped: usize,
    total: usize,
    total_pages: usize,
    paging_options: PagingOptionsEnum<Uuid>,
}

pub async fn list_open_channels_by_filter(
    ctx: MmArc,
    req: ListOpenChannelsRequest,
) -> ListChannelsResult<ListOpenChannelsResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(ListChannelsError::UnsupportedCoin(e.ticker().to_string())),
    };

    let result = ln_coin
        .get_open_channels_by_filter(req.filter, req.paging_options.clone(), req.limit)
        .await;

    Ok(ListOpenChannelsResponse {
        open_channels: result.channels,
        limit: req.limit,
        skipped: result.skipped,
        total: result.total,
        total_pages: calc_total_pages(result.total, req.limit),
        paging_options: req.paging_options,
    })
}

#[derive(Deserialize)]
pub struct ListClosedChannelsRequest {
    pub coin: String,
    pub filter: Option<ClosedChannelsFilter>,
    #[serde(default = "ten")]
    limit: usize,
    #[serde(default)]
    paging_options: PagingOptionsEnum<Uuid>,
}

#[derive(Serialize)]
pub struct ListClosedChannelsResponse {
    closed_channels: Vec<DBChannelDetails>,
    limit: usize,
    skipped: usize,
    total: usize,
    total_pages: usize,
    paging_options: PagingOptionsEnum<Uuid>,
}

pub async fn list_closed_channels_by_filter(
    ctx: MmArc,
    req: ListClosedChannelsRequest,
) -> ListChannelsResult<ListClosedChannelsResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(ListChannelsError::UnsupportedCoin(e.ticker().to_string())),
    };
    let closed_channels_res = ln_coin
        .db
        .get_closed_channels_by_filter(req.filter, req.paging_options.clone(), req.limit)
        .await?;

    Ok(ListClosedChannelsResponse {
        closed_channels: closed_channels_res.channels,
        limit: req.limit,
        skipped: closed_channels_res.skipped,
        total: closed_channels_res.total,
        total_pages: calc_total_pages(closed_channels_res.total, req.limit),
        paging_options: req.paging_options,
    })
}
