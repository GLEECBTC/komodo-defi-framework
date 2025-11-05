use crate::coin_balance::HDAddressBalance;
use crate::hd_wallet::{
    AddressDerivingError, ConfirmAddressStatus, HDConfirmAddress, HDConfirmAddressError, InvalidBip44ChainError,
    NewAddressDeriveConfirmError, NewAddressDerivingError, RpcTaskConfirmAddress,
};
use crate::utxo::UtxoCommonOps;
use crate::{
    lp_coinfind_or_err, BalanceError, CoinBalance, CoinBalanceMap, CoinFindError, CoinsContext, MmCoinEnum,
    UnexpectedDerivationMethod,
};
use async_trait::async_trait;
use common::{HttpStatusCode, SuccessResponse};
use crypto::hw_rpc_task::{
    HwConnectStatuses, HwRpcTaskAwaitingStatus, HwRpcTaskUserAction, HwRpcTaskUserActionRequest,
};
use crypto::trezor::utxo::TrezorInputScriptType;
use crypto::trezor::TrezorMessageType;
use crypto::{from_hw_error, Bip44Chain, HwError, HwRpcError, WithHwRpcError};
use derive_more::Display;
use enum_derives::EnumFromTrait;
use http::StatusCode;
use keys::AddressFormat;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use rpc_task::rpc_common::{
    CancelRpcTaskError, CancelRpcTaskRequest, InitRpcTaskResponse, RpcTaskStatusError, RpcTaskStatusRequest,
    RpcTaskUserActionError,
};
use rpc_task::{
    RpcInitReq, RpcTask, RpcTaskError, RpcTaskHandleShared, RpcTaskManager, RpcTaskManagerShared, RpcTaskStatus,
    RpcTaskTypes,
};
use std::time::Duration;

pub type GetNewAddressUserAction = HwRpcTaskUserAction;
pub type GetNewAddressAwaitingStatus = HwRpcTaskAwaitingStatus;
pub type GetNewAddressTaskManager = RpcTaskManager<InitGetNewAddressTask>;
pub type GetNewAddressTaskManagerShared = RpcTaskManagerShared<InitGetNewAddressTask>;
pub type GetNewAddressTaskHandleShared = RpcTaskHandleShared<InitGetNewAddressTask>;
pub type GetNewAddressRpcTaskStatus = RpcTaskStatus<
    GetNewAddressResponseEnum,
    GetNewAddressRpcError,
    GetNewAddressInProgressStatus,
    GetNewAddressAwaitingStatus,
>;

#[derive(Clone, Debug, Display, EnumFromTrait, PartialEq, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum GetNewAddressRpcError {
    #[display(fmt = "Hardware Wallet context is not initialized")]
    HwContextNotInitialized,
    #[display(fmt = "No such coin {coin}")]
    NoSuchCoin { coin: String },
    #[display(fmt = "RPC 'task' is awaiting '{expected}' user action")]
    UnexpectedUserAction { expected: String },
    #[display(fmt = "Coin is expected to be activated with the HD wallet derivation method")]
    CoinIsActivatedNotWithHDWallet,
    #[display(fmt = "HD account '{account_id}' is not activated")]
    UnknownAccount { account_id: u32 },
    #[display(fmt = "Coin doesn't support the given BIP44 chain: {chain:?}")]
    InvalidBip44Chain { chain: Bip44Chain },
    #[display(fmt = "Error deriving an address: {_0}")]
    ErrorDerivingAddress(String),
    #[display(fmt = "Addresses limit reached. Max number of addresses: {max_addresses_number}")]
    AddressLimitReached { max_addresses_number: u32 },
    #[display(fmt = "Empty addresses limit reached. Gap limit: {gap_limit}")]
    EmptyAddressesLimitReached { gap_limit: u32 },
    #[display(fmt = "Electrum/Native RPC invalid response: {_0}")]
    RpcInvalidResponse(String),
    #[display(fmt = "HD wallet storage error: {_0}")]
    WalletStorageError(String),
    #[display(fmt = "Failed scripthash subscription. Error: {_0}")]
    FailedScripthashSubscription(String),
    #[from_trait(WithTimeout::timeout)]
    #[display(fmt = "RPC timed out {_0:?}")]
    Timeout(Duration),
    #[from_trait(WithHwRpcError::hw_rpc_error)]
    HwError(HwRpcError),
    #[display(fmt = "Transport: {_0}")]
    Transport(String),
    #[from_trait(WithInternal::internal)]
    #[display(fmt = "Internal: {_0}")]
    Internal(String),
}

