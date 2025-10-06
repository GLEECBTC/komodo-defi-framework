use crate::{
    prelude::TryPlatformCoinFromMmCoinEnum,
    token::{EnableTokenError, TokenActivationOps, TokenProtocolParams},
};
use async_trait::async_trait;
use coins::{
    tendermint::{
        TendermintCoin, TendermintToken, TendermintTokenActivationParams, TendermintTokenInitError,
        TendermintTokenProtocolInfo,
    },
    CoinBalance, MarketCoinOps, MmCoinEnum,
};
use common::Future01CompatExt;
use mm2_err_handle::{
    map_mm_error::MmResultExt,
    prelude::{MapMmError, MmError},
};
use serde::Serialize;
use serde_json::Value as Json;
use std::collections::HashMap;

impl From<TendermintTokenInitError> for EnableTokenError {
    fn from(err: TendermintTokenInitError) -> Self {
        match err {
            TendermintTokenInitError::MyAddressError(e) | TendermintTokenInitError::Internal(e) => {
                EnableTokenError::Internal(e)
            },
            TendermintTokenInitError::CouldNotFetchBalance(e) => EnableTokenError::CouldNotFetchBalance(e),
            TendermintTokenInitError::PlatformCoinMismatch => EnableTokenError::PlatformCoinMismatch,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TendermintTokenInitResult {
    balances: HashMap<String, CoinBalance>,
    platform_coin: String,
}

impl TryPlatformCoinFromMmCoinEnum for TendermintCoin {
    fn try_from_mm_coin(coin: MmCoinEnum) -> Option<Self>
    where
        Self: Sized,
    {
        match coin {
            MmCoinEnum::Tendermint(coin) => Some(coin),
            _ => None,
        }
    }
}

impl TokenProtocolParams for TendermintTokenProtocolInfo {
    fn platform_coin_ticker(&self) -> &str {
        &self.platform
    }
}

#[async_trait]
impl TokenActivationOps for TendermintToken {
    type ActivationParams = TendermintTokenActivationParams;
    type ProtocolInfo = TendermintTokenProtocolInfo;
    type ActivationResult = TendermintTokenInitResult;
    type ActivationError = TendermintTokenInitError;

    async fn enable_token(
        ticker: String,
        platform_coin: Self::PlatformCoin,
        _activation_params: Self::ActivationParams,
        _token_conf: Json,
        protocol_conf: Self::ProtocolInfo,
        _is_custom: bool,
    ) -> Result<(Self, Self::ActivationResult), MmError<Self::ActivationError>> {
        let token = TendermintToken::new(ticker, platform_coin, protocol_conf.decimals, protocol_conf.denom)?;

        let balance = token
            .my_balance()
            .compat()
            .await
            .mm_err(|e| TendermintTokenInitError::CouldNotFetchBalance(e.to_string()))?;

        let my_address = token.my_address().map_mm_err()?;
        let balances = HashMap::from([(my_address, balance)]);

        let init_result = TendermintTokenInitResult {
            balances,
            platform_coin: token.platform_ticker().into(),
        };

        Ok((token, init_result))
    }
}
