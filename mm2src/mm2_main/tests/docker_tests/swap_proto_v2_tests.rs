use crate::{generate_utxo_coin_with_random_privkey, MYCOIN, MYCOIN1, SET_BURN_PUBKEY_TO_ALICE};
use bitcrypto::dhash160;
use coins::utxo::UtxoCommonOps;
use coins::{
    ConfirmPaymentInput, DexFee, FundingTxSpend, GenTakerFundingSpendArgs, GenTakerPaymentSpendArgs,
    MakerCoinSwapOpsV2, MarketCoinOps, ParseCoinAssocTypes, RefundFundingSecretArgs, RefundMakerPaymentSecretArgs,
    RefundMakerPaymentTimelockArgs, RefundTakerPaymentArgs, SendMakerPaymentArgs, SendTakerFundingArgs,
    SwapTxTypeWithSecretHash, TakerCoinSwapOpsV2, Transaction, ValidateMakerPaymentArgs, ValidateTakerFundingArgs,
};
use crypto::privkey::key_pair_from_secret;
//use futures01::Future;
use common::{block_on, block_on_f01, now_sec};
use mm2_number::MmNumber;
use mm2_test_helpers::for_tests::{
    active_swaps, check_recent_swaps, coins_needed_for_kickstart, disable_coin, disable_coin_err, enable_native,
    get_locked_amount, mm_dump, my_swap_status, mycoin1_conf, mycoin_conf, start_swaps, wait_for_swap_finished,
    wait_for_swap_status, MarketMakerIt, Mm2TestConf,
};
use mm2_test_helpers::structs::MmNumberMultiRepr;
use script::{Builder, Opcode};
use serialization::serialize;
use std::time::Duration;
use uuid::Uuid;

#[test]
fn send_and_refund_taker_funding_timelock() {
    let (_mm_arc, coin, _privkey) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());

    let funding_time_lock = now_sec() - 1000;
    let taker_secret_hash = &[0; 20];
    let maker_pub = coin.my_public_key().unwrap();
    let dex_fee = &DexFee::Standard("0.01".into());

    let send_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock: 0,
        taker_secret_hash,
        maker_secret_hash: &[0; 20],
        maker_pub,
        dex_fee,
        premium_amount: "0.1".parse().unwrap(),
        trading_amount: 1.into(),
        swap_unique_data: &[],
    };
    let taker_funding_utxo_tx = block_on(coin.send_taker_funding(send_args)).unwrap();
    log!("{:02x}", taker_funding_utxo_tx.tx_hash_as_bytes());
    // tx must have 3 outputs: actual funding, OP_RETURN containing the secret hash and change
    assert_eq!(3, taker_funding_utxo_tx.outputs.len());

    // dex_fee_amount + premium_amount + trading_amount
    let expected_amount = 111000000u64;
    assert_eq!(expected_amount, taker_funding_utxo_tx.outputs[0].value);

    let expected_op_return = Builder::default()
        .push_opcode(Opcode::OP_RETURN)
        .push_data(&[0; 20])
        .into_bytes();
    assert_eq!(expected_op_return, taker_funding_utxo_tx.outputs[1].script_pubkey);

    let validate_args = ValidateTakerFundingArgs {
        funding_tx: &taker_funding_utxo_tx,
        payment_time_lock: 0,
        funding_time_lock,
        taker_secret_hash,
        maker_secret_hash: &[],
        taker_pub: maker_pub,
        dex_fee,
        premium_amount: "0.1".parse().unwrap(),
        trading_amount: 1.into(),
        swap_unique_data: &[],
    };
    block_on(coin.validate_taker_funding(validate_args)).unwrap();

    let refund_args = RefundTakerPaymentArgs {
        payment_tx: &serialize(&taker_funding_utxo_tx).take(),
        time_lock: funding_time_lock,
        maker_pub: coin.my_public_key().unwrap(),
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerFunding {
            taker_secret_hash: &[0; 20],
        },
        swap_unique_data: &[],
        watcher_reward: false,
        dex_fee,
        premium_amount: Default::default(),
        trading_amount: Default::default(),
    };

    let refund_tx = block_on(coin.refund_taker_funding_timelock(refund_args)).unwrap();
    log!("{:02x}", refund_tx.tx_hash_as_bytes());

    // refund tx has to be confirmed before it can be found as payment spend in native mode
    let confirm_input = ConfirmPaymentInput {
        payment_tx: refund_tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 20,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_input)).unwrap();

    let found_refund_tx =
        block_on(coin.search_for_taker_funding_spend(&taker_funding_utxo_tx, 1, taker_secret_hash)).unwrap();
    match found_refund_tx {
        Some(FundingTxSpend::RefundedTimelock(found_tx)) => assert_eq!(found_tx, refund_tx),
        unexpected => panic!("Got unexpected FundingTxSpend variant {:?}", unexpected),
    }
}

