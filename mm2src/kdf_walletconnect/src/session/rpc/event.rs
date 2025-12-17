use crate::{
    chain::{WcChain, WcChainId},
    error::{WalletConnectError, UNSUPPORTED_CHAINS},
    WalletConnectCtxImpl,
};

use common::log::{error, info};
use mm2_err_handle::prelude::*;
use relay_rpc::{
    domain::{MessageId, Topic},
    rpc::{
        params::{session_event::SessionEventRequest, ResponseParamsError},
        ErrorData,
    },
};

pub async fn handle_session_event(
    ctx: &WalletConnectCtxImpl,
    topic: &Topic,
    message_id: &MessageId,
    event: SessionEventRequest,
) -> MmResult<(), WalletConnectError> {
    let chain_id = WcChainId::try_from_str(&event.chain_id)?;
    let event_name = event.event.name.as_str();

    match event_name {
        "chainChanged" => {
            let session =
                ctx.session_manager
                    .get_session(topic)
                    .ok_or(MmError::new(WalletConnectError::SessionError(
                        "No active WalletConnect session found".to_string(),
                    )))?;

            if WcChain::Eip155 != chain_id.chain {
                return Ok(());
            };

            ctx.validate_chain_id(&session, &chain_id)?;

            if session.get_active_chain_id().as_ref() == Some(&chain_id) {
                return Ok(());
            };

            // check if if new chain_id is supported.
            let new_id = serde_json::from_value::<u32>(event.event.data)?;
            let new_chain = chain_id.chain.derive_chain_id(new_id.to_string());
            if let Err(err) = ctx.validate_chain_id(&session, &new_chain) {
                error!("[{topic}] {err:?}");
                let error_data = ErrorData {
                    code: UNSUPPORTED_CHAINS,
                    message: "Unsupported chain id".to_string(),
                    data: None,
                };
                let params = ResponseParamsError::SessionEvent(error_data);
                ctx.publish_response_err(topic, params, message_id).await?;
            } else {
                {
                    ctx.session_manager
                        .write()
                        .get_mut(topic)
                        .ok_or(MmError::new(WalletConnectError::SessionError(
                            "No active WalletConnect session found".to_string(),
                        )))?
                        .set_active_chain_id(chain_id);
                }
            };
        },
        "accountsChanged" => {
            // TODO: Handle accountsChanged event logic.
        },
        _ => {
            // TODO: Handle other event logic.,
        },
    };

    info!("[{topic}] {event_name} event handled successfully");
    Ok(())
}
