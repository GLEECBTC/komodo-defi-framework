use crate::account::storage::{AccountStorage, AccountStorageBuilder, AccountStorageError, AccountStorageResult};
use crate::account::{AccountId, AccountInfo, AccountWithCoins, AccountWithEnabledFlag, EnabledAccountId, HwPubkey};
use mm2_number::BigDecimal;
use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;
use std::collections::{BTreeMap, BTreeSet};

const HD_0_ACCOUNT: AccountId = AccountId::HD { account_idx: 0 };
const HD_1_ACCOUNT: AccountId = AccountId::HD { account_idx: 1 };
const HD_2_ACCOUNT: AccountId = AccountId::HD { account_idx: 2 };
const HD_3_ACCOUNT: AccountId = AccountId::HD { account_idx: 3 };

fn account_ids_for_test() -> Vec<AccountId> {
    vec![
        AccountId::Iguana,
        HD_0_ACCOUNT,
        AccountId::HW {
            device_pubkey: HwPubkey::from("1549128bbfb33b997949b4105b6a6371c998e212"),
        },
        AccountId::HW {
            device_pubkey: HwPubkey::from("f97d3a43dbea0993f1b7a6a299377d4ee164c849"),
        },
        AccountId::HW {
            device_pubkey: HwPubkey::from("69a20008cea0c15ee483b5bbdff942752634aa07"),
        },
        HD_1_ACCOUNT,
    ]
}

fn accounts_for_test() -> Vec<AccountInfo> {
    account_ids_for_test()
        .iter()
        .enumerate()
        .map(|(i, account_id)| AccountInfo {
            account_id: account_id.clone(),
            name: format!("Account {i}"),
            description: format!("Description {i}"),
            balance_usd: BigDecimal::from(i as u64),
        })
        .collect()
}

fn accounts_to_map(accounts: Vec<AccountInfo>) -> BTreeMap<AccountId, AccountInfo> {
    accounts
        .into_iter()
        .map(|account| (account.account_id.clone(), account))
        .collect()
}

fn tag_with_enabled_flag(
    accounts: BTreeMap<AccountId, AccountInfo>,
    enabled: AccountId,
) -> BTreeMap<AccountId, AccountWithEnabledFlag> {
    accounts
        .into_iter()
        .map(|(account_id, account_info)| {
            (
                account_id.clone(),
                AccountWithEnabledFlag {
                    account_info,
                    enabled: account_id == enabled,
                },
            )
        })
        .collect()
}

async fn fill_storage(storage: &dyn AccountStorage, accounts: Vec<AccountInfo>) -> AccountStorageResult<()> {
    for account in accounts {
        storage.upload_account(account.clone()).await?;
    }
    Ok(())
}

async fn test_init_collection_impl() {
    let ctx = mm_ctx_with_custom_db();
    let storage = AccountStorageBuilder::new(&ctx).build().unwrap();

    storage.init().await.unwrap();
    // repetitive init must not fail
    storage.init().await.unwrap();
}

async fn test_upload_account_impl() {
    let ctx = mm_ctx_with_custom_db();
    let storage = AccountStorageBuilder::new(&ctx).build().unwrap();
    storage.init().await.unwrap();

    for account in accounts_for_test() {
        storage.upload_account(account.clone()).await.unwrap();

        let account_id = account.account_id.clone();
        let error = storage.upload_account(account).await.expect_err(&format!(
            "Uploading should have since the account {account_id:?} has been uploaded already"
        ));
        match error.into_inner() {
            AccountStorageError::AccountExistsAlready(found) if found == account_id => (),
            other => panic!("Expected 'AccountExistsAlready({account_id:?})' found {other:?}"),
        }
    }
}

