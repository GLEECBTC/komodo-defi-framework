use super::{EstimationSource, FeePerGasEstimated, FeePerGasLevel, PriorityLevelId, FEE_PRIORITY_LEVEL_N};
use crate::eth::web3_transport::FeeHistoryResult;
use crate::eth::{wei_from_gwei_decimal, wei_to_gwei_decimal, EthCoin, Web3RpcError, Web3RpcResult};
use mm2_err_handle::mm_error::MmError;
use mm2_err_handle::or_mm_error::OrMmError;
use mm2_err_handle::prelude::MmResultExt;
use mm2_number::BigDecimal;

use ethereum_types::U256;
use num_traits::FromPrimitive;
use web3::types::BlockNumber;

/// Simple priority fee per gas estimator based on fee history
/// normally used if gas api provider is not available
pub(crate) struct FeePerGasSimpleEstimator {}

impl FeePerGasSimpleEstimator {
    // TODO: add minimal max fee and priority fee
    /// depth to look for fee history to estimate priority fees
    const FEE_PRIORITY_DEPTH: u64 = 5u64;

    /// percentiles to pass to eth_feeHistory
    const HISTORY_PERCENTILES: [f64; FEE_PRIORITY_LEVEL_N] = [25.0, 50.0, 75.0];

    /// percentile to predict next base fee over historical rewards
    const BASE_FEE_PERCENTILE: f64 = 75.0;

    /// percentiles to calc max priority fee over historical rewards
    const PRIORITY_FEE_PERCENTILES: [f64; FEE_PRIORITY_LEVEL_N] = [50.0, 50.0, 50.0];

    /// adjustment for max fee per gas picked up by sampling
    const ADJUST_BASE_FEE: [f64; FEE_PRIORITY_LEVEL_N] = [1.1, 1.175, 1.25]; // 1.25 assures max_fee_per_gas will be over next block base_fee

    /// adjustment for max priority fee picked up by sampling
    const ADJUST_PRIORITY_FEE: [f64; FEE_PRIORITY_LEVEL_N] = [1.0, 1.0, 1.0];

    /// block depth for eth_feeHistory
    pub fn history_depth() -> u64 {
        Self::FEE_PRIORITY_DEPTH
    }

