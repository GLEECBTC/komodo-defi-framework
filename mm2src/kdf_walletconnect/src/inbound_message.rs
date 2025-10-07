use crate::{
    error::WalletConnectError,
    pairing::{reply_pairing_delete_response, reply_pairing_extend_response, reply_pairing_ping_response},
    session::rpc::{
        delete::reply_session_delete_request,
        event::handle_session_event,
        extend::reply_session_extend_request,
        ping::reply_session_ping_request,
        propose::{process_session_propose_response, reply_session_proposal_request},
        settle::reply_session_settle_request,
        update::reply_session_update_request,
    },
    WalletConnectCtxImpl,
};

use common::log::{info, LogOnError};
use mm2_err_handle::prelude::*;
use relay_rpc::domain::{MessageId, Topic};
use relay_rpc::rpc::{params::ResponseParamsSuccess, Params, Request, Response};

pub(crate) type SessionMessageType = MmResult<SessionMessage, WalletConnectError>;

#[derive(Debug)]
pub struct SessionMessage {
    pub message_id: MessageId,
    pub topic: Topic,
    pub data: ResponseParamsSuccess,
}

/// Processes an inbound WalletConnect request and performs the appropriate action based on the request type.
///
/// Handles various session and pairing requests, routing them to their corresponding handlers.
pub(crate) async fn process_inbound_request(
    ctx: &WalletConnectCtxImpl,
    request: Request,
    topic: &Topic,
) -> MmResult<(), WalletConnectError> {
    let message_id = request.id;
    match request.params {
        Params::SessionPropose(proposal) => reply_session_proposal_request(ctx, proposal, topic, &message_id).await?,
        Params::SessionExtend(param) => reply_session_extend_request(ctx, topic, &message_id, param).await?,
        Params::SessionDelete(param) => reply_session_delete_request(ctx, topic, &message_id, param).await?,
        Params::SessionPing(()) => reply_session_ping_request(ctx, topic, &message_id).await?,
        Params::SessionSettle(param) => reply_session_settle_request(ctx, topic, param).await?,
        Params::SessionUpdate(param) => reply_session_update_request(ctx, topic, &message_id, param).await?,
        Params::SessionEvent(param) => handle_session_event(ctx, topic, &message_id, param).await?,
        Params::SessionRequest(_param) => {
            // TODO: Implement when integrating KDF as a Dapp.
            return MmError::err(WalletConnectError::NotImplemented);
        },

        Params::PairingPing(_param) => reply_pairing_ping_response(ctx, topic, &message_id).await?,
        Params::PairingDelete(param) => reply_pairing_delete_response(ctx, topic, &message_id, param).await?,
        Params::PairingExtend(param) => reply_pairing_extend_response(ctx, topic, &message_id, param).await?,
        _ => {
            info!("Unknown request params received.");
            return MmError::err(WalletConnectError::InvalidRequest);
        },
    };

    Ok(())
}

/// Processes an inbound WalletConnect response and sends the result to the provided message channel.
///
/// Handles successful responses, errors, and specific session proposal processing.
pub(crate) async fn process_inbound_response(ctx: &WalletConnectCtxImpl, response: Response, topic: &Topic) {
    let message_id = response.id();
    let result = match &response {
        Response::Success(value) => match serde_json::from_value::<ResponseParamsSuccess>(value.result.clone()) {
            Ok(ResponseParamsSuccess::SessionPropose(propose)) => {
                // If this is a session propose response, process it right away and return.
                // Session proposal responses are not waited for since it might take a long time
                // for the proposal to be accepted (user interaction). So they are handled in async fashion.
                ctx.pending_requests
                    .lock()
                    .expect("pending request lock shouldn't fail!")
                    .remove(&message_id);
                return process_session_propose_response(ctx, topic, &propose)
                    .await
                    .error_log_with_msg("Failed to process session propose response");
            },
            Ok(data) => Ok(SessionMessage {
                message_id,
                topic: topic.clone(),
                data,
            }),
            Err(err) => MmError::err(WalletConnectError::SerdeError(err.to_string())),
        },
        Response::Error(err) => MmError::err(WalletConnectError::UnSuccessfulResponse(format!("{err:?}"))),
    };

    let mut pending_requests = ctx
        .pending_requests
        .lock()
        .expect("pending request lock shouldn't fail!");
    if let Some(tx) = pending_requests.remove(&message_id) {
        tx.send(result).ok();
    } else {
        common::log::error!("[{topic}] unrecognized inbound response/message: {response:?}");
    };
}
