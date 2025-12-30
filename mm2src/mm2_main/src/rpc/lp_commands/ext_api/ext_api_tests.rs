use crate::rpc::lp_commands::ext_api::{
    ext_api_types::{
        ClassicSwapCreateOptParams, ClassicSwapCreateRequest, ClassicSwapQuoteOptParams, ClassicSwapQuoteRequest,
    },
    one_inch_v6_0_classic_swap_create_rpc, one_inch_v6_0_classic_swap_quote_rpc,
};
use coins::eth::EthCoin;
use coins_activation::platform_for_tests::init_platform_coin_with_tokens_loop;
use common::block_on;
use crypto::CryptoCtx;
use mm2_core::mm_ctx::MmCtxBuilder;
use mm2_number::{BigDecimal, MmNumber};
use mocktopus::mocking::{MockResult, Mockable};
use std::str::FromStr;
use trading_api::one_inch_api::{classic_swap_types::ClassicSwapData, client::ApiClient};

#[test]
fn test_classic_swap_response_conversion() {
    let ticker_coin = "ETH".to_owned();
    let ticker_token = "JST".to_owned();
    let eth_conf = json!({
        "coin": ticker_coin,
        "name": "ethereum",
        "derivation_path": "m/44'/1'",
        "chain_id": 1,
        "decimals": 18,
        "protocol": {
            "type": "ETH",
            "protocol_data": {
                "chain_id": 1,
            }
        },
        "trezor_coin": "Ethereum"
    });
    let jst_conf = json!({
        "coin": ticker_token,
        "name": "jst",
        "chain_id": 1,
        "decimals": 6,
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH",
                "contract_address": "0x09d0d71FBC00D7CCF9CFf132f5E6825C88293F19"
            }
        },
    });

    let conf = json!({
        "coins": [eth_conf, jst_conf],
        "1inch_api": "https://api.1inch.dev"
    });
    let ctx = MmCtxBuilder::new().with_conf(conf).into_mm_arc();
    CryptoCtx::init_with_iguana_passphrase(ctx.clone(), "123").unwrap();

    block_on(init_platform_coin_with_tokens_loop::<EthCoin>(
        ctx.clone(),
        serde_json::from_value(json!({
            "ticker": ticker_coin,
            "rpc_mode": "Default",
            "nodes": [
                {"url": "https://sepolia.drpc.org"},
                {"url": "https://ethereum-sepolia-rpc.publicnode.com"},
                {"url": "https://rpc2.sepolia.org"},
                {"url": "https://rpc.sepolia.org/"}
            ],
            "swap_contract_address": "0xeA6D65434A15377081495a9E7C5893543E7c32cB",
            "erc20_tokens_requests": [{"ticker": ticker_token}],
            "priv_key_policy": { "type": "ContextPrivKey" }
        }))
        .unwrap(),
    ))
    .unwrap();

    let response_quote_raw = json!({
        "dstAmount": "13",
        "srcToken": {
            "address": "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "symbol": ticker_coin,
            "name": "Ether",
            "decimals": 18,
            "eip2612": false,
            "isFoT": false,
            "logoURI": "https://tokens.1inch.io/0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee.png",
            "tags": [
                "crosschain",
                "GROUP:ETH",
                "native",
                "PEG:ETH"
            ]
        },
        "dstToken": {
            "address": "0x1234567890123456789012345678901234567890",
            "symbol": ticker_token,
            "name": "Test just token",
            "decimals": 6,
            "eip2612": false,
            "isFoT": false,
            "logoURI": "https://example.org/0x1234567890123456789012345678901234567890.png",
            "tags": [
                "crosschain",
                "GROUP:JSTT",
                "PEG:JST",
                "tokens"
            ]
        },
        "protocols": [
        [
            [
            {
                "name": "SUSHI",
                "part": 100,
                "fromTokenAddress": "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                "toTokenAddress": "0xf16e81dce15b08f326220742020379b855b87df9"
            }
            ],
            [
            {
                "name": "ONE_INCH_LIMIT_ORDER_V3",
                "part": 100,
                "fromTokenAddress": "0xf16e81dce15b08f326220742020379b855b87df9",
                "toTokenAddress": "0xdac17f958d2ee523a2206206994597c13d831ec7"
            }
            ]
        ]
        ],
        "gas": 452704
    });

    let response_create_raw = json!({
        "dstAmount": "13",
        "tx": {
            "from": "0x590559f6fb7720f24ff3e2fccf6015b466e9c92c",
            "to": "0x111111125421ca6dc452d289314280a0f8842a65",
            "data": "0x07ed23790000000000000000000000005f515f6c524b18ca30f7783fb58dd4be2e9904ec000000000000000000000000eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee000000000000000000000000dac17f958d2ee523a2206206994597c13d831ec70000000000000000000000005f515f6c524b18ca30f7783fb58dd4be2e9904ec000000000000000000000000590559f6fb7720f24ff3e2fccf6015b466e9c92c0000000000000000000000000000000000000000000000000000000000989680000000000000000000000000000000000000000000000000000000000000000d000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001200000000000000000000000000000000000000000000000000000000000000648e8755f7ac30b5e4fa3f9c00e2cb6667501797b8bc01a7a367a4b2889ca6a05d9c31a31a781c12a4c3bdfc2ef1e02942e388b6565989ebe860bd67925bda74fbe0000000000000000000000000000000000000000000000000005ea0005bc00a007e5c0d200000000000000000000000000000000059800057e00018500009500001a4041c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2d0e30db00c20c02aaa39b223fe8d0a0e5c4f27ead9083c756cc27b73644935b8e68019ac6356c40661e1bc3158606ae4071118002dc6c07b73644935b8e68019ac6356c40661e1bc3158600000000000000000000000000000000000000000000000000294932ccadc9c58c02aaa39b223fe8d0a0e5c4f27ead9083c756cc251204dff5675ecff96b565ba3804dd4a63799ccba406761d38e5ddf6ccf6cf7c55759d5210750b5d60f30044e331d039000000000000000000000000761d38e5ddf6ccf6cf7c55759d5210750b5d60f3000000000000000000000000111111111117dc0aa78b770fa6a738034120c302000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002f8a744a79be00000000000000000000000042f527f50f16a103b6ccab48bccca214500c10210000000000000000000000005f515f6c524b18ca30f7783fb58dd4be2e9904ec00a0860a32ec00000000000000000000000000000000000000000000000000003005635d54300003d05120ead050515e10fdb3540ccd6f8236c46790508a76111111111117dc0aa78b770fa6a738034120c30200c4e525b10b000000000000000000000000000000000000000000000000000000000000002000000000000000000000000022b1a53ac4be63cdc1f47c99572290eff1edd8020000000000000000000000006a32cc044dd6359c27bb66e7b02dce6dd0fda2470000000000000000000000005f515f6c524b18ca30f7783fb58dd4be2e9904ec000000000000000000000000111111111117dc0aa78b770fa6a738034120c302000000000000000000000000dac17f958d2ee523a2206206994597c13d831ec7000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000003005635d5430000000000000000000000000000000000000000000000000000000000000000e0000000000000000000000000000000000000000000000000000000067138e8c00000000000000000000000000000000000000000000000000030fb9b1525d8185f8d63fbcbe42e5999263c349cb5d81000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000026000000000000000000000000067297ee4eb097e072b4ab6f1620268061ae8046400000000000000000000000060cba82ddbf4b5ddcd4398cdd05354c6a790c309000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002e0000000000000000000000000000000000000000000000000000000000000036000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000041d26038ef66344af785ff342b86db3da06c4cc6a62f0ca80ffd78affc0a95ccad44e814acebb1deda729bbfe3050bec14a47af487cc1cadc75f43db2d073016c31c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000041a66cd52a747c5f60b9db637ffe30d0e413ec87858101832b4c5c1ae154bf247f3717c8ed4133e276ddf68d43a827f280863c91d6c42bc6ad1ec7083b2315b6fd1c0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000020d6bdbf78dac17f958d2ee523a2206206994597c13d831ec780a06c4eca27dac17f958d2ee523a2206206994597c13d831ec7111111125421ca6dc452d289314280a0f8842a65000000000000000000000000000000000000000000000000c095c0a2",
            "value": "10000001",
            "gas": 721429,
            "gasPrice": "9525172167"
        },
        "srcToken": {
            "address": "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "symbol": ticker_coin,
            "name": "Ether",
            "decimals": 18,
            "eip2612": false,
            "isFoT": false,
            "logoURI": "https://tokens.1inch.io/0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee.png",
            "tags": [
                "crosschain",
                "GROUP:ETH",
                "native",
                "PEG:ETH"
            ]
        },
        "dstToken": {
            "address": "0x1234567890123456789012345678901234567890",
            "symbol": ticker_token,
            "name": "Just Token",
            "decimals": 6,
            "eip2612": false,
            "isFoT": false,
            "logoURI": "https://tokens.1inch.io/0x1234567890123456789012345678901234567890.png",
            "tags": [
                "crosschain",
                "GROUP:USDT",
                "PEG:USD",
                "tokens"
            ]
        },
        "protocols": [
        [
            [
            {
                "name": "UNISWAP_V2",
                "part": 100,
                "fromTokenAddress": "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                "toTokenAddress": "0x761d38e5ddf6ccf6cf7c55759d5210750b5d60f3"
            }
            ],
            [
            {
                "name": "ONE_INCH_LP_1_1",
                "part": 100,
                "fromTokenAddress": "0x761d38e5ddf6ccf6cf7c55759d5210750b5d60f3",
                "toTokenAddress": "0x111111111117dc0aa78b770fa6a738034120c302"
            }
            ],
            [
            {
                "name": "PMM11",
                "part": 100,
                "fromTokenAddress": "0x111111111117dc0aa78b770fa6a738034120c302",
                "toTokenAddress": "0xdac17f958d2ee523a2206206994597c13d831ec7"
            }
            ]
        ]
        ]
    });

    let quote_req = ClassicSwapQuoteRequest {
        base: ticker_coin.clone(),
        rel: ticker_token.clone(),
        amount: MmNumber::from("1.0"),
        opt_params: ClassicSwapQuoteOptParams {
            fee: None,
            protocols: None,
            gas_price: None,
            complexity_level: None,
            parts: None,
            main_route_parts: None,
            gas_limit: None,
            include_tokens_info: true,
            include_protocols: true,
            include_gas: true,
            connector_tokens: None,
        },
    };

    let create_req = ClassicSwapCreateRequest {
        base: ticker_coin.clone(),
        rel: ticker_token.clone(),
        amount: MmNumber::from("1.0"),
        slippage: 0.0,
        opt_params: ClassicSwapCreateOptParams {
            fee: None,
            protocols: None,
            gas_price: None,
            complexity_level: None,
            parts: None,
            main_route_parts: None,
            gas_limit: None,
            include_tokens_info: true,
            include_protocols: true,
            include_gas: true,
            connector_tokens: None,
            excluded_protocols: None,
            permit: None,
            compatibility: None,
            receiver: None,
            referrer: None,
            disable_estimate: None,
            allow_partial_fill: None,
            use_permit2: None,
        },
    };

    ApiClient::call_api::<ClassicSwapData>.mock_safe(move |_| {
        let response_quote_raw = response_quote_raw.clone();
        MockResult::Return(Box::pin(async move {
            Ok(serde_json::from_value::<ClassicSwapData>(response_quote_raw).unwrap())
        }))
    });

    let quote_response = block_on(one_inch_v6_0_classic_swap_quote_rpc(ctx.clone(), quote_req)).unwrap();
    assert_eq!(
        quote_response.dst_amount.amount,
        BigDecimal::from_str("0.000013").unwrap()
    );
    assert_eq!(quote_response.src_token.as_ref().unwrap().symbol, ticker_coin);
    assert_eq!(quote_response.src_token.as_ref().unwrap().decimals, 18);
    assert_eq!(quote_response.dst_token.as_ref().unwrap().symbol, ticker_token);
    assert_eq!(quote_response.dst_token.as_ref().unwrap().decimals, 6);
    assert_eq!(quote_response.gas.unwrap(), 452704_u64);

    ApiClient::call_api::<ClassicSwapData>.mock_safe(move |_| {
        let response_create_raw = response_create_raw.clone();
        MockResult::Return(Box::pin(async move {
            Ok(serde_json::from_value::<ClassicSwapData>(response_create_raw).unwrap())
        }))
    });
    let create_response = block_on(one_inch_v6_0_classic_swap_create_rpc(ctx, create_req)).unwrap();
    assert_eq!(
        create_response.dst_amount.amount,
        BigDecimal::from_str("0.000013").unwrap()
    );
    assert_eq!(create_response.src_token.as_ref().unwrap().symbol, ticker_coin);
    assert_eq!(create_response.src_token.as_ref().unwrap().decimals, 18);
    assert_eq!(create_response.dst_token.as_ref().unwrap().symbol, ticker_token);
    assert_eq!(create_response.dst_token.as_ref().unwrap().decimals, 6);
    assert_eq!(create_response.tx.as_ref().unwrap().data.len(), 1960);
    assert_eq!(
        create_response.tx.as_ref().unwrap().value,
        BigDecimal::from_str("0.000000000010000001").unwrap()
    );
}