#[test]
fn send_and_refund_taker_funding_secret() {
    let (_mm_arc, coin, _privkey) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());

    let funding_time_lock = now_sec() - 1000;
    let taker_secret = &[0; 32];
    let taker_secret_hash_owned = dhash160(taker_secret);
    let taker_secret_hash = taker_secret_hash_owned.as_slice();
    let maker_pub = coin.my_public_key().unwrap();
    let dex_fee = &DexFee::Standard("0.01".into());

    let send_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock: 0,
        taker_secret_hash,
        maker_secret_hash: &[0; 20],
        maker_pub,
        dex_fee,
        premium_amount: "0.1".parse().unwrap(),
        trading_amount: 1.into(),
        swap_unique_data: &[],
    };
    let taker_funding_utxo_tx = block_on(coin.send_taker_funding(send_args)).unwrap();
    log!("{:02x}", taker_funding_utxo_tx.tx_hash_as_bytes());
    // tx must have 3 outputs: actual funding, OP_RETURN containing the secret hash and change
    assert_eq!(3, taker_funding_utxo_tx.outputs.len());

    // dex_fee_amount + premium_amount + trading_amount
    let expected_amount = 111000000u64;
    assert_eq!(expected_amount, taker_funding_utxo_tx.outputs[0].value);

    let expected_op_return = Builder::default()
        .push_opcode(Opcode::OP_RETURN)
        .push_data(taker_secret_hash)
        .into_bytes();
    assert_eq!(expected_op_return, taker_funding_utxo_tx.outputs[1].script_pubkey);

    let validate_args = ValidateTakerFundingArgs {
        funding_tx: &taker_funding_utxo_tx,
        funding_time_lock,
        payment_time_lock: 0,
        taker_secret_hash,
        maker_secret_hash: &[],
        taker_pub: maker_pub,
        dex_fee,
        premium_amount: "0.1".parse().unwrap(),
        trading_amount: 1.into(),
        swap_unique_data: &[],
    };
    block_on(coin.validate_taker_funding(validate_args)).unwrap();

    let refund_args = RefundFundingSecretArgs {
        funding_tx: &taker_funding_utxo_tx,
        funding_time_lock,
        payment_time_lock: 0,
        maker_pubkey: maker_pub,
        taker_secret,
        taker_secret_hash,
        maker_secret_hash: &[],
        dex_fee,
        premium_amount: "0.1".parse().unwrap(),
        trading_amount: 1.into(),
        swap_unique_data: &[],
        watcher_reward: false,
    };

    let refund_tx = block_on(coin.refund_taker_funding_secret(refund_args)).unwrap();
    log!("{:02x}", refund_tx.tx_hash_as_bytes());

    // refund tx has to be confirmed before it can be found as payment spend in native mode
    let confirm_input = ConfirmPaymentInput {
        payment_tx: refund_tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 20,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_input)).unwrap();

    let found_refund_tx =
        block_on(coin.search_for_taker_funding_spend(&taker_funding_utxo_tx, 1, taker_secret_hash)).unwrap();
    match found_refund_tx {
        Some(FundingTxSpend::RefundedSecret { tx, secret }) => {
            assert_eq!(refund_tx, tx);
            assert_eq!(taker_secret, &secret);
        },
        unexpected => panic!("Got unexpected FundingTxSpend variant {:?}", unexpected),
    }
}

