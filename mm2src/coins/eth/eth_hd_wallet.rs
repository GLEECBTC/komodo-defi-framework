use super::chain_address::ChainTaggedAddress;
use super::*;
use crate::coin_balance::HDAddressBalanceScanner;
use crate::hd_wallet::{
    ExtractExtendedPubkey, HDAccount, HDAddress, HDExtractPubkeyError, HDWallet, HDXPubExtractor, TrezorCoinError,
};
use async_trait::async_trait;
use bip32::DerivationPath;
use crypto::Secp256k1ExtendedPublicKey;
use ethereum_types::Public;

pub type EthHDAddress = HDAddress<ChainTaggedAddress, Public>;
pub type EthHDAccount = HDAccount<EthHDAddress, Secp256k1ExtendedPublicKey>;
pub type EthHDWallet = HDWallet<EthHDAccount>;

#[async_trait]
impl ExtractExtendedPubkey for EthCoin {
    type ExtendedPublicKey = Secp256k1ExtendedPublicKey;

    async fn extract_extended_pubkey<XPubExtractor>(
        &self,
        xpub_extractor: Option<XPubExtractor>,
        derivation_path: DerivationPath,
    ) -> MmResult<Self::ExtendedPublicKey, HDExtractPubkeyError>
    where
        XPubExtractor: HDXPubExtractor + Send,
    {
        extract_extended_pubkey_impl(self, xpub_extractor, derivation_path).await
    }
}

#[async_trait]
impl HDWalletCoinOps for EthCoin {
    type HDWallet = EthHDWallet;

    fn address_from_extended_pubkey(
        &self,
        extended_pubkey: &Secp256k1ExtendedPublicKey,
        derivation_path: DerivationPath,
    ) -> EthHDAddress {
        let pubkey = pubkey_from_extended(extended_pubkey);
        let raw = public_to_address(&pubkey);
        let family = ChainFamily::from(&self.0.chain_spec);

        EthHDAddress {
            address: ChainTaggedAddress::new(raw, family),
            pubkey,
            derivation_path,
        }
    }

    fn trezor_coin(&self) -> MmResult<String, TrezorCoinError> {
        self.trezor_coin.clone().or_mm_err(|| {
            let ticker = self.ticker();
            let error = format!("'{ticker}' coin has 'trezor_coin' field as `None` in the coins config");
            TrezorCoinError::Internal(error)
        })
    }
}

impl HDCoinWithdrawOps for EthCoin {}

#[async_trait]
#[cfg_attr(test, mockable)]
impl HDAddressBalanceScanner for EthCoin {
    type Address = ChainTaggedAddress;

    async fn is_address_used(&self, address: &Self::Address) -> BalanceResult<bool> {
        // TODO: Once EVM is migrated to ChainRpcClient, this can use a unified
        // `rpc_client.is_address_used_basic(address)` call without chain_spec matching.
        // See docs/plans/chain-rpc-client-refactor.md Section 17.
        let raw = address.inner();
        match &self.0.chain_spec {
            ChainSpec::Tron { .. } => {
                // TRON address usage detection.
                //
                // We use AccountCapsule existence check instead of transaction count because:
                // - TRON doesn't have an account nonce like Ethereum - it uses TAPOS (Transaction
                //   as Proof of Stake) with reference blocks for replay protection instead
                // - There's no `eth_getTransactionCount` equivalent on TRON (it throws an error)
                // - TRON requires an AccountCapsule to exist before ANY transaction can be made
                // - AccountCapsule check is a single efficient API call
                //
                // Note on Gas-Free: TRON's "Gas-Free" USDT transfers (paying fees in USDT instead
                // of TRX) still require the sender's account to be activated first.
                //
                // TODO: EIP-2612 permit edge case (applies to both EVM and TRON):
                // If a token implements permit, an address can receive tokens, sign offline, and
                // have a relayer call transferFrom() - without the owner making any transaction.
                // After tokens are transferred out, the address appears unused:
                // - EVM: account nonce = 0 (eth_getTransactionCount only counts OUTGOING txs)
                // - TRON: no AccountCapsule, balance = 0
                // The token contract stores a separate `nonces(owner)` that tracks permit usage,
                // but checking it requires RPC calls to each permit-enabled token contract.
                // Mainstream tokens (USDT, etc.) don't implement permit, so this is rare.
                let tron_client = self
                    .0
                    .tron_rpc()
                    .ok_or_else(|| BalanceError::Internal("TRON chain_spec but no TRON rpc_client".to_string()))?;
                let tron_addr = tron::TronAddress::from(raw);

                // First check: on-chain account existence via /wallet/getaccount API.
                // Returns true if the account's on-chain record (TRON calls this "AccountCapsule")
                // exists. Created when the address receives TRX or TRC10 tokens.
                if tron_client.is_address_used_basic(tron_addr).await.map_mm_err()? {
                    return Ok(true);
                }

                // Second check: TRC20 token balances for user-configured tokens.
                //
                // Edge case: TRC20 tokens can exist even if the account isn't activated on-chain.
                // Unlike TRX/TRC10, TRC20 balances are stored in the token contract's internal
                // mapping (not in the account's on-chain record), so an address can hold TRC20 tokens before
                // ever receiving TRX.
                //
                // This can happen when:
                // - Someone sends real tokens (USDT, etc.) to a new address before it's activated
                // - Legitimate project airdrops to addresses that haven't been used yet
                //
                // Note: We only query tokens the user has explicitly configured, so spam/phishing
                // tokens (address poisoning attacks) won't trigger false positives here.
                let token_balance_map = self.get_tokens_balance_list_for_address(raw).await?;
                Ok(token_balance_map.values().any(|balance| !balance.get_total().is_zero()))
            },
            ChainSpec::Evm { .. } => {
                // EVM path: `eth_getTransactionCount` returns the account nonce - the number of
                // OUTGOING transactions sent FROM this address (not incoming, not contract calls
                // made by others). If count > 0, the address has sent at least one transaction.
                // If count = 0, we fall back to balance checks to detect received-only addresses.
                //
                // Note: This misses the EIP-2612 permit edge case (see TODO in TRON branch above).
                // The token contract's `nonces(owner)` is separate from this account nonce.
                let count = self.transaction_count(raw, None).await?;
                if count > U256::zero() {
                    return Ok(true);
                }

                // We check for platform balance only first to reduce the number of requests to the node.
                // If this is a token added using init_token, then we check for this token balance only, and
                // we don't check for platform balance or other tokens that was added before.
                let platform_balance = self.address_balance(*address).compat().await?;
                if !platform_balance.is_zero() {
                    return Ok(true);
                }

                // This is done concurrently which increases the cost of the requests to the node.
                // But it's better than doing it sequentially to reduce the time.
                let token_balance_map = self.get_tokens_balance_list_for_address(raw).await?;
                Ok(token_balance_map.values().any(|balance| !balance.get_total().is_zero()))
            },
        }
    }
}