async fn test_enable_account_impl() {
    let ctx = mm_ctx_with_custom_db();
    let storage = AccountStorageBuilder::new(&ctx).build().unwrap();
    storage.init().await.unwrap();

    let error = storage
        .enable_account(EnabledAccountId::Iguana)
        .await
        .expect_err("'enable_account' should have failed due to the selected account is not present in the storage");
    match error.into_inner() {
        AccountStorageError::NoSuchAccount(AccountId::Iguana) => (),
        other => panic!("Expected 'NoSuchAccount(Iguana)', found {other:?}"),
    }

    let accounts = accounts_to_map(accounts_for_test());

    let account_iguana = accounts.get(&AccountId::Iguana).unwrap().clone();
    storage.upload_account(account_iguana).await.unwrap();
    storage.enable_account(EnabledAccountId::Iguana).await.unwrap();

    // Try to enable an unknown account and check if `Iguana` is still enabled.
    let error = storage
        .enable_account(EnabledAccountId::HD { account_idx: 3 })
        .await
        .expect_err("'enable_account' should have failed due to the selected account is not present in the storage");
    match error.into_inner() {
        AccountStorageError::NoSuchAccount(HD_3_ACCOUNT) => (),
        other => panic!("Expected 'NoSuchAccount(HD)', found {other:?}"),
    }
    let actual_enabled = storage.load_enabled_account_id().await.unwrap();
    assert_eq!(actual_enabled, EnabledAccountId::Iguana);

    // Upload new accounts.
    let account_hd_1 = accounts.get(&HD_0_ACCOUNT).unwrap().clone();
    storage.upload_account(account_hd_1).await.unwrap();

    let account_hd_2 = accounts.get(&HD_1_ACCOUNT).unwrap().clone();
    storage.upload_account(account_hd_2).await.unwrap();

    // Check if Iguana account is still enabled.
    let actual_enabled = storage.load_enabled_account_id().await.unwrap();
    assert_eq!(actual_enabled, EnabledAccountId::Iguana);

    // Enable HD-1 account
    storage
        .enable_account(EnabledAccountId::HD { account_idx: 1 })
        .await
        .unwrap();
    let actual_enabled = storage.load_enabled_account_id().await.unwrap();
    assert_eq!(actual_enabled, EnabledAccountId::HD { account_idx: 1 });
}

async fn test_set_name_desc_balance_impl() {
    let ctx = mm_ctx_with_custom_db();
    let storage = AccountStorageBuilder::new(&ctx).build().unwrap();
    storage.init().await.unwrap();

    let accounts = accounts_for_test();

    fill_storage(storage.as_ref(), accounts.clone()).await.unwrap();
    storage.enable_account(EnabledAccountId::Iguana).await.unwrap();

    storage
        .set_name(AccountId::Iguana, "New name".to_string())
        .await
        .unwrap();

    storage
        .set_description(HD_1_ACCOUNT, "New description".to_string())
        .await
        .unwrap();

    let hw_id = AccountId::HW {
        device_pubkey: HwPubkey::from("69a20008cea0c15ee483b5bbdff942752634aa07"),
    };
    storage.set_balance(hw_id.clone(), BigDecimal::from(23)).await.unwrap();

    let mut expected = accounts_to_map(accounts);
    expected.get_mut(&AccountId::Iguana).unwrap().name = "New name".to_string();
    expected.get_mut(&HD_1_ACCOUNT).unwrap().description = "New description".to_string();
    expected.get_mut(&hw_id).unwrap().balance_usd = BigDecimal::from(23);

    let actual = storage.load_accounts().await.unwrap();
    assert_eq!(actual, expected);

    let error = storage
        .set_name(HD_2_ACCOUNT, "New name 4".to_string())
        .await
        .expect_err("'AccountStorage::set_name' should have failed due to an unknown 'AccountId'");

    match error.into_inner() {
        AccountStorageError::NoSuchAccount(HD_2_ACCOUNT) => (),
        other => panic!("Expected 'NoSuchAccount(HD)' error, found: {other}"),
    }
}

