use crate::eth::tron::gasfree::address::compute_gasfree_address_for_network;
use crate::eth::tron::gasfree::api_types::{GasfreeAccountAsset, GasfreeAccountInfo};
use crate::eth::tron::gasfree::config::ResolvedTronGaslessProvider;
use crate::eth::tron::gasfree::error::TronGasfreeError;
use crate::eth::tron::{TronAddress, TronApiClient};
use async_trait::async_trait;
use ethereum_types::U256;
use mm2_err_handle::prelude::*;

/// Caller intent for a single GasFree transfer: which TRC-20 token and how much.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GasfreeTransferRequest {
    pub token_address: TronAddress,
    pub transfer_value: U256,
    pub expected_token_decimals: u8,
}

/// Snapshot of everything needed to decide and execute a GasFree transfer.
///
/// Carries raw source-of-truth fields only, derived quantities (spendable, total fee, required
/// balances) should be recomputed by callers that need them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GasfreeTransferPreflight {
    /// Locally-derived CREATE2 custody address for this user + network.
    pub gasfree_address: TronAddress,
    /// Provider-reported nonce bound into the signed authorization.
    pub nonce: U256,
    /// Whether the GasFree account has been on-chain activated.
    pub account_active: bool,
    /// TRC-20 balance at `gasfree_address`.
    pub on_chain_balance: U256,
    /// Amount currently locked by in-flight transfers. This should be reported by the GasFree provider.
    pub frozen_balance: U256,
    /// Per-transfer fee in the token, should be reported by the provider for this account + token.
    pub transfer_fee: U256,
    /// Extra fee charged when the account is inactive and will auto-activate. Zero when active.
    pub activation_fee: U256,
    /// Primary preflight decision.
    pub availability: GasfreeAvailability,
}

/// Result of the preflight decision. Distinguishes transient waits from permanent failures
/// so callers can choose to retry, fall back to native, or surface an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GasfreeAvailability {
    /// Gasless rail is ready; withdraw may sign and submit.
    Available,
    /// GasFree enforces "one pending transfer per account" — caller must wait for the
    /// current transfer to settle and retry. Transient, not an error.
    PendingTransfer,
    /// Gasless is not usable for this request. `reason` distinguishes recoverable from
    /// systemic failures.
    Disabled { reason: DisabledReason },
}

/// Why a preflight returned `Disabled`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisabledReason {
    /// Provider-reported GasFree address disagrees with our local CREATE2 derivation.
    /// Indicates systemic drift (wrong controller, wrong formula, or malicious provider)
    /// and is treated as a safety stop: withdraw must not sign against an unverifiable custody.
    AddressMismatch {
        expected: TronAddress,
        provider_reported: TronAddress,
    },
    /// Token is not enrolled in the caller's GasFree account. Gasless cannot be used here;
    /// caller should fall back to native or error.
    TokenUnsupported { token_address: TronAddress },
    /// Provider-reported token decimals disagree with the activated token decimals.
    /// Indicates provider/config drift, so callers must not sign or silently fall back.
    TokenDecimalMismatch {
        token_address: TronAddress,
        expected: u8,
        provider_reported: u8,
    },
    /// Spendable (on-chain minus frozen) balance is below the required spendable amount
    /// (`value + transfer_fee + activation_fee`).
    InsufficientSpendableBalance,
    /// For an inactive account, the on-chain balance must cover the required total
    /// (`value + transfer_fee + activation_fee + frozen`). This variant fires when it
    /// doesn't — preflight must reject before signing to avoid a guaranteed on-chain failure.
    InactiveAccountInsufficientBalance,
}

/// Abstraction over on-chain TRC-20 `balanceOf` reads.
///
/// Keeps the service decoupled from the coin and useful for unit tests to
/// drive the preflight math without standing up a live chain RPC or Coin.
/// Production withdraw code uses [`TronOnChainBalanceFetcher`] backed by [`TronApiClient`].
#[async_trait]
pub trait OnChainBalanceFetcher: Send + Sync {
    async fn trc20_balance(
        &self,
        token_address: &TronAddress,
        owner_address: &TronAddress,
    ) -> MmResult<U256, TronGasfreeError>;
}

pub struct TronOnChainBalanceFetcher<'a> {
    tron: &'a TronApiClient,
}

impl<'a> TronOnChainBalanceFetcher<'a> {
    pub fn new(tron: &'a TronApiClient) -> Self {
        TronOnChainBalanceFetcher { tron }
    }
}

