use super::*;
use crate::{DexFee, TxFeeDetails, WaitForHTLCTxSpendArgs};
use common::{block_on, block_on_f01, wait_until_sec};
use crypto::Secp256k1Secret;
use itertools::Itertools;
use keys::Address;
use mm2_core::mm_ctx::MmCtxBuilder;
use mm2_number::bigdecimal::Zero;
use mm2_test_helpers::electrums::tqtum_electrums;
use rpc::v1::types::ToTxHash;
use std::convert::TryFrom;
use std::mem::discriminant;

cfg_native!(
    use crate::utxo::rpc_clients::UnspentInfo;

    use chain::OutPoint;
    use keys::AddressBuilder;
    use mocktopus::mocking::{MockResult, Mockable};
);

const DEFAULT_TX_FEE_RATE: i64 = 1000;
const CONTRACT_CALL_GAS_FEE: i64 = (QRC20_GAS_LIMIT_DEFAULT * QRC20_GAS_PRICE_DEFAULT) as i64;
const SWAP_PAYMENT_GAS_FEE: i64 = (QRC20_PAYMENT_GAS_LIMIT * QRC20_GAS_PRICE_DEFAULT) as i64;
const TAKER_PAYMENT_SPEND_SEARCH_INTERVAL: f64 = 1.;

pub fn qrc20_coin_for_test(priv_key: [u8; 32], fallback_swap: Option<&str>) -> (MmArc, Qrc20Coin) {
    let conf = json!({
        "coin":"QRC20",
        "decimals": 8,
        "required_confirmations":0,
        "pubtype":120,
        "p2shtype":110,
        "wiftype":128,
        "mm2":1,
        "mature_confirmations":2000,
        "dust":72800,
    });
    let req = json!({
        "method": "electrum",
        "servers": tqtum_electrums(),
        "swap_contract_address": "0xba8b71f3544b93e2f681f996da519a98ace0107a",
        "fallback_swap_contract": fallback_swap,
    });
    let contract_address = H160::from_str("0xd362e096e873eb7907e205fadc6175c6fec7bc44").unwrap();
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = Qrc20ActivationParams::from_legacy_req(&req).unwrap();

    let coin = block_on(qrc20_coin_with_priv_key(
        &ctx,
        "QRC20",
        "QTUM",
        &conf,
        &params,
        Secp256k1Secret::from(priv_key),
        contract_address,
    ))
    .unwrap();
    (ctx, coin)
}

fn check_fee_rate(coin: &Qrc20Coin, expected_fee_rate: ActualFeeRate) {
    let actual_fee_rate = block_on(coin.get_fee_rate()).unwrap();
    assert_eq!(actual_fee_rate, expected_fee_rate);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_withdraw_to_p2sh_address_should_fail() {
    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let (_, coin) = qrc20_coin_for_test(priv_key, None);

    let p2sh_address = AddressBuilder::new(
        UtxoAddressFormat::Standard,
        *block_on(coin.as_ref().derivation_method.unwrap_single_addr()).checksum_type(),
        coin.as_ref().conf.address_prefixes.clone(),
        coin.as_ref().conf.bech32_hrp.clone(),
    )
    .as_sh(
        block_on(coin.as_ref().derivation_method.unwrap_single_addr())
            .hash()
            .clone(),
    )
    .build()
    .expect("valid address props");

    let req = WithdrawRequest {
        amount: 10.into(),
        to: p2sh_address.to_string(),
        coin: "QRC20".into(),
        ..Default::default()
    };
    let err = block_on_f01(coin.withdraw(req)).unwrap_err().into_inner();
    let expect = WithdrawError::InvalidAddress("QRC20 can be sent to P2PKH addresses only".to_owned());
    assert_eq!(err, expect);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_withdraw_impl_fee_details() {
    // priv_key of qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG
    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);

    Qrc20Coin::get_unspent_ordered_list.mock_safe(|coin, _| {
        let fut = async move {
            let cache = coin.as_ref().recently_spent_outpoints.lock().await;
            let unspents = vec![UnspentInfo {
                outpoint: OutPoint {
                    hash: 1.into(),
                    index: 0,
                },
                value: 1000000000,
                height: Default::default(),
                script: coin
                    .script_for_address(&coin.as_ref().derivation_method.unwrap_single_addr().await)
                    .unwrap(),
            }];
            Ok((unspents, cache))
        };
        MockResult::Return(fut.boxed())
    });

    let withdraw_req = WithdrawRequest {
        amount: 10.into(),
        to: "qHmJ3KA6ZAjR9wGjpFASn4gtUSeFAqdZgs".into(),
        coin: "QRC20".into(),
        fee: Some(WithdrawFee::Qrc20Gas {
            gas_limit: 2_500_000,
            gas_price: 40,
        }),
        ..Default::default()
    };
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();

    let expected: Qrc20FeeDetails = json::from_value(json!({
        "coin": "QTUM",
        // 1000 from satoshi,
        // where decimals = 8,
        //       1000 is fixed fee
        "miner_fee": "0.00000299",
        "gas_limit": 2_500_000,
        "gas_price": 40,
        // (gas_limit * gas_price) from satoshi in Qtum
        "total_gas_fee": "1",
    }))
    .unwrap();
    assert_eq!(tx_details.fee_details, Some(TxFeeDetails::Qrc20(expected)));
}

#[test]
fn test_validate_maker_payment() {
    // this priv_key corresponds to "taker_passphrase" passphrase
    let priv_key = [
        24, 181, 194, 193, 18, 152, 142, 168, 71, 73, 70, 244, 9, 101, 92, 168, 243, 61, 132, 48, 25, 39, 103, 92, 29,
        17, 11, 29, 113, 235, 48, 70,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);

    assert_eq!(
        block_on(coin.utxo.derivation_method.unwrap_single_addr()),
        Address::from_legacyaddress(
            "qUX9FGHubczidVjWPCUWuwCUJWpkAtGCgf",
            &coin.as_ref().conf.address_prefixes
        )
        .unwrap()
    );

    // tx_hash: 016a59dd2b181b3906b0f0333d5c7561dacb332dc99ac39679a591e523f2c49a
    let payment_tx = hex::decode("010000000194448324c14fc6b78c7a52c59debe3240fc392019dbd6f1457422e3308ce1e75010000006b483045022100800a4956a30a36708536d98e8ea55a3d0983b963af6c924f60241616e2ff056d0220239e622f8ec8f1a0f5ef0fc93ff094a8e6b5aab964a62bed680b17bf6a848aac012103693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9ffffffff020000000000000000e35403a0860101284cc49b415b2a0c692f2ec8ebab181a79e31b7baab30fef0902e57f901c47a342643eeafa6b510000000000000000000000000000000000000000000000000000000001312d00000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc44000000000000000000000000783cf0be521101942da509846ea476e683aad8320101010101010101010101010101010101010101000000000000000000000000000000000000000000000000000000000000000000000000000000005f72ec7514ba8b71f3544b93e2f681f996da519a98ace0107ac201319302000000001976a9149e032d4b0090a11dc40fe6c47601499a35d55fbb88ac40ed725f").unwrap();
    // pubkey of "cMhHM3PMpMrChygR4bLF7QsTdenhWpFrrmf2UezBG3eeFsz41rtL" passphrase
    let correct_maker_pub = hex::decode("03693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9").unwrap();
    let correct_amount = BigDecimal::from_str("0.2").unwrap();

    let mut input = ValidatePaymentInput {
        payment_tx,
        time_lock_duration: 0,
        time_lock: 1601367157,
        other_pub: correct_maker_pub.clone(),
        secret_hash: vec![1; 20],
        amount: correct_amount.clone(),
        swap_contract_address: coin.swap_contract_address(),
        try_spv_proof_until: wait_until_sec(30),
        confirmations: 1,
        unique_swap_data: Vec::new(),
        watcher_reward: None,
    };

    block_on(coin.validate_maker_payment(input.clone())).unwrap();

    input.other_pub = hex::decode("022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1a").unwrap();
    let error = block_on(coin.validate_maker_payment(input.clone()))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("Payment tx 0x9e032d4b0090a11dc40fe6c47601499a35d55fbb was sent from wrong address, expected 0x783cf0be521101942da509846ea476e683aad832")),
        _ => panic!("Expected `WrongPaymentTx` wrong address, found {:?}", error),
    }
    input.other_pub = correct_maker_pub;

    input.amount = BigDecimal::from_str("0.3").unwrap();
    let error = block_on(coin.validate_maker_payment(input.clone()))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Unexpected 'erc20Payment' contract call bytes"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` unexpected contract call bytes, found {:?}",
            error
        ),
    }
    input.amount = correct_amount;

    input.secret_hash = vec![2; 20];
    let error = block_on(coin.validate_maker_payment(input.clone()))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::UnexpectedPaymentState(err) => {
            assert!(err.contains("Payment state is not PAYMENT_STATE_SENT, got 0"))
        },
        _ => panic!(
            "Expected `UnexpectedPaymentState` state not PAYMENT_STATE_SENT, found {:?}",
            error
        ),
    }
    input.secret_hash = vec![1; 20];

    input.time_lock = 123;
    let error = block_on(coin.validate_maker_payment(input)).unwrap_err().into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::UnexpectedPaymentState(err) => {
            assert!(err.contains("Payment state is not PAYMENT_STATE_SENT, got 0"))
        },
        _ => panic!(
            "Expected `UnexpectedPaymentState` state not PAYMENT_STATE_SENT, found {:?}",
            error
        ),
    }
}

