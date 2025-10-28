use super::utils::*;

use coins::siacoin::ApiClientHelpers;
use mm2_test_helpers::electrums::doc_electrums;
use mm2_test_helpers::for_tests::{enable_utxo_v2_electrum, start_swaps, wait_for_swap_finished_or_err};

use std::str::FromStr;

// FIXME Alright - WIP stub to demonstrate used of a shared DSIA container amongst multiple tokio tests.
#[tokio::test]
#[ignore]
async fn test_shared_dsia_container_wip() {
    let container = init_global_walletd_container().await;
    let sia_client = &container.client;
    println!(
        "first test height before : {}",
        sia_client.current_height().await.unwrap()
    );

    fund_address(sia_client, &ALICE_SIA_ADDRESS, Currency::COIN * 10).await;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        let bal_resp = sia_client.address_balance(ALICE_SIA_ADDRESS.clone()).await.unwrap();
        println!("first test balance: {:?}", bal_resp);
    }
}

/// Not a real test, useful to start a DSIA container with identical parameters as the one used in
/// other tests.
/// Starts a new DSIA container and prints the port walletd is bound to on the host.
///
/// The container must be manually stopped.
#[tokio::test]
async fn debug_init_walletd_container() {
    use std::mem;
    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let dsia = init_walletd_container(&temp_dir).await;
    println!("DSIA host port: {}", dsia.host_port);

    let address =
        Address::from_str("591fcf237f8854b5653d1ac84ae4c107b37f148c3c7b413f292d48db0c25a8840be0653e411f").unwrap();

    let response = dsia.client.address_balance(address.clone()).await.unwrap();
    println!("Address balance: {:?}", response);

    dsia.client.mine_blocks(1, &address).await.unwrap();

    let response = dsia.client.address_balance(address).await.unwrap();
    println!("Address balance: {:?}", response);
    assert_eq!(response.immature_siacoins, Currency(299999000000000000000000000000));

    // Prevent automatic testcontainers cleanup
    mem::forget(dsia);
}

/// Not a real test, useful to start a ZEN testnet container
/// Starts a new walletd container and prints the port walletd is bound to on the host.
///
/// The container must be manually stopped.
/// This is for debugging purposes.
/// This creates a public API endpoint with the password "password".
#[tokio::test]
async fn debug_init_zen_container() {
    use std::mem;
    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let dsia = init_zen_container(&temp_dir).await;
    println!("ZEN host port: {}", dsia.host_port);

    // Prevent automatic testcontainers cleanup
    mem::forget(dsia);
}

/// Initialize Alice KDF instance
#[tokio::test]
async fn test_init_alice() {
    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let netid = get_unique_netid();
    let (_, _) = init_alice(&temp_dir, netid, None).await;
}

/// Initialize Bob KDF instance
#[tokio::test]
async fn test_init_bob() {
    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let netid = get_unique_netid();
    let (_, _) = init_bob(&temp_dir, netid, None).await;
}

/// Initialize Alice and Bob, check that they connected via p2p network
#[tokio::test]
#[ignore]
async fn test_init_alice_and_bob() {
    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let netid = get_unique_netid();

    // initialize Bob first because he acts as a seed node
    let (_ctx_bob, mm_bob) = init_bob(&temp_dir, netid, None).await;
    let (_ctx_alice, mm_alice) = init_alice(&temp_dir, netid, None).await;

    wait_for_peers_connected(&mm_alice, &mm_bob, std::time::Duration::from_secs(30))
        .await
        .unwrap();
}

/// Initialize Alice and Bob, initialize Sia testnet container, enable DSIA for both parties
#[tokio::test]
#[ignore]
async fn test_alice_and_bob_enable_dsia() {
    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let dsia = init_walletd_container(&temp_dir).await;
    let netid = get_unique_netid();

    let (_ctx_bob, mm_bob) = init_bob(&temp_dir, netid, None).await;
    let (_ctx_alice, mm_alice) = init_alice(&temp_dir, netid, None).await;

    let _bob_enable_sia_resp = enable_dsia(&mm_alice, dsia.host_port).await;
    let _alice_enable_sia_resp = enable_dsia(&mm_bob, dsia.host_port).await;
}

