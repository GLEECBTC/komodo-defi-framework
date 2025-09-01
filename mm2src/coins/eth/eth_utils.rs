use super::*;
use crate::{coin_conf, NumConversError, NumConversResult};
use ethabi::{Function, Token};
use ethereum_types::{Address, FromDecStrErr, U256};
use ethkey::{public_to_address, Public};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::MapToMmResult;
use mm2_number::{BigDecimal, MmNumber};
use secp256k1::PublicKey;
use serde::de::DeserializeOwned;
use serde_json::Value as Json;

/// Coin config parameter name for the max supported eth transaction type
pub(super) const MAX_ETH_TX_TYPE_SUPPORTED: &str = "max_eth_tx_type";
/// Coin config parameter name for the eth gas price adjustment values
pub(super) const GAS_PRICE_ADJUST: &str = "gas_price_adjust";
/// Coin config parameter name for the eth estimate gas multiplier
pub(super) const ESTIMATE_GAS_MULT: &str = "estimate_gas_mult";
/// Coin config parameter name for the default eth swap gas fee policy
pub(super) const SWAP_GAS_FEE_POLICY: &str = "swap_gas_fee_policy";

pub(crate) mod nonce_sequencer {
    use super::*;

    type PerNetNonceLocksMap = Arc<AsyncMutex<HashMap<Address, Arc<AsyncMutex<()>>>>>;

    /// TODO: better to use ChainSpec instead of ticker
    type AllNetsNonceLocks = Mutex<HashMap<String, PerNetNonceLocks>>;

    // We can use a nonce lock shared between tokens using the same platform coin and the platform itself.
    // For example, ETH/USDT-ERC20 should use the same lock, but it will be different for BNB/USDT-BEP20.
    // This lock is used to ensure that only one transaction is sent at a time per address.
    lazy_static! {
        static ref ALL_NETS_NONCE_LOCKS: AllNetsNonceLocks = Mutex::new(HashMap::new());
    }

    #[derive(Clone)]
    pub(crate) struct PerNetNonceLocks {
        locks: PerNetNonceLocksMap,
    }

    impl PerNetNonceLocks {
        fn new_nonce_lock() -> PerNetNonceLocks {
            Self {
                locks: Arc::new(AsyncMutex::new(HashMap::new())),
            }
        }

        pub(crate) fn get_net_locks(platform_ticker: String) -> Self {
            let mut networks = ALL_NETS_NONCE_LOCKS.lock().unwrap();
            networks
                .entry(platform_ticker)
                .or_insert_with(Self::new_nonce_lock)
                .clone()
        }

        /// Retrieves the nonce lock associated with a given eth address.
        /// If the address does not have an associated lock, a new one is created and stored.
        pub(crate) async fn get_adddress_lock(&self, address: Address) -> Arc<AsyncMutex<()>> {
            let mut locks = self.locks.lock().await;
            locks
                .entry(address)
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        }
    }
}

pub(crate) fn get_function_input_data(decoded: &[Token], func: &Function, index: usize) -> Result<Token, String> {
    decoded.get(index).cloned().ok_or(format!(
        "Missing input in function {}: No input found at index {}",
        func.name.clone(),
        index
    ))
}

pub(crate) fn get_function_name(name: &str, watcher_reward: bool) -> String {
    if watcher_reward {
        format!("{}{}", name, "Reward")
    } else {
        name.to_owned()
    }
}

pub fn addr_from_raw_pubkey(pubkey: &[u8]) -> Result<Address, String> {
    let pubkey = try_s!(PublicKey::from_slice(pubkey).map_err(|e| ERRL!("{:?}", e)));
    let eth_public = Public::from_slice(&pubkey.serialize_uncompressed()[1..65]);
    Ok(public_to_address(&eth_public))
}

pub fn addr_from_pubkey_str(pubkey: &str) -> Result<String, String> {
    let pubkey_bytes = try_s!(hex::decode(pubkey));
    let addr = try_s!(addr_from_raw_pubkey(&pubkey_bytes));
    Ok(format!("{addr:#02x}"))
}

pub(crate) fn display_u256_with_decimal_point(number: U256, decimals: u8) -> String {
    let mut string = number.to_string();
    let decimals = decimals as usize;
    if string.len() <= decimals {
        string.insert_str(0, &"0".repeat(decimals - string.len() + 1));
    }

    string.insert(string.len() - decimals, '.');
    string.trim_end_matches('0').into()
}

