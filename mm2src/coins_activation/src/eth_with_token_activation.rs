use crate::context::CoinsActivationContext;
use crate::platform_coin_with_tokens::{
    platform_coin_xpub_extractor_rpc_statuses, EnablePlatformCoinWithTokensError, GetPlatformBalance,
    InitPlatformCoinWithTokensAwaitingStatus, InitPlatformCoinWithTokensInProgressStatus,
    InitPlatformCoinWithTokensTaskManagerShared, InitPlatformCoinWithTokensUserAction, InitTokensAsMmCoinsError,
    PlatformCoinWithTokensActivationOps, RegisterTokenInfo, TokenActivationParams, TokenActivationRequest,
    TokenAsMmCoinInitializer, TokenInitializer, TokenOf,
};
use crate::prelude::*;
use async_trait::async_trait;
use coins::coin_balance::{CoinBalanceReport, EnableCoinBalanceOps};
use coins::eth::v2_activation::{
    eth_coin_from_conf_and_request_v2, Erc20Protocol, Erc20TokenActivationRequest, EthActivationV2Error,
    EthActivationV2Request, EthPrivKeyActivationPolicy, EthTokenActivationError, NftActivationRequest, NftProviderEnum,
};
use coins::eth::{ChainSpec, Erc20TokenDetails, EthCoin, EthCoinType, EthPrivKeyBuildPolicy};
use coins::hd_wallet::{DisplayAddress, RpcTaskXPubExtractor};
use coins::my_tx_history_v2::TxHistoryStorage;
use coins::nft::nft_structs::NftInfo;
use coins::{
    CoinBalance, CoinBalanceMap, CoinProtocol, CoinWithDerivationMethod, DerivationMethod, MarketCoinOps, MmCoin,
    MmCoinEnum,
};

use crate::platform_coin_with_tokens::InitPlatformCoinWithTokensTask;
use common::Future01CompatExt;
use common::{drop_mutability, true_f};
use crypto::HwRpcError;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
#[cfg(target_arch = "wasm32")]
use mm2_metamask::MetamaskRpcError;
use mm2_number::BigDecimal;
use rpc_task::RpcTaskHandleShared;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::{HashMap, HashSet};

pub type EthTaskManagerShared = InitPlatformCoinWithTokensTaskManagerShared<EthCoin>;

impl From<EthActivationV2Error> for EnablePlatformCoinWithTokensError {
    fn from(err: EthActivationV2Error) -> Self {
        match err {
            EthActivationV2Error::InvalidPayload(e)
            | EthActivationV2Error::InvalidSwapContractAddr(e)
            | EthActivationV2Error::InvalidFallbackSwapContract(e)
            | EthActivationV2Error::ErrorDeserializingDerivationPath(e)
            | EthActivationV2Error::InvalidPathToAddress(e) => EnablePlatformCoinWithTokensError::InvalidPayload(e),
            EthActivationV2Error::ChainIdNotSet => {
                EnablePlatformCoinWithTokensError::Internal("`chain_id` is not set in coin config".to_string())
            },
            EthActivationV2Error::UnsupportedChain { .. } => {
                EnablePlatformCoinWithTokensError::Internal("Unsupported chain".to_string())
            },
            EthActivationV2Error::ActivationFailed { ticker, error } => {
                EnablePlatformCoinWithTokensError::PlatformCoinCreationError { ticker, error }
            },
            EthActivationV2Error::AtLeastOneNodeRequired => EnablePlatformCoinWithTokensError::AtLeastOneNodeRequired(
                "Enable request for ETH coin must have at least 1 node".to_string(),
            ),
            EthActivationV2Error::CouldNotFetchBalance(e) | EthActivationV2Error::UnreachableNodes(e) => {
                EnablePlatformCoinWithTokensError::Transport(e)
            },
            EthActivationV2Error::PrivKeyPolicyNotAllowed(e) => {
                EnablePlatformCoinWithTokensError::PrivKeyPolicyNotAllowed(e)
            },
            EthActivationV2Error::FailedSpawningBalanceEvents(e) => {
                EnablePlatformCoinWithTokensError::FailedSpawningBalanceEvents(e)
            },
            EthActivationV2Error::HDWalletStorageError(e) => EnablePlatformCoinWithTokensError::Internal(e),
            EthActivationV2Error::WalletConnectError(e) => EnablePlatformCoinWithTokensError::WalletConnectError(e),
            #[cfg(target_arch = "wasm32")]
            EthActivationV2Error::MetamaskError(metamask) => {
                EnablePlatformCoinWithTokensError::Transport(metamask.to_string())
            },
            EthActivationV2Error::InternalError(e) => EnablePlatformCoinWithTokensError::Internal(e),
            EthActivationV2Error::Transport(e) => EnablePlatformCoinWithTokensError::Transport(e),
            EthActivationV2Error::UnexpectedDerivationMethod(e) => {
                EnablePlatformCoinWithTokensError::UnexpectedDerivationMethod(e.to_string())
            },
            EthActivationV2Error::HwContextNotInitialized => {
                EnablePlatformCoinWithTokensError::Internal("Hardware wallet is not initalised".to_string())
            },
            EthActivationV2Error::CoinDoesntSupportTrezor => {
                EnablePlatformCoinWithTokensError::Internal("Coin does not support Trezor wallet".to_string())
            },
            EthActivationV2Error::TaskTimedOut { .. } => {
                EnablePlatformCoinWithTokensError::Internal("Coin activation timed out".to_string())
            },
            EthActivationV2Error::HwError(e) => EnablePlatformCoinWithTokensError::Internal(e.to_string()),
            EthActivationV2Error::InvalidHardwareWalletCall => EnablePlatformCoinWithTokensError::Internal(
                "Hardware wallet must be used within rpc task manager".to_string(),
            ),
            EthActivationV2Error::CustomTokenError(e) => EnablePlatformCoinWithTokensError::CustomTokenError(e),
            EthActivationV2Error::PlatformCoinMismatch => EnablePlatformCoinWithTokensError::PlatformCoinMismatch,
        }
    }
}

