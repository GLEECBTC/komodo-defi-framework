use crate::coin_balance::{BalanceObjectOps, HDAccountBalance, HDAccountBalanceEnum};
use crate::hd_wallet::{HDExtractPubkeyError, HDXPubExtractor, NewAccountCreationError, RpcTaskXPubExtractor};
use crate::{
    lp_coinfind_or_err, BalanceError, CoinFindError, CoinProtocol, CoinWithDerivationMethod, CoinsContext,
    MarketCoinOps, MmCoinEnum, UnexpectedDerivationMethod,
};
use async_trait::async_trait;
use common::{true_f, HttpStatusCode, SuccessResponse};
use crypto::hw_rpc_task::{
    HwConnectStatuses, HwRpcTaskAwaitingStatus, HwRpcTaskUserAction, HwRpcTaskUserActionRequest,
};
use crypto::{from_hw_error, Bip44Chain, HwError, HwRpcError, RpcDerivationPath, WithHwRpcError};
use derive_more::Display;
use enum_derives::EnumFromTrait;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use parking_lot::Mutex as PaMutex;
use rpc_task::rpc_common::{
    CancelRpcTaskError, CancelRpcTaskRequest, InitRpcTaskResponse, RpcTaskStatusError, RpcTaskStatusRequest,
    RpcTaskUserActionError,
};
use rpc_task::{
    RpcInitReq, RpcTask, RpcTaskError, RpcTaskHandleShared, RpcTaskManager, RpcTaskManagerShared, RpcTaskStatus,
    RpcTaskTypes,
};
use std::sync::Arc;
use std::time::Duration;

pub type CreateAccountUserAction = HwRpcTaskUserAction;
pub type CreateAccountAwaitingStatus = HwRpcTaskAwaitingStatus;
pub type CreateAccountTaskManager = RpcTaskManager<InitCreateAccountTask>;
pub type CreateAccountTaskManagerShared = RpcTaskManagerShared<InitCreateAccountTask>;
pub type CreateAccountTaskHandleShared = RpcTaskHandleShared<InitCreateAccountTask>;
pub type CreateAccountRpcTaskStatus = RpcTaskStatus<
    HDAccountBalanceEnum,
    CreateAccountRpcError,
    CreateAccountInProgressStatus,
    CreateAccountAwaitingStatus,
>;

type CreateAccountXPubExtractor = RpcTaskXPubExtractor<InitCreateAccountTask>;

#[derive(Clone, Debug, Display, EnumFromTrait, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum CreateAccountRpcError {
    #[display(fmt = "Hardware Wallet context is not initialized")]
    HwContextNotInitialized,
    #[display(fmt = "No such coin {coin}")]
    NoSuchCoin { coin: String },
    #[display(fmt = "RPC 'task' is awaiting '{expected}' user action")]
    UnexpectedUserAction { expected: String },
    #[from_trait(WithTimeout::timeout)]
    #[display(fmt = "RPC timed out {_0:?}")]
    Timeout(Duration),
    #[display(fmt = "Coin is expected to be activated with the HD wallet derivation method")]
    CoinIsActivatedNotWithHDWallet,
    #[display(fmt = "Coin doesn't support the given BIP44 chain: {chain:?}")]
    InvalidBip44Chain { chain: Bip44Chain },
    #[display(fmt = "Accounts limit reached. Max number of accounts: {max_accounts_number}")]
    AccountLimitReached { max_accounts_number: u32 },
    #[display(fmt = "Electrum/Native RPC invalid response: {_0}")]
    RpcInvalidResponse(String),
    #[display(fmt = "HD wallet storage error: {_0}")]
    WalletStorageError(String),
    #[from_trait(WithHwRpcError::hw_rpc_error)]
    #[display(fmt = "{_0}")]
    HwError(HwRpcError),
    #[display(fmt = "Transport: {_0}")]
    Transport(String),
    #[from_trait(WithInternal::internal)]
    #[display(fmt = "Internal: {_0}")]
    Internal(String),
}

impl From<CoinFindError> for CreateAccountRpcError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => CreateAccountRpcError::NoSuchCoin { coin },
        }
    }
}

impl From<UnexpectedDerivationMethod> for CreateAccountRpcError {
    fn from(e: UnexpectedDerivationMethod) -> Self {
        match e {
            UnexpectedDerivationMethod::ExpectedHDWallet => CreateAccountRpcError::CoinIsActivatedNotWithHDWallet,
            unexpected_error => CreateAccountRpcError::Internal(unexpected_error.to_string()),
        }
    }
}

