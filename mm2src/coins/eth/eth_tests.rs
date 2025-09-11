use super::*;
use crate::IguanaPrivKey;
use common::block_on;
use mm2_core::mm_ctx::MmCtxBuilder;

cfg_native!(
    use crate::eth::for_tests::{eth_coin_for_test, eth_coin_from_keypair};
    use crate::DexFee;

    use common::{now_sec, block_on_f01};
    use ethkey::{Generator, Random};
    use mm2_test_helpers::for_tests::{
        ETH_MAINNET_CHAIN_ID, ETH_MAINNET_NODES, ETH_SEPOLIA_CHAIN_ID, ETH_SEPOLIA_NODES, ETH_SEPOLIA_TOKEN_CONTRACT,
        eth_sepolia_conf,
    };
    use mocktopus::mocking::*;

    /// The gas price for the tests
    const GAS_PRICE: u64 = 50_000_000_000;
    /// `GAS_PRICE` increased by 3%
    const GAS_PRICE_APPROXIMATION_ON_START_SWAP: u64 = 51_500_000_000;
    /// `GAS_PRICE` increased by 5%
    const GAS_PRICE_APPROXIMATION_ON_ORDER_ISSUE: u64 = 52_500_000_000;
    /// `GAS_PRICE` increased by 7%
    const GAS_PRICE_APPROXIMATION_ON_TRADE_PREIMAGE: u64 = 53_500_000_000;
);

// old way to add some extra gas to the returned value from gas station (non-existent now), still used in tests
const GAS_PRICE_PERCENT: u64 = 10;
const MATIC_CHAIN_ID: u64 = 137;

const ETH: &str = "ETH";

fn check_sum(addr: &str, expected: &str) {
    let actual = checksum_address(addr);
    assert_eq!(expected, actual);
}

#[test]
/// https://github.com/ethereum/EIPs/blob/master/EIPS/eip-55.md#test-cases
fn test_check_sum_address() {
    check_sum(
        "0xfb6916095ca1df60bb79ce92ce3ea74c37c5d359",
        "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
    );
    check_sum(
        "0x52908400098527886e0f7030069857d2e4169ee7",
        "0x52908400098527886E0F7030069857D2E4169EE7",
    );
    check_sum(
        "0x8617e340b3d01fa5f11f306f4090fd50e238070d",
        "0x8617E340B3D01FA5F11F306F4090FD50E238070D",
    );
    check_sum(
        "0xde709f2102306220921060314715629080e2fb77",
        "0xde709f2102306220921060314715629080e2fb77",
    );
    check_sum(
        "0x27b1fdb04752bbc536007a920d24acb045561c26",
        "0x27b1fdb04752bbc536007a920d24acb045561c26",
    );
    check_sum(
        "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
        "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
    );
    check_sum(
        "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
        "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
    );
    check_sum(
        "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
        "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
    );
    check_sum(
        "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
        "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
    );
}

#[test]
fn test_is_valid_checksum_addr() {
    assert!(is_valid_checksum_addr("0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359"));
    assert!(is_valid_checksum_addr("0x52908400098527886E0F7030069857D2E4169EE7"));
    assert!(!is_valid_checksum_addr("0x8617e340B3D01FA5F11F306F4090FD50E238070D"));
    assert!(!is_valid_checksum_addr("0xd1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb"));
}

#[test]
fn display_u256_with_point() {
    let number = U256::from_dec_str("1000000000000000000").unwrap();
    let string = display_u256_with_decimal_point(number, 18);
    assert_eq!("1.", string);

    let number = U256::from_dec_str("10000000000000000000000000000000000000000000000000000000000").unwrap();
    let string = display_u256_with_decimal_point(number, 18);
    assert_eq!("10000000000000000000000000000000000000000.", string);

    let number = U256::from_dec_str("1234567890000000000").unwrap();
    let string = display_u256_with_decimal_point(number, 18);
    assert_eq!("1.23456789", string);

    let number = U256::from_dec_str("1234567890000000000").unwrap();
    let string = display_u256_with_decimal_point(number, 16);
    assert_eq!("123.456789", string);

    let number = U256::from_dec_str("1234567890000000000").unwrap();
    let string = display_u256_with_decimal_point(number, 0);
    assert_eq!("1234567890000000000.", string);

    let number = U256::from_dec_str("1000000000000000").unwrap();
    let string = display_u256_with_decimal_point(number, 18);
    assert_eq!("0.001", string);

    let number = U256::from_dec_str("0").unwrap();
    let string = display_u256_with_decimal_point(number, 18);
    assert_eq!("0.", string);

    let number = U256::from_dec_str("0").unwrap();
    let string = display_u256_with_decimal_point(number, 0);
    assert_eq!("0.", string);
}

