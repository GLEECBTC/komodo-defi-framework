use crate::{
    prelude::{TryFromCoinProtocol, TryPlatformCoinFromMmCoinEnum},
    token::{EnableTokenError, TokenActivationOps, TokenProtocolParams},
};
use async_trait::async_trait;
use coins::eth::v2_activation::{EthTokenActivationParams, EthTokenProtocol, NftProtocol, NftProviderEnum};
use coins::hd_wallet::DisplayAddress;
use coins::nft::nft_structs::NftInfo;
use coins::{
    eth::{
        v2_activation::{Erc20Protocol, EthTokenActivationError},
        valid_addr_from_str, EthCoin,
    },
    CoinBalance, CoinProtocol, CoinWithDerivationMethod, MarketCoinOps, MmCoin, MmCoinEnum,
};
use common::Future01CompatExt;
use mm2_err_handle::prelude::*;
use serde::Serialize;
use serde_json::Value as Json;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum EthTokenInitResult {
    Erc20(Erc20InitResult),
    Nft(NftInitResult),
}

#[derive(Debug, Serialize)]
pub struct Erc20InitResult {
    balances: HashMap<String, CoinBalance>,
    platform_coin: String,
    token_contract_address: String,
    required_confirmations: u64,
}

#[derive(Debug, Serialize)]
pub struct NftInitResult {
    nfts: HashMap<String, NftInfo>,
    platform_coin: String,
}

impl From<EthTokenActivationError> for EnableTokenError {
    fn from(err: EthTokenActivationError) -> Self {
        match err {
            EthTokenActivationError::InternalError(e) => EnableTokenError::Internal(e),
            EthTokenActivationError::CouldNotFetchBalance(e)
            | EthTokenActivationError::Transport(e)
            | EthTokenActivationError::ClientConnectionFailed(e) => EnableTokenError::Transport(e),
            EthTokenActivationError::InvalidPayload(e) => EnableTokenError::InvalidPayload(e),
            EthTokenActivationError::UnexpectedDerivationMethod(e) => EnableTokenError::UnexpectedDerivationMethod(e),
            EthTokenActivationError::PrivKeyPolicyNotAllowed(e) => EnableTokenError::PrivKeyPolicyNotAllowed(e),
            EthTokenActivationError::CustomTokenError(e) => EnableTokenError::CustomTokenError(e),
            EthTokenActivationError::PlatformCoinMismatch => EnableTokenError::PlatformCoinMismatch,
        }
    }
}

impl TryPlatformCoinFromMmCoinEnum for EthCoin {
    fn try_from_mm_coin(coin: MmCoinEnum) -> Option<Self>
    where
        Self: Sized,
    {
        match coin {
            MmCoinEnum::EthCoin(coin) => Some(coin),
            _ => None,
        }
    }
}

impl TryFromCoinProtocol for Erc20Protocol {
    fn try_from_coin_protocol(proto: CoinProtocol) -> Result<Self, MmError<CoinProtocol>>
    where
        Self: Sized,
    {
        match proto {
            CoinProtocol::ERC20 {
                platform,
                contract_address,
            } => {
                let token_addr = valid_addr_from_str(&contract_address).map_err(|_| CoinProtocol::ERC20 {
                    // TODO: maybe add error description to this err (we're losing 'Invalid address checksum' here)
                    platform: platform.clone(),
                    contract_address,
                })?;

                Ok(Erc20Protocol { platform, token_addr })
            },
            proto => MmError::err(proto),
        }
    }
}

impl TryFromCoinProtocol for NftProtocol {
    fn try_from_coin_protocol(proto: CoinProtocol) -> Result<Self, MmError<CoinProtocol>>
    where
        Self: Sized,
    {
        match proto {
            CoinProtocol::NFT { platform } => Ok(NftProtocol { platform }),
            proto => MmError::err(proto),
        }
    }
}