#[test]
fn test_wait_for_confirmations_excepted() {
    // this priv_key corresponds to "taker_passphrase" passphrase
    let priv_key = [
        24, 181, 194, 193, 18, 152, 142, 168, 71, 73, 70, 244, 9, 101, 92, 168, 243, 61, 132, 48, 25, 39, 103, 92, 29,
        17, 11, 29, 113, 235, 48, 70,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);

    assert_eq!(
        block_on(coin.utxo.derivation_method.unwrap_single_addr()),
        Address::from_legacyaddress(
            "qUX9FGHubczidVjWPCUWuwCUJWpkAtGCgf",
            &coin.as_ref().conf.address_prefixes
        )
        .unwrap()
    );

    // tx_hash: 35e03bc529528a853ee75dde28f27eec8ed7b152b6af7ab6dfa5d55ea46f25ac
    // `approve` contract call excepted only, and `erc20Payment` completed
    let payment_tx = hex::decode("0100000003b1fcca3d7c15bb7f694b4e58b939b8835bce4d535e8441d41855d9910a33372f020000006b48304502210091342b2251d13ae0796f6ebf563bb861883d652cbee9f5606dd5bb875af84039022077a21545ff6ec69c9c4eca35e1f127a450abc4f4e60dd032724d70910d6b2835012102cd7745ea1c03c9a1ebbcdb7ab9ee19d4e4d306f44665295d996db7c38527da6bffffffff874c96188a610850d4cd2c29a7fd20e5b9eb7f6748970792a74ad189405b7d9b020000006a473044022055dc1bf716880764e9bcbe8dd3aea05f634541648ec4f5d224eba93fedc54f8002205e38b6136adc46ef8ca65c0b0e9390837e539cbb19df451e33a90e534c12da4c012102cd7745ea1c03c9a1ebbcdb7ab9ee19d4e4d306f44665295d996db7c38527da6bffffffffd52e234ead3b8a2a4718cb6fee039fa96862063fccf95149fb11f27a52bcc352010000006a4730440220527ce41324e53c99b827d3f34e7078d991abf339f24108b7e677fff1b6cf0ffa0220690fe96d4fb8f1673458bc08615b5119f354f6cd589754855fe1dba5f82653aa012102cd7745ea1c03c9a1ebbcdb7ab9ee19d4e4d306f44665295d996db7c38527da6bffffffff030000000000000000625403a08601012844095ea7b3000000000000000000000000ba8b71f3544b93e2f681f996da519a98ace0107a0000000000000000000000000000000000000000000000000000000001312d0014d362e096e873eb7907e205fadc6175c6fec7bc44c20000000000000000e35403a0860101284cc49b415b2a756dd4fe3852ea4a0378c5e984ebb5e4bfa01eca31785457d1729d5928198ef00000000000000000000000000000000000000000000000000000000001312d00000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc440000000000000000000000000240b898276ad2cc0d2fe6f527e8e31104e7fde30101010101010101010101010101010101010101000000000000000000000000000000000000000000000000000000000000000000000000000000005f686cef14ba8b71f3544b93e2f681f996da519a98ace0107ac21082fb03000000001976a914f36e14131c70e5f15a3f92b1d7e8622a62e570d888acb86d685f").unwrap();

    let confirmations = 1;
    let requires_nota = false;
    let wait_until = now_sec() + 10; // the transaction is mined already
    let check_every = 1;
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx,
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    // tx_hash: ed53b97deb2ad76974c972cb084f6ba63bd9f16c91c4a39106a20c6d14599b2a
    // `erc20Payment` contract call excepted
    let payment_tx = hex::decode("01000000014c1411bac38ca25a2816342b019df81f503e1db75b25c6da618b08484dc2ff49010000006b483045022100da3e90fbcc45a94573c28213b36dc616630e3adfa42a7f16bdf917e8a76b954502206ad0830bb16e5c25466903ae7f749e291586726f1497ae9fc2e709c1b6cd1857012103693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9ffffffff040000000000000000625403a08601012844095ea7b3000000000000000000000000ba8b71f3544b93e2f681f996da519a98ace0107a000000000000000000000000000000000000000000000000000000000000000014d362e096e873eb7907e205fadc6175c6fec7bc44c20000000000000000625403a08601012844095ea7b3000000000000000000000000ba8b71f3544b93e2f681f996da519a98ace0107a000000000000000000000000000000000000000000000000000000000000000a14d362e096e873eb7907e205fadc6175c6fec7bc44c20000000000000000e35403a0860101284cc49b415b2a0a1a8b4af2762154115ced87e2424b3cb940c0181cc3c850523702f1ec298fef0000000000000000000000000000000000000000000000000000000000000064000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc44000000000000000000000000783cf0be521101942da509846ea476e683aad8324b6b2e5444c2639cc0fb7bcea5afba3f3cdce239000000000000000000000000000000000000000000000000000000000000000000000000000000005fa0fffb14ba8b71f3544b93e2f681f996da519a98ace0107ac2493d4a03000000001976a9149e032d4b0090a11dc40fe6c47601499a35d55fbb88acae2ea15f").unwrap();
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx,
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    let error = block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap_err();
    log!("error: {:?}", error);
    assert!(error.contains("Contract call failed with an error: Revert"));

    // tx_hash: aa992c028c07e239dbd2ff32bf67251f026929c644b4d02a469e351cb44abab7
    // `receiverSpend` contract call excepted
    let payment_tx = hex::decode("0100000007077ccb377a68fd6079503f856df4e553e337015f8419cd0f2a949c31db175df7050000006a473044022058097f54be31ae5af197f72e4410b33b22f29fad5b1a1cefb30ee45b3b3477dc02205c1098850fa2f2c1929c27af6261f83abce7682eb769f909dd09e9be5e0bd469012102aa32922f4b05cbc7384dd85b86021c98e4102f5da3df48bc516aa76f8119559affffffffc191895a431db3dccbf4f9d4b8cd8301124343e66275194ad734a77ffe56b95e030000006a4730440220491fed7954c6f43acc7226c337bb16ac71b38df50f55a819441d9b2b9e4a04b502201f95be6941b6619c0ca246e15adb090b82cd908f7c85108a1dcc02eafb7cc725012102aa32922f4b05cbc7384dd85b86021c98e4102f5da3df48bc516aa76f8119559afffffffff678de174fb81d3820df43a2c29945b08df4fb080deb8088ef11b3711c0fe8df020000006a473044022071d9c0ec57ab23360a4f73d0edfc2f67614b56f6d2e54387b39c3de1fa894c7d022030ea65d157784ff68cae9c9acb0dd626205073f478003b1cb1d0d581dcb27b1c012102aa32922f4b05cbc7384dd85b86021c98e4102f5da3df48bc516aa76f8119559affffffffe1ef8740ce51ed3172efea91a5e559b5fe63dc6fede8a9037ad47fbc38560b51040000006a47304402203f056dff0be1f24ed96c72904c9aac3ac964913d0c3228bfab3fa4bef7f22c060220658a121bf8f29d86c18ec1aee4460f363c0704d2f05cc9d7923e978e917f48ca012102aa32922f4b05cbc7384dd85b86021c98e4102f5da3df48bc516aa76f8119559affffffffe825dea61113bbd67dd35cbc9d88890ac222f55bf0201a7f9fb96592e0614d4d080000006b483045022100bb10f195c57c1eed9de3d9d9726484f839e25d83deb54cf2142df37099df6a8d02202a025182caaa5348350b410ee783180e9ce3ccac5e361eb50b162311e9d803f1012102aa32922f4b05cbc7384dd85b86021c98e4102f5da3df48bc516aa76f8119559affffffffe1ef8740ce51ed3172efea91a5e559b5fe63dc6fede8a9037ad47fbc38560b51060000006a47304402205550e0b4e1425f2f7a8645c6fd408ba0603cca5ca408202729041f5eab0b0cd202205c98fc8e91a37960d38f0104e81d3d48f737c4000ef45e2372c84d857455da34012102aa32922f4b05cbc7384dd85b86021c98e4102f5da3df48bc516aa76f8119559affffffffe825dea61113bbd67dd35cbc9d88890ac222f55bf0201a7f9fb96592e0614d4d060000006b483045022100b0d21cbb5d94b4995d9cb81e7440849dbe645416bca6d51bb5450e10753523220220299f105d573cdb785233699b5a9be8f907d9821a74cfd91fb72911a4a6e1bdb8012102aa32922f4b05cbc7384dd85b86021c98e4102f5da3df48bc516aa76f8119559affffffff020000000000000000c35403a0860101284ca402ed292be8b1d4904e8f1924bd7a2eb4d8085214c17af3d8d7574b2740a86b6296d343c00000000000000000000000000000000000000000000000000000000005f5e10028fcc0c5f6d9619d3c1f90af51e891d62333eb748c568f7da2a7734240d37d38000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc44000000000000000000000000d020b63f5a989776516bdc04d426ba118130c00214ba8b71f3544b93e2f681f996da519a98ace0107ac270630800000000001976a914fb7dad7ce97deecf50a4573a2bd7639c79bdc08588aca64aaa5f").unwrap();
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx,
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    let error = block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap_err();
    log!("error: {:?}", error);
    assert!(error.contains("Contract call failed with an error: Revert"));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_validate_fee() {
    // priv_key of qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG

    use common::DEX_FEE_ADDR_RAW_PUBKEY;
    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);

    // QRC20 transfer tx "f97d3a43dbea0993f1b7a6a299377d4ee164c84935a1eb7d835f70c9429e6a1d"
    let tx = TransactionEnum::UtxoTx("010000000160fd74b5714172f285db2b36f0b391cd6883e7291441631c8b18f165b0a4635d020000006a47304402205d409e141111adbc4f185ae856997730de935ac30a0d2b1ccb5a6c4903db8171022024fc59bbcfdbba283556d7eeee4832167301dc8e8ad9739b7865f67b9676b226012103693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9ffffffff020000000000000000625403a08601012844a9059cbb000000000000000000000000ca1e04745e8ca0c60d8c5881531d51bec470743f00000000000000000000000000000000000000000000000000000000000f424014d362e096e873eb7907e205fadc6175c6fec7bc44c200ada205000000001976a9149e032d4b0090a11dc40fe6c47601499a35d55fbb88acfe967d5f".into());
    let sender_pub = hex::decode("03693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9").unwrap();

    let amount = BigDecimal::from_str("0.01").unwrap();

    let result = block_on(coin.validate_fee(ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: &sender_pub,
        dex_fee: &DexFee::Standard(amount.clone().into()),
        min_block_number: 0,
        uuid: &[],
    }));
    assert!(result.is_ok());

    // wrong dex address
    <Qrc20Coin as SwapOps>::dex_pubkey.mock_safe(|_| {
        MockResult::Return(Box::leak(Box::new(
            hex::decode("03bc2c7ba671bae4a6fc835244c9762b41647b9827d4780a89a949b984a8ddcc05").unwrap(),
        )))
    });
    let err = block_on(coin.validate_fee(ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: &sender_pub,
        dex_fee: &DexFee::Standard(amount.clone().into()),
        min_block_number: 0,
        uuid: &[],
    }))
    .expect_err("Expected an error")
    .into_inner();
    log!("error: {:?}", err);
    match err {
        ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("QRC20 Fee tx was sent to wrong address")),
        _ => panic!("Expected `WrongPaymentTx` wrong receiver address, found {:?}", err),
    }
    <Qrc20Coin as SwapOps>::dex_pubkey.clear_mock();

    let err = block_on(coin.validate_fee(ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: &DEX_FEE_ADDR_RAW_PUBKEY,
        dex_fee: &DexFee::Standard(amount.clone().into()),
        min_block_number: 0,
        uuid: &[],
    }))
    .expect_err("Expected an error")
    .into_inner();
    log!("error: {:?}", err);
    match err {
        ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("was sent from wrong address")),
        _ => panic!("Expected `WrongPaymentTx` wrong sender address, found {:?}", err),
    }

    let err = block_on(coin.validate_fee(ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: &sender_pub,
        dex_fee: &DexFee::Standard(amount.clone().into()),
        min_block_number: 2000000,
        uuid: &[],
    }))
    .expect_err("Expected an error")
    .into_inner();
    log!("error: {:?}", err);
    match err {
        ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("confirmed before min_block")),
        _ => panic!("Expected `WrongPaymentTx` early confirmation, found {:?}", err),
    }

    let amount_dif = BigDecimal::from_str("0.02").unwrap();
    let err = block_on(coin.validate_fee(ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: &sender_pub,
        dex_fee: &DexFee::Standard(amount_dif.into()),
        min_block_number: 0,
        uuid: &[],
    }))
    .expect_err("Expected an error")
    .into_inner();
    log!("error: {:?}", err);
    match err {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("QRC20 Fee tx value 1000000 is less than expected 2000000"))
        },
        _ => panic!("Expected `WrongPaymentTx` invalid fee value, found {:?}", err),
    }

    // QTUM tx "8a51f0ffd45f34974de50f07c5bf2f0949da4e88433f8f75191953a442cf9310"
    let tx = TransactionEnum::UtxoTx("020000000113640281c9332caeddd02a8dd0d784809e1ad87bda3c972d89d5ae41f5494b85010000006a47304402207c5c904a93310b8672f4ecdbab356b65dd869a426e92f1064a567be7ccfc61ff02203e4173b9467127f7de4682513a21efb5980e66dbed4da91dff46534b8e77c7ef012102baefe72b3591de2070c0da3853226b00f082d72daa417688b61cb18c1d543d1afeffffff020001b2c4000000001976a9149e032d4b0090a11dc40fe6c47601499a35d55fbb88acbc4dd20c2f0000001976a9144208fa7be80dcf972f767194ad365950495064a488ac76e70800".into());
    let sender_pub = hex::decode("02baefe72b3591de2070c0da3853226b00f082d72daa417688b61cb18c1d543d1a").unwrap();
    let err = block_on(coin.validate_fee(ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: &sender_pub,
        dex_fee: &DexFee::Standard(amount.into()),
        min_block_number: 0,
        uuid: &[],
    }))
    .expect_err("Expected an error")
    .into_inner();
    log!("error: {:?}", err);
    match err {
        ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("Expected 'transfer' contract call")),
        _ => panic!("Expected `WrongPaymentTx` invalid contract call, found {:?}", err),
    }
}

