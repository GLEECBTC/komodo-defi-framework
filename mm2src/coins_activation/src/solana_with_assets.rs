#![allow(unused_variables)]

use std::collections::HashMap;

use async_trait::async_trait;
use coins::{
    my_tx_history_v2::TxHistoryStorage,
    solana::{
        RpcNode, SolanaCoin, SolanaInitError, SolanaInitErrorKind, SolanaProtocolInfo, SolanaToken,
        SolanaTokenInitError, SolanaTokenInitErrorKind, SolanaTokenProtocolInfo,
    },
    CoinBalance, CoinProtocol, MarketCoinOps, MmCoinEnum, PrivKeyBuildPolicy,
};
use common::Future01CompatExt;
use futures::future::join_all;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::BigDecimal;
use rpc_task::RpcTaskHandleShared;
use serde::{Deserialize, Serialize};

use crate::{
    context::CoinsActivationContext,
    platform_coin_with_tokens::{
        EnablePlatformCoinWithTokensError, GetPlatformBalance, InitPlatformCoinWithTokensAwaitingStatus,
        InitPlatformCoinWithTokensInProgressStatus, InitPlatformCoinWithTokensTask,
        InitPlatformCoinWithTokensTaskManagerShared, InitPlatformCoinWithTokensUserAction,
        PlatformCoinWithTokensActivationOps, RegisterTokenInfo, TokenActivationParams, TokenActivationRequest,
        TokenAsMmCoinInitializer, TokenInitializer, TokenOf,
    },
    prelude::{ActivationRequestInfo, CurrentBlock, TryFromCoinProtocol, TryPlatformCoinFromMmCoinEnum, TxHistory},
    solana_token_activation::SolanaTokenActivationParams,
};

pub type SolanaCoinTaskManagerShared = InitPlatformCoinWithTokensTaskManagerShared<SolanaCoin>;

impl RegisterTokenInfo<SolanaToken> for SolanaCoin {
    fn register_token_info(&self, token: &SolanaToken) {
        self.add_activated_token(token.ticker().to_owned(), token.protocol_info.clone());
    }
}

#[derive(Clone, Deserialize)]
pub struct SolanaActivationRequest {
    nodes: Vec<RpcNode>,
    #[serde(default)]
    tx_history: bool,
    pub tokens_params: Vec<TokenActivationRequest<SolanaTokenActivationParams>>,
}

impl TxHistory for SolanaActivationRequest {
    fn tx_history(&self) -> bool {
        self.tx_history
    }
}

impl ActivationRequestInfo for SolanaActivationRequest {
    fn is_hw_policy(&self) -> bool {
        false
    }
}

impl TryFromCoinProtocol for SolanaProtocolInfo {
    fn try_from_coin_protocol(proto: CoinProtocol) -> Result<Self, MmError<CoinProtocol>> {
        match proto {
            CoinProtocol::SOLANA(proto) => Ok(proto),
            other => MmError::err(other),
        }
    }
}