impl From<BalanceError> for GetNewAddressRpcError {
    fn from(e: BalanceError) -> Self {
        match e {
            BalanceError::Transport(transport) => GetNewAddressRpcError::Transport(transport),
            BalanceError::InvalidResponse(rpc) => GetNewAddressRpcError::RpcInvalidResponse(rpc),
            BalanceError::UnexpectedDerivationMethod(der_path) => GetNewAddressRpcError::from(der_path),
            BalanceError::WalletStorageError(internal) | BalanceError::Internal(internal) => {
                GetNewAddressRpcError::Internal(internal)
            },
            BalanceError::NoSuchCoin { coin } => GetNewAddressRpcError::NoSuchCoin { coin },
        }
    }
}

impl From<UnexpectedDerivationMethod> for GetNewAddressRpcError {
    fn from(e: UnexpectedDerivationMethod) -> Self {
        match e {
            UnexpectedDerivationMethod::ExpectedHDWallet => GetNewAddressRpcError::CoinIsActivatedNotWithHDWallet,
            unexpected_error => GetNewAddressRpcError::Internal(unexpected_error.to_string()),
        }
    }
}

impl From<CoinFindError> for GetNewAddressRpcError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => GetNewAddressRpcError::NoSuchCoin { coin },
        }
    }
}

impl From<InvalidBip44ChainError> for GetNewAddressRpcError {
    fn from(e: InvalidBip44ChainError) -> Self {
        GetNewAddressRpcError::InvalidBip44Chain { chain: e.chain }
    }
}

impl From<NewAddressDerivingError> for GetNewAddressRpcError {
    fn from(e: NewAddressDerivingError) -> Self {
        match e {
            NewAddressDerivingError::AddressLimitReached { max_addresses_number } => {
                GetNewAddressRpcError::AddressLimitReached { max_addresses_number }
            },
            NewAddressDerivingError::InvalidBip44Chain { chain } => GetNewAddressRpcError::InvalidBip44Chain { chain },
            NewAddressDerivingError::Bip32Error(bip32) => GetNewAddressRpcError::Internal(bip32),
            NewAddressDerivingError::WalletStorageError(storage) => {
                GetNewAddressRpcError::WalletStorageError(storage.to_string())
            },
            NewAddressDerivingError::Internal(internal) => GetNewAddressRpcError::Internal(internal),
        }
    }
}

impl From<AddressDerivingError> for GetNewAddressRpcError {
    fn from(e: AddressDerivingError) -> Self {
        match e {
            AddressDerivingError::InvalidBip44Chain { chain } => GetNewAddressRpcError::InvalidBip44Chain { chain },
            AddressDerivingError::Bip32Error(bip32) => GetNewAddressRpcError::ErrorDerivingAddress(bip32),
            AddressDerivingError::Internal(internal) => GetNewAddressRpcError::Internal(internal),
        }
    }
}

impl From<NewAddressDeriveConfirmError> for GetNewAddressRpcError {
    fn from(e: NewAddressDeriveConfirmError) -> Self {
        match e {
            NewAddressDeriveConfirmError::DeriveError(derive) => GetNewAddressRpcError::from(derive),
            NewAddressDeriveConfirmError::ConfirmError(confirm) => GetNewAddressRpcError::from(confirm),
        }
    }
}