#[test]
#[ignore]
fn test_wait_for_tx_spend_malicious() {
    // priv_key of qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG
    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);

    // f94d79f89e9ec785db40bb8bb8dca9bc01b7761429618d4c843bbebbc31836b7
    // the transaction has two outputs:
    //   1 - with an invalid secret (this case should be processed correctly)
    //   2 - correct spend tx
    let expected_tx: UtxoTx = "01000000022bc8299981ec0cea664cdf9df4f8306396a02e2067d6ac2d3770b34646d2bc2a010000006b483045022100eb13ef2d99ac1cd9984045c2365654b115dd8a7815b7fbf8e2a257f0b93d1592022060d648e73118c843e97f75fafc94e5ff6da70ec8ba36ae255f8c96e2626af6260121022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1affffffffd92a0a10ac6d144b36033916f67ae79889f40f35096629a5cd87be1a08f40ee7010000006b48304502210080cdad5c4770dfbeb760e215494c63cc30da843b8505e75e7bf9e8dad18568000220234c0b11c41bfbcdd50046c69059976aedabe17657fe43d809af71e9635678e20121022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1affffffff030000000000000000c35403a0860101284ca402ed292b8620ad3b72361a5aeba5dffd333fb64750089d935a1ec974d6a91ef4f24ff6ba0000000000000000000000000000000000000000000000000000000001312d000202020202020202020202020202020202020202020202020202020202020202000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc440000000000000000000000009e032d4b0090a11dc40fe6c47601499a35d55fbb14ba8b71f3544b93e2f681f996da519a98ace0107ac20000000000000000c35403a0860101284ca402ed292b8620ad3b72361a5aeba5dffd333fb64750089d935a1ec974d6a91ef4f24ff6ba0000000000000000000000000000000000000000000000000000000001312d000101010101010101010101010101010101010101010101010101010101010101000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc440000000000000000000000009e032d4b0090a11dc40fe6c47601499a35d55fbb14ba8b71f3544b93e2f681f996da519a98ace0107ac2b8ea82d3010000001976a914783cf0be521101942da509846ea476e683aad83288ac735d855f".into();

    // 15fd8f71be6b2678b021e1300e67fa99574a2ad877df08276ac275728ac12304
    let payment_tx = hex::decode("01000000016601daa208531d20532c460d0c86b74a275f4a126bbffcf4eafdf33835af2859010000006a47304402205825657548bc1b5acf3f4bb2f89635a02b04f3228cd08126e63c5834888e7ac402207ca05fa0a629a31908a97a508e15076e925f8e621b155312b7526a6666b06a76012103693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9ffffffff020000000000000000e35403a0860101284cc49b415b2a8620ad3b72361a5aeba5dffd333fb64750089d935a1ec974d6a91ef4f24ff6ba0000000000000000000000000000000000000000000000000000000001312d00000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc44000000000000000000000000783cf0be521101942da509846ea476e683aad8324b6b2e5444c2639cc0fb7bcea5afba3f3cdce239000000000000000000000000000000000000000000000000000000000000000000000000000000005f855c7614ba8b71f3544b93e2f681f996da519a98ace0107ac2203de400000000001976a9149e032d4b0090a11dc40fe6c47601499a35d55fbb88ac415d855f").unwrap();
    let wait_until = now_sec() + 1;
    let from_block = 696245;
    let found = block_on(coin.wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
        tx_bytes: &payment_tx,
        secret_hash: &[],
        wait_until,
        from_block,
        swap_contract_address: &coin.swap_contract_address(),
        check_every: TAKER_PAYMENT_SPEND_SEARCH_INTERVAL,
        watcher_reward: false,
    }))
    .unwrap();

    let spend_tx = match found {
        TransactionEnum::UtxoTx(tx) => tx,
        _ => panic!("Unexpected Transaction type"),
    };

    assert_eq!(spend_tx, expected_tx);
}

