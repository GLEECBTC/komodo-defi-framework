use crate::session::{WcRequestResponseResult, THIRTY_DAYS};
use crate::{error::WalletConnectError, WalletConnectCtxImpl};

use chrono::Utc;
use mm2_err_handle::prelude::MmResult;
use relay_rpc::domain::MessageId;
use relay_rpc::rpc::params::pairing_ping::PairingPingRequest;
use relay_rpc::rpc::params::{RelayProtocolMetadata, RequestParams};
use relay_rpc::{
    domain::Topic,
    rpc::params::{pairing_delete::PairingDeleteRequest, pairing_extend::PairingExtendRequest, ResponseParamsSuccess},
};

pub(crate) async fn reply_pairing_ping_response(
    ctx: &WalletConnectCtxImpl,
    topic: &Topic,
    message_id: &MessageId,
) -> MmResult<(), WalletConnectError> {
    let param = ResponseParamsSuccess::PairingPing(true);
    ctx.publish_response_ok(topic, param, message_id).await?;

    Ok(())
}

pub(crate) async fn reply_pairing_extend_response(
    ctx: &WalletConnectCtxImpl,
    topic: &Topic,
    message_id: &MessageId,
    extend: PairingExtendRequest,
) -> MmResult<(), WalletConnectError> {
    ctx.pairing.activate(topic)?;
    let param = ResponseParamsSuccess::PairingExtend(true);
    ctx.publish_response_ok(topic, param, message_id).await?;

    Ok(())
}

pub(crate) async fn reply_pairing_delete_response(
    ctx: &WalletConnectCtxImpl,
    topic: &Topic,
    message_id: &MessageId,
    _delete: PairingDeleteRequest,
) -> MmResult<(), WalletConnectError> {
    ctx.pairing.disconnect_rpc(topic, &ctx.client).await?;
    let param = ResponseParamsSuccess::PairingDelete(true);
    ctx.publish_response_ok(topic, param, message_id).await?;

    Ok(())
}