impl From<HDConfirmAddressError> for GetNewAddressRpcError {
    fn from(e: HDConfirmAddressError) -> Self {
        match e {
            HDConfirmAddressError::HwContextNotInitialized => GetNewAddressRpcError::HwContextNotInitialized,
            HDConfirmAddressError::RpcTaskError(rpc) => GetNewAddressRpcError::from(rpc),
            HDConfirmAddressError::HardwareWalletError(hw) => GetNewAddressRpcError::from(hw),
            HDConfirmAddressError::InvalidAddress { expected, found } => GetNewAddressRpcError::Internal(format!(
                "Confirmation address mismatched: expected '{expected}, found '{found}''"
            )),
            HDConfirmAddressError::NoAddressReceived => {
                GetNewAddressRpcError::Internal("No address received".to_string())
            },
            HDConfirmAddressError::Internal(internal) => GetNewAddressRpcError::Internal(internal),
        }
    }
}

impl From<HwError> for GetNewAddressRpcError {
    fn from(e: HwError) -> Self {
        from_hw_error(e)
    }
}

impl From<RpcTaskError> for GetNewAddressRpcError {
    fn from(e: RpcTaskError) -> Self {
        let error = e.to_string();
        match e {
            RpcTaskError::Cancelled => GetNewAddressRpcError::Internal("Cancelled".to_owned()),
            RpcTaskError::Timeout(timeout) => GetNewAddressRpcError::Timeout(timeout),
            RpcTaskError::NoSuchTask(_) | RpcTaskError::UnexpectedTaskStatus { .. } => {
                GetNewAddressRpcError::Internal(error)
            },
            RpcTaskError::UnexpectedUserAction { expected } => GetNewAddressRpcError::UnexpectedUserAction { expected },
            RpcTaskError::Internal(internal) => GetNewAddressRpcError::Internal(internal),
        }
    }
}

impl HttpStatusCode for GetNewAddressRpcError {
    fn status_code(&self) -> StatusCode {
        match self {
            GetNewAddressRpcError::HwContextNotInitialized
            | GetNewAddressRpcError::NoSuchCoin { .. }
            | GetNewAddressRpcError::UnexpectedUserAction { .. }
            | GetNewAddressRpcError::CoinIsActivatedNotWithHDWallet
            | GetNewAddressRpcError::UnknownAccount { .. }
            | GetNewAddressRpcError::InvalidBip44Chain { .. }
            | GetNewAddressRpcError::ErrorDerivingAddress(_)
            | GetNewAddressRpcError::AddressLimitReached { .. }
            | GetNewAddressRpcError::EmptyAddressesLimitReached { .. } => StatusCode::BAD_REQUEST,
            GetNewAddressRpcError::Transport(_)
            | GetNewAddressRpcError::RpcInvalidResponse(_)
            | GetNewAddressRpcError::WalletStorageError(_)
            | GetNewAddressRpcError::FailedScripthashSubscription(_)
            | GetNewAddressRpcError::HwError(_)
            | GetNewAddressRpcError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            GetNewAddressRpcError::Timeout(_) => StatusCode::REQUEST_TIMEOUT,
        }
    }
}

#[derive(Deserialize, Clone)]
pub struct GetNewAddressRequest {
    coin: String,
    #[serde(flatten)]
    params: GetNewAddressParams,
}

#[derive(Clone, Deserialize)]
pub struct GetNewAddressParams {
    pub(crate) account_id: u32,
    pub(crate) chain: Option<Bip44Chain>,
    // The max number of empty addresses in a row.
    // If there are more or equal to the `gap_limit` last empty addresses in a row,
    // we'll not allow to generate new address.
    pub(crate) gap_limit: Option<u32>,
}

