use crate::coin_balance::{BalanceObjectOps, HDAccountBalance, HDAccountBalanceEnum};
use crate::rpc_command::hd_account_balance_rpc_error::HDAccountBalanceRpcError;
use crate::{lp_coinfind_or_err, CoinsContext, MmCoinEnum};
use async_trait::async_trait;
use common::{SerdeInfallible, SuccessResponse};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use rpc_task::rpc_common::{
    CancelRpcTaskError, CancelRpcTaskRequest, InitRpcTaskResponse, RpcTaskStatusError, RpcTaskStatusRequest,
};
use rpc_task::{
    RpcInitReq, RpcTask, RpcTaskHandleShared, RpcTaskManager, RpcTaskManagerShared, RpcTaskStatus, RpcTaskTypes,
};

pub type AccountBalanceUserAction = SerdeInfallible;
pub type AccountBalanceAwaitingStatus = SerdeInfallible;
pub type AccountBalanceTaskManager = RpcTaskManager<InitAccountBalanceTask>;
pub type AccountBalanceTaskManagerShared = RpcTaskManagerShared<InitAccountBalanceTask>;
pub type InitAccountBalanceTaskHandleShared = RpcTaskHandleShared<InitAccountBalanceTask>;
pub type AccountBalanceRpcTaskStatus = RpcTaskStatus<
    HDAccountBalanceEnum,
    HDAccountBalanceRpcError,
    AccountBalanceInProgressStatus,
    AccountBalanceAwaitingStatus,
>;

#[derive(Clone, Serialize)]
pub enum AccountBalanceInProgressStatus {
    RequestingAccountBalance,
}

#[derive(Deserialize)]
pub struct InitAccountBalanceRequest {
    coin: String,
    #[serde(flatten)]
    params: InitAccountBalanceParams,
}

#[derive(Clone, Deserialize)]
pub struct InitAccountBalanceParams {
    account_index: u32,
}

#[async_trait]
pub trait InitAccountBalanceRpcOps {
    type BalanceObject;

    async fn init_account_balance_rpc(
        &self,
        params: InitAccountBalanceParams,
    ) -> MmResult<HDAccountBalance<Self::BalanceObject>, HDAccountBalanceRpcError>;
}

pub struct InitAccountBalanceTask {
    coin: MmCoinEnum,
    req: InitAccountBalanceRequest,
}

impl RpcTaskTypes for InitAccountBalanceTask {
    type Item = HDAccountBalanceEnum;
    type Error = HDAccountBalanceRpcError;
    type InProgressStatus = AccountBalanceInProgressStatus;
    type AwaitingStatus = AccountBalanceAwaitingStatus;
    type UserAction = AccountBalanceUserAction;
}

#[async_trait]
impl RpcTask for InitAccountBalanceTask {
    fn initial_status(&self) -> Self::InProgressStatus {
        AccountBalanceInProgressStatus::RequestingAccountBalance
    }

    // Do nothing if the task has been cancelled.
    async fn cancel(self) {}

    async fn run(
        &mut self,
        _task_handle: InitAccountBalanceTaskHandleShared,
    ) -> Result<Self::Item, MmError<Self::Error>> {
        match self.coin {
            MmCoinEnum::UtxoCoin(ref utxo) => Ok(HDAccountBalanceEnum::Map(
                utxo.init_account_balance_rpc(self.req.params.clone()).await?,
            )),
            MmCoinEnum::QtumCoin(ref qtum) => Ok(HDAccountBalanceEnum::Map(
                qtum.init_account_balance_rpc(self.req.params.clone()).await?,
            )),
            MmCoinEnum::EthCoin(ref eth) => Ok(HDAccountBalanceEnum::Map(
                eth.init_account_balance_rpc(self.req.params.clone()).await?,
            )),
            _ => MmError::err(HDAccountBalanceRpcError::CoinIsActivatedNotWithHDWallet),
        }
    }
}

pub async fn init_account_balance(
    ctx: MmArc,
    req: RpcInitReq<InitAccountBalanceRequest>,
) -> MmResult<InitRpcTaskResponse, HDAccountBalanceRpcError> {
    let (client_id, req) = (req.client_id, req.inner);
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    let spawner = coin.spawner();
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(HDAccountBalanceRpcError::Internal)?;
    let task = InitAccountBalanceTask { coin, req };
    let task_id =
        AccountBalanceTaskManager::spawn_rpc_task(&coins_ctx.account_balance_task_manager, &spawner, task, client_id)
            .map_mm_err()?;
    Ok(InitRpcTaskResponse { task_id })
}

pub async fn init_account_balance_status(
    ctx: MmArc,
    req: RpcTaskStatusRequest,
) -> MmResult<AccountBalanceRpcTaskStatus, RpcTaskStatusError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(RpcTaskStatusError::Internal)?;
    let mut task_manager = coins_ctx
        .account_balance_task_manager
        .lock()
        .map_to_mm(|e| RpcTaskStatusError::Internal(e.to_string()))?;
    task_manager
        .task_status(req.task_id, req.forget_if_finished)
        .or_mm_err(|| RpcTaskStatusError::NoSuchTask(req.task_id))
}

pub async fn cancel_account_balance(
    ctx: MmArc,
    req: CancelRpcTaskRequest,
) -> MmResult<SuccessResponse, CancelRpcTaskError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(CancelRpcTaskError::Internal)?;
    let mut task_manager = coins_ctx
        .account_balance_task_manager
        .lock()
        .map_to_mm(|e| CancelRpcTaskError::Internal(e.to_string()))?;
    task_manager.cancel_task(req.task_id).map_mm_err()?;
    Ok(SuccessResponse::new())
}

pub mod common_impl {
    use super::*;
    use crate::coin_balance::{HDWalletBalanceObject, HDWalletBalanceOps};
    use crate::hd_wallet::{HDAccountOps, HDCoinAddress, HDWalletOps};
    use crate::CoinWithDerivationMethod;
    use crypto::RpcDerivationPath;
    use std::fmt;

    pub async fn init_account_balance_rpc<Coin>(
        coin: &Coin,
        params: InitAccountBalanceParams,
    ) -> MmResult<HDAccountBalance<HDWalletBalanceObject<Coin>>, HDAccountBalanceRpcError>
    where
        Coin: HDWalletBalanceOps + CoinWithDerivationMethod + Sync,
        HDCoinAddress<Coin>: fmt::Display + Clone,
    {
        let account_id = params.account_index;
        let hd_account = coin
            .derivation_method()
            .hd_wallet_or_err()
            .map_mm_err()?
            .get_account(account_id)
            .await
            .or_mm_err(|| HDAccountBalanceRpcError::UnknownAccount { account_id })?;

        let addresses = coin.all_known_addresses_balances(&hd_account).await.map_mm_err()?;

        let total_balance = addresses
            .iter()
            .fold(HDWalletBalanceObject::<Coin>::new(), |mut total, addr_balance| {
                total.add(addr_balance.balance.clone());
                total
            });

        Ok(HDAccountBalance {
            account_index: account_id,
            derivation_path: RpcDerivationPath(hd_account.account_derivation_path()),
            total_balance,
            addresses,
        })
    }
}
