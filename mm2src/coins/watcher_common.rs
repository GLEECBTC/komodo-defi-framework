use crate::ValidatePaymentError;
use mm2_err_handle::prelude::MmError;

/// Gas amount used to calculate watcher reward.
///
/// This value (150K) is set to cover actual watcher gas costs:
///
/// 1. **Reward functions use more gas than non-reward functions:**
///    - `receiverSpendReward` / `senderRefundReward` have additional hashing (more parameters)
///    - Multiple external transfers (2-4 vs 1 in non-reward functions)
///    - ERC20 is ~2× more expensive (double token transfers + ETH transfers)
///
/// # Watcher Economics
///
/// Taker always benefits from watcher actions:
/// - `receiverSpendReward`: Watcher spends maker payment → taker gets coins
/// - `senderRefundReward`: Watcher refunds taker payment → taker gets refund
///
/// Therefore taker should pay the watcher reward (which the current design does,
/// except for ETH/ETH maker payments which use a shared contract pool).
///
/// # Future Improvements
///
/// - Use operation-specific gas constants (measure actual `gasUsed` for each reward function)
/// - Dynamic reward adjustment based on network conditions
pub const REWARD_GAS_AMOUNT: u64 = 150000;

/// Overpay factor for watcher reward calculation (1.5 = 50% overpay).
///
/// When calculating watcher reward at payment time, multiply the gas cost by this factor
/// to account for:
///
/// 1. **Gas price volatility:** Reward is set at payment time but validator checks it later.
///    Gas price can increase significantly between these times.
///
/// 2. **Profit margin:** Provides buffer for watcher profit (~10%+).
///
/// The 50% overpay ensures the reward remains valid even if gas price increases by 30-40%
/// between payment creation and validation.
pub const REWARD_OVERPAY_FACTOR: f64 = 1.5;

/// Margin for reward validation (10%).
///
/// When validating rewards in non-exact mode, actual reward must be at least
/// `expected_reward * (1 - REWARD_MARGIN)` to pass validation.
const REWARD_MARGIN: f64 = 0.1;

/// Validates that the actual watcher reward in a payment transaction is acceptable.
///
/// # Call sites
/// - `validate_payment` (eth.rs) - counterparty validates payment before proceeding with swap
/// - `watcher_validate_taker_payment` (eth.rs) - watcher validates taker payment before helping
///
/// # Arguments
/// - `expected_reward`: Reward calculated by validator at validation time (based on current gas price)
/// - `actual_reward`: Reward encoded in the payment transaction (set by payer at payment time)
/// - `is_exact`: If true, requires exact match; if false, only enforces lower bound
///
/// # Validation logic
///
/// **Exact mode (`is_exact == true`):**
/// Requires `actual_reward == expected_reward`. Used when reward amount was pre-negotiated.
///
/// **Non-exact mode (`is_exact == false`):**
/// Only enforces lower bound: `actual_reward >= expected_reward * (1 - REWARD_MARGIN)`.
///
/// Upper bound is NOT enforced because:
/// 1. The payer of the reward chooses the amount when building the tx.
///    - Maker pays for maker payment reward, taker pays for taker payment reward.
///    - Their node computes `WatcherReward.amount` and encodes it in the contract call.
/// 2. No one can increase `_rewardAmount` after the fact - it's locked into `paymentHash`.
///    The contract's `receiverSpendReward`/`senderRefundReward` require exact hash match.
/// 3. Gas price volatility causes the reward (set at payment time) to exceed the
///    expected reward (calculated by validator at validation time).
/// 4. A higher reward benefits watchers and doesn't reduce what the counterparty receives.
///
/// Lower bound provides a sanity check that the reward is in a reasonable range,
/// though note that due to gas price volatility, even this check can fail if gas prices
/// rise significantly between payment time and validation time.
///
/// # Watcher Execution Flexibility
///
/// - Watcher can execute with less gas than budgeted (they profit more)
/// - Watcher can wait if gas is high and retry later before locktime expires
/// - Multiple watchers can compete - first to call `receiverSpendReward`/`senderRefundReward`
///   gets the reward (reward goes to `msg.sender` in the contract)
///
/// # Trade Amount Invariants
///
/// `maker_amount` and `taker_amount` from ordermatching are **net trade amounts**,
/// independent of watcher rewards. The reward is funded separately (e.g., via `msg.value`
/// for ERC20 payments) and does not reduce what the counterparty receives.
pub fn validate_watcher_reward(
    expected_reward: u64,
    actual_reward: u64,
    is_exact: bool,
) -> Result<(), MmError<ValidatePaymentError>> {
    if is_exact {
        if actual_reward != expected_reward {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Payment tx reward_amount arg {actual_reward} is invalid, expected {expected_reward}",
            )));
        }
    } else {
        let min_acceptable_reward = get_reward_lower_boundary(expected_reward);
        if actual_reward < min_acceptable_reward {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Provided watcher reward {actual_reward} is less than minimum acceptable {min_acceptable_reward}"
            )));
        }
    }
    Ok(())
}

fn get_reward_lower_boundary(reward: u64) -> u64 {
    (reward as f64 * (1. - REWARD_MARGIN)) as u64
}
