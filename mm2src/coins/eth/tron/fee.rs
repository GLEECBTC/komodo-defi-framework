//! TRON fee estimation for bandwidth and energy costs.
//!
//! Bandwidth is charged per byte of serialized transaction; energy is charged per
//! unit consumed by smart contract execution (TRC20 transfers). Both are priced in
//! SUN and converted to TRX at the current chain rate.

use super::proto::{Transaction, TransactionRaw};
use super::TRX_DECIMALS;
use mm2_number::bigdecimal::BigDecimal;
use mm2_number::BigInt;
use prost::Message;
use serde::{Deserialize, Serialize};

/// Per-contract TRON bandwidth overhead in bytes.
///
/// Java-tron charges bandwidth as:
/// `tx.clearRet().getSerializedSize() + contract_count * MAX_RESULT_SIZE_IN_TX`,
/// with `MAX_RESULT_SIZE_IN_TX = 64`. We mirror that as
/// `encoded_len + 64 * contract_count` to avoid underestimating multi-contract txs.
///
/// References:
/// - https://github.com/tronprotocol/java-tron/blob/develop/chainbase/src/main/java/org/tron/core/db/BandwidthProcessor.java#L117-L128
/// - https://github.com/tronprotocol/java-tron/blob/develop/common/src/main/java/org/tron/core/Constant.java#L41
const RESULT_BYTES_OVERHEAD_PER_CONTRACT: u64 = 64;
/// TRON signatures are 65 bytes (`r || s || v`), used here as estimation placeholder.
const PLACEHOLDER_SIGNATURE_LEN: usize = 65;

/// Fee breakdown for a TRON transaction.
///
/// All monetary fields (`bandwidth_fee`, `energy_fee`, `total_fee`) are in TRX (not SUN).
/// Resource fields (`bandwidth_used`, `energy_used`) are in native units (bytes / energy units).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TronTxFeeDetails {
    pub coin: String,
    pub bandwidth_used: u64,
    pub energy_used: u64,
    pub bandwidth_fee: BigDecimal,
    pub energy_fee: BigDecimal,
    /// Fee for creating a new account at the destination address (burned by the network).
    /// `None` when the destination already exists on chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_creation_fee: Option<BigDecimal>,
    pub total_fee: BigDecimal,
}

/// Subset of TRON's `AccountResourceMessage` (defined in `api.proto` of the
/// [tronprotocol/protocol](https://github.com/tronprotocol/protocol/blob/master/api/api.proto)
/// repo). The full message has 17 fields covering bandwidth, energy, storage,
/// and Tron Power; we only deserialize the 6 we need for fee estimation.
///
/// Returned by the `/wallet/getaccountresource` HTTP endpoint as proto3 JSON.
/// Proto3 JSON keeps the original proto field names as-is, which is why the
/// casing is mixed (`freeNetUsed` vs `NetUsed` vs `EnergyUsed`).
///
/// All fields use `#[serde(default)]` because proto3 JSON omits zero-value
/// fields. An empty `{}` response (unactivated account) deserializes to all
/// zeros.
///
/// Values are raw units:
/// - bandwidth: bytes
/// - energy: energy units
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct TronAccountResources {
    #[serde(default, rename = "freeNetUsed")]
    pub free_net_used: u64,
    #[serde(default, rename = "freeNetLimit")]
    pub free_net_limit: u64,
    #[serde(default, rename = "NetUsed")]
    pub net_used: u64,
    #[serde(default, rename = "NetLimit")]
    pub net_limit: u64,
    #[serde(default, rename = "EnergyUsed")]
    pub energy_used: u64,
    #[serde(default, rename = "EnergyLimit")]
    pub energy_limit: u64,
}

impl TronAccountResources {
    /// Total bandwidth still available to the account:
    /// `max(0, free_limit - free_used) + max(0, staked_limit - staked_used)`.
    pub fn available_bandwidth(&self) -> u64 {
        let free_bandwidth = self.free_net_limit.saturating_sub(self.free_net_used);
        let staked_bandwidth = self.net_limit.saturating_sub(self.net_used);
        free_bandwidth.saturating_add(staked_bandwidth)
    }

