//! RPC implementations for swaps with liquidity routing (LR) of EVM tokens

use super::one_inch::types::ClassicSwapDetails;
use crate::rpc::lp_commands::one_inch::errors::ApiIntegrationRpcError;
use crate::rpc::lp_commands::one_inch::rpcs::get_coin_for_one_inch;
use lr_impl::find_best_swap_path_with_lr;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::{map_mm_error::MapMmError, mm_error::MmResult};
use types::{
    LrExecuteRoutedTradeRequest, LrExecuteRoutedTradeResponse, LrFindBestQuoteRequest, LrFindBestQuoteResponse,
    LrGetQuotesForTokensRequest,
};

mod lr_impl;
mod types;

/// Finds the most cost-effective swap path using liquidity routing (LR) for EVM-compatible tokens (`Aggregated taker swap` path),
/// by selecting the best option from a list of orderbook entries (ask/bid orders).
/// This RPC returns the data needed for actual execution of the swap with LR .
///
/// # Overview
/// This RPC helps users execute token swaps even if they do not directly hold the tokens required
/// by the maker orders. It uses external liquidity routing (e.g., via 1inch provider) to perform necessary conversions, currently for EVM networks
///
/// A swap path may consist of:
///     - A liquidity routing (LR) step before or after the atomic swap.
///     - An atomic swap step to fill the selected maker order (ask or bid).
///
/// Use Case
/// The user wants to buy a specific amount of a token `user_base`, but only holds a different token `user_rel`.
/// This RPC evaluates possible swap paths by combining:
///     - Converting `user_rel` (`user_base`) to the token required by a maker order via LR.
///     - Filling the order through an atomic swap.
///     - Converting the token required by a maker order to `user_base` (`user_rel`) via LR.
/// It then selects and returns the most price-effective path, taking into account:
///     - prices of orders (provided in the params)
///     - 1inch LR quotes
///     - (TODO) Total swap and routing fees
/// Sell requests are processed in a similar way.
///
/// Example
/// A user wants to buy 1 BTC with their USDT, but the best available order sells 1 BTC for DAI.
/// This RPC calculates the total cost of liquidity routing the user's USDT into DAI and then using the
/// acquired DAI to take the BTC order. It compares this path against other potential candidates
/// (e.g., a order selling BTC for USDC and routing the user's USDT into USDC via LR) to find the cheapest option.
///
/// Inputs
/// - A list of maker ask or bid orders (orderbook entries)
/// - Trade method (`buy` or `sell`)
/// - Target or source amount to buy/sell
/// - User’s tokens `user_rel` and `user_base` to be used for the swap
///
/// Outputs
/// - The best swap path including any required LR steps
///
/// Current Limitations
/// - Only supports filling ask orders with:
///   - `user_rel` (sell request)
///   - Liquidity routing before the atomic swap: `user_rel` -> maker `rel`
/// - Does not yet support:
///   - User's buy request
///   - Filling bid orders
///   - Liquidity routing after the atomic swap
///
/// TODO:
/// - Return full trade fee breakdown (e.g., DEX fees, LR fees)
/// - Support the following additional aggregated swap configurations:
///   - Filling ask orders with LR after the atomic swap
///   - Filling bid orders with LR before and after the atomic swap
/// - Support user's buy request
///
/// Notes:
/// - This function relies on external quote APIs (currently 1inch) and may incur latency.
/// - Use this RPC when a direct atomic swap is not available or optimal, and pre/post-routing is needed.
pub async fn lr_find_best_quote_rpc(
    ctx: MmArc,
    req: LrFindBestQuoteRequest,
) -> MmResult<LrFindBestQuoteResponse, ApiIntegrationRpcError> {
    // TODO: add validation:
    // order.base_min_volume << req.amount <= order.base_max_volume
    // order.coin is supported in 1inch
    // order.price not zero
    // when best order is selected validate against req.rel_max_volume and req.rel_min_volume
    // coins in orders should be unique

    let (user_rel_coin, _) = get_coin_for_one_inch(&ctx, &req.user_rel).await?;
    let user_rel_chain = user_rel_coin
        .chain_id()
        .ok_or(ApiIntegrationRpcError::ChainNotSupported)?;
    let (swap_data, best_order, total_price) =
        find_best_swap_path_with_lr(&ctx, req.user_base, req.user_rel, req.asks, req.bids, &req.volume).await?;
    let lr_swap_details = ClassicSwapDetails::from_api_classic_swap_data(&ctx, user_rel_chain, swap_data)
        .await
        .mm_err(|err| ApiIntegrationRpcError::ApiDataError(err.to_string()))?;
    Ok(LrFindBestQuoteResponse {
        lr_swap_details,
        best_order,
        total_price,
        // TODO: implement later
        // trade_fee: ...
    })
}