#[test]
fn send_and_spend_taker_funding() {
    let (_mm_arc, taker_coin, _privkey) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());
    let (_mm_arc, maker_coin, _privkey) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());

    let funding_time_lock = now_sec() - 1000;
    let taker_secret_hash = &[0; 20];

    let taker_pub = taker_coin.my_public_key().unwrap();
    let maker_pub = maker_coin.my_public_key().unwrap();

    let dex_fee = &DexFee::Standard("0.01".into());

    let send_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock: 0,
        taker_secret_hash,
        maker_secret_hash: &[0; 20],
        maker_pub,
        dex_fee,
        premium_amount: "0.1".parse().unwrap(),
        trading_amount: 1.into(),
        swap_unique_data: &[],
    };
    let taker_funding_utxo_tx = block_on(taker_coin.send_taker_funding(send_args)).unwrap();
    log!("Funding tx {:02x}", taker_funding_utxo_tx.tx_hash_as_bytes());
    // tx must have 3 outputs: actual funding, OP_RETURN containing the secret hash and change
    assert_eq!(3, taker_funding_utxo_tx.outputs.len());

    // dex_fee_amount + premium_amount + trading_amount
    let expected_amount = 111000000u64;
    assert_eq!(expected_amount, taker_funding_utxo_tx.outputs[0].value);

    let expected_op_return = Builder::default()
        .push_opcode(Opcode::OP_RETURN)
        .push_data(&[0; 20])
        .into_bytes();
    assert_eq!(expected_op_return, taker_funding_utxo_tx.outputs[1].script_pubkey);

    let validate_args = ValidateTakerFundingArgs {
        funding_tx: &taker_funding_utxo_tx,
        payment_time_lock: 0,
        funding_time_lock,
        taker_secret_hash,
        maker_secret_hash: &[],
        taker_pub,
        dex_fee,
        premium_amount: "0.1".parse().unwrap(),
        trading_amount: 1.into(),
        swap_unique_data: &[],
    };
    block_on(maker_coin.validate_taker_funding(validate_args)).unwrap();

    let preimage_args = GenTakerFundingSpendArgs {
        funding_tx: &taker_funding_utxo_tx,
        maker_pub,
        taker_pub,
        funding_time_lock,
        taker_secret_hash,
        taker_payment_time_lock: 0,
        maker_secret_hash: &[0; 20],
    };
    let preimage = block_on(maker_coin.gen_taker_funding_spend_preimage(&preimage_args, &[])).unwrap();

    let payment_tx = block_on(taker_coin.sign_and_send_taker_funding_spend(&preimage, &preimage_args, &[])).unwrap();
    log!("Taker payment tx {:02x}", payment_tx.tx_hash_as_bytes());

    // payment tx has to be confirmed before it can be found as payment spend in native mode
    let confirm_input = ConfirmPaymentInput {
        payment_tx: payment_tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 20,
        check_every: 1,
    };
    block_on_f01(taker_coin.wait_for_confirmations(confirm_input)).unwrap();

    let found_spend_tx =
        block_on(taker_coin.search_for_taker_funding_spend(&taker_funding_utxo_tx, 1, taker_secret_hash)).unwrap();
    match found_spend_tx {
        Some(FundingTxSpend::TransferredToTakerPayment(tx)) => {
            assert_eq!(payment_tx, tx);
        },
        unexpected => panic!("Got unexpected FundingTxSpend variant {:?}", unexpected),
    }
}

#[test]
fn send_and_spend_taker_payment_dex_fee_burn_kmd() {
    let (_mm_arc, taker_coin, _privkey) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());
    let (_mm_arc, maker_coin, _privkey) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());

    let funding_time_lock = now_sec() - 1000;
    let taker_secret_hash = &[0; 20];

    let maker_secret = &[1; 32];
    let maker_secret_hash_owned = dhash160(maker_secret);
    let maker_secret_hash = maker_secret_hash_owned.as_slice();

    let taker_pub = taker_coin.my_public_key().unwrap();
    let maker_pub = maker_coin.my_public_key().unwrap();

    let dex_fee = &DexFee::create_from_fields("0.75".into(), "0.25".into(), "KMD");

    let send_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock: 0,
        taker_secret_hash,
        maker_secret_hash,
        maker_pub,
        dex_fee,
        premium_amount: 0.into(),
        trading_amount: 777.into(),
        swap_unique_data: &[],
    };
    let taker_funding_utxo_tx = block_on(taker_coin.send_taker_funding(send_args)).unwrap();
    log!("Funding tx {:02x}", taker_funding_utxo_tx.tx_hash_as_bytes());
    // tx must have 3 outputs: actual funding, OP_RETURN containing the secret hash and change
    assert_eq!(3, taker_funding_utxo_tx.outputs.len());

    // dex_fee_amount (with burn) + premium_amount (zero) + trading_amount
    let expected_amount = 77800000000u64;
    assert_eq!(expected_amount, taker_funding_utxo_tx.outputs[0].value);

    let expected_op_return = Builder::default()
        .push_opcode(Opcode::OP_RETURN)
        .push_data(&[0; 20])
        .into_bytes();
    assert_eq!(expected_op_return, taker_funding_utxo_tx.outputs[1].script_pubkey);

    let validate_args = ValidateTakerFundingArgs {
        funding_tx: &taker_funding_utxo_tx,
        funding_time_lock,
        payment_time_lock: 0,
        taker_secret_hash,
        maker_secret_hash,
        taker_pub,
        dex_fee,
        premium_amount: 0.into(),
        trading_amount: 777.into(),
        swap_unique_data: &[],
    };
    block_on(maker_coin.validate_taker_funding(validate_args)).unwrap();

    let preimage_args = GenTakerFundingSpendArgs {
        funding_tx: &taker_funding_utxo_tx,
        maker_pub,
        taker_pub,
        funding_time_lock,
        taker_secret_hash,
        taker_payment_time_lock: 0,
        maker_secret_hash,
    };
    let preimage = block_on(maker_coin.gen_taker_funding_spend_preimage(&preimage_args, &[])).unwrap();

    let payment_tx = block_on(taker_coin.sign_and_send_taker_funding_spend(&preimage, &preimage_args, &[])).unwrap();
    log!("Taker payment tx {:02x}", payment_tx.tx_hash_as_bytes());

    let gen_taker_payment_spend_args = GenTakerPaymentSpendArgs {
        taker_tx: &payment_tx,
        time_lock: 0,
        maker_secret_hash,
        maker_pub,
        maker_address: &block_on(maker_coin.my_addr()),
        taker_pub,
        dex_fee,
        premium_amount: 0.into(),
        trading_amount: 777.into(),
    };
    let taker_payment_spend_preimage =
        block_on(taker_coin.gen_taker_payment_spend_preimage(&gen_taker_payment_spend_args, &[])).unwrap();

    // tx must have 3 outputs, dex fee, dex fee burn, and payment amount spent to maker address
    assert_eq!(taker_payment_spend_preimage.preimage.outputs.len(), 3);
    assert_eq!(taker_payment_spend_preimage.preimage.outputs[0].value, 75000000);
    assert_eq!(taker_payment_spend_preimage.preimage.outputs[1].value, 25000000);
    assert_eq!(taker_payment_spend_preimage.preimage.outputs[2].value, 77699999008);

    block_on(
        maker_coin.validate_taker_payment_spend_preimage(&gen_taker_payment_spend_args, &taker_payment_spend_preimage),
    )
    .unwrap();

    let taker_payment_spend = block_on(maker_coin.sign_and_broadcast_taker_payment_spend(
        Some(&taker_payment_spend_preimage),
        &gen_taker_payment_spend_args,
        maker_secret,
        &[],
    ))
    .unwrap();
    log!("Taker payment spend tx {:02x}", taker_payment_spend.tx_hash_as_bytes());
}