impl TryPlatformCoinFromMmCoinEnum for SolanaCoin {
    fn try_from_mm_coin(coin: MmCoinEnum) -> Option<Self>
    where
        Self: Sized,
    {
        match coin {
            MmCoinEnum::Solana(coin) => Some(coin),
            _ => None,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct SolanaActivationResult {
    ticker: String,
    address: String,
    current_block: u64,
    balance: CoinBalance,
    tokens_balances: HashMap<String, CoinBalance>,
}

impl CurrentBlock for SolanaActivationResult {
    fn current_block(&self) -> u64 {
        self.current_block
    }
}

impl GetPlatformBalance for SolanaActivationResult {
    fn get_platform_balance(&self) -> Option<BigDecimal> {
        Some(self.balance.spendable.clone())
    }
}

impl From<SolanaInitError> for EnablePlatformCoinWithTokensError {
    fn from(e: SolanaInitError) -> Self {
        EnablePlatformCoinWithTokensError::PlatformCoinCreationError {
            ticker: e.ticker,
            error: e.kind.to_string(),
        }
    }
}

#[async_trait]
impl PlatformCoinWithTokensActivationOps for SolanaCoin {
    type ActivationRequest = SolanaActivationRequest;
    type PlatformProtocolInfo = SolanaProtocolInfo;
    type ActivationResult = SolanaActivationResult;
    type ActivationError = SolanaInitError;

    type InProgressStatus = InitPlatformCoinWithTokensInProgressStatus;
    type AwaitingStatus = InitPlatformCoinWithTokensAwaitingStatus;
    type UserAction = InitPlatformCoinWithTokensUserAction;

    async fn enable_platform_coin(
        ctx: MmArc,
        ticker: String,
        coin_conf: &serde_json::Value,
        activation_request: Self::ActivationRequest,
        protocol_conf: Self::PlatformProtocolInfo,
    ) -> Result<Self, MmError<Self::ActivationError>> {
        let priv_key_policy = PrivKeyBuildPolicy::detect_priv_key_policy(&ctx).map_err(|e| SolanaInitError {
            ticker: ticker.clone(),
            kind: SolanaInitErrorKind::Internal { reason: e.to_string() },
        })?;

        let coin = Self::init(&ctx, ticker, protocol_conf, activation_request.nodes, priv_key_policy).await?;

        Ok(coin)
    }

    async fn enable_global_nft(
        &self,
        _activation_request: &Self::ActivationRequest,
    ) -> Result<Option<MmCoinEnum>, MmError<Self::ActivationError>> {
        Ok(None)
    }

    fn try_from_mm_coin(coin: MmCoinEnum) -> Option<Self>
    where
        Self: Sized,
    {
        match coin {
            MmCoinEnum::Solana(coin) => Some(coin),
            _ => None,
        }
    }

    fn token_initializers(
        &self,
    ) -> Vec<Box<dyn TokenAsMmCoinInitializer<PlatformCoin = Self, ActivationRequest = Self::ActivationRequest>>> {
        vec![Box::new(self.clone())]
    }

    async fn get_activation_result(
        &self,
        _task_handle: Option<RpcTaskHandleShared<InitPlatformCoinWithTokensTask<SolanaCoin>>>,
        activation_request: &Self::ActivationRequest,
        _nft_global: &Option<MmCoinEnum>,
    ) -> Result<Self::ActivationResult, MmError<Self::ActivationError>> {
        let balance = self.my_balance().compat().await.map_err(|e| SolanaInitError {
            ticker: self.ticker().to_owned(),
            kind: SolanaInitErrorKind::QueryError {
                reason: format!("Failed to fetch '{}' balance: {e}", self.ticker()),
            },
        })?;

        let current_block = self.current_block().compat().await.map_err(|e| SolanaInitError {
            ticker: self.ticker().to_owned(),
            kind: SolanaInitErrorKind::QueryError {
                reason: format!("Failed to fetch current block: {e}"),
            },
        })?;

        let address = self.my_address().map_err(|e| SolanaInitError {
            ticker: self.ticker().to_owned(),
            kind: SolanaInitErrorKind::Internal {
                reason: e.into_inner().to_string(),
            },
        })?;

        let tokens_info = self.tokens_info.lock().clone();
        let tasks = tokens_info.into_iter().map(|(ticker, info)| {
            let coin = self.clone();
            async move {
                let balance = coin
                    .token_balance(&info.mint_address)
                    .await
                    .map_err(|e| SolanaInitError {
                        ticker: coin.ticker().to_owned(),
                        kind: SolanaInitErrorKind::QueryError {
                            reason: format!("Failed to fetch '{ticker}' balance: {e}"),
                        },
                    })?;

                Ok::<_, SolanaInitError>((ticker, balance))
            }
        });

        let tokens_balances: HashMap<_, _> = join_all(tasks).await.into_iter().collect::<Result<_, _>>()?;

        Ok(SolanaActivationResult {
            ticker: self.ticker().to_owned(),
            address,
            current_block,
            balance,
            tokens_balances,
        })
    }

    fn start_history_background_fetching(
        &self,
        ctx: MmArc,
        storage: impl TxHistoryStorage,
        initial_balance: Option<BigDecimal>,
    ) {
        todo!()
    }

    fn rpc_task_manager(
        activation_ctx: &CoinsActivationContext,
    ) -> &InitPlatformCoinWithTokensTaskManagerShared<SolanaCoin> {
        &activation_ctx.init_solana_coin_task_manager
    }
}

#[async_trait]
impl TokenInitializer for SolanaCoin {
    type Token = SolanaToken;
    type TokenActivationRequest = SolanaTokenActivationParams;
    type TokenProtocol = SolanaTokenProtocolInfo;
    type InitTokensError = SolanaTokenInitError;

    fn tokens_requests_from_platform_request(
        platform_request: &SolanaActivationRequest,
    ) -> Vec<TokenActivationRequest<Self::TokenActivationRequest>> {
        platform_request.tokens_params.clone()
    }

    async fn enable_tokens(
        &self,
        params: Vec<TokenActivationParams<Self::TokenActivationRequest, Self::TokenProtocol>>,
    ) -> Result<Vec<Self::Token>, MmError<Self::InitTokensError>> {
        params
            .into_iter()
            .map(|param| SolanaToken::init(param.ticker, self.platform_coin().clone(), param.protocol.clone()))
            .collect()
    }

    fn platform_coin(&self) -> &<Self::Token as TokenOf>::PlatformCoin {
        self
    }

    fn validate_token_params(
        &self,
        params: &[TokenActivationParams<Self::TokenActivationRequest, Self::TokenProtocol>],
    ) -> MmResult<(), Self::InitTokensError> {
        for token_param in params {
            match &token_param.protocol {
                SolanaTokenProtocolInfo { platform, .. } if platform == self.platform_coin().ticker() => {},
                other => {
                    return MmError::err(SolanaTokenInitError {
                        ticker: self.ticker().to_owned(),
                        kind: SolanaTokenInitErrorKind::PlatformCoinMismatch {
                            expected_platform_coin: self.platform_coin().ticker().to_owned(),
                            actual_platform_coin: other.platform.clone(),
                        },
                    })
                },
            }
        }

        Ok(())
    }
}
