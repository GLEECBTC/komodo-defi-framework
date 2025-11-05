use crate::coin_balance::HDAddressBalance;
use crate::rpc_command::hd_account_balance_rpc_error::HDAccountBalanceRpcError;
use crate::{lp_coinfind_or_err, CoinBalance, CoinBalanceMap, CoinsContext, MmCoinEnum};
use async_trait::async_trait;
use common::{SerdeInfallible, SuccessResponse};
use crypto::RpcDerivationPath;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use rpc_task::rpc_common::{
    CancelRpcTaskError, CancelRpcTaskRequest, InitRpcTaskResponse, RpcTaskStatusError, RpcTaskStatusRequest,
};
use rpc_task::{
    RpcInitReq, RpcTask, RpcTaskHandleShared, RpcTaskManager, RpcTaskManagerShared, RpcTaskStatus, RpcTaskTypes,
};

pub type ScanAddressesUserAction = SerdeInfallible;
pub type ScanAddressesAwaitingStatus = SerdeInfallible;
pub type ScanAddressesTaskManager = RpcTaskManager<InitScanAddressesTask>;
pub type ScanAddressesTaskManagerShared = RpcTaskManagerShared<InitScanAddressesTask>;
pub type ScanAddressesTaskHandleShared = RpcTaskHandleShared<InitScanAddressesTask>;
pub type ScanAddressesRpcTaskStatus = RpcTaskStatus<
    ScanAddressesResponseEnum,
    HDAccountBalanceRpcError,
    ScanAddressesInProgressStatus,
    ScanAddressesAwaitingStatus,
>;

/// Generic response for the `scan_for_new_addresses` RPC commands.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScanAddressesResponse<BalanceObject> {
    pub account_index: u32,
    pub derivation_path: RpcDerivationPath,
    pub new_addresses: Vec<HDAddressBalance<BalanceObject>>,
}

/// Enum for the response of the `scan_for_new_addresses` RPC commands.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ScanAddressesResponseEnum {
    Single(ScanAddressesResponse<CoinBalance>),
    Map(ScanAddressesResponse<CoinBalanceMap>),
}

#[derive(Deserialize)]
pub struct ScanAddressesRequest {
    coin: String,
    #[serde(flatten)]
    params: ScanAddressesParams,
}

#[derive(Clone, Deserialize)]
pub struct ScanAddressesParams {
    pub account_index: u32,
    // The max number of empty addresses in a row.
    // If transactions were sent to an address outside the `gap_limit`, they will not be identified.
    pub gap_limit: Option<u32>,
}

#[derive(Clone, Serialize)]
pub enum ScanAddressesInProgressStatus {
    InProgress,
}

/// Trait for the `scan_for_new_addresses` RPC commands.
#[async_trait]
pub trait InitScanAddressesRpcOps {
    type BalanceObject;

    async fn init_scan_for_new_addresses_rpc(
        &self,
        params: ScanAddressesParams,
    ) -> MmResult<ScanAddressesResponse<Self::BalanceObject>, HDAccountBalanceRpcError>;
}

pub struct InitScanAddressesTask {
    req: ScanAddressesRequest,
    coin: MmCoinEnum,
}

impl RpcTaskTypes for InitScanAddressesTask {
    type Item = ScanAddressesResponseEnum;
    type Error = HDAccountBalanceRpcError;
    type InProgressStatus = ScanAddressesInProgressStatus;
    type AwaitingStatus = ScanAddressesAwaitingStatus;
    type UserAction = ScanAddressesUserAction;
}

#[async_trait]
impl RpcTask for InitScanAddressesTask {
    #[inline]
    fn initial_status(&self) -> Self::InProgressStatus {
        ScanAddressesInProgressStatus::InProgress
    }

    // Do nothing if the task has been cancelled.
    async fn cancel(self) {}