#[test]
fn test_extract_secret() {
    // priv_key of qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG
    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);

    let expected_secret = [1; 32];
    let secret_hash = &*dhash160(&expected_secret);

    // taker spent maker payment - d3f5dab4d54c14b3d7ed8c7f5c8cc7f47ccf45ce589fdc7cd5140a3c1c3df6e1
    let tx_hex = hex::decode("01000000033f56ecafafc8602fde083ba868d1192d6649b8433e42e1a2d79ba007ea4f7abb010000006b48304502210093404e90e40d22730013035d31c404c875646dcf2fad9aa298348558b6d65ba60220297d045eac5617c1a3eddb71d4bca9772841afa3c4c9d6c68d8d2d42ee6de3950121022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1affffffff9cac7fe90d597922a1d92e05306c2215628e7ea6d5b855bfb4289c2944f4c73a030000006b483045022100b987da58c2c0c40ce5b6ef2a59e8124ed4ef7a8b3e60c7fb631139280019bc93022069649bcde6fe4dd5df9462a1fcae40598488d6af8c324cd083f5c08afd9568be0121022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1affffffff70b9870f2b0c65d220a839acecebf80f5b44c3ca4c982fa2fdc5552c037f5610010000006a473044022071b34dd3ebb72d29ca24f3fa0fc96571c815668d3b185dd45cc46a7222b6843f02206c39c030e618d411d4124f7b3e7ca1dd5436775bd8083a85712d123d933a51300121022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1affffffff020000000000000000c35403a0860101284ca402ed292b806a1835a1b514ad643f2acdb5c8db6b6a9714accff3275ea0d79a3f23be8fd00000000000000000000000000000000000000000000000000000000001312d000101010101010101010101010101010101010101010101010101010101010101000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc440000000000000000000000009e032d4b0090a11dc40fe6c47601499a35d55fbb14ba8b71f3544b93e2f681f996da519a98ace0107ac2c02288d4010000001976a914783cf0be521101942da509846ea476e683aad83288ac0f047f5f").unwrap();
    let secret = block_on(coin.extract_secret(secret_hash, &tx_hex, false)).unwrap();

    assert_eq!(secret, expected_secret);
}

#[test]
fn test_extract_secret_malicious() {
    // priv_key of qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG
    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);

    // f94d79f89e9ec785db40bb8bb8dca9bc01b7761429618d4c843bbebbc31836b7
    // the transaction has two outputs:
    //   1 - with an invalid secret (this case should be processed correctly)
    //   2 - correct spend tx
    let spend_tx = hex::decode("01000000022bc8299981ec0cea664cdf9df4f8306396a02e2067d6ac2d3770b34646d2bc2a010000006b483045022100eb13ef2d99ac1cd9984045c2365654b115dd8a7815b7fbf8e2a257f0b93d1592022060d648e73118c843e97f75fafc94e5ff6da70ec8ba36ae255f8c96e2626af6260121022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1affffffffd92a0a10ac6d144b36033916f67ae79889f40f35096629a5cd87be1a08f40ee7010000006b48304502210080cdad5c4770dfbeb760e215494c63cc30da843b8505e75e7bf9e8dad18568000220234c0b11c41bfbcdd50046c69059976aedabe17657fe43d809af71e9635678e20121022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1affffffff030000000000000000c35403a0860101284ca402ed292b8620ad3b72361a5aeba5dffd333fb64750089d935a1ec974d6a91ef4f24ff6ba0000000000000000000000000000000000000000000000000000000001312d000202020202020202020202020202020202020202020202020202020202020202000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc440000000000000000000000009e032d4b0090a11dc40fe6c47601499a35d55fbb14ba8b71f3544b93e2f681f996da519a98ace0107ac20000000000000000c35403a0860101284ca402ed292b8620ad3b72361a5aeba5dffd333fb64750089d935a1ec974d6a91ef4f24ff6ba0000000000000000000000000000000000000000000000000000000001312d000101010101010101010101010101010101010101010101010101010101010101000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc440000000000000000000000009e032d4b0090a11dc40fe6c47601499a35d55fbb14ba8b71f3544b93e2f681f996da519a98ace0107ac2b8ea82d3010000001976a914783cf0be521101942da509846ea476e683aad83288ac735d855f").unwrap();
    let expected_secret = [1; 32];
    let secret_hash = &*dhash160(&expected_secret);
    let actual = block_on(coin.extract_secret(secret_hash, &spend_tx, false));
    assert_eq!(actual, Ok(expected_secret));
}