#[test]
fn send_and_spend_taker_payment_dex_fee_burn_non_kmd() {
    let (_mm_arc, taker_coin, _privkey) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());
    let (_mm_arc, maker_coin, _privkey) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());

    let funding_time_lock = now_sec() - 1000;
    let taker_secret_hash = &[0; 20];

    let maker_secret = &[1; 32];
    let maker_secret_hash_owned = dhash160(maker_secret);
    let maker_secret_hash = maker_secret_hash_owned.as_slice();

    let taker_pub = taker_coin.my_public_key().unwrap();
    let maker_pub = maker_coin.my_public_key().unwrap();

    let dex_fee = &DexFee::create_from_fields("0.75".into(), "0.25".into(), "MYCOIN");

    let send_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock: 0,
        taker_secret_hash,
        maker_secret_hash,
        maker_pub,
        dex_fee,
        premium_amount: 0.into(),
        trading_amount: 777.into(),
        swap_unique_data: &[],
    };
    let taker_funding_utxo_tx = block_on(taker_coin.send_taker_funding(send_args)).unwrap();
    log!("Funding tx {:02x}", taker_funding_utxo_tx.tx_hash_as_bytes());
    // tx must have 3 outputs: actual funding, OP_RETURN containing the secret hash and change
    assert_eq!(3, taker_funding_utxo_tx.outputs.len());

    // dex_fee_amount (with burn) + premium_amount (zero) + trading_amount
    let expected_amount = 77800000000u64;
    assert_eq!(expected_amount, taker_funding_utxo_tx.outputs[0].value);

    let expected_op_return = Builder::default()
        .push_opcode(Opcode::OP_RETURN)
        .push_data(&[0; 20])
        .into_bytes();
    assert_eq!(expected_op_return, taker_funding_utxo_tx.outputs[1].script_pubkey);

    let validate_args = ValidateTakerFundingArgs {
        funding_tx: &taker_funding_utxo_tx,
        funding_time_lock,
        payment_time_lock: 0,
        taker_secret_hash,
        maker_secret_hash,
        taker_pub,
        dex_fee,
        premium_amount: 0.into(),
        trading_amount: 777.into(),
        swap_unique_data: &[],
    };
    block_on(maker_coin.validate_taker_funding(validate_args)).unwrap();

    let preimage_args = GenTakerFundingSpendArgs {
        funding_tx: &taker_funding_utxo_tx,
        maker_pub,
        taker_pub,
        funding_time_lock,
        taker_secret_hash,
        taker_payment_time_lock: 0,
        maker_secret_hash,
    };
    let preimage = block_on(maker_coin.gen_taker_funding_spend_preimage(&preimage_args, &[])).unwrap();

    let payment_tx = block_on(taker_coin.sign_and_send_taker_funding_spend(&preimage, &preimage_args, &[])).unwrap();
    log!("Taker payment tx {:02x}", payment_tx.tx_hash_as_bytes());

    let gen_taker_payment_spend_args = GenTakerPaymentSpendArgs {
        taker_tx: &payment_tx,
        time_lock: 0,
        maker_secret_hash,
        maker_pub,
        maker_address: &block_on(maker_coin.my_addr()),
        taker_pub,
        dex_fee,
        premium_amount: 0.into(),
        trading_amount: 777.into(),
    };
    let taker_payment_spend_preimage =
        block_on(taker_coin.gen_taker_payment_spend_preimage(&gen_taker_payment_spend_args, &[])).unwrap();

    // tx must have 3 outputs: dex fee, burn (for non-kmd too), and maker amount
    // because of the burn output we can't use SIGHASH_SINGLE and taker must add the maker output
    assert_eq!(taker_payment_spend_preimage.preimage.outputs.len(), 3);
    assert_eq!(taker_payment_spend_preimage.preimage.outputs[0].value, 75_000_000);
    assert_eq!(taker_payment_spend_preimage.preimage.outputs[1].value, 25_000_000);
    assert_eq!(taker_payment_spend_preimage.preimage.outputs[2].value, 77699999008);

    block_on(
        maker_coin.validate_taker_payment_spend_preimage(&gen_taker_payment_spend_args, &taker_payment_spend_preimage),
    )
    .unwrap();

    let taker_payment_spend = block_on(maker_coin.sign_and_broadcast_taker_payment_spend(
        Some(&taker_payment_spend_preimage),
        &gen_taker_payment_spend_args,
        maker_secret,
        &[],
    ))
    .unwrap();
    log!(
        "Taker payment spend tx hash {:02x}",
        taker_payment_spend.tx_hash_as_bytes()
    );
}