    async fn run(&mut self, _task_handle: ScanAddressesTaskHandleShared) -> Result<Self::Item, MmError<Self::Error>> {
        match self.coin {
            MmCoinEnum::UtxoCoin(ref utxo) => Ok(ScanAddressesResponseEnum::Map(
                utxo.init_scan_for_new_addresses_rpc(self.req.params.clone()).await?,
            )),
            MmCoinEnum::QtumCoin(ref qtum) => Ok(ScanAddressesResponseEnum::Map(
                qtum.init_scan_for_new_addresses_rpc(self.req.params.clone()).await?,
            )),
            MmCoinEnum::EthCoin(ref eth) => Ok(ScanAddressesResponseEnum::Map(
                eth.init_scan_for_new_addresses_rpc(self.req.params.clone()).await?,
            )),
            _ => MmError::err(HDAccountBalanceRpcError::CoinIsActivatedNotWithHDWallet),
        }
    }
}

pub async fn init_scan_for_new_addresses(
    ctx: MmArc,
    req: RpcInitReq<ScanAddressesRequest>,
) -> MmResult<InitRpcTaskResponse, HDAccountBalanceRpcError> {
    let (client_id, req) = (req.client_id, req.inner);
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    let spawner = coin.spawner();
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(HDAccountBalanceRpcError::Internal)?;
    let task = InitScanAddressesTask { req, coin };
    let task_id =
        ScanAddressesTaskManager::spawn_rpc_task(&coins_ctx.scan_addresses_manager, &spawner, task, client_id)
            .map_mm_err()?;
    Ok(InitRpcTaskResponse { task_id })
}

pub async fn init_scan_for_new_addresses_status(
    ctx: MmArc,
    req: RpcTaskStatusRequest,
) -> MmResult<ScanAddressesRpcTaskStatus, RpcTaskStatusError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(RpcTaskStatusError::Internal)?;
    let mut task_manager = coins_ctx
        .scan_addresses_manager
        .lock()
        .map_to_mm(|e| RpcTaskStatusError::Internal(e.to_string()))?;
    task_manager
        .task_status(req.task_id, req.forget_if_finished)
        .or_mm_err(|| RpcTaskStatusError::NoSuchTask(req.task_id))
}

pub async fn cancel_scan_for_new_addresses(
    ctx: MmArc,
    req: CancelRpcTaskRequest,
) -> MmResult<SuccessResponse, CancelRpcTaskError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(CancelRpcTaskError::Internal)?;
    let mut task_manager = coins_ctx
        .scan_addresses_manager
        .lock()
        .map_to_mm(|e| CancelRpcTaskError::Internal(e.to_string()))?;
    task_manager.cancel_task(req.task_id).map_mm_err()?;
    Ok(SuccessResponse::new())
}

pub mod common_impl {
    use super::*;
    use crate::coin_balance::{HDWalletBalanceObject, HDWalletBalanceOps};
    use crate::hd_wallet::{HDAccountOps, HDWalletOps};
    use crate::CoinWithDerivationMethod;
    use std::collections::HashSet;
    use std::ops::DerefMut;

    pub async fn scan_for_new_addresses_rpc<Coin>(
        coin: &Coin,
        params: ScanAddressesParams,
    ) -> MmResult<ScanAddressesResponse<HDWalletBalanceObject<Coin>>, HDAccountBalanceRpcError>
    where
        Coin: CoinWithDerivationMethod + HDWalletBalanceOps + Sync,
    {
        let hd_wallet = coin.derivation_method().hd_wallet_or_err().map_mm_err()?;

        let account_id = params.account_index;
        let mut hd_account = hd_wallet
            .get_account_mut(account_id)
            .await
            .or_mm_err(|| HDAccountBalanceRpcError::CoinIsActivatedNotWithHDWallet)?;
        let account_derivation_path = hd_account.account_derivation_path();
        let address_scanner = coin.produce_hd_address_scanner().await.map_mm_err()?;
        let gap_limit = params.gap_limit.unwrap_or_else(|| hd_wallet.gap_limit());

        let new_addresses = coin
            .scan_for_new_addresses(hd_wallet, hd_account.deref_mut(), &address_scanner, gap_limit)
            .await
            .map_mm_err()?;

        let addresses: HashSet<_> = new_addresses
            .iter()
            .map(|address_balance| address_balance.address.clone())
            .collect();

        coin.prepare_addresses_for_balance_stream_if_enabled(addresses)
            .await
            .map_err(|e| HDAccountBalanceRpcError::FailedScripthashSubscription(e.to_string()))?;

        Ok(ScanAddressesResponse {
            account_index: account_id,
            derivation_path: RpcDerivationPath(account_derivation_path),
            new_addresses,
        })
    }
}
