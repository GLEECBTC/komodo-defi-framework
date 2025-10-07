use kdf_walletconnect::session::rpc::send_session_ping_request;
use kdf_walletconnect::session::SessionRpcInfo;
use kdf_walletconnect::WalletConnectCtx;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use serde::Serialize;

use super::{EmptyRpcRequest, EmptyRpcResponse, WalletConnectRpcError};

#[derive(Debug, PartialEq, Serialize)]
pub struct SessionResponse {
    pub result: String,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct GetSessionsResponse {
    pub sessions: Vec<SessionRpcInfo>,
}

/// `Get all sessions connection` RPC command implementation.
pub async fn get_all_sessions(
    ctx: MmArc,
    _req: Option<EmptyRpcRequest>,
) -> MmResult<GetSessionsResponse, WalletConnectRpcError> {
    let wc_ctx =
        WalletConnectCtx::from_ctx(&ctx).mm_err(|err| WalletConnectRpcError::InitializationError(err.to_string()))?;
    let sessions = wc_ctx.session_manager.get_sessions().collect::<Vec<_>>();

    Ok(GetSessionsResponse { sessions })
}

#[derive(Debug, Serialize)]
pub struct GetSessionResponse {
    pub session: Option<SessionRpcInfo>,
}

#[derive(Deserialize)]
pub struct GetSessionRequest {
    topic: String,
    #[serde(default)]
    with_pairing_topic: bool,
}

/// `Get session connection` RPC command implementation.
pub async fn get_session(ctx: MmArc, req: GetSessionRequest) -> MmResult<GetSessionResponse, WalletConnectRpcError> {
    let wc_ctx =
        WalletConnectCtx::from_ctx(&ctx).mm_err(|err| WalletConnectRpcError::InitializationError(err.to_string()))?;
    let session = wc_ctx
        .session_manager
        .get_session_with_any_topic(&req.topic.into(), req.with_pairing_topic)
        .map(SessionRpcInfo::from);

    Ok(GetSessionResponse { session })
}

/// `Delete session connection` RPC command implementation.
pub async fn disconnect_session(
    ctx: MmArc,
    req: GetSessionRequest,
) -> MmResult<EmptyRpcResponse, WalletConnectRpcError> {
    let wc_ctx =
        WalletConnectCtx::from_ctx(&ctx).mm_err(|err| WalletConnectRpcError::InitializationError(err.to_string()))?;
    wc_ctx
        .drop_session(&req.topic.into())
        .await
        .mm_err(|err| WalletConnectRpcError::SessionRequestError(err.to_string()))?;

    Ok(EmptyRpcResponse {})
}

/// `ping session` RPC command implementation.
pub async fn ping_session(ctx: MmArc, req: GetSessionRequest) -> MmResult<SessionResponse, WalletConnectRpcError> {
    let wc_ctx =
        WalletConnectCtx::from_ctx(&ctx).mm_err(|err| WalletConnectRpcError::InitializationError(err.to_string()))?;
    send_session_ping_request(&wc_ctx, &req.topic.into())
        .await
        .mm_err(|err| WalletConnectRpcError::SessionRequestError(err.to_string()))?;

    Ok(SessionResponse {
        result: "Ping successful".to_owned(),
    })
}