impl From<NewAccountCreationError> for CreateAccountRpcError {
    fn from(e: NewAccountCreationError) -> Self {
        match e {
            NewAccountCreationError::HwContextNotInitialized => CreateAccountRpcError::HwContextNotInitialized,
            NewAccountCreationError::HDWalletUnavailable => CreateAccountRpcError::CoinIsActivatedNotWithHDWallet,
            NewAccountCreationError::CoinDoesntSupportTrezor => {
                CreateAccountRpcError::Internal("Coin must support Trezor at this point".to_string())
            },
            NewAccountCreationError::RpcTaskError(rpc) => CreateAccountRpcError::from(rpc),
            NewAccountCreationError::HardwareWalletError(hw) => CreateAccountRpcError::from(hw),
            NewAccountCreationError::AccountLimitReached { max_accounts_number } => {
                CreateAccountRpcError::AccountLimitReached { max_accounts_number }
            },
            NewAccountCreationError::ErrorSavingAccountToStorage(e) => {
                let error = format!("Error uploading HD account info to the storage: {e}");
                CreateAccountRpcError::WalletStorageError(error)
            },
            NewAccountCreationError::Internal(internal) => CreateAccountRpcError::Internal(internal),
        }
    }
}

impl From<BalanceError> for CreateAccountRpcError {
    fn from(e: BalanceError) -> Self {
        match e {
            BalanceError::Transport(transport) => CreateAccountRpcError::Transport(transport),
            BalanceError::InvalidResponse(rpc) => CreateAccountRpcError::RpcInvalidResponse(rpc),
            BalanceError::UnexpectedDerivationMethod(der_path) => CreateAccountRpcError::from(der_path),
            BalanceError::WalletStorageError(internal) | BalanceError::Internal(internal) => {
                CreateAccountRpcError::Internal(internal)
            },
            BalanceError::NoSuchCoin { coin } => CreateAccountRpcError::NoSuchCoin { coin },
        }
    }
}

impl From<HDExtractPubkeyError> for CreateAccountRpcError {
    fn from(e: HDExtractPubkeyError) -> Self {
        CreateAccountRpcError::from(NewAccountCreationError::from(e))
    }
}

impl From<RpcTaskError> for CreateAccountRpcError {
    fn from(e: RpcTaskError) -> Self {
        let error = e.to_string();
        match e {
            RpcTaskError::Cancelled => CreateAccountRpcError::Internal("Cancelled".to_owned()),
            RpcTaskError::Timeout(timeout) => CreateAccountRpcError::Timeout(timeout),
            RpcTaskError::NoSuchTask(_) | RpcTaskError::UnexpectedTaskStatus { .. } => {
                CreateAccountRpcError::Internal(error)
            },
            RpcTaskError::UnexpectedUserAction { expected } => CreateAccountRpcError::UnexpectedUserAction { expected },
            RpcTaskError::Internal(internal) => CreateAccountRpcError::Internal(internal),
        }
    }
}

impl From<HwError> for CreateAccountRpcError {
    fn from(e: HwError) -> Self {
        from_hw_error(e)
    }
}

impl HttpStatusCode for CreateAccountRpcError {
    fn status_code(&self) -> StatusCode {
        match self {
            CreateAccountRpcError::HwContextNotInitialized
            | CreateAccountRpcError::NoSuchCoin { .. }
            | CreateAccountRpcError::UnexpectedUserAction { .. }
            | CreateAccountRpcError::CoinIsActivatedNotWithHDWallet
            | CreateAccountRpcError::InvalidBip44Chain { .. }
            | CreateAccountRpcError::AccountLimitReached { .. } => StatusCode::BAD_REQUEST,
            CreateAccountRpcError::HwError(_) => StatusCode::GONE,
            CreateAccountRpcError::Timeout(_) => StatusCode::REQUEST_TIMEOUT,
            CreateAccountRpcError::Transport(_)
            | CreateAccountRpcError::RpcInvalidResponse(_)
            | CreateAccountRpcError::WalletStorageError(_)
            | CreateAccountRpcError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Deserialize, Clone)]
pub struct CreateNewAccountRequest {
    coin: String,
    #[serde(flatten)]
    params: CreateNewAccountParams,
}

#[derive(Clone, Deserialize)]
pub struct CreateNewAccountParams {
    #[serde(default = "true_f")]
    scan: bool,
    // The max number of empty addresses in a row.
    // If transactions were sent to an address outside the `gap_limit`, they will not be identified.
    gap_limit: Option<u32>,
    account_id: Option<u32>,
}

#[derive(Clone, Serialize)]
pub enum CreateAccountInProgressStatus {
    Preparing,
    RequestingAccountBalance,
    Finishing,
    /// The following statuses don't require the user to send `UserAction`,
    /// but they tell the user that he should confirm/decline the operation on his device.
    WaitingForTrezorToConnect,
    FollowHwDeviceInstructions,
}

#[derive(Default)]
struct StateData {
    account_id: Option<u32>,
}

#[derive(Clone, Default)]
pub struct CreateAccountState(Arc<PaMutex<StateData>>);

impl CreateAccountState {
    pub fn on_account_created(&self, account_id: u32) {
        self.0.lock().account_id = Some(account_id);
    }

