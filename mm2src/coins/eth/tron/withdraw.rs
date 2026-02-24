//! TRON withdrawal pipeline.
//!
//! Free functions that build, estimate fees for, and prepare TRON withdrawal
//! transactions (TRX native and TRC20 token). Signing and `TransactionDetails`
//! assembly happen in the calling `EthWithdraw` trait (see `eth_withdraw.rs`).

use crate::eth::chain_rpc::ChainRpcOps;
use crate::eth::tron::fee::{
    estimate_trc20_transfer_fee, estimate_trx_transfer_fee, tx_with_placeholder_signature, TronAccountResources,
    TronChainPrices, TronTxFeeDetails,
};
use crate::eth::tron::proto::TransactionRaw;
use crate::eth::tron::tx_builder::{build_trc20_transfer, build_trx_transfer};
use crate::eth::tron::{TaposBlockData, TronAddress, TronApiClient, TRX_DECIMALS};
use crate::eth::{u256_from_big_decimal, u256_to_big_decimal};
use crate::{WithdrawError, WithdrawFee};
use ethereum_types::U256;
use mm2_err_handle::map_mm_error::MmResultExt;
use mm2_err_handle::prelude::{MapToMmResult, MmError};
use mm2_number::bigdecimal::BigDecimal;
use std::convert::TryInto;

/// Shared context for TRON withdrawal operations.
///
/// Groups the parameters common to both TRX and TRC20 withdrawals: sender/recipient
/// addresses, block data for TAPOS, account resources, chain prices, and fee coin.
pub struct TronWithdrawContext<'a> {
    pub from: &'a TronAddress,
    pub to: &'a TronAddress,
    pub block_data: &'a TaposBlockData,
    pub resources: TronAccountResources,
    pub prices: TronChainPrices,
    pub fee_coin: &'a str,
}

/// Reject EVM gas fee policies for TRON. TRON always auto-estimates fees.
#[allow(clippy::result_large_err)]
pub fn validate_tron_fee_policy(fee: &Option<WithdrawFee>) -> Result<(), MmError<WithdrawError>> {
    match fee {
        None => Ok(()),
        Some(WithdrawFee::EthGas { .. }) | Some(WithdrawFee::EthGasEip1559 { .. }) => {
            MmError::err(WithdrawError::InvalidFeePolicy(
                "EVM gas fee options are not supported for TRON withdraw; omit the fee field".to_owned(),
            ))
        },
        Some(other) => MmError::err(WithdrawError::InvalidFeePolicy(format!(
            "Manual fee ({:?}) is not supported for TRON withdraw; omit the fee field",
            other
        ))),
    }
}

/// Convert a U256 to u64, returning `WithdrawError` on overflow.
#[allow(clippy::result_large_err)]
pub fn u256_to_u64_checked(value: U256) -> Result<u64, MmError<WithdrawError>> {
    if value > U256::from(u64::MAX) {
        return MmError::err(WithdrawError::InternalError(format!("value {value} exceeds u64::MAX")));
    }
    Ok(value.as_u64())
}

/// Build TRX (native) withdraw: estimate fees, handle max-deduction, return final tx raw.
#[allow(clippy::result_large_err)]
pub fn build_tron_trx_withdraw(
    ctx: &TronWithdrawContext,
    amount_base_units: U256,
    my_balance: U256,
    my_balance_dec: &BigDecimal,
    is_max: bool,
) -> Result<(TransactionRaw, TronTxFeeDetails, U256), MmError<WithdrawError>> {
    let balance_sun = u256_to_u64_checked(my_balance)?;
    let mut amount_sun = u256_to_u64_checked(amount_base_units)?;
    let mut amount_sun_i64: i64 = amount_sun
        .try_into()
        .map_to_mm(|_| WithdrawError::InternalError(format!("amount {amount_sun} exceeds i64::MAX")))?;
    let mut raw = build_trx_transfer(ctx.from, ctx.to, amount_sun_i64, ctx.block_data);

    // Iteratively estimate fee and adjust amount until stable.
    // Non-max: runs once — amount is fixed, just checks balance sufficiency.
    // Max: converges in 1-2 iterations — fee depends on tx size (varint-encoded
    // amount), so changing the amount can change the fee. The `>=` (not `==`)
    // break prevents infinite oscillation at varint boundaries, where reducing
    // the amount lowers the fee but increasing it raises the fee back. This may
    // leave up to 1 bandwidth byte of dust (~1000 SUN) in that rare edge case.
    loop {
        // Estimate fee for the current transaction
        let tx = tx_with_placeholder_signature(&raw);
        let fee_details = estimate_trx_transfer_fee(&tx, ctx.resources, ctx.prices, ctx.fee_coin);
        let fee_sun = u256_to_u64_checked(u256_from_big_decimal(&fee_details.total_fee, TRX_DECIMALS).map_mm_err()?)?;

        // How much can we afford to send after paying the fee? (0 if fee >= balance)
        let affordable = balance_sun.saturating_sub(fee_sun);

        // Balance covers amount + fee — done.
        if affordable >= amount_sun {
            return Ok((raw, fee_details, U256::from(amount_sun)));
        }

        // Non-max: amount is user-specified and can't be reduced — insufficient balance.
        if !is_max {
            let required =
                u256_to_big_decimal(U256::from(amount_sun) + U256::from(fee_sun), TRX_DECIMALS).map_mm_err()?;
            return MmError::err(WithdrawError::NotSufficientBalance {
                coin: ctx.fee_coin.to_owned(),
                available: my_balance_dec.clone(),
                required,
            });
        }

        // Max: fee consumes the entire balance, nothing left to send.
        if affordable == 0 {
            return MmError::err(WithdrawError::AmountTooLow {
                amount: BigDecimal::from(0),
                threshold: fee_details.total_fee.clone(),
            });
        }

        // Max: reduce amount to what's affordable after fee, rebuild tx and re-estimate.
        amount_sun = affordable;
        amount_sun_i64 = amount_sun
            .try_into()
            .map_to_mm(|_| WithdrawError::InternalError(format!("amount {amount_sun} exceeds i64::MAX")))?;
        raw = build_trx_transfer(ctx.from, ctx.to, amount_sun_i64, ctx.block_data);
    }
}