#[test]
fn send_and_refund_maker_payment_timelock() {
    let (_mm_arc, coin, _privkey) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());

    let time_lock = now_sec() - 1000;
    let taker_secret_hash = &[0; 20];
    let maker_secret_hash = &[1; 20];
    let taker_pub = coin.my_public_key().unwrap();
    let maker_pub = coin.my_public_key().unwrap();

    let send_args = SendMakerPaymentArgs {
        time_lock,
        taker_secret_hash,
        maker_secret_hash,
        amount: 1.into(),
        taker_pub,
        swap_unique_data: &[],
    };
    let maker_payment = block_on(coin.send_maker_payment_v2(send_args)).unwrap();
    log!("{:02x}", maker_payment.tx_hash_as_bytes());
    // tx must have 3 outputs: actual payment, OP_RETURN containing the secret hash and change
    assert_eq!(3, maker_payment.outputs.len());

    // trading_amount
    let expected_amount = 100000000u64;
    assert_eq!(expected_amount, maker_payment.outputs[0].value);

    let expected_op_return_data = [maker_secret_hash.as_slice(), taker_secret_hash].concat();
    let expected_op_return = Builder::default()
        .push_opcode(Opcode::OP_RETURN)
        .push_data(&expected_op_return_data)
        .into_bytes();
    assert_eq!(expected_op_return, maker_payment.outputs[1].script_pubkey);

    let validate_args = ValidateMakerPaymentArgs {
        maker_payment_tx: &maker_payment,
        time_lock,
        taker_secret_hash,
        maker_secret_hash,
        amount: 1.into(),
        swap_unique_data: &[],
        maker_pub,
    };
    block_on(coin.validate_maker_payment_v2(validate_args)).unwrap();

    let refund_args = RefundMakerPaymentTimelockArgs {
        payment_tx: &serialize(&maker_payment).take(),
        time_lock,
        taker_pub: coin.my_public_key().unwrap(),
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::MakerPaymentV2 {
            taker_secret_hash,
            maker_secret_hash,
        },
        swap_unique_data: &[],
        watcher_reward: false,
        amount: Default::default(),
    };

    let refund_tx = block_on(coin.refund_maker_payment_v2_timelock(refund_args)).unwrap();
    log!("{:02x}", refund_tx.tx_hash_as_bytes());
}

