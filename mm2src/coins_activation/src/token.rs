// Contains token activation traits and their implementations for various coins

use crate::platform_coin_with_tokens::{self, RegisterTokenInfo};
use crate::prelude::*;
use async_trait::async_trait;
use coins::utxo::rpc_clients::UtxoRpcError;
use coins::{
    lp_coinfind, lp_coinfind_or_err, BalanceError, CoinProtocol, CoinsContext, CustomTokenError, MmCoinEnum,
    PrivKeyPolicyNotAllowed, RegisterCoinError, UnexpectedDerivationMethod,
};
use common::{HttpStatusCode, StatusCode};
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use ser_error_derive::SerializeErrorType;
use serde_derive::{Deserialize, Serialize};
use serde_json::Value as Json;

pub trait TokenProtocolParams {
    fn platform_coin_ticker(&self) -> &str;
}

#[async_trait]
pub trait TokenActivationOps: Into<MmCoinEnum> + platform_coin_with_tokens::TokenOf {
    type ActivationParams;
    type ProtocolInfo: TokenProtocolParams + TryFromCoinProtocol;
    type ActivationResult;
    type ActivationError: NotMmError;

    async fn enable_token(
        ticker: String,
        platform_coin: Self::PlatformCoin,
        activation_params: Self::ActivationParams,
        token_conf: Json,
        protocol_conf: Self::ProtocolInfo,
        is_custom: bool,
    ) -> Result<(Self, Self::ActivationResult), MmError<Self::ActivationError>>;
}

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum EnableTokenError {
    #[display(fmt = "Token {_0} is already activated")]
    TokenIsAlreadyActivated(String),
    #[display(fmt = "Token {_0} config is not found")]
    TokenConfigIsNotFound(String),
    #[display(fmt = "Token {ticker} protocol parsing failed: {error}")]
    TokenProtocolParseError {
        ticker: String,
        error: String,
    },
    #[display(fmt = "Unexpected token protocol {protocol} for {ticker}")]
    UnexpectedTokenProtocol {
        ticker: String,
        protocol: Json,
    },
    #[display(fmt = "Platform coin {_0} is not activated")]
    PlatformCoinIsNotActivated(String),
    #[display(fmt = "{platform_coin_ticker} is not a platform coin for token {token_ticker}")]
    UnsupportedPlatformCoin {
        platform_coin_ticker: String,
        token_ticker: String,
    },
    #[display(fmt = "{_0}")]
    UnexpectedDerivationMethod(UnexpectedDerivationMethod),
    CouldNotFetchBalance(String),
    InvalidConfig(String),
    Transport(String),
    Internal(String),
    InvalidPayload(String),
    PrivKeyPolicyNotAllowed(PrivKeyPolicyNotAllowed),
    #[display(fmt = "Custom token error: {_0}")]
    CustomTokenError(CustomTokenError),
    PlatformCoinMismatch,
}

impl From<RegisterCoinError> for EnableTokenError {
    fn from(err: RegisterCoinError) -> Self {
        match err {
            RegisterCoinError::CoinIsInitializedAlready { coin } => Self::TokenIsAlreadyActivated(coin),
            RegisterCoinError::Internal(err) => Self::Internal(err),
        }
    }
}

impl From<CoinConfWithProtocolError> for EnableTokenError {
    fn from(err: CoinConfWithProtocolError) -> Self {
        match err {
            CoinConfWithProtocolError::ConfigIsNotFound(ticker) => EnableTokenError::TokenConfigIsNotFound(ticker),
            CoinConfWithProtocolError::CoinProtocolParseError { ticker, err } => {
                EnableTokenError::TokenProtocolParseError {
                    ticker,
                    error: err.to_string(),
                }
            },
            CoinConfWithProtocolError::UnexpectedProtocol { ticker, protocol } => {
                EnableTokenError::UnexpectedTokenProtocol { ticker, protocol }
            },
            CoinConfWithProtocolError::CustomTokenError(e) => EnableTokenError::CustomTokenError(e),
        }
    }
}

