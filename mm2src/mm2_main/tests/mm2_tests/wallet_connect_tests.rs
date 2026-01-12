use coins::PrivKeyActivationPolicy;
use common::executor::Timer;
use common::{block_on, log};
use mm2_test_helpers::for_tests::{
    enable_utxo_v2_electrum, new_walletconnect_connection, start_swaps, wait_for_swaps_finish_and_check_status,
    wait_for_walletconnect_session, MarketMakerIt, Mm2InitPrivKeyPolicy, Mm2TestConfForSwap,
};
use mm2_test_helpers::structs::CreateConnectionResponse;
use serde_json::json;

#[cfg(not(target_arch = "wasm32"))]
/// Perform a swap using WalletConnect protocol with two tBTC (testnet4) coins.
async fn perform_walletconnect_swap() {
    let walletconnect_namespaces = json!({
        "required_namespaces": {
            "bip122": {
                "chains": [
                    "bip122:00000000da84f2bafbbc53dee25a72ae" // Bitcoin testnet4 chain_id
                ],
                "methods": [
                    "getAccountAddresses", // Needed for activation
                    "signMessage", // Might be needed for activation (when the wallet doesn't send the pubkeys from getAccountAddresses)
                    "signPsbt", // Needed for HTLC signing (but we use it for any signing as well)
                ],
                "events": []
            }
        }
    });
    let electrums = vec![
        json!({ "url": "testnet.aranguren.org:52001", "protocol": "TCP" }),
        json!({ "url": "blackie.c3-soft.com:57010", "protocol": "SSL" }),
    ];

    // Create two tBTC coins with different coin names to test swapping them.
    let coins: Vec<_> = (1..=2)
        .map(|coin_number| {
            json!({
                "coin": format!("tBTC-{coin_number}"),
                "name": format!("tbitcoin-{coin_number}"),
                "fname": format!("Bitcoin Testnet {coin_number}"),
                "orderbook_ticker": format!("tBTC-{coin_number}"),
                "sign_message_prefix": "Bitcoin Signed Message:\n",
                "bech32_hrp": "tb",
                "txfee": 0,
                "pubtype": 111,
                "p2shtype": 196,
                "dust": 1000,
                "txfee": 2000,
                "segwit": true,
                "address_format": {
                    "format": "segwit"
                },
                "mm2": 1,
                "is_testnet": true,
                "required_confirmations": 0,
                "protocol": {
                    "type": "UTXO",
                    "protocol_data": {
                        "chain_id": "bip122:00000000da84f2bafbbc53dee25a72ae"
                    }
                },
                "derivation_path": "m/84'/1'",
            })
        })
        .collect();
    let trading_pair = (coins[0]["coin"].as_str().unwrap(), coins[1]["coin"].as_str().unwrap());
    let coins = json!(coins);

    let bob_conf = Mm2TestConfForSwap::bob_conf_with_policy(&Mm2InitPrivKeyPolicy::GlobalHDAccount, &coins);
    // Uncomment to test the refund case. The quickest way to test both refunds is to reject signing TakerPaymentSpend (the 4th signing prompt).
    // This will force the taker to refund himself and after sometime the maker will also refund himself because he can't spend the TakerPayment anymore (as it's already refunded).
    // Note that you need to run the test with `--features custom-swap-locktime` to enable the custom `payment_locktime` feature.
    // bob_conf.conf["payment_locktime"] = (1 * 60).into();
    let mut mm_bob = MarketMakerIt::start_async(bob_conf.conf, bob_conf.rpc_password, None)
        .await
        .unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());
    Timer::sleep(2.).await;

    let alice_conf = Mm2TestConfForSwap::alice_conf_with_policy(
        &Mm2InitPrivKeyPolicy::GlobalHDAccount,
        &coins,
        &mm_bob.my_seed_addr(),
    );
    // Uncomment to test the refund case
    // alice_conf.conf["payment_locktime"] = (1 * 60).into();
    let mut mm_alice = MarketMakerIt::start_async(alice_conf.conf, alice_conf.rpc_password, None)
        .await
        .unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());
    Timer::sleep(2.).await;

    for (mm, operator) in [(&mut mm_bob, "Bob"), (&mut mm_alice, "Alice")] {
        // Create a WalletConnect connection.
        let CreateConnectionResponse { url, pairing_topic } =
            new_walletconnect_connection(mm, walletconnect_namespaces.clone()).await;
        log!("{operator}'s WalletConnect connection:\n{url}\n\n");
        // Wait for the user to approve the connection and establish the session.
        let session_topic = wait_for_walletconnect_session(mm, &pairing_topic, 300).await;
        let priv_key_policy = PrivKeyActivationPolicy::WalletConnect {
            session_topic: session_topic.into(),
        };
        // Enable the coin pair for this operator.
        let rc = enable_utxo_v2_electrum(
            mm,
            trading_pair.0,
            electrums.clone(),
            None,
            600,
            Some(json!(priv_key_policy)),
        )
        .await;
        log!("enable {} ({operator}): {rc:?}", trading_pair.0);
        let rc = enable_utxo_v2_electrum(
            mm,
            trading_pair.1,
            electrums.clone(),
            None,
            600,
            Some(json!(priv_key_policy)),
        )
        .await;
        log!("enable {} ({operator}): {rc:?}", trading_pair.1);
    }

    // Start the swap
    let uuids = start_swaps(&mut mm_bob, &mut mm_alice, &[trading_pair], 1.0, 1.0, 0.0002).await;
    // Wait for the swaps to finish (you need to accept signing the HTLCs in the WalletConnect in this stage).
    wait_for_swaps_finish_and_check_status(&mut mm_bob, &mut mm_alice, &uuids, 0.0002, 1.0).await;

    mm_bob.stop().await.unwrap();
    mm_alice.stop().await.unwrap();
}

#[test]
#[ignore]
fn test_walletconnect_swap() {
    block_on(perform_walletconnect_swap());
}
