use super::{DisplayAddress, HDPathAccountToAddressId, HDWalletOps, HDWithdrawError};
use crate::hd_wallet::{HDAccountOps, HDAddressOps, HDAddressSelector, HDCoinAddress, HDWalletCoinOps};
use async_trait::async_trait;
use bip32::DerivationPath;
use mm2_err_handle::prelude::*;

type HDCoinPubKey<T> =
    <<<<T as HDWalletCoinOps>::HDWallet as HDWalletOps>::HDAccount as HDAccountOps>::HDAddress as HDAddressOps>::Pubkey;

/// Contains the details of the sender address for a withdraw operation.
pub struct WithdrawSenderAddress<Address, Pubkey> {
    pub(crate) address: Address,
    pub(crate) pubkey: Pubkey,
    pub(crate) derivation_path: Option<DerivationPath>,
}

/// `HDCoinWithdrawOps`: Operations that should be implemented for coins to support withdraw from HD wallets.
#[async_trait]
pub trait HDCoinWithdrawOps: HDWalletCoinOps {
    /// Fetches the sender address for a withdraw operation.
    /// This is the address from which the funds will be withdrawn.
    async fn get_withdraw_hd_sender(
        &self,
        hd_wallet: &Self::HDWallet,
        from: &HDAddressSelector,
    ) -> MmResult<WithdrawSenderAddress<HDCoinAddress<Self>, HDCoinPubKey<Self>>, HDWithdrawError> {
        let HDPathAccountToAddressId {
            account_id,
            chain,
            address_id,
        } = from
            .to_address_path(hd_wallet.coin_type())
            .mm_err(|err| HDWithdrawError::UnexpectedFromAddress(err.to_string()))?;

        let hd_account = hd_wallet
            .get_account(account_id)
            .await
            .or_mm_err(|| HDWithdrawError::UnknownAccount { account_id })?;

        let is_address_activated = hd_account
            .is_address_activated(chain, address_id)
            // If [`HDWalletCoinOps::derive_address`] succeeds, [`HDAccountOps::is_address_activated`] shouldn't fails with an `InvalidBip44ChainError`.
            .mm_err(|e| HDWithdrawError::InternalError(e.to_string()))?;

        let hd_address = self.derive_address(&hd_account, chain, address_id).await.map_mm_err()?;
        let address = hd_address.address();
        if !is_address_activated {
            let error = format!("'{}' address is not activated", address.display_address());
            return MmError::err(HDWithdrawError::UnexpectedFromAddress(error));
        }

        Ok(WithdrawSenderAddress {
            address,
            pubkey: hd_address.pubkey(),
            derivation_path: Some(hd_address.derivation_path().clone()),
        })
    }
}
