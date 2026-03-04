//! TRON activation integration tests
//!
//! Run with: cargo test --test mm2_tests_main --features tron-network-tests tron_

use coins::eth::tron::TronAddress;
use common::block_on;
use mm2_test_helpers::for_tests::{
    account_balance, enable_erc20_token_v2, enable_trx_with_tokens, get_new_address, task_enable_trx,
    task_enable_trx_with_tokens, trc20_usdt_nile_conf, trx_conf, MarketMakerIt, Mm2TestConf, Mm2TestConfForSwap,
    TRON_NILE_NODES, TRON_NILE_TRC20_USDT_CONTRACT, TRON_NILE_TRC20_USDT_TICKER,
};
use mm2_test_helpers::structs::{Bip44Chain, EnableCoinBalanceMap, EthWithTokensActivationResult, HDAccountAddressId};

/// Test mnemonic for used-but-zero-balance scenario.
/// Index 0: TSqB9tqfaQ1DYSdMCbVSLPzQsaNVjeu9hq (funded ~1777.8 TRX)
/// Index 2: TPoJwueR4xfZCXuQTYqem4edQgoM3uV78n (0 balance but has tx history)
const TRON_USED_ZERO_BALANCE_PASSPHRASE: &str =
    "top wonder island doctor gesture velvet local media begin impose soccer radar";

/// BOB_HD_PASSPHRASE address at index 10 - funded with TRC20 USDT only (no TRX).
/// Beyond the last TRX-funded address (index 7), used to verify TRC20-only detection
/// during HD wallet gap scanning.
const BOB_HD_TRC20_ONLY_ADDRESS_INDEX_10: &str = "THng6CmEwpJqu5GJN6TabY2sRicKqJPS25";

