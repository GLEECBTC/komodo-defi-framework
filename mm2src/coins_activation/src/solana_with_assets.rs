#![allow(unused_variables)]

use std::collections::HashMap;

use async_trait::async_trait;
use coins::{
    my_tx_history_v2::TxHistoryStorage,
    solana::{RpcNode, SolanaCoin, SolanaInitError, SolanaInitErrorKind, SolanaProtocolInfo, SolanaToken},
    CoinBalance, CoinProtocol, MarketCoinOps, MmCoinEnum, PrivKeyBuildPolicy,
};
use common::{true_f, Future01CompatExt};
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
        PlatformCoinWithTokensActivationOps, RegisterTokenInfo, TokenAsMmCoinInitializer,
    },
    prelude::{ActivationRequestInfo, CurrentBlock, TryFromCoinProtocol, TryPlatformCoinFromMmCoinEnum, TxHistory},
};

pub type SolanaCoinTaskManagerShared = InitPlatformCoinWithTokensTaskManagerShared<SolanaCoin>;

impl RegisterTokenInfo<SolanaToken> for SolanaCoin {
    fn register_token_info(&self, token: &SolanaToken) {
        todo!()
    }
}

#[derive(Clone, Deserialize)]
pub struct SolanaActivationRequest {
    nodes: Vec<RpcNode>,
    #[serde(default)]
    tx_history: bool,
    #[serde(default = "true_f")]
    pub get_balances: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    balance: Option<CoinBalance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens_balances: Option<HashMap<String, CoinBalance>>,
}

impl CurrentBlock for SolanaActivationResult {
    fn current_block(&self) -> u64 {
        self.current_block
    }
}

impl GetPlatformBalance for SolanaActivationResult {
    fn get_platform_balance(&self) -> Option<BigDecimal> {
        self.balance.as_ref().map(|b| b.spendable.clone())
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
        // TODO
        vec![]
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
                reason: format!("Failed to fetch balance: {e}"),
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

        Ok(SolanaActivationResult {
            ticker: self.ticker().to_owned(),
            address,
            current_block,
            balance: Some(balance),
            tokens_balances: None,
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
