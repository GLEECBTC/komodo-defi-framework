mod common_impl;
mod init_bch_activation;
mod init_qtum_activation;
mod init_utxo_standard_activation;
mod init_utxo_standard_activation_error;
mod init_utxo_standard_statuses;
mod utxo_standard_activation_result;

pub use init_bch_activation::BchTaskManagerShared;
pub use init_qtum_activation::QtumTaskManagerShared;
pub use init_utxo_standard_activation::UtxoStandardTaskManagerShared;

/// helpers for use in tests in other modules
#[cfg(not(target_arch = "wasm32"))]
pub mod for_tests {
    use common::{executor::Timer, now_ms, wait_until_ms};
    use mm2_core::mm_ctx::MmArc;
    use mm2_err_handle::prelude::MmResult;
    use rpc_task::{RpcInitReq, RpcTaskStatus};

    use crate::{
        init_standalone_coin, init_standalone_coin_status,
        standalone_coin::{InitStandaloneCoinActivationOps, InitStandaloneCoinError, InitStandaloneCoinInitialStatus},
        InitStandaloneCoinReq, InitStandaloneCoinStatusRequest,
    };

    /// test helper to activate standalone coin with waiting for the result
    pub async fn init_standalone_coin_loop<Standalone>(
        ctx: MmArc,
        request: InitStandaloneCoinReq<Standalone::ActivationRequest>,
    ) -> MmResult<Standalone::ActivationResult, InitStandaloneCoinError>
    where
        Standalone: InitStandaloneCoinActivationOps + Send + Sync + 'static,
        Standalone::InProgressStatus: InitStandaloneCoinInitialStatus,
        InitStandaloneCoinError: From<Standalone::ActivationError>,
    {
        let request = RpcInitReq {
            client_id: 0,
            inner: request,
        };
        let init_result = init_standalone_coin::<Standalone>(ctx.clone(), request).await.unwrap();
        let timeout = wait_until_ms(150000);
        loop {
            if now_ms() > timeout {
                panic!("init_standalone_coin timed out");
            }
            let status_req = InitStandaloneCoinStatusRequest {
                task_id: init_result.task_id,
                forget_if_finished: true,
            };
            let status_res = init_standalone_coin_status::<Standalone>(ctx.clone(), status_req).await;
            if let Ok(status) = status_res {
                match status {
                    RpcTaskStatus::Ok(result) => break Ok(result),
                    RpcTaskStatus::Error(e) => break Err(e),
                    _ => Timer::sleep(1.).await,
                }
            } else {
                panic!("could not get init_standalone_coin status");
            }
        }
    }
}