    /// Available staked bandwidth only: `max(0, net_limit - net_used)`.
    pub fn available_staked_bandwidth(&self) -> u64 {
        self.net_limit.saturating_sub(self.net_used)
    }

    /// Energy still available to the account: `max(0, energy_limit - energy_used)`.
    pub fn available_energy(&self) -> u64 {
        self.energy_limit.saturating_sub(self.energy_used)
    }
}

/// Current chain prices (SUN per unit) from chain parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TronChainPrices {
    /// SUN per bandwidth byte (`getTransactionFee`).
    pub bandwidth_price_sun: u64,
    /// SUN per energy unit (`getEnergyFee`).
    pub energy_price_sun: u64,
    /// Flat SUN fee for creating a new account via system contract
    /// (`getCreateNewAccountFeeInSystemContract`). Currently 1 TRX on mainnet.
    pub create_new_account_fee_sun: u64,
    /// Flat SUN fee charged when sender lacks bandwidth for account creation
    /// (`getCreateAccountFee`). Currently 0.1 TRX on mainnet.
    pub create_account_bandwidth_fee_sun: u64,
    /// Multiplier for bandwidth consumed by account creation
    /// (`getCreateNewAccountBandwidthRate`).
    pub create_new_account_bandwidth_rate: u64,
}

/// Account creation fee context for a withdrawal destination.
///
/// Used instead of a raw `bool` to make call sites self-documenting and to carry
/// the precomputed fee values from chain parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DestAccountState {
    /// Destination account already exists on chain. No creation fees apply.
    Activated,
    /// Destination is a new/unactivated address. Creation fees apply for native
    /// TRX transfers (`TransferContract`). TRC20 transfers use energy for account
    /// creation instead, so they should always use `Activated`.
    NewAccount {
        /// `CreateNewAccountFeeInSystemContract` in SUN — always charged.
        creation_fee_sun: u64,
        /// `CreateAccountFee` in SUN — flat fallback if sender lacks bandwidth
        /// for the account creation bandwidth portion.
        bandwidth_fallback_sun: u64,
        /// `CreateNewAccountBandwidthRate` multiplier applied to account
        /// creation bandwidth cost.
        bandwidth_rate: u64,
    },
}

/// Builds a transaction clone with a synthetic 65-byte signature.
///
/// Bandwidth depends on full serialized transaction size, and signature bytes are
/// part of that size. For pre-sign estimation, we use a placeholder signature
/// matching TRON's real signature length.
pub fn tx_with_placeholder_signature(raw: &TransactionRaw) -> Transaction {
    Transaction {
        raw_data: Some(raw.clone()),
        signature: vec![vec![0u8; PLACEHOLDER_SIGNATURE_LEN]],
    }
}

/// Estimates bandwidth bytes charged for this transaction.
///
/// Formula:
/// `encoded_tx_size + RESULT_BYTES_OVERHEAD_PER_CONTRACT * contract_count`
///
/// `contract_count` is clamped to at least 1 to keep estimation conservative when
/// tx metadata is missing.
pub fn estimate_bandwidth(tx: &Transaction) -> u64 {
    let contract_count = tx
        .raw_data
        .as_ref()
        .map(|raw| raw.contract.len().max(1) as u64)
        .unwrap_or(1);
    let tx_size = tx.encoded_len() as u64;
    tx_size.saturating_add(RESULT_BYTES_OVERHEAD_PER_CONTRACT.saturating_mul(contract_count))
}

/// Estimates fee details for native TRX transfer (bandwidth-only path).
///
/// When `dest_state` is `NewAccount`, includes the account creation fee and adjusts
/// bandwidth calculations for the extra bandwidth consumed by account creation.
pub fn estimate_trx_transfer_fee(
    tx: &Transaction,
    resources: TronAccountResources,
    prices: TronChainPrices,
    fee_coin: &str,
    dest_state: DestAccountState,
) -> TronTxFeeDetails {
    estimate_fee_details(tx, 0, resources, prices, fee_coin, dest_state)
}