impl TryFromCoinProtocol for ChainSpec {
    fn try_from_coin_protocol(proto: CoinProtocol) -> Result<Self, MmError<CoinProtocol>>
    where
        Self: Sized,
    {
        match proto {
            CoinProtocol::ETH { chain_id } => Ok(ChainSpec::Evm { chain_id }),
            CoinProtocol::TRX { network } => Ok(ChainSpec::Tron { network }),
            protocol => MmError::err(protocol),
        }
    }
}

pub struct Erc20Initializer {
    platform_coin: EthCoin,
}

impl From<EthTokenActivationError> for InitTokensAsMmCoinsError {
    fn from(error: EthTokenActivationError) -> Self {
        match error {
            EthTokenActivationError::InternalError(e) => InitTokensAsMmCoinsError::Internal(e),
            EthTokenActivationError::CouldNotFetchBalance(e) | EthTokenActivationError::ClientConnectionFailed(e) => {
                InitTokensAsMmCoinsError::CouldNotFetchBalance(e)
            },
            EthTokenActivationError::InvalidPayload(e) => InitTokensAsMmCoinsError::InvalidPayload(e),
            EthTokenActivationError::Transport(e) => InitTokensAsMmCoinsError::Transport(e),
            EthTokenActivationError::UnexpectedDerivationMethod(e) => {
                InitTokensAsMmCoinsError::UnexpectedDerivationMethod(e)
            },
            EthTokenActivationError::PrivKeyPolicyNotAllowed(e) => InitTokensAsMmCoinsError::Internal(e.to_string()),
            EthTokenActivationError::CustomTokenError(e) => InitTokensAsMmCoinsError::CustomTokenError(e),
            EthTokenActivationError::PlatformCoinMismatch => InitTokensAsMmCoinsError::PlatformCoinMismatch,
        }
    }
}

#[async_trait]
impl TokenInitializer for Erc20Initializer {
    type Token = EthCoin;
    type TokenActivationRequest = Erc20TokenActivationRequest;
    type TokenProtocol = Erc20Protocol;
    type InitTokensError = EthTokenActivationError;

    fn tokens_requests_from_platform_request(
        platform_params: &EthWithTokensActivationRequest,
    ) -> Vec<TokenActivationRequest<Self::TokenActivationRequest>> {
        platform_params.erc20_tokens_requests.clone()
    }

    async fn enable_tokens(
        &self,
        activation_params: Vec<TokenActivationParams<Erc20TokenActivationRequest, Erc20Protocol>>,
    ) -> Result<Vec<EthCoin>, MmError<EthTokenActivationError>> {
        let mut tokens = Vec::with_capacity(activation_params.len());
        for param in activation_params {
            let token: EthCoin = self
                .platform_coin
                .initialize_erc20_token(
                    param.ticker,
                    param.activation_request,
                    param.conf,
                    param.protocol,
                    param.is_custom,
                )
                .await?;
            tokens.push(token);
        }

        Ok(tokens)
    }