/// Test TRX + TRC20 activation works via enable_eth_with_tokens (immediate mode).
/// Also validates TRC20 token balance propagation in HD wallet structure.
#[test]
fn test_trx_activation_immediate() {
    // Validate TRC20 contract address constant (from_base58 checks the 0x41 prefix that encodes to 'T')
    TronAddress::from_base58(TRON_NILE_TRC20_USDT_CONTRACT).expect("Invalid TRC20 Base58 contract address constant");

    let coins = serde_json::json!([trx_conf(), trc20_usdt_nile_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let result = block_on(enable_trx_with_tokens(
        &mm,
        TRON_NILE_NODES,
        &[TRON_NILE_TRC20_USDT_TICKER],
    ));

    assert!(result.get("result").is_some(), "Expected result field in response");
    let activation: EthWithTokensActivationResult =
        serde_json::from_value(result["result"].clone()).expect("Failed to parse activation result");

    let hd = match activation {
        EthWithTokensActivationResult::HD(hd) => hd,
        EthWithTokensActivationResult::Iguana(_) => {
            panic!("Expected HD activation result for TRX+TRC20 platform activation")
        },
    };

    assert!(hd.current_block > 0, "current_block should be greater than 0");
    assert_eq!(hd.ticker, "TRX", "Platform ticker should be TRX");

    // Validate TRC20 token balance is present at specific address (like ETH tests)
    let balance = match hd.wallet_balance {
        EnableCoinBalanceMap::HD(hd_bal) => hd_bal,
        _ => panic!("Expected EnableCoinBalanceMap::HD"),
    };
    let account0 = balance.accounts.first().expect("Expected first HD account entry");
    assert!(
        account0.addresses[0].balance.contains_key(TRON_NILE_TRC20_USDT_TICKER),
        "Expected TRC20 {} balance entry for address index 0",
        TRON_NILE_TRC20_USDT_TICKER
    );

    block_on(mm.stop()).unwrap();
}

/// Test TRX + TRC20 activation works via task::enable_eth::init (task-based mode).
/// Also validates TRC20 token balance propagation in HD wallet structure.
#[test]
fn test_trx_activation_task_based() {
    // Validate TRC20 contract address constant (from_base58 checks the 0x41 prefix that encodes to 'T')
    TronAddress::from_base58(TRON_NILE_TRC20_USDT_CONTRACT).expect("Invalid TRC20 Base58 contract address constant");

    let coins = serde_json::json!([trx_conf(), trc20_usdt_nile_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let result = block_on(task_enable_trx_with_tokens(
        &mm,
        TRON_NILE_NODES,
        &[TRON_NILE_TRC20_USDT_TICKER],
        90,
        None,
    ))
    .expect("TRX+TRC20 task-based activation should succeed");

    let hd = match result {
        EthWithTokensActivationResult::HD(hd) => hd,
        EthWithTokensActivationResult::Iguana(_) => {
            panic!("Expected HD activation result for TRX+TRC20 platform activation (task-based)")
        },
    };

    assert!(hd.current_block > 0, "current_block should be greater than 0");
    assert_eq!(hd.ticker, "TRX", "Ticker should be TRX");

    // Validate TRC20 token balance is present at specific address (like ETH tests)
    let balance = match hd.wallet_balance {
        EnableCoinBalanceMap::HD(hd_bal) => hd_bal,
        _ => panic!("Expected EnableCoinBalanceMap::HD"),
    };
    let account0 = balance.accounts.first().expect("Expected first HD account entry");
    assert!(
        account0.addresses[0].balance.contains_key(TRON_NILE_TRC20_USDT_TICKER),
        "Expected TRC20 {} balance entry for address index 0",
        TRON_NILE_TRC20_USDT_TICKER
    );

    block_on(mm.stop()).unwrap();
}

/// Test node failover: dead node first, good node second = success
#[test]
fn test_trx_activation_node_failover() {
    let coins = serde_json::json!([trx_conf()]);
    let conf = Mm2TestConf::seednode(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let nodes = ["http://127.0.0.1:1", TRON_NILE_NODES[0]];
    let result =
        block_on(task_enable_trx(&mm, &nodes, 60, None)).expect("Expected TRX activation to succeed via node failover");

    match result {
        EthWithTokensActivationResult::Iguana(r) => {
            assert!(r.current_block > 0);
            assert!(!r.eth_addresses_infos.is_empty(), "Expected at least one address");
            for addr in r.eth_addresses_infos.keys() {
                TronAddress::from_base58(addr).expect("Invalid base58check TRON address");
            }
        },
        EthWithTokensActivationResult::HD(r) => {
            assert!(r.current_block > 0);
            assert_eq!(r.ticker, "TRX");
        },
    }

    block_on(mm.stop()).unwrap();
}

/// Test HD wallet activation with specific derivation path
#[test]
fn test_trx_hd_activation_with_path() {
    let coins = serde_json::json!([trx_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let path_to_address = HDAccountAddressId {
        account_id: 0,
        chain: Bip44Chain::External,
        address_id: 0,
    };

    let result = block_on(task_enable_trx(&mm, TRON_NILE_NODES, 60, Some(path_to_address)))
        .expect("Expected TRX HD activation to succeed");

    let hd = match result {
        EthWithTokensActivationResult::HD(hd) => hd,
        EthWithTokensActivationResult::Iguana(_) => panic!("Expected HD activation result"),
    };

    let balance = match hd.wallet_balance {
        EnableCoinBalanceMap::HD(hd_bal) => hd_bal,
        EnableCoinBalanceMap::Iguana(_) => panic!("Expected EnableCoinBalanceMap::HD"),
    };

    let account0 = balance
        .accounts
        .first()
        .expect("Expected account 0 in HD wallet balance");
    let addr0 = &account0.addresses[0].address;

    TronAddress::from_base58(addr0).expect("Invalid base58check TRON address");

    block_on(mm.stop()).unwrap();
}

/// Test get_new_address and account_balance RPCs with TRC20 token propagation.
/// Validates that TRC20 token balances are included in new address responses.
#[test]
fn test_trx_get_new_address_rpc_hd() {
    let coins = serde_json::json!([trx_conf(), trc20_usdt_nile_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    // Activate TRX with TRC20 token
    let _activation = block_on(task_enable_trx_with_tokens(
        &mm,
        TRON_NILE_NODES,
        &[TRON_NILE_TRC20_USDT_TICKER],
        90,
        Some(HDAccountAddressId {
            account_id: 0,
            chain: Bip44Chain::External,
            address_id: 0,
        }),
    ))
    .expect("Expected TRX+TRC20 HD activation to succeed");

    // Test get_new_address for TRX
    let addr1 = block_on(get_new_address(&mm, "TRX", 0, Some(Bip44Chain::External)));
    TronAddress::from_base58(&addr1.new_address.address)
        .expect("Invalid base58check TRON address returned by get_new_address");

    match addr1.new_address.chain {
        Bip44Chain::External => (),
        Bip44Chain::Internal => panic!("Expected External chain for get_new_address(TRX)"),
    };

    assert!(
        addr1.new_address.derivation_path.starts_with("m/44'/195'/0'/0/"),
        "Unexpected TRX derivation_path: {}",
        addr1.new_address.derivation_path
    );
    assert!(
        addr1.new_address.balance.contains_key("TRX"),
        "Expected TRX balance entry for get_new_address response"
    );
    // TRC20 token balance should be included in the new address balance map
    assert!(
        addr1.new_address.balance.contains_key(TRON_NILE_TRC20_USDT_TICKER),
        "Expected TRC20 {} balance entry for get_new_address response",
        TRON_NILE_TRC20_USDT_TICKER
    );

    // Test account_balance includes the new address.
    // During HD activation the scanner walks addresses up to the gap limit (default 20) checking
    // both TRX account existence and TRC20 balances via is_address_used(). This means the wallet
    // can have 20+ known addresses after activation. account_balance defaults to page size 10,
    // so we pass limit=50 to ensure the newly generated address is included in the response.
    let bal = block_on(account_balance(&mm, "TRX", 0, Bip44Chain::External, Some(50)));
    let found = bal.addresses.iter().any(|a| a.address == addr1.new_address.address);
    assert!(
        found,
        "Expected get_new_address(TRX) address to be present in account_balance addresses list"
    );

    // Verify TRC20 token balance is present in account_balance response
    let addr_with_token = bal.addresses.iter().find(|a| a.address == addr1.new_address.address);
    assert!(
        addr_with_token.is_some_and(|a| a.balance.contains_key(TRON_NILE_TRC20_USDT_TICKER)),
        "Expected TRC20 {} balance in account_balance address entry",
        TRON_NILE_TRC20_USDT_TICKER
    );

    let addr2 = block_on(get_new_address(&mm, "TRX", 0, Some(Bip44Chain::External)));
    assert_ne!(addr1.new_address.address, addr2.new_address.address);

    block_on(mm.stop()).unwrap();
}

/// Test HD balance structure with funded addresses (BOB_HD_PASSPHRASE)
/// Funding: index 0 (~1967 TRX), index 1 (20 TRX), index 7 (5 TRX)
/// Also validates TRC20 token balance structure propagation across all addresses.
#[test]
fn test_trx_hd_balance_structure_assertions_and_funded_amounts() {
    let coins = serde_json::json!([trx_conf(), trc20_usdt_nile_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let result = block_on(task_enable_trx_with_tokens(
        &mm,
        TRON_NILE_NODES,
        &[TRON_NILE_TRC20_USDT_TICKER],
        90,
        Some(HDAccountAddressId {
            account_id: 0,
            chain: Bip44Chain::External,
            address_id: 7,
        }),
    ))
    .expect("Expected TRX+TRC20 HD activation to succeed");

    let hd = match result {
        EthWithTokensActivationResult::HD(hd) => hd,
        _ => panic!("Expected HD activation result"),
    };
    assert_eq!(hd.ticker, "TRX");
    assert!(hd.current_block > 0);

    let balance = match hd.wallet_balance {
        EnableCoinBalanceMap::HD(hd_bal) => hd_bal,
        _ => panic!("Expected EnableCoinBalanceMap::HD"),
    };

    let account0 = balance.accounts.first().expect("Expected first HD account entry");
    assert_eq!(account0.account_index, 0, "Expected account_index=0");
    assert!(
        account0.addresses.len() >= 8,
        "Expected at least 8 addresses (0..=7), got {}",
        account0.addresses.len()
    );

    assert_eq!(account0.addresses[0].address, "TYiKfTcdB3q9ZMRkoDM9qQ5CasvdBaoSdP");
    assert_eq!(account0.addresses[1].address, "TKzvw3u4SXzxfu69rVvNpjs5NiE5ZE4NJi");
    assert_eq!(account0.addresses[7].address, "TBic1drXQNM1BiBevg751GsZtv59GWb6ZK");

    for idx in [0usize, 1usize, 7usize] {
        TronAddress::from_base58(&account0.addresses[idx].address).expect("Invalid TRON Base58 address");
        assert!(
            account0.addresses[idx].balance.contains_key("TRX"),
            "Expected TRX balance entry for address index {}",
            idx
        );
        // TRC20 token balance should be present for each address
        assert!(
            account0.addresses[idx]
                .balance
                .contains_key(TRON_NILE_TRC20_USDT_TICKER),
            "Expected TRC20 {} balance entry for address index {}",
            TRON_NILE_TRC20_USDT_TICKER,
            idx
        );
    }

    let spendable0 = &account0.addresses[0].balance.get("TRX").unwrap().spendable;
    let spendable1 = &account0.addresses[1].balance.get("TRX").unwrap().spendable;
    let spendable7 = &account0.addresses[7].balance.get("TRX").unwrap().spendable;

    assert!(
        *spendable0 > 1900.into(),
        "Expected index 0 to have a large funded TRX balance, got {:?}",
        spendable0
    );
    assert!(
        *spendable1 > 15.into(),
        "Expected index 1 to have ~20 TRX funded balance, got {:?}",
        spendable1
    );
    assert!(
        *spendable7 > 3.into(),
        "Expected index 7 to have ~5 TRX funded balance, got {:?}",
        spendable7
    );

    block_on(mm.stop()).unwrap();
}

/// Test HD with account_id = 77 (mirrors ETH test pattern)
/// Also validates TRC20 token balance propagation and derivation paths.
#[test]
fn test_trx_hd_multiple_account_ids_account_77() {
    let coins = serde_json::json!([trx_conf(), trc20_usdt_nile_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let result = block_on(task_enable_trx_with_tokens(
        &mm,
        TRON_NILE_NODES,
        &[TRON_NILE_TRC20_USDT_TICKER],
        90,
        Some(HDAccountAddressId {
            account_id: 77,
            chain: Bip44Chain::External,
            address_id: 7,
        }),
    ))
    .expect("Expected TRX+TRC20 HD activation (account 77) to succeed");

    let hd = match result {
        EthWithTokensActivationResult::HD(hd) => hd,
        _ => panic!("Expected HD activation result"),
    };

    let balance = match hd.wallet_balance {
        EnableCoinBalanceMap::HD(hd_bal) => hd_bal,
        _ => panic!("Expected EnableCoinBalanceMap::HD"),
    };

    let account = balance.accounts.first().expect("Expected first HD account entry");
    assert_eq!(account.account_index, 77, "Expected account_index=77");
    assert_eq!(
        account.derivation_path, "m/44'/195'/77'",
        "Unexpected account derivation_path"
    );
    assert!(
        account.addresses.len() >= 8,
        "Expected at least 8 addresses (0..=7), got {}",
        account.addresses.len()
    );

    let addr7 = &account.addresses[7];
    assert_eq!(addr7.derivation_path, "m/44'/195'/77'/0/7");
    match addr7.chain {
        Bip44Chain::External => (),
        Bip44Chain::Internal => panic!("Expected External chain for account 77, index 7"),
    };
    TronAddress::from_base58(&addr7.address).expect("Invalid base58check TRON address for account 77, index 7");

    // Validate TRC20 token balance is present at address 7
    assert!(
        addr7.balance.contains_key(TRON_NILE_TRC20_USDT_TICKER),
        "Expected TRC20 {} balance entry for account 77, address index 7",
        TRON_NILE_TRC20_USDT_TICKER
    );

    block_on(mm.stop()).unwrap();
}

/// Test gap limit scanning - finds funded index 7 after unfunded gaps at 2..=6
#[test]
fn test_trx_hd_gap_limit_scanning_finds_index_7_after_unfunded_gaps() {
    let coins = serde_json::json!([trx_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let result = block_on(task_enable_trx(
        &mm,
        TRON_NILE_NODES,
        60,
        Some(HDAccountAddressId {
            account_id: 0,
            chain: Bip44Chain::External,
            address_id: 7,
        }),
    ))
    .expect("Expected TRX HD activation to succeed");

    let hd = match result {
        EthWithTokensActivationResult::HD(hd) => hd,
        _ => panic!("Expected HD activation result"),
    };

    let balance = match hd.wallet_balance {
        EnableCoinBalanceMap::HD(hd_bal) => hd_bal,
        _ => panic!("Expected EnableCoinBalanceMap::HD"),
    };

    let account0 = balance.accounts.first().expect("Expected first HD account entry");
    assert!(
        account0.addresses.len() >= 8,
        "Expected at least 8 addresses (0..=7), got {}",
        account0.addresses.len()
    );

    // Indices 2..=6 are expected to be unfunded
    for i in 2usize..=6usize {
        assert_eq!(
            account0.addresses[i].derivation_path,
            format!("m/44'/195'/0'/0/{}", i),
            "Unexpected derivation_path at index {}",
            i
        );
        if let Some(trx_balance) = account0.addresses[i].balance.get("TRX") {
            assert!(
                trx_balance.spendable < 1.into(),
                "Expected index {} to be unfunded (< 1 TRX), got {:?}",
                i,
                trx_balance.spendable
            );
        }
    }

    // Index 7 is funded
    assert_eq!(account0.addresses[7].address, "TBic1drXQNM1BiBevg751GsZtv59GWb6ZK");
    let spendable7 = &account0.addresses[7].balance.get("TRX").unwrap().spendable;
    assert!(
        *spendable7 > 3.into(),
        "Expected index 7 to be funded (~5 TRX), got {:?}",
        spendable7
    );

    block_on(mm.stop()).unwrap();
}

/// Test HD scanning detects addresses with transaction history but zero balance.
/// Uses TRON_USED_ZERO_BALANCE_PASSPHRASE:
/// - Index 0: funded (~1777.8 TRX)
/// - Index 2: has tx history but 0 balance (used but empty)
#[test]
fn test_trx_hd_scanning_detects_used_but_zero_balance_address() {
    let coins = serde_json::json!([trx_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(TRON_USED_ZERO_BALANCE_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let result = block_on(task_enable_trx(
        &mm,
        TRON_NILE_NODES,
        60,
        Some(HDAccountAddressId {
            account_id: 0,
            chain: Bip44Chain::External,
            address_id: 0,
        }),
    ))
    .expect("Expected TRX HD activation to succeed");

    let hd = match result {
        EthWithTokensActivationResult::HD(hd) => hd,
        _ => panic!("Expected HD activation result"),
    };

    let balance = match hd.wallet_balance {
        EnableCoinBalanceMap::HD(hd_bal) => hd_bal,
        _ => panic!("Expected EnableCoinBalanceMap::HD"),
    };

    let account0 = balance.accounts.first().expect("Expected first HD account entry");
    assert!(
        account0.addresses.len() >= 3,
        "Expected at least 3 addresses (0, 1, 2), got {}",
        account0.addresses.len()
    );

    // Index 0 should be funded
    assert_eq!(
        account0.addresses[0].address, "TSqB9tqfaQ1DYSdMCbVSLPzQsaNVjeu9hq",
        "Unexpected address at index 0"
    );
    let spendable0 = &account0.addresses[0].balance.get("TRX").unwrap().spendable;
    assert!(
        *spendable0 > 1700.into(),
        "Expected index 0 to have ~1777.8 TRX, got {:?}",
        spendable0
    );

    // Index 2 should be detected via gap limit scanning (has tx history) but have zero balance
    assert_eq!(
        account0.addresses[2].address, "TPoJwueR4xfZCXuQTYqem4edQgoM3uV78n",
        "Unexpected address at index 2"
    );

    // Verify index 2 has strictly zero balance
    if let Some(trx_balance) = account0.addresses[2].balance.get("TRX") {
        assert!(
            trx_balance.spendable == 0.into(),
            "Expected index 2 to have exactly 0 TRX, got {:?}",
            trx_balance.spendable
        );
    }

    block_on(mm.stop()).unwrap();
}

// =============================================================================
// TRC20 Token Tests
// =============================================================================

/// Test TRC20 activation via enable_erc20_token_v2 after TRX is already active.
#[test]
fn test_trc20_activation_after_platform() {
    TronAddress::from_base58(TRON_NILE_TRC20_USDT_CONTRACT).expect("Invalid TRC20 Base58 contract address constant");

    let coins = serde_json::json!([trx_conf(), trc20_usdt_nile_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    block_on(task_enable_trx(&mm, TRON_NILE_NODES, 90, None)).expect("Expected TRX activation to succeed");

    let token_activation = block_on(enable_erc20_token_v2(&mm, TRON_NILE_TRC20_USDT_TICKER, None, 90, None))
        .expect("Expected TRC20 token activation to succeed after TRX is active");

    assert_eq!(
        token_activation.platform_coin, "TRX",
        "Expected platform_coin to be TRX"
    );

    TronAddress::from_base58(&token_activation.token_contract_address)
        .expect("Invalid base58check TRC20 contract address returned in activation result");

    // Validate TRC20 token balance is present at specific address
    let balance = match token_activation.wallet_balance {
        EnableCoinBalanceMap::HD(hd_bal) => hd_bal,
        _ => panic!("Expected EnableCoinBalanceMap::HD"),
    };
    let account0 = balance.accounts.first().expect("Expected first HD account entry");
    assert!(
        account0.addresses[0].balance.contains_key(TRON_NILE_TRC20_USDT_TICKER),
        "Expected TRC20 {} balance entry for address index 0",
        TRON_NILE_TRC20_USDT_TICKER
    );

    block_on(mm.stop()).unwrap();
}

/// Test TRC20 HD activation with specific derivation path.
#[test]
fn test_trc20_hd_activation_with_path() {
    TronAddress::from_base58(TRON_NILE_TRC20_USDT_CONTRACT).expect("Invalid TRC20 Base58 contract address constant");

    let coins = serde_json::json!([trx_conf(), trc20_usdt_nile_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let path_to_address = HDAccountAddressId {
        account_id: 0,
        chain: Bip44Chain::External,
        address_id: 0,
    };

    block_on(task_enable_trx(&mm, TRON_NILE_NODES, 90, Some(path_to_address.clone())))
        .expect("Expected TRX HD activation to succeed");

    let token_activation = block_on(enable_erc20_token_v2(
        &mm,
        TRON_NILE_TRC20_USDT_TICKER,
        None,
        90,
        Some(path_to_address.clone()),
    ))
    .expect("Expected TRC20 token activation in HD mode to succeed");

    assert_eq!(
        token_activation.platform_coin, "TRX",
        "Expected platform_coin to be TRX"
    );

    TronAddress::from_base58(&token_activation.token_contract_address)
        .expect("Invalid base58check TRC20 contract address returned in activation result");

    // Validate TRC20 token balance and derivation path at specific address
    let balance = match token_activation.wallet_balance {
        EnableCoinBalanceMap::HD(hd_bal) => hd_bal,
        _ => panic!("Expected EnableCoinBalanceMap::HD"),
    };
    let account0 = balance.accounts.first().expect("Expected first HD account entry");
    assert!(
        account0.addresses[0].balance.contains_key(TRON_NILE_TRC20_USDT_TICKER),
        "Expected TRC20 {} balance entry for address index 0",
        TRON_NILE_TRC20_USDT_TICKER
    );
    assert_eq!(
        account0.addresses[0].derivation_path, "m/44'/195'/0'/0/0",
        "Unexpected derivation path for address index 0"
    );

    block_on(mm.stop()).unwrap();
}

/// Test TRC20-only address detection during HD gap scanning.
///
/// Index 10 holds only TRC20 USDT (no TRX) and sits beyond the last TRX-funded address (index 7).
/// If `is_address_used()` didn't check TRC20 balances, the scanner would treat index 10 as empty.
///
/// Funding setup (BOB_HD_PASSPHRASE on Nile testnet):
/// - Index 0, 1, 7: TRX (native balance)
/// - Index 10: TRC20 USDT only (5 USDT, no TRX)
/// - Index 8, 9: Nothing (gap addresses)
#[test]
fn test_trc20_hd_gap_scanning() {
    let coins = serde_json::json!([trx_conf(), trc20_usdt_nile_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    // Activate at index 0 and let gap scanning (limit=20) discover all used addresses.
    // The scanner walks forward from 0; after the last TRX address (7), it continues for
    // up to 20 consecutive unused addresses. Index 10 falls within that window.
    let result = block_on(task_enable_trx_with_tokens(
        &mm,
        TRON_NILE_NODES,
        &[TRON_NILE_TRC20_USDT_TICKER],
        120,
        Some(HDAccountAddressId {
            account_id: 0,
            chain: Bip44Chain::External,
            address_id: 0,
        }),
    ))
    .expect("Expected TRX+TRC20 HD activation to succeed");

    let hd = match result {
        EthWithTokensActivationResult::HD(hd) => hd,
        _ => panic!("Expected HD activation result"),
    };

    let balance = match hd.wallet_balance {
        EnableCoinBalanceMap::HD(hd_bal) => hd_bal,
        _ => panic!("Expected EnableCoinBalanceMap::HD"),
    };

    let account0 = balance.accounts.first().expect("Expected first HD account entry");
    assert!(
        account0.addresses.len() >= 11,
        "Expected at least 11 addresses (0..=10), got {}",
        account0.addresses.len()
    );

    // Verify TRX-funded addresses (0, 1, 7) have balances
    for idx in [0usize, 1usize, 7usize] {
        assert!(
            account0.addresses[idx].balance.contains_key("TRX"),
            "Expected TRX balance entry for address index {}",
            idx
        );
    }

    // KEY TEST: Verify address index 10 is discovered via TRC20 activity alone.
    // This is BEYOND the last TRX address (7), proving TRC20 detection works.
    let addr10 = &account0.addresses[10];
    assert_eq!(
        addr10.address, BOB_HD_TRC20_ONLY_ADDRESS_INDEX_10,
        "Address at index 10 should match BOB_HD_TRC20_ONLY_ADDRESS_INDEX_10"
    );

    // Verify TRC20 balance is present AND non-zero (proves detection via TRC20)
    let trc20_balance = addr10
        .balance
        .get(TRON_NILE_TRC20_USDT_TICKER)
        .expect("Expected TRC20 balance entry for TRC20-only address at index 10");
    assert!(
        trc20_balance.spendable > 0.into(),
        "TRC20 balance at index 10 should be non-zero (proves TRC20 detection), got: {}",
        trc20_balance.spendable
    );

    // TRC20-only address should have zero TRX balance
    let trx_balance = addr10
        .balance
        .get("TRX")
        .expect("Expected TRX balance entry for address at index 10");
    assert_eq!(
        trx_balance.spendable,
        0.into(),
        "TRC20-only address at index 10 should have zero TRX balance"
    );

    // Verify indices 8-9 are true gap addresses (empty balance maps).
    // This contrasts with index 10 which has balances, proving TRC20 detection.
    for idx in 8usize..=9usize {
        assert!(
            account0.addresses[idx].balance.is_empty(),
            "Gap address index {} should have empty balance (unlike TRC20-detected index 10), got keys: {:?}",
            idx,
            account0.addresses[idx].balance.keys().collect::<Vec<_>>()
        );
    }

    block_on(mm.stop()).unwrap();
}