#[test]
fn send_and_refund_maker_payment_taker_secret() {
    let (_mm_arc, coin, _privkey) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());
    let taker_secret = &[1; 32];

    let time_lock = now_sec() + 1000;
    let taker_secret_hash_owned = dhash160(taker_secret);
    let taker_secret_hash = taker_secret_hash_owned.as_slice();
    let maker_secret_hash = &[1; 20];
    let taker_pub = coin.my_public_key().unwrap();
    let maker_pub = coin.my_public_key().unwrap();

    let send_args = SendMakerPaymentArgs {
        time_lock,
        taker_secret_hash,
        maker_secret_hash,
        amount: 1.into(),
        taker_pub,
        swap_unique_data: &[],
    };
    let maker_payment = block_on(coin.send_maker_payment_v2(send_args)).unwrap();
    log!("{:02x}", maker_payment.tx_hash_as_bytes());
    // tx must have 3 outputs: actual payment, OP_RETURN containing the secret hash and change
    assert_eq!(3, maker_payment.outputs.len());

    // trading_amount
    let expected_amount = 100000000u64;
    assert_eq!(expected_amount, maker_payment.outputs[0].value);

    let op_return_data = [maker_secret_hash, taker_secret_hash].concat();
    let expected_op_return = Builder::default()
        .push_opcode(Opcode::OP_RETURN)
        .push_data(&op_return_data)
        .into_bytes();
    assert_eq!(expected_op_return, maker_payment.outputs[1].script_pubkey);

    let validate_args = ValidateMakerPaymentArgs {
        maker_payment_tx: &maker_payment,
        time_lock,
        taker_secret_hash,
        maker_secret_hash,
        amount: 1.into(),
        swap_unique_data: &[],
        maker_pub,
    };
    block_on(coin.validate_maker_payment_v2(validate_args)).unwrap();

    let refund_args = RefundMakerPaymentSecretArgs {
        maker_payment_tx: &maker_payment,
        time_lock,
        taker_secret_hash,
        maker_secret_hash,
        swap_unique_data: &[],
        taker_secret,
        taker_pub,
        amount: Default::default(),
    };

    let refund_tx = block_on(coin.refund_maker_payment_v2_secret(refund_args)).unwrap();
    log!("{:02x}", refund_tx.tx_hash_as_bytes());
}

#[test]
fn test_v2_swap_utxo_utxo() {
    test_v2_swap_utxo_utxo_impl();
}

// test a swap when taker is burn pubkey (no dex fee should be paid)
#[test]
fn test_v2_swap_utxo_utxo_burnkey_as_alice() {
    SET_BURN_PUBKEY_TO_ALICE.set(true);
    test_v2_swap_utxo_utxo_impl();
}

fn test_v2_swap_utxo_utxo_impl() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey(MYCOIN1, 1000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);

    let alice_pubkey_str = hex::encode(
        key_pair_from_secret(&alice_priv_key)
            .expect("valid test key pair")
            .public()
            .to_vec(),
    );
    let mut envs = vec![];
    if SET_BURN_PUBKEY_TO_ALICE.get() {
        envs.push(("TEST_BURN_ADDR_RAW_PUBKEY", alice_pubkey_str.as_str()));
    }

    let bob_conf = Mm2TestConf::seednode_trade_v2(&format!("0x{}", hex::encode(bob_priv_key)), &coins);
    let mut mm_bob = block_on(MarketMakerIt::start_with_envs(
        bob_conf.conf,
        bob_conf.rpc_password,
        None,
        &envs,
    ))
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    log!("Bob log path: {}", mm_bob.log_path.display());

    let alice_conf = Mm2TestConf::light_node_trade_v2(
        &format!("0x{}", hex::encode(alice_priv_key)),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mut mm_alice = block_on(MarketMakerIt::start_with_envs(
        alice_conf.conf,
        alice_conf.rpc_password,
        None,
        &envs,
    ))
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
    log!("Alice log path: {}", mm_alice.log_path.display());

    log!("{:?}", block_on(enable_native(&mm_bob, MYCOIN, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, MYCOIN1, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, MYCOIN, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, MYCOIN1, &[], None)));

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[(MYCOIN, MYCOIN1)],
        1.0,
        1.0,
        777.,
    ));
    log!("{:?}", uuids);

    let parsed_uuids: Vec<Uuid> = uuids.iter().map(|u| u.parse().unwrap()).collect();

    let active_swaps_bob = block_on(active_swaps(&mm_bob));
    assert_eq!(active_swaps_bob.uuids, parsed_uuids);

    let active_swaps_alice = block_on(active_swaps(&mm_alice));
    assert_eq!(active_swaps_alice.uuids, parsed_uuids);

    // disabling coins used in active swaps must not work
    let err = block_on(disable_coin_err(&mm_bob, MYCOIN, false));
    assert_eq!(err.active_swaps, parsed_uuids);

    let err = block_on(disable_coin_err(&mm_bob, MYCOIN1, false));
    assert_eq!(err.active_swaps, parsed_uuids);

    let err = block_on(disable_coin_err(&mm_alice, MYCOIN, false));
    assert_eq!(err.active_swaps, parsed_uuids);

    let err = block_on(disable_coin_err(&mm_alice, MYCOIN1, false));
    assert_eq!(err.active_swaps, parsed_uuids);

    // coins must be virtually locked until swap transactions are sent
    let locked_bob = block_on(get_locked_amount(&mm_bob, MYCOIN));
    assert_eq!(locked_bob.coin, MYCOIN);
    let expected: MmNumberMultiRepr = MmNumber::from("777.00000274").into();
    assert_eq!(locked_bob.locked_amount, expected);

    let locked_alice = block_on(get_locked_amount(&mm_alice, MYCOIN1));
    assert_eq!(locked_alice.coin, MYCOIN1);
    let expected: MmNumberMultiRepr = if SET_BURN_PUBKEY_TO_ALICE.get() {
        MmNumber::from("777.00000770").into()
    } else {
        MmNumber::from("778.00000770").into()
    };
    assert_eq!(locked_alice.locked_amount, expected);

    // amount must unlocked after funding tx is sent
    block_on(mm_alice.wait_for_log(20., |log| log.contains("Sent taker funding"))).unwrap();
    let locked_alice = block_on(get_locked_amount(&mm_alice, MYCOIN1));
    assert_eq!(locked_alice.coin, MYCOIN1);
    let expected: MmNumberMultiRepr = MmNumber::from("0").into();
    assert_eq!(locked_alice.locked_amount, expected);

    // amount must unlocked after maker payment is sent
    block_on(mm_bob.wait_for_log(20., |log| log.contains("Sent maker payment"))).unwrap();
    let locked_bob = block_on(get_locked_amount(&mm_bob, MYCOIN));
    assert_eq!(locked_bob.coin, MYCOIN);
    let expected: MmNumberMultiRepr = MmNumber::from("0").into();
    assert_eq!(locked_bob.locked_amount, expected);

    for uuid in uuids {
        block_on(wait_for_swap_finished(&mm_bob, &uuid, 60));
        block_on(wait_for_swap_finished(&mm_alice, &uuid, 30));

        let maker_swap_status = block_on(my_swap_status(&mm_bob, &uuid));
        log!("{:?}", maker_swap_status);

        let taker_swap_status = block_on(my_swap_status(&mm_alice, &uuid));
        log!("{:?}", taker_swap_status);
    }

    block_on(check_recent_swaps(&mm_bob, 1));
    block_on(check_recent_swaps(&mm_alice, 1));

    // Disabling coins on both nodes should be successful at this point
    block_on(disable_coin(&mm_bob, MYCOIN, false));
    block_on(disable_coin(&mm_bob, MYCOIN1, false));
    block_on(disable_coin(&mm_alice, MYCOIN, false));
    block_on(disable_coin(&mm_alice, MYCOIN1, false));
}