/// Generic response for the `get_new_address` RPC command.
#[derive(Clone, Debug, Serialize)]
pub struct GetNewAddressResponse<BalanceObject> {
    new_address: HDAddressBalance<BalanceObject>,
}

/// Enum for the response of the `get_new_address` RPC command.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum GetNewAddressResponseEnum {
    Single(GetNewAddressResponse<CoinBalance>),
    Map(GetNewAddressResponse<CoinBalanceMap>),
}

#[derive(Clone, Serialize)]
pub enum GetNewAddressInProgressStatus {
    Preparing,
    RequestingAccountBalance,
    Finishing,
    /// The following statuses don't require the user to send `UserAction`,
    /// but they tell the user that he should confirm/decline the operation on his device.
    WaitingForTrezorToConnect,
    FollowHwDeviceInstructions,
    ConfirmAddress {
        expected_address: String,
    },
}

impl ConfirmAddressStatus for GetNewAddressInProgressStatus {
    fn confirm_addr_status(expected_address: String) -> Self {
        GetNewAddressInProgressStatus::ConfirmAddress { expected_address }
    }
}

/// A trait for the `get_new_address` RPC commands.
#[async_trait]
pub trait GetNewAddressRpcOps {
    type BalanceObject;

    /// Generates a new address.
    /// TODO remove once GUI integrates `task::get_new_address::init`.
    async fn get_new_address_rpc_without_conf(
        &self,
        params: GetNewAddressParams,
    ) -> MmResult<GetNewAddressResponse<Self::BalanceObject>, GetNewAddressRpcError>;

    /// Generates and asks the user to confirm a new address.
    async fn get_new_address_rpc<ConfirmAddress>(
        &self,
        params: GetNewAddressParams,
        confirm_address: &ConfirmAddress,
    ) -> MmResult<GetNewAddressResponse<Self::BalanceObject>, GetNewAddressRpcError>
    where
        ConfirmAddress: HDConfirmAddress;
}

#[derive(Clone)]
pub struct InitGetNewAddressTask {
    ctx: MmArc,
    coin: MmCoinEnum,
    req: GetNewAddressRequest,
}

impl RpcTaskTypes for InitGetNewAddressTask {
    type Item = GetNewAddressResponseEnum;
    type Error = GetNewAddressRpcError;
    type InProgressStatus = GetNewAddressInProgressStatus;
    type AwaitingStatus = GetNewAddressAwaitingStatus;
    type UserAction = GetNewAddressUserAction;
}

#[async_trait]
impl RpcTask for InitGetNewAddressTask {
    fn initial_status(&self) -> Self::InProgressStatus {
        GetNewAddressInProgressStatus::Preparing
    }

    // Do nothing if the task has been cancelled.
    async fn cancel(self) {}