/// Initialize Komodods container, initialize KomododClient for Alice and Bob
/// Validate Alice and Bob's addresses were imported via `importaddress`
#[tokio::test]
#[ignore]
async fn test_init_utxo_container_and_client() {
    let (_container, (alice_client, bob_client)) = init_komodod_clients(ALICE_KMD_KEY, BOB_KMD_KEY).await;

    let alice_validate_address_resp = alice_client
        .rpc("validateaddress", json!([ALICE_KMD_KEY.address]))
        .await;
    let bob_validate_address_resp = bob_client.rpc("validateaddress", json!([BOB_KMD_KEY.address])).await;

    assert_eq!(alice_validate_address_resp["result"]["iswatchonly"], true);
    assert_eq!(bob_validate_address_resp["result"]["iswatchonly"], true);
}

/// Initialize Alice and Bob, initialize Sia testnet container
/// Bob sells DOC for Alice's DSIA
/// Will fail if Bob is not prefunded with DOC
#[tokio::test]
#[ignore]
async fn test_bob_sells_doc_for_dsia() {
    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let netid = get_unique_netid();

    // Start the Sia container
    let dsia = init_walletd_container(&temp_dir).await;

    // Mine blocks to give Alice some funds. Coinbase maturity requires >150 confirmations.
    dsia.client.mine_blocks(155, &ALICE_SIA_ADDRESS).await.unwrap();

    // Initalize Alice and Bob KDF instances
    let (_ctx_bob, mut mm_bob) = init_bob(&temp_dir, netid, None).await;
    let (_ctx_alice, mut mm_alice) = init_alice(&temp_dir, netid, None).await;

    // Enable DOC coin via electrum for Alice and Bob
    let _ = enable_utxo_v2_electrum(&mm_bob, "DOC", doc_electrums(), None, 60, None).await;
    let _ = enable_utxo_v2_electrum(&mm_alice, "DOC", doc_electrums(), None, 60, None).await;

    // Enable DSIA coin for Alice and Bob
    let _ = enable_dsia(&mm_bob, dsia.host_port).await;
    let _ = enable_dsia(&mm_alice, dsia.host_port).await;

    // Wait for Alice and Bob KDF instances to peer
    wait_for_peers_connected(&mm_bob, &mm_alice, std::time::Duration::from_secs(30))
        .await
        .unwrap();

    // Start a swap where Bob sells DOC for Alice's DSIA
    let uuid = start_swaps(&mut mm_bob, &mut mm_alice, &[("DOC", "DSIA")], 1., 1., 0.05)
        .await
        .first()
        .cloned()
        .unwrap();

    // Mine a block every 10 seconds to progress DSIA chain
    tokio::spawn(async move {
        loop {
            dsia.client.mine_blocks(1, &CHARLIE_SIA_ADDRESS).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });

    // Wait for the swap to complete
    wait_for_swap_finished_or_err(&mm_alice, &uuid, 600).await.unwrap();
    wait_for_swap_finished_or_err(&mm_bob, &uuid, 120).await.unwrap();
}

/// Initialize Alice and Bob, initialize Sia testnet container
/// Bob sells DSIA for Alice's DOC
/// Will fail if Alice is not prefunded with DOC
#[tokio::test]
#[ignore]
async fn test_bob_sells_dsia_for_doc() {
    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let netid = get_unique_netid();

    // Start the Sia container
    let dsia = init_walletd_container(&temp_dir).await;

    // Mine blocks to give Bob some funds. Coinbase maturity requires >150 confirmations.
    dsia.client.mine_blocks(155, &BOB_SIA_ADDRESS).await.unwrap();

    // Initalize Alice and Bob KDF instances
    let (_ctx_bob, mut mm_bob) = init_bob(&temp_dir, netid, None).await;
    let (_ctx_alice, mut mm_alice) = init_alice(&temp_dir, netid, None).await;

    // Enable DOC coin via electrum for Alice and Bob
    let _ = enable_utxo_v2_electrum(&mm_bob, "DOC", doc_electrums(), None, 60, None).await;
    let _ = enable_utxo_v2_electrum(&mm_alice, "DOC", doc_electrums(), None, 60, None).await;

    // Enable DSIA coin for Alice and Bob
    let _ = enable_dsia(&mm_bob, dsia.host_port).await;
    let _ = enable_dsia(&mm_alice, dsia.host_port).await;

    // Wait for Alice and Bob KDF instances to peer
    wait_for_peers_connected(&mm_bob, &mm_alice, std::time::Duration::from_secs(30))
        .await
        .unwrap();

    // Start a swap where Bob sells DSIA for Alice's DOC
    let uuid = start_swaps(&mut mm_bob, &mut mm_alice, &[("DSIA", "DOC")], 1., 1., 0.05)
        .await
        .first()
        .cloned()
        .unwrap();

    // Mine a block every 10 seconds to progress DSIA chain
    tokio::spawn(async move {
        loop {
            dsia.client.mine_blocks(1, &CHARLIE_SIA_ADDRESS).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });

    // Wait for the swap to complete
    wait_for_swap_finished_or_err(&mm_alice, &uuid, 600).await.unwrap();
    wait_for_swap_finished_or_err(&mm_bob, &uuid, 120).await.unwrap();
}

/// Initialize Alice and Bob, initialize Sia testnet container, initialize UTXO testnet container,
/// Bob sells DSIA for Alice's DUTXO
#[tokio::test]
#[ignore]
async fn test_bob_sells_dsia_for_dutxo() {
    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let netid = get_unique_netid();

    // Start the Utxo nodes container with Alice as miner
    let (utxo_container, (alice_client, bob_client)) = init_komodod_clients(ALICE_KMD_KEY, BOB_KMD_KEY).await;

    // temporarily capture the utxo container stdout and pipe it to the test stdout - TODO debugging
    let stdout = utxo_container.stdout(true);
    tokio::spawn(async move {
        pipe_buf_to_stdout(stdout).await;
    });

    // Start the Sia container and mine 155 blocks to Bob
    let dsia = init_walletd_container(&temp_dir).await;
    dsia.client.mine_blocks(155, &BOB_SIA_ADDRESS).await.unwrap();

    // Initalize Alice and Bob KDF instances
    let (_ctx_bob, mut mm_bob) = init_bob(&temp_dir, netid, Some(bob_client.conf.port)).await;
    let (_ctx_alice, mut mm_alice) = init_alice(&temp_dir, netid, Some(alice_client.conf.port)).await;

    // Enable DSIA coin for Alice and Bob
    let _ = enable_dsia(&mm_bob, dsia.host_port).await;
    let _ = enable_dsia(&mm_alice, dsia.host_port).await;

    // Enable DUTXO coin via Native node for Alice and Bob
    let _ = enable_dutxo(&mm_alice).await;
    let _ = enable_dutxo(&mm_bob).await;

    // Wait for Alice and Bob KDF instances to connect
    wait_for_peers_connected(&mm_alice, &mm_bob, std::time::Duration::from_secs(30))
        .await
        .unwrap();

    // Start a swap where Bob sells DSIA for Alice's DUTXO
    let uuid = start_swaps(&mut mm_bob, &mut mm_alice, &[("DSIA", "DUTXO")], 1., 1., 0.05)
        .await
        .first()
        .cloned()
        .unwrap();

    // Mine a block every 10 seconds to progress DSIA chain
    tokio::spawn(async move {
        loop {
            dsia.client.mine_blocks(1, &CHARLIE_SIA_ADDRESS).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });

    // Wait for the swap to complete
    wait_for_swap_finished_or_err(&mm_alice, &uuid, 360).await.unwrap();
    wait_for_swap_finished_or_err(&mm_bob, &uuid, 60).await.unwrap();
}

/// Initialize Alice and Bob, initialize Sia testnet container, initialize UTXO testnet container,
/// Bob sells DUTXO for Alice's DSIA
#[tokio::test]
#[ignore]
async fn test_bob_sells_dutxo_for_dsia() {
    let temp_dir = init_test_dir(current_function_name!(), true).await;

    let netid = get_unique_netid();

    // Start the Utxo nodes container with Bob as funded key
    let (_utxo_container, (bob_komodod_client, alice_komodod_client)) =
        init_komodod_clients(BOB_KMD_KEY, ALICE_KMD_KEY).await;

    // Start the Sia container and mine 155 blocks to Alice
    let dsia = init_walletd_container(&temp_dir).await;
    dsia.client.mine_blocks(155, &ALICE_SIA_ADDRESS).await.unwrap();

    // Initalize Alice and Bob KDF instances
    let (_ctx_bob, mut mm_bob) = init_bob(&temp_dir, netid, Some(bob_komodod_client.conf.port)).await;
    let (_ctx_alice, mut mm_alice) = init_alice(&temp_dir, netid, Some(alice_komodod_client.conf.port)).await;

    // Enable DSIA coin for Alice and Bob
    let _ = enable_dsia(&mm_bob, dsia.host_port).await;
    let _ = enable_dsia(&mm_alice, dsia.host_port).await;

    // Enable DUTXO coin via Native node for Alice and Bob
    let _ = enable_dutxo(&mm_alice).await;
    let _ = enable_dutxo(&mm_bob).await;

    // Wait for Alice and Bob KDF instances to connect
    wait_for_peers_connected(&mm_alice, &mm_bob, std::time::Duration::from_secs(30))
        .await
        .unwrap();

    // Start a swap where Bob sells DUTXO for Alice's DSIA
    let uuid = start_swaps(&mut mm_bob, &mut mm_alice, &[("DUTXO", "DSIA")], 1., 1., 0.05)
        .await
        .first()
        .cloned()
        .unwrap();

    // Mine a block every 10 seconds to progress DSIA chain
    tokio::spawn(async move {
        loop {
            dsia.client.mine_blocks(1, &CHARLIE_SIA_ADDRESS).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });

    // Wait for the swap to complete
    wait_for_swap_finished_or_err(&mm_alice, &uuid, 600).await.unwrap();
    wait_for_swap_finished_or_err(&mm_bob, &uuid, 60).await.unwrap();
}

/*
// WIP the following tests are "functional tests" and lie somewhere between a unit test and integration test
// All are disabled for now until this sia_tests module can be better organized.
// These were written as SiaCoin implementation was being developed and are not currently maintained

use crate::lp_swap::SecretHashAlgo;
use crate::lp_wallet::initialize_wallet_passphrase;

use coins::siacoin::{ApiClientHelpers, SiaCoin, SiaCoinActivationRequest};
use coins::Transaction;

use common::now_sec;

use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_number::BigDecimal;
use coins::{PrivKeyBuildPolicy, RefundPaymentArgs, SendPaymentArgs, SpendPaymentArgs, SwapOps,
            SwapTxTypeWithSecretHash, TransactionEnum};
fn helper_activation_request(port: u16) -> SiaCoinActivationRequest {
    let activation_request_json = json!(
        {
            "tx_history": true,
            "client_conf": {
                "server_url": format!("http://localhost:{}/", port),
                "password": "password"
            }
        }
    );
    serde_json::from_value::<SiaCoinActivationRequest>(activation_request_json).unwrap()
}

/// Initialize a minimal MarketMaker intended for unit testing.
/// See `init_bob` or `init_alice` for creating "full" MarketMaker instances.
async fn init_ctx(passphrase: &str, netid: u16) -> MmArc {
    let kdf_conf = json!({
        "gui": "sia-docker-tests",
        "netid": netid,
        "rpc_password": "rpc_password",
        "passphrase": passphrase,
    });

    let ctx = MmCtxBuilder::new().with_conf(kdf_conf).into_mm_arc();

    initialize_wallet_passphrase(&ctx).await.unwrap();
    ctx
}

async fn init_siacoin(ctx: MmArc, ticker: &str, request: &SiaCoinActivationRequest) -> SiaCoin {
    let coin_conf_str = json!(
        {
            "coin": ticker,
            "required_confirmations": 1,
        }
    );

    let priv_key_policy = PrivKeyBuildPolicy::detect_priv_key_policy(&ctx).unwrap();
    SiaCoin::new(&ctx, coin_conf_str, request, priv_key_policy)
        .await
        .unwrap()
}

/**
 * Initialize ctx and SiaCoin for both parties, maker and taker
 * Initialize a new SiaCoin testnet and mine blocks to maker for funding
 * Send a HTLC payment from maker
 * Spend the HTLC payment from taker
 *
 * maker_* indicates data created by the maker
 * taker_* indicates data created by the taker
 * negotiated_* indicates data that is negotiated via p2p communication
 */
#[tokio::test]
async fn test_send_maker_payment_then_spend_maker_payment() {
    let docker = Cli::default();

    // Start the container
    let (_container, host_port) = init_walletd_container(&docker);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let maker_ctx = init_ctx("maker passphrase", 9995).await;
    let maker_sia_coin = init_siacoin(maker_ctx, "TSIA", &helper_activation_request(host_port)).await;
    let maker_public_key = maker_sia_coin.my_keypair().unwrap().public();
    let maker_address = maker_public_key.address();
    let maker_secret = vec![0u8; 32];
    let maker_secret_hash = SecretHashAlgo::SHA256.hash_secret(&maker_secret);
    maker_sia_coin.client.mine_blocks(201, &maker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let taker_ctx = init_ctx("taker passphrase", 9995).await;
    let taker_sia_coin = init_siacoin(taker_ctx, "TSIA", &helper_activation_request(host_port)).await;
    let taker_public_key = taker_sia_coin.my_keypair().unwrap().public();

    let negotiated_time_lock = now_sec();
    let negotiated_time_lock_duration = 10u64;
    let negotiated_amount: BigDecimal = 1u64.into();

    let maker_send_payment_args = SendPaymentArgs {
        time_lock_duration: negotiated_time_lock_duration,
        time_lock: negotiated_time_lock,
        other_pubkey: taker_public_key.as_bytes(),
        secret_hash: &maker_secret_hash,
        amount: negotiated_amount,
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let maker_payment_tx = match maker_sia_coin
        .send_maker_payment(maker_send_payment_args)
        .await
        .unwrap()
    {
        TransactionEnum::SiaTransaction(tx) => tx,
        _ => panic!("Expected SiaTransaction"),
    };
    maker_sia_coin.client.mine_blocks(1, &maker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let taker_spend_payment_args = SpendPaymentArgs {
        other_payment_tx: &maker_payment_tx.tx_hex(),
        time_lock: negotiated_time_lock,
        other_pubkey: maker_public_key.as_bytes(),
        secret: &maker_secret,
        secret_hash: &maker_secret_hash,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };

    let taker_spends_maker_payment_tx = match taker_sia_coin
        .send_taker_spends_maker_payment(taker_spend_payment_args)
        .await
        .unwrap()
    {
        TransactionEnum::SiaTransaction(tx) => tx,
        _ => panic!("Expected SiaTransaction"),
    };
    maker_sia_coin.client.mine_blocks(1, &maker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let event = maker_sia_coin
        .client
        .get_event(&taker_spends_maker_payment_tx.txid())
        .await
        .unwrap();
    assert_eq!(event.confirmations, 1u64);
}

/**
 * Initialize ctx and SiaCoin for both parties, maker and taker
 * Initialize a new SiaCoin testnet and mine blocks to taker for funding
 * Send a HTLC payment from taker
 * Spend the HTLC payment from maker
 */
#[tokio::test]
async fn test_send_taker_payment_then_spend_taker_payment() {
    let docker = Cli::default();

    // Start the container
    let (_container, host_port) = init_walletd_container(&docker);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let taker_ctx = init_ctx("taker passphrase", 9995).await;
    let taker_sia_coin = init_siacoin(taker_ctx, "TSIA", &helper_activation_request(host_port)).await;
    let taker_public_key = taker_sia_coin.my_keypair().unwrap().public();
    let taker_address = taker_public_key.address();
    taker_sia_coin.client.mine_blocks(201, &taker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let maker_ctx = init_ctx("maker passphrase", 9995).await;
    let maker_sia_coin = init_siacoin(maker_ctx, "TSIA", &helper_activation_request(host_port)).await;
    let maker_public_key = maker_sia_coin.my_keypair().unwrap().public();
    let maker_secret = vec![0u8; 32];
    let maker_secret_hash = SecretHashAlgo::SHA256.hash_secret(&maker_secret);

    let negotiated_time_lock = now_sec();
    let negotiated_time_lock_duration = 10u64;
    let negotiated_amount: BigDecimal = 1u64.into();

    let taker_send_payment_args = SendPaymentArgs {
        time_lock_duration: negotiated_time_lock_duration,
        time_lock: negotiated_time_lock,
        other_pubkey: maker_public_key.as_bytes(),
        secret_hash: &maker_secret_hash,
        amount: negotiated_amount,
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let taker_payment_tx = match taker_sia_coin
        .send_taker_payment(taker_send_payment_args)
        .await
        .unwrap()
    {
        TransactionEnum::SiaTransaction(tx) => tx,
        _ => panic!("Expected SiaTransaction"),
    };
    taker_sia_coin.client.mine_blocks(1, &taker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let maker_spend_payment_args = SpendPaymentArgs {
        other_payment_tx: &taker_payment_tx.tx_hex(),
        time_lock: negotiated_time_lock,
        other_pubkey: taker_public_key.as_bytes(),
        secret: &maker_secret,
        secret_hash: &maker_secret_hash,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };

    let maker_spends_taker_payment_tx = match maker_sia_coin
        .send_maker_spends_taker_payment(maker_spend_payment_args)
        .await
        .unwrap()
    {
        TransactionEnum::SiaTransaction(tx) => tx,
        _ => panic!("Expected SiaTransaction"),
    };
    taker_sia_coin.client.mine_blocks(1, &taker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    taker_sia_coin
        .client
        .get_transaction(&maker_spends_taker_payment_tx.txid())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_send_maker_payment_then_refund_maker_payment() {
    let docker = Cli::default();

    // Start the container
    let (_container, host_port) = init_walletd_container(&docker);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let maker_ctx = init_ctx("maker passphrase", 9995).await;
    let maker_sia_coin = init_siacoin(maker_ctx, "TSIA", &helper_activation_request(host_port)).await;
    let maker_public_key = maker_sia_coin.my_keypair().unwrap().public();
    let maker_address = maker_public_key.address();
    let maker_secret = vec![0u8; 32];
    let maker_secret_hash = SecretHashAlgo::SHA256.hash_secret(&maker_secret);
    maker_sia_coin.client.mine_blocks(201, &maker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let taker_ctx = init_ctx("taker passphrase", 9995).await;
    let taker_sia_coin = init_siacoin(taker_ctx, "TSIA", &helper_activation_request(host_port)).await;
    let taker_public_key = taker_sia_coin.my_keypair().unwrap().public();

    // time lock is set in the past to allow immediate refund
    let negotiated_time_lock = now_sec() - 1000;
    let negotiated_time_lock_duration = 10u64;
    let negotiated_amount: BigDecimal = 1u64.into();

    let maker_send_payment_args = SendPaymentArgs {
        time_lock_duration: negotiated_time_lock_duration,
        time_lock: negotiated_time_lock,
        other_pubkey: taker_public_key.as_bytes(),
        secret_hash: &maker_secret_hash,
        amount: negotiated_amount,
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let maker_payment_tx = match maker_sia_coin
        .send_maker_payment(maker_send_payment_args)
        .await
        .unwrap()
    {
        TransactionEnum::SiaTransaction(tx) => tx,
        _ => panic!("Expected SiaTransaction"),
    };
    maker_sia_coin.client.mine_blocks(1, &maker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let secret_hash_type = SwapTxTypeWithSecretHash::TakerOrMakerPayment {
        maker_secret_hash: &maker_secret_hash,
    };
    let maker_refunds_payment_args = RefundPaymentArgs {
        payment_tx: &maker_payment_tx.tx_hex(),
        time_lock: negotiated_time_lock,
        other_pubkey: taker_public_key.as_bytes(),
        tx_type_with_secret_hash: secret_hash_type,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };

    let maker_refunds_maker_payment_tx = match maker_sia_coin
        .send_maker_refunds_payment(maker_refunds_payment_args)
        .await
        .unwrap()
    {
        TransactionEnum::SiaTransaction(tx) => tx,
        _ => panic!("Expected SiaTransaction"),
    };
    maker_sia_coin.client.mine_blocks(1, &maker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    maker_sia_coin
        .client
        .get_transaction(&maker_refunds_maker_payment_tx.txid())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_send_taker_payment_then_refund_taker_payment() {
    let docker = Cli::default();

    // Start the container
    let (_container, host_port) = init_walletd_container(&docker);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let maker_ctx = init_ctx("maker passphrase", 9995).await;
    let maker_sia_coin = init_siacoin(maker_ctx, "TSIA", &helper_activation_request(host_port)).await;
    let maker_public_key = maker_sia_coin.my_keypair().unwrap().public();
    let maker_secret = vec![0u8; 32];
    let maker_secret_hash = SecretHashAlgo::SHA256.hash_secret(&maker_secret);

    let taker_ctx = init_ctx("taker passphrase", 9995).await;
    let taker_sia_coin = init_siacoin(taker_ctx, "TSIA", &helper_activation_request(host_port)).await;
    let taker_public_key = taker_sia_coin.my_keypair().unwrap().public();
    let taker_address = taker_public_key.address();
    taker_sia_coin.client.mine_blocks(201, &taker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // time lock is set in the past to allow immediate refund
    let negotiated_time_lock = now_sec() - 1000;
    let negotiated_time_lock_duration = 10u64;
    let negotiated_amount: BigDecimal = 1u64.into();

    let taker_send_payment_args = SendPaymentArgs {
        time_lock_duration: negotiated_time_lock_duration,
        time_lock: negotiated_time_lock,
        other_pubkey: maker_public_key.as_bytes(),
        secret_hash: &maker_secret_hash,
        amount: negotiated_amount,
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let taker_payment_tx = match taker_sia_coin
        .send_maker_payment(taker_send_payment_args)
        .await
        .unwrap()
    {
        TransactionEnum::SiaTransaction(tx) => tx,
        _ => panic!("Expected SiaTransaction"),
    };
    taker_sia_coin.client.mine_blocks(1, &taker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let secret_hash_type = SwapTxTypeWithSecretHash::TakerOrMakerPayment {
        maker_secret_hash: &maker_secret_hash,
    };
    let taker_refunds_payment_args = RefundPaymentArgs {
        payment_tx: &taker_payment_tx.tx_hex(),
        time_lock: negotiated_time_lock,
        other_pubkey: maker_public_key.as_bytes(),
        tx_type_with_secret_hash: secret_hash_type,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };

    let taker_refunds_taker_payment_tx = match taker_sia_coin
        .send_taker_refunds_payment(taker_refunds_payment_args)
        .await
        .unwrap()
    {
        TransactionEnum::SiaTransaction(tx) => tx,
        _ => panic!("Expected SiaTransaction"),
    };
    taker_sia_coin.client.mine_blocks(1, &taker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    taker_sia_coin
        .client
        .get_transaction(&taker_refunds_taker_payment_tx.txid())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_spend_taker_payment_then_taker_extract_secret() {
    let docker = Cli::default();

    // Start the container
    let (_container, host_port) = init_walletd_container(&docker);
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let taker_ctx = init_ctx("taker passphrase", 9995).await;
    let taker_sia_coin = init_siacoin(taker_ctx, "TSIA", &helper_activation_request(host_port)).await;
    let taker_public_key = taker_sia_coin.my_keypair().unwrap().public();
    let taker_address = taker_public_key.address();
    taker_sia_coin.client.mine_blocks(201, &taker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let maker_ctx = init_ctx("maker passphrase", 9995).await;
    let maker_sia_coin = init_siacoin(maker_ctx, "TSIA", &helper_activation_request(host_port)).await;
    let maker_public_key = maker_sia_coin.my_keypair().unwrap().public();
    let maker_secret = vec![0u8; 32];
    let maker_secret_hash = SecretHashAlgo::SHA256.hash_secret(&maker_secret);

    let negotiated_time_lock = now_sec();
    let negotiated_time_lock_duration = 10u64;
    let negotiated_amount: BigDecimal = 1u64.into();

    let taker_send_payment_args = SendPaymentArgs {
        time_lock_duration: negotiated_time_lock_duration,
        time_lock: negotiated_time_lock,
        other_pubkey: maker_public_key.as_bytes(),
        secret_hash: &maker_secret_hash,
        amount: negotiated_amount,
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let taker_payment_tx = match taker_sia_coin
        .send_taker_payment(taker_send_payment_args)
        .await
        .unwrap()
    {
        TransactionEnum::SiaTransaction(tx) => tx,
        _ => panic!("Expected SiaTransaction"),
    };
    taker_sia_coin.client.mine_blocks(1, &taker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let maker_spend_payment_args = SpendPaymentArgs {
        other_payment_tx: &taker_payment_tx.tx_hex(),
        time_lock: negotiated_time_lock,
        other_pubkey: taker_public_key.as_bytes(),
        secret: &maker_secret,
        secret_hash: &maker_secret_hash,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };

    let maker_spends_taker_payment_tx = match maker_sia_coin
        .send_maker_spends_taker_payment(maker_spend_payment_args)
        .await
        .unwrap()
    {
        TransactionEnum::SiaTransaction(tx) => tx,
        _ => panic!("Expected SiaTransaction"),
    };
    taker_sia_coin.client.mine_blocks(1, &taker_address).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    taker_sia_coin
        .client
        .get_transaction(&maker_spends_taker_payment_tx.txid())
        .await
        .unwrap();

    let maker_spends_taker_payment_tx_hex = maker_spends_taker_payment_tx.tx_hex();

    let taker_extracted_secret = taker_sia_coin
        .extract_secret(&maker_secret_hash, maker_spends_taker_payment_tx_hex.as_slice(), false)
        .await
        .unwrap();

    assert_eq!(taker_extracted_secret, maker_secret);
}
*/