/// Find possible swaps with liquidity routing of several user tokens to fill one order.
/// For the provided single order the RPC searches for the most price-effective swap path with LR for user tokens.
///
/// More info:
/// User is interested in buying some coin. There is an order available the User would like to fill but the User does not have tokens from the order.
/// User calls this RPC with the order, desired coin name, amount to buy or sell and list of User tokens to convert to/from with LR.
/// The RPC calls several 1inch classic swap quotes (to find most efficient token conversions)
/// and return possible LR paths to fill the order, with total swap prices.
/// TODO: should also returns total fees.
///
/// NOTE: this RPC does not select the best quote between User tokens because it finds routes for different tokens (with own value),
/// so returns all of them.
/// That is, it's up to the User to select the most cost effective swap, for e.g. comparing token fiat value.
/// In fact, this could be done even in this RPC as 1inch also can get value in fiat but maybe User evaludation is more prefferable.
/// Again, it's a TODO.
pub async fn lr_get_quotes_for_tokens_rpc(
    _ctx: MmArc,
    _req: LrGetQuotesForTokensRequest,
) -> MmResult<LrFindBestQuoteResponse, ApiIntegrationRpcError> {
    // TODO: impl later
    todo!()
}

/// Run a swap with LR to fill a maker order
pub async fn lr_execute_routed_trade_rpc(
    _ctx: MmArc,
    _req: LrExecuteRoutedTradeRequest,
) -> MmResult<LrExecuteRoutedTradeResponse, ApiIntegrationRpcError> {
    todo!()
}

#[cfg(all(test, feature = "test-ext-api", not(target_arch = "wasm32")))]
mod tests {
    use super::types::{AsksForCoin, LrFindBestQuoteRequest};
    use crate::lp_ordermatch::{OrderbookAddress, RpcOrderbookEntryV2};
    use crate::rpc::lp_commands::legacy::electrum;
    use coins::eth::EthCoin;
    use coins_activation::platform_for_tests::init_platform_coin_with_tokens_loop;
    use crypto::CryptoCtx;
    use mm2_number::{MmNumber, MmNumberMultiRepr};
    use mm2_test_helpers::for_tests::{btc_with_spv_conf, mm_ctx_with_custom_db_with_conf};
    use std::str::FromStr;
    use uuid::Uuid;