impl From<BalanceError> for EnableTokenError {
    fn from(e: BalanceError) -> Self {
        match e {
            BalanceError::Transport(e) | BalanceError::InvalidResponse(e) => EnableTokenError::Transport(e),
            BalanceError::UnexpectedDerivationMethod(e) => EnableTokenError::UnexpectedDerivationMethod(e),
            BalanceError::Internal(e) | BalanceError::WalletStorageError(e) => EnableTokenError::Internal(e),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct EnableTokenRequest<T> {
    ticker: String,
    protocol: Option<CoinProtocol>,
    activation_params: T,
}

pub async fn enable_token<Token>(
    ctx: MmArc,
    req: EnableTokenRequest<Token::ActivationParams>,
) -> Result<Token::ActivationResult, MmError<EnableTokenError>>
where
    Token: TokenActivationOps + Clone,
    EnableTokenError: From<Token::ActivationError>,
{
    if let Ok(Some(_)) = lp_coinfind(&ctx, &req.ticker).await {
        return MmError::err(EnableTokenError::TokenIsAlreadyActivated(req.ticker));
    }

    let (token_conf, token_protocol): (_, Token::ProtocolInfo) =
        coin_conf_with_protocol(&ctx, &req.ticker, req.protocol.clone()).map_mm_err()?;

    let platform_coin = lp_coinfind_or_err(&ctx, token_protocol.platform_coin_ticker())
        .await
        .mm_err(|_| EnableTokenError::PlatformCoinIsNotActivated(token_protocol.platform_coin_ticker().to_owned()))?;

    let platform_coin = Token::PlatformCoin::try_from_mm_coin(platform_coin).or_mm_err(|| {
        EnableTokenError::UnsupportedPlatformCoin {
            platform_coin_ticker: token_protocol.platform_coin_ticker().into(),
            token_ticker: req.ticker.clone(),
        }
    })?;

    let (token, activation_result) = Token::enable_token(
        req.ticker,
        platform_coin.clone(),
        req.activation_params,
        token_conf,
        token_protocol,
        req.protocol.is_some(),
    )
    .await
    .map_mm_err()?;

    let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
    coins_ctx.add_token(token.clone().into()).await.map_mm_err()?;

    platform_coin.register_token_info(&token);

    Ok(activation_result)
}

impl From<UtxoRpcError> for EnableTokenError {
    fn from(err: UtxoRpcError) -> Self {
        match err {
            UtxoRpcError::Transport(e) | UtxoRpcError::ResponseParseError(e) => {
                EnableTokenError::Transport(e.to_string())
            },
            UtxoRpcError::InvalidResponse(e) => EnableTokenError::Transport(e),
            UtxoRpcError::Internal(e) => EnableTokenError::Internal(e),
        }
    }
}

impl HttpStatusCode for EnableTokenError {
    fn status_code(&self) -> StatusCode {
        match self {
            EnableTokenError::TokenIsAlreadyActivated(_)
            | EnableTokenError::PlatformCoinIsNotActivated(_)
            | EnableTokenError::TokenConfigIsNotFound { .. }
            | EnableTokenError::UnexpectedTokenProtocol { .. }
            | EnableTokenError::InvalidPayload(_)
            | EnableTokenError::PlatformCoinMismatch
            | EnableTokenError::CustomTokenError(_) => StatusCode::BAD_REQUEST,
            EnableTokenError::TokenProtocolParseError { .. }
            | EnableTokenError::UnsupportedPlatformCoin { .. }
            | EnableTokenError::UnexpectedDerivationMethod(_)
            | EnableTokenError::Transport(_)
            | EnableTokenError::CouldNotFetchBalance(_)
            | EnableTokenError::InvalidConfig(_)
            | EnableTokenError::Internal(_)
            | EnableTokenError::PrivKeyPolicyNotAllowed(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