impl<'a> From<&'a TronApiClient> for TronOnChainBalanceFetcher<'a> {
    fn from(tron: &'a TronApiClient) -> Self {
        TronOnChainBalanceFetcher::new(tron)
    }
}

#[async_trait]
impl OnChainBalanceFetcher for TronOnChainBalanceFetcher<'_> {
    async fn trc20_balance(
        &self,
        token_address: &TronAddress,
        owner_address: &TronAddress,
    ) -> MmResult<U256, TronGasfreeError> {
        self.tron
            .trc20_balance_of(token_address, owner_address)
            .await
            .mm_err(TronGasfreeError::from)
    }
}

/// Stateless per-request preflight layer over a [`ResolvedTronGaslessProvider`] and an
/// [`OnChainBalanceFetcher`].
///
/// Construction computes the user's CREATE2 GasFree address once; each call to
/// [`Self::preflight_transfer`] fetches fresh provider + on-chain state.
pub struct GasfreeAccountService<'a, B: OnChainBalanceFetcher> {
    provider: &'a ResolvedTronGaslessProvider,
    user_address: TronAddress,
    local_gasfree_address: TronAddress,
    balance_fetcher: B,
}

impl<'a, B> GasfreeAccountService<'a, B>
where
    B: OnChainBalanceFetcher,
{
    pub fn new(provider: &'a ResolvedTronGaslessProvider, user_address: TronAddress, balance_fetcher: B) -> Self {
        let local_gasfree_address = compute_gasfree_address_for_network(provider.network(), &user_address);
        GasfreeAccountService {
            provider,
            user_address,
            local_gasfree_address,
            balance_fetcher,
        }
    }

    pub fn local_gasfree_address(&self) -> &TronAddress {
        &self.local_gasfree_address
    }

    /// Fetch provider account state, verify the GasFree address against local CREATE2, and
    /// compute preflight availability for a single transfer.
    ///
    /// Returns `Err` only on transport/provider errors; any preflight-level rejection (bad
    /// config, unsupported token, pending transfer, insufficient balance) surfaces through
    /// [`GasfreeTransferPreflight::availability`].
    pub async fn preflight_transfer(
        &self,
        request: GasfreeTransferRequest,
    ) -> MmResult<GasfreeTransferPreflight, TronGasfreeError> {
        let account_info_fetcher = ProviderAccountInfoFetcher {
            provider: self.provider,
        };
        self.preflight_with_account_fetcher(&request, &account_info_fetcher)
            .await
    }

    async fn preflight_with_account_fetcher<F>(
        &self,
        request: &GasfreeTransferRequest,
        account_info_fetcher: &F,
    ) -> MmResult<GasfreeTransferPreflight, TronGasfreeError>
    where
        F: AccountInfoFetcher,
    {
        let account_info = account_info_fetcher.get_account_info(&self.user_address).await?;
        let snapshot = AccountSnapshot::from(&account_info);

        if account_info.gas_free_address != self.local_gasfree_address {
            return self.unavailable_preflight(
                GasfreeAvailability::Disabled {
                    reason: DisabledReason::AddressMismatch {
                        expected: self.local_gasfree_address,
                        provider_reported: account_info.gas_free_address,
                    },
                },
                snapshot,
            );
        }

        let Some(asset) = account_info
            .assets
            .iter()
            .find(|asset| asset.token_address == request.token_address)
            .cloned()
        else {
            return self.unavailable_preflight(
                GasfreeAvailability::Disabled {
                    reason: DisabledReason::TokenUnsupported {
                        token_address: request.token_address,
                    },
                },
                snapshot,
            );
        };

        if asset.decimal != request.expected_token_decimals {
            return self.unavailable_preflight(
                GasfreeAvailability::Disabled {
                    reason: DisabledReason::TokenDecimalMismatch {
                        token_address: request.token_address,
                        expected: request.expected_token_decimals,
                        provider_reported: asset.decimal,
                    },
                },
                snapshot.with_asset_details(
                    asset.frozen,
                    asset.transfer_fee,
                    if account_info.active {
                        U256::zero()
                    } else {
                        asset.activate_fee
                    },
                ),
            );
        }

        if !account_info.allow_submit {
            let transfer_fee = asset.transfer_fee;
            let activation_fee = if account_info.active {
                U256::zero()
            } else {
                asset.activate_fee
            };
            return self.unavailable_preflight(
                GasfreeAvailability::PendingTransfer,
                snapshot.with_asset_details(asset.frozen, transfer_fee, activation_fee),
            );
        }

        let on_chain_balance = self
            .balance_fetcher
            .trc20_balance(&request.token_address, &self.local_gasfree_address)
            .await?;

        self.evaluate_account_state(request, &account_info, &asset, on_chain_balance)
    }

    fn evaluate_account_state(
        &self,
        request: &GasfreeTransferRequest,
        account_info: &GasfreeAccountInfo,
        asset: &GasfreeAccountAsset,
        on_chain_balance: U256,
    ) -> MmResult<GasfreeTransferPreflight, TronGasfreeError> {
        let frozen_balance = asset.frozen;
        let spendable_balance = on_chain_balance.saturating_sub(frozen_balance);
        let activation_fee = if account_info.active {
            U256::zero()
        } else {
            asset.activate_fee
        };
        let total_token_fee = checked_add_u256(asset.transfer_fee, activation_fee, "total token fee")?;
        let required_spendable_balance =
            checked_add_u256(request.transfer_value, total_token_fee, "required spendable")?;
        let required_total_balance = checked_add_u256(required_spendable_balance, frozen_balance, "required total")?;

        let availability = if !account_info.active && on_chain_balance < required_total_balance {
            GasfreeAvailability::Disabled {
                reason: DisabledReason::InactiveAccountInsufficientBalance,
            }
        } else if spendable_balance < required_spendable_balance {
            GasfreeAvailability::Disabled {
                reason: DisabledReason::InsufficientSpendableBalance,
            }
        } else {
            GasfreeAvailability::Available
        };

        Ok(GasfreeTransferPreflight {
            gasfree_address: self.local_gasfree_address,
            nonce: account_info.nonce,
            account_active: account_info.active,
            on_chain_balance,
            frozen_balance,
            transfer_fee: asset.transfer_fee,
            activation_fee,
            availability,
        })
    }

    fn unavailable_preflight(
        &self,
        availability: GasfreeAvailability,
        snapshot: AccountSnapshot,
    ) -> MmResult<GasfreeTransferPreflight, TronGasfreeError> {
        Ok(GasfreeTransferPreflight {
            gasfree_address: self.local_gasfree_address,
            nonce: snapshot.nonce,
            account_active: snapshot.account_active,
            on_chain_balance: U256::zero(),
            frozen_balance: snapshot.frozen_balance,
            transfer_fee: snapshot.transfer_fee,
            activation_fee: snapshot.activation_fee,
            availability,
        })
    }
}