    async fn run(&mut self, task_handle: RpcTaskHandleShared<Self>) -> Result<Self::Item, MmError<Self::Error>> {
        /// Caller to get and confirm a new HD address for an HD wallet
        async fn get_new_address_helper<Coin>(
            ctx: &MmArc,
            coin: &Coin,
            params: GetNewAddressParams,
            task_handle: GetNewAddressTaskHandleShared,
            trezor_message_type: TrezorMessageType,
            trezor_script_type: Option<TrezorInputScriptType>,
        ) -> MmResult<GetNewAddressResponse<<Coin as GetNewAddressRpcOps>::BalanceObject>, GetNewAddressRpcError>
        where
            Coin: GetNewAddressRpcOps + Send + Sync,
        {
            let hw_statuses = HwConnectStatuses {
                on_connect: GetNewAddressInProgressStatus::WaitingForTrezorToConnect,
                on_connected: GetNewAddressInProgressStatus::Preparing,
                on_connection_failed: GetNewAddressInProgressStatus::Finishing,
                on_button_request: GetNewAddressInProgressStatus::FollowHwDeviceInstructions,
                on_pin_request: GetNewAddressAwaitingStatus::EnterTrezorPin,
                on_passphrase_request: GetNewAddressAwaitingStatus::EnterTrezorPassphrase,
                on_ready: GetNewAddressInProgressStatus::RequestingAccountBalance,
            };
            let confirm_address: RpcTaskConfirmAddress<InitGetNewAddressTask> =
                RpcTaskConfirmAddress::new(ctx, task_handle, hw_statuses, trezor_message_type, trezor_script_type)
                    .map_mm_err()?;
            coin.get_new_address_rpc(params, &confirm_address).await
        }

        match self.coin {
            MmCoinEnum::UtxoCoin(ref utxo) => {
                // Set script type to enable Trezor to correctly validate the derivation path
                let trezor_script_type = match utxo.addr_format() {
                    AddressFormat::Standard | AddressFormat::CashAddress { .. } => {
                        Some(TrezorInputScriptType::SpendAddress)
                    },
                    AddressFormat::Segwit => Some(TrezorInputScriptType::SpendWitness),
                };
                Ok(GetNewAddressResponseEnum::Map(
                    get_new_address_helper(
                        &self.ctx,
                        utxo,
                        self.req.params.clone(),
                        task_handle,
                        TrezorMessageType::Bitcoin,
                        trezor_script_type,
                    )
                    .await?,
                ))
            },
            MmCoinEnum::QtumCoin(ref qtum) => {
                // Set script type to enable Trezor to correctly validate the derivation path
                let trezor_script_type = match qtum.addr_format() {
                    AddressFormat::Standard | AddressFormat::CashAddress { .. } => {
                        Some(TrezorInputScriptType::SpendAddress)
                    },
                    AddressFormat::Segwit => Some(TrezorInputScriptType::SpendWitness),
                };
                Ok(GetNewAddressResponseEnum::Map(
                    get_new_address_helper(
                        &self.ctx,
                        qtum,
                        self.req.params.clone(),
                        task_handle,
                        TrezorMessageType::Bitcoin,
                        trezor_script_type,
                    )
                    .await?,
                ))
            },
            MmCoinEnum::EthCoin(ref eth) => Ok(GetNewAddressResponseEnum::Map(
                get_new_address_helper(
                    &self.ctx,
                    eth,
                    self.req.params.clone(),
                    task_handle,
                    TrezorMessageType::Ethereum,
                    None,
                )
                .await?,
            )),
            _ => MmError::err(GetNewAddressRpcError::CoinIsActivatedNotWithHDWallet),
        }
    }
}

/// Generates a new address.
pub async fn get_new_address(
    ctx: MmArc,
    req: GetNewAddressRequest,
) -> MmResult<GetNewAddressResponseEnum, GetNewAddressRpcError> {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    match coin {
        MmCoinEnum::UtxoCoin(utxo) => Ok(GetNewAddressResponseEnum::Map(
            utxo.get_new_address_rpc_without_conf(req.params).await?,
        )),
        MmCoinEnum::QtumCoin(qtum) => Ok(GetNewAddressResponseEnum::Map(
            qtum.get_new_address_rpc_without_conf(req.params).await?,
        )),
        MmCoinEnum::EthCoin(eth) => Ok(GetNewAddressResponseEnum::Map(
            eth.get_new_address_rpc_without_conf(req.params).await?,
        )),
        _ => MmError::err(GetNewAddressRpcError::CoinIsActivatedNotWithHDWallet),
    }
}

/// Generates a new address.
/// TODO remove once GUI integrates `task::get_new_address::init`.
pub async fn init_get_new_address(
    ctx: MmArc,
    req: RpcInitReq<GetNewAddressRequest>,
) -> MmResult<InitRpcTaskResponse, GetNewAddressRpcError> {
    let (client_id, req) = (req.client_id, req.inner);
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(GetNewAddressRpcError::Internal)?;
    let spawner = coin.spawner();
    let task = InitGetNewAddressTask { ctx, coin, req };
    let task_id =
        GetNewAddressTaskManager::spawn_rpc_task(&coins_ctx.get_new_address_manager, &spawner, task, client_id)
            .map_mm_err()?;
    Ok(InitRpcTaskResponse { task_id })
}

