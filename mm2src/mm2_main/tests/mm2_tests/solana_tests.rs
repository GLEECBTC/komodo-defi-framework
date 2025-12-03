use common::{block_on, log};
use mm2_number::BigDecimal;
use mm2_test_helpers::for_tests::Mm2TestConf;
use mm2_test_helpers::for_tests::{send_raw_transaction, withdraw_v1, MarketMakerIt};

use serde_json::json;

const SOLANA_DEVNET_RPC_URL: &str = "https://api.devnet.solana.com";
// TODO: Use different seed here.
const SOLANA_DEVNET_TEST_SEED: &str = "iris test seed";

fn solana_coin_config() -> serde_json::Value {
    json!({
        "coin": "SOL-DEV",
        "name": "solana",
        "fname": "Solana",
        "required_confirmations": 2,
        "avg_blocktime": 3,
        "protocol": {
            "type": "SOLANA",
            "protocol_data": {}
        },
        "derivation_path": "m/44'/501'"
    })
}

fn usdc_token_config() -> serde_json::Value {
    json!({
        "coin": "USDC-SOL-DEV",
        "name": "usd-coin-devnet",
        "fname": "USD Coin (Devnet)",
        "required_confirmations": 2,
        "avg_blocktime": 3,
        "protocol": {
            "type": "SOLANATOKEN",
            "protocol_data": {
                "platform": "SOL-DEV",
                "decimals": 6,
                "mint_address": "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU",
                "program_id": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
            }
        },
        "derivation_path": "m/44'/501'"
    })
}

pub async fn enable_solana(mm: &MarketMakerIt, coin: &str, tokens: &[&str], rpc_urls: &[&str]) -> serde_json::Value {
    let tokens: Vec<_> = tokens.iter().map(|ticker| json!({ "ticker": ticker })).collect();
    let nodes: Vec<_> = rpc_urls.iter().map(|u| json!({ "url": u })).collect();
    let method = "experimental::enable_solana_with_assets";

    let request = json!({
        "userpass": mm.userpass,
        "method": method,
        "mmrpc": "2.0",
        "params": {
            "ticker": coin,
            "tokens_params": tokens,
            "nodes": nodes
        }
    });
    log!("{method} request {}", serde_json::to_string(&request).unwrap());

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(request.0, http::StatusCode::OK, "'{method}' failed: {}", request.1);
    log!("{method} response {}", request.1);
    serde_json::from_str(&request.1).unwrap()
}

#[test]
fn enable_with_tokens_and_withdraw() {
    const MY_ADDRESS: &str = "5dbw8U6zrLFwtYQgy3gxvMFe6PzWRs8yXB7bXShCnbfT";

    let coins = json!([solana_coin_config(), usdc_token_config()]);
    let coin = coins[0]["coin"].as_str().unwrap();
    let usdc_token = coins[1]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(SOLANA_DEVNET_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_res = block_on(enable_solana(&mm, coin, &[usdc_token], &[SOLANA_DEVNET_RPC_URL]));
    log!("Activation {}", serde_json::to_string(&activation_res).unwrap());

    let to_address = "devwuNsNYACyiEYxRNqMNseBpNnGfnd4ZwNHL7sphqv";
    // Just call withdraw without sending to check response correctness.
    let tx_details = block_on(withdraw_v1(&mm, coin, to_address, "0.1", None));
    log!("Withdraw to other {}", serde_json::to_string(&tx_details).unwrap());
    assert_eq!(tx_details.received_by_me, BigDecimal::default());
    assert_eq!(tx_details.to, vec![to_address.to_owned()]);
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);

    // Withdraw and send transaction to ourselves.
    let tx_details = block_on(withdraw_v1(&mm, coin, MY_ADDRESS, "0.1", None));
    log!("Withdraw to self {}", serde_json::to_string(&tx_details).unwrap());

    let expected_received: BigDecimal = "0.1".parse().unwrap();
    assert_eq!(tx_details.received_by_me, expected_received);

    assert_eq!(tx_details.to, vec![MY_ADDRESS.to_owned()]);
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);

    let send_raw_tx = block_on(send_raw_transaction(&mm, coin, &tx_details.tx_hex));
    log!("Send raw tx {}", serde_json::to_string(&send_raw_tx).unwrap());
}