#[test]
fn test_generate_token_transfer_script_pubkey() {
    // priv_key of qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG
    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);

    let gas_limit = 2_500_000;
    let gas_price = 40;

    // sample QRC20 transfer from https://testnet.qtum.info/tx/51e9cec885d7eb26271f8b1434c000f6cf07aad47671268fc8d36cee9d48f6de
    // the script is a script_pubkey of one of the transaction output
    let expected_script: Script = "5403a02526012844a9059cbb0000000000000000000000000240b898276ad2cc0d2fe6f527e8e31104e7fde3000000000000000000000000000000000000000000000000000000003b9aca0014d362e096e873eb7907e205fadc6175c6fec7bc44c2".into();
    let expected = ContractCallOutput {
        value: 0,
        script_pubkey: expected_script.to_bytes(),
        gas_limit,
        gas_price,
    };

    let to_addr: UtxoAddress = UtxoAddress::from_legacyaddress(
        "qHmJ3KA6ZAjR9wGjpFASn4gtUSeFAqdZgs",
        &coin.as_ref().conf.address_prefixes,
    )
    .unwrap();
    let to_addr = qtum::contract_addr_from_utxo_addr(to_addr).unwrap();
    let amount: U256 = 1000000000.into();
    let actual = coin.transfer_output(to_addr, amount, gas_limit, gas_price).unwrap();
    assert_eq!(expected, actual);

    assert!(coin
        .transfer_output(
            to_addr, amount, 0, // gas_limit cannot be zero
            gas_price,
        )
        .is_err());

    assert!(coin
        .transfer_output(
            to_addr,
            amount,
            gas_limit,
            0, // gas_price cannot be zero
        )
        .is_err());
}

#[test]
fn test_transfer_details_by_hash() {
    // priv_key of qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG
    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);
    let tx_hash_bytes = hex::decode("85ede12ccc12fb1709c4d9e403e96c0c394b0916f2f6098d41d8dfa00013fcdb").unwrap();
    let tx_hash: [u8; 32] = tx_hash_bytes.clone().try_into().unwrap();
    let tx_hex:BytesJson = hex::decode("0100000001426d27fde82e12e1ce84e73ca41e2a30420f4c94aaa37b30d4c5b8b4f762c042040000006a473044022032665891693ee732571cefaa6d322ec5114c78259f2adbe03a0d7e6b65fbf40d022035c9319ca41e5423e09a8a613ac749a20b8f5ad6ba4ad6bb60e4a020b085d009012103693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9ffffffff050000000000000000625403a08601012844095ea7b30000000000000000000000001549128bbfb33b997949b4105b6a6371c998e212000000000000000000000000000000000000000000000000000000000000000014d362e096e873eb7907e205fadc6175c6fec7bc44c20000000000000000625403a08601012844095ea7b30000000000000000000000001549128bbfb33b997949b4105b6a6371c998e21200000000000000000000000000000000000000000000000000000000000927c014d362e096e873eb7907e205fadc6175c6fec7bc44c20000000000000000835403a0860101284c640c565ae300000000000000000000000000000000000000000000000000000000000493e0000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc440000000000000000000000000000000000000000000000000000000000000000141549128bbfb33b997949b4105b6a6371c998e212c20000000000000000835403a0860101284c640c565ae300000000000000000000000000000000000000000000000000000000000493e0000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc440000000000000000000000000000000000000000000000000000000000000001141549128bbfb33b997949b4105b6a6371c998e212c231754b04000000001976a9149e032d4b0090a11dc40fe6c47601499a35d55fbb88acf7cd8b5f").unwrap().into();

    let details = block_on(coin.transfer_details_by_hash(tx_hash.into())).unwrap();
    let mut it = details.into_iter().sorted_by(|(id_x, _), (id_y, _)| id_x.cmp(id_y));

    let expected_fee_details = |total_gas_fee: &str| -> TxFeeDetails {
        let fee = Qrc20FeeDetails {
            coin: "QTUM".into(),
            miner_fee: BigDecimal::from_str("0.15806792").unwrap(),
            gas_limit: 100000,
            gas_price: 40,
            total_gas_fee: BigDecimal::from_str(total_gas_fee).unwrap(),
        };
        TxFeeDetails::Qrc20(fee)
    };

    // qKVvtDqpnFGDxsDzck5jmLwdnD2jRH6aM8 is UTXO representation of 1549128bbfb33b997949b4105b6a6371c998e212 contract address
    let (_id, actual) = it.next().unwrap();
    let expected = TransactionDetails {
        tx: TransactionData::new_signed(tx_hex.clone(), tx_hash_bytes.to_tx_hash()),
        from: vec!["qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG".into()],
        to: vec!["qKVvtDqpnFGDxsDzck5jmLwdnD2jRH6aM8".into()],
        total_amount: BigDecimal::from_str("0.003").unwrap(),
        spent_by_me: BigDecimal::from_str("0.003").unwrap(),
        received_by_me: BigDecimal::zero(),
        my_balance_change: BigDecimal::from_str("-0.003").unwrap(),
        block_height: 699545,
        timestamp: 1602997840,
        fee_details: Some(expected_fee_details("0.00059074")),
        coin: "QRC20".into(),
        internal_id: hex::decode(
            "85ede12ccc12fb1709c4d9e403e96c0c394b0916f2f6098d41d8dfa00013fcdb00000000000000020000000000000000",
        )
        .unwrap()
        .into(),
        kmd_rewards: None,
        transaction_type: Default::default(),
        memo: None,
    };
    assert_eq!(actual, expected);

    let (_id, actual) = it.next().unwrap();
    let expected = TransactionDetails {
        tx: TransactionData::new_signed(tx_hex.clone(), tx_hash_bytes.to_tx_hash()),
        from: vec!["qKVvtDqpnFGDxsDzck5jmLwdnD2jRH6aM8".into()],
        to: vec!["qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG".into()],
        total_amount: BigDecimal::from_str("0.00295").unwrap(),
        spent_by_me: BigDecimal::zero(),
        received_by_me: BigDecimal::from_str("0.00295").unwrap(),
        my_balance_change: BigDecimal::from_str("0.00295").unwrap(),
        block_height: 699545,
        timestamp: 1602997840,
        fee_details: Some(expected_fee_details("0.00059074")),
        coin: "QRC20".into(),
        internal_id: hex::decode(
            "85ede12ccc12fb1709c4d9e403e96c0c394b0916f2f6098d41d8dfa00013fcdb00000000000000020000000000000001",
        )
        .unwrap()
        .into(),
        kmd_rewards: None,
        transaction_type: Default::default(),
        memo: None,
    };
    assert_eq!(actual, expected);

    let (_id, actual) = it.next().unwrap();
    let expected = TransactionDetails {
        tx: TransactionData::new_signed(tx_hex.clone(), tx_hash_bytes.to_tx_hash()),
        from: vec!["qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG".into()],
        to: vec!["qKVvtDqpnFGDxsDzck5jmLwdnD2jRH6aM8".into()],
        total_amount: BigDecimal::from_str("0.003").unwrap(),
        spent_by_me: BigDecimal::from_str("0.003").unwrap(),
        received_by_me: BigDecimal::zero(),
        my_balance_change: BigDecimal::from_str("-0.003").unwrap(),
        block_height: 699545,
        timestamp: 1602997840,
        fee_details: Some(expected_fee_details("0.00059118")),
        coin: "QRC20".into(),
        internal_id: hex::decode(
            "85ede12ccc12fb1709c4d9e403e96c0c394b0916f2f6098d41d8dfa00013fcdb00000000000000030000000000000000",
        )
        .unwrap()
        .into(),
        kmd_rewards: None,
        transaction_type: Default::default(),
        memo: None,
    };
    assert_eq!(actual, expected);

    let (_id, actual) = it.next().unwrap();
    let expected = TransactionDetails {
        tx: TransactionData::new_signed(tx_hex.clone(), tx_hash_bytes.to_tx_hash()),
        from: vec!["qKVvtDqpnFGDxsDzck5jmLwdnD2jRH6aM8".into()],
        to: vec!["qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG".into()],
        total_amount: BigDecimal::from_str("0.00295").unwrap(),
        spent_by_me: BigDecimal::zero(),
        received_by_me: BigDecimal::from_str("0.00295").unwrap(),
        my_balance_change: BigDecimal::from_str("0.00295").unwrap(),
        block_height: 699545,
        timestamp: 1602997840,
        fee_details: Some(expected_fee_details("0.00059118")),
        coin: "QRC20".into(),
        internal_id: hex::decode(
            "85ede12ccc12fb1709c4d9e403e96c0c394b0916f2f6098d41d8dfa00013fcdb00000000000000030000000000000001",
        )
        .unwrap()
        .into(),
        kmd_rewards: None,
        transaction_type: Default::default(),
        memo: None,
    };
    assert_eq!(actual, expected);

    let (_id, actual) = it.next().unwrap();
    let expected = TransactionDetails {
        tx: TransactionData::new_signed(tx_hex, tx_hash_bytes.to_tx_hash()),
        from: vec!["qKVvtDqpnFGDxsDzck5jmLwdnD2jRH6aM8".into()],
        to: vec!["qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG".into()],
        total_amount: BigDecimal::from_str("0.00005000").unwrap(),
        spent_by_me: BigDecimal::zero(),
        received_by_me: BigDecimal::from_str("0.00005000").unwrap(),
        my_balance_change: BigDecimal::from_str("0.00005000").unwrap(),
        block_height: 699545,
        timestamp: 1602997840,
        fee_details: Some(expected_fee_details("0.00059118")),
        coin: "QRC20".into(),
        internal_id: hex::decode(
            "85ede12ccc12fb1709c4d9e403e96c0c394b0916f2f6098d41d8dfa00013fcdb00000000000000030000000000000002",
        )
        .unwrap()
        .into(),
        kmd_rewards: None,
        transaction_type: Default::default(),
        memo: None,
    };
    assert_eq!(actual, expected);
    assert!(it.next().is_none());
}

