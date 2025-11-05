use crate::hd_wallet::{HDAccountStorageItem, HDWalletId, HDWalletStorageInternalOps, HDWalletStorageResult};
use async_trait::async_trait;
use mm2_core::mm_ctx::MmArc;
#[cfg(test)]
use mocktopus::macros::*;

pub(crate) struct HDWalletMockStorage;

#[async_trait]
#[cfg_attr(test, mockable)]
impl HDWalletStorageInternalOps for HDWalletMockStorage {
    async fn init(_ctx: &MmArc) -> HDWalletStorageResult<Self>
    where
        Self: Sized,
    {
        unimplemented!()
    }

    async fn load_accounts(&self, _wallet_id: HDWalletId) -> HDWalletStorageResult<Vec<HDAccountStorageItem>> {
        unimplemented!()
    }

    async fn load_account(
        &self,
        _wallet_id: HDWalletId,
        _account_id: u32,
    ) -> HDWalletStorageResult<Option<HDAccountStorageItem>> {
        unimplemented!()
    }

    async fn update_external_addresses_number(
        &self,
        _wallet_id: HDWalletId,
        _account_id: u32,
        _new_external_addresses_number: u32,
    ) -> HDWalletStorageResult<()> {
        unimplemented!()
    }

    async fn update_internal_addresses_number(
        &self,
        _wallet_id: HDWalletId,
        _account_id: u32,
        _new_internal_addresses_number: u32,
    ) -> HDWalletStorageResult<()> {
        unimplemented!()
    }

    async fn upload_new_account(
        &self,
        _wallet_id: HDWalletId,
        _account: HDAccountStorageItem,
    ) -> HDWalletStorageResult<()> {
        unimplemented!()
    }

    async fn clear_accounts(&self, _wallet_id: HDWalletId) -> HDWalletStorageResult<()> {
        unimplemented!()
    }
}
