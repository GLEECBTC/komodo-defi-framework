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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TronTxFeeDetails {
    pub coin: String,
    pub bandwidth_used: u64,
    pub energy_used: u64,
    pub bandwidth_fee: BigDecimal,
    pub energy_fee: BigDecimal,
    pub total_fee: BigDecimal,
}

/// Snapshot of account resource usage/limits returned by TRON RPC.
///
/// Values are raw units:
/// - bandwidth: bytes
/// - energy: energy units
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TronAccountResources {
    pub free_net_used: u64,
    pub free_net_limit: u64,
    pub net_used: u64,
    pub net_limit: u64,
    pub energy_used: u64,
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
pub fn estimate_trx_transfer_fee(
    tx: &Transaction,
    resources: TronAccountResources,
    prices: TronChainPrices,
    fee_coin: &str,
) -> TronTxFeeDetails {
    estimate_fee_details(tx, 0, resources, prices, fee_coin)
}

/// Estimates fee details for TRC20 transfer (bandwidth + energy path).
///
/// `energy_used` should come from `estimateenergy`/receipt-compatible estimation.
pub fn estimate_trc20_transfer_fee(
    tx: &Transaction,
    energy_used: u64,
    resources: TronAccountResources,
    prices: TronChainPrices,
    fee_coin: &str,
) -> TronTxFeeDetails {
    estimate_fee_details(tx, energy_used, resources, prices, fee_coin)
}

/// Shared fee computation used by TRX/TRC20 paths.
///
/// Steps:
/// 1. Estimate bandwidth usage from serialized tx size.
/// 2. Compute deficits against account resources.
/// 3. Price deficits using chain prices.
/// 4. Return fixed-scale TRX decimals.
fn estimate_fee_details(
    tx: &Transaction,
    energy_used: u64,
    resources: TronAccountResources,
    prices: TronChainPrices,
    fee_coin: &str,
) -> TronTxFeeDetails {
    let bandwidth_used = estimate_bandwidth(tx);

    let bandwidth_deficit = bandwidth_used.saturating_sub(resources.available_bandwidth());
    let energy_deficit = energy_used.saturating_sub(resources.available_energy());

    let bandwidth_fee_sun = bandwidth_deficit.saturating_mul(prices.bandwidth_price_sun);
    let energy_fee_sun = energy_deficit.saturating_mul(prices.energy_price_sun);
    let total_fee_sun = bandwidth_fee_sun.saturating_add(energy_fee_sun);

    TronTxFeeDetails {
        coin: fee_coin.to_owned(),
        bandwidth_used,
        energy_used,
        bandwidth_fee: sun_to_trx_decimal(bandwidth_fee_sun),
        energy_fee: sun_to_trx_decimal(energy_fee_sun),
        total_fee: sun_to_trx_decimal(total_fee_sun),
    }
}

/// Converts SUN to TRX BigDecimal while preserving fixed 6-decimal scale.
fn sun_to_trx_decimal(sun: u64) -> BigDecimal {
    BigDecimal::new(BigInt::from(sun), i64::from(TRX_DECIMALS))
}

#[cfg(test)]
mod tests {
    use super::super::proto::{Any, ContractType, TransactionContract, TYPE_URL_TRANSFER_CONTRACT};
    use super::*;

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
        TransactionContract {
            r#type: ContractType::TransferContract as i32,
            parameter: Some(Any {
                type_url: TYPE_URL_TRANSFER_CONTRACT.to_string(),
                value: vec![1],
            }),
            permission_id: 0,
        }
    }

    #[test]
    fn bandwidth_estimation_uses_encoded_tx_size_plus_result_buffer() {
        let tx = tx_with_placeholder_signature(&sample_raw());
        let expected = tx.encoded_len() as u64 + RESULT_BYTES_OVERHEAD_PER_CONTRACT;
        assert_eq!(estimate_bandwidth(&tx), expected);
    }

    #[test]
    fn bandwidth_estimation_scales_with_contract_count() {
        let mut raw = sample_raw();
        raw.contract = vec![sample_contract(), sample_contract()];

        let tx = tx_with_placeholder_signature(&raw);
        let expected = tx.encoded_len() as u64 + RESULT_BYTES_OVERHEAD_PER_CONTRACT * 2;
        assert_eq!(estimate_bandwidth(&tx), expected);
    }

    #[test]
    fn bandwidth_estimation_defaults_to_single_contract_overhead_when_raw_is_missing() {
        let tx = Transaction {
            raw_data: None,
            signature: Vec::new(),
        };

        assert_eq!(estimate_bandwidth(&tx), RESULT_BYTES_OVERHEAD_PER_CONTRACT);
    }

    #[test]
    fn trx_fee_is_zero_when_bandwidth_is_fully_available() {
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
        };

        let details = estimate_trx_transfer_fee(&tx, resources, prices, "TRX");
        assert_eq!(details.coin, "TRX");
        assert_eq!(details.energy_used, 0);
        assert_eq!(details.bandwidth_fee, BigDecimal::from(0));
        assert_eq!(details.energy_fee, BigDecimal::from(0));
        assert_eq!(details.total_fee, BigDecimal::from(0));
    }

    #[test]
    fn trc20_fee_calculation_handles_bandwidth_and_energy_deficits() {
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
    }

    #[test]
    fn fee_calculation_saturates_on_large_inputs() {
        let tx = tx_with_placeholder_signature(&sample_raw());
        let resources = TronAccountResources::default();
        let prices = TronChainPrices {
            bandwidth_price_sun: u64::MAX,
            energy_price_sun: u64::MAX,
        };

        let details = estimate_trc20_transfer_fee(&tx, u64::MAX, resources, prices, "TRX");
        assert_eq!(details.bandwidth_fee, sun_to_trx_decimal(u64::MAX));
        assert_eq!(details.energy_fee, sun_to_trx_decimal(u64::MAX));
        assert_eq!(details.total_fee, sun_to_trx_decimal(u64::MAX));
    }

    /// Verifies that `sun_to_trx_decimal` preserves a fixed 6-digit scale in
    /// the internal `BigDecimal` representation. This guards against replacing
    /// `BigDecimal::new` with division, which would normalize whole-number
    /// results (e.g. 1 TRX becomes `(1, 0)` instead of `(1_000_000, 6)`),
    /// breaking consistent serialization.
    #[test]
    fn sun_to_trx_decimal_uses_fixed_six_decimal_scale() {
        let one_sun = sun_to_trx_decimal(1);
        let one_trx = sun_to_trx_decimal(1_000_000);

        let (one_sun_int, one_sun_scale) = one_sun.as_bigint_and_exponent();
        let (one_trx_int, one_trx_scale) = one_trx.as_bigint_and_exponent();

        assert_eq!(one_sun_int, BigInt::from(1));
        assert_eq!(one_sun_scale, i64::from(TRX_DECIMALS));

        // Whole TRX value must still be stored as (1_000_000, 6), not normalized to (1, 0).
        assert_eq!(one_trx_int, BigInt::from(1_000_000u64));
        assert_eq!(one_trx_scale, i64::from(TRX_DECIMALS));
    }
}
