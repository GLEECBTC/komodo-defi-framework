#![allow(dead_code)]
#![allow(unused_variables)]

use async_trait::async_trait;
use coins::{
    solana::{SolanaCoin, SolanaToken, SolanaTokenInitError, SolanaTokenInitErrorKind, SolanaTokenProtocolInfo},
    CoinBalance, CoinProtocol, MarketCoinOps,
};
use common::Future01CompatExt;
use mm2_err_handle::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    platform_coin_with_tokens::{InitTokensAsMmCoinsError, TokenOf},
    prelude::TryFromCoinProtocol,
    token::{EnableTokenError, TokenActivationOps, TokenProtocolParams},
};

#[derive(Clone, Deserialize)]
pub struct SolanaTokenActivationParams {}

#[derive(Clone, Serialize)]
pub struct SolanaTokenInitResult {
    ticker: String,
    address: String,
    current_block: u64,
    balance: CoinBalance,
}

impl TokenOf for SolanaToken {
    type PlatformCoin = SolanaCoin;
}

impl TryFromCoinProtocol for SolanaTokenProtocolInfo {
    fn try_from_coin_protocol(proto: CoinProtocol) -> Result<Self, MmError<CoinProtocol>> {
        match proto {
            CoinProtocol::SOLANATOKEN(proto) => Ok(proto),
            other => MmError::err(other),
        }
    }
}

impl TokenProtocolParams for SolanaTokenProtocolInfo {
    fn platform_coin_ticker(&self) -> &str {
        &self.platform
    }
}

impl From<SolanaTokenInitError> for EnableTokenError {
    fn from(err: SolanaTokenInitError) -> Self {
        match err.kind {
            SolanaTokenInitErrorKind::QueryError { reason } => EnableTokenError::Transport(reason.to_string()),
            SolanaTokenInitErrorKind::Internal { reason } => EnableTokenError::Internal(reason.to_string()),
            SolanaTokenInitErrorKind::PlatformCoinMismatch { .. } => EnableTokenError::PlatformCoinMismatch,
        }
    }
}

impl From<SolanaTokenInitError> for InitTokensAsMmCoinsError {
    fn from(err: SolanaTokenInitError) -> Self {
        match err.kind {
            SolanaTokenInitErrorKind::QueryError { reason } => InitTokensAsMmCoinsError::Transport(reason.to_string()),
            SolanaTokenInitErrorKind::Internal { reason } => InitTokensAsMmCoinsError::Internal(reason.to_string()),
            SolanaTokenInitErrorKind::PlatformCoinMismatch { .. } => InitTokensAsMmCoinsError::PlatformCoinMismatch,
        }
    }
}

#[async_trait]
impl TokenActivationOps for SolanaToken {
    type ActivationParams = SolanaTokenActivationParams;
    type ProtocolInfo = SolanaTokenProtocolInfo;
    type ActivationResult = SolanaTokenInitResult;
    type ActivationError = SolanaTokenInitError;

    async fn enable_token(
        ticker: String,
        platform_coin: Self::PlatformCoin,
        _activation_params: Self::ActivationParams,
        _token_conf: serde_json::Value,
        protocol_conf: Self::ProtocolInfo,
        _is_custom: bool,
    ) -> Result<(Self, Self::ActivationResult), MmError<Self::ActivationError>> {
        let token = SolanaToken::init(ticker.clone(), platform_coin, protocol_conf)?;

        let address = token.my_address().map_err(|e| SolanaTokenInitError {
            ticker: ticker.clone(),
            kind: SolanaTokenInitErrorKind::Internal {
                reason: e.into_inner().to_string(),
            },
        })?;

        let balance = token.my_balance().compat().await.mm_err(|e| SolanaTokenInitError {
            ticker: ticker.clone(),
            kind: SolanaTokenInitErrorKind::QueryError {
                reason: format!("Failed to fetch balance: {e}"),
            },
        })?;

        let current_block = token.current_block().compat().await.map_err(|e| SolanaTokenInitError {
            ticker: ticker.clone(),
            kind: SolanaTokenInitErrorKind::QueryError {
                reason: format!("Failed to fetch current block: {e}"),
            },
        })?;

        let init_result = SolanaTokenInitResult {
            ticker,
            address,
            current_block,
            balance,
        };

        Ok((token, init_result))
    }
}