#[test]
fn test_wei_from_big_decimal() {
    let amount = "0.000001".parse().unwrap();
    let wei = u256_from_big_decimal(&amount, 18).unwrap();
    let expected_wei: U256 = 1000000000000u64.into();
    assert_eq!(expected_wei, wei);

    let amount = "1.000001".parse().unwrap();
    let wei = u256_from_big_decimal(&amount, 18).unwrap();
    let expected_wei: U256 = 1000001000000000000u64.into();
    assert_eq!(expected_wei, wei);

    let amount = 1.into();
    let wei = u256_from_big_decimal(&amount, 18).unwrap();
    let expected_wei: U256 = 1000000000000000000u64.into();
    assert_eq!(expected_wei, wei);

    let amount = "0.000000000000000001".parse().unwrap();
    let wei = u256_from_big_decimal(&amount, 18).unwrap();
    let expected_wei: U256 = 1u64.into();
    assert_eq!(expected_wei, wei);

    let amount = 1234.into();
    let wei = u256_from_big_decimal(&amount, 9).unwrap();
    let expected_wei: U256 = 1234000000000u64.into();
    assert_eq!(expected_wei, wei);

    let amount = 1234.into();
    let wei = u256_from_big_decimal(&amount, 0).unwrap();
    let expected_wei: U256 = 1234u64.into();
    assert_eq!(expected_wei, wei);

    let amount = 1234.into();
    let wei = u256_from_big_decimal(&amount, 1).unwrap();
    let expected_wei: U256 = 12340u64.into();
    assert_eq!(expected_wei, wei);

    let amount = "1234.12345".parse().unwrap();
    let wei = u256_from_big_decimal(&amount, 1).unwrap();
    let expected_wei: U256 = 12341u64.into();
    assert_eq!(expected_wei, wei);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_wait_for_payment_spend_timeout() {
    use futures::future;

    const TAKER_PAYMENT_SPEND_SEARCH_INTERVAL: f64 = 1.;

    EthCoin::events_from_block.mock_safe(|_, _, _, _, _, _| MockResult::Return(Box::pin(future::ok(vec![]))));
    EthCoin::current_block.mock_safe(|_| MockResult::Return(Box::new(futures01::future::ok(900))));

    let key_pair = Random.generate().unwrap();
    let (_ctx, coin) = eth_coin_from_keypair(
        EthCoinType::Eth,
        ETH_SEPOLIA_NODES,
        None,
        key_pair,
        ETH_SEPOLIA_CHAIN_ID,
        eth_sepolia_conf(),
    );

    let wait_until = now_sec() - 1;
    let from_block = 1;
    // raw transaction bytes of https://etherscan.io/tx/0x0869be3e5d4456a29d488a533ad6c118620fef450f36778aecf31d356ff8b41f
    let tx_bytes = [
        248, 240, 3, 133, 1, 42, 5, 242, 0, 131, 2, 73, 240, 148, 133, 0, 175, 192, 188, 82, 20, 114, 128, 130, 22, 51,
        38, 194, 255, 12, 115, 244, 168, 113, 135, 110, 205, 245, 24, 127, 34, 254, 184, 132, 21, 44, 243, 175, 73, 33,
        143, 82, 117, 16, 110, 27, 133, 82, 200, 114, 233, 42, 140, 198, 35, 21, 201, 249, 187, 180, 20, 46, 148, 40,
        9, 228, 193, 130, 71, 199, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 152, 41, 132, 9, 201, 73, 19, 94, 237, 137, 35,
        61, 4, 194, 207, 239, 152, 75, 175, 245, 157, 174, 10, 214, 161, 207, 67, 70, 87, 246, 231, 212, 47, 216, 119,
        68, 237, 197, 125, 141, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 93, 72, 125, 102, 28, 159, 180, 237, 198, 97, 87, 80, 82, 200, 104, 40, 245,
        221, 7, 28, 122, 104, 91, 99, 1, 159, 140, 25, 131, 101, 74, 87, 50, 168, 146, 187, 90, 160, 51, 1, 123, 247,
        6, 108, 165, 181, 188, 40, 56, 47, 211, 229, 221, 73, 5, 15, 89, 81, 117, 225, 216, 108, 98, 226, 119, 232, 94,
        184, 42, 106,
    ];

    assert!(block_on(coin.wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
        tx_bytes: &tx_bytes,
        secret_hash: &[],
        wait_until,
        from_block,
        swap_contract_address: &coin.swap_contract_address(),
        check_every: TAKER_PAYMENT_SPEND_SEARCH_INTERVAL,
        watcher_reward: false
    }))
    .is_err());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_withdraw_impl_manual_fee() {
    let (_ctx, coin) = eth_coin_for_test(EthCoinType::Eth, &["http://dummy.dummy"], None, ETH_SEPOLIA_CHAIN_ID);

    EthCoin::address_balance.mock_safe(|_, _| {
        let balance = u256_from_big_decimal(&1000000000.into(), 18).unwrap();
        MockResult::Return(Box::new(futures01::future::ok(balance)))
    });
    EthCoin::get_addr_nonce.mock_safe(|_, _| MockResult::Return(Box::new(futures01::future::ok((0.into(), vec![])))));

    let withdraw_req = WithdrawRequest {
        amount: 1.into(),
        to: "0x7Bc1bBDD6A0a722fC9bffC49c921B685ECB84b94".to_string(),
        coin: ETH.to_string(),
        fee: Some(WithdrawFee::EthGas {
            gas: gas_limit::ETH_MAX_TRADE_GAS,
            gas_price: 1.into(),
        }),
        ..Default::default()
    };
    block_on_f01(coin.get_balance()).unwrap();

    let tx_details = block_on(withdraw_impl(coin, withdraw_req)).unwrap();
    let expected = Some(
        EthTxFeeDetails {
            coin: ETH.into(),
            gas_price: "0.000000001".parse().unwrap(),
            gas: gas_limit::ETH_MAX_TRADE_GAS,
            total_fee: "0.00015".parse().unwrap(),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        }
        .into(),
    );
    assert_eq!(expected, tx_details.fee_details);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_withdraw_impl_fee_details() {
    let (_ctx, coin) = eth_coin_for_test(
        EthCoinType::Erc20 {
            platform: ETH.to_string(),
            token_addr: Address::from_str(ETH_SEPOLIA_TOKEN_CONTRACT).unwrap(),
        },
        &["http://dummy.dummy"],
        None,
        ETH_SEPOLIA_CHAIN_ID,
    );

    EthCoin::address_balance.mock_safe(|_, _| {
        let balance = u256_from_big_decimal(&1000000000.into(), 18).unwrap();
        MockResult::Return(Box::new(futures01::future::ok(balance)))
    });
    EthCoin::get_addr_nonce.mock_safe(|_, _| MockResult::Return(Box::new(futures01::future::ok((0.into(), vec![])))));

    let withdraw_req = WithdrawRequest {
        amount: 1.into(),
        to: "0x7Bc1bBDD6A0a722fC9bffC49c921B685ECB84b94".to_string(),
        coin: "JST".to_string(),
        fee: Some(WithdrawFee::EthGas {
            gas: gas_limit::ETH_MAX_TRADE_GAS,
            gas_price: 1.into(),
        }),
        ..Default::default()
    };
    block_on_f01(coin.get_balance()).unwrap();

    let tx_details = block_on(withdraw_impl(coin, withdraw_req)).unwrap();
    let expected = Some(
        EthTxFeeDetails {
            coin: ETH.into(),
            gas_price: "0.000000001".parse().unwrap(),
            gas: gas_limit::ETH_MAX_TRADE_GAS,
            total_fee: "0.00015".parse().unwrap(),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        }
        .into(),
    );
    assert_eq!(expected, tx_details.fee_details);
}

#[test]
fn test_add_ten_pct() {
    let num = u256_from_big_decimal(&"0.1".parse().unwrap(), 9).unwrap();
    let expected = u256_from_big_decimal(&"0.11".parse().unwrap(), 9).unwrap();
    let actual = increase_by_percent(num, GAS_PRICE_PERCENT);
    assert_eq!(expected, actual);

    let num = u256_from_big_decimal(&"9.9".parse().unwrap(), 9).unwrap();
    let expected = u256_from_big_decimal(&"10.89".parse().unwrap(), 9).unwrap();
    let actual = increase_by_percent(num, GAS_PRICE_PERCENT);
    assert_eq!(expected, actual);

    let num = u256_from_big_decimal(&"30.1".parse().unwrap(), 9).unwrap();
    let expected = u256_from_big_decimal(&"33.11".parse().unwrap(), 9).unwrap();
    let actual = increase_by_percent(num, GAS_PRICE_PERCENT);
    assert_eq!(expected, actual);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn get_sender_trade_preimage() {
    /// Trade fee for the ETH coin is `2 * 150_000 * gas_price` always.
    fn expected_fee(gas_price: u64, gas_limit: u64) -> TradeFee {
        let amount = u256_to_big_decimal((gas_limit * gas_price).into(), 18).expect("!u256_to_big_decimal");
        TradeFee {
            coin: ETH.to_owned(),
            amount: amount.into(),
            paid_from_trading_vol: false,
        }
    }

    EthCoin::get_gas_price.mock_safe(|_| MockResult::Return(Box::pin(futures::future::ok(GAS_PRICE.into()))));

    let (_ctx, coin) = eth_coin_for_test(EthCoinType::Eth, &["http://dummy.dummy"], None, ETH_SEPOLIA_CHAIN_ID);

    let actual = block_on(coin.get_sender_trade_fee(
        TradePreimageValue::UpperBound(150.into()),
        FeeApproxStage::WithoutApprox,
    ))
    .expect("!get_sender_trade_fee");
    let expected = expected_fee(GAS_PRICE, gas_limit::ETH_PAYMENT);
    assert_eq!(actual, expected);

    let value = u256_to_big_decimal(100.into(), 18).expect("!u256_to_big_decimal");
    let actual = block_on(coin.get_sender_trade_fee(TradePreimageValue::Exact(value), FeeApproxStage::OrderIssue))
        .expect("!get_sender_trade_fee");
    let expected = expected_fee(GAS_PRICE_APPROXIMATION_ON_ORDER_ISSUE, gas_limit::ETH_PAYMENT);
    assert_eq!(actual, expected);

    let value = u256_to_big_decimal(1.into(), 18).expect("!u256_to_big_decimal");
    let actual = block_on(coin.get_sender_trade_fee(TradePreimageValue::Exact(value), FeeApproxStage::StartSwap))
        .expect("!get_sender_trade_fee");
    let expected = expected_fee(GAS_PRICE_APPROXIMATION_ON_START_SWAP, gas_limit::ETH_PAYMENT);
    assert_eq!(actual, expected);

    let value = u256_to_big_decimal(10000000000u64.into(), 18).expect("!u256_to_big_decimal");
    let actual =
        block_on(coin.get_sender_trade_fee(TradePreimageValue::Exact(value), FeeApproxStage::TradePreimageMax))
            .expect("!get_sender_trade_fee");
    let expected = expected_fee(
        GAS_PRICE_APPROXIMATION_ON_TRADE_PREIMAGE,
        gas_limit::ETH_PAYMENT + gas_limit::ETH_SENDER_REFUND,
    );
    assert_eq!(actual, expected);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn get_erc20_sender_trade_preimage() {
    const APPROVE_GAS_LIMIT: u64 = 60_000;
    static ALLOWANCE: AtomicU64 = AtomicU64::new(0);
    static ESTIMATE_GAS_CALLED: AtomicBool = AtomicBool::new(false);

    EthCoin::allowance.mock_safe(|_, _| {
        MockResult::Return(Box::new(futures01::future::ok(
            ALLOWANCE.load(AtomicOrdering::Relaxed).into(),
        )))
    });

    EthCoin::get_gas_price.mock_safe(|_| MockResult::Return(Box::pin(futures::future::ok(GAS_PRICE.into()))));
    EthCoin::estimate_gas_wrapper.mock_safe(|_, _| {
        ESTIMATE_GAS_CALLED.store(true, AtomicOrdering::Relaxed);
        MockResult::Return(Box::new(futures01::future::ok(APPROVE_GAS_LIMIT.into())))
    });

    fn expected_trade_fee(gas_limit: u64, gas_price: u64) -> TradeFee {
        let amount = u256_to_big_decimal((gas_limit * gas_price).into(), 18).expect("!u256_to_big_decimal");
        TradeFee {
            coin: ETH.to_owned(),
            amount: amount.into(),
            paid_from_trading_vol: false,
        }
    }

    let (_ctx, coin) = eth_coin_for_test(
        EthCoinType::Erc20 {
            platform: ETH.to_string(),
            token_addr: Address::default(),
        },
        &["http://dummy.dummy"],
        None,
        ETH_SEPOLIA_CHAIN_ID,
    );

    // value is allowed
    ALLOWANCE.store(1000, AtomicOrdering::Relaxed);
    let value = u256_to_big_decimal(1000.into(), 18).expect("u256_to_big_decimal");
    let actual =
        block_on(coin.get_sender_trade_fee(TradePreimageValue::UpperBound(value), FeeApproxStage::WithoutApprox))
            .expect("!get_sender_trade_fee");
    log!("{:?}", actual.amount.to_decimal());
    assert!(!ESTIMATE_GAS_CALLED.load(AtomicOrdering::Relaxed));
    assert_eq!(actual, expected_trade_fee(gas_limit::ERC20_PAYMENT, GAS_PRICE));

    // value is greater than allowance
    ALLOWANCE.store(999, AtomicOrdering::Relaxed);
    let value = u256_to_big_decimal(1000.into(), 18).expect("u256_to_big_decimal");
    let actual = block_on(coin.get_sender_trade_fee(TradePreimageValue::UpperBound(value), FeeApproxStage::StartSwap))
        .expect("!get_sender_trade_fee");

    assert!(ESTIMATE_GAS_CALLED.load(AtomicOrdering::Relaxed));
    ESTIMATE_GAS_CALLED.store(false, AtomicOrdering::Relaxed);

    assert_eq!(
        actual,
        expected_trade_fee(
            gas_limit::ERC20_PAYMENT + APPROVE_GAS_LIMIT,
            GAS_PRICE_APPROXIMATION_ON_START_SWAP
        )
    );

    // value is allowed
    ALLOWANCE.store(1000, AtomicOrdering::Relaxed);
    let value = u256_to_big_decimal(999.into(), 18).expect("u256_to_big_decimal");
    let actual = block_on(coin.get_sender_trade_fee(TradePreimageValue::Exact(value), FeeApproxStage::OrderIssue))
        .expect("!get_sender_trade_fee");

    assert!(!ESTIMATE_GAS_CALLED.load(AtomicOrdering::Relaxed));

    assert_eq!(
        actual,
        expected_trade_fee(gas_limit::ERC20_PAYMENT, GAS_PRICE_APPROXIMATION_ON_ORDER_ISSUE)
    );

    // value is greater than allowance
    ALLOWANCE.store(1000, AtomicOrdering::Relaxed);
    let value = u256_to_big_decimal(1500.into(), 18).expect("u256_to_big_decimal");
    let actual = block_on(coin.get_sender_trade_fee(TradePreimageValue::Exact(value), FeeApproxStage::TradePreimage))
        .expect("!get_sender_trade_fee");

    assert!(ESTIMATE_GAS_CALLED.load(AtomicOrdering::Relaxed));
    ESTIMATE_GAS_CALLED.store(false, AtomicOrdering::Relaxed);

    assert_eq!(
        actual,
        expected_trade_fee(
            gas_limit::ERC20_PAYMENT + gas_limit::ERC20_SENDER_REFUND + APPROVE_GAS_LIMIT,
            GAS_PRICE_APPROXIMATION_ON_TRADE_PREIMAGE
        )
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn get_receiver_trade_preimage() {
    EthCoin::get_gas_price.mock_safe(|_| MockResult::Return(Box::pin(futures::future::ok(GAS_PRICE.into()))));

    let (_ctx, coin) = eth_coin_for_test(EthCoinType::Eth, &["http://dummy.dummy"], None, ETH_SEPOLIA_CHAIN_ID);
    let amount =
        u256_to_big_decimal((gas_limit::ETH_RECEIVER_SPEND * GAS_PRICE).into(), 18).expect("!u256_to_big_decimal");
    let expected_fee = TradeFee {
        coin: ETH.to_owned(),
        amount: amount.into(),
        paid_from_trading_vol: false,
    };

    let actual =
        block_on_f01(coin.get_receiver_trade_fee(FeeApproxStage::WithoutApprox)).expect("!get_sender_trade_fee");
    assert_eq!(actual, expected_fee);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_get_fee_to_send_taker_fee() {
    const DEX_FEE_AMOUNT: u64 = 100_000;
    const TRANSFER_GAS_LIMIT: u64 = 40_000;

    EthCoin::get_gas_price.mock_safe(|_| MockResult::Return(Box::pin(futures::future::ok(GAS_PRICE.into()))));
    EthCoin::estimate_gas_wrapper
        .mock_safe(|_, _| MockResult::Return(Box::new(futures01::future::ok(TRANSFER_GAS_LIMIT.into()))));

    // fee to send taker fee is `TRANSFER_GAS_LIMIT * gas_price` always.
    let amount = u256_to_big_decimal((TRANSFER_GAS_LIMIT * GAS_PRICE).into(), 18).expect("!u256_to_big_decimal");
    let expected_fee = TradeFee {
        coin: ETH.to_owned(),
        amount: amount.into(),
        paid_from_trading_vol: false,
    };

    let dex_fee_amount = u256_to_big_decimal(DEX_FEE_AMOUNT.into(), 18).expect("!u256_to_big_decimal");

    let (_ctx, coin) = eth_coin_for_test(EthCoinType::Eth, &["http://dummy.dummy"], None, ETH_SEPOLIA_CHAIN_ID);
    let actual = block_on(coin.get_fee_to_send_taker_fee(
        DexFee::Standard(MmNumber::from(&dex_fee_amount)),
        FeeApproxStage::WithoutApprox,
    ))
    .expect("!get_fee_to_send_taker_fee");
    assert_eq!(actual, expected_fee);

    let (_ctx, coin) = eth_coin_for_test(
        EthCoinType::Erc20 {
            platform: ETH.to_string(),
            token_addr: Address::from_str("0xaD22f63404f7305e4713CcBd4F296f34770513f4").unwrap(),
        },
        &["http://dummy.dummy"],
        None,
        ETH_SEPOLIA_CHAIN_ID,
    );
    let actual = block_on(coin.get_fee_to_send_taker_fee(
        DexFee::Standard(MmNumber::from(dex_fee_amount)),
        FeeApproxStage::WithoutApprox,
    ))
    .expect("!get_fee_to_send_taker_fee");
    assert_eq!(actual, expected_fee);
}

/// Some ERC20 tokens return the `error: -32016, message: \"The execution failed due to an exception.\"` error
/// if the balance is insufficient.
/// So [`EthCoin::get_fee_to_send_taker_fee`] must return [`TradePreimageError::NotSufficientBalance`].
///
/// Please note this test doesn't work correctly now,
/// because as of now [`EthCoin::get_fee_to_send_taker_fee`] doesn't process the `Exception` web3 error correctly.
#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore]
fn test_get_fee_to_send_taker_fee_insufficient_balance() {
    const DEX_FEE_AMOUNT: u64 = 100_000_000_000;

    EthCoin::get_gas_price.mock_safe(|_| MockResult::Return(Box::pin(futures::future::ok(40.into()))));
    let (_ctx, coin) = eth_coin_for_test(
        EthCoinType::Erc20 {
            platform: ETH.to_string(),
            token_addr: Address::from_str("0xaD22f63404f7305e4713CcBd4F296f34770513f4").unwrap(),
        },
        ETH_MAINNET_NODES,
        None,
        ETH_MAINNET_CHAIN_ID,
    );
    let dex_fee_amount = u256_to_big_decimal(DEX_FEE_AMOUNT.into(), 18).expect("!u256_to_big_decimal");

    let error = block_on(coin.get_fee_to_send_taker_fee(
        DexFee::Standard(MmNumber::from(dex_fee_amount)),
        FeeApproxStage::WithoutApprox,
    ))
    .unwrap_err();
    log!("{}", error);
    assert!(
        matches!(error.get_inner(), TradePreimageError::NotSufficientBalance { .. }),
        "Expected TradePreimageError::NotSufficientBalance"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn validate_dex_fee_invalid_sender_eth() {
    let (_ctx, coin) = eth_coin_for_test(EthCoinType::Eth, ETH_MAINNET_NODES, None, ETH_MAINNET_CHAIN_ID);
    // the real dex fee sent on mainnet
    // https://etherscan.io/tx/0x7e9ca16c85efd04ee5e31f2c1914b48f5606d6f9ce96ecce8c96d47d6857278f
    let tx = block_on(block_on(coin.web3()).unwrap().eth().transaction(TransactionId::Hash(
        H256::from_str("0x7e9ca16c85efd04ee5e31f2c1914b48f5606d6f9ce96ecce8c96d47d6857278f").unwrap(),
    )))
    .unwrap()
    .unwrap();
    let tx = signed_tx_from_web3_tx(tx).unwrap().into();
    let amount: BigDecimal = "0.000526435076465".parse().unwrap();
    let validate_fee_args = ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: &DEX_FEE_ADDR_RAW_PUBKEY,
        dex_fee: &DexFee::Standard(amount.into()),
        min_block_number: 0,
        uuid: &[],
    };
    let error = block_on(coin.validate_fee(validate_fee_args)).unwrap_err().into_inner();
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("was sent from wrong address")),
        _ => panic!("Expected `WrongPaymentTx` wrong sender address, found {:?}", error),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn validate_dex_fee_invalid_sender_erc() {
    let (_ctx, coin) = eth_coin_for_test(
        EthCoinType::Erc20 {
            platform: ETH.to_string(),
            token_addr: Address::from_str("0xa1d6df714f91debf4e0802a542e13067f31b8262").unwrap(),
        },
        ETH_MAINNET_NODES,
        None,
        ETH_MAINNET_CHAIN_ID,
    );
    // the real dex fee sent on mainnet
    // https://etherscan.io/tx/0xd6403b41c79f9c9e9c83c03d920ee1735e7854d85d94cef48d95dfeca95cd600
    let tx = block_on(block_on(coin.web3()).unwrap().eth().transaction(TransactionId::Hash(
        H256::from_str("0xd6403b41c79f9c9e9c83c03d920ee1735e7854d85d94cef48d95dfeca95cd600").unwrap(),
    )))
    .unwrap()
    .unwrap();
    let tx = signed_tx_from_web3_tx(tx).unwrap().into();
    let amount: BigDecimal = "5.548262548262548262".parse().unwrap();
    let validate_fee_args = ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: &DEX_FEE_ADDR_RAW_PUBKEY,
        dex_fee: &DexFee::Standard(amount.into()),
        min_block_number: 0,
        uuid: &[],
    };
    let error = block_on(coin.validate_fee(validate_fee_args)).unwrap_err().into_inner();
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("was sent from wrong address")),
        _ => panic!("Expected `WrongPaymentTx` wrong sender address, found {:?}", error),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn sender_compressed_pub(tx: &SignedEthTx) -> [u8; 33] {
    let tx_pubkey = tx.public.unwrap();
    let mut raw_pubkey = [0; 65];
    raw_pubkey[0] = 0x04;
    raw_pubkey[1..].copy_from_slice(tx_pubkey.as_bytes());
    let secp_public = PublicKey::from_slice(&raw_pubkey).unwrap();
    secp_public.serialize()
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn validate_dex_fee_eth_confirmed_before_min_block() {
    let (_ctx, coin) = eth_coin_for_test(EthCoinType::Eth, ETH_MAINNET_NODES, None, ETH_MAINNET_CHAIN_ID);
    // the real dex fee sent on mainnet
    // https://etherscan.io/tx/0x7e9ca16c85efd04ee5e31f2c1914b48f5606d6f9ce96ecce8c96d47d6857278f
    let tx = block_on(block_on(coin.web3()).unwrap().eth().transaction(TransactionId::Hash(
        H256::from_str("0x7e9ca16c85efd04ee5e31f2c1914b48f5606d6f9ce96ecce8c96d47d6857278f").unwrap(),
    )))
    .unwrap()
    .unwrap();
    let tx = signed_tx_from_web3_tx(tx).unwrap();
    let compressed_public = sender_compressed_pub(&tx);
    let tx = tx.into();
    let amount: BigDecimal = "0.000526435076465".parse().unwrap();
    let validate_fee_args = ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: &compressed_public,
        dex_fee: &DexFee::Standard(amount.into()),
        min_block_number: 11784793,
        uuid: &[],
    };
    let error = block_on(coin.validate_fee(validate_fee_args)).unwrap_err().into_inner();
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("confirmed before min_block")),
        _ => panic!("Expected `WrongPaymentTx` early confirmation, found {:?}", error),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn validate_dex_fee_erc_confirmed_before_min_block() {
    let (_ctx, coin) = eth_coin_for_test(
        EthCoinType::Erc20 {
            platform: ETH.to_string(),
            token_addr: Address::from_str("0xa1d6df714f91debf4e0802a542e13067f31b8262").unwrap(),
        },
        ETH_MAINNET_NODES,
        None,
        ETH_MAINNET_CHAIN_ID,
    );
    // the real dex fee sent on mainnet
    // https://etherscan.io/tx/0xd6403b41c79f9c9e9c83c03d920ee1735e7854d85d94cef48d95dfeca95cd600
    let tx = block_on(block_on(coin.web3()).unwrap().eth().transaction(TransactionId::Hash(
        H256::from_str("0xd6403b41c79f9c9e9c83c03d920ee1735e7854d85d94cef48d95dfeca95cd600").unwrap(),
    )))
    .unwrap()
    .unwrap();

    let tx = signed_tx_from_web3_tx(tx).unwrap();
    let compressed_public = sender_compressed_pub(&tx);
    let tx = tx.into();
    let amount: BigDecimal = "5.548262548262548262".parse().unwrap();
    let validate_fee_args = ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: &compressed_public,
        dex_fee: &DexFee::Standard(amount.into()),
        min_block_number: 11823975,
        uuid: &[],
    };
    let error = block_on(coin.validate_fee(validate_fee_args)).unwrap_err().into_inner();
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("confirmed before min_block")),
        _ => panic!("Expected `WrongPaymentTx` early confirmation, found {:?}", error),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_negotiate_swap_contract_addr_no_fallback() {
    let (_, coin) = eth_coin_for_test(EthCoinType::Eth, ETH_MAINNET_NODES, None, ETH_MAINNET_CHAIN_ID);

    let input = None;
    let error = coin.negotiate_swap_contract_addr(input).unwrap_err().into_inner();
    assert_eq!(NegotiateSwapContractAddrErr::NoOtherAddrAndNoFallback, error);

    let slice: &[u8] = &[1; 1];
    let error = coin.negotiate_swap_contract_addr(Some(slice)).unwrap_err().into_inner();
    assert_eq!(
        NegotiateSwapContractAddrErr::InvalidOtherAddrLen(slice.to_vec().into()),
        error
    );

    let slice: &[u8] = &[1; 20];
    let error = coin.negotiate_swap_contract_addr(Some(slice)).unwrap_err().into_inner();
    assert_eq!(
        NegotiateSwapContractAddrErr::UnexpectedOtherAddr(slice.to_vec().into()),
        error
    );

    let slice: &[u8] = coin.swap_contract_address.as_ref();
    let result = coin.negotiate_swap_contract_addr(Some(slice)).unwrap();
    assert_eq!(Some(slice.to_vec().into()), result);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_negotiate_swap_contract_addr_has_fallback() {
    let fallback = Address::from_str("0x8500AFc0bc5214728082163326C2FF0C73f4a871").unwrap();

    let (_, coin) = eth_coin_for_test(
        EthCoinType::Eth,
        ETH_MAINNET_NODES,
        Some(fallback),
        ETH_MAINNET_CHAIN_ID,
    );

    let input = None;
    let result = coin.negotiate_swap_contract_addr(input).unwrap();
    assert_eq!(Some(fallback.0.to_vec().into()), result);

    let slice: &[u8] = &[1; 1];
    let error = coin.negotiate_swap_contract_addr(Some(slice)).unwrap_err().into_inner();
    assert_eq!(
        NegotiateSwapContractAddrErr::InvalidOtherAddrLen(slice.to_vec().into()),
        error
    );

    let slice: &[u8] = &[1; 20];
    let error = coin.negotiate_swap_contract_addr(Some(slice)).unwrap_err().into_inner();
    assert_eq!(
        NegotiateSwapContractAddrErr::UnexpectedOtherAddr(slice.to_vec().into()),
        error
    );

    let slice: &[u8] = coin.swap_contract_address.as_ref();
    let result = coin.negotiate_swap_contract_addr(Some(slice)).unwrap();
    assert_eq!(Some(slice.to_vec().into()), result);

    let slice: &[u8] = fallback.as_ref();
    let result = coin.negotiate_swap_contract_addr(Some(slice)).unwrap();
    assert_eq!(Some(fallback.0.to_vec().into()), result);
}

#[test]
#[ignore]
fn polygon_check_if_my_payment_sent() {
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let conf = json!({
      "coin": "MATIC",
      "name": "matic",
      "fname": "Polygon",
      "rpcport": 80,
      "mm2": 1,
      "avg_blocktime": 0.03,
      "required_confirmations": 3,
      "protocol": {
        "type": "ETH",
        "protocol_data": {
            "chain_id": MATIC_CHAIN_ID
        }
      }
    });

    let request = json!({
        "method": "enable",
        "coin": "MATIC",
        "urls": ["https://polygon-mainnet.g.alchemy.com/v2/9YYl6iMLmXXLoflMPHnMTC4Dcm2L2tFH"],
        "swap_contract_address": "0x9130b257d37a52e52f21054c4da3450c72f595ce",
    });

    let priv_key_policy = PrivKeyBuildPolicy::IguanaPrivKey(IguanaPrivKey::from([1; 32]));
    let coin = block_on(eth_coin_from_conf_and_request(
        &ctx,
        "MATIC",
        &conf,
        &request,
        CoinProtocol::ETH {
            chain_id: MATIC_CHAIN_ID,
        },
        priv_key_policy,
    ))
    .unwrap();

    log!("{}", coin.my_address().unwrap());

    let secret_hash = hex::decode("fc33114b389f0ee1212abf2867e99e89126f4860").unwrap();
    let swap_contract_address = "9130b257d37a52e52f21054c4da3450c72f595ce".into();
    let if_my_payment_sent_args = CheckIfMyPaymentSentArgs {
        time_lock: 1638764369,
        other_pub: &[],
        secret_hash: &secret_hash,
        search_from_block: 22185109,
        swap_contract_address: &Some(swap_contract_address),
        swap_unique_data: &[],
        amount: &BigDecimal::default(),
        payment_instructions: &None,
    };
    let my_payment = block_on(coin.check_if_my_payment_sent(if_my_payment_sent_args))
        .unwrap()
        .unwrap();
    let expected_hash = BytesJson::from("69a20008cea0c15ee483b5bbdff942752634aa072dfd2ff715fe87eec302de11");
    assert_eq!(expected_hash, my_payment.tx_hash_as_bytes());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_message_hash() {
    let key_pair = Random.generate().unwrap();
    let (_ctx, coin) = eth_coin_from_keypair(
        EthCoinType::Eth,
        ETH_SEPOLIA_NODES,
        None,
        key_pair,
        ETH_SEPOLIA_CHAIN_ID,
        eth_sepolia_conf(),
    );

    let message_hash = coin.sign_message_hash("test").unwrap();
    assert_eq!(
        hex::encode(message_hash),
        "4a5c5d454721bbbb25540c3317521e71c373ae36458f960d2ad46ef088110e95"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_sign_verify_message() {
    let key_pair = KeyPair::from_secret_slice(
        &hex::decode("809465b17d0a4ddb3e4c69e8f23c2cabad868f51f8bed5c765ad1d6516c3306f").unwrap(),
    )
    .unwrap();
    let (_ctx, coin) = eth_coin_from_keypair(
        EthCoinType::Eth,
        ETH_SEPOLIA_NODES,
        None,
        key_pair,
        ETH_SEPOLIA_CHAIN_ID,
        eth_sepolia_conf(),
    );

    let message = "test";
    let signature = coin.sign_message(message, None).unwrap();
    assert_eq!(signature, "0xcdf11a9c4591fb7334daa4b21494a2590d3f7de41c7d2b333a5b61ca59da9b311b492374cc0ba4fbae53933260fa4b1c18f15d95b694629a7b0620eec77a938600");

    let is_valid = coin
        .verify_message(&signature, message, "0xbAB36286672fbdc7B250804bf6D14Be0dF69fa29")
        .unwrap();
    assert!(is_valid);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_eth_extract_secret() {
    let key_pair = Random.generate().unwrap();
    let coin_type = EthCoinType::Erc20 {
        platform: ETH.to_string(),
        token_addr: Address::from_str("0xc0eb7aed740e1796992a08962c15661bdeb58003").unwrap(),
    };
    let (_ctx, coin) = eth_coin_from_keypair(
        coin_type,
        &["http://dummy.dummy"],
        None,
        key_pair,
        ETH_SEPOLIA_CHAIN_ID,
        eth_sepolia_conf(),
    );

    // raw transaction bytes of https://ropsten.etherscan.io/tx/0xcb7c14d3ff309996d582400369393b6fa42314c52245115d4a3f77f072c36da9
    let tx_bytes = &[
        249, 1, 9, 37, 132, 119, 53, 148, 0, 131, 2, 73, 240, 148, 123, 193, 187, 221, 106, 10, 114, 47, 201, 191, 252,
        73, 201, 33, 182, 133, 236, 184, 75, 148, 128, 184, 164, 2, 237, 41, 43, 188, 96, 248, 252, 165, 132, 81, 30,
        243, 34, 85, 165, 46, 224, 176, 90, 137, 30, 19, 123, 224, 67, 83, 53, 74, 57, 148, 140, 95, 45, 70, 147, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 71, 13, 228, 223, 130, 0, 0, 168, 151, 11,
        232, 224, 253, 63, 180, 26, 114, 23, 184, 27, 10, 161, 80, 178, 251, 73, 204, 80, 174, 97, 118, 149, 204, 186,
        187, 243, 185, 19, 128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 157, 73, 251, 238, 138, 245, 142, 240, 85, 44, 209, 63, 194, 242,
        109, 242, 246, 6, 76, 176, 27, 160, 29, 157, 226, 23, 81, 174, 34, 82, 93, 182, 41, 248, 119, 42, 221, 214, 38,
        243, 128, 2, 235, 208, 193, 192, 74, 208, 242, 26, 221, 83, 54, 74, 160, 111, 29, 92, 8, 75, 61, 97, 103, 199,
        100, 189, 72, 74, 221, 144, 66, 170, 68, 121, 29, 105, 19, 194, 35, 245, 196, 131, 236, 29, 105, 101, 30,
    ];

    let secret = block_on(coin.extract_secret(&[0u8; 20], tx_bytes.as_slice(), false));
    assert!(secret.is_ok());
    let expect_secret = &[
        168, 151, 11, 232, 224, 253, 63, 180, 26, 114, 23, 184, 27, 10, 161, 80, 178, 251, 73, 204, 80, 174, 97, 118,
        149, 204, 186, 187, 243, 185, 19, 128,
    ];
    assert_eq!(expect_secret.as_slice(), &secret.unwrap());

    // Test for unexpected contract signature
    // raw transaction bytes of ethPayment contract https://etherscan
    // .io/tx/0x0869be3e5d4456a29d488a533ad6c118620fef450f36778aecf31d356ff8b41f
    let tx_bytes = [
        248, 240, 3, 133, 1, 42, 5, 242, 0, 131, 2, 73, 240, 148, 133, 0, 175, 192, 188, 82, 20, 114, 128, 130, 22, 51,
        38, 194, 255, 12, 115, 244, 168, 113, 135, 110, 205, 245, 24, 127, 34, 254, 184, 132, 21, 44, 243, 175, 73, 33,
        143, 82, 117, 16, 110, 27, 133, 82, 200, 114, 233, 42, 140, 198, 35, 21, 201, 249, 187, 180, 20, 46, 148, 40,
        9, 228, 193, 130, 71, 199, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 152, 41, 132, 9, 201, 73, 19, 94, 237, 137, 35,
        61, 4, 194, 207, 239, 152, 75, 175, 245, 157, 174, 10, 214, 161, 207, 67, 70, 87, 246, 231, 212, 47, 216, 119,
        68, 237, 197, 125, 141, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 93, 72, 125, 102, 28, 159, 180, 237, 198, 97, 87, 80, 82, 200, 104, 40, 245,
        221, 7, 28, 122, 104, 91, 99, 1, 159, 140, 25, 131, 101, 74, 87, 50, 168, 146, 187, 90, 160, 51, 1, 123, 247,
        6, 108, 165, 181, 188, 40, 56, 47, 211, 229, 221, 73, 5, 15, 89, 81, 117, 225, 216, 108, 98, 226, 119, 232, 94,
        184, 42, 106,
    ];
    let secret = block_on(coin.extract_secret(&[0u8; 20], tx_bytes.as_slice(), false))
        .err()
        .unwrap();
    assert!(secret.contains("Expected 'receiverSpend' contract call signature"));
}

#[test]
fn test_eth_validate_valid_and_invalid_pubkey() {
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let conf = json!({
      "coin": "MATIC",
      "name": "matic",
      "fname": "Polygon",
      "rpcport": 80,
      "mm2": 1,
      "avg_blocktime": 0.03,
      "required_confirmations": 3,
      "protocol": {
        "type": "ETH",
        "protocol_data": {
            "chain_id": MATIC_CHAIN_ID
        }
      }
    });

    let request = json!({
        "method": "enable",
        "coin": "MATIC",
        "urls": ["https://polygon-mainnet.g.alchemy.com/v2/9YYl6iMLmXXLoflMPHnMTC4Dcm2L2tFH"],
        "swap_contract_address": "0x9130b257d37a52e52f21054c4da3450c72f595ce",
    });

    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let priv_key_policy = PrivKeyBuildPolicy::IguanaPrivKey(IguanaPrivKey::from(priv_key));
    let coin = block_on(eth_coin_from_conf_and_request(
        &ctx,
        "MATIC",
        &conf,
        &request,
        CoinProtocol::ETH {
            chain_id: MATIC_CHAIN_ID,
        },
        priv_key_policy,
    ))
    .unwrap();
    // Test expected to pass at this point as we're using a valid pubkey to validate against a valid pubkey
    assert!(coin
        .validate_other_pubkey(&[
            3, 23, 183, 225, 206, 31, 159, 148, 195, 42, 67, 115, 146, 41, 248, 140, 11, 3, 51, 41, 111, 180, 110, 143,
            114, 134, 88, 73, 198, 174, 52, 184, 78
        ])
        .is_ok());
    // Test expected to fail at this point as we're using a valid pubkey to validate against an invalid pubkeys
    assert!(coin.validate_other_pubkey(&[1u8; 20]).is_err());
    assert!(coin.validate_other_pubkey(&[1u8; 8]).is_err());
}

#[test]
fn test_get_enabled_erc20_by_contract_and_platform() {
    use super::erc20::get_enabled_erc20_by_platform_and_contract;
    use crate::rpc_command::get_enabled_coins::get_enabled_coins_rpc;
    const BNB_TOKEN: &str = "1INCH-BEP20";
    const ETH_TOKEN: &str = "1INCH-ERC20";

    let conf = json!({
        "coins": [{
      "coin": "BNB",
      "name": "binancesmartchain",
      "fname": "Binance Coin",
      "avg_blocktime": 3,
      "rpcport": 80,
      "mm2": 1,
      "use_access_list": true,
      "max_eth_tx_type": 2,
      "required_confirmations": 3,
      "protocol": {
        "type": "ETH",
        "protocol_data": {
            "chain_id": 56
        }
      },
      "derivation_path": "m/44'/60'",
      "trezor_coin": "Binance Smart Chain",
      "links": {
        "homepage": "https://www.binance.org"
      }
    },{
      "coin": BNB_TOKEN,
      "name": "1inch_bep20",
      "fname": "1Inch",
      "rpcport": 80,
      "mm2": 1,
      "avg_blocktime": 3,
      "required_confirmations": 3,
      "protocol": {
        "type": "ERC20",
        "protocol_data": {
          "platform": "BNB",
          "contract_address": "0x111111111117dC0aa78b770fA6A738034120C302"
        }
      },
      "derivation_path": "m/44'/60'",
      "use_access_list": true,
      "max_eth_tx_type": 2,
      "gas_limit": {
          "eth_send_erc20": 60000,
          "erc20_payment": 110000,
          "erc20_receiver_spend": 85000,
          "erc20_sender_refund": 85000
      }
    },{
      "coin": "ETH",
      "name": "ethereum",
      "fname": "Ethereum",
      "rpcport": 80,
      "mm2": 1,
      "sign_message_prefix": "Ethereum Signed Message:\n",
      "required_confirmations": 3,
      "avg_blocktime": 15,
      "protocol": {
        "type": "ETH",
        "protocol_data": {
            "chain_id": 1
        }
      },
      "derivation_path": "m/44'/60'"
    },{
      "coin": ETH_TOKEN,
      "name": "1inch_erc20",
      "fname": "1Inch",
      "rpcport": 80,
      "mm2": 1,
      "avg_blocktime": 15,
      "required_confirmations": 3,
      "decimals": 18,
      "protocol": {
        "type": "ERC20",
        "protocol_data": {
          "platform": "ETH",
          "contract_address": "0x111111111117dC0aa78b770fA6A738034120C302"
        }
      },
      "derivation_path": "m/44'/60'"
    }]
    });

    let ctx = MmCtxBuilder::new().with_conf(conf).into_mm_arc();
    CryptoCtx::init_with_iguana_passphrase(
        ctx.clone(),
        "spice describe gravity federal blast come thank unfair canal monkey style afraid",
    )
    .unwrap();

    let req_bnb_token = json!({
        "urls":["https://bsc-dataseed1.binance.org","https://bsc-dataseed1.defibit.io"],
        "swap_contract_address":"0x9130b257d37a52e52f21054c4da3450c72f595ce",
    });
    block_on(lp_coininit(&ctx, BNB_TOKEN, &req_bnb_token)).unwrap();

    let req_eth_token = json!({
        "urls":["https://ethereum-rpc.publicnode.com", "https://eth.drpc.org"],
        "swap_contract_address":"0x9130b257d37a52e52f21054c4da3450c72f595ce",
    });
    block_on(lp_coininit(&ctx, ETH_TOKEN, &req_eth_token)).unwrap();

    let coins = block_on(get_enabled_coins_rpc(ctx.clone(), None)).unwrap();
    assert_eq!(coins.coins.len(), 2);

    let contract_address = Address::from_str("0x111111111117dC0aa78b770fA6A738034120C302").unwrap();
    let res = block_on(get_enabled_erc20_by_platform_and_contract(&ctx, ETH, &contract_address)).unwrap();
    assert!(res.is_some());
    assert_eq!(res.unwrap().platform_ticker(), ETH);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_fee_history() {
    use mm2_test_helpers::for_tests::ETH_SEPOLIA_NODES;

    let (_ctx, coin) = eth_coin_for_test(EthCoinType::Eth, ETH_SEPOLIA_NODES, None, ETH_SEPOLIA_CHAIN_ID);
    // check fee history without percentiles decoded okay
    let res = block_on(coin.eth_fee_history(U256::from(1u64), BlockNumber::Latest, &[]));
    assert!(res.is_ok());
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_gas_limit_conf() {
    use mm2_test_helpers::for_tests::ETH_SEPOLIA_SWAP_CONTRACT;

    let conf = json!({
        "coins": [{
            "coin": "ETH",
            "name": "ethereum",
            "fname": "Ethereum",
            "protocol":{
                "type": "ETH",
                "protocol_data": {
                    "chain_id": ETH_SEPOLIA_CHAIN_ID
                }
            },
            "rpcport": 80,
            "mm2": 1,
            "gas_limit": {
                "erc20_payment": 120000,
                "erc20_receiver_spend": 130000,
                "erc20_sender_refund": 110000
            }
        }]
    });

    let ctx = MmCtxBuilder::new().with_conf(conf).into_mm_arc();
    CryptoCtx::init_with_iguana_passphrase(ctx.clone(), "123456").unwrap();

    let req = json!({
        "urls":ETH_SEPOLIA_NODES,
        "swap_contract_address":ETH_SEPOLIA_SWAP_CONTRACT
    });
    let coin = block_on(lp_coininit(&ctx, ETH, &req)).unwrap();
    let eth_coin = match coin {
        MmCoinEnum::EthCoin(eth_coin) => eth_coin,
        _ => panic!("not eth coin"),
    };
    assert!(
        eth_coin.gas_limit.eth_send_coins == 21_000
            && eth_coin.gas_limit.erc20_payment == 120000
            && eth_coin.gas_limit.erc20_receiver_spend == 130000
            && eth_coin.gas_limit.erc20_sender_refund == 110000
            && eth_coin.gas_limit.eth_max_trade_gas == 150_000
    );
}

#[test]
fn test_h256_to_str() {
    let h = H256::from_str("5136701f11060010841c9708c3eb26f6606a070b8ae43f4b98b6d7b10a545258").unwrap();
    let b: BytesJson = h.0.to_vec().into();
    println!("H256=0x{b:02x}");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_eth_conf_params() {
    let key_pair = Random.generate().unwrap();
    let (_ctx, coin) = eth_coin_from_keypair(
        EthCoinType::Eth,
        ETH_SEPOLIA_NODES,
        None,
        key_pair,
        ETH_SEPOLIA_CHAIN_ID,
        json!({
            "coin": "ETH",
            "name": "ethereum",
            "derivation_path": "m/44'/60'",
            "protocol": {
                "type": "ETH",
                "protocol_data": {
                    "chain_id": ETH_SEPOLIA_CHAIN_ID,
                }
            },
            "swap_gas_fee_policy": "High",
            "max_eth_tx_type": 2,
            "estimate_gas_mult": 1.25,
            "gas_price_adjust": {
                "legacy_price_mult": 0.25,
                "base_fee_mult": [0.1, 0.2, 0.3],
                "priority_fee_mult": [0.4, 0.5, 0.6]
            },
            "gas_limit": {
                "eth_send_coins": 21_000,
                "eth_send_erc20": 120_123,
                "eth_payment": 75_456,
                "erc20_payment": 110_999,
                "eth_receiver_spend": 50_111,
                "erc20_receiver_spend":120_333,
                "eth_sender_refund": 65_222,
                "erc20_sender_refund": 135_777,
                "eth_max_trade_gas": 150_234,
            },
            "gas_limit_v2": {
                "maker": {
                    "eth_payment": 110_111,
                    "erc20_payment": 120_111,
                    "eth_taker_spend": 130_111,
                    "erc20_taker_spend": 140_111,
                    "eth_maker_refund_timelock": 105_111,
                    "erc20_maker_refund_timelock": 115_111,
                    "eth_maker_refund_secret": 125_111,
                    "erc20_maker_refund_secret": 135_111
                },
                "taker": {
                    "eth_payment": 110_222,
                    "erc20_payment": 111_222,
                    "eth_maker_spend": 112_222,
                    "erc20_maker_spend": 113_222,
                    "eth_taker_refund_timelock": 114_222,
                    "erc20_taker_refund_timelock": 115_222,
                    "eth_taker_refund_secret": 116_222,
                    "erc20_taker_refund_secret": 117_222,
                    "approve_payment": 118_222
                },
                "nft_maker": {
                    "erc721_payment": 120_333,
                    "erc1155_payment": 121_333,
                    "erc721_taker_spend": 122_333,
                    "erc1155_taker_spend": 123_333,
                    "erc721_maker_refund_timelock": 124_333,
                    "erc1155_maker_refund_timelock": 125_333,
                    "erc721_maker_refund_secret": 126_333,
                    "erc1155_maker_refund_secret": 127_333
                }
            }
        }),
    );
    assert_eq!(coin.max_eth_tx_type, Some(2));
    assert_eq!(coin.estimate_gas_mult, Some(1.25));
    assert_eq!(coin.gas_limit.eth_send_erc20, 120_123);
    assert_eq!(coin.gas_limit.eth_max_trade_gas, 150_234);
    assert_eq!(coin.gas_limit_v2.maker.eth_maker_refund_timelock, 105_111);
    assert_eq!(coin.gas_limit_v2.taker.eth_taker_refund_secret, 116_222);
    assert_eq!(coin.gas_limit_v2.nft_maker.erc1155_maker_refund_timelock, 125_333);
    assert_eq!(coin.gas_price_adjust.as_ref().unwrap().legacy_price_mult, 0.25);
    assert_eq!(coin.gas_price_adjust.as_ref().unwrap().base_fee_mult, [0.1, 0.2, 0.3]);
    assert_eq!(
        coin.gas_price_adjust.as_ref().unwrap().priority_fee_mult,
        [0.4, 0.5, 0.6]
    );
    assert!(matches!(
        *coin.swap_gas_fee_policy.lock().unwrap(),
        SwapGasFeePolicy::High
    ));
}
