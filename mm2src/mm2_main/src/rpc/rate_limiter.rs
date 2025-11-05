use crate::rpc::DispatcherError;
use derive_more::Display;
use futures::lock::Mutex as AsyncMutex;
use mm2_core::mm_ctx::from_ctx;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

pub const LIMIT_FAILED_REQUEST: usize = 10;
pub type RateInfosRegistry = HashMap<IpAddr, usize>;

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum RateLimitError {
    #[display(fmt = "Rate Limit attempts left: {_0}")]
    NbAttemptsLeft(usize),
}

#[derive(Default)]
pub struct RateLimitContext(AsyncMutex<RateInfosRegistry>);

impl RateLimitContext {
    pub fn from_ctx(ctx: &MmArc) -> Result<Arc<RateLimitContext>, String> {
        Ok(try_s!(from_ctx(&ctx.rate_limit_ctx, move || {
            Ok(RateLimitContext::default())
        })))
    }

    pub async fn is_banned(&self, client_ip: IpAddr) -> bool {
        let rate_infos = self.0.lock().await;
        if let Some(limit) = rate_infos.get(&client_ip) {
            return *limit >= LIMIT_FAILED_REQUEST;
        }
        false
    }
}

pub async fn process_rate_limit(ctx: &MmArc, client: &SocketAddr) -> MmError<DispatcherError> {
    let rate_limit_ctx = RateLimitContext::from_ctx(ctx).unwrap();
    let mut rate_limit_registry = rate_limit_ctx.0.lock().await;

    match rate_limit_registry.get_mut(&client.ip()) {
        Some(limit) => {
            if *limit >= LIMIT_FAILED_REQUEST {
                return MmError::new(DispatcherError::Banned);
            }
            *limit += 1;
            MmError::new(DispatcherError::UserpassIsInvalid(RateLimitError::NbAttemptsLeft(
                LIMIT_FAILED_REQUEST - *limit,
            )))
        },
        None => {
            rate_limit_registry.insert(client.ip(), 1);
            MmError::new(DispatcherError::UserpassIsInvalid(RateLimitError::NbAttemptsLeft(
                LIMIT_FAILED_REQUEST - 1,
            )))
        },
    }
}
