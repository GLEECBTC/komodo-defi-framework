#[cfg(feature = "enable-sia")]
use coins::siacoin::SiaCoinActivationParams;
use coins::utxo::bch::BchActivationRequest;
use coins::utxo::UtxoActivationParams;
use coins::z_coin::ZcoinActivationParams;
use coins::{coin_conf, CoinBalance, CoinProtocol, CustomTokenError, DerivationMethodResponse, MmCoinEnum};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::BigDecimal;
use serde_derive::Serialize;
use serde_json::{self as json, json, Value as Json};
use std::collections::{HashMap, HashSet};

pub trait CurrentBlock {
    fn current_block(&self) -> u64;
}

pub trait TxHistory {
    fn tx_history(&self) -> bool;
}

impl TxHistory for UtxoActivationParams {
    fn tx_history(&self) -> bool {
        self.tx_history
    }
}

impl TxHistory for BchActivationRequest {
    fn tx_history(&self) -> bool {
        self.utxo_params.tx_history
    }
}

#[cfg(feature = "enable-sia")]
impl TxHistory for SiaCoinActivationParams {
    fn tx_history(&self) -> bool {
        self.tx_history
    }
}

impl TxHistory for ZcoinActivationParams {
    fn tx_history(&self) -> bool {
        false
    }
}

pub trait GetAddressesBalances {
    fn get_addresses_balances(&self) -> HashMap<String, BigDecimal>;
}

#[derive(Clone, Debug, Serialize)]
pub struct CoinAddressInfo<Balance> {
    pub(crate) derivation_method: DerivationMethodResponse,
    pub(crate) pubkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) balances: Option<Balance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tickers: Option<HashSet<String>>,
}

pub type TokenBalances = HashMap<String, CoinBalance>;

pub trait TryPlatformCoinFromMmCoinEnum {
    fn try_from_mm_coin(coin: MmCoinEnum) -> Option<Self>
    where
        Self: Sized;
}

pub trait TryFromCoinProtocol {
    #[allow(clippy::result_large_err)]
    fn try_from_coin_protocol(proto: CoinProtocol) -> Result<Self, MmError<CoinProtocol>>
    where
        Self: Sized;
}

#[derive(Debug)]
pub enum CoinConfWithProtocolError {
    ConfigIsNotFound(String),
    CoinProtocolParseError { ticker: String, err: json::Error },
    UnexpectedProtocol { ticker: String, protocol: Json },
    CustomTokenError(CustomTokenError),
}

/// Determines the coin configuration and protocol information for a given coin or NFT ticker.
pub fn coin_conf_with_protocol<T: TryFromCoinProtocol>(
    ctx: &MmArc,
    coin: &str,
    protocol_from_request: Option<CoinProtocol>,
) -> Result<(Json, T), MmError<CoinConfWithProtocolError>> {
    let conf = coin_conf(ctx, coin);
    let is_ticker_in_config = !conf.is_null();

    // For `protocol_from_request`: None = config-based activation, Some = custom token activation
    match (protocol_from_request, is_ticker_in_config) {
        // Config-based activation requested with an existing configuration
        // Proceed with parsing protocol info from config
        (None, true) => parse_coin_protocol_from_config(conf, coin),
        // Custom token activation requested and no matching ticker in config
        // Proceed with custom token config creation from protocol info
        (Some(protocol), false) => create_custom_token_config(ctx, coin, protocol),
        // Custom token activation requested but a coin with the same ticker already exists in config
        (Some(_), true) => Err(MmError::new(CoinConfWithProtocolError::CustomTokenError(
            CustomTokenError::DuplicateTickerInConfig {
                ticker_in_config: coin.to_string(),
            },
        ))),
        // Config-based activation requested but ticker not found in config
        (None, false) => Err(MmError::new(CoinConfWithProtocolError::ConfigIsNotFound(coin.into()))),
    }
}

fn parse_coin_protocol_from_config<T: TryFromCoinProtocol>(
    conf: Json,
    coin: &str,
) -> Result<(Json, T), MmError<CoinConfWithProtocolError>> {
    let protocol = json::from_value(conf["protocol"].clone()).map_to_mm(|err| {
        CoinConfWithProtocolError::CoinProtocolParseError {
            ticker: coin.into(),
            err,
        }
    })?;

    let coin_protocol =
        T::try_from_coin_protocol(protocol).mm_err(|p| CoinConfWithProtocolError::UnexpectedProtocol {
            ticker: coin.into(),
            protocol: json!(p),
        })?;

    Ok((conf, coin_protocol))
}

fn create_custom_token_config<T: TryFromCoinProtocol>(
    ctx: &MmArc,
    coin: &str,
    protocol: CoinProtocol,
) -> Result<(Json, T), MmError<CoinConfWithProtocolError>> {
    protocol
        .custom_token_validations(ctx)
        .mm_err(CoinConfWithProtocolError::CustomTokenError)?;

    let conf = json!({
        "protocol": protocol,
        "wallet_only": true
    });

    let coin_protocol =
        T::try_from_coin_protocol(protocol).mm_err(|p| CoinConfWithProtocolError::UnexpectedProtocol {
            ticker: coin.into(),
            protocol: json!(p),
        })?;

    Ok((conf, coin_protocol))
}

/// A trait to be implemented for coin activation requests to determine some information about the request.
pub trait ActivationRequestInfo {
    /// Checks if the activation request is for a hardware wallet.
    fn is_hw_policy(&self) -> bool;
}

impl ActivationRequestInfo for UtxoActivationParams {
    fn is_hw_policy(&self) -> bool {
        self.priv_key_policy.is_hw_policy()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ActivationRequestInfo for ZcoinActivationParams {
    fn is_hw_policy(&self) -> bool {
        false
    } // TODO: fix when device policy is added
}