#[test]
fn test_get_trade_fee() {
    // priv_key of qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG
    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);
    // check if the coin's tx fee rate is expected
    check_fee_rate(&coin, ActualFeeRate::FixedPerKb(DEFAULT_TX_FEE_RATE as u64));

    let actual_trade_fee = block_on_f01(coin.get_trade_fee()).unwrap();
    let expected_trade_fee_amount = big_decimal_from_sat(
        2 * CONTRACT_CALL_GAS_FEE + SWAP_PAYMENT_GAS_FEE + DEFAULT_TX_FEE_RATE,
        coin.utxo.decimals,
    );
    let expected = TradeFee {
        coin: "QTUM".into(),
        amount: expected_trade_fee_amount.into(),
        paid_from_trading_vol: false,
    };
    assert_eq!(actual_trade_fee, expected);
}

/// `qKEDGuogDhtH9zBnc71QtqT1KDamaR1KJ3` address has `0` allowance,
/// so only one `approve` and one `erc20Payment` contract calls should be included in the estimated trade fee.
#[test]
fn test_sender_trade_preimage_zero_allowance() {
    // priv_key of qKEDGuogDhtH9zBnc71QtqT1KDamaR1KJ3
    // please note this address should have an immutable balance
    let priv_key = [
        222, 243, 64, 156, 9, 153, 78, 253, 85, 119, 62, 117, 230, 140, 75, 69, 171, 21, 243, 19, 119, 29, 97, 174, 63,
        231, 153, 202, 20, 238, 120, 64,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);
    const EXPECTED_PAYMENT_TX_FEE: i64 = 535;
    const EXPECTED_REFUND_TX_FEE: i64 = 396;
    // check if the coin's tx fee rate is expected
    check_fee_rate(&coin, ActualFeeRate::FixedPerKb(DEFAULT_TX_FEE_RATE as u64));

    let allowance = block_on(coin.allowance(coin.swap_contract_address)).expect("!allowance");
    assert_eq!(allowance, 0.into());

    let erc20_payment_fee_with_one_approve = big_decimal_from_sat(
        CONTRACT_CALL_GAS_FEE + SWAP_PAYMENT_GAS_FEE + EXPECTED_PAYMENT_TX_FEE,
        coin.utxo.decimals,
    );
    let sender_refund_fee = big_decimal_from_sat(CONTRACT_CALL_GAS_FEE + EXPECTED_REFUND_TX_FEE, coin.utxo.decimals);

    let actual =
        block_on(coin.get_sender_trade_fee(TradePreimageValue::Exact(1.into()), FeeApproxStage::TradePreimageMax)) // pass TradePreimageMax to add change output txfee, to correct the max vol calc
            .expect("!get_sender_trade_fee");
    // one `approve` contract call should be included into the expected trade fee
    let expected = TradeFee {
        coin: "QTUM".to_owned(),
        amount: (erc20_payment_fee_with_one_approve + sender_refund_fee).into(),
        paid_from_trading_vol: false,
    };
    assert_eq!(actual, expected);
}

/// `qeUbAVgkPiF62syqd792VJeB9BaqMtLcZV` address has `3` allowance,
/// so if the value is `2.5`, then only one `erc20Payment` contract call should be included in the estimated trade fee,
/// if the value is `3.5`, then two `approve` and one `erc20Payment` contract call should be included in the estimated trade fee.
#[test]
fn test_sender_trade_preimage_with_allowance() {
    // priv_key of qeUbAVgkPiF62syqd792VJeB9BaqMtLcZV
    // please note this address should have an immutable balance
    let priv_key = [
        32, 192, 195, 65, 165, 53, 21, 68, 180, 241, 67, 147, 54, 54, 41, 117, 174, 253, 139, 155, 56, 101, 69, 39, 32,
        143, 221, 19, 47, 74, 175, 100,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);
    const EXPECTED_PAYMENT_WITHOUT_APPROVE_TX_FEE: i64 = 576;
    const EXPECTED_PAYMENT_WITH_APPROVES_TX_FEE: i64 = 790;
    const EXPECTED_REFUND_TX_FEE: i64 = 544;
    // check if the coin's tx fee rate is expected
    check_fee_rate(&coin, ActualFeeRate::FixedPerKb(DEFAULT_TX_FEE_RATE as u64));

    let allowance = block_on(coin.allowance(coin.swap_contract_address)).expect("!allowance");
    assert_eq!(allowance, 300_000_000.into());

    let erc20_payment_fee_without_approve = big_decimal_from_sat(
        SWAP_PAYMENT_GAS_FEE + EXPECTED_PAYMENT_WITHOUT_APPROVE_TX_FEE,
        coin.utxo.decimals,
    );
    let erc20_payment_fee_with_two_approves = big_decimal_from_sat(
        2 * CONTRACT_CALL_GAS_FEE + SWAP_PAYMENT_GAS_FEE + EXPECTED_PAYMENT_WITH_APPROVES_TX_FEE,
        coin.utxo.decimals,
    );
    let sender_refund_fee = big_decimal_from_sat(CONTRACT_CALL_GAS_FEE + EXPECTED_REFUND_TX_FEE, coin.utxo.decimals);

    let actual = block_on(coin.get_sender_trade_fee(
        TradePreimageValue::Exact(BigDecimal::try_from(2.5).unwrap()),
        FeeApproxStage::TradePreimageMax,
    ))
    .expect("!get_sender_trade_fee");
    // the expected fee should not include any `approve` contract call
    let expected = TradeFee {
        coin: "QTUM".to_owned(),
        amount: (erc20_payment_fee_without_approve + sender_refund_fee.clone()).into(),
        paid_from_trading_vol: false,
    };
    assert_eq!(actual, expected);

    let actual = block_on(coin.get_sender_trade_fee(
        TradePreimageValue::Exact(BigDecimal::try_from(3.5).unwrap()),
        FeeApproxStage::TradePreimageMax,
    ))
    .expect("!get_sender_trade_fee");
    // two `approve` contract calls should be included into the expected trade fee
    let expected = TradeFee {
        coin: "QTUM".to_owned(),
        amount: (erc20_payment_fee_with_two_approves + sender_refund_fee).into(),
        paid_from_trading_vol: false,
    };
    assert_eq!(actual, expected);
}