async fn test_activate_deactivate_coins_impl() {
    let ctx = mm_ctx_with_custom_db();
    let storage = AccountStorageBuilder::new(&ctx).build().unwrap();
    storage.init().await.unwrap();

    let accounts = accounts_for_test();

    let error = storage.load_account_coins(AccountId::Iguana).await.expect_err(
        "'AccountStorage::load_enabled_account_with_coins' should have failed since no account was enabled",
    );
    match error.into_inner() {
        AccountStorageError::NoSuchAccount(AccountId::Iguana) => (),
        other => panic!("Expected 'NoSuchAccount(Iguana)' error, found: {other}"),
    }

    fill_storage(storage.as_ref(), accounts).await.unwrap();

    // Deactivating unknown coins should never fail.
    storage
        .deactivate_coins(AccountId::Iguana, vec!["RICK".to_string(), "MORTY".to_string()])
        .await
        .unwrap();

    // Try to reactivate `RICK` coin, it should be ignored.
    storage
        .activate_coins(AccountId::Iguana, vec!["RICK".to_string()])
        .await
        .unwrap();
    // Try to reactivate `MORTY` and activate `BTC` coins, `MORTY` should be ignored.
    storage
        .activate_coins(AccountId::Iguana, vec!["MORTY".to_string(), "BTC".to_string()])
        .await
        .unwrap();
    storage
        .activate_coins(
            HD_0_ACCOUNT,
            vec!["MORTY".to_string(), "QTUM".to_string(), "KMD".to_string()],
        )
        .await
        .unwrap();

    let actual = storage.load_account_coins(AccountId::Iguana).await.unwrap();
    let expected = vec!["RICK".to_string(), "MORTY".to_string(), "BTC".to_string()]
        .into_iter()
        .collect();
    assert_eq!(actual, expected);

    let actual = storage.load_account_coins(HD_0_ACCOUNT).await.unwrap();
    let expected = vec!["MORTY".to_string(), "QTUM".to_string(), "KMD".to_string()]
        .into_iter()
        .collect();
    assert_eq!(actual, expected);

    // Deactivate `QTUM` and an unknown `BCH` coins for the `HD{0}` account.
    storage
        .deactivate_coins(HD_0_ACCOUNT, vec!["BCH".to_string(), "QTUM".to_string()])
        .await
        .unwrap();
    let actual = storage.load_account_coins(HD_0_ACCOUNT).await.unwrap();
    let expected = vec!["MORTY".to_string(), "KMD".to_string()].into_iter().collect();
    assert_eq!(actual, expected);

    // Deactivate all `HD{0}` account's coins.
    storage
        .deactivate_coins(HD_0_ACCOUNT, vec!["MORTY".to_string(), "KMD".to_string()])
        .await
        .unwrap();
    let actual = storage.load_account_coins(HD_0_ACCOUNT).await.unwrap();
    assert!(actual.is_empty());

    // Try to activate a coin for an unknown `HD{2}` account.
    let error = storage
        .activate_coins(HD_2_ACCOUNT, vec!["RICK".to_string()])
        .await
        .expect_err("'AccountStorage::activate_coins' should have failed due to an unknown account_id");
    match error.into_inner() {
        AccountStorageError::NoSuchAccount(HD_2_ACCOUNT) => (),
        other => panic!("Expected 'NoSuchAccount(HD)' error, found: {other}"),
    }

    // Try to deactivate a coin for an unknown `HD{3}` account.
    let error = storage
        .deactivate_coins(HD_3_ACCOUNT, vec!["MORTY".to_string()])
        .await
        .expect_err("'AccountStorage::deactivate_coins' should have failed due to an unknown account_id");
    match error.into_inner() {
        AccountStorageError::NoSuchAccount(HD_3_ACCOUNT) => (),
        other => panic!("Expected 'NoSuchAccount(HD)' error, found: {other}"),
    }
}

async fn test_load_enabled_account_with_coins_impl() {
    let ctx = mm_ctx_with_custom_db();
    let storage = AccountStorageBuilder::new(&ctx).build().unwrap();
    storage.init().await.unwrap();

    let accounts = accounts_for_test();
    let accounts_map = accounts_to_map(accounts.clone());
    fill_storage(storage.as_ref(), accounts).await.unwrap();

    let error = storage.load_enabled_account_with_coins().await.expect_err(
        "'AccountStorage::load_enabled_account_with_coins' should have failed since no account was enabled",
    );
    match error.into_inner() {
        AccountStorageError::NoEnabledAccount => (),
        other => panic!("Expected 'NoEnabledAccount' error, found: {other}"),
    }

    storage.enable_account(EnabledAccountId::Iguana).await.unwrap();

    storage
        .activate_coins(AccountId::Iguana, vec!["RICK".to_string(), "MORTY".to_string()])
        .await
        .unwrap();
    storage
        .activate_coins(
            HD_0_ACCOUNT,
            vec!["MORTY".to_string(), "QTUM".to_string(), "KMD".to_string()],
        )
        .await
        .unwrap();

    let actual = storage.load_enabled_account_with_coins().await.unwrap();
    let expected = AccountWithCoins {
        account_info: accounts_map.get(&AccountId::Iguana).unwrap().clone(),
        coins: vec!["RICK".to_string(), "MORTY".to_string()].into_iter().collect(),
    };
    assert_eq!(actual, expected);

    // Enable `HD{0}` account and load its activated coins.
    storage
        .enable_account(EnabledAccountId::HD { account_idx: 0 })
        .await
        .unwrap();
    let actual = storage.load_enabled_account_with_coins().await.unwrap();
    let expected = AccountWithCoins {
        account_info: accounts_map.get(&HD_0_ACCOUNT).unwrap().clone(),
        coins: vec!["MORTY".to_string(), "QTUM".to_string(), "KMD".to_string()]
            .into_iter()
            .collect(),
    };
    assert_eq!(actual, expected);

    // Deactivate all `HD{0}` account's coins.
    storage
        .deactivate_coins(
            HD_0_ACCOUNT,
            vec!["MORTY".to_string(), "QTUM".to_string(), "KMD".to_string()],
        )
        .await
        .unwrap();
    let actual = storage.load_enabled_account_with_coins().await.unwrap();
    let expected = AccountWithCoins {
        account_info: accounts_map.get(&HD_0_ACCOUNT).unwrap().clone(),
        coins: BTreeSet::new(),
    };
    assert_eq!(actual, expected);
}