/// Estimates fee details for TRC20 transfer (bandwidth + energy path).
///
/// `energy_used` should come from `estimateenergy`/receipt-compatible estimation.
///
/// TRC20 transfers do not require account activation and therefore do not charge
/// account-activation fees. The energy estimation API already accounts for the
/// extra energy cost of sending to an unactivated address.
pub fn estimate_trc20_transfer_fee(
    tx: &Transaction,
    energy_used: u64,
    resources: TronAccountResources,
    prices: TronChainPrices,
    fee_coin: &str,
) -> TronTxFeeDetails {
    // TRC20 never uses system-contract account creation fees.
    estimate_fee_details(
        tx,
        energy_used,
        resources,
        prices,
        fee_coin,
        DestAccountState::Activated,
    )
}

/// Shared fee computation used by TRX/TRC20 paths.
///
/// Steps:
/// 1. Estimate bandwidth usage from serialized tx size.
/// 2. If destination is unactivated, use account-creation charging path.
/// 3. Otherwise (activated destination), compute regular bandwidth deficit fee.
/// 4. Price deficits using chain prices.
/// 5. Return fixed-scale TRX decimals.
fn estimate_fee_details(
    tx: &Transaction,
    energy_used: u64,
    resources: TronAccountResources,
    prices: TronChainPrices,
    fee_coin: &str,
    dest_state: DestAccountState,
) -> TronTxFeeDetails {
    let mut bandwidth_used = estimate_bandwidth(tx);

    let (account_creation_fee_sun, bandwidth_fee_sun) = match dest_state {
        DestAccountState::Activated => {
            let bandwidth_deficit = bandwidth_used.saturating_sub(resources.available_bandwidth());
            (0u64, bandwidth_deficit.saturating_mul(prices.bandwidth_price_sun))
        },
        DestAccountState::NewAccount {
            creation_fee_sun,
            bandwidth_fallback_sun,
            bandwidth_rate,
        } => {
            let available = resources.available_staked_bandwidth();
            // Account creation bandwidth uses charged bandwidth units for this tx,
            // multiplied by `getCreateNewAccountBandwidthRate`.
            // If sender has enough staked bandwidth, it's consumed; otherwise the flat fallback fee applies.
            bandwidth_used = bandwidth_used.saturating_mul(bandwidth_rate);
            if available >= bandwidth_used {
                // Staked bandwidth covers account creation.
                // For create-account transactions, java-tron does not additionally apply
                // regular per-byte bandwidth fee (`getTransactionFee`) path.
                (creation_fee_sun, 0)
            } else {
                // Staked bandwidth insufficient — flat fallback fee.
                bandwidth_used = 0;
                (creation_fee_sun, bandwidth_fallback_sun)
            }
        },
    };

    let energy_deficit = energy_used.saturating_sub(resources.available_energy());
    let energy_fee_sun = energy_deficit.saturating_mul(prices.energy_price_sun);
    let total_fee_sun = bandwidth_fee_sun
        .saturating_add(energy_fee_sun)
        .saturating_add(account_creation_fee_sun);

    let account_creation_fee = if account_creation_fee_sun > 0 {
        Some(sun_to_trx_decimal(account_creation_fee_sun))
    } else {
        None
    };

    TronTxFeeDetails {
        coin: fee_coin.to_owned(),
        bandwidth_used,
        energy_used,
        bandwidth_fee: sun_to_trx_decimal(bandwidth_fee_sun),
        energy_fee: sun_to_trx_decimal(energy_fee_sun),
        account_creation_fee,
        total_fee: sun_to_trx_decimal(total_fee_sun),
    }
}

