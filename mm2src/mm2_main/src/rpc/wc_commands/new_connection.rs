use kdf_walletconnect::{NewConnection, WalletConnectCtx, WcTopic};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use serde::{Deserialize, Serialize};

use super::WalletConnectRpcError;

#[derive(Debug, PartialEq, Serialize)]
pub struct CreateConnectionResponse {
    pub url: String,
    pub pairing_topic: WcTopic,
}

#[derive(Deserialize)]
pub struct NewConnectionRequest {
    required_namespaces: serde_json::Value,
    optional_namespaces: Option<serde_json::Value>,
}

/// `new_connection` RPC command implementation.
pub async fn new_connection(
    ctx: MmArc,
    req: NewConnectionRequest,
) -> MmResult<CreateConnectionResponse, WalletConnectRpcError> {
    let wc_ctx =
        WalletConnectCtx::from_ctx(&ctx).mm_err(|err| WalletConnectRpcError::InitializationError(err.to_string()))?;
    let NewConnection { url, pairing_topic } = wc_ctx
        .new_connection(req.required_namespaces, req.optional_namespaces)
        .await
        .mm_err(|err| WalletConnectRpcError::SessionRequestError(err.to_string()))?;

    Ok(CreateConnectionResponse { url, pairing_topic })
}