async fn test_load_accounts_with_enabled_flag_impl() {
    let ctx = mm_ctx_with_custom_db();
    let storage = AccountStorageBuilder::new(&ctx).build().unwrap();
    storage.init().await.unwrap();

    let accounts = accounts_for_test();
    let accounts_map = accounts_to_map(accounts.clone());

    fill_storage(storage.as_ref(), accounts.clone()).await.unwrap();

    let error = storage.load_accounts_with_enabled_flag().await.expect_err(
        "'AccountStorage::load_accounts_with_enabled_flag' should have failed since no account was enabled",
    );
    match error.into_inner() {
        AccountStorageError::NoEnabledAccount => (),
        other => panic!("Expected 'NoEnabledAccount' error, found: {other}"),
    }

    storage
        .enable_account(EnabledAccountId::HD { account_idx: 0 })
        .await
        .unwrap();
    let actual = storage.load_accounts_with_enabled_flag().await.unwrap();
    let expected = tag_with_enabled_flag(accounts_map.clone(), HD_0_ACCOUNT);
    assert_eq!(actual, expected);

    storage
        .enable_account(EnabledAccountId::HD { account_idx: 1 })
        .await
        .unwrap();
    let actual = storage.load_accounts_with_enabled_flag().await.unwrap();
    let expected = tag_with_enabled_flag(accounts_map.clone(), HD_1_ACCOUNT);
    assert_eq!(actual, expected);

    // Try to re-enable the same `HD{1}` account.
    storage
        .enable_account(EnabledAccountId::HD { account_idx: 1 })
        .await
        .unwrap();
    let actual = storage.load_accounts_with_enabled_flag().await.unwrap();
    let expected = tag_with_enabled_flag(accounts_map.clone(), HD_1_ACCOUNT);
    assert_eq!(actual, expected);

    storage.enable_account(EnabledAccountId::Iguana).await.unwrap();
    let actual = storage.load_accounts_with_enabled_flag().await.unwrap();
    let expected = tag_with_enabled_flag(accounts_map.clone(), AccountId::Iguana);
    assert_eq!(actual, expected);
}

async fn test_delete_account_impl() {
    let ctx = mm_ctx_with_custom_db();
    let storage = AccountStorageBuilder::new(&ctx).build().unwrap();
    storage.init().await.unwrap();

    let accounts = accounts_for_test();
    let accounts_map = accounts_to_map(accounts.clone());

    fill_storage(storage.as_ref(), accounts).await.unwrap();

    let hw_id = AccountId::HW {
        device_pubkey: HwPubkey::from("69a20008cea0c15ee483b5bbdff942752634aa07"),
    };
    storage.delete_account(hw_id.clone()).await.unwrap();
    let actual_accounts = storage.load_accounts().await.unwrap();
    let mut expected_accounts = accounts_map.clone();
    expected_accounts.remove(&hw_id);
    assert_eq!(actual_accounts, expected_accounts);

    // Try to delete the same account twice.
    let error = storage
        .delete_account(hw_id)
        .await
        .expect_err("'AccountStorage::delete_account' should have failed due to unknown account");
    match error.into_inner() {
        AccountStorageError::NoSuchAccount(AccountId::HW { .. }) => (),
        other => panic!("Expected 'NoSuchAccount' error, found: {other}"),
    }

    // Enable `HD{1}` account and try to remove `HD{0}` to check if `HD{1}` will stay enabled.
    storage
        .enable_account(EnabledAccountId::HD { account_idx: 1 })
        .await
        .unwrap();
    storage.delete_account(HD_0_ACCOUNT).await.unwrap();
    let actual = storage.load_enabled_account_with_coins().await.unwrap();
    let expected = AccountWithCoins {
        account_info: accounts_map.get(&HD_1_ACCOUNT).unwrap().clone(),
        coins: BTreeSet::new(),
    };
    assert_eq!(actual, expected);

    // Delete `HD{1}` account, and then try to get an enabled account with coins.
    storage
        .enable_account(EnabledAccountId::HD { account_idx: 1 })
        .await
        .unwrap();
    storage.delete_account(HD_1_ACCOUNT).await.unwrap();
    let error = storage
        .load_enabled_account_with_coins()
        .await
        .expect_err("'AccountStorage::load_enabled_account_with_coins' should have failed since no enabled account");
    match error.into_inner() {
        AccountStorageError::NoEnabledAccount => (),
        other => panic!("Expected 'NoEnabledAccount' error, found: {other}"),
    }
}