    /// percentiles for priority rewards obtained with eth_feeHistory
    pub fn history_percentiles() -> &'static [f64] {
        &Self::HISTORY_PERCENTILES
    }

    /// percentile for vector
    fn percentile_of(v: &[U256], percent: f64) -> U256 {
        let mut v_mut = v.to_owned();
        v_mut.sort();

        // validate bounds:
        let percent = if percent > 100.0 { 100.0 } else { percent };
        let percent = if percent < 0.0 { 0.0 } else { percent };

        let value_pos = ((v_mut.len() - 1) as f64 * percent / 100.0).round() as usize;
        v_mut[value_pos]
    }

    /// Estimate simplified gas priority fees based on fee history
    pub async fn estimate_fee_by_history(coin: &EthCoin) -> Web3RpcResult<FeePerGasEstimated> {
        let res: Result<FeeHistoryResult, web3::Error> = coin
            .eth_fee_history(
                U256::from(Self::history_depth()),
                BlockNumber::Latest,
                Self::history_percentiles(),
            )
            .await;

        match res {
            Ok(fee_history) => Ok(Self::calculate_with_history(coin, &fee_history)?),
            Err(_) => MmError::err(Web3RpcError::Internal("Eth requests failed".into())),
        }
    }

    fn predict_base_fee(base_fees: &[U256]) -> U256 {
        Self::percentile_of(base_fees, Self::BASE_FEE_PERCENTILE)
    }

    fn priority_fee_for_level(
        coin: &EthCoin,
        level: PriorityLevelId,
        base_fee: BigDecimal,
        fee_history: &FeeHistoryResult,
    ) -> Web3RpcResult<FeePerGasLevel> {
        let level_index = level as usize;
        let level_rewards = fee_history
            .priority_rewards
            .as_ref()
            .or_mm_err(|| Web3RpcError::Internal("expected reward in eth_feeHistory".into()))?
            .iter()
            .map(|rewards| rewards.get(level_index).copied().unwrap_or_else(|| U256::from(0)))
            .collect::<Vec<_>>();

        // Calculate the max priority fee per gas based on the rewards percentile.
        let max_priority_fee_per_gas = Self::percentile_of(&level_rewards, Self::PRIORITY_FEE_PERCENTILES[level_index]);
        let max_priority_fee_per_gas_gwei =
            wei_to_gwei_decimal(max_priority_fee_per_gas).unwrap_or_else(|_| BigDecimal::from(0));

        let base_fee_mult = if let Some(ref gas_price_adjust) = coin.gas_price_adjust {
            gas_price_adjust.base_fee_mult
        } else {
            Self::ADJUST_BASE_FEE
        }
        .get(level_index)
        .cloned()
        .unwrap_or(0.0);

        let priority_fee_mult = if let Some(ref gas_price_adjust) = coin.gas_price_adjust {
            gas_price_adjust.priority_fee_mult
        } else {
            Self::ADJUST_PRIORITY_FEE
        }
        .get(level_index)
        .cloned()
        .unwrap_or(0.0);

        // Calculate the max fee per gas by adjusting the base fee and adding the priority fee.
        let base_fee_mult = BigDecimal::from_f64(base_fee_mult).unwrap_or_else(|| BigDecimal::from(0));
        let priority_fee_mult = BigDecimal::from_f64(priority_fee_mult).unwrap_or_else(|| BigDecimal::from(0));

        // TODO: consider use checked ops
        let max_fee_per_gas_dec = base_fee * base_fee_mult + max_priority_fee_per_gas_gwei * priority_fee_mult;

        Ok(FeePerGasLevel {
            max_priority_fee_per_gas,
            max_fee_per_gas: wei_from_gwei_decimal(&max_fee_per_gas_dec).map_mm_err()?,
            // TODO: Consider adding default wait times if applicable (and mark them as uncertain).
            min_wait_time: None,
            max_wait_time: None,
        })
    }

    /// estimate priority fees by fee history
    fn calculate_with_history(coin: &EthCoin, fee_history: &FeeHistoryResult) -> Web3RpcResult<FeePerGasEstimated> {
        // For estimation of max fee and max priority fee we use latest block base_fee but adjusted.
        // Apparently for this simple fee estimator for assured high priority we should assume
        // that the real base_fee may go up by 1,25 (i.e. if the block is full). This is covered by high priority ADJUST_MAX_FEE multiplier
        let latest_base_fee = fee_history
            .base_fee_per_gas
            .first()
            .cloned()
            .unwrap_or_else(|| U256::from(0));
        let latest_base_fee_dec = wei_to_gwei_decimal(latest_base_fee).unwrap_or_else(|_| BigDecimal::from(0));

        // The predicted base fee is not used for calculating eip1559 values here and is provided for other purposes
        // (f.e if the caller would like to do own estimates of max fee and max priority fee)
        let predicted_base_fee = Self::predict_base_fee(&fee_history.base_fee_per_gas);
        Ok(FeePerGasEstimated {
            base_fee: predicted_base_fee,
            low: Self::priority_fee_for_level(coin, PriorityLevelId::Low, latest_base_fee_dec.clone(), fee_history)?,
            medium: Self::priority_fee_for_level(
                coin,
                PriorityLevelId::Medium,
                latest_base_fee_dec.clone(),
                fee_history,
            )?,
            high: Self::priority_fee_for_level(coin, PriorityLevelId::High, latest_base_fee_dec, fee_history)?,
            source: EstimationSource::Simple,
            base_fee_trend: String::default(),
            priority_fee_trend: String::default(),
        })
    }
}
