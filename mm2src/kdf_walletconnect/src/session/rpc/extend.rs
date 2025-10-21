use crate::{error::WalletConnectError, WalletConnectCtxImpl};

use mm2_err_handle::prelude::MmResult;
use relay_rpc::{
    domain::{MessageId, Topic},
    rpc::params::{session_extend::SessionExtendRequest, ResponseParamsSuccess},
};

/// Process session extend request.
pub(crate) async fn reply_session_extend_request(
    ctx: &WalletConnectCtxImpl,
    topic: &Topic,
    message_id: &MessageId,
    extend: SessionExtendRequest,
) -> MmResult<(), WalletConnectError> {
    ctx.session_manager.extend_session(topic, extend.expiry);

    let param = ResponseParamsSuccess::SessionExtend(true);
    ctx.publish_response_ok(topic, param, message_id).await?;

    Ok(())
}