pub async fn init_get_new_address_status(
    ctx: MmArc,
    req: RpcTaskStatusRequest,
) -> MmResult<GetNewAddressRpcTaskStatus, RpcTaskStatusError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(RpcTaskStatusError::Internal)?;
    let mut task_manager = coins_ctx
        .get_new_address_manager
        .lock()
        .map_to_mm(|e| RpcTaskStatusError::Internal(e.to_string()))?;
    task_manager
        .task_status(req.task_id, req.forget_if_finished)
        .or_mm_err(|| RpcTaskStatusError::NoSuchTask(req.task_id))
}

pub async fn init_get_new_address_user_action(
    ctx: MmArc,
    req: HwRpcTaskUserActionRequest,
) -> MmResult<SuccessResponse, RpcTaskUserActionError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(RpcTaskUserActionError::Internal)?;
    let mut task_manager = coins_ctx
        .get_new_address_manager
        .lock()
        .map_to_mm(|e| RpcTaskUserActionError::Internal(e.to_string()))?;
    task_manager.on_user_action(req.task_id, req.user_action).map_mm_err()?;
    Ok(SuccessResponse::new())
}

pub async fn cancel_get_new_address(
    ctx: MmArc,
    req: CancelRpcTaskRequest,
) -> MmResult<SuccessResponse, CancelRpcTaskError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(CancelRpcTaskError::Internal)?;
    let mut task_manager = coins_ctx
        .get_new_address_manager
        .lock()
        .map_to_mm(|e| CancelRpcTaskError::Internal(e.to_string()))?;
    task_manager.cancel_task(req.task_id).map_mm_err()?;
    Ok(SuccessResponse::new())
}

pub(crate) mod common_impl {
    use super::*;
    use crate::coin_balance::{HDAddressBalanceScanner, HDWalletBalanceObject, HDWalletBalanceOps};
    use crate::hd_wallet::{DisplayAddress, HDAccountOps, HDAddressOps, HDCoinAddress, HDCoinHDAccount, HDWalletOps};
    use crate::CoinWithDerivationMethod;
    use crypto::RpcDerivationPath;
    use std::collections::HashSet;
    use std::fmt;
    use std::fmt::Display;
    use std::hash::Hash;
    use std::ops::DerefMut;

    /// TODO remove once GUI integrates `task::get_new_address::init`.
    pub async fn get_new_address_rpc_without_conf<Coin>(
        coin: &Coin,
        params: GetNewAddressParams,
    ) -> MmResult<GetNewAddressResponse<HDWalletBalanceObject<Coin>>, GetNewAddressRpcError>
    where
        Coin: HDWalletBalanceOps + CoinWithDerivationMethod + Sync + Send,
        HDCoinAddress<Coin>: fmt::Display,
    {
        let hd_wallet = coin.derivation_method().hd_wallet_or_err().map_mm_err()?;

        let account_id = params.account_id;
        let mut hd_account = hd_wallet
            .get_account_mut(account_id)
            .await
            .or_mm_err(|| GetNewAddressRpcError::UnknownAccount { account_id })?;

        let chain = params.chain.unwrap_or_else(|| hd_wallet.default_receiver_chain());
        let gap_limit = params.gap_limit.unwrap_or_else(|| hd_wallet.gap_limit());

        // Check if we can generate new address.
        check_if_can_get_new_address(coin, &hd_account, chain, gap_limit).await?;

        let hd_address = coin
            .generate_new_address(hd_wallet, hd_account.deref_mut(), chain)
            .await
            .map_mm_err()?;
        let address = hd_address.address();
        let balance = coin.known_address_balance(&address).await.map_mm_err()?;

        Ok(GetNewAddressResponse {
            new_address: HDAddressBalance {
                address: address.display_address(),
                derivation_path: RpcDerivationPath(hd_address.derivation_path().clone()),
                chain,
                balance,
            },
        })
    }