/// Build TRC20 withdraw: estimate energy + bandwidth fees, return final tx raw.
pub async fn build_tron_trc20_withdraw(
    ctx: &TronWithdrawContext<'_>,
    tron: &TronApiClient,
    contract_tron: &TronAddress,
    amount_base_units: U256,
) -> Result<(TransactionRaw, TronTxFeeDetails, U256), MmError<WithdrawError>> {
    // Estimate energy for TRC20 transfer
    let energy_used = tron
        .estimate_trc20_transfer_energy(ctx.from, contract_tron, ctx.to, amount_base_units)
        .await
        .map_mm_err()?;

    // Compute fee_limit as full energy cap (max TRX burn allowed for energy in SUN).
    // The actual paid fee is still calculated separately via `estimate_trc20_transfer_fee`.
    let fee_limit_sun = energy_used.saturating_mul(ctx.prices.energy_price_sun);
    let fee_limit_i64: i64 = fee_limit_sun
        .try_into()
        .map_to_mm(|_| WithdrawError::InternalError(format!("fee_limit {fee_limit_sun} exceeds i64::MAX")))?;

    // Build unsigned TRC20 transfer tx
    let raw = build_trc20_transfer(
        ctx.from,
        contract_tron,
        ctx.to,
        amount_base_units,
        fee_limit_i64,
        ctx.block_data,
    )
    .map_to_mm(|e| WithdrawError::InternalError(format!("TRC20 ABI encoding failed: {e}")))?;

    // Estimate fee details (bandwidth + energy)
    let tx = tx_with_placeholder_signature(&raw);
    let fee_details = estimate_trc20_transfer_fee(&tx, energy_used, ctx.resources, ctx.prices, ctx.fee_coin);

    // Verify sufficient TRX balance for fees (fees are paid in TRX, not the token)
    let trx_balance = tron.balance_native(*ctx.from).await.map_mm_err()?;
    let total_fee_u256 = u256_from_big_decimal(&fee_details.total_fee, TRX_DECIMALS).map_mm_err()?;

    if trx_balance < total_fee_u256 {
        let trx_balance_dec = u256_to_big_decimal(trx_balance, TRX_DECIMALS).map_mm_err()?;
        return MmError::err(WithdrawError::NotSufficientPlatformBalanceForFee {
            coin: ctx.fee_coin.to_owned(),
            available: trx_balance_dec,
            required: fee_details.total_fee.clone(),
        });
    }

    // TRC20 max or non-max: token amount is NOT reduced by fees
    Ok((raw, fee_details, amount_base_units))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::tron::fee::estimate_trx_transfer_fee;
    use crate::eth::tron::sign::sign_tron_transaction;
    use crate::eth::tron::test_fixtures::{nile_block_64687673, TEST_FROM_HEX, TEST_TO_HEX};
    use crate::eth::tron::tx_builder::build_trx_transfer;
    use crate::eth::tron::TronAddress;
    use mm2_number::bigdecimal::BigDecimal;
    use prost::Message;

    #[test]
    fn validate_tron_fee_policy_rejects_evm_gas_options() {
        // None (auto) is accepted
        assert!(validate_tron_fee_policy(&None).is_ok());

        // EthGas is rejected
        let eth_gas = Some(WithdrawFee::EthGas {
            gas_price: BigDecimal::from(20),
            gas: 21_000,
        });
        let err = validate_tron_fee_policy(&eth_gas).unwrap_err().into_inner();
        assert!(matches!(err, WithdrawError::InvalidFeePolicy(_)));

        // EthGasEip1559 is rejected
        let eip1559 = Some(WithdrawFee::EthGasEip1559 {
            max_priority_fee_per_gas: BigDecimal::from(2),
            max_fee_per_gas: BigDecimal::from(30),
            gas_option: crate::EthGasLimitOption::Calc,
        });
        let err = validate_tron_fee_policy(&eip1559).unwrap_err().into_inner();
        assert!(matches!(err, WithdrawError::InvalidFeePolicy(_)));

        // Other fee types (e.g. UtxoFixed) are also rejected for TRON
        let utxo = Some(WithdrawFee::UtxoFixed {
            amount: BigDecimal::from(1),
        });
        let err = validate_tron_fee_policy(&utxo).unwrap_err().into_inner();
        assert!(matches!(err, WithdrawError::InvalidFeePolicy(_)));
    }

    #[test]
    fn tron_signed_protobuf_bytes_are_not_valid_rlp() {
        // Build a deterministic TRON transaction
        let block_data = TaposBlockData {
            number: 54_242_114,
            block_id: {
                let bytes = hex::decode("00000000033bab42567444cc8af3dbaeb5cf26b514b7e90b9a23424ea8392641").unwrap();
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                arr
            },
            timestamp: 1_738_799_040_000,
        };
        let from = TronAddress::from_hex(TEST_FROM_HEX).unwrap();
        let to = TronAddress::from_hex(TEST_TO_HEX).unwrap();
        let raw = build_trx_transfer(&from, &to, 1_000_000, &block_data);

        // Sign with a deterministic test key
        let secret = ethkey::Secret::from_slice(&[1u8; 32]).expect("valid test secret");
        let (_tx_id, signed_tx) = sign_tron_transaction(&raw, &secret).unwrap();

        // Encode to protobuf bytes (what would be passed to send_raw_tx)
        let tx_bytes = signed_tx.encode_to_vec();

        // These protobuf bytes must NOT be decodable as EVM RLP
        let rlp_result = crate::eth::signed_eth_tx_from_bytes(&tx_bytes);
        assert!(rlp_result.is_err(), "TRON protobuf bytes must not decode as EVM RLP");
    }

    #[test]
    fn trx_max_withdraw_deducts_fee_and_returns_consistent_details() {
        let from = TronAddress::from_hex(TEST_FROM_HEX).unwrap();
        let to = TronAddress::from_hex(TEST_TO_HEX).unwrap();
        let block_data = nile_block_64687673();

        let balance = U256::from(10_000_000u64); // 10 TRX
        let balance_dec = BigDecimal::from(10);
        let resources = TronAccountResources::default(); // no free bandwidth
        let prices = TronChainPrices {
            bandwidth_price_sun: 1_000,
            energy_price_sun: 420,
        };

        let ctx = TronWithdrawContext {
            from: &from,
            to: &to,
            block_data: &block_data,
            resources,
            prices,
            fee_coin: "TRX",
        };
        let (raw, fee_details, final_amount) =
            build_tron_trx_withdraw(&ctx, balance, balance, &balance_dec, true).unwrap();

        // Verify: final_amount + fee <= balance
        let fee_sun =
            u256_to_u64_checked(u256_from_big_decimal(&fee_details.total_fee, TRX_DECIMALS).unwrap()).unwrap();
        assert!(final_amount.as_u64() + fee_sun <= balance.as_u64());
        assert!(final_amount > U256::zero());

        // Verify fee_details corresponds to the final raw tx
        let tx = tx_with_placeholder_signature(&raw);
        let recomputed = estimate_trx_transfer_fee(&tx, resources, prices, "TRX");
        assert_eq!(fee_details, recomputed, "fee_details must match the final raw tx");
    }

    #[test]
    fn trx_non_max_withdraw_rejects_insufficient_balance() {
        let from = TronAddress::from_hex(TEST_FROM_HEX).unwrap();
        let to = TronAddress::from_hex(TEST_TO_HEX).unwrap();
        let block_data = nile_block_64687673();

        let balance = U256::from(1_000_000u64); // 1 TRX
        let balance_dec = BigDecimal::from(1);
        let amount = U256::from(999_999u64); // just under 1 TRX — fee will push it over
        let resources = TronAccountResources::default();
        let prices = TronChainPrices {
            bandwidth_price_sun: 1_000,
            energy_price_sun: 420,
        };

        let ctx = TronWithdrawContext {
            from: &from,
            to: &to,
            block_data: &block_data,
            resources,
            prices,
            fee_coin: "TRX",
        };
        let result = build_tron_trx_withdraw(&ctx, amount, balance, &balance_dec, false);

        assert!(result.is_err());
        let err = result.unwrap_err().into_inner();
        assert!(matches!(err, WithdrawError::NotSufficientBalance { .. }));
    }
}
