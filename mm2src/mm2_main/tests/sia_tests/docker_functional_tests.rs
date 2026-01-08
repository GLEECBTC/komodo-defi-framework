use crate::docker_tests::helpers::env::random_secp256k1_secret;
use crate::docker_tests::helpers::utxo::fund_privkey_utxo;

use super::utils::*;

use coins::siacoin::{ApiClientHelpers, SiaTransactionTypes};
use mm2_number::BigDecimal;
use mm2_test_helpers::for_tests::{start_swaps, wait_for_swap_finished_or_err};
use serde::Deserialize;
use serde_json::{json, Value as Json};

use std::str::FromStr;

#[derive(Debug, Deserialize)]
struct SiaWithdrawResponse {
    tx_json: SiaTransactionTypes,
    from: Vec<String>,
    to: Vec<String>,
    total_amount: BigDecimal,
    spent_by_me: BigDecimal,
    received_by_me: BigDecimal,
    my_balance_change: BigDecimal,
    fee_details: Json,
    coin: String,
}

/// Tests sia client and it's connectivity to the sia walletd global container.
#[tokio::test]
async fn debug_init_sia_client() {
    let client = init_sia_client().await.unwrap();

    let address =
        Address::from_str("439536d27e5cbf46b0ff873056fa8ef5424fd3f574e5ed694450c8dc4323fe6062d40a11fbc9").unwrap();

    let response = client.address_balance(address.clone()).await.unwrap();
    log!("Address balance: {:?}", response);
    assert_eq!(response.siacoins, Currency(0));

    fund_address(&address, Currency(10)).await;

    let response = client.address_balance(address).await.unwrap();
    log!("Address balance: {:?}", response);

    assert_eq!(response.siacoins, Currency(10));
}

/// Initialize Alice and Bob, check that they connected via p2p network, enable DSIA for both parties
#[tokio::test]
async fn test_alice_and_bob_enable_dsia() {
    let alice_priv = random_secp256k1_secret();
    let bob_priv = random_secp256k1_secret();

    let mm_bob = init_bob(&bob_priv, None).await;
    let mm_alice = init_alice(&alice_priv, &mm_bob.ip, None).await;

    wait_for_peers_connected(&mm_alice, &mm_bob, std::time::Duration::from_secs(30))
        .await
        .unwrap();

    let _bob_enable_sia_resp = enable_dsia(&mm_alice).await;
    let _alice_enable_sia_resp = enable_dsia(&mm_bob).await;
}

/// Test komodo client and it's connectivity to the komodod (mycoin) global container.
/// Validate Alice and Bob's addresses were imported via `importaddress`
#[tokio::test]
async fn test_utxo_container_and_client() {
    let client = get_komodod_client(
        "RNa3bJJC2L3UUCGQ9WY5fhCSzSd5ExiAWr",
        "RLHqXM7q689D1PZvt9nH5nmouSPMG9sopG",
    )
    .await;

    let alice_validate_address_resp = client
        .rpc("validateaddress", json!(["RNa3bJJC2L3UUCGQ9WY5fhCSzSd5ExiAWr"]))
        .await;
    let bob_validate_address_resp = client
        .rpc("validateaddress", json!(["RLHqXM7q689D1PZvt9nH5nmouSPMG9sopG"]))
        .await;

    assert_eq!(alice_validate_address_resp["result"]["iswatchonly"], true);
    assert_eq!(bob_validate_address_resp["result"]["iswatchonly"], true);
}

