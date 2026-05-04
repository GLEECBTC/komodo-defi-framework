use crate::eth::eth_withdraw::EthWithdraw;
use crate::eth::tron::fee::{TronGaslessFeeDetails, TronGaslessFeeMethod, TRON_GASFREE_PROVIDER_NAME};
use crate::eth::tron::gasfree::relay_payload::SignedWithdrawRelayPayload;
use crate::eth::tron::gasfree::{
    sign_permit_transfer, DisabledReason, GasfreeAccountService, GasfreeAvailability, GasfreeTransferPreflight,
    GasfreeTransferRequest, GaslessWithdrawError, PermitTransferData, ResolvedTronGaslessProvider, TronGasfreeError,
    TronGasfreeRelayPayload, TronOnChainBalanceFetcher,
};
use crate::eth::tron::{TronAddress, TronApiClient};
use crate::eth::{u256_from_big_decimal, u256_to_big_decimal, ChainTaggedAddress, EthCoinType, EthPrivKeyPolicy};
use crate::hd_wallet::DisplayAddress;
use crate::{
    BigDecimal, EthCoin, TransactionData, TransactionDetails, WithdrawError, WithdrawFeeMethod, WithdrawRequest,
    WithdrawResult,
};
use common::{now_sec, utc_now_rfc3339_secs};
use ethereum_types::U256;
use mm2_err_handle::map_mm_error::{MapMmError, MmResultExt};
use mm2_err_handle::prelude::{MapToMmResult, MmError, OrMmError};
use std::convert::TryFrom;

const DEFAULT_GASLESS_DEADLINE_SECONDS: u64 = 300;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TronGaslessMode {
    Native,
    Gasless,
    Auto,
}

struct QuotedGaslessWithdraw {
    preflight: GasfreeTransferPreflight,
    gasfree_provider: ResolvedTronGaslessProvider,
    permit: PermitTransferData,
    total_token_fee_token_base_units: U256,
    signed_max_fee_token_base_units: U256,
}

#[derive(Clone, Copy, Debug)]
struct ResolvedTronGaslessWithdrawPolicy {
    fallback_to_standard_fee_route: bool,
    request_max_fee_token_base_units: Option<U256>,
    config_max_fee_token_base_units: Option<U256>,
    deadline_seconds: u64,
}

impl ResolvedTronGaslessWithdrawPolicy {
    fn effective_authorization_max_fee_token_base_units(self, quoted_total_fee_token_base_units: U256) -> U256 {
        match (
            self.request_max_fee_token_base_units,
            self.config_max_fee_token_base_units,
        ) {
            (Some(request_cap), Some(token_cap)) => request_cap.min(token_cap),
            (Some(request_cap), None) => request_cap,
            (None, Some(token_cap)) => token_cap,
            (None, None) => quoted_total_fee_token_base_units,
        }
    }
}

pub(crate) async fn maybe_build_tron_gasless_withdraw<W: EthWithdraw>(
    withdraw: &W,
    tron: &TronApiClient,
    from_tagged: &ChainTaggedAddress,
    to_tagged: &ChainTaggedAddress,
    contract_tron: TronAddress,
    mode: TronGaslessMode,
) -> Result<Option<TransactionDetails>, MmError<WithdrawError>> {
    match mode {
        TronGaslessMode::Native => return Ok(None),
        TronGaslessMode::Auto if withdraw.request().max => return Ok(None),
        TronGaslessMode::Gasless | TronGaslessMode::Auto => {},
    }

    let coin = withdraw.coin();
    let Some(policy) = resolve_gasless_withdraw_policy(coin, withdraw.request(), mode)? else {
        return Ok(None);
    };

    let gasfree_provider = resolve_gasfree_provider_for_withdraw_coin(coin).await?;
    let Some(gasfree_provider) = gasfree_provider else {
        return match mode {
            TronGaslessMode::Gasless if policy.fallback_to_standard_fee_route => Ok(None),
            TronGaslessMode::Gasless => MmError::err(WithdrawError::Gasless(GaslessWithdrawError::Unavailable)),
            TronGaslessMode::Auto | TronGaslessMode::Native => Ok(None),
        };
    };

    let gasless_result = {
        let address_lock = coin.get_address_lock(from_tagged.inner()).await;
        let _nonce_lock = address_lock.lock().await;
        match quote_tron_gasless_withdraw(
            withdraw,
            gasfree_provider,
            policy,
            tron,
            from_tagged,
            to_tagged,
            contract_tron,
        )
        .await
        {
            Ok(quoted) => match mode {
                TronGaslessMode::Gasless | TronGaslessMode::Auto => {
                    finalize_tron_gasless_withdraw(withdraw, from_tagged, to_tagged, quoted)
                        .await
                        .map(Some)
                },
                TronGaslessMode::Native => Ok(None),
            },
            Err(err) => Err(err),
        }
    };

    match gasless_result {
        Ok(outcome) => Ok(outcome),
        Err(err)
            if mode == TronGaslessMode::Gasless
                && policy.fallback_to_standard_fee_route
                && is_deterministic_gasless_unavailable(&err) =>
        {
            Ok(None)
        },
        Err(err) => Err(err),
    }
}

