use crate::{error::WalletConnectError, WalletConnectCtxImpl};

use common::custom_futures::timeout::FutureTimerExt;
use mm2_err_handle::prelude::*;
use relay_rpc::{
    domain::{MessageId, Topic},
    rpc::params::{RequestParams, ResponseParamsSuccess},
};

pub(crate) async fn reply_session_ping_request(
    ctx: &WalletConnectCtxImpl,
    topic: &Topic,
    message_id: &MessageId,
) -> MmResult<(), WalletConnectError> {
    let param = ResponseParamsSuccess::SessionPing(true);
    ctx.publish_response_ok(topic, param, message_id).await?;

    Ok(())
}

pub async fn send_session_ping_request(ctx: &WalletConnectCtxImpl, topic: &Topic) -> MmResult<(), WalletConnectError> {
    let param = RequestParams::SessionPing(());
    let (rx, ttl) = ctx.publish_request(topic, param).await?;
    println!("ping sent successfuly");
    rx.timeout(ttl)
        .await
        .map_to_mm(|_| WalletConnectError::TimeoutError)?
        .map_to_mm(|err| WalletConnectError::InternalError(err.to_string()))??;
    println!("ping sent successfuly");

    Ok(())
}