/// Fund a DSIA account, call `withdraw` with `max: true`, and verify that:
/// * The TransactionDetails report spending the full balance.
/// * The fixed Sia fee is correctly reflected in `fee_details`.
/// * The transaction targets the expected address.
/// * After broadcasting, the Sia address has zero remaining balance (no unexpected change).
#[tokio::test]
async fn test_dsia_withdraw_max_spends_full_balance_minus_fee() {
    // Use a fresh private key so the DSIA account starts empty.
    let priv_key = random_secp256k1_secret();

    // Match the fixed fee used in `SiaWithdrawBuilder::build`.
    // If `TX_FEE_HASTINGS` in `sia_withdraw.rs` changes, this test should be updated.
    const FIXED_WITHDRAW_FEE_HASTINGS: u128 = 10_000_000_000_000_000_000; // 1e19 Hastings
    const FUNDING_MULTIPLIER: u128 = 5;
    let funding_amount_hastings = FIXED_WITHDRAW_FEE_HASTINGS * FUNDING_MULTIPLIER;

    // Fund the DSIA account on the Sia testnet.
    fund_privkey_sia(&priv_key, Currency(funding_amount_hastings)).await;

    // Compute the Sia address corresponding to this MarketMaker key.
    let keypair = Keypair::from_private_bytes(priv_key.as_slice()).unwrap();
    let mm_sia_address = Address::from_public_key(&keypair.public());

    let client = init_sia_client().await.unwrap();
    let balance_before = client.address_balance(mm_sia_address.clone()).await.unwrap();
    assert_eq!(balance_before.siacoins, Currency(funding_amount_hastings));

    // Spin up a MarketMaker node using the same key and enable DSIA.
    let mm = init_bob(&priv_key, None).await;
    let _ = enable_dsia(&mm).await;

    // Withdraw everything to a distinct address (Charlie's).
    let to_address = CHARLIE_SIA_ADDRESS.to_string();

    let tx_details: SiaWithdrawResponse = mm
        .rpc_typed(&json!({
            "method": "withdraw",
            "coin": "DSIA",
            "to": to_address,
            "max": true,
        }))
        .await
        .unwrap();

    // Basic shape assertions.
    assert_eq!(tx_details.coin, "DSIA");
    assert_eq!(tx_details.from.len(), 1);
    assert_eq!(tx_details.to, vec![to_address.clone()]);

    // Sia has 24 decimal places; 1 siacoin = 10^24 Hastings.
    // We'll convert our known Hastings amounts into BigDecimal siacoin amounts
    // using this fixed scale.
    let scale = BigDecimal::from_str("1000000000000000000000000").unwrap(); // 10^24

    let expected_fee = BigDecimal::from_str(&FIXED_WITHDRAW_FEE_HASTINGS.to_string()).unwrap() / scale.clone();
    let expected_total = BigDecimal::from_str(&(funding_amount_hastings - FIXED_WITHDRAW_FEE_HASTINGS).to_string())
        .unwrap()
        / scale.clone();
    let expected_spent = &expected_total + &expected_fee;
    let zero = BigDecimal::from(0);

    // Amount semantics:
    // * total_amount == value sent to the recipient (funds minus fee)
    // * spent_by_me == total_amount + fee (full amount deducted from our wallet)
    // * received_by_me == 0 (no change back to ourselves)
    // * my_balance_change == -spent_by_me
    assert_eq!(tx_details.total_amount, expected_total);
    assert_eq!(tx_details.spent_by_me, expected_spent);
    assert_eq!(tx_details.received_by_me, zero);
    assert_eq!(&tx_details.my_balance_change + &tx_details.spent_by_me, zero);

    // Fee details should reflect the fixed Sia withdraw fee.
    let fee_total_str = tx_details.fee_details["total_amount"]
        .as_str()
        .expect("fee_details.total_amount as string");
    let fee_total: BigDecimal = fee_total_str.parse().unwrap();
    assert_eq!(fee_total, expected_fee);

    // Broadcast the transaction on Sia and ensure no balance remains for the DSIA address.
    let signed_tx = match tx_details.tx_json {
        SiaTransactionTypes::V2Transaction(tx) => tx,
        _ => panic!("Expected V2Transaction in tx_json"),
    };

    client.broadcast_transaction(&signed_tx).await.unwrap();
    client.mine_blocks(1, &CHARLIE_SIA_ADDRESS).await.unwrap();

    let balance_after = client.address_balance(mm_sia_address).await.unwrap();
    assert_eq!(balance_after.siacoins, Currency(0));
}

/// Initialize Alice and Bob, initialize Sia testnet container, initialize UTXO testnet container,
/// Bob sells DSIA for Alice's MYCOIN
#[tokio::test]
async fn test_bob_sells_dsia_for_mycoin() {
    let alice_priv = random_secp256k1_secret();
    let bob_priv = random_secp256k1_secret();

    // Give bob some sia and alice some mycoin
    fund_privkey_sia(&bob_priv, Currency(1e23 as u128)).await;
    fund_privkey_utxo("MYCOIN", 5.into(), &alice_priv).await;

    // Initalize Alice and Bob KDF instances
    let mut mm_bob = init_bob(&bob_priv, None).await;
    let mut mm_alice = init_alice(&alice_priv, &mm_bob.ip, None).await;

    // Enable DSIA coin for Alice and Bob
    let _ = enable_dsia(&mm_bob).await;
    let _ = enable_dsia(&mm_alice).await;

    // Enable MYCOIN coin via Native node for Alice and Bob
    let _ = enable_mycoin(&mm_alice).await;
    let _ = enable_mycoin(&mm_bob).await;

    // Wait for Alice and Bob KDF instances to connect
    wait_for_peers_connected(&mm_alice, &mm_bob, std::time::Duration::from_secs(30))
        .await
        .unwrap();

    // Start a swap where Bob sells DSIA for Alice's MYCOIN
    let uuid = start_swaps(&mut mm_bob, &mut mm_alice, &[("DSIA", "MYCOIN")], 1., 1., 0.05)
        .await
        .first()
        .cloned()
        .unwrap();

    // Wait for the swap to complete
    wait_for_swap_finished_or_err(&mm_alice, &uuid, 360).await.unwrap();
    wait_for_swap_finished_or_err(&mm_bob, &uuid, 60).await.unwrap();
}

/// Initialize Alice and Bob, initialize Sia testnet container, initialize UTXO testnet container,
/// Bob sells MYCOIN for Alice's DSIA
#[tokio::test]
async fn test_bob_sells_mycoin_for_dsia() {
    let alice_priv = random_secp256k1_secret();
    let bob_priv = random_secp256k1_secret();

    // Give alice some sia and bob some mycoin
    fund_privkey_sia(&alice_priv, Currency(1e23 as u128)).await;
    fund_privkey_utxo("MYCOIN", 5.into(), &bob_priv).await;

    // Initalize Alice and Bob KDF instances
    let mut mm_bob = init_bob(&bob_priv, None).await;
    let mut mm_alice = init_alice(&alice_priv, &mm_bob.ip, None).await;

    // Enable DSIA coin for Alice and Bob
    let _ = enable_dsia(&mm_bob).await;
    let _ = enable_dsia(&mm_alice).await;

    // Enable MYCOIN coin via Native node for Alice and Bob
    let _ = enable_mycoin(&mm_alice).await;
    let _ = enable_mycoin(&mm_bob).await;

    // Wait for Alice and Bob KDF instances to connect
    wait_for_peers_connected(&mm_alice, &mm_bob, std::time::Duration::from_secs(30))
        .await
        .unwrap();

    // Start a swap where Bob sells MYCOIN for Alice's DSIA
    let uuid = start_swaps(&mut mm_bob, &mut mm_alice, &[("MYCOIN", "DSIA")], 1., 1., 0.05)
        .await
        .first()
        .cloned()
        .unwrap();

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