impl TryFrom<&WithdrawRequest> for TronGaslessMode {
    type Error = MmError<WithdrawError>;

    fn try_from(req: &WithdrawRequest) -> Result<Self, Self::Error> {
        match req.fee_method {
            None | Some(WithdrawFeeMethod::Native) => {
                if req.gasless.is_some() {
                    return MmError::err(WithdrawError::InvalidFee {
                        reason: "Gasless options can only be used with fee_method 'gasless' or 'auto'".to_string(),
                        details: None,
                    });
                }
                Ok(TronGaslessMode::Native)
            },
            Some(WithdrawFeeMethod::Gasless) => Ok(TronGaslessMode::Gasless),
            Some(WithdrawFeeMethod::Auto) => Ok(TronGaslessMode::Auto),
        }
    }
}

async fn quote_tron_gasless_withdraw<W: EthWithdraw>(
    withdraw: &W,
    gasfree_provider: ResolvedTronGaslessProvider,
    policy: ResolvedTronGaslessWithdrawPolicy,
    tron: &TronApiClient,
    from_tagged: &ChainTaggedAddress,
    to_tagged: &ChainTaggedAddress,
    contract_tron: TronAddress,
) -> Result<QuotedGaslessWithdraw, MmError<WithdrawError>> {
    let coin = withdraw.coin();
    let req = withdraw.request();

    if req.max {
        return MmError::err(WithdrawError::UnsupportedError(
            "Gasless TRC20 max withdraw is not supported".to_string(),
        ));
    }

    let amount = u256_from_big_decimal(&req.amount, coin.decimals).map_mm_err()?;
    ensure_nonzero_tron_gasless_amount(amount)?;
    let from_tron = TronAddress::from(from_tagged.inner());
    let to_tron = TronAddress::from(to_tagged.inner());

    withdraw.on_fetching_gasless_quote()?;
    let service = GasfreeAccountService::new(&gasfree_provider, from_tron, TronOnChainBalanceFetcher::from(tron));
    let preflight = service
        .preflight_transfer(GasfreeTransferRequest {
            token_address: contract_tron,
            transfer_value: amount,
            expected_token_decimals: coin.decimals,
        })
        .await
        .mm_err(WithdrawError::from)?;

    ensure_gasfree_preflight_available(coin, amount, &preflight)?;
    let total_token_fee_token_base_units =
        checked_add_u256(preflight.transfer_fee, preflight.activation_fee, "gasless total fee")?;
    let effective_cap = policy.effective_authorization_max_fee_token_base_units(total_token_fee_token_base_units);
    if total_token_fee_token_base_units > effective_cap {
        return MmError::err(WithdrawError::Gasless(GaslessWithdrawError::MaxFeeExceeded));
    }

    let deadline = now_sec()
        .checked_add(policy.deadline_seconds)
        .or_mm_err(|| WithdrawError::InvalidFee {
            reason: "Gasless deadline overflow".to_string(),
            details: None,
        })?;

    let nonce = preflight.nonce;

    Ok(QuotedGaslessWithdraw {
        gasfree_provider,
        permit: PermitTransferData {
            token: contract_tron,
            user: from_tron,
            receiver: to_tron,
            value: amount,
            max_fee: effective_cap,
            deadline: U256::from(deadline),
            nonce,
        },
        preflight,
        total_token_fee_token_base_units,
        signed_max_fee_token_base_units: effective_cap,
    })
}

