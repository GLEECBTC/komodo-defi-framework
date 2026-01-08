use crate::docker_tests::helpers::env::random_secp256k1_secret;
use crate::docker_tests::helpers::utxo::fund_privkey_utxo;

use super::utils::*;

use mm2_test_helpers::for_tests::{start_swaps, wait_until_event};

/*
KDF currently sits in an odd state between a binary and a library. These tests fall between a
"unit test" and a "integration test" due to this.

These Sia "functional tests" are running multiple KDF instances(multiple MmCtx using lp_init) within
the same process. This was not supported until now, and we encounter some issues with it.

The "payment_locktime" conf field used to set the HTLC locktime.

This "short_locktime_tests" module is an extension of "docker_functional_tests" and is simply a hack
to allow grouping the relevant tests together via `cargo test` commands. The tests in this module will
use a custom locktime of 60 seconds.

The "docker_functional_tests" will hold any tests that
can use the default of 900 seconds (CUSTOM_PAYMENT_LOCKTIME_DEFAULT).
*/
/// Initialize Alice and Bob, initialize Sia testnet container, initialize UTXO testnet container,
/// Bob sells DSIA for Alice's MYCOIN
/// Alice pays fee, Bob locks payment, Alice disappears prior to locking her payment
#[tokio::test]
async fn test_bob_sells_dsia_for_mycoin_alice_fails_to_lock() {
    let alice_priv = random_secp256k1_secret();
    let bob_priv = random_secp256k1_secret();

    // Give bob some sia and alice some mycoin
    fund_privkey_sia(&bob_priv, Currency(1e23 as u128)).await;
    fund_privkey_utxo("MYCOIN", 5.into(), &alice_priv).await;

    // Initalize Alice and Bob KDF instances
    let mut mm_bob = init_bob(&bob_priv, Some(60)).await;
    let mut mm_alice = init_alice(&alice_priv, &mm_bob.ip, Some(60)).await;

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

    // Stop Alice before she locks her payment
    wait_until_event(&mm_alice, &uuid, "TakerFeeSent", 600).await;
    mm_alice.stop().await.unwrap();

    // Wait for the swap to complete
    wait_until_event(&mm_bob, &uuid, "MakerPaymentRefundFinished", 600).await;
}

/// Initialize Alice and Bob, initialize Sia testnet container, initialize UTXO testnet container,
/// Bob sells DSIA for Alice's MYCOIN
/// Alice pays fee, Bob locks payment, Alice locks payment, Bob disappears prior to spending Alice's
/// payment, Alice refunds her payment, Bob refunds his payment
#[tokio::test]
async fn bob_sells_dsia_for_mycoin_bob_fails_to_spend() {
    let alice_priv = random_secp256k1_secret();
    let bob_priv = random_secp256k1_secret();

    // Give bob some sia and alice some mycoin
    fund_privkey_sia(&bob_priv, Currency(1e23 as u128)).await;
    fund_privkey_utxo("MYCOIN", 5.into(), &alice_priv).await;

    // Initalize Alice and Bob KDF instances
    let mut mm_bob = init_bob(&bob_priv, Some(60)).await;
    let mut mm_alice = init_alice(&alice_priv, &mm_bob.ip, Some(60)).await;

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

    // Stop Bob before he spends Alice's payment
    wait_until_event(&mm_bob, &uuid, "MakerPaymentSent", 600).await;
    mm_bob.stop().await.unwrap();

    // Wait for Alice to refund alice_payment
    wait_until_event(&mm_alice, &uuid, "TakerPaymentRefundFinished", 600).await;

    // Restart Bob and activate coins
    let mm_bob = re_init_bob(&mm_bob, &bob_priv, Some(60)).await;
    let _ = enable_dsia(&mm_bob).await;
    let _ = enable_mycoin(&mm_bob).await;

    // Wait for Bob to refund bob_payment
    wait_until_event(&mm_bob, &uuid, "MakerPaymentRefundFinished", 600).await;
}

/// Initialize Alice and Bob, initialize Sia testnet container, initialize UTXO testnet container,
/// Bob sells MYCOIN for Alice's DSIA
/// Alice pays fee, Bob locks payment, Alice locks payment, Bob disappears prior to spending Alice's
/// payment, Alice refunds her payment, Bob refunds his payment
#[tokio::test]
async fn bob_sells_mycoin_for_dsia_bob_fails_to_spend() {
    let alice_priv = random_secp256k1_secret();
    let bob_priv = random_secp256k1_secret();

    // Give alice some sia and bob some mycoin
    fund_privkey_sia(&alice_priv, Currency(1e23 as u128)).await;
    fund_privkey_utxo("MYCOIN", 5.into(), &bob_priv).await;

    // Initalize Alice and Bob KDF instances
    let mut mm_bob = init_bob(&bob_priv, Some(60)).await;
    let mut mm_alice = init_alice(&alice_priv, &mm_bob.ip, Some(60)).await;

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
    let uuid = start_swaps(&mut mm_bob, &mut mm_alice, &[("MYCOIN", "DSIA")], 1., 1., 0.05)
        .await
        .first()
        .cloned()
        .unwrap();

    // Stop Bob before he spends Alice's payment
    wait_until_event(&mm_bob, &uuid, "MakerPaymentSent", 600).await;
    mm_bob.stop().await.unwrap();

    // Wait for Alice to refund alice_payment
    wait_until_event(&mm_alice, &uuid, "TakerPaymentRefundFinished", 600).await;

    // Restart Bob and activate coins
    let mm_bob = re_init_bob(&mm_bob, &bob_priv, Some(60)).await;
    let _ = enable_dsia(&mm_bob).await;
    let _ = enable_mycoin(&mm_bob).await;

    // Wait for Bob to refund bob_payment
    wait_until_event(&mm_bob, &uuid, "MakerPaymentRefundFinished", 600).await;
}