#[test]
fn test_v2_swap_utxo_utxo_kickstart() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey(MYCOIN1, 1000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);

    let mut bob_conf = Mm2TestConf::seednode_trade_v2(&format!("0x{}", hex::encode(bob_priv_key)), &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf.clone(), bob_conf.rpc_password.clone(), None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    log!("Bob log path: {}", mm_bob.log_path.display());

    let mut alice_conf = Mm2TestConf::light_node_trade_v2(
        &format!("0x{}", hex::encode(alice_priv_key)),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf.clone(), alice_conf.rpc_password.clone(), None).unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
    log!("Alice log path: {}", mm_alice.log_path.display());

    log!("{:?}", block_on(enable_native(&mm_bob, MYCOIN, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, MYCOIN1, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, MYCOIN, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, MYCOIN1, &[], None)));

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[(MYCOIN, MYCOIN1)],
        1.0,
        1.0,
        777.,
    ));
    log!("{:?}", uuids);

    let parsed_uuids: Vec<Uuid> = uuids.iter().map(|u| u.parse().unwrap()).collect();

    for uuid in uuids.iter() {
        let maker_swap_status = block_on(my_swap_status(&mm_bob, uuid));
        log!("Maker swap {} status before stop {:?}", uuid, maker_swap_status);

        let taker_swap_status = block_on(my_swap_status(&mm_alice, uuid));
        log!("Taker swap {} status before stop  {:?}", uuid, taker_swap_status);
    }

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();

    bob_conf.conf["dbdir"] = mm_bob.folder.join("DB").to_str().unwrap().into();
    bob_conf.conf["log"] = mm_bob.folder.join("mm2_dup.log").to_str().unwrap().into();

    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    log!("Bob log path: {}", mm_bob.log_path.display());

    alice_conf.conf["dbdir"] = mm_alice.folder.join("DB").to_str().unwrap().into();
    alice_conf.conf["log"] = mm_alice.folder.join("mm2_dup.log").to_str().unwrap().into();
    alice_conf.conf["seednodes"] = vec![mm_bob.ip.to_string()].into();

    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
    log!("Alice log path: {}", mm_alice.log_path.display());

    let mut coins_needed_for_kickstart_bob = block_on(coins_needed_for_kickstart(&mm_bob));
    coins_needed_for_kickstart_bob.sort();
    assert_eq!(coins_needed_for_kickstart_bob, [MYCOIN, MYCOIN1]);

    let mut coins_needed_for_kickstart_alice = block_on(coins_needed_for_kickstart(&mm_alice));
    coins_needed_for_kickstart_alice.sort();
    assert_eq!(coins_needed_for_kickstart_alice, [MYCOIN, MYCOIN1]);

    log!("{:?}", block_on(enable_native(&mm_bob, MYCOIN, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, MYCOIN1, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, MYCOIN, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, MYCOIN1, &[], None)));

    // give swaps 1 second to restart
    std::thread::sleep(Duration::from_secs(1));

    let active_swaps_bob = block_on(active_swaps(&mm_bob));
    assert_eq!(active_swaps_bob.uuids, parsed_uuids);

    let active_swaps_alice = block_on(active_swaps(&mm_alice));
    assert_eq!(active_swaps_alice.uuids, parsed_uuids);

    // coins must be virtually locked after kickstart until swap transactions are sent
    let locked_alice = block_on(get_locked_amount(&mm_alice, MYCOIN1));
    assert_eq!(locked_alice.coin, MYCOIN1);
    let expected: MmNumberMultiRepr = MmNumber::from("778.00000770").into();
    assert_eq!(locked_alice.locked_amount, expected);

    let locked_bob = block_on(get_locked_amount(&mm_bob, MYCOIN));
    assert_eq!(locked_bob.coin, MYCOIN);
    let expected: MmNumberMultiRepr = MmNumber::from("777.00000274").into();
    assert_eq!(locked_bob.locked_amount, expected);

    // amount must unlocked after funding tx is sent
    block_on(mm_alice.wait_for_log(20., |log| log.contains("Sent taker funding"))).unwrap();
    let locked_alice = block_on(get_locked_amount(&mm_alice, MYCOIN1));
    assert_eq!(locked_alice.coin, MYCOIN1);
    let expected: MmNumberMultiRepr = MmNumber::from("0").into();
    assert_eq!(locked_alice.locked_amount, expected);

    // amount must unlocked after maker payment is sent
    block_on(mm_bob.wait_for_log(20., |log| log.contains("Sent maker payment"))).unwrap();
    let locked_bob = block_on(get_locked_amount(&mm_bob, MYCOIN));
    assert_eq!(locked_bob.coin, MYCOIN);
    let expected: MmNumberMultiRepr = MmNumber::from("0").into();
    assert_eq!(locked_bob.locked_amount, expected);

    for uuid in uuids {
        block_on(wait_for_swap_finished(&mm_bob, &uuid, 60));
        block_on(wait_for_swap_finished(&mm_alice, &uuid, 30));
    }
}