async fn finalize_tron_gasless_withdraw<W: EthWithdraw>(
    withdraw: &W,
    from_tagged: &ChainTaggedAddress,
    to_tagged: &ChainTaggedAddress,
    quoted: QuotedGaslessWithdraw,
) -> WithdrawResult {
    let coin = withdraw.coin();
    let req = withdraw.request();
    let derivation_path = match coin.priv_key_policy {
        EthPrivKeyPolicy::HDWallet { .. } => Some(withdraw.get_withdraw_derivation_path(req).await?),
        EthPrivKeyPolicy::Iguana(_) => None,
        EthPrivKeyPolicy::Trezor | EthPrivKeyPolicy::WalletConnect { .. } => None,
        #[cfg(target_arch = "wasm32")]
        EthPrivKeyPolicy::Metamask(_) => None,
    };

    withdraw.on_signing_gasless_authorization()?;
    let signed_authorization = sign_permit_transfer(
        &quoted.gasfree_provider,
        &quoted.permit,
        &coin.priv_key_policy,
        derivation_path.as_ref(),
    )
    .mm_err(WithdrawError::from)?;

    let payload = TronGasfreeRelayPayload::from(SignedWithdrawRelayPayload {
        provider: &quoted.gasfree_provider,
        coin: coin.ticker.clone(),
        from: req.from.clone(),
        from_address: from_tagged.display_address(),
        gasfree_address: quoted.preflight.gasfree_address,
        signed_authorization,
        created_at: utc_now_rfc3339_secs(),
    });
    let tx = TransactionData::new_unsigned(
        serde_json::to_value(payload).map_to_mm(|e| WithdrawError::InternalError(e.to_string()))?,
    );
    let fee_details = build_tron_gasless_fee_details(coin, &quoted)?;

    withdraw.on_finishing()?;
    build_gasless_transaction_details(
        coin,
        from_tagged,
        to_tagged,
        tx,
        quoted.permit.value,
        quoted.total_token_fee_token_base_units,
        fee_details,
    )
}

fn is_deterministic_gasless_unavailable(err: &MmError<WithdrawError>) -> bool {
    matches!(
        err.get_inner(),
        WithdrawError::Gasless(GaslessWithdrawError::Unavailable) | WithdrawError::NotSufficientBalance { .. }
    )
}

pub(crate) fn is_standard_tron_withdraw_unavailable(err: &MmError<WithdrawError>) -> bool {
    matches!(
        err.get_inner(),
        WithdrawError::NotSufficientBalance { .. }
            | WithdrawError::NotSufficientPlatformBalanceForFee { .. }
            | WithdrawError::ZeroBalanceToWithdrawMax
            | WithdrawError::AmountTooLow { .. }
    )
}

#[allow(clippy::result_large_err)]
pub(crate) fn validate_non_tron_gasless_request(req: &WithdrawRequest) -> Result<(), MmError<WithdrawError>> {
    match req.fee_method {
        Some(WithdrawFeeMethod::Gasless) | Some(WithdrawFeeMethod::Auto) => {
            MmError::err(WithdrawError::Gasless(GaslessWithdrawError::Unavailable))
        },
        Some(WithdrawFeeMethod::Native) | None if req.gasless.is_some() => MmError::err(WithdrawError::InvalidFee {
            reason: "Gasless options can only be used with fee_method 'gasless' or 'auto'".to_string(),
            details: None,
        }),
        Some(WithdrawFeeMethod::Native) | None => Ok(()),
    }
}

