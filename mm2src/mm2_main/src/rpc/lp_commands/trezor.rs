use common::HttpStatusCode;
use crypto::{CryptoCtx, CryptoCtxError, HwConnectionStatus, HwPubkey};
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::map_mm_error::MmResultExt;
use mm2_err_handle::mm_error::{MmError, MmResult};
use mm2_err_handle::or_mm_error::OrMmError;

#[derive(Serialize, Display, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum TrezorConnectionError {
    #[display(fmt = "Trezor hasn't been initialized yet")]
    TrezorNotInitialized,
    #[display(fmt = "Found unexpected device. Please re-initialize Hardware wallet")]
    FoundUnexpectedDevice,
    Internal(String),
}

impl From<CryptoCtxError> for TrezorConnectionError {
    fn from(e: CryptoCtxError) -> Self {
        TrezorConnectionError::Internal(format!("'CryptoCtx' is not available: {e}"))
    }
}

impl HttpStatusCode for TrezorConnectionError {
    fn status_code(&self) -> StatusCode {
        match self {
            TrezorConnectionError::TrezorNotInitialized => StatusCode::BAD_REQUEST,
            TrezorConnectionError::FoundUnexpectedDevice | TrezorConnectionError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
        }
    }
}

#[derive(Deserialize)]
pub struct TrezorConnectionStatusReq {
    /// Can be used to make sure that the Trezor device is expected.
    device_pubkey: Option<HwPubkey>,
}

#[derive(Serialize)]
pub struct TrezorConnectionStatusRes {
    status: HwConnectionStatus,
}

pub async fn trezor_connection_status(
    ctx: MmArc,
    req: TrezorConnectionStatusReq,
) -> MmResult<TrezorConnectionStatusRes, TrezorConnectionError> {
    let crypto_ctx = CryptoCtx::from_ctx(&ctx).map_mm_err()?;
    let hw_ctx = crypto_ctx
        .hw_ctx()
        .or_mm_err(|| TrezorConnectionError::TrezorNotInitialized)?;

    if let Some(expected) = req.device_pubkey {
        if hw_ctx.hw_pubkey() != expected {
            return MmError::err(TrezorConnectionError::FoundUnexpectedDevice);
        }
    }

    Ok(TrezorConnectionStatusRes {
        status: hw_ctx.trezor_connection_status().await,
    })
}