#[test]
fn test_get_sender_trade_fee_preimage_for_correct_ticker() {
    // where balance for tQTUM is at 0 for the priv_key below (using 0 bal just to make sure we get required(TradePreimageError::NotSufficientBalance) result for get_sender_trade_fee)
    let priv_key = [
        48, 88, 65, 23, 20, 154, 63, 74, 243, 8, 108, 134, 154, 199, 60, 197, 51, 238, 7, 68, 199, 14, 127, 221, 89,
        80, 37, 174, 221, 178, 233, 65,
    ];
    let conf = json!({
        "coin":"QRC20",
        "required_confirmations":0,
        "pubtype":120,
        "p2shtype":110,
        "wiftype":128,
        "mm2":1,
        "mature_confirmations":2000,
        "protocol": {
        "type": "QRC20",
        "protocol_data": {
        "platform": "tQTUM",
        "contract_address": "0xd362e096e873eb7907e205fadc6175c6fec7bc44"
      }
    }
    });
    let req = json!({
        "method": "electrum",
        "servers": [{"url":"electrum1.cipig.net:10071"}, {"url":"electrum2.cipig.net:10071"}, {"url":"electrum3.cipig.net:10071"}],
        "swap_contract_address": "0xba8b71f3544b93e2f681f996da519a98ace0107a",
    });

    let contract_address = H160::from_str("0xd362e096e873eb7907e205fadc6175c6fec7bc44").unwrap();
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = Qrc20ActivationParams::from_legacy_req(&req).unwrap();

    let coin = block_on(qrc20_coin_with_priv_key(
        &ctx,
        "QRC20",
        "tQTUM",
        &conf,
        &params,
        Secp256k1Secret::from(priv_key),
        contract_address,
    ))
    .unwrap();

    let actual = block_on(coin.get_sender_trade_fee(TradePreimageValue::Exact(0.into()), FeeApproxStage::OrderIssue))
        .err()
        .unwrap()
        .into_inner();
    // expecting TradePreimageError::NotSufficientBalance
    let expected = TradePreimageError::NotSufficientBalance {
        coin: "tQTUM".to_string(),
        available: BigDecimal::from_str("0").unwrap(),
        required: BigDecimal::from_str("0.08").unwrap(),
    };
    assert_eq!(expected, actual);
}

/// `receiverSpend` should be included in the estimated trade fee.
#[test]
fn test_receiver_trade_preimage() {
    // priv_key of qeUbAVgkPiF62syqd792VJeB9BaqMtLcZV
    // please note this address should have an immutable balance
    let priv_key = [
        32, 192, 195, 65, 165, 53, 21, 68, 180, 241, 67, 147, 54, 54, 41, 117, 174, 253, 139, 155, 56, 101, 69, 39, 32,
        143, 221, 19, 47, 74, 175, 100,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);
    const EXPECTED_TX_FEE: i64 = 544;
    // check if the coin's tx fee rate is expected
    check_fee_rate(&coin, ActualFeeRate::FixedPerKb(DEFAULT_TX_FEE_RATE as u64));

    let actual =
        block_on_f01(coin.get_receiver_trade_fee(FeeApproxStage::WithoutApprox)).expect("!get_receiver_trade_fee");
    // only one contract call should be included into the expected trade fee
    let expected_receiver_fee = big_decimal_from_sat(CONTRACT_CALL_GAS_FEE + EXPECTED_TX_FEE, coin.utxo.decimals);
    let expected = TradeFee {
        coin: "QTUM".to_owned(),
        amount: expected_receiver_fee.into(),
        paid_from_trading_vol: false,
    };
    assert_eq!(actual, expected);
}

/// `qeUbAVgkPiF62syqd792VJeB9BaqMtLcZV` address has `5` QRC20 tokens.
#[test]
fn test_taker_fee_tx_fee() {
    // priv_key of qeUbAVgkPiF62syqd792VJeB9BaqMtLcZV
    // please note this address should have an immutable balance
    let priv_key = [
        32, 192, 195, 65, 165, 53, 21, 68, 180, 241, 67, 147, 54, 54, 41, 117, 174, 253, 139, 155, 56, 101, 69, 39, 32,
        143, 221, 19, 47, 74, 175, 100,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);
    const EXPECTED_TX_FEE: i64 = 447;
    // check if the coin's tx fee rate is expected
    check_fee_rate(&coin, ActualFeeRate::FixedPerKb(DEFAULT_TX_FEE_RATE as u64));
    let expected_balance = CoinBalance {
        spendable: BigDecimal::from(5u32),
        unspendable: BigDecimal::from(0u32),
    };
    assert_eq!(block_on_f01(coin.my_balance()).expect("!my_balance"), expected_balance);

    let dex_fee_amount = BigDecimal::from(5u32);
    let actual = block_on(coin.get_fee_to_send_taker_fee(
        DexFee::Standard(MmNumber::from(dex_fee_amount)),
        FeeApproxStage::WithoutApprox,
    ))
    .expect("!get_fee_to_send_taker_fee");
    // only one contract call should be included into the expected trade fee
    let expected_receiver_fee = big_decimal_from_sat(CONTRACT_CALL_GAS_FEE + EXPECTED_TX_FEE, coin.utxo.decimals);
    let expected = TradeFee {
        coin: "QTUM".to_owned(),
        amount: expected_receiver_fee.into(),
        paid_from_trading_vol: false,
    };
    assert_eq!(actual, expected);
}

#[test]
fn test_coin_from_conf_without_decimals() {
    // priv_key of qXxsj5RtciAby9T7m98AgAATL4zTi4UwDG
    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let conf = json!({
        "coin":"QRC20",
        "required_confirmations":0,
        "pubtype":120,
        "p2shtype":110,
        "wiftype":128,
        "mm2":1,
        "mature_confirmations":2000,
    });
    let req = json!({
        "method": "electrum",
        "servers": [{"url":"electrum1.cipig.net:10071"}, {"url":"electrum2.cipig.net:10071"}, {"url":"electrum3.cipig.net:10071"}],
        "swap_contract_address": "0xba8b71f3544b93e2f681f996da519a98ace0107a",
    });
    // 0459c999c3edf05e73c83f3fbae9f0f020919f91 has 12 decimals instead of standard 8
    let contract_address = H160::from_str("0x0459c999c3edf05e73c83f3fbae9f0f020919f91").unwrap();
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = Qrc20ActivationParams::from_legacy_req(&req).unwrap();

    let coin = block_on(qrc20_coin_with_priv_key(
        &ctx,
        "QRC20",
        "QTUM",
        &conf,
        &params,
        Secp256k1Secret::from(priv_key),
        contract_address,
    ))
    .unwrap();

    assert_eq!(coin.utxo.decimals, 12);
    assert_eq!(coin.decimals(), 12);
}