#[allow(clippy::result_large_err)]
fn resolve_gasless_withdraw_policy(
    coin: &EthCoin,
    req: &WithdrawRequest,
    mode: TronGaslessMode,
) -> Result<Option<ResolvedTronGaslessWithdrawPolicy>, MmError<WithdrawError>> {
    let fallback_to_standard_fee_route = req
        .gasless
        .as_ref()
        .map(|options| options.fallback_to_native)
        .unwrap_or(false);

    let Some(token_config) = coin.0.tron_gasless_token_config.as_ref() else {
        return match mode {
            TronGaslessMode::Gasless if fallback_to_standard_fee_route => Ok(None),
            TronGaslessMode::Gasless => MmError::err(WithdrawError::Gasless(GaslessWithdrawError::Unavailable)),
            TronGaslessMode::Auto | TronGaslessMode::Native => Ok(None),
        };
    };

    let deadline_seconds = req
        .gasless
        .as_ref()
        .and_then(|options| options.deadline_seconds)
        .unwrap_or(DEFAULT_GASLESS_DEADLINE_SECONDS);
    if deadline_seconds == 0 {
        return MmError::err(WithdrawError::InvalidFee {
            reason: "Gasless deadline_seconds must be greater than zero".to_string(),
            details: None,
        });
    }

    let request_max_fee_token_base_units = req
        .gasless
        .as_ref()
        .and_then(|options| options.max_fee.as_ref())
        .map(|cap| request_gasless_max_fee_to_token_base_units(cap, coin.decimals))
        .transpose()?;

    Ok(Some(ResolvedTronGaslessWithdrawPolicy {
        fallback_to_standard_fee_route,
        request_max_fee_token_base_units,
        config_max_fee_token_base_units: token_config.transfer_max_fee_token_base_units,
        deadline_seconds,
    }))
}

async fn resolve_gasfree_provider_for_withdraw_coin(
    coin: &EthCoin,
) -> Result<Option<ResolvedTronGaslessProvider>, MmError<WithdrawError>> {
    if let Some(provider) = coin.0.tron_gasless_provider.clone() {
        return Ok(Some(provider));
    }

    match coin.coin_type {
        EthCoinType::Erc20 { .. } => Ok(coin.platform_coin().await.map_mm_err()?.0.tron_gasless_provider.clone()),
        EthCoinType::Eth | EthCoinType::Nft { .. } => Ok(None),
    }
}

#[allow(clippy::result_large_err)]
fn build_gasless_transaction_details(
    coin: &EthCoin,
    from_tagged: &ChainTaggedAddress,
    to_tagged: &ChainTaggedAddress,
    tx: TransactionData,
    amount: U256,
    token_fee: U256,
    fee_details: TronGaslessFeeDetails,
) -> WithdrawResult {
    let amount_decimal = u256_to_big_decimal(amount, coin.decimals).map_mm_err()?;
    let token_fee_decimal = u256_to_big_decimal(token_fee, coin.decimals).map_mm_err()?;
    let spent_by_me = &amount_decimal + &token_fee_decimal;
    let received_by_me = if to_tagged.inner() == from_tagged.inner() {
        amount_decimal.clone()
    } else {
        0.into()
    };

    Ok(TransactionDetails {
        to: vec![to_tagged.display_address()],
        from: vec![from_tagged.display_address()],
        total_amount: amount_decimal,
        my_balance_change: &received_by_me - &spent_by_me,
        spent_by_me,
        received_by_me,
        tx,
        block_height: 0,
        fee_details: Some(fee_details.into()),
        coin: coin.ticker.clone(),
        internal_id: vec![].into(),
        timestamp: now_sec(),
        kmd_rewards: None,
        transaction_type: Default::default(),
        memo: None,
    })
}