    pub fn create_account_id(&self) -> Option<u32> {
        self.0.lock().account_id
    }
}

#[async_trait]
pub trait InitCreateAccountRpcOps {
    type BalanceObject;

    async fn init_create_account_rpc<XPubExtractor>(
        &self,
        params: CreateNewAccountParams,
        state: CreateAccountState,
        xpub_extractor: Option<XPubExtractor>,
    ) -> MmResult<HDAccountBalance<Self::BalanceObject>, CreateAccountRpcError>
    where
        XPubExtractor: HDXPubExtractor + Send;

    async fn revert_creating_account(&self, account_id: u32);
}

#[derive(Clone)]
pub struct InitCreateAccountTask {
    ctx: MmArc,
    coin: MmCoinEnum,
    req: CreateNewAccountRequest,
    /// The state of the account creation. It's used to revert changes if the task has been cancelled.
    task_state: CreateAccountState,
}

impl RpcTaskTypes for InitCreateAccountTask {
    type Item = HDAccountBalanceEnum;
    type Error = CreateAccountRpcError;
    type InProgressStatus = CreateAccountInProgressStatus;
    type AwaitingStatus = CreateAccountAwaitingStatus;
    type UserAction = CreateAccountUserAction;
}

#[async_trait]
impl RpcTask for InitCreateAccountTask {
    fn initial_status(&self) -> Self::InProgressStatus {
        CreateAccountInProgressStatus::Preparing
    }

    async fn cancel(self) {
        if let Some(account_id) = self.task_state.create_account_id() {
            // We created the account already, so need to revert the changes.
            match self.coin {
                MmCoinEnum::UtxoCoin(utxo) => utxo.revert_creating_account(account_id).await,
                MmCoinEnum::QtumCoin(qtum) => qtum.revert_creating_account(account_id).await,
                MmCoinEnum::EthCoin(eth) => eth.revert_creating_account(account_id).await,
                _ => (),
            }
        };
    }