    fn platform_coin(&self) -> &EthCoin {
        &self.platform_coin
    }

    fn validate_token_params(
        &self,
        params: &[TokenActivationParams<Self::TokenActivationRequest, Self::TokenProtocol>],
    ) -> MmResult<(), Self::InitTokensError> {
        for token_param in params {
            match &token_param.protocol {
                Erc20Protocol { platform, .. } if platform == self.platform_coin().ticker() => {},
                _ => return MmError::err(EthTokenActivationError::PlatformCoinMismatch),
            }
        }
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
pub struct EthWithTokensActivationRequest {
    #[serde(flatten)]
    platform_request: EthActivationV2Request,
    erc20_tokens_requests: Vec<TokenActivationRequest<Erc20TokenActivationRequest>>,
    #[serde(default = "true_f")]
    pub get_balances: bool,
    nft_req: Option<NftActivationRequest>,
}

impl TxHistory for EthWithTokensActivationRequest {
    fn tx_history(&self) -> bool {
        false
    }
}

impl ActivationRequestInfo for EthWithTokensActivationRequest {
    fn is_hw_policy(&self) -> bool {
        self.platform_request.priv_key_policy.is_hw_policy()
    }
}

impl TokenOf for EthCoin {
    type PlatformCoin = EthCoin;
}

impl RegisterTokenInfo<EthCoin> for EthCoin {
    fn register_token_info(&self, token: &EthCoin) {
        // Dont register Nft in platform coin.
        if matches!(token.coin_type, EthCoinType::Nft { .. }) {
            return;
        }

        self.add_erc_token_info(
            token.ticker().to_string(),
            Erc20TokenDetails {
                token_address: token.erc20_token_address().unwrap(),
                decimals: token.decimals(),
            },
        );
    }
}

/// Activation result for activating an EVM-based coin along with its associated tokens (ERC20 and NFTs) for Iguana wallets.
#[derive(Serialize, Clone)]
pub struct IguanaEthWithTokensActivationResult {
    current_block: u64,
    eth_addresses_infos: HashMap<String, CoinAddressInfo<CoinBalance>>,
    erc20_addresses_infos: HashMap<String, CoinAddressInfo<TokenBalances>>,
    nfts_infos: HashMap<String, NftInfo>,
}

/// Activation result for activating an EVM-based coin along with its associated tokens (ERC20 and NFTs) for HD wallets.
#[derive(Serialize, Clone)]
pub struct HDEthWithTokensActivationResult {
    current_block: u64,
    ticker: String,
    wallet_balance: CoinBalanceReport<CoinBalanceMap>,
    // Todo: Move to wallet_balance when implementing HDWallet for NFTs
    nfts_infos: HashMap<String, NftInfo>,
}

/// Represents the result of activating an Ethereum-based coin along with its associated tokens (ERC20 and NFTs).
///
/// This structure provides a snapshot of the relevant activation data, including the current blockchain block,
/// information about Ethereum addresses and their balances, ERC-20 token balances, and a summary of NFT ownership.
#[derive(Serialize, Clone)]
#[serde(untagged)]
pub enum EthWithTokensActivationResult {
    Iguana(IguanaEthWithTokensActivationResult),
    HD(HDEthWithTokensActivationResult),
}

impl GetPlatformBalance for EthWithTokensActivationResult {
    fn get_platform_balance(&self) -> Option<BigDecimal> {
        match self {
            EthWithTokensActivationResult::Iguana(result) => result
                .eth_addresses_infos
                .iter()
                .try_fold(BigDecimal::from(0), |total, (_, addr_info)| {
                    addr_info.balances.as_ref().map(|b| total + b.get_total())
                }),
            EthWithTokensActivationResult::HD(result) => result
                .wallet_balance
                .to_addresses_total_balances(&result.ticker)
                .iter()
                .fold(None, |maybe_total, (_, maybe_balance)| {
                    match (maybe_total, maybe_balance) {
                        (Some(total), Some(balance)) => Some(total + balance),
                        (None, Some(balance)) => Some(balance.clone()),
                        (total, None) => total,
                    }
                }),
        }
    }
}

impl CurrentBlock for EthWithTokensActivationResult {
    fn current_block(&self) -> u64 {
        match self {
            EthWithTokensActivationResult::Iguana(result) => result.current_block,
            EthWithTokensActivationResult::HD(result) => result.current_block,
        }
    }
}

#[async_trait]
impl PlatformCoinWithTokensActivationOps for EthCoin {
    type ActivationRequest = EthWithTokensActivationRequest;
    type PlatformProtocolInfo = ChainSpec;
    type ActivationResult = EthWithTokensActivationResult;
    type ActivationError = EthActivationV2Error;

    type InProgressStatus = InitPlatformCoinWithTokensInProgressStatus;
    type AwaitingStatus = InitPlatformCoinWithTokensAwaitingStatus;
    type UserAction = InitPlatformCoinWithTokensUserAction;

    async fn enable_platform_coin(
        ctx: MmArc,
        ticker: String,
        platform_conf: &Json,
        activation_request: Self::ActivationRequest,
        protocol: Self::PlatformProtocolInfo,
    ) -> Result<Self, MmError<Self::ActivationError>> {
        let priv_key_policy =
            eth_priv_key_build_policy(&ctx, &activation_request.platform_request.priv_key_policy).await?;

        let platform_coin = eth_coin_from_conf_and_request_v2(
            &ctx,
            &ticker,
            platform_conf,
            activation_request.platform_request,
            priv_key_policy,
            protocol,
        )
        .await?;

        Ok(platform_coin)
    }

    async fn enable_global_nft(
        &self,
        activation_request: &Self::ActivationRequest,
    ) -> Result<Option<MmCoinEnum>, MmError<Self::ActivationError>> {
        let (url, proxy_auth) = match &activation_request.nft_req {
            Some(nft_req) => match &nft_req.provider {
                NftProviderEnum::Moralis { url, komodo_proxy } => (url, *komodo_proxy),
            },
            None => return Ok(None),
        };
        let nft_global = self.initialize_global_nft(url, proxy_auth).await.map_mm_err()?;
        Ok(Some(MmCoinEnum::EthCoin(nft_global)))
    }

    fn try_from_mm_coin(coin: MmCoinEnum) -> Option<Self>
    where
        Self: Sized,
    {
        match coin {
            MmCoinEnum::EthCoin(coin) => Some(coin),
            _ => None,
        }
    }

    fn token_initializers(
        &self,
    ) -> Vec<Box<dyn TokenAsMmCoinInitializer<PlatformCoin = Self, ActivationRequest = Self::ActivationRequest>>> {
        vec![Box::new(Erc20Initializer {
            platform_coin: self.clone(),
        })]
    }

    async fn get_activation_result(
        &self,
        task_handle: Option<RpcTaskHandleShared<InitPlatformCoinWithTokensTask<EthCoin>>>,
        activation_request: &Self::ActivationRequest,
        nft_global: &Option<MmCoinEnum>,
    ) -> Result<EthWithTokensActivationResult, MmError<EthActivationV2Error>> {
        let current_block = self
            .current_block()
            .compat()
            .await
            .map_err(EthActivationV2Error::InternalError)?;

        let nfts_map = if let Some(MmCoinEnum::EthCoin(nft_global)) = nft_global {
            nft_global.nfts_infos.lock().await.clone()
        } else {
            Default::default()
        };

        match self.derivation_method() {
            DerivationMethod::SingleAddress(my_address) => {
                let pubkey = self.get_public_key().await.map_mm_err()?;
                let mut eth_address_info = CoinAddressInfo {
                    derivation_method: self.derivation_method().to_response().await.map_mm_err()?,
                    pubkey: pubkey.clone(),
                    balances: None,
                    tickers: None,
                };
                let mut erc20_address_info = CoinAddressInfo {
                    derivation_method: self.derivation_method().to_response().await.map_mm_err()?,
                    pubkey,
                    balances: None,
                    tickers: None,
                };
                // Todo: make get_balances work with HDWallet if it's needed
                if !activation_request.get_balances {
                    drop_mutability!(eth_address_info);
                    let tickers: HashSet<_> = self.get_erc_tokens_infos().into_keys().collect();
                    erc20_address_info.tickers = Some(tickers);
                    drop_mutability!(erc20_address_info);

                    return Ok(EthWithTokensActivationResult::Iguana(
                        IguanaEthWithTokensActivationResult {
                            current_block,
                            eth_addresses_infos: HashMap::from([(my_address.display_address(), eth_address_info)]),
                            erc20_addresses_infos: HashMap::from([(my_address.display_address(), erc20_address_info)]),
                            nfts_infos: nfts_map,
                        },
                    ));
                }

                let eth_balance = self
                    .my_balance()
                    .compat()
                    .await
                    .map_err(|e| EthActivationV2Error::CouldNotFetchBalance(e.to_string()))?;
                eth_address_info.balances = Some(eth_balance);
                drop_mutability!(eth_address_info);

                let token_balances = self
                    .get_tokens_balance_list()
                    .await
                    .map_err(|e| EthActivationV2Error::CouldNotFetchBalance(e.to_string()))?;
                erc20_address_info.balances = Some(token_balances);
                drop_mutability!(erc20_address_info);

                Ok(EthWithTokensActivationResult::Iguana(
                    IguanaEthWithTokensActivationResult {
                        current_block,
                        eth_addresses_infos: HashMap::from([(my_address.display_address(), eth_address_info)]),
                        erc20_addresses_infos: HashMap::from([(my_address.display_address(), erc20_address_info)]),
                        nfts_infos: nfts_map,
                    },
                ))
            },
            DerivationMethod::HDWallet(_) => {
                let xpub_extractor = if self.is_trezor() {
                    let ctx = MmArc::from_weak(&self.ctx).ok_or(EthActivationV2Error::InvalidHardwareWalletCall)?;
                    let task_handle = task_handle.ok_or_else(|| {
                        EthActivationV2Error::InternalError(
                            "Hardware wallet must be accessed under task manager".to_string(),
                        )
                    })?;
                    Some(
                        RpcTaskXPubExtractor::new_trezor_extractor(
                            &ctx,
                            task_handle,
                            platform_coin_xpub_extractor_rpc_statuses(),
                            // Todo: add support for Tron by checking self.chain_spec
                            CoinProtocol::ETH {
                                chain_id: self.chain_id().ok_or_else(|| {
                                    EthActivationV2Error::InternalError(
                                        "chain_id should be available for an EVM coin".to_string(),
                                    )
                                })?,
                            },
                        )
                        .map_err(|_| MmError::new(EthActivationV2Error::HwError(HwRpcError::NotInitialized)))?,
                    )
                } else {
                    None
                };

                let wallet_balance = self
                    .enable_coin_balance(
                        xpub_extractor,
                        activation_request.platform_request.enable_params.clone(),
                        &activation_request.platform_request.path_to_address,
                    )
                    .await
                    .map_mm_err()?;

                Ok(EthWithTokensActivationResult::HD(HDEthWithTokensActivationResult {
                    current_block,
                    ticker: self.ticker().to_string(),
                    wallet_balance,
                    nfts_infos: nfts_map,
                }))
            },
        }
    }

    fn start_history_background_fetching(
        &self,
        _ctx: MmArc,
        _storage: impl TxHistoryStorage + 'static,
        _initial_balance: Option<BigDecimal>,
    ) {
    }

    fn rpc_task_manager(
        activation_ctx: &CoinsActivationContext,
    ) -> &InitPlatformCoinWithTokensTaskManagerShared<EthCoin> {
        &activation_ctx.init_eth_task_manager
    }
}

async fn eth_priv_key_build_policy(
    ctx: &MmArc,
    activation_policy: &EthPrivKeyActivationPolicy,
) -> MmResult<EthPrivKeyBuildPolicy, EthActivationV2Error> {
    match activation_policy {
        EthPrivKeyActivationPolicy::ContextPrivKey => {
            Ok(EthPrivKeyBuildPolicy::detect_priv_key_policy(ctx).map_mm_err()?)
        },
        #[cfg(target_arch = "wasm32")]
        EthPrivKeyActivationPolicy::Metamask => {
            let metamask_ctx = crypto::CryptoCtx::from_ctx(ctx)
                .map_mm_err()?
                .metamask_ctx()
                .or_mm_err(|| EthActivationV2Error::MetamaskError(MetamaskRpcError::MetamaskCtxNotInitialized))?;
            Ok(EthPrivKeyBuildPolicy::Metamask(metamask_ctx))
        },
        EthPrivKeyActivationPolicy::Trezor => Ok(EthPrivKeyBuildPolicy::Trezor),
        EthPrivKeyActivationPolicy::WalletConnect { session_topic } => Ok(EthPrivKeyBuildPolicy::WalletConnect {
            session_topic: session_topic.clone(),
        }),
    }
}