#[allow(clippy::result_large_err)]
fn ensure_nonzero_tron_gasless_amount(amount: U256) -> Result<(), MmError<WithdrawError>> {
    if amount.is_zero() {
        return MmError::err(WithdrawError::InvalidFee {
            reason: "Gasless withdraw amount must be greater than zero".to_string(),
            details: None,
        });
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn checked_add_u256(lhs: U256, rhs: U256, field_name: &str) -> Result<U256, MmError<WithdrawError>> {
    lhs.checked_add(rhs)
        .or_mm_err(|| WithdrawError::InternalError(format!("Overflow while computing {field_name}")))
}

#[allow(clippy::result_large_err)]
fn request_gasless_max_fee_to_token_base_units(
    cap: &BigDecimal,
    token_decimals: u8,
) -> Result<U256, MmError<WithdrawError>> {
    u256_from_big_decimal(cap, token_decimals).mm_err(|_| WithdrawError::InvalidFee {
        reason: format!("Invalid gasless max_fee value '{cap}'"),
        details: None,
    })
}

#[allow(clippy::result_large_err)]
fn ensure_gasfree_preflight_available(
    coin: &EthCoin,
    amount: U256,
    preflight: &GasfreeTransferPreflight,
) -> Result<(), MmError<WithdrawError>> {
    match &preflight.availability {
        GasfreeAvailability::Available => Ok(()),
        GasfreeAvailability::PendingTransfer => {
            MmError::err(WithdrawError::Gasless(GaslessWithdrawError::PendingTransfer))
        },
        GasfreeAvailability::Disabled { reason } => match reason {
            DisabledReason::TokenUnsupported { .. } => {
                MmError::err(WithdrawError::Gasless(GaslessWithdrawError::Unavailable))
            },
            DisabledReason::AddressMismatch { .. } | DisabledReason::TokenDecimalMismatch { .. } => {
                MmError::err(WithdrawError::Gasless(GaslessWithdrawError::InvalidProviderResponse))
            },
            DisabledReason::InsufficientSpendableBalance => {
                let total_fee =
                    checked_add_u256(preflight.transfer_fee, preflight.activation_fee, "gasless total fee")?;
                let required = checked_add_u256(amount, total_fee, "gasless spendable required")?;
                let available = preflight.on_chain_balance.saturating_sub(preflight.frozen_balance);
                MmError::err(WithdrawError::NotSufficientBalance {
                    coin: coin.ticker.clone(),
                    available: u256_to_big_decimal(available, coin.decimals).map_mm_err()?,
                    required: u256_to_big_decimal(required, coin.decimals).map_mm_err()?,
                })
            },
            DisabledReason::InactiveAccountInsufficientBalance => {
                let total_fee =
                    checked_add_u256(preflight.transfer_fee, preflight.activation_fee, "gasless total fee")?;
                let required_spendable = checked_add_u256(amount, total_fee, "gasless inactive spendable required")?;
                let required_total = checked_add_u256(
                    required_spendable,
                    preflight.frozen_balance,
                    "gasless inactive total required",
                )?;
                MmError::err(WithdrawError::NotSufficientBalance {
                    coin: coin.ticker.clone(),
                    available: u256_to_big_decimal(preflight.on_chain_balance, coin.decimals).map_mm_err()?,
                    required: u256_to_big_decimal(required_total, coin.decimals).map_mm_err()?,
                })
            },
        },
    }
}

impl From<TronGasfreeError> for WithdrawError {
    fn from(err: TronGasfreeError) -> Self {
        match err {
            TronGasfreeError::InvalidResponse(_) => {
                WithdrawError::Gasless(GaslessWithdrawError::InvalidProviderResponse)
            },
            TronGasfreeError::ProviderBadRequest(_) => WithdrawError::Gasless(GaslessWithdrawError::ProviderRejected),
            TronGasfreeError::Unauthorized(message)
            | TronGasfreeError::Forbidden(message)
            | TronGasfreeError::RateLimited(message)
            | TronGasfreeError::Upstream(message)
            | TronGasfreeError::Transport(message)
            | TronGasfreeError::Timeout(message) => WithdrawError::Transport(format!("GasFree {message}")),
            TronGasfreeError::InvalidRequest(message)
            | TronGasfreeError::Internal(message)
            | TronGasfreeError::NotImplemented(message) => WithdrawError::InternalError(format!("GasFree {message}")),
        }
    }
}

#[allow(clippy::result_large_err)]
fn build_tron_gasless_fee_details(
    coin: &EthCoin,
    quoted: &QuotedGaslessWithdraw,
) -> Result<TronGaslessFeeDetails, MmError<WithdrawError>> {
    let transfer_fee = u256_to_big_decimal(quoted.preflight.transfer_fee, coin.decimals).map_mm_err()?;
    let activation_fee = u256_to_big_decimal(quoted.preflight.activation_fee, coin.decimals).map_mm_err()?;
    let total_token_fee = u256_to_big_decimal(quoted.total_token_fee_token_base_units, coin.decimals).map_mm_err()?;

    let signed_max_fee = u256_to_big_decimal(quoted.signed_max_fee_token_base_units, coin.decimals).map_mm_err()?;

    Ok(TronGaslessFeeDetails {
        coin: coin.ticker.clone(),
        fee_method: TronGaslessFeeMethod::Gasless,
        provider_name: TRON_GASFREE_PROVIDER_NAME.to_string(),
        gasfree_address: quoted.preflight.gasfree_address.to_base58(),
        transfer_fee,
        activation_fee,
        total_token_fee,
        signed_max_fee: Some(signed_max_fee),
        trace_id: None,
    })
}