/// Test [`Qrc20Coin::validate_maker_payment`] and [`Qrc20Coin::erc20_payment_details_from_tx`]
/// with malicious maker payment.
///
/// Maker could send a payment to another malicious swap contract with the same `erc20Payment` function.
/// He could pass the correct arguments and this malicious swap contract could emit a `Transfer` event with the correct topics.
///
/// Example of malicious `erc20Payment` function:
///
/// ```solidity
/// function erc20Payment(
///     bytes32 _id,
///     uint256 _amount,
///     address _tokenAddress,
///     address _receiver,
///     bytes20 _secretHash,
///     uint64 _lockTime
/// ) external payable {
///     require(_receiver != address(0) && _amount > 0 && payments[_id].state == PaymentState.Uninitialized);
///     bytes20 paymentHash = ripemd160(abi.encodePacked(
///         _receiver,
///         msg.sender,
///         _secretHash,
///         _tokenAddress,
///         _amount
///     ));
///     payments[_id] = Payment(
///         paymentHash,
///         _lockTime,
///         PaymentState.PaymentSent
///     );
///     // actual swap contract address 0xba8b71f3544b93e2f681f996da519a98ace0107a in Mixed-case address format
///     address swapContract = 0xbA8B71f3544b93E2f681f996da519A98aCE0107A;
///     IERC20 token = IERC20(_tokenAddress);
///     // transfer some little amounts from the sender to actual swap contract to emit a `Transfer` event with the correct topics
///     require(token.transferFrom(msg.sender, swapContract, 1000));
///     emit PaymentSent(_id);
/// }
/// ```
///
/// In the function above maker spent only 1000 amount, but value is 100000000 in the arguments.
/// Note maker initialized payment with the corresponding swap_id in
/// b61ef2f9911d075e80a3623444cce8fd948932f66c9148283860d46e9af4f2c8 tx.
#[test]
fn test_validate_maker_payment_malicious() {
    // priv_key of qUX9FGHubczidVjWPCUWuwCUJWpkAtGCgf
    let priv_key = [
        24, 181, 194, 193, 18, 152, 142, 168, 71, 73, 70, 244, 9, 101, 92, 168, 243, 61, 132, 48, 25, 39, 103, 92, 29,
        17, 11, 29, 113, 235, 48, 70,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);

    // Malicious tx 81540dc6abe59cf1e301a97a7e1c9b66d5f475da916faa3f0ef7ea896c0b3e5a
    let payment_tx = hex::decode("01000000010144e2b8b5e6da0666faf1db95075653ef49e2acaa8924e1ec595f6b89a6f715050000006a4730440220415adec5e24148db8e9654a6beda4b1af8aded596ab1cd8667af32187853e8f5022007a91d44ee13046194aafc07ca46ec44f770e75b41187acaa4e38e17d4eccb5d012103693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9ffffffff030000000000000000625403a08601012844095ea7b300000000000000000000000085a4df739bbb2d247746bea611d5d365204725830000000000000000000000000000000000000000000000000000000005f5e10014d362e096e873eb7907e205fadc6175c6fec7bc44c20000000000000000e35403a0860101284cc49b415b2a0a1a8b4af2762154115ced87e2424b3cb940c0181cc3c850523702f1ec298fef0000000000000000000000000000000000000000000000000000000005f5e100000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc44000000000000000000000000783cf0be521101942da509846ea476e683aad8324b6b2e5444c2639cc0fb7bcea5afba3f3cdce239000000000000000000000000000000000000000000000000000000000000000000000000000000005fa0fffb1485a4df739bbb2d247746bea611d5d36520472583c208535c01000000001976a9149e032d4b0090a11dc40fe6c47601499a35d55fbb88acc700a15f").unwrap();
    let maker_pub = hex::decode("03693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9").unwrap();
    let secret = &[1; 32];
    let secret_hash = dhash160(secret).to_vec();
    let amount = BigDecimal::from_str("1").unwrap();

    let input = ValidatePaymentInput {
        payment_tx,
        time_lock_duration: 0,
        time_lock: 1604386811,
        secret_hash,
        amount,
        swap_contract_address: coin.swap_contract_address(),
        try_spv_proof_until: wait_until_sec(30),
        confirmations: 1,
        other_pub: maker_pub,
        unique_swap_data: Vec::new(),
        watcher_reward: None,
    };
    let error = block_on(coin.validate_maker_payment(input))
        .expect_err("'erc20Payment' was called from another swap contract, expected an error")
        .into_inner();
    log!("error: {}", error);
    match error {
        ValidatePaymentError::TxDeserializationError(err) => {
            assert!(err.contains("Unexpected amount 1000 in 'Transfer' event, expected 100000000"))
        },
        _ => panic!("Expected `TxDeserializationError` unexpected amount, found {:?}", error),
    }
}

#[test]
fn test_negotiate_swap_contract_addr_no_fallback() {
    let (_, coin) = qrc20_coin_for_test([1; 32], None);

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

#[test]
fn test_negotiate_swap_contract_addr_has_fallback() {
    let fallback = "0x8500AFc0bc5214728082163326C2FF0C73f4a871";
    let fallback_addr = qtum::contract_addr_from_str(fallback).unwrap();

    let (_, coin) = qrc20_coin_for_test([1; 32], Some(fallback));

    let input = None;
    let result = coin.negotiate_swap_contract_addr(input).unwrap();
    assert_eq!(Some(fallback_addr.0.to_vec().into()), result);

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

    let slice: &[u8] = fallback_addr.as_ref();
    let result = coin.negotiate_swap_contract_addr(Some(slice)).unwrap();
    assert_eq!(Some(fallback_addr.0.to_vec().into()), result);
}

#[test]
fn test_send_contract_calls_recoverable_tx() {
    let priv_key = [
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ];
    let (_ctx, coin) = qrc20_coin_for_test(priv_key, None);

    let tx = TransactionEnum::UtxoTx("010000000160fd74b5714172f285db2b36f0b391cd6883e7291441631c8b18f165b0a4635d020000006a47304402205d409e141111adbc4f185ae856997730de935ac30a0d2b1ccb5a6c4903db8171022024fc59bbcfdbba283556d7eeee4832167301dc8e8ad9739b7865f67b9676b226012103693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9ffffffff020000000000000000625403a08601012844a9059cbb000000000000000000000000ca1e04745e8ca0c60d8c5881531d51bec470743f00000000000000000000000000000000000000000000000000000000000f424014d362e096e873eb7907e205fadc6175c6fec7bc44c200ada205000000001976a9149e032d4b0090a11dc40fe6c47601499a35d55fbb88acfe967d5f".into());

    let fee_addr = hex::decode("03bc2c7ba671bae4a6fc835244c9762b41647b9827d4780a89a949b984a8ddcc05").unwrap();
    let to_address = coin.contract_address_from_raw_pubkey(&fee_addr).unwrap();
    let amount = BigDecimal::try_from(0.2).unwrap();
    let amount = u256_from_big_decimal(&amount, coin.utxo.decimals).unwrap();
    let mut transfer_output = coin
        .transfer_output(to_address, amount, QRC20_GAS_LIMIT_DEFAULT, QRC20_GAS_PRICE_DEFAULT)
        .unwrap();

    // break the transfer output
    transfer_output.value = 777;
    transfer_output.gas_limit = 777;
    transfer_output.gas_price = 777;

    let tx_err = block_on(coin.send_contract_calls(vec![transfer_output])).unwrap_err();

    // The error variant should equal to `TxRecoverable`
    assert_eq!(
        discriminant(&tx_err),
        discriminant(&TransactionErr::TxRecoverable(tx, String::new()))
    );
}