#[async_trait]
impl HDWalletBalanceOps for EthCoin {
    type HDAddressScanner = Self;
    type BalanceObject = CoinBalanceMap;

    async fn produce_hd_address_scanner(&self) -> BalanceResult<Self::HDAddressScanner> {
        Ok(self.clone())
    }

    async fn enable_hd_wallet<XPubExtractor>(
        &self,
        hd_wallet: &Self::HDWallet,
        xpub_extractor: Option<XPubExtractor>,
        params: EnabledCoinBalanceParams,
        path_to_address: &HDPathAccountToAddressId,
    ) -> MmResult<HDWalletBalance<Self::BalanceObject>, EnableCoinBalanceError>
    where
        XPubExtractor: HDXPubExtractor + Send,
    {
        coin_balance::common_impl::enable_hd_wallet(self, hd_wallet, xpub_extractor, params, path_to_address).await
    }

    async fn scan_for_new_addresses(
        &self,
        hd_wallet: &Self::HDWallet,
        hd_account: &mut EthHDAccount,
        address_scanner: &Self::HDAddressScanner,
        gap_limit: u32,
    ) -> BalanceResult<Vec<HDAddressBalance<Self::BalanceObject>>> {
        scan_for_new_addresses_impl(
            self,
            hd_wallet,
            hd_account,
            address_scanner,
            Bip44Chain::External,
            gap_limit,
        )
        .await
    }

    async fn all_known_addresses_balances(
        &self,
        hd_account: &EthHDAccount,
    ) -> BalanceResult<Vec<HDAddressBalance<Self::BalanceObject>>> {
        let external_addresses = hd_account
            .known_addresses_number(Bip44Chain::External)
            // A UTXO coin should support both [`Bip44Chain::External`] and [`Bip44Chain::Internal`].
            .mm_err(|e| BalanceError::Internal(e.to_string()))?;

        self.known_addresses_balances_with_ids(hd_account, Bip44Chain::External, 0..external_addresses)
            .await
    }

    async fn known_address_balance(&self, address: &ChainTaggedAddress) -> BalanceResult<Self::BalanceObject> {
        let balance = self
            .address_balance(*address)
            .and_then(move |result| u256_to_big_decimal(result, self.decimals()).map_mm_err())
            .compat()
            .await?;

        let coin_balance = CoinBalance {
            spendable: balance,
            unspendable: BigDecimal::from(0),
        };

        let mut balances = CoinBalanceMap::new();
        balances.insert(self.ticker().to_string(), coin_balance);

        let token_balances = self.get_tokens_balance_list_for_address(address.inner()).await?;
        balances.extend(token_balances);
        Ok(balances)
    }

    async fn known_addresses_balances(
        &self,
        addresses: Vec<ChainTaggedAddress>,
    ) -> BalanceResult<Vec<(ChainTaggedAddress, Self::BalanceObject)>> {
        let mut balance_futs = Vec::new();
        for address in addresses {
            let fut = async move {
                let balance = self.known_address_balance(&address).await?;
                Ok((address, balance))
            };
            balance_futs.push(fut);
        }
        try_join_all(balance_futs).await
    }

    async fn prepare_addresses_for_balance_stream_if_enabled(
        &self,
        _addresses: HashSet<String>,
    ) -> MmResult<(), String> {
        Ok(())
    }
}