/// Converts SUN to TRX BigDecimal while preserving fixed 6-decimal scale.
fn sun_to_trx_decimal(sun: u64) -> BigDecimal {
    BigDecimal::new(BigInt::from(sun), i64::from(TRX_DECIMALS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::tron::proto::{ContractType, TransactionContract, TYPE_URL_TRANSFER_CONTRACT};
    use crate::eth::tron::test_fixtures::{mainnet_prices, new_account_state};
    use crate::eth::tron::tx_builder::wrap_contract;
    use common::cross_test;

    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::*;

    fn sample_raw() -> TransactionRaw {
        TransactionRaw {
            ref_block_bytes: vec![0x00, 0x01],
            ref_block_hash: vec![0u8; 8],
            expiration: 1_770_522_483_000,
            data: Vec::new(),
            contract: Vec::new(),
            timestamp: 1_770_522_424_709,
            fee_limit: 0,
        }
    }

    fn sample_contract() -> TransactionContract {
        wrap_contract(ContractType::TransferContract, TYPE_URL_TRANSFER_CONTRACT, vec![1])
    }

    cross_test!(bandwidth_estimation_uses_encoded_tx_size_plus_result_buffer, {
        let tx = tx_with_placeholder_signature(&sample_raw());
        let expected = tx.encoded_len() as u64 + RESULT_BYTES_OVERHEAD_PER_CONTRACT;
        assert_eq!(estimate_bandwidth(&tx), expected);
    });

    cross_test!(bandwidth_estimation_scales_with_contract_count, {
        let mut raw = sample_raw();
        raw.contract = vec![sample_contract(), sample_contract()];

        let tx = tx_with_placeholder_signature(&raw);
        let expected = tx.encoded_len() as u64 + RESULT_BYTES_OVERHEAD_PER_CONTRACT * 2;
        assert_eq!(estimate_bandwidth(&tx), expected);
    });

    cross_test!(
        bandwidth_estimation_defaults_to_single_contract_overhead_when_raw_is_missing,
        {
            let tx = Transaction {
                raw_data: None,
                signature: Vec::new(),
            };

            assert_eq!(estimate_bandwidth(&tx), RESULT_BYTES_OVERHEAD_PER_CONTRACT);
        }
    );

    cross_test!(trx_fee_is_zero_when_bandwidth_is_fully_available, {
        let tx = tx_with_placeholder_signature(&sample_raw());
        let bandwidth_used = estimate_bandwidth(&tx);
        let resources = TronAccountResources {
            free_net_used: 0,
            free_net_limit: bandwidth_used,
            net_used: 0,
            net_limit: 0,
            energy_used: 0,
            energy_limit: 0,
        };
        let prices = TronChainPrices {
            bandwidth_price_sun: 1_000,
            energy_price_sun: 420,
            create_new_account_fee_sun: 0,
            create_account_bandwidth_fee_sun: 0,
            create_new_account_bandwidth_rate: 1,
        };

        let details = estimate_trx_transfer_fee(&tx, resources, prices, "TRX", DestAccountState::Activated);
        assert_eq!(details.coin, "TRX");
        assert_eq!(details.energy_used, 0);
        assert_eq!(details.bandwidth_fee, BigDecimal::from(0));
        assert_eq!(details.energy_fee, BigDecimal::from(0));
        assert_eq!(details.account_creation_fee, None);
        assert_eq!(details.total_fee, BigDecimal::from(0));
    });

    cross_test!(trc20_fee_calculation_handles_bandwidth_and_energy_deficits, {
        let tx = tx_with_placeholder_signature(&sample_raw());
        let bandwidth_used = estimate_bandwidth(&tx);
        let resources = TronAccountResources {
            free_net_used: 100,
            free_net_limit: 100, // no free bandwidth left
            net_used: 30,
            net_limit: 80, // 50 bandwidth left
            energy_used: 200,
            energy_limit: 300, // 100 energy left
        };
        let prices = TronChainPrices {
            bandwidth_price_sun: 1_000,
            energy_price_sun: 420,
            create_new_account_fee_sun: 0,
            create_account_bandwidth_fee_sun: 0,
            create_new_account_bandwidth_rate: 1,
        };
        let energy_used = 500u64;

        let details = estimate_trc20_transfer_fee(&tx, energy_used, resources, prices, "TRX");

        let bandwidth_deficit = bandwidth_used.saturating_sub(50);
        let expected_bw_fee_sun = bandwidth_deficit * 1_000;
        let expected_energy_fee_sun = 400 * 420;
        let expected_total_fee_sun = expected_bw_fee_sun + expected_energy_fee_sun;

        assert_eq!(details.bandwidth_used, bandwidth_used);
        assert_eq!(details.energy_used, energy_used);
        assert_eq!(details.bandwidth_fee, sun_to_trx_decimal(expected_bw_fee_sun));
        assert_eq!(details.energy_fee, sun_to_trx_decimal(expected_energy_fee_sun));
        assert_eq!(details.total_fee, sun_to_trx_decimal(expected_total_fee_sun));
    });

    cross_test!(fee_calculation_saturates_on_large_inputs, {
        let tx = tx_with_placeholder_signature(&sample_raw());
        let resources = TronAccountResources::default();
        let prices = TronChainPrices {
            bandwidth_price_sun: u64::MAX,
            energy_price_sun: u64::MAX,
            create_new_account_fee_sun: 0,
            create_account_bandwidth_fee_sun: 0,
            create_new_account_bandwidth_rate: 1,
        };

        let details = estimate_trc20_transfer_fee(&tx, u64::MAX, resources, prices, "TRX");
        assert_eq!(details.bandwidth_fee, sun_to_trx_decimal(u64::MAX));
        assert_eq!(details.energy_fee, sun_to_trx_decimal(u64::MAX));
        assert_eq!(details.total_fee, sun_to_trx_decimal(u64::MAX));
    });

    cross_test!(account_creation_fee_included_for_new_dest_no_bandwidth, {
        let tx = tx_with_placeholder_signature(&sample_raw());
        let resources = TronAccountResources::default(); // no bandwidth
        let prices = mainnet_prices();

        let details = estimate_trx_transfer_fee(&tx, resources, prices, "TRX", new_account_state());

        // account_creation_fee should be 1 TRX
        assert_eq!(details.account_creation_fee, Some(sun_to_trx_decimal(1_000_000)));
        // bandwidth_fee should be only the 0.1 TRX flat fallback fee
        // (`getCreateAccountFee`) for create-account path.
        let expected_bw_fee = 100_000;
        assert_eq!(details.bandwidth_fee, sun_to_trx_decimal(expected_bw_fee));
        // total = bandwidth_fee + account_creation_fee
        let expected_total = expected_bw_fee + 1_000_000;
        assert_eq!(details.total_fee, sun_to_trx_decimal(expected_total));
    });

    cross_test!(account_creation_fee_no_extra_bw_fee_when_bandwidth_available, {
        let tx = tx_with_placeholder_signature(&sample_raw());
        let bandwidth_used = estimate_bandwidth(&tx);
        // Give sender enough staked bandwidth only for account creation path.
        // Free bandwidth is ignored for create-account flows.
        let resources = TronAccountResources {
            free_net_used: 0,
            free_net_limit: 0,
            net_used: 0,
            net_limit: bandwidth_used,
            energy_used: 0,
            energy_limit: 0,
        };
        let prices = mainnet_prices();

        let details = estimate_trx_transfer_fee(&tx, resources, prices, "TRX", new_account_state());

        // account_creation_fee should be 1 TRX
        assert_eq!(details.account_creation_fee, Some(sun_to_trx_decimal(1_000_000)));
        // bandwidth_fee should be 0 — sender has enough staked bandwidth
        assert_eq!(details.bandwidth_fee, BigDecimal::from(0));
        // total = just the 1 TRX creation fee
        assert_eq!(details.total_fee, sun_to_trx_decimal(1_000_000));
    });

    cross_test!(account_creation_fee_fallback_when_only_free_bandwidth_available, {
        let tx = tx_with_placeholder_signature(&sample_raw());
        let bandwidth_used = estimate_bandwidth(&tx);
        // Although free bandwidth is sufficient, staked bandwidth is zero, which is mandatory for withdraws that trigger account creation.
        let resources = TronAccountResources {
            free_net_used: 0,
            free_net_limit: bandwidth_used,
            net_used: 0,
            net_limit: 0,
            energy_used: 0,
            energy_limit: 0,
        };
        let prices = mainnet_prices();

        let details = estimate_trx_transfer_fee(&tx, resources, prices, "TRX", new_account_state());

        // account_creation_fee should be 1 TRX.
        assert_eq!(details.account_creation_fee, Some(sun_to_trx_decimal(1_000_000)));
        // staked bandwidth missing triggers fallback. bandwidth_fee should be 0.1 TRX.
        assert_eq!(details.bandwidth_fee, sun_to_trx_decimal(100_000));
        // total = 1.1 TRX
        assert_eq!(details.total_fee, sun_to_trx_decimal(1_100_000));
    });

    cross_test!(account_creation_fee_none_for_activated_destination, {
        let tx = tx_with_placeholder_signature(&sample_raw());
        let resources = TronAccountResources::default();
        let prices = mainnet_prices();

        let details = estimate_trx_transfer_fee(&tx, resources, prices, "TRX", DestAccountState::Activated);
        assert_eq!(details.account_creation_fee, None);
    });

    cross_test!(serde_roundtrip_with_account_creation_fee, {
        let with_fee = TronTxFeeDetails {
            coin: "TRX".to_owned(),
            bandwidth_used: 300,
            energy_used: 0,
            bandwidth_fee: sun_to_trx_decimal(400_000),
            energy_fee: sun_to_trx_decimal(0),
            account_creation_fee: Some(sun_to_trx_decimal(1_000_000)),
            total_fee: sun_to_trx_decimal(1_400_000),
        };
        let json = serde_json::to_string(&with_fee).unwrap();
        assert!(json.contains("account_creation_fee"));
        let deserialized: TronTxFeeDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(with_fee, deserialized);
    });

    cross_test!(serde_roundtrip_without_account_creation_fee, {
        let without_fee = TronTxFeeDetails {
            coin: "TRX".to_owned(),
            bandwidth_used: 300,
            energy_used: 0,
            bandwidth_fee: sun_to_trx_decimal(400_000),
            energy_fee: sun_to_trx_decimal(0),
            account_creation_fee: None,
            total_fee: sun_to_trx_decimal(400_000),
        };
        let json = serde_json::to_string(&without_fee).unwrap();
        // skip_serializing_if omits the field when None
        assert!(!json.contains("account_creation_fee"));
        let deserialized: TronTxFeeDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(without_fee, deserialized);
    });

    // Verifies that `sun_to_trx_decimal` preserves a fixed 6-digit scale in
    // the internal `BigDecimal` representation. This guards against replacing
    // `BigDecimal::new` with division, which would normalize whole-number
    // results (e.g. 1 TRX becomes `(1, 0)` instead of `(1_000_000, 6)`),
    // breaking consistent serialization.
    cross_test!(sun_to_trx_decimal_uses_fixed_six_decimal_scale, {
        let one_sun = sun_to_trx_decimal(1);
        let one_trx = sun_to_trx_decimal(1_000_000);

        let (one_sun_int, one_sun_scale) = one_sun.as_bigint_and_exponent();
        let (one_trx_int, one_trx_scale) = one_trx.as_bigint_and_exponent();

        assert_eq!(one_sun_int, BigInt::from(1));
        assert_eq!(one_sun_scale, i64::from(TRX_DECIMALS));

        // Whole TRX value must still be stored as (1_000_000, 6), not normalized to (1, 0).
        assert_eq!(one_trx_int, BigInt::from(1_000_000u64));
        assert_eq!(one_trx_scale, i64::from(TRX_DECIMALS));
    });
}
