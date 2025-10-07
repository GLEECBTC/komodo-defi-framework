use crate::storage::WalletConnectStorageOps;
use crate::{error::WalletConnectError, WalletConnectCtxImpl};

use common::log::info;
use mm2_err_handle::prelude::*;
use relay_rpc::domain::{MessageId, Topic};
use relay_rpc::rpc::params::{session_update::SessionUpdateRequest, ResponseParamsSuccess};

pub(crate) async fn reply_session_update_request(
    ctx: &WalletConnectCtxImpl,
    topic: &Topic,
    message_id: &MessageId,
    update: SessionUpdateRequest,
) -> MmResult<(), WalletConnectError> {
    {
        let mut session = ctx.session_manager.write();
        let Some(session) = session.get_mut(topic) else {
            return MmError::err(WalletConnectError::SessionError(format!(
                "No session found for topic: {topic}"
            )));
        };
        update
            .namespaces
            .caip2_validate()
            .map_to_mm(|err| WalletConnectError::InternalError(err.to_string()))?;
        session.namespaces = update.namespaces.0;

        info!("Updated extended, info: {:?}", session.topic);
    }

    //  Update storage session.
    let session = ctx
        .session_manager
        .get_session(topic)
        .ok_or(MmError::new(WalletConnectError::SessionError(format!(
            "session not foun topic: {topic}"
        ))))?;
    ctx.session_manager
        .storage()
        .update_session(&session)
        .await
        .mm_err(|err| WalletConnectError::StorageError(err.to_string()))?;

    let param = ResponseParamsSuccess::SessionUpdate(true);
    ctx.publish_response_ok(topic, param, message_id).await?;

    Ok(())
}
