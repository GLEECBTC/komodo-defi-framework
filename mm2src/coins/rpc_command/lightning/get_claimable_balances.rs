use crate::lightning::ln_serialization::ClaimableBalance;
use crate::{lp_coinfind_or_err, CoinFindError, MmCoinEnum};
use common::{async_blocking, HttpStatusCode};
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;

type ClaimableBalancesResult<T> = Result<T, MmError<ClaimableBalancesError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum ClaimableBalancesError {
    #[display(fmt = "Lightning network is not supported for {_0}")]
    UnsupportedCoin(String),
    #[display(fmt = "No such coin {_0}")]
    NoSuchCoin(String),
}

impl HttpStatusCode for ClaimableBalancesError {
    fn status_code(&self) -> StatusCode {
        match self {
            ClaimableBalancesError::UnsupportedCoin(_) => StatusCode::BAD_REQUEST,
            ClaimableBalancesError::NoSuchCoin(_) => StatusCode::NOT_FOUND,
        }
    }
}

impl From<CoinFindError> for ClaimableBalancesError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => ClaimableBalancesError::NoSuchCoin(coin),
        }
    }
}

#[derive(Deserialize)]
pub struct ClaimableBalancesReq {
    pub coin: String,
    #[serde(default)]
    pub include_open_channels_balances: bool,
}

pub async fn get_claimable_balances(
    ctx: MmArc,
    req: ClaimableBalancesReq,
) -> ClaimableBalancesResult<Vec<ClaimableBalance>> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(ClaimableBalancesError::UnsupportedCoin(e.ticker().to_string())),
    };
    let ignored_channels = if req.include_open_channels_balances {
        Vec::new()
    } else {
        ln_coin.list_channels().await
    };
    let claimable_balances = async_blocking(move || {
        ln_coin
            .chain_monitor
            .get_claimable_balances(&ignored_channels.iter().collect::<Vec<_>>()[..])
            .into_iter()
            .map(From::from)
            .collect()
    })
    .await;

    Ok(claimable_balances)
}
