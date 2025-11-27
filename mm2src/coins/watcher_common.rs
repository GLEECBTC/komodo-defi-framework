use crate::ValidatePaymentError;
use mm2_err_handle::prelude::MmError;

pub const REWARD_GAS_AMOUNT: u64 = 70000;
const REWARD_MARGIN: f64 = 0.1;

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
        let max_acceptable_reward = get_reward_upper_boundary(expected_reward);
        if actual_reward < min_acceptable_reward || actual_reward > max_acceptable_reward {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Provided watcher reward {actual_reward} is not within the expected interval {min_acceptable_reward} - {max_acceptable_reward}"
            )));
        }
    }
    Ok(())
}

fn get_reward_lower_boundary(reward: u64) -> u64 {
    (reward as f64 * (1. - REWARD_MARGIN)) as u64
}

fn get_reward_upper_boundary(reward: u64) -> u64 {
    (reward as f64 * (1. + REWARD_MARGIN)) as u64
}