async fn test_delete_account_clears_coins_impl() {
    let ctx = mm_ctx_with_custom_db();
    let storage = AccountStorageBuilder::new(&ctx).build().unwrap();
    storage.init().await.unwrap();

    let accounts = accounts_for_test();
    let accounts_map = accounts_to_map(accounts.clone());

    fill_storage(storage.as_ref(), accounts).await.unwrap();

    // Activate coins, delete the account and re-activate it again to make sure that all associated coins were deleted.
    storage
        .activate_coins(AccountId::Iguana, vec!["RICK".to_string(), "MORTY".to_string()])
        .await
        .unwrap();
    // Activate also coins for another account.
    storage
        .activate_coins(HD_0_ACCOUNT, vec!["RICK".to_string(), "KMD".to_string()])
        .await
        .unwrap();

    storage.delete_account(AccountId::Iguana).await.unwrap();

    let new_iguana = AccountInfo {
        account_id: AccountId::Iguana,
        name: "My iguana".to_string(),
        description: "My description".to_string(),
        balance_usd: BigDecimal::from(123),
    };
    storage.upload_account(new_iguana.clone()).await.unwrap();
    storage.enable_account(EnabledAccountId::Iguana).await.unwrap();

    let actual = storage.load_enabled_account_with_coins().await.unwrap();
    let expected = AccountWithCoins {
        account_info: new_iguana,
        coins: BTreeSet::new(),
    };
    assert_eq!(actual, expected);

    // Check if `HD{0}` coins haven't been cleared.
    storage
        .enable_account(EnabledAccountId::HD { account_idx: 0 })
        .await
        .unwrap();
    let actual = storage.load_enabled_account_with_coins().await.unwrap();
    let expected = AccountWithCoins {
        account_info: accounts_map.get(&HD_0_ACCOUNT).unwrap().clone(),
        coins: vec!["RICK".to_string(), "KMD".to_string()].into_iter().collect(),
    };
    assert_eq!(actual, expected);
}

#[cfg(not(target_arch = "wasm32"))]
mod native_tests {
    use common::block_on;

    #[test]
    fn test_init_collection() {
        block_on(super::test_init_collection_impl())
    }

    #[test]
    fn test_upload_account() {
        block_on(super::test_upload_account_impl())
    }

    #[test]
    fn test_enable_account() {
        block_on(super::test_enable_account_impl())
    }

    #[test]
    fn test_set_name_desc_balance() {
        block_on(super::test_set_name_desc_balance_impl())
    }

    #[test]
    fn test_activate_deactivate_coins() {
        block_on(super::test_activate_deactivate_coins_impl())
    }

    #[test]
    fn test_load_enabled_account_with_coins() {
        block_on(super::test_load_enabled_account_with_coins_impl())
    }

    #[test]
    fn test_load_accounts_with_enabled_flag() {
        block_on(super::test_load_accounts_with_enabled_flag_impl())
    }

    #[test]
    fn test_delete_account() {
        block_on(super::test_delete_account_impl())
    }

    #[test]
    fn test_delete_account_clears_coins() {
        block_on(super::test_delete_account_clears_coins_impl())
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_tests {
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    async fn test_init_collection() {
        super::test_init_collection_impl().await
    }

    #[wasm_bindgen_test]
    async fn test_upload_account() {
        super::test_upload_account_impl().await
    }

    #[wasm_bindgen_test]
    async fn test_enable_account() {
        super::test_enable_account_impl().await
    }

    #[wasm_bindgen_test]
    async fn test_set_name_desc_balance() {
        super::test_set_name_desc_balance_impl().await
    }

    #[wasm_bindgen_test]
    async fn test_activate_deactivate_coins() {
        super::test_activate_deactivate_coins_impl().await
    }

    #[wasm_bindgen_test]
    async fn test_load_enabled_account_with_coins() {
        super::test_load_enabled_account_with_coins_impl().await
    }

    #[wasm_bindgen_test]
    async fn test_load_accounts_with_enabled_flag() {
        super::test_load_accounts_with_enabled_flag_impl().await
    }

    #[wasm_bindgen_test]
    async fn test_delete_account() {
        super::test_delete_account_impl().await
    }

    #[wasm_bindgen_test]
    async fn test_delete_account_clears_coins() {
        super::test_delete_account_clears_coins_impl().await
    }
}