    pub async fn get_new_address_rpc<Coin, ConfirmAddress>(
        coin: &Coin,
        params: GetNewAddressParams,
        confirm_address: &ConfirmAddress,
    ) -> MmResult<GetNewAddressResponse<HDWalletBalanceObject<Coin>>, GetNewAddressRpcError>
    where
        ConfirmAddress: HDConfirmAddress,
        Coin: HDWalletBalanceOps + CoinWithDerivationMethod + Send + Sync,
        HDCoinAddress<Coin>: Display + Eq + Hash,
    {
        let hd_wallet = coin.derivation_method().hd_wallet_or_err().map_mm_err()?;

        let account_id = params.account_id;
        let mut hd_account = hd_wallet
            .get_account_mut(account_id)
            .await
            .or_mm_err(|| GetNewAddressRpcError::UnknownAccount { account_id })?;

        let chain = params.chain.unwrap_or_else(|| hd_wallet.default_receiver_chain());
        let gap_limit = params.gap_limit.unwrap_or_else(|| hd_wallet.gap_limit());

        // Check if we can generate new address.
        check_if_can_get_new_address(coin, &hd_account, chain, gap_limit).await?;

        let hd_address = coin
            .generate_and_confirm_new_address(hd_wallet, &mut hd_account, chain, confirm_address)
            .await
            .map_mm_err()?;
        let address = hd_address.address();
        let balance = coin.known_address_balance(&address).await.map_mm_err()?;

        let formatted_address = address.display_address();
        coin.prepare_addresses_for_balance_stream_if_enabled(HashSet::from([formatted_address.clone()]))
            .await
            .map_err(|e| GetNewAddressRpcError::FailedScripthashSubscription(e.to_string()))?;

        Ok(GetNewAddressResponse {
            new_address: HDAddressBalance {
                address: formatted_address,
                derivation_path: RpcDerivationPath(hd_address.derivation_path().clone()),
                chain,
                balance,
            },
        })
    }

    async fn check_if_can_get_new_address<Coin>(
        coin: &Coin,
        hd_account: &HDCoinHDAccount<Coin>,
        chain: Bip44Chain,
        gap_limit: u32,
    ) -> MmResult<(), GetNewAddressRpcError>
    where
        Coin: HDWalletBalanceOps + Sync,
        HDCoinAddress<Coin>: fmt::Display,
    {
        let known_addresses_number = hd_account.known_addresses_number(chain).map_mm_err()?;
        if known_addresses_number == 0 || gap_limit > known_addresses_number {
            return Ok(());
        }

        let max_addresses_number = hd_account.address_limit();
        if known_addresses_number >= max_addresses_number {
            return MmError::err(GetNewAddressRpcError::AddressLimitReached { max_addresses_number });
        }

        let address_scanner = coin.produce_hd_address_scanner().await.map_mm_err()?;

        // Address IDs start from 0, so the `last_known_address_id = known_addresses_number - 1`.
        // At this point we are sure that `known_addresses_number > 0`.
        let last_address_id = known_addresses_number - 1;

        for address_id in (0..=last_address_id).rev() {
            let address = coin
                .derive_address(hd_account, chain, address_id)
                .await
                .map_mm_err()?
                .address();
            if address_scanner.is_address_used(&address).await.map_mm_err()? {
                return Ok(());
            }

            let empty_addresses_number = last_address_id - address_id + 1;
            if empty_addresses_number >= gap_limit {
                // We already have `gap_limit` empty addresses.
                return MmError::err(GetNewAddressRpcError::EmptyAddressesLimitReached { gap_limit });
            }
        }

        Ok(())
    }
}
