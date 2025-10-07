use common::PagingOptions;
use cosmrs::staking::{Commission, Description, Validator};
use mm2_err_handle::prelude::{MmError, MmResultExt};
use mm2_number::BigDecimal;

use crate::{
    hd_wallet::HDAddressSelector, tendermint::TendermintCoinRpcError, MmCoinEnum, StakingInfoError, WithdrawFee,
};

/// Represents current status of the validator.
#[derive(Debug, Default, Deserialize)]
pub(crate) enum ValidatorStatus {
    All,
    /// Validator is in the active set and participates in consensus.
    #[default]
    Bonded,
    /// Validator is not in the active set and does not participate in consensus.
    /// Accordingly, they do not receive rewards and cannot be slashed.
    /// It is still possible to delegate tokens to a validator in this state.
    Unbonded,
}

impl std::fmt::Display for ValidatorStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // An empty string doesn't filter any validators and we get an unfiltered result.
            ValidatorStatus::All => write!(f, ""),
            ValidatorStatus::Bonded => write!(f, "BOND_STATUS_BONDED"),
            ValidatorStatus::Unbonded => write!(f, "BOND_STATUS_UNBONDED"),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ValidatorsQuery {
    #[serde(flatten)]
    paging: PagingOptions,
    #[serde(default)]
    filter_by_status: ValidatorStatus,
}

#[derive(Clone, Serialize)]
pub struct ValidatorsQueryResponse {
    validators: Vec<serde_json::Value>,
}

impl From<TendermintCoinRpcError> for StakingInfoError {
    fn from(e: TendermintCoinRpcError) -> Self {
        match e {
            TendermintCoinRpcError::InvalidResponse(e)
            | TendermintCoinRpcError::PerformError(e)
            | TendermintCoinRpcError::RpcClientError(e)
            | TendermintCoinRpcError::NotFound(e) => StakingInfoError::Transport(e),
            TendermintCoinRpcError::Prost(e) | TendermintCoinRpcError::InternalError(e) => StakingInfoError::Internal(e),
            TendermintCoinRpcError::UnexpectedAccountType { .. } => StakingInfoError::Internal(
                "RPC client got an unexpected error 'TendermintCoinRpcError::UnexpectedAccountType', this isn't normal."
                    .into(),
            ),
        }
    }
}

pub async fn validators_rpc(
    coin: MmCoinEnum,
    req: ValidatorsQuery,
) -> Result<ValidatorsQueryResponse, MmError<StakingInfoError>> {
    fn maybe_jsonize_description(description: Option<Description>) -> Option<serde_json::Value> {
        description.map(|d| {
            json!({
                "moniker": d.moniker,
                "identity": d.identity,
                "website": d.website,
                "security_contact": d.security_contact,
                "details": d.details,
            })
        })
    }

    fn maybe_jsonize_commission(commission: Option<Commission>) -> Option<serde_json::Value> {
        commission.map(|c| {
            let rates = c.commission_rates.map(|cr| {
                json!({
                    "rate": cr.rate,
                    "max_rate": cr.max_rate,
                    "max_change_rate": cr.max_change_rate
                })
            });

            json!({
                "commission_rates": rates,
                "update_time": c.update_time
            })
        })
    }

    fn jsonize_validator(v: Validator) -> serde_json::Value {
        json!({
            "operator_address": v.operator_address,
            "consensus_pubkey": v.consensus_pubkey,
            "jailed": v.jailed,
            "status": v.status,
            "tokens": v.tokens,
            "delegator_shares": v.delegator_shares,
            "description": maybe_jsonize_description(v.description),
            "unbonding_height": v.unbonding_height,
            "unbonding_time": v.unbonding_time,
            "commission": maybe_jsonize_commission(v.commission),
            "min_self_delegation": v.min_self_delegation,
        })
    }

    let validators = match coin {
        MmCoinEnum::Tendermint(coin) => coin
            .validators_list(req.filter_by_status, req.paging)
            .await
            .map_mm_err()?,
        MmCoinEnum::TendermintToken(token) => token
            .platform_coin
            .validators_list(req.filter_by_status, req.paging)
            .await
            .map_mm_err()?,
        other => {
            return MmError::err(StakingInfoError::InvalidPayload {
                reason: format!("{} is not a Cosmos coin", other.ticker()),
            })
        },
    };

    Ok(ValidatorsQueryResponse {
        validators: validators.into_iter().map(jsonize_validator).collect(),
    })
}

#[derive(Clone, Debug, Deserialize)]
pub struct DelegationPayload {
    pub validator_address: String,
    pub fee: Option<WithdrawFee>,
    pub withdraw_from: Option<HDAddressSelector>,
    #[serde(default)]
    pub memo: String,
    #[serde(default)]
    pub amount: BigDecimal,
    #[serde(default)]
    pub max: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ClaimRewardsPayload {
    pub validator_address: String,
    pub fee: Option<WithdrawFee>,
    #[serde(default)]
    pub memo: String,
    /// If transaction fee exceeds the reward amount users will be
    /// prevented from claiming their rewards as it will not be profitable.
    /// Setting `force` to `true` disables this logic.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Deserialize)]
pub struct SimpleListQuery {
    #[serde(flatten)]
    pub(crate) paging: PagingOptions,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct DelegationsQueryResponse {
    pub(crate) delegations: Vec<Delegation>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Delegation {
    pub(crate) validator_address: String,
    pub(crate) delegated_amount: BigDecimal,
    pub(crate) reward_amount: BigDecimal,
}

#[derive(Serialize)]
pub struct UndelegationsQueryResponse {
    pub(crate) ongoing_undelegations: Vec<Undelegation>,
}

#[derive(Serialize)]
pub(crate) struct Undelegation {
    pub(crate) validator_address: String,
    pub(crate) entries: Vec<UndelegationEntry>,
}

#[derive(Serialize)]
pub(crate) struct UndelegationEntry {
    pub(crate) creation_height: i64,
    pub(crate) completion_datetime: String,
    pub(crate) balance: BigDecimal,
}