    /// Test to find best swap with LR.
    /// checks how to find an order from an utxo/token ask order list, which is the most price efficient if route from my token into the token in the order.
    /// With this test use --features test-ext-api and set ONE_INCH_API_TEST_AUTH env to the 1inch dev auth key
    /// TODO: make it mockable to run within CI
    #[tokio::test]
    async fn test_find_best_lr_swap_for_order_list() {
        // let _ = env_logger::try_init(); // enable to print log messages in the impl
        let main_net_url: String = std::env::var("ETH_MAIN_NET_URL_FOR_TEST").unwrap_or_default();
        let platform_coin = "ETH".to_owned();
        let base_conf = btc_with_spv_conf();
        let platform_coin_conf = json!({
            "coin": platform_coin.clone(),
            "name": "ethereum",
            "derivation_path": "m/44'/1'",
            "protocol": {
                "type": "ETH",
                "protocol_data": {
                    "chain_id": 1
                }
            }
        });

        // WETH = 2696.90 USD
        let weth_conf = json!({
            "coin": "WETH-ERC20",
            "name": "WETH-ERC20",
            "derivation_path": "m/44'/1'",
            "decimals": 18,
            "protocol": {
                "type": "ERC20",
                "protocol_data": {
                    "platform": "ETH",
                    "contract_address": "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
                }
            }
        });

        // BNB = 612.36 USD
        let bnb_conf = json!({
            "coin": "BNB-ERC20",
            "name": "BNB token",
            "derivation_path": "m/44'/1'",
            "decimals": 18,
            "protocol": {
                "type": "ERC20",
                "protocol_data": {
                    "platform": "ETH",
                    "contract_address": "0xB8c77482e45F1F44dE1745F52C74426C631bDD52"
                }
            }
        });
        // AAVE 258.75 USD
        let aave_conf = json!({
            "coin": "AAVE-ERC20",
            "name": "AAVE token",
            "derivation_path": "m/44'/1'",
            "decimals": 18,
            "protocol": {
                "type": "ERC20",
                "protocol_data": {
                    "platform": "ETH",
                    "contract_address": "0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9"
                }
            }
        });
        // CNC 0.136968 USD USD
        let cnc_conf = json!({
            "coin": "CNC-ERC20",
            "name": "CNC token",
            "derivation_path": "m/44'/1'",
            "decimals": 18,
            "protocol": {
                "type": "ERC20",
                "protocol_data": {
                    "platform": "ETH",
                    "contract_address": "0x9aE380F0272E2162340a5bB646c354271c0F5cFC"
                }
            }
        });

        let base_ticker = base_conf["coin"].as_str().unwrap().to_owned();
        let weth_ticker = weth_conf["coin"].as_str().unwrap().to_owned();
        let bnb_ticker = bnb_conf["coin"].as_str().unwrap().to_owned();
        let aave_ticker = aave_conf["coin"].as_str().unwrap().to_owned();
        let cnc_ticker = cnc_conf["coin"].as_str().unwrap().to_owned();

        let conf = json!({
            "coins": [base_conf, platform_coin_conf, weth_conf, bnb_conf, aave_conf, cnc_conf],
            "1inch_api": "https://api.1inch.dev"
        });
        let ctx = mm_ctx_with_custom_db_with_conf(Some(conf));
        CryptoCtx::init_with_iguana_passphrase(ctx.clone(), "123").unwrap();

        electrum(
            ctx.clone(),
            json!({
                "coin": base_ticker,
                "mm2": 1,
                "method": "electrum",
                "servers": [
                    {"url": "electrum1.cipig.net:10001"},
                    {"url": "electrum2.cipig.net:10001"},
                    {"url": "electrum3.cipig.net:10001"}
                ],
                "tx_history": false
            }),
        )
        .await
        .unwrap();
        init_platform_coin_with_tokens_loop::<EthCoin>(
            ctx.clone(),
            serde_json::from_value(json!({
                "ticker": platform_coin.clone(),
                "rpc_mode": "Default",
                "nodes": [
                    {"url": main_net_url}
                ],
                "swap_contract_address": "0xeA6D65434A15377081495a9E7C5893543E7c32cB",
                "erc20_tokens_requests": [
                    {"ticker": weth_ticker.clone()},
                    {"ticker": bnb_ticker.clone()},
                    {"ticker": aave_ticker.clone()},
                    {"ticker": cnc_ticker.clone()}
                ],
                "priv_key_policy": { "type": "ContextPrivKey" }
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        let asks = vec![AsksForCoin {
            base: base_ticker.clone(),
            orders: vec![
                RpcOrderbookEntryV2 {
                    coin: bnb_ticker,
                    address: OrderbookAddress::Transparent("RLL6n4ayAv1haokcEd1QUEYniyeoiYkn7W".into()),
                    price: MmNumberMultiRepr::from(MmNumber::from("145.69")),
                    pubkey: "02f3578fbc0fc76056eae34180a71e9190ee08ad05d40947aab7a286666e2ce798".to_owned(),
                    uuid: Uuid::from_str("7f26dc6a-39ab-4685-b5f1-55f12268ea50").unwrap(),
                    is_mine: false,
                    base_max_volume: MmNumberMultiRepr::from(MmNumber::from("1")),
                    base_min_volume: MmNumberMultiRepr::from(MmNumber::from("0.1")),
                    rel_max_volume: MmNumberMultiRepr::from(MmNumber::from("145.69")),
                    rel_min_volume: MmNumberMultiRepr::from(MmNumber::from("14.569")),
                    conf_settings: Default::default(),
                },
                RpcOrderbookEntryV2 {
                    coin: aave_ticker,
                    address: OrderbookAddress::Transparent("RK1JDwZ1LvH47Tvqm6pQM7aSqC2Zo6JwRF".into()),
                    price: MmNumberMultiRepr::from(MmNumber::from("370.334")),
                    pubkey: "02470bfb8e7710be4a7c2b8e9ba4bcfc5362a71643e64fc2e33b0d64c844ee9123".to_owned(),
                    uuid: Uuid::from_str("2aadf450-6a8e-4e4e-8b89-19ca10f23cc3").unwrap(),
                    is_mine: false,
                    base_max_volume: MmNumberMultiRepr::from(MmNumber::from("1")),
                    base_min_volume: MmNumberMultiRepr::from(MmNumber::from("0.1")),
                    rel_max_volume: MmNumberMultiRepr::from(MmNumber::from("370.334")),
                    rel_min_volume: MmNumberMultiRepr::from(MmNumber::from("37.0334")),
                    conf_settings: Default::default(),
                },
                RpcOrderbookEntryV2 {
                    coin: cnc_ticker,
                    address: OrderbookAddress::Transparent("RK1JDwZ1LvH47Tvqm6pQM7aSqC2Zo6JwRF".into()),
                    price: MmNumberMultiRepr::from(MmNumber::from("699300.69")),
                    pubkey: "03de96cb66dcfaceaa8b3d4993ce8914cd5fe84e3fd53cefdae45add8032792a12".to_owned(),
                    uuid: Uuid::from_str("89ab019f-b2fe-4d89-9764-96ac4a3fbf8e").unwrap(),
                    is_mine: false,
                    base_max_volume: MmNumberMultiRepr::from(MmNumber::from("1")),
                    base_min_volume: MmNumberMultiRepr::from(MmNumber::from("0.1")),
                    rel_max_volume: MmNumberMultiRepr::from(MmNumber::from("699300.69")),
                    rel_min_volume: MmNumberMultiRepr::from(MmNumber::from("69930.069")),
                    conf_settings: Default::default(),
                },
            ],
        }];
        let bids = vec![];

        let req = LrFindBestQuoteRequest {
            user_base: base_ticker,
            volume: "0.123".into(),
            asks,
            method: "buy".to_string(),
            bids,
            user_rel: weth_ticker,
        };

        let response = super::lr_find_best_quote_rpc(ctx, req).await;
        // log!("response={:?}", response); // enable to investigate the response
        assert!(response.is_ok());

        // BTC / WETH price around 35.0
        log!("response total_price={}", response.unwrap().total_price.to_decimal());
    }
}
