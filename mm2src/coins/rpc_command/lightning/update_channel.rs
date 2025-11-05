use crate::lightning::ln_conf::ChannelOptions;
use crate::{lp_coinfind_or_err, CoinFindError, MmCoinEnum};
use common::{async_blocking, HttpStatusCode};
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use uuid::Uuid;

type UpdateChannelResult<T> = Result<T, MmError<UpdateChannelError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum UpdateChannelError {
    #[display(fmt = "Lightning network is not supported for {_0}")]
    UnsupportedCoin(String),
    #[display(fmt = "No such coin {_0}")]
    NoSuchCoin(String),
    #[display(fmt = "No such channel with uuid {_0}")]
    NoSuchChannel(Uuid),
    #[display(fmt = "Failure to channel {_0}: {_1}")]
    FailureToUpdateChannel(Uuid, String),
}

impl HttpStatusCode for UpdateChannelError {
    fn status_code(&self) -> StatusCode {
        match self {
            UpdateChannelError::UnsupportedCoin(_) => StatusCode::BAD_REQUEST,
            UpdateChannelError::NoSuchChannel(_) | UpdateChannelError::NoSuchCoin(_) => StatusCode::NOT_FOUND,
            UpdateChannelError::FailureToUpdateChannel(_, _) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CoinFindError> for UpdateChannelError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => UpdateChannelError::NoSuchCoin(coin),
        }
    }
}

#[derive(Deserialize)]
pub struct UpdateChannelReq {
    pub coin: String,
    pub uuid: Uuid,
    pub channel_options: ChannelOptions,
}

#[derive(Serialize)]
pub struct UpdateChannelResponse {
    channel_options: ChannelOptions,
}

/// Updates configuration for an open channel.
pub async fn update_channel(ctx: MmArc, req: UpdateChannelReq) -> UpdateChannelResult<UpdateChannelResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(UpdateChannelError::UnsupportedCoin(e.ticker().to_string())),
    };

    let channel_details = ln_coin
        .get_channel_by_uuid(req.uuid)
        .await
        .ok_or(UpdateChannelError::NoSuchChannel(req.uuid))?;

    async_blocking(move || {
        let mut channel_options = ln_coin
            .conf
            .channel_options
            .unwrap_or_else(|| req.channel_options.clone());
        if channel_options != req.channel_options {
            channel_options.update_according_to(req.channel_options.clone());
        }
        drop_mutability!(channel_options);
        let channel_ids = &[channel_details.channel_id];
        let counterparty_node_id = channel_details.counterparty.node_id;
        ln_coin
            .channel_manager
            .update_channel_config(&counterparty_node_id, channel_ids, &channel_options.clone().into())
            .map_to_mm(|e| UpdateChannelError::FailureToUpdateChannel(req.uuid, format!("{e:?}")))?;
        Ok(UpdateChannelResponse { channel_options })
    })
    .await
}
