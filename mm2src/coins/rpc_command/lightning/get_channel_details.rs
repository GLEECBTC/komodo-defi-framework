use crate::lightning::ln_db::{DBChannelDetails, LightningDB};
use crate::lightning::ln_serialization::ChannelDetailsForRPC;
use crate::{lp_coinfind_or_err, CoinFindError, MmCoinEnum};
use common::HttpStatusCode;
use db_common::sqlite::rusqlite::Error as SqlError;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use uuid::Uuid;

type GetChannelDetailsResult<T> = Result<T, MmError<GetChannelDetailsError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum GetChannelDetailsError {
    #[display(fmt = "Lightning network is not supported for {_0}")]
    UnsupportedCoin(String),
    #[display(fmt = "No such coin {_0}")]
    NoSuchCoin(String),
    #[display(fmt = "Channel with uuid: {_0} is not found")]
    NoSuchChannel(Uuid),
    #[display(fmt = "DB error {_0}")]
    DbError(String),
}

impl HttpStatusCode for GetChannelDetailsError {
    fn status_code(&self) -> StatusCode {
        match self {
            GetChannelDetailsError::UnsupportedCoin(_) => StatusCode::BAD_REQUEST,
            GetChannelDetailsError::NoSuchCoin(_) | GetChannelDetailsError::NoSuchChannel(_) => StatusCode::NOT_FOUND,
            GetChannelDetailsError::DbError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CoinFindError> for GetChannelDetailsError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => GetChannelDetailsError::NoSuchCoin(coin),
        }
    }
}

impl From<SqlError> for GetChannelDetailsError {
    fn from(err: SqlError) -> GetChannelDetailsError {
        GetChannelDetailsError::DbError(err.to_string())
    }
}

#[derive(Deserialize)]
pub struct GetChannelDetailsRequest {
    pub coin: String,
    pub uuid: Uuid,
}

#[derive(Serialize)]
#[serde(tag = "status", content = "details")]
pub enum GetChannelDetailsResponse {
    Open(ChannelDetailsForRPC),
    Closed(DBChannelDetails),
}

pub async fn get_channel_details(
    ctx: MmArc,
    req: GetChannelDetailsRequest,
) -> GetChannelDetailsResult<GetChannelDetailsResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(GetChannelDetailsError::UnsupportedCoin(e.ticker().to_string())),
    };

    let channel_details = match ln_coin.get_channel_by_uuid(req.uuid).await {
        Some(details) => GetChannelDetailsResponse::Open(details.into()),
        None => GetChannelDetailsResponse::Closed(
            ln_coin
                .db
                .get_channel_from_db(req.uuid)
                .await?
                .ok_or(GetChannelDetailsError::NoSuchChannel(req.uuid))?,
        ),
    };

    Ok(channel_details)
}