impl TryFromCoinProtocol for EthTokenProtocol {
    fn try_from_coin_protocol(proto: CoinProtocol) -> Result<Self, MmError<CoinProtocol>>
    where
        Self: Sized,
    {
        match proto {
            CoinProtocol::ERC20 { .. } => {
                let erc20_protocol = Erc20Protocol::try_from_coin_protocol(proto)?;
                Ok(EthTokenProtocol::Erc20(erc20_protocol))
            },
            CoinProtocol::NFT { platform } => Ok(EthTokenProtocol::Nft(NftProtocol { platform })),
            proto => MmError::err(proto),
        }
    }
}

impl TokenProtocolParams for Erc20Protocol {
    fn platform_coin_ticker(&self) -> &str {
        &self.platform
    }
}

impl TokenProtocolParams for EthTokenProtocol {
    fn platform_coin_ticker(&self) -> &str {
        match self {
            EthTokenProtocol::Erc20(erc20_protocol) => erc20_protocol.platform_coin_ticker(),
            EthTokenProtocol::Nft(nft_protocol) => &nft_protocol.platform,
        }
    }
}

#[async_trait]
impl TokenActivationOps for EthCoin {
    type ActivationParams = EthTokenActivationParams;
    type ProtocolInfo = EthTokenProtocol;
    type ActivationResult = EthTokenInitResult;
    type ActivationError = EthTokenActivationError;

    async fn enable_token(
        ticker: String,
        platform_coin: Self::PlatformCoin,
        activation_params: Self::ActivationParams,
        token_conf: Json,
        protocol_conf: Self::ProtocolInfo,
        is_custom: bool,
    ) -> Result<(Self, Self::ActivationResult), MmError<Self::ActivationError>> {
        match activation_params {
            EthTokenActivationParams::Erc20(erc20_init_params) => match protocol_conf {
                EthTokenProtocol::Erc20(erc20_protocol) => {
                    let token = platform_coin
                        .initialize_erc20_token(
                            ticker.clone(),
                            erc20_init_params,
                            token_conf,
                            erc20_protocol,
                            is_custom,
                        )
                        .await
                        .map_mm_err()?;

                    let address = token
                        .derivation_method()
                        .single_addr_or_err()
                        .await
                        .map_mm_err()?
                        .display_address();
                    let token_contract_address = token.erc20_token_address().ok_or_else(|| {
                        EthTokenActivationError::InternalError("Token contract address is missing".to_string())
                    })?;

                    let balance = token
                        .my_balance()
                        .compat()
                        .await
                        .map_err(|e| EthTokenActivationError::CouldNotFetchBalance(e.to_string()))?;

                    let balances = HashMap::from([(address, balance)]);

                    let init_result = EthTokenInitResult::Erc20(Erc20InitResult {
                        balances,
                        platform_coin: token.platform_ticker().to_owned(),
                        required_confirmations: token.required_confirmations(),
                        token_contract_address: format!("{token_contract_address:#02x}"),
                    });

                    Ok((token, init_result))
                },
                _ => Err(MmError::new(EthTokenActivationError::InternalError(
                    "Mismatched protocol info for ERC-20".to_string(),
                ))),
            },
            EthTokenActivationParams::Nft(nft_init_params) => match protocol_conf {
                EthTokenProtocol::Nft(nft_protocol) => {
                    if nft_protocol.platform != platform_coin.ticker() {
                        return MmError::err(EthTokenActivationError::InternalError(
                            "NFT platform coin ticker does not match the expected platform".to_string(),
                        ));
                    }
                    let nft_global = match &nft_init_params.provider {
                        NftProviderEnum::Moralis { url, komodo_proxy } => platform_coin
                            .initialize_global_nft(url, *komodo_proxy)
                            .await
                            .map_mm_err()?,
                    };
                    let nfts = nft_global.nfts_infos.lock().await.clone();
                    let init_result = EthTokenInitResult::Nft(NftInitResult {
                        nfts,
                        platform_coin: platform_coin.ticker().to_owned(),
                    });
                    Ok((nft_global, init_result))
                },
                _ => Err(MmError::new(EthTokenActivationError::InternalError(
                    "Mismatched protocol info for NFT".to_string(),
                ))),
            },
        }
    }
}