    async fn run(&mut self, task_handle: CreateAccountTaskHandleShared) -> Result<Self::Item, MmError<Self::Error>> {
        async fn create_new_account_helper<Coin>(
            ctx: &MmArc,
            coin: &Coin,
            params: CreateNewAccountParams,
            state: CreateAccountState,
            task_handle: CreateAccountTaskHandleShared,
            is_trezor: bool,
            coin_protocol: CoinProtocol,
        ) -> MmResult<HDAccountBalance<<Coin as InitCreateAccountRpcOps>::BalanceObject>, CreateAccountRpcError>
        where
            Coin: InitCreateAccountRpcOps + Send + Sync,
        {
            let xpub_extractor = if is_trezor {
                let hw_statuses = HwConnectStatuses {
                    on_connect: CreateAccountInProgressStatus::WaitingForTrezorToConnect,
                    on_connected: CreateAccountInProgressStatus::Preparing,
                    on_connection_failed: CreateAccountInProgressStatus::Finishing,
                    on_button_request: CreateAccountInProgressStatus::FollowHwDeviceInstructions,
                    on_pin_request: CreateAccountAwaitingStatus::EnterTrezorPin,
                    on_passphrase_request: CreateAccountAwaitingStatus::EnterTrezorPassphrase,
                    on_ready: CreateAccountInProgressStatus::RequestingAccountBalance,
                };
                Some(
                    CreateAccountXPubExtractor::new_trezor_extractor(ctx, task_handle, hw_statuses, coin_protocol)
                        .map_mm_err()?,
                )
            } else {
                None
            };
            coin.init_create_account_rpc(params, state, xpub_extractor).await
        }

        match self.coin {
            MmCoinEnum::UtxoCoin(ref utxo) => Ok(HDAccountBalanceEnum::Map(
                create_new_account_helper(
                    &self.ctx,
                    utxo,
                    self.req.params.clone(),
                    self.task_state.clone(),
                    task_handle,
                    utxo.is_trezor(),
                    // Note that the actual UtxoProtocolInfo isn't needed by trezor XPUB extractor.
                    CoinProtocol::UTXO(Default::default()),
                )
                .await?,
            )),
            MmCoinEnum::QtumCoin(ref qtum) => Ok(HDAccountBalanceEnum::Map(
                create_new_account_helper(
                    &self.ctx,
                    qtum,
                    self.req.params.clone(),
                    self.task_state.clone(),
                    task_handle,
                    qtum.is_trezor(),
                    CoinProtocol::QTUM,
                )
                .await?,
            )),
            MmCoinEnum::EthCoin(ref eth) => Ok(HDAccountBalanceEnum::Map(
                create_new_account_helper(
                    &self.ctx,
                    eth,
                    self.req.params.clone(),
                    self.task_state.clone(),
                    task_handle,
                    eth.is_trezor(),
                    // Todo: add support for Tron by checking eth.chain_spec
                    CoinProtocol::ETH {
                        chain_id: eth.chain_id().ok_or_else(|| {
                            CreateAccountRpcError::Internal("chain_id should be available for an EVM coin".to_string())
                        })?,
                    },
                )
                .await?,
            )),
            _ => MmError::err(CreateAccountRpcError::CoinIsActivatedNotWithHDWallet),
        }
    }
}

pub async fn init_create_new_account(
    ctx: MmArc,
    req: RpcInitReq<CreateNewAccountRequest>,
) -> MmResult<InitRpcTaskResponse, CreateAccountRpcError> {
    let (client_id, req) = (req.client_id, req.inner);
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(CreateAccountRpcError::Internal)?;
    let spawner = coin.spawner();
    let task = InitCreateAccountTask {
        ctx,
        coin,
        req,
        task_state: CreateAccountState::default(),
    };
    let task_id =
        CreateAccountTaskManager::spawn_rpc_task(&coins_ctx.create_account_manager, &spawner, task, client_id)
            .map_mm_err()?;
    Ok(InitRpcTaskResponse { task_id })
}

pub async fn init_create_new_account_status(
    ctx: MmArc,
    req: RpcTaskStatusRequest,
) -> MmResult<CreateAccountRpcTaskStatus, RpcTaskStatusError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(RpcTaskStatusError::Internal)?;
    let mut task_manager = coins_ctx
        .create_account_manager
        .lock()
        .map_to_mm(|e| RpcTaskStatusError::Internal(e.to_string()))?;
    task_manager
        .task_status(req.task_id, req.forget_if_finished)
        .or_mm_err(|| RpcTaskStatusError::NoSuchTask(req.task_id))
}

pub async fn init_create_new_account_user_action(
    ctx: MmArc,
    req: HwRpcTaskUserActionRequest,
) -> MmResult<SuccessResponse, RpcTaskUserActionError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(RpcTaskUserActionError::Internal)?;
    let mut task_manager = coins_ctx
        .create_account_manager
        .lock()
        .map_to_mm(|e| RpcTaskUserActionError::Internal(e.to_string()))?;
    task_manager.on_user_action(req.task_id, req.user_action).map_mm_err()?;
    Ok(SuccessResponse::new())
}

pub async fn cancel_create_new_account(
    ctx: MmArc,
    req: CancelRpcTaskRequest,
) -> MmResult<SuccessResponse, CancelRpcTaskError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(CancelRpcTaskError::Internal)?;
    let mut task_manager = coins_ctx
        .create_account_manager
        .lock()
        .map_to_mm(|e| CancelRpcTaskError::Internal(e.to_string()))?;
    task_manager.cancel_task(req.task_id).map_mm_err()?;
    Ok(SuccessResponse::new())
}