#[test]
fn test_v2_swap_utxo_utxo_file_lock() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey(MYCOIN, 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey(MYCOIN1, 1000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);

    let mut bob_conf = Mm2TestConf::seednode_trade_v2(&format!("0x{}", hex::encode(bob_priv_key)), &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf.clone(), bob_conf.rpc_password.clone(), None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    log!("Bob log path: {}", mm_bob.log_path.display());

    let mut alice_conf = Mm2TestConf::light_node_trade_v2(
        &format!("0x{}", hex::encode(alice_priv_key)),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf.clone(), alice_conf.rpc_password.clone(), None).unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
    log!("Alice log path: {}", mm_alice.log_path.display());

    log!("{:?}", block_on(enable_native(&mm_bob, MYCOIN, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, MYCOIN1, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, MYCOIN, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, MYCOIN1, &[], None)));

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[(MYCOIN, MYCOIN1)],
        1.0,
        1.0,
        100.,
    ));
    log!("{:?}", uuids);

    for uuid in uuids.iter() {
        block_on(wait_for_swap_status(&mm_bob, uuid, 10));
        block_on(wait_for_swap_status(&mm_alice, uuid, 10));
    }

    bob_conf.conf["dbdir"] = mm_bob.folder.join("DB").to_str().unwrap().into();
    bob_conf.conf["log"] = mm_bob.folder.join("mm2_dup.log").to_str().unwrap().into();

    let mut mm_bob_dup = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob_dup.log_path);
    log!("Bob dup log path: {}", mm_bob_dup.log_path.display());

    alice_conf.conf["dbdir"] = mm_alice.folder.join("DB").to_str().unwrap().into();
    alice_conf.conf["log"] = mm_alice.folder.join("mm2_dup.log").to_str().unwrap().into();
    alice_conf.conf["seednodes"] = vec![mm_bob.ip.to_string()].into();

    let mut mm_alice_dup = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice_dup.log_path);
    log!("Alice dup log path: {}", mm_alice_dup.log_path.display());

    log!("{:?}", block_on(enable_native(&mm_bob_dup, MYCOIN, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob_dup, MYCOIN1, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice_dup, MYCOIN, &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice_dup, MYCOIN1, &[], None)));

    for uuid in uuids {
        let expected_log = format!("Swap {uuid} file lock already acquired");
        block_on(mm_bob_dup.wait_for_log(22., |log| log.contains(&expected_log))).unwrap();
        block_on(mm_alice_dup.wait_for_log(22., |log| log.contains(&expected_log))).unwrap();
    }
}
