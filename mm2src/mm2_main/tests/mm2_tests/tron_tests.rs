//! TRON activation integration tests
//!
//! Run with: cargo test --test mm2_tests_main --features tron-network-tests tron_

use coins::eth::tron::TronAddress;
use common::block_on;
use mm2_test_helpers::for_tests::{
    account_balance, enable_trx, get_new_address, task_enable_trx, trx_conf, MarketMakerIt, Mm2TestConf,
    Mm2TestConfForSwap, TRON_NILE_NODES,
};
use mm2_test_helpers::structs::{Bip44Chain, EnableCoinBalanceMap, EthWithTokensActivationResult, HDAccountAddressId};

/// Test mnemonic for used-but-zero-balance scenario.
/// Index 0: TSqB9tqfaQ1DYSdMCbVSLPzQsaNVjeu9hq (funded ~1777.8 TRX)
/// Index 2: TPoJwueR4xfZCXuQTYqem4edQgoM3uV78n (0 balance but has tx history)
const TRON_USED_ZERO_BALANCE_PASSPHRASE: &str =
    "top wonder island doctor gesture velvet local media begin impose soccer radar";

/// Test TRX activation works via enable_eth_with_tokens (immediate mode).
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_trx_activation_immediate() {
    let coins = serde_json::json!([trx_conf()]);
    let conf = Mm2TestConf::seednode(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let result = block_on(enable_trx(&mm, TRON_NILE_NODES));

    assert!(result.get("result").is_some(), "Expected result field in response");
    let result_inner = &result["result"];

    let current_block = result_inner["current_block"]
        .as_u64()
        .expect("current_block should be u64");
    assert!(current_block > 0, "current_block should be greater than 0");

    let eth_addresses_infos = result_inner["eth_addresses_infos"]
        .as_object()
        .expect("eth_addresses_infos should be an object");
    assert!(!eth_addresses_infos.is_empty(), "Should have at least one address");

    for (address, _info) in eth_addresses_infos {
        assert!(
            address.starts_with('T'),
            "TRON address should start with 'T', got: {}",
            address
        );
    }

    block_on(mm.stop()).unwrap();
}

/// Test TRX activation works via task::enable_eth::init (task-based mode).
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_trx_activation_task_based() {
    let coins = serde_json::json!([trx_conf()]);
    let conf = Mm2TestConf::seednode(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let result =
        block_on(task_enable_trx(&mm, TRON_NILE_NODES, 60, None)).expect("TRX task-based activation should succeed");

    match result {
        EthWithTokensActivationResult::Iguana(iguana_result) => {
            assert!(
                iguana_result.current_block > 0,
                "current_block should be greater than 0"
            );
            assert!(
                !iguana_result.eth_addresses_infos.is_empty(),
                "Should have at least one address"
            );
            for address in iguana_result.eth_addresses_infos.keys() {
                assert!(
                    address.starts_with('T'),
                    "TRON address should start with 'T', got: {}",
                    address
                );
            }
        },
        EthWithTokensActivationResult::HD(hd_result) => {
            assert!(hd_result.current_block > 0, "current_block should be greater than 0");
            assert_eq!(hd_result.ticker, "TRX", "Ticker should be TRX");
        },
    }

    block_on(mm.stop()).unwrap();
}

/// Test node failover: dead node first, good node second = success
#[test]
#[cfg(not(target_arch = "wasm32"))]
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
                assert!(addr.starts_with('T'), "Expected base58 TRON address, got {}", addr);
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
#[cfg(not(target_arch = "wasm32"))]
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

    assert!(
        addr0.starts_with('T'),
        "TRON address should start with 'T', got: {}",
        addr0
    );
    TronAddress::from_base58(addr0).expect("Invalid base58check TRON address");

    block_on(mm.stop()).unwrap();
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_trx_get_new_address_rpc_hd() {
    let coins = serde_json::json!([trx_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let _activation = block_on(task_enable_trx(
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

    let addr1 = block_on(get_new_address(&mm, "TRX", 0, Some(Bip44Chain::External)));
    assert!(addr1.new_address.address.starts_with('T'));
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

    let bal = block_on(account_balance(&mm, "TRX", 0, Bip44Chain::External));
    let found = bal.addresses.iter().any(|a| a.address == addr1.new_address.address);
    assert!(
        found,
        "Expected get_new_address(TRX) address to be present in account_balance addresses list"
    );

    let addr2 = block_on(get_new_address(&mm, "TRX", 0, Some(Bip44Chain::External)));
    assert_ne!(addr1.new_address.address, addr2.new_address.address);

    block_on(mm.stop()).unwrap();
}

/// Test HD balance structure with funded addresses (BOB_HD_PASSPHRASE)
/// Funding: index 0 (~1967 TRX), index 1 (20 TRX), index 7 (5 TRX)
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_trx_hd_balance_structure_assertions_and_funded_amounts() {
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
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_trx_hd_multiple_account_ids_account_77() {
    let coins = serde_json::json!([trx_conf()]);
    let conf = Mm2TestConf::seednode_with_hd_account(Mm2TestConfForSwap::BOB_HD_PASSPHRASE, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    let result = block_on(task_enable_trx(
        &mm,
        TRON_NILE_NODES,
        60,
        Some(HDAccountAddressId {
            account_id: 77,
            chain: Bip44Chain::External,
            address_id: 7,
        }),
    ))
    .expect("Expected TRX HD activation (account 77) to succeed");

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
    assert!(addr7.address.starts_with('T'));
    TronAddress::from_base58(&addr7.address).expect("Invalid base58check TRON address for account 77, index 7");

    block_on(mm.stop()).unwrap();
}

/// Test gap limit scanning - finds funded index 7 after unfunded gaps at 2..=6
#[test]
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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
            address_id: 2,
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
    TronAddress::from_base58(&account0.addresses[0].address).expect("Invalid TRON address at index 0");
    let spendable0 = &account0.addresses[0].balance.get("TRX").unwrap().spendable;
    assert!(
        *spendable0 > 1700.into(),
        "Expected index 0 to have ~1777.8 TRX, got {:?}",
        spendable0
    );

    // Index 2 should be detected (has tx history) but have zero balance
    assert_eq!(
        account0.addresses[2].address, "TPoJwueR4xfZCXuQTYqem4edQgoM3uV78n",
        "Unexpected address at index 2"
    );
    TronAddress::from_base58(&account0.addresses[2].address).expect("Invalid TRON address at index 2");

    // Verify index 2 has zero or negligible balance
    if let Some(trx_balance) = account0.addresses[2].balance.get("TRX") {
        assert!(
            trx_balance.spendable < 1.into(),
            "Expected index 2 to have ~0 TRX (used but empty), got {:?}",
            trx_balance.spendable
        );
    }

    block_on(mm.stop()).unwrap();
}