/// Converts 'number' to value with decimal point and shifts it left by 'decimals' places
pub fn u256_to_big_decimal(number: U256, decimals: u8) -> NumConversResult<BigDecimal> {
    let string = display_u256_with_decimal_point(number, decimals);
    Ok(string.parse::<BigDecimal>()?)
}

/// Shifts 'number' with decimal point right by 'decimals' places and converts it to U256 value
pub fn u256_from_big_decimal(amount: &BigDecimal, decimals: u8) -> NumConversResult<U256> {
    let mut amount = amount.to_string();
    let dot = amount.find('.');
    let decimals = decimals as usize;
    if let Some(index) = dot {
        let mut fractional = amount.split_off(index);
        // remove the dot from fractional part
        fractional.remove(0);
        if fractional.len() < decimals {
            fractional.insert_str(fractional.len(), &"0".repeat(decimals - fractional.len()));
        }
        fractional.truncate(decimals);
        amount.push_str(&fractional);
    } else {
        amount.insert_str(amount.len(), &"0".repeat(decimals));
    }
    U256::from_dec_str(&amount).map_to_mm(|e| NumConversError::new(format!("{e:?}")))
}

/// Converts BigDecimal gwei value to wei value as U256
#[inline(always)]
pub fn wei_from_gwei_decimal(bigdec: &BigDecimal) -> NumConversResult<U256> {
    u256_from_big_decimal(bigdec, ETH_GWEI_DECIMALS)
}

/// Converts a U256 wei value to an gwei value as a BigDecimal
#[inline(always)]
pub fn wei_to_gwei_decimal(wei: U256) -> NumConversResult<BigDecimal> {
    u256_to_big_decimal(wei, ETH_GWEI_DECIMALS)
}

/// Converts a U256 wei value to an ETH value as a BigDecimal
/// TODO: use wei_to_eth_decimal instead of u256_to_big_decimal(gas_cost_wei, ETH_DECIMALS)
#[inline(always)]
pub fn wei_to_eth_decimal(wei: U256) -> NumConversResult<BigDecimal> {
    u256_to_big_decimal(wei, ETH_DECIMALS)
}

#[inline]
pub fn mm_number_to_u256(mm_number: &MmNumber) -> Result<U256, FromDecStrErr> {
    U256::from_dec_str(mm_number.to_ratio().to_integer().to_string().as_str())
}

#[inline]
pub fn mm_number_from_u256(u256: U256) -> MmNumber {
    MmNumber::from(u256.to_string().as_str())
}

#[inline]
pub fn wei_from_coins_mm_number(mm_number: &MmNumber, decimals: u8) -> NumConversResult<U256> {
    u256_from_big_decimal(&mm_number.to_decimal(), decimals)
}

#[inline]
#[allow(unused)]
pub fn wei_to_coins_mm_number(u256: U256, decimals: u8) -> NumConversResult<MmNumber> {
    Ok(MmNumber::from(u256_to_big_decimal(u256, decimals)?))
}

pub(super) fn get_conf_param_or_from_plaform_coin<T: DeserializeOwned>(
    ctx: &MmArc,
    conf: &Json,
    coin_type: &EthCoinType,
    param_name: &str,
) -> Result<Option<T>, String> {
    /// Get "max_eth_tx_type" param from a token conf, or from the platform coin conf
    fn read_conf_param_or_from_plaform(ctx: &MmArc, conf: &Json, param: &str, coin_type: &EthCoinType) -> Option<Json> {
        match &coin_type {
            EthCoinType::Eth => conf.get(param).cloned(),
            EthCoinType::Erc20 { platform, .. } | EthCoinType::Nft { platform } => conf
                .get(param)
                .cloned()
                .or(coin_conf(ctx, platform).get(param).cloned()),
        }
    }

    match read_conf_param_or_from_plaform(ctx, conf, param_name, coin_type) {
        Some(val) => {
            let param_val: T = serde_json::from_value(val).map_err(|_| format!("{param_name} in coins is invalid"))?;
            Ok(Some(param_val))
        },
        None => Ok(None),
    }
}