pub(crate) mod common_impl {
    use super::*;
    use crate::coin_balance::{HDWalletBalanceObject, HDWalletBalanceOps};
    use crate::hd_wallet::{
        create_new_account, ExtractExtendedPubkey, HDAccountOps, HDAccountStorageOps, HDCoinExtendedPubkey,
        HDCoinHDAccount, HDWalletOps,
    };

    pub async fn init_create_new_account_rpc<Coin, XPubExtractor>(
        coin: &Coin,
        params: CreateNewAccountParams,
        state: CreateAccountState,
        xpub_extractor: Option<XPubExtractor>,
    ) -> MmResult<HDAccountBalance<HDWalletBalanceObject<Coin>>, CreateAccountRpcError>
    where
        Coin: ExtractExtendedPubkey<ExtendedPublicKey = HDCoinExtendedPubkey<Coin>>
            + HDWalletBalanceOps
            + CoinWithDerivationMethod
            + Send
            + Sync,
        XPubExtractor: HDXPubExtractor + Send,
        HDCoinHDAccount<Coin>: HDAccountStorageOps,
    {
        let hd_wallet = coin.derivation_method().hd_wallet_or_err().map_mm_err()?;

        let mut new_account = create_new_account(coin, hd_wallet, xpub_extractor, params.account_id)
            .await
            .map_mm_err()?;
        let account_index = new_account.account_id();
        let account_derivation_path = new_account.account_derivation_path();

        // Change the task state.
        state.on_account_created(account_index);

        let addresses = if params.scan {
            let gap_limit = params.gap_limit.unwrap_or_else(|| hd_wallet.gap_limit());
            let address_scanner = coin.produce_hd_address_scanner().await.map_mm_err()?;
            coin.scan_for_new_addresses(hd_wallet, &mut new_account, &address_scanner, gap_limit)
                .await
                .map_mm_err()?
        } else {
            Vec::new()
        };

        let total_balance = addresses
            .iter()
            .fold(HDWalletBalanceObject::<Coin>::new(), |mut total, addr_balance| {
                total.add(addr_balance.balance.clone());
                total
            });

        Ok(HDAccountBalance {
            account_index,
            derivation_path: RpcDerivationPath(account_derivation_path),
            total_balance,
            addresses,
        })
    }

    pub async fn revert_creating_account<Coin>(coin: &Coin, account_id: u32)
    where
        Coin: HDWalletBalanceOps + CoinWithDerivationMethod + Sync,
    {
        if let Some(hd_wallet) = coin.derivation_method().hd_wallet() {
            hd_wallet.remove_account_if_last(account_id).await;
        }
    }
}

#[cfg(all(feature = "for-tests", not(target_arch = "wasm32")))]
pub mod for_tests {
    use super::{init_create_new_account, init_create_new_account_status, CreateAccountRpcError};
    use crate::coin_balance::HDAccountBalanceEnum;
    use common::executor::Timer;
    use common::{now_ms, wait_until_ms};
    use mm2_core::mm_ctx::MmArc;
    use mm2_err_handle::prelude::MmResult;
    use rpc_task::rpc_common::RpcTaskStatusRequest;
    use rpc_task::RpcTaskStatus;
    use serde_json::{self, json};

    /// Helper to call init_create_new_account fn and wait for completion
    pub async fn test_create_new_account_init_loop(
        ctx: MmArc,
        ticker: &str,
        account_id: Option<u32>,
    ) -> MmResult<HDAccountBalanceEnum, CreateAccountRpcError> {
        let req = serde_json::from_value(json!({
            "coin": ticker,
            "params": {
                "account_id": account_id
            }
        }))
        .unwrap();
        let init = init_create_new_account(ctx.clone(), req).await.unwrap();
        let timeout = wait_until_ms(150000);
        loop {
            if now_ms() > timeout {
                panic!("{} init_withdraw timed out", ticker);
            }
            let status = init_create_new_account_status(
                ctx.clone(),
                RpcTaskStatusRequest {
                    task_id: init.task_id,
                    forget_if_finished: true,
                },
            )
            .await;
            if let Ok(status) = status {
                match status {
                    RpcTaskStatus::Ok(account_balance) => break Ok(account_balance),
                    RpcTaskStatus::Error(e) => break Err(e),
                    _ => Timer::sleep(1.).await,
                }
            } else {
                panic!("{} could not get withdraw_status", ticker)
            }
        }
    }
}
