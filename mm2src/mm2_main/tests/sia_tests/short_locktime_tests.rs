use super::utils::*;
use mm2_main::lp_swap::PAYMENT_LOCKTIME;
use std::sync::atomic::Ordering;

use coins::siacoin::ApiClientHelpers;

use mm2_test_helpers::for_tests::{start_swaps, wait_until_event};

/*
KDF currently sits in an odd state between a binary and a library. These tests fall between a
"unit test" and a "integration test" due to this.

These Sia "functional tests" are running multiple KDF instances(multiple MmCtx using lp_init) within
the same process. This was not supported until now, and we encounter some issues with it.

The PAYMENT_LOCKTIME variable used to set the HTLC locktime is a constant, and typically it cannot be
changed. We have addressed this in other tests with the "custom-swap-locktime" feature. However, this
feature is not suitable when we execute many KDF instances within the same process. Any other
current tests requiring a custom value for PAYMENT_LOCKTIME work because we spawn each KDF instance
as a subprocess.

Ideally PAYMENT_LOCKTIME would be added to MmCtx, but this would require significant changes.

We do have several coins/protocols requiring a different value. The way we address
these currently is by multiplying the constant by a modifier based on the ticker symbol. See
lp_atomic_locktime_v1 or lp_atomic_locktime_v2

This "short_locktime_tests" module is an extension of "docker_functional_tests" and is simply a hack
to allow grouping the relevant tests together via `cargo test` commands. The tests in this module will
use a custom locktime of 60 seconds.

The "docker_functional_tests" will hold any tests that
can use the default of 900 seconds (CUSTOM_PAYMENT_LOCKTIME_DEFAULT).
*/

/// Initialize Alice and Bob, initialize Sia testnet container, initialize UTXO testnet container,
/// Bob sells DSIA for Alice's DUTXO
/// Alice pays fee, Bob locks payment, Alice disappears prior to locking her payment
#[tokio::test]
async fn test_bob_sells_dsia_for_dutxo_alice_fails_to_lock() {
    // set payment locktime to 60 seconds
    // FIXME this is a global setting and will affect other tests
    PAYMENT_LOCKTIME.store(60, Ordering::Relaxed);

    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let netid = get_unique_netid();

    // Start the Utxo nodes container with Alice as miner
    let client = get_komodod_client(ALICE_KMD_KEY, BOB_KMD_KEY).await;

    // Start the Sia container and mine 155 blocks to Bob
    let dsia = get_global_walletd_container().await;
    dsia.client.mine_blocks(155, &BOB_SIA_ADDRESS).await.unwrap();

    // Initalize Alice and Bob KDF instances
    let (_ctx_bob, mut mm_bob) = init_bob(&temp_dir, netid, Some(client.conf.port)).await;
    let (ctx_alice, mut mm_alice) = init_alice(&temp_dir, netid, Some(client.conf.port)).await;

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

    // Stop Alice before she locks her payment
    wait_until_event(&mm_alice, &uuid, "TakerFeeSent", 600).await;
    ctx_alice.stop().await.unwrap();

    // Wait for the swap to complete
    wait_until_event(&mm_bob, &uuid, "MakerPaymentRefundFinished", 600).await;
}

/// Initialize Alice and Bob, initialize Sia testnet container, initialize UTXO testnet container,
/// Bob sells DSIA for Alice's DUTXO
/// Alice pays fee, Bob locks payment, Alice locks payment, Bob disappears prior to spending Alice's
/// payment, Alice refunds her payment, Bob refunds his payment
#[tokio::test]
async fn bob_sells_dsia_for_dutxo_bob_fails_to_spend() {
    // set payment locktime to 60 seconds
    // FIXME this is a global setting and will affect other tests
    PAYMENT_LOCKTIME.store(60, Ordering::Relaxed);

    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let netid = get_unique_netid();

    // Start the Utxo nodes container with Alice as miner
    let client = get_komodod_client(ALICE_KMD_KEY, BOB_KMD_KEY).await;

    // Start the Sia container and mine 155 blocks to Bob
    let dsia = get_global_walletd_container().await;
    dsia.client.mine_blocks(155, &BOB_SIA_ADDRESS).await.unwrap();

    // Initalize Alice and Bob KDF instances
    let (ctx_bob, mut mm_bob) = init_bob(&temp_dir, netid, Some(client.conf.port)).await;
    let (_ctx_alice, mut mm_alice) = init_alice(&temp_dir, netid, Some(client.conf.port)).await;

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

    let dsia_port = dsia.host_port;

    // Stop Bob before he spends Alice's payment
    wait_until_event(&mm_bob, &uuid, "MakerPaymentSent", 600).await;
    ctx_bob.stop().await.unwrap();

    // Wait for Alice to refund alice_payment
    wait_until_event(&mm_alice, &uuid, "TakerPaymentRefundFinished", 600).await;

    // Restart Bob and activate coins
    let (_ctx_bob, mm_bob) = init_bob(&temp_dir, netid, Some(client.conf.port)).await;
    let _ = enable_dsia(&mm_bob, dsia_port).await;
    let _ = enable_dutxo(&mm_bob).await;

    // Wait for Bob to refund bob_payment
    wait_until_event(&mm_bob, &uuid, "MakerPaymentRefundFinished", 600).await;
}

/// Initialize Alice and Bob, initialize Sia testnet container, initialize UTXO testnet container,
/// Bob sells DUTXO for Alice's DSIA
/// Alice pays fee, Bob locks payment, Alice locks payment, Bob disappears prior to spending Alice's
/// payment, Alice refunds her payment, Bob refunds his payment
#[tokio::test]
async fn bob_sells_dutxo_for_dsia_bob_fails_to_spend() {
    // set payment locktime to 60 seconds
    // FIXME this is a global setting and will affect other tests
    PAYMENT_LOCKTIME.store(60, Ordering::Relaxed);

    let temp_dir = init_test_dir(current_function_name!(), true).await;
    let netid = get_unique_netid();

    // Start the Utxo nodes container with Bob as funded key
    let client = get_komodod_client(BOB_KMD_KEY, ALICE_KMD_KEY).await;

    // Start the Sia container and mine 155 blocks to Alice
    let dsia = get_global_walletd_container().await;
    dsia.client.mine_blocks(155, &ALICE_SIA_ADDRESS).await.unwrap();

    // Initalize Alice and Bob KDF instances
    let (ctx_bob, mut mm_bob) = init_bob(&temp_dir, netid, Some(client.conf.port)).await;
    let (_ctx_alice, mut mm_alice) = init_alice(&temp_dir, netid, Some(client.conf.port)).await;

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
    let uuid = start_swaps(&mut mm_bob, &mut mm_alice, &[("DUTXO", "DSIA")], 1., 1., 0.05)
        .await
        .first()
        .cloned()
        .unwrap();

    let dsia_port = dsia.host_port;

    // Stop Bob before he spends Alice's payment
    wait_until_event(&mm_bob, &uuid, "MakerPaymentSent", 600).await;
    ctx_bob.stop().await.unwrap();

    // Wait for Alice to refund alice_payment
    wait_until_event(&mm_alice, &uuid, "TakerPaymentRefundFinished", 600).await;

    // Restart Bob and activate coins
    let (_ctx_bob, mm_bob) = init_bob(&temp_dir, netid, Some(client.conf.port)).await;
    let _ = enable_dsia(&mm_bob, dsia_port).await;
    let _ = enable_dutxo(&mm_bob).await;

    // Wait for Bob to refund bob_payment
    wait_until_event(&mm_bob, &uuid, "MakerPaymentRefundFinished", 600).await;
}