#[async_trait]
trait AccountInfoFetcher: Send + Sync {
    async fn get_account_info(&self, account: &TronAddress) -> MmResult<GasfreeAccountInfo, TronGasfreeError>;
}

struct ProviderAccountInfoFetcher<'a> {
    provider: &'a ResolvedTronGaslessProvider,
}

#[async_trait]
impl<'a> AccountInfoFetcher for ProviderAccountInfoFetcher<'a> {
    async fn get_account_info(&self, account: &TronAddress) -> MmResult<GasfreeAccountInfo, TronGasfreeError> {
        self.provider.client().get_account_info(account).await
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AccountSnapshot {
    nonce: U256,
    account_active: bool,
    frozen_balance: U256,
    transfer_fee: U256,
    activation_fee: U256,
}

impl From<&GasfreeAccountInfo> for AccountSnapshot {
    fn from(account_info: &GasfreeAccountInfo) -> Self {
        AccountSnapshot {
            nonce: account_info.nonce,
            account_active: account_info.active,
            ..AccountSnapshot::default()
        }
    }
}

impl AccountSnapshot {
    fn with_asset_details(mut self, frozen_balance: U256, transfer_fee: U256, activation_fee: U256) -> Self {
        self.frozen_balance = frozen_balance;
        self.transfer_fee = transfer_fee;
        self.activation_fee = activation_fee;
        self
    }
}

fn checked_add_u256(lhs: U256, rhs: U256, field_name: &str) -> MmResult<U256, TronGasfreeError> {
    lhs.checked_add(rhs)
        .or_mm_err(|| TronGasfreeError::Internal(format!("Overflow while computing {field_name}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::tron::gasfree::resolve_tron_gasless_provider;
    use crate::eth::tron::gasfree::test_helpers::{provider_config, DEFAULT_SERVICE_PROVIDER, TEST_BASE_URL};
    use crate::eth::tron::Network;
    use crate::eth::ChainSpec;
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct MockOnChainBalanceFetcher {
        balances: HashMap<(TronAddress, TronAddress), U256>,
        calls: Arc<Mutex<usize>>,
    }

    impl MockOnChainBalanceFetcher {
        fn with_balance(mut self, token: TronAddress, owner: TronAddress, balance: U256) -> Self {
            self.balances.insert((token, owner), balance);
            self
        }

        fn call_count(&self) -> usize {
            *self.calls.lock()
        }
    }

    #[async_trait]
    impl OnChainBalanceFetcher for MockOnChainBalanceFetcher {
        async fn trc20_balance(
            &self,
            token_address: &TronAddress,
            owner_address: &TronAddress,
        ) -> MmResult<U256, TronGasfreeError> {
            *self.calls.lock() += 1;
            self.balances
                .get(&(*token_address, *owner_address))
                .copied()
                .or_mm_err(|| {
                    TronGasfreeError::InvalidRequest(format!(
                        "missing mock balance for token {} owner {}",
                        token_address, owner_address
                    ))
                })
        }
    }

    #[derive(Clone, Default)]
    struct MockAccountInfoFetcher {
        account_info_by_account: HashMap<TronAddress, GasfreeAccountInfo>,
        calls: Arc<Mutex<usize>>,
    }

    impl MockAccountInfoFetcher {
        fn returning(account_info: GasfreeAccountInfo) -> Self {
            Self::default().with_account_info(user_address(), account_info)
        }

        fn with_account_info(mut self, account: TronAddress, account_info: GasfreeAccountInfo) -> Self {
            self.account_info_by_account.insert(account, account_info);
            self
        }

        fn call_count(&self) -> usize {
            *self.calls.lock()
        }
    }

    #[async_trait]
    impl AccountInfoFetcher for MockAccountInfoFetcher {
        async fn get_account_info(&self, account: &TronAddress) -> MmResult<GasfreeAccountInfo, TronGasfreeError> {
            *self.calls.lock() += 1;
            self.account_info_by_account.get(account).cloned().or_mm_err(|| {
                TronGasfreeError::InvalidRequest(format!("missing mock account info for account {}", account))
            })
        }
    }

    fn provider() -> ResolvedTronGaslessProvider {
        let raw = provider_config(TEST_BASE_URL, DEFAULT_SERVICE_PROVIDER);
        resolve_tron_gasless_provider(&ChainSpec::Tron { network: Network::Nile }, Some(&raw))
            .unwrap()
            .unwrap()
    }

    fn user_address() -> TronAddress {
        "TMVQGm1qAQYVdetCeGRRkTWYYrLXuHK2HC".parse().unwrap()
    }

    fn token_address() -> TronAddress {
        "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf".parse().unwrap()
    }

    fn request(value: u64) -> GasfreeTransferRequest {
        GasfreeTransferRequest {
            token_address: token_address(),
            transfer_value: U256::from(value),
            expected_token_decimals: 6,
        }
    }

    fn account_info(
        provider_gasfree_address: TronAddress,
        active: bool,
        allow_submit: bool,
        frozen: u64,
        transfer_fee: u64,
        activation_fee: u64,
        nonce: u64,
    ) -> GasfreeAccountInfo {
        account_info_with_decimals(
            provider_gasfree_address,
            active,
            allow_submit,
            frozen,
            transfer_fee,
            activation_fee,
            nonce,
            6,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn account_info_with_decimals(
        provider_gasfree_address: TronAddress,
        active: bool,
        allow_submit: bool,
        frozen: u64,
        transfer_fee: u64,
        activation_fee: u64,
        nonce: u64,
        decimals: u8,
    ) -> GasfreeAccountInfo {
        GasfreeAccountInfo {
            account_address: user_address(),
            gas_free_address: provider_gasfree_address,
            active,
            nonce: U256::from(nonce),
            allow_submit,
            assets: vec![GasfreeAccountAsset {
                token_address: token_address(),
                token_symbol: "USDT".into(),
                activate_fee: U256::from(activation_fee),
                transfer_fee: U256::from(transfer_fee),
                decimal: decimals,
                frozen: U256::from(frozen),
            }],
        }
    }

    #[test]
    fn inactive_account_without_required_total_balance_is_disabled_before_signing() {
        let provider = provider();
        let user = user_address();
        let local_gasfree = compute_gasfree_address_for_network(provider.network(), &user);
        let service = GasfreeAccountService::new(
            &provider,
            user,
            MockOnChainBalanceFetcher::default().with_balance(token_address(), local_gasfree, U256::from(59u64)),
        );
        let fetcher = MockAccountInfoFetcher::returning(account_info(local_gasfree, false, true, 10, 5, 7, 9));

        let preflight = common::block_on(service.preflight_with_account_fetcher(&request(38), &fetcher)).unwrap();

        assert_eq!(
            preflight.availability,
            GasfreeAvailability::Disabled {
                reason: DisabledReason::InactiveAccountInsufficientBalance,
            }
        );
    }

    #[test]
    fn frozen_balance_is_subtracted_from_spendable_balance() {
        let provider = provider();
        let user = user_address();
        let local_gasfree = compute_gasfree_address_for_network(provider.network(), &user);
        let service = GasfreeAccountService::new(
            &provider,
            user,
            MockOnChainBalanceFetcher::default().with_balance(token_address(), local_gasfree, U256::from(100u64)),
        );
        let fetcher = MockAccountInfoFetcher::returning(account_info(local_gasfree, true, true, 30, 10, 0, 3));

        let preflight = common::block_on(service.preflight_with_account_fetcher(&request(65), &fetcher)).unwrap();

        assert_eq!(preflight.on_chain_balance, U256::from(100u64));
        assert_eq!(preflight.frozen_balance, U256::from(30u64));
        assert_eq!(
            preflight.availability,
            GasfreeAvailability::Disabled {
                reason: DisabledReason::InsufficientSpendableBalance,
            }
        );
    }

    #[test]
    fn inactive_account_uses_required_total_balance_even_when_spendable_saturates() {
        let provider = provider();
        let user = user_address();
        let local_gasfree = compute_gasfree_address_for_network(provider.network(), &user);
        let service = GasfreeAccountService::new(
            &provider,
            user,
            MockOnChainBalanceFetcher::default().with_balance(token_address(), local_gasfree, U256::from(5u64)),
        );
        let fetcher = MockAccountInfoFetcher::returning(account_info(local_gasfree, false, true, 10, 0, 0, 4));

        let preflight = common::block_on(service.preflight_with_account_fetcher(&request(0), &fetcher)).unwrap();

        assert_eq!(
            preflight.availability,
            GasfreeAvailability::Disabled {
                reason: DisabledReason::InactiveAccountInsufficientBalance,
            }
        );
    }

    #[test]
    fn pending_transfer_short_circuits_before_balance_fetch() {
        let provider = provider();
        let user = user_address();
        let balance_fetcher = MockOnChainBalanceFetcher::default();
        let service = GasfreeAccountService::new(&provider, user, balance_fetcher.clone());
        let fetcher =
            MockAccountInfoFetcher::returning(account_info(*service.local_gasfree_address(), true, false, 0, 9, 0, 7));

        let preflight = common::block_on(service.preflight_with_account_fetcher(&request(10), &fetcher)).unwrap();

        assert_eq!(preflight.availability, GasfreeAvailability::PendingTransfer);
        assert_eq!(balance_fetcher.call_count(), 0);
    }

    #[test]
    fn address_mismatch_is_detected_on_every_preflight() {
        let provider = provider();
        let user = user_address();
        let second_user: TronAddress = "TEkj3ndMVEmFLYaFrATMwMjBRZ1EAZkucT".parse().unwrap();
        let balance_fetcher = MockOnChainBalanceFetcher::default();
        let service = GasfreeAccountService::new(&provider, user, balance_fetcher.clone());
        let second_service = GasfreeAccountService::new(&provider, second_user, balance_fetcher.clone());
        let first_wrong_gasfree_address: TronAddress = "TLvVuqx74fMy8QMjEsMT4dWwmVbuNwYt8X".parse().unwrap();
        let second_wrong_gasfree_address: TronAddress = "TKtWbdzEq5ss9vTS9kwRhBp5mXmBfBns3E".parse().unwrap();
        let fetcher = MockAccountInfoFetcher::default()
            .with_account_info(user, account_info(first_wrong_gasfree_address, true, true, 0, 5, 0, 1))
            .with_account_info(
                second_user,
                account_info(second_wrong_gasfree_address, true, true, 0, 5, 0, 1),
            );
        let first = common::block_on(service.preflight_with_account_fetcher(&request(1), &fetcher)).unwrap();

        assert_eq!(
            first.availability,
            GasfreeAvailability::Disabled {
                reason: DisabledReason::AddressMismatch {
                    expected: *service.local_gasfree_address(),
                    provider_reported: first_wrong_gasfree_address,
                },
            }
        );

        let second = common::block_on(second_service.preflight_with_account_fetcher(&request(1), &fetcher)).unwrap();

        assert_eq!(
            second.availability,
            GasfreeAvailability::Disabled {
                reason: DisabledReason::AddressMismatch {
                    expected: *second_service.local_gasfree_address(),
                    provider_reported: second_wrong_gasfree_address,
                },
            }
        );
        assert_eq!(fetcher.call_count(), 2);
        assert_eq!(balance_fetcher.call_count(), 0);
    }
}
