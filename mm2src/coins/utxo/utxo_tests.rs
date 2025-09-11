use super::*;
use crate::coin_balance::HDAddressBalance;
use crate::coin_errors::ValidatePaymentError;
use crate::hd_wallet::{
    HDAccountsMap, HDAccountsMutex, HDAddressesCache, HDConfirmAddress, HDConfirmAddressError, HDWallet,
    HDWalletCoinStorage, HDWalletMockStorage, HDWalletStorageInternalOps, MockableConfirmAddress,
};
use crate::my_tx_history_v2::for_tests::init_storage_for;
use crate::my_tx_history_v2::CoinWithTxHistoryV2;
use crate::rpc_command::account_balance::{AccountBalanceParams, AccountBalanceRpcOps, HDAccountBalanceResponse};
use crate::rpc_command::get_new_address::{GetNewAddressParams, GetNewAddressRpcError, GetNewAddressRpcOps};
use crate::rpc_command::init_scan_for_new_addresses::{
    InitScanAddressesRpcOps, ScanAddressesParams, ScanAddressesResponse,
};
use crate::utxo::qtum::{qtum_coin_with_priv_key, QtumCoin, QtumDelegationOps, QtumDelegationRequest};
#[cfg(not(target_arch = "wasm32"))]
use crate::utxo::rpc_clients::{BlockHashOrHeight, NativeUnspent};
use crate::utxo::rpc_clients::{
    ElectrumBalance, ElectrumBlockHeader, ElectrumClient, ElectrumClientImpl, GetAddressInfoRes, ListSinceBlockRes,
    NativeClient, NativeClientImpl, NetworkInfo, UtxoRpcClientOps, ValidateAddressRes, VerboseBlock,
};
use crate::utxo::spv::SimplePaymentVerification;
#[cfg(not(target_arch = "wasm32"))]
use crate::utxo::utxo_block_header_storage::{BlockHeaderStorage, SqliteBlockHeadersStorage};
use crate::utxo::utxo_builder::{UtxoArcBuilder, UtxoCoinBuilder, UtxoCoinBuilderCommonOps};
use crate::utxo::utxo_common::UtxoTxBuilder;
#[cfg(not(target_arch = "wasm32"))]
use crate::utxo::utxo_common_tests::TEST_COIN_DECIMALS;
use crate::utxo::utxo_common_tests::{self, utxo_coin_fields_for_test, utxo_coin_from_fields, TEST_COIN_NAME};
use crate::utxo::utxo_hd_wallet::UtxoHDAccount;
use crate::utxo::utxo_standard::{utxo_standard_coin_with_priv_key, UtxoStandardCoin};
use crate::utxo::utxo_tx_history_v2::{UtxoTxDetailsParams, UtxoTxHistoryOps};
use crate::{
    BlockHeightAndTime, CoinBalance, CoinBalanceMap, ConfirmPaymentInput, DexFee, IguanaPrivKey, PrivKeyBuildPolicy,
    SearchForSwapTxSpendInput, SpendPaymentArgs, StakingInfosDetails, SwapOps, TradePreimageValue, TxFeeDetails,
    TxMarshalingErr, ValidateFeeArgs, INVALID_SENDER_ERR_LOG,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::{WaitForHTLCTxSpendArgs, WithdrawFee};
use chain::{BlockHeader, BlockHeaderBits, OutPoint};
use common::executor::Timer;
use common::{block_on, block_on_f01, wait_until_sec, OrdRange, PagingOptionsEnum, DEX_FEE_ADDR_RAW_PUBKEY};
use crypto::{privkey::key_pair_from_seed, Bip44Chain, HDPathToAccount, RpcDerivationPath, Secp256k1Secret};
#[cfg(not(target_arch = "wasm32"))]
use db_common::sqlite::rusqlite::Connection;
use futures::channel::mpsc::channel;
use futures::future::{join_all, Either, FutureExt, TryFutureExt};
use hex::FromHex;
use keys::prefixes::*;
use mm2_core::mm_ctx::MmCtxBuilder;
use mm2_number::bigdecimal::{BigDecimal, Signed};
use mm2_number::MmNumber;
use mm2_test_helpers::electrums::doc_electrums;
use mm2_test_helpers::for_tests::{
    electrum_servers_rpc, mm_ctx_with_custom_db, DOC_ELECTRUM_ADDRS, MARTY_ELECTRUM_ADDRS, T_BCH_ELECTRUMS,
};
use mocktopus::mocking::*;
use rpc::v1::types::H256 as H256Json;
use serialization::{deserialize, CoinVariant, CompactInteger, Reader};
use spv_validation::conf::{BlockHeaderValidationParams, SPVBlockHeader};
use spv_validation::storage::BlockHeaderStorageOps;
use spv_validation::work::DifficultyAlgorithm;
#[cfg(not(target_arch = "wasm32"))]
use std::convert::TryFrom;
use std::iter;
use std::num::NonZeroUsize;
use std::str::FromStr;

#[cfg(test)]
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

#[cfg(not(target_arch = "wasm32"))]
const TAKER_PAYMENT_SPEND_SEARCH_INTERVAL: f64 = 1.;

pub fn electrum_client_for_test(servers: &[&str]) -> ElectrumClient {
    let ctx = MmCtxBuilder::default().into_mm_arc();
    let servers: Vec<_> = servers.iter().map(|server| json!({ "url": server })).collect();
    let req = json!({
        "method": "electrum",
        "servers": servers,
    });
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let priv_key_policy = PrivKeyBuildPolicy::IguanaPrivKey(IguanaPrivKey::default());
    let builder = UtxoArcBuilder::new(
        &ctx,
        TEST_COIN_NAME,
        &Json::Null,
        &params,
        priv_key_policy,
        UtxoStandardCoin::from,
    );
    let args = ElectrumBuilderArgs {
        spawn_ping: false,
        negotiate_version: true,
        collect_metrics: false,
    };

    let servers = servers.into_iter().map(|s| json::from_value(s).unwrap()).collect();
    let abortable_system = AbortableQueue::default();
    block_on(builder.electrum_client(abortable_system, args, servers, (None, None))).unwrap()
}

/// Returned client won't work by default, requires some mocks to be usable
#[cfg(not(target_arch = "wasm32"))]
fn native_client_for_test() -> NativeClient {
    NativeClient(Arc::new(NativeClientImpl::default()))
}

fn utxo_coin_for_test(
    rpc_client: UtxoRpcClientEnum,
    force_seed: Option<&str>,
    is_segwit_coin: bool,
) -> UtxoStandardCoin {
    utxo_coin_from_fields(utxo_coin_fields_for_test(rpc_client, force_seed, is_segwit_coin))
}

/// Returns `TransactionDetails` of the given `tx_hash` via [`UtxoStandardOps::tx_details_by_hash`].
#[track_caller]
fn get_tx_details_by_hash<Coin: UtxoStandardOps>(coin: &Coin, tx_hash: &str) -> TransactionDetails {
    let hash = <[u8; 32]>::from_hex(tx_hash).unwrap().into();
    let mut input_transactions = HistoryUtxoTxMap::new();

    block_on(UtxoStandardOps::tx_details_by_hash(
        coin,
        &hash,
        &mut input_transactions,
    ))
    .unwrap()
}

/// Returns `TransactionDetails` of the given `tx_hash` via [`UtxoTxHistoryOps::tx_details_by_hash`].
fn get_tx_details_by_hash_v2<Coin>(coin: &Coin, tx_hash: &str, height: u64, timestamp: u64) -> Vec<TransactionDetails>
where
    Coin: CoinWithTxHistoryV2 + UtxoTxHistoryOps,
{
    let my_addresses = block_on(coin.my_addresses()).unwrap();
    let (_ctx, storage) = init_storage_for(coin);
    let params = UtxoTxDetailsParams {
        hash: &<[u8; 32]>::from_hex(tx_hash).unwrap().into(),
        block_height_and_time: Some(BlockHeightAndTime { height, timestamp }),
        storage: &storage,
        my_addresses: &my_addresses,
    };

    block_on(UtxoTxHistoryOps::tx_details_by_hash(coin, params)).unwrap()
}

/// Returns `TransactionDetails` of the given `tx_hash` and checks that
/// [`UtxoTxHistoryOps::tx_details_by_hash`] and [`UtxoStandardOps::tx_details_by_hash`] return the same TX details.
#[track_caller]
fn get_tx_details_eq_for_both_versions<Coin>(coin: &Coin, tx_hash: &str) -> TransactionDetails
where
    Coin: CoinWithTxHistoryV2 + UtxoTxHistoryOps + UtxoStandardOps,
{
    let tx_details_v1 = get_tx_details_by_hash(coin, tx_hash);
    let tx_details_v2 = get_tx_details_by_hash_v2(coin, tx_hash, tx_details_v1.block_height, tx_details_v1.timestamp);

    assert_eq!(vec![tx_details_v1.clone()], tx_details_v2);
    tx_details_v1
}

#[test]
fn test_extract_secret() {
    let client = electrum_client_for_test(MARTY_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(client.into(), None, false);

    let tx_hex = hex::decode("0400008085202f890125236f423b7f585e6a86d8a6c45c6805bbd5823851a57a00f6dcd3a41dc7487500000000d8483045022100ce7246314170b7c84df41a9d987dad5b572cfca5c27ee738d2682ce147c460a402206fa477fc27bec62600b13ea8a3f81fbad1fa9adad28bc1fa5c212a12ecdccd7f01205c62072b57b6473aeee6d35270c8b56d86975e6d6d4245b25425d771239fae32004c6b630476ac3765b1752103242d9cb2168968d785f6914c494c303ff1c27ba0ad882dbc3c15cfa773ea953cac6782012088a914f95ae6f5fb6a4c4e69b00b4c1dbc0698746c0f0288210210e0f210673a2024d4021270bb711664a637bb542317ed9be5ad592475320c0cac68ffffffff0128230000000000001976a9142c445a7af3da3feb2ba7d5f2a32002c772acc1e188ac76ac3765000000000000000000000000000000").unwrap();
    let expected_secret =
        <[u8; 32]>::from_hex("5c62072b57b6473aeee6d35270c8b56d86975e6d6d4245b25425d771239fae32").unwrap();
    let secret_hash = &*dhash160(&expected_secret);
    let secret = block_on(coin.extract_secret(secret_hash, &tx_hex, false)).unwrap();
    assert_eq!(secret, expected_secret);
}

#[test]
fn test_send_maker_spends_taker_payment_recoverable_tx() {
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(client.into(), None, false);
    let tx_hex = hex::decode("0100000001de7aa8d29524906b2b54ee2e0281f3607f75662cbc9080df81d1047b78e21dbc00000000d7473044022079b6c50820040b1fbbe9251ced32ab334d33830f6f8d0bf0a40c7f1336b67d5b0220142ccf723ddabb34e542ed65c395abc1fbf5b6c3e730396f15d25c49b668a1a401209da937e5609680cb30bff4a7661364ca1d1851c2506fa80c443f00a3d3bf7365004c6b6304f62b0e5cb175210270e75970bb20029b3879ec76c4acd320a8d0589e003636264d01a7d566504bfbac6782012088a9142fb610d856c19fd57f2d0cffe8dff689074b3d8a882103f368228456c940ac113e53dad5c104cf209f2f102a409207269383b6ab9b03deac68ffffffff01d0dc9800000000001976a9146d9d2b554d768232320587df75c4338ecc8bf37d88ac40280e5c").unwrap();
    let secret = hex::decode("9da937e5609680cb30bff4a7661364ca1d1851c2506fa80c443f00a3d3bf7365").unwrap();
    let maker_spends_payment_args = SpendPaymentArgs {
        other_payment_tx: &tx_hex,
        time_lock: 777,
        other_pubkey: coin.my_public_key().unwrap(),
        secret: &secret,
        secret_hash: &*dhash160(&secret),
        swap_contract_address: &coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let tx_err = block_on(coin.send_maker_spends_taker_payment(maker_spends_payment_args))
        .expect_err("!send_maker_spends_taker_payment should error missing tx inputs");

    // The error variant should be `TxRecoverable`
    assert!(matches!(tx_err, TransactionErr::TxRecoverable(_, _)));
}

#[test]
fn test_generate_transaction() {
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(client.into(), None, false);
    let unspents = vec![UnspentInfo {
        value: 10000000000,
        outpoint: OutPoint::default(),
        height: Default::default(),
        script: Vec::new().into(),
    }];

    let outputs = vec![TransactionOutput {
        script_pubkey: vec![].into(),
        value: 999,
    }];

    let builder = block_on(UtxoTxBuilder::new(&coin))
        .add_available_inputs(unspents)
        .add_outputs(outputs);
    let generated = block_on(builder.build());
    // must not allow to use output with value < dust
    generated.unwrap_err();

    let unspents = vec![UnspentInfo {
        value: 100000,
        outpoint: OutPoint::default(),
        height: Default::default(),
        script: Vec::new().into(),
    }];

    let outputs = vec![TransactionOutput {
        script_pubkey: vec![].into(),
        value: 98781,
    }];

    let builder = block_on(UtxoTxBuilder::new(&coin))
        .add_available_inputs(unspents)
        .add_outputs(outputs);
    let generated = block_on(builder.build()).unwrap();
    // the change that is less than dust must be included to miner fee
    // so no extra outputs should appear in generated transaction
    assert_eq!(generated.0.outputs.len(), 1);

    assert_eq!(generated.1.fee_amount, 220 + 999);
    assert_eq!(generated.1.received_by_me, 0);
    assert_eq!(generated.1.spent_by_me, 100000);

    let unspents = vec![UnspentInfo {
        value: 100000,
        outpoint: OutPoint::default(),
        height: Default::default(),
        script: Vec::new().into(),
    }];

    let outputs = vec![TransactionOutput {
        script_pubkey: Builder::build_p2pkh(block_on(coin.as_ref().derivation_method.unwrap_single_addr()).hash())
            .to_bytes(),
        value: 100000,
    }];

    // test that fee is properly deducted from output amount equal to input amount (max withdraw case)
    let builder = block_on(UtxoTxBuilder::new(&coin))
        .add_available_inputs(unspents)
        .add_outputs(outputs)
        .with_fee_policy(FeePolicy::DeductFromOutput(0));

    let generated = block_on(builder.build()).unwrap();
    assert_eq!(generated.0.outputs.len(), 1);

    assert_eq!(generated.1.fee_amount, 211);
    assert_eq!(generated.1.received_by_me, 99789);
    assert_eq!(generated.1.spent_by_me, 100000);
    assert_eq!(generated.0.outputs[0].value, 99789);

    let unspents = vec![UnspentInfo {
        value: 100000,
        outpoint: OutPoint::default(),
        height: Default::default(),
        script: Vec::new().into(),
    }];

    let outputs = vec![TransactionOutput {
        script_pubkey: vec![].into(),
        value: 100000,
    }];

    // test that generate_transaction returns an error when input amount is not sufficient to cover output + fee
    let builder = block_on(UtxoTxBuilder::new(&coin))
        .add_available_inputs(unspents)
        .add_outputs(outputs);

    block_on(builder.build()).unwrap_err();
}

#[test]
fn test_addresses_from_script() {
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(client.into(), None, false);
    // P2PKH
    let script: Script = "76a91405aab5342166f8594baf17a7d9bef5d56744332788ac".into();
    let expected_addr: Vec<Address> = vec![Address::from_legacyaddress(
        "R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW",
        &coin.as_ref().conf.address_prefixes,
    )
    .unwrap()];
    let actual_addr = coin.addresses_from_script(&script).unwrap();
    assert_eq!(expected_addr, actual_addr);

    // P2SH
    let script: Script = "a914e71a6120653ebd526e0f9d7a29cde5969db362d487".into();
    let expected_addr: Vec<Address> = vec![Address::from_legacyaddress(
        "bZoEPR7DjTqSDiQTeRFNDJuQPTRY2335LD",
        &coin.as_ref().conf.address_prefixes,
    )
    .unwrap()];
    let actual_addr = coin.addresses_from_script(&script).unwrap();
    assert_eq!(expected_addr, actual_addr);
}

#[test]
fn test_kmd_interest() {
    let height = Some(1000001);
    let value = 64605500822;
    let lock_time = 1556623906;
    let current_time = 1556623906 + 3600 + 300;

    let expected = 36870;
    let actual = kmd_interest(height, value, lock_time, current_time).unwrap();
    assert_eq!(expected, actual);

    // UTXO amount must be at least 10 KMD to be eligible for interest
    let actual = kmd_interest(height, 999999999, lock_time, current_time);
    assert_eq!(actual, Err(KmdRewardsNotAccruedReason::UtxoAmountLessThanTen));

    // Transaction is not mined yet (height is None)
    let actual = kmd_interest(None, value, lock_time, current_time);
    assert_eq!(actual, Err(KmdRewardsNotAccruedReason::TransactionInMempool));

    // Locktime is not set
    let actual = kmd_interest(height, value, 0, current_time);
    assert_eq!(actual, Err(KmdRewardsNotAccruedReason::LocktimeNotSet));

    // interest will stop accrue after block 7_777_777
    let actual = kmd_interest(Some(7_777_778), value, lock_time, current_time);
    assert_eq!(actual, Err(KmdRewardsNotAccruedReason::UtxoHeightGreaterThanEndOfEra));

    // interest doesn't accrue for lock_time < 500_000_000
    let actual = kmd_interest(height, value, 499_999_999, current_time);
    assert_eq!(actual, Err(KmdRewardsNotAccruedReason::LocktimeLessThanThreshold));

    // current time must be greater than tx lock_time
    let actual = kmd_interest(height, value, lock_time, lock_time - 1);
    assert_eq!(actual, Err(KmdRewardsNotAccruedReason::OneHourNotPassedYet));

    // at least 1 hour should pass
    let actual = kmd_interest(height, value, lock_time, lock_time + 30);
    assert_eq!(actual, Err(KmdRewardsNotAccruedReason::OneHourNotPassedYet));
}

#[test]
fn test_kmd_interest_accrue_stop_at() {
    let lock_time = 1595845640;
    let height = 1000001;

    let expected = lock_time + 31 * 24 * 60 * 60;
    let actual = kmd_interest_accrue_stop_at(height, lock_time);
    assert_eq!(expected, actual);

    let height = 999999;

    let expected = lock_time + 365 * 24 * 60 * 60;
    let actual = kmd_interest_accrue_stop_at(height, lock_time);
    assert_eq!(expected, actual);
}

#[test]
// Test case taken from this PR: https://github.com/KomodoPlatform/komodo/pull/584
fn test_kmd_interest_kip_0001_reduction() {
    let height = Some(7777776);
    let value = 64605500822;
    let lock_time = 1663839248;
    let current_time = 1663839248 + (31 * 24 * 60 - 1) * 60 + 3600;

    // Starting from dPoW 7th season, according to KIP0001 AUR should be reduced from 5% to 0.01%, i.e. div by 500
    let expected = value / 10512000 * (31 * 24 * 60 - 59) / 500;
    log!("expected: {}", expected);
    let actual = kmd_interest(height, value, lock_time, current_time).unwrap();
    assert_eq!(expected, actual);
}

#[test]
fn test_sat_from_big_decimal() {
    let amount = "0.000001".parse().unwrap();
    let sat = sat_from_big_decimal(&amount, 18).unwrap();
    let expected_sat = 1000000000000;
    assert_eq!(expected_sat, sat);

    let amount = "0.12345678".parse().unwrap();
    let sat = sat_from_big_decimal(&amount, 8).unwrap();
    let expected_sat = 12345678;
    assert_eq!(expected_sat, sat);

    let amount = "1.000001".parse().unwrap();
    let sat = sat_from_big_decimal(&amount, 18).unwrap();
    let expected_sat = 1000001000000000000;
    assert_eq!(expected_sat, sat);

    let amount = 1.into();
    let sat = sat_from_big_decimal(&amount, 18).unwrap();
    let expected_sat = 1000000000000000000;
    assert_eq!(expected_sat, sat);

    let amount = "0.000000000000000001".parse().unwrap();
    let sat = sat_from_big_decimal(&amount, 18).unwrap();
    let expected_sat = 1u64;
    assert_eq!(expected_sat, sat);

    let amount = 1234.into();
    let sat = sat_from_big_decimal(&amount, 9).unwrap();
    let expected_sat = 1234000000000;
    assert_eq!(expected_sat, sat);

    let amount = 1234.into();
    let sat = sat_from_big_decimal(&amount, 0).unwrap();
    let expected_sat = 1234;
    assert_eq!(expected_sat, sat);

    let amount = 1234.into();
    let sat = sat_from_big_decimal(&amount, 1).unwrap();
    let expected_sat = 12340;
    assert_eq!(expected_sat, sat);

    let amount = "1234.12345".parse().unwrap();
    let sat = sat_from_big_decimal(&amount, 1).unwrap();
    let expected_sat = 12341;
    assert_eq!(expected_sat, sat);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_wait_for_payment_spend_timeout_native() {
    let client = NativeClientImpl::default();

    static OUTPUT_SPEND_CALLED: AtomicBool = AtomicBool::new(false);
    NativeClient::find_output_spend.mock_safe(|_, _, _, _, _, _| {
        OUTPUT_SPEND_CALLED.store(true, Ordering::Relaxed);
        MockResult::Return(Box::new(futures01::future::ok(None)))
    });
    let client = UtxoRpcClientEnum::Native(NativeClient(Arc::new(client)));
    let coin = utxo_coin_for_test(client, None, false);
    let transaction = hex::decode("01000000000102fff7f7881a8099afa6940d42d1e7f6362bec38171ea3edf433541db4e4ad969f00000000494830450221008b9d1dc26ba6a9cb62127b02742fa9d754cd3bebf337f7a55d114c8e5cdd30be022040529b194ba3f9281a99f2b1c0a19c0489bc22ede944ccf4ecbab4cc618ef3ed01eeffffffef51e1b804cc89d182d279655c3aa89e815b1b309fe287d9b2b55d57b90ec68a0100000000ffffffff02202cb206000000001976a9148280b37df378db99f66f85c95a783a76ac7a6d5988ac9093510d000000001976a9143bde42dbee7e4dbe6a21b2d50ce2f0167faa815988ac000247304402203609e17b84f6a7d30c80bfa610b5b4542f32a8a0d5447a12fb1366d7f01cc44a0220573a954c4518331561406f90300e8f3358f51928d43c212a8caed02de67eebee0121025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee635711000000")
        .unwrap();
    let wait_until = now_sec() - 1;
    let from_block = 1000;

    assert!(block_on(coin.wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
        tx_bytes: &transaction,
        secret_hash: &[],
        wait_until,
        from_block,
        swap_contract_address: &None,
        check_every: TAKER_PAYMENT_SPEND_SEARCH_INTERVAL,
        watcher_reward: false
    }))
    .is_err());

    assert!(OUTPUT_SPEND_CALLED.load(Ordering::Relaxed));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_wait_for_payment_spend_timeout_electrum() {
    use mm2_event_stream::StreamingManager;
    use rpc_clients::ElectrumClientSettings;

    static OUTPUT_SPEND_CALLED: AtomicBool = AtomicBool::new(false);

    ElectrumClient::find_output_spend.mock_safe(|_, _, _, _, _, _| {
        OUTPUT_SPEND_CALLED.store(true, Ordering::Relaxed);
        MockResult::Return(Box::new(futures01::future::ok(None)))
    });

    let block_headers_storage = BlockHeaderStorage {
        inner: Box::new(SqliteBlockHeadersStorage {
            ticker: TEST_COIN_NAME.into(),
            conn: Arc::new(Mutex::new(Connection::open_in_memory().unwrap())),
        }),
    };
    let abortable_system = AbortableQueue::default();

    let client_settings = ElectrumClientSettings {
        client_name: "test".to_string(),
        servers: vec![],
        coin_ticker: TEST_COIN_NAME.into(),
        spawn_ping: true,
        negotiate_version: true,
        min_connected: 1,
        max_connected: 1,
    };
    let client = ElectrumClient::try_new(
        client_settings,
        Default::default(),
        block_headers_storage,
        StreamingManager::default(),
        abortable_system,
    )
    .expect("Expected electrum_client_impl constructed without a problem");
    let client = UtxoRpcClientEnum::Electrum(client);
    let coin = utxo_coin_for_test(client, None, false);
    let transaction = hex::decode("01000000000102fff7f7881a8099afa6940d42d1e7f6362bec38171ea3edf433541db4e4ad969f00000000494830450221008b9d1dc26ba6a9cb62127b02742fa9d754cd3bebf337f7a55d114c8e5cdd30be022040529b194ba3f9281a99f2b1c0a19c0489bc22ede944ccf4ecbab4cc618ef3ed01eeffffffef51e1b804cc89d182d279655c3aa89e815b1b309fe287d9b2b55d57b90ec68a0100000000ffffffff02202cb206000000001976a9148280b37df378db99f66f85c95a783a76ac7a6d5988ac9093510d000000001976a9143bde42dbee7e4dbe6a21b2d50ce2f0167faa815988ac000247304402203609e17b84f6a7d30c80bfa610b5b4542f32a8a0d5447a12fb1366d7f01cc44a0220573a954c4518331561406f90300e8f3358f51928d43c212a8caed02de67eebee0121025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee635711000000")
        .unwrap();
    let wait_until = now_sec() - 1;
    let from_block = 1000;

    assert!(block_on(coin.wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
        tx_bytes: &transaction,
        secret_hash: &[],
        wait_until,
        from_block,
        swap_contract_address: &None,
        check_every: TAKER_PAYMENT_SPEND_SEARCH_INTERVAL,
        watcher_reward: false
    }))
    .is_err());
    assert!(OUTPUT_SPEND_CALLED.load(Ordering::Relaxed));
}

#[test]
fn test_search_for_swap_tx_spend_electrum_was_spent() {
    let secret = hex::decode("a1c44607b870cd714a75d5243347fa36debcd3a91ff1f50b79f52d83238a0b2d").unwrap();
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(
        client.into(),
        Some("spice describe gravity federal blast come thank unfair canal monkey style afraid"),
        false,
    );

    // raw tx bytes of https://doc.komodo.earth/tx/2f216f7ddb4350ed52e4c5b3a649da7aa63c932e623a1046066f91bdf00015a0
    let payment_tx_bytes = hex::decode("0400008085202f890129f70bfc256e71600471be0a0e10f31d7025f350301e4c5aab71d4910bc29cb20200000069463043022027283d1a28ce25f3a937376f40873441a7b5f9c4a38c6637642b78845874f12b021f6cf1f4836724e186c5c299507e2c5db40855ed2c0acad73e3446a6f1e00d83012102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ffffffff0320a107000000000017a9148d3a47615c562a08ae3c42c237ca5a6f5f517b7a870000000000000000166a147c0c02ee06e3769376bb2b31e05a9e9965045ffbd2ae2b7b000000001976a91405aab5342166f8594baf17a7d9bef5d56744332788acf318fc65000000000000000000000000000000")
        .unwrap();

    // raw tx bytes of https://doc.komodo.earth/tx/2450b60c8ab0d2498d9cee3cfb67ecbe08e335ec8fa20c7f95c474734d5e007a
    let spend_tx_bytes = hex::decode("0400008085202f8901a01500f0bd916f0646103a622e933ca67ada49a6b3c5e452ed5043db7d6f212f00000000d7473044022042dbb34a97d9cdcea2c0db9871f3d1bbeb35ed74d373095eb76286573b121579022010bdee78e995f5b18f50a3ae82fac57db7147d90bedd9b96c42e54a1f65546540120a1c44607b870cd714a75d5243347fa36debcd3a91ff1f50b79f52d83238a0b2d004c6b6304df55fc65b1752102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ac6782012088a9147c0c02ee06e3769376bb2b31e05a9e9965045ffb882102d74dc5ec4c823f40dae5c563d7b22aab52c80f9f18226f47ea6d83107618df62ac68ffffffff01389d0700000000001976a914f26650dc9aa4e4505978ad635cdb15491cee70e188acdf55fc65000000000000000000000000000000")
        .unwrap();
    let spend_tx = TransactionEnum::UtxoTx(deserialize(spend_tx_bytes.as_slice()).unwrap());

    let search_input = SearchForSwapTxSpendInput {
        time_lock: 1711035871,
        other_pub: &hex::decode("02d74dc5ec4c823f40dae5c563d7b22aab52c80f9f18226f47ea6d83107618df62").unwrap(),
        secret_hash: &*dhash160(&secret),
        tx: &payment_tx_bytes,
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let found = block_on(coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();
    assert_eq!(FoundSwapTxSpend::Spent(spend_tx), found);
}

#[test]
fn test_search_for_swap_tx_spend_electrum_was_refunded() {
    let secret_hash = hex::decode("7a752434d4564c11b9333743122dab3a0aa21bd9").unwrap();
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(
        client.into(),
        Some("spice describe gravity federal blast come thank unfair canal monkey style afraid"),
        false,
    );

    // raw tx bytes of https://doc.komodo.earth/tx/ba08822b2da3f7120f5ad90cd999d5f0f4be4a63de496f4f76af202c68e4f5eb
    let payment_tx_bytes = hex::decode("0400008085202f8901b2781d994b79be8e1f687a7f376109063bbe8b51ab36d04b35d4ff437b21d2a5010000006a47304402201832294ceb2b62a197bc2049218dcee69dcabb414249403efcc35ad65c17e73d0220348fbbb2b40880408bdef604e5a6cba34f514b84d5d6a9a4c4713669cd765fd2012102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ffffffff0320a107000000000017a91489dd2a32ae17a0575759581afb0002176296777b870000000000000000166a147a752434d4564c11b9333743122dab3a0aa21bd94af2de7a000000001976a91405aab5342166f8594baf17a7d9bef5d56744332788ac2b0efe65000000000000000000000000000000")
        .unwrap();

    // raw tx bytes of https://doc.komodo.earth/tx/1b4e30f0e3101374464ec159c2f35b03412034fadee8c5769e8adcd5c91359bd
    let refund_tx_bytes = hex::decode("0400008085202f8901ebf5e4682c20af764f6f49de634abef4f0d599d90cd95a0f12f7a32d2b8208ba00000000b647304402205bd140728b1b6de7b891025873d552a439c488dd7fe4234fd982eaa193e5776602205ad9fea8bc771d94de9d2fdc450186ea21f4fc81f28c2f2f75b5d2ad84891d8f01514c6b63049f0efe65b1752102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ac6782012088a9147a752434d4564c11b9333743122dab3a0aa21bd9882102d74dc5ec4c823f40dae5c563d7b22aab52c80f9f18226f47ea6d83107618df62ac68feffffff01389d0700000000001976a91405aab5342166f8594baf17a7d9bef5d56744332788acb80efe65000000000000000000000000000000")
        .unwrap();
    let refund_tx = TransactionEnum::UtxoTx(deserialize(refund_tx_bytes.as_slice()).unwrap());

    let search_input = SearchForSwapTxSpendInput {
        time_lock: 1711148703,
        other_pub: &hex::decode("02d74dc5ec4c823f40dae5c563d7b22aab52c80f9f18226f47ea6d83107618df62").unwrap(),
        secret_hash: &secret_hash,
        tx: &payment_tx_bytes,
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let found = block_on(coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();
    assert_eq!(FoundSwapTxSpend::Refunded(refund_tx), found);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_impl_set_fixed_fee() {
    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(|coin, _| {
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

    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    let withdraw_req = WithdrawRequest {
        amount: 1u64.into(),
        to: "RQq6fWoy8aGGMLjvRfMY5mBNVm2RQxJyLa".to_string(),
        coin: TEST_COIN_NAME.into(),
        fee: Some(WithdrawFee::UtxoFixed {
            amount: "0.1".parse().unwrap(),
        }),
        ..Default::default()
    };
    let expected = Some(
        UtxoFeeDetails {
            coin: Some(TEST_COIN_NAME.into()),
            amount: "0.0245".parse().unwrap(),
        }
        .into(),
    );
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();
    assert_eq!(expected, tx_details.fee_details);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_impl_sat_per_kb_fee() {
    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(|coin, _| {
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

    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    let withdraw_req = WithdrawRequest {
        amount: 1u64.into(),
        to: "RQq6fWoy8aGGMLjvRfMY5mBNVm2RQxJyLa".to_string(),
        coin: TEST_COIN_NAME.into(),
        fee: Some(WithdrawFee::UtxoPerKbyte {
            amount: "0.1".parse().unwrap(),
        }),
        ..Default::default()
    };
    // The resulting transaction size might be 244 or 245 bytes depending on signature size
    // MM2 always expects the worst case during fee calculation
    // 0.1 * 245 / 1000 ~ 0.0245
    let expected = Some(
        UtxoFeeDetails {
            coin: Some(TEST_COIN_NAME.into()),
            amount: "0.0245".parse().unwrap(),
        }
        .into(),
    );
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();
    assert_eq!(expected, tx_details.fee_details);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_impl_sat_per_kb_fee_amount_equal_to_max() {
    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(|coin, _| {
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

    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    let withdraw_req = WithdrawRequest {
        amount: "9.9789".parse().unwrap(),
        to: "RQq6fWoy8aGGMLjvRfMY5mBNVm2RQxJyLa".to_string(),
        coin: TEST_COIN_NAME.into(),
        fee: Some(WithdrawFee::UtxoPerKbyte {
            amount: "0.1".parse().unwrap(),
        }),
        ..Default::default()
    };
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();
    // The resulting transaction size might be 210 or 211 bytes (no change output) depending on signature size
    // MM2 always expects the worst case during fee calculation:
    // tx_fee = 0.1 * 211 / 1000 = 0.0211
    let expected_fee = Some(
        UtxoFeeDetails {
            coin: Some(TEST_COIN_NAME.into()),
            amount: "0.0211".parse().unwrap(),
        }
        .into(),
    );
    assert_eq!(expected_fee, tx_details.fee_details);
    let expected_balance_change = BigDecimal::from(-10i32);
    assert_eq!(expected_balance_change, tx_details.my_balance_change);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_impl_sat_per_kb_fee_amount_equal_to_max_dust_included_to_fee() {
    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(|coin, _| {
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

    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    let withdraw_req = WithdrawRequest {
        amount: "9.9789".parse().unwrap(),
        to: "RQq6fWoy8aGGMLjvRfMY5mBNVm2RQxJyLa".to_string(),
        coin: TEST_COIN_NAME.into(),
        fee: Some(WithdrawFee::UtxoPerKbyte {
            amount: "0.09999999".parse().unwrap(),
        }),
        ..Default::default()
    };
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();
    // The resulting transaction size might be 210 or 211 bytes depending on signature size
    // MM2 always expects the worst case during fee calculation
    // 0.1 * 211 / 1000 = 0.0211
    let expected_fee = Some(
        UtxoFeeDetails {
            coin: Some(TEST_COIN_NAME.into()),
            amount: "0.0211".parse().unwrap(),
        }
        .into(),
    );
    assert_eq!(expected_fee, tx_details.fee_details);
    let expected_balance_change = BigDecimal::from(-10i32);
    assert_eq!(expected_balance_change, tx_details.my_balance_change);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_impl_sat_per_kb_fee_amount_over_max() {
    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(|coin, _| {
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

    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    let withdraw_req = WithdrawRequest {
        amount: "9.97939455".parse().unwrap(),
        to: "RQq6fWoy8aGGMLjvRfMY5mBNVm2RQxJyLa".to_string(),
        coin: TEST_COIN_NAME.into(),
        fee: Some(WithdrawFee::UtxoPerKbyte {
            amount: "0.1".parse().unwrap(),
        }),
        ..Default::default()
    };
    block_on_f01(coin.withdraw(withdraw_req)).unwrap_err();
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_impl_sat_per_kb_fee_max() {
    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(|coin, _| {
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

    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    let withdraw_req = WithdrawRequest {
        amount: 0u64.into(),
        to: "RQq6fWoy8aGGMLjvRfMY5mBNVm2RQxJyLa".to_string(),
        coin: TEST_COIN_NAME.into(),
        max: true,
        fee: Some(WithdrawFee::UtxoPerKbyte {
            amount: "0.1".parse().unwrap(),
        }),
        ..Default::default()
    };
    // The resulting transaction size might be 210 or 211 bytes depending on signature size
    // MM2 always expects the worst case during fee calculation
    // 0.1 * 211 / 1000 = 0.0211
    let expected = Some(
        UtxoFeeDetails {
            coin: Some(TEST_COIN_NAME.into()),
            amount: "0.0211".parse().unwrap(),
        }
        .into(),
    );
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();
    assert_eq!(expected, tx_details.fee_details);
}

#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_kmd_rewards_impl(
    tx_hash: &'static str,
    tx_hex: &'static str,
    verbose_serialized: &str,
    current_mtp: u32,
    expected_rewards: Option<BigDecimal>,
) {
    let verbose: RpcTransaction = json::from_str(verbose_serialized).unwrap();
    let unspent_height = verbose.height;
    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(move |coin: &UtxoStandardCoin, _| {
        let fut = async move {
            let tx: UtxoTx = tx_hex.into();
            let unspents = vec![UnspentInfo {
                outpoint: OutPoint {
                    hash: tx.hash(),
                    index: 0,
                },
                value: tx.outputs[0].value,
                height: unspent_height,
                script: coin
                    .script_for_address(&coin.as_ref().derivation_method.unwrap_single_addr().await)
                    .unwrap(),
            }];
            let cache = coin.as_ref().recently_spent_outpoints.lock().await;
            Ok((unspents, cache))
        };
        MockResult::Return(fut.boxed())
    });
    UtxoStandardCoin::get_current_mtp
        .mock_safe(move |_fields| MockResult::Return(Box::pin(futures::future::ok(current_mtp))));
    NativeClient::get_verbose_transactions.mock_safe(move |_coin, txids| {
        let expected = <[u8; 32]>::from_hex(tx_hash).unwrap().into();
        assert_eq!(txids, &[expected]);
        MockResult::Return(Box::new(futures01::future::ok([verbose.clone()].into())))
    });

    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let mut fields = utxo_coin_fields_for_test(UtxoRpcClientEnum::Native(client), None, false);
    fields.conf.ticker = "KMD".to_owned();
    let coin = utxo_coin_from_fields(fields);

    let withdraw_req = WithdrawRequest {
        amount: BigDecimal::from_str("0.00001").unwrap(),
        to: "RQq6fWoy8aGGMLjvRfMY5mBNVm2RQxJyLa".to_string(),
        coin: "KMD".to_owned(),
        ..Default::default()
    };
    let expected_fee = TxFeeDetails::Utxo(UtxoFeeDetails {
        coin: Some("KMD".into()),
        amount: "0.00000245".parse().unwrap(),
    });
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();
    assert_eq!(tx_details.fee_details, Some(expected_fee));

    let expected_rewards = expected_rewards.map(|amount| KmdRewardsDetails {
        amount,
        claimed_by_me: true,
    });
    assert_eq!(tx_details.kmd_rewards, expected_rewards);
}

/// https://kmdexplorer.io/tx/535ffa3387d3fca14f4a4d373daf7edf00e463982755afce89bc8c48d8168024
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_kmd_rewards() {
    const TX_HASH: &str = "535ffa3387d3fca14f4a4d373daf7edf00e463982755afce89bc8c48d8168024";
    const TX_HEX: &str = "0400008085202f8901afcadb73880bc1c9e7ce96b8274c2e2a4547415e649f425f98791685be009b73020000006b483045022100b8fbb77efea482b656ad16fc53c5a01d289054c2e429bf1d7bab16c3e822a83602200b87368a95c046b2ce6d0d092185138a3f234a7eb0d7f8227b196ef32358b93f012103b1e544ce2d860219bc91314b5483421a553a7b33044659eff0be9214ed58adddffffffff01dd15c293000000001976a91483762a373935ca241d557dfce89171d582b486de88ac99fe9960000000000000000000000000000000";
    const VERBOSE_SERIALIZED: &str = r#"{"hex":"0400008085202f8901afcadb73880bc1c9e7ce96b8274c2e2a4547415e649f425f98791685be009b73020000006b483045022100b8fbb77efea482b656ad16fc53c5a01d289054c2e429bf1d7bab16c3e822a83602200b87368a95c046b2ce6d0d092185138a3f234a7eb0d7f8227b196ef32358b93f012103b1e544ce2d860219bc91314b5483421a553a7b33044659eff0be9214ed58adddffffffff01dd15c293000000001976a91483762a373935ca241d557dfce89171d582b486de88ac99fe9960000000000000000000000000000000","txid":"535ffa3387d3fca14f4a4d373daf7edf00e463982755afce89bc8c48d8168024","hash":null,"size":null,"vsize":null,"version":4,"locktime":1620704921,"vin":[{"txid":"739b00be851679985f429f645e4147452a2e4c27b896cee7c9c10b8873dbcaaf","vout":2,"scriptSig":{"asm":"3045022100b8fbb77efea482b656ad16fc53c5a01d289054c2e429bf1d7bab16c3e822a83602200b87368a95c046b2ce6d0d092185138a3f234a7eb0d7f8227b196ef32358b93f[ALL] 03b1e544ce2d860219bc91314b5483421a553a7b33044659eff0be9214ed58addd","hex":"483045022100b8fbb77efea482b656ad16fc53c5a01d289054c2e429bf1d7bab16c3e822a83602200b87368a95c046b2ce6d0d092185138a3f234a7eb0d7f8227b196ef32358b93f012103b1e544ce2d860219bc91314b5483421a553a7b33044659eff0be9214ed58addd"},"sequence":4294967295,"txinwitness":null}],"vout":[{"value":24.78970333,"n":0,"scriptPubKey":{"asm":"OP_DUP OP_HASH160 83762a373935ca241d557dfce89171d582b486de OP_EQUALVERIFY OP_CHECKSIG","hex":"76a91483762a373935ca241d557dfce89171d582b486de88ac","reqSigs":1,"type":"pubkeyhash","addresses":["RMGJ9tRST45RnwEKHPGgBLuY3moSYP7Mhk"]}}],"blockhash":"0b438a8e50afddb38fb1c7be4536ffc7f7723b76bbc5edf7c28f2c17924dbdfa","confirmations":33186,"rawconfirmations":33186,"time":1620705483,"blocktime":1620705483,"height":2387532}"#;
    const CURRENT_MTP: u32 = 1622724281;

    let expected_rewards = BigDecimal::from_str("0.07895295").unwrap();
    test_withdraw_kmd_rewards_impl(TX_HASH, TX_HEX, VERBOSE_SERIALIZED, CURRENT_MTP, Some(expected_rewards));
}

/// If the ticker is `KMD` AND no rewards were accrued due to a value less than 10 or for any other reasons,
/// then `TransactionDetails::kmd_rewards` has to be `Some(0)`, not `None`.
/// https://kmdexplorer.io/tx/8c43e5a0402648faa5d0ae3550137544507ab1553425fa1b6f481a66a53f7a2d
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_kmd_rewards_zero() {
    const TX_HASH: &str = "8c43e5a0402648faa5d0ae3550137544507ab1553425fa1b6f481a66a53f7a2d";
    const TX_HEX: &str = "0400008085202f8901c3651b6fb9ddf372e7a9d4d829c27eeea6cdfaab4f2e6e3527905c2a14f3702b010000006a47304402206819b3e51f076841ed5946bc9a48b9d75024b60abd8e854bfe50cbdfae8a268e022001a3648d2a4b33a761090676e4a8c676ee67cb602f29fef74ea5bbb8b516a178012103832b54342019dd5ecc08f1143757fbcf4ac6c8696653d456a84b40f34653c9a8ffffffff0200e1f505000000001976a91483762a373935ca241d557dfce89171d582b486de88ac60040c35000000001976a9142b33504039790fde428e4ab084aa1baf6aee209288acb0edd45f000000000000000000000000000000";
    const VERBOSE_SERIALIZED: &str = r#"{"hex":"0400008085202f8901c3651b6fb9ddf372e7a9d4d829c27eeea6cdfaab4f2e6e3527905c2a14f3702b010000006a47304402206819b3e51f076841ed5946bc9a48b9d75024b60abd8e854bfe50cbdfae8a268e022001a3648d2a4b33a761090676e4a8c676ee67cb602f29fef74ea5bbb8b516a178012103832b54342019dd5ecc08f1143757fbcf4ac6c8696653d456a84b40f34653c9a8ffffffff0200e1f505000000001976a91483762a373935ca241d557dfce89171d582b486de88ac60040c35000000001976a9142b33504039790fde428e4ab084aa1baf6aee209288acb0edd45f000000000000000000000000000000","txid":"8c43e5a0402648faa5d0ae3550137544507ab1553425fa1b6f481a66a53f7a2d","hash":null,"size":null,"vsize":null,"version":4,"locktime":1607790000,"vin":[{"txid":"2b70f3142a5c9027356e2e4fabfacda6ee7ec229d8d4a9e772f3ddb96f1b65c3","vout":1,"scriptSig":{"asm":"304402206819b3e51f076841ed5946bc9a48b9d75024b60abd8e854bfe50cbdfae8a268e022001a3648d2a4b33a761090676e4a8c676ee67cb602f29fef74ea5bbb8b516a178[ALL] 03832b54342019dd5ecc08f1143757fbcf4ac6c8696653d456a84b40f34653c9a8","hex":"47304402206819b3e51f076841ed5946bc9a48b9d75024b60abd8e854bfe50cbdfae8a268e022001a3648d2a4b33a761090676e4a8c676ee67cb602f29fef74ea5bbb8b516a178012103832b54342019dd5ecc08f1143757fbcf4ac6c8696653d456a84b40f34653c9a8"},"sequence":4294967295,"txinwitness":null}],"vout":[{"value":1.0,"n":0,"scriptPubKey":{"asm":"OP_DUP OP_HASH160 83762a373935ca241d557dfce89171d582b486de OP_EQUALVERIFY OP_CHECKSIG","hex":"76a91483762a373935ca241d557dfce89171d582b486de88ac","reqSigs":1,"type":"pubkeyhash","addresses":["RMGJ9tRST45RnwEKHPGgBLuY3moSYP7Mhk"]}},{"value":8.8998,"n":1,"scriptPubKey":{"asm":"OP_DUP OP_HASH160 2b33504039790fde428e4ab084aa1baf6aee2092 OP_EQUALVERIFY OP_CHECKSIG","hex":"76a9142b33504039790fde428e4ab084aa1baf6aee209288ac","reqSigs":1,"type":"pubkeyhash","addresses":["RDDcc63q27t6k95LrysuDwtwrxuAXqNiXe"]}}],"blockhash":"0000000054ed9fc7a4316430659e127eac5776ebc2d2382db0cb9be3eb970d7b","confirmations":243859,"rawconfirmations":243859,"time":1607790977,"blocktime":1607790977,"height":2177114}"#;
    const CURRENT_MTP: u32 = 1622724281;

    let expected_rewards = BigDecimal::from(0);
    test_withdraw_kmd_rewards_impl(TX_HASH, TX_HEX, VERBOSE_SERIALIZED, CURRENT_MTP, Some(expected_rewards));
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_rick_rewards_none() {
    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    // https://rick.explorer.dexstats.info/tx/7181400be323acc6b5f3164240e6c4601ff4c252f40ce7649f87e81634330209
    const TX_HEX: &str = "0400008085202f8901df8119c507aa61d32332cd246dbfeb3818a4f96e76492454c1fbba5aa097977e000000004847304402205a7e229ea6929c97fd6dde254c19e4eb890a90353249721701ae7a1c477d99c402206a8b7c5bf42b5095585731d6b4c589ce557f63c20aed69ff242eca22ecfcdc7a01feffffff02d04d1bffbc050000232102afdbba3e3c90db5f0f4064118f79cf308f926c68afd64ea7afc930975663e4c4ac402dd913000000001976a9143e17014eca06281ee600adffa34b4afb0922a22288ac2bdab86035a00e000000000000000000000000";

    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(move |coin, _| {
        let fut = async move {
            let tx: UtxoTx = TX_HEX.into();
            let unspents = vec![UnspentInfo {
                outpoint: OutPoint {
                    hash: tx.hash(),
                    index: 0,
                },
                value: tx.outputs[0].value,
                height: Some(1431628),
                script: coin
                    .script_for_address(&coin.as_ref().derivation_method.unwrap_single_addr().await)
                    .unwrap(),
            }];
            let cache = coin.as_ref().recently_spent_outpoints.lock().await;
            Ok((unspents, cache))
        };
        MockResult::Return(fut.boxed())
    });

    let withdraw_req = WithdrawRequest {
        amount: BigDecimal::from_str("0.00001").unwrap(),
        to: "RQq6fWoy8aGGMLjvRfMY5mBNVm2RQxJyLa".to_string(),
        coin: "RICK".to_owned(),
        ..Default::default()
    };
    let expected_fee = TxFeeDetails::Utxo(UtxoFeeDetails {
        coin: Some(TEST_COIN_NAME.into()),
        amount: "0.00000245".parse().unwrap(),
    });
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();
    assert_eq!(tx_details.fee_details, Some(expected_fee));
    assert_eq!(tx_details.kmd_rewards, None);
}

#[test]
fn test_utxo_lock() {
    // send several transactions concurrently to check that they are not using same inputs
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(client.into(), None, false);
    let output = TransactionOutput {
        value: 1000000,
        script_pubkey: Builder::build_p2pkh(block_on(coin.as_ref().derivation_method.unwrap_single_addr()).hash())
            .to_bytes(),
    };
    let mut futures = vec![];
    for _ in 0..5 {
        futures.push(send_outputs_from_my_address_impl(coin.clone(), vec![output.clone()]));
    }
    let results = block_on(join_all(futures));
    for result in results {
        result.unwrap();
    }
}

#[test]
fn test_spv_proof() {
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);

    // https://doc.explorer.dexstats.info/tx/a3ebedbe20f82e43708f276152cf7dfb03a6050921c8f266e48c00ab66e891fb
    let tx_str = "0400008085202f8901e15182af2c252bcfbd58884f3bdbd4d85ed036e53cfe2fd1f904ecfea10cb9f2010000006b483045022100d2435e0c9211114271ac452dc47fd08d3d2dc4bdd484d5750ee6bbda41056d520220408bfb236b7028b6fde0e59a1b6522949131a611584cce36c3df1e934c1748630121022d7424c741213a2b9b49aebdaa10e84419e642a8db0a09e359a3d4c850834846ffffffff02a09ba104000000001976a914054407d1a2224268037cfc7ca3bc438d082bedf488acdd28ce9157ba11001976a914046922483fab8ca76b23e55e9d338605e2dbab6088ac03d63665000000000000000000000000000000";
    let tx: UtxoTx = tx_str.into();

    let header: BlockHeader = deserialize(
        block_on(client.blockchain_block_header(263240).compat())
            .unwrap()
            .as_slice(),
    )
    .unwrap();
    let mut headers = HashMap::new();
    headers.insert(263240, header);
    let storage = client.block_headers_storage();
    block_on(storage.add_block_headers_to_storage(headers)).unwrap();

    let res = block_on(client.validate_spv_proof(&tx, wait_until_sec(30)));
    res.unwrap();
}

#[test]
fn list_since_block_btc_serde() {
    // https://github.com/KomodoPlatform/atomicDEX-API/issues/563
    let input = r#"{"lastblock":"000000000000000000066f896cca2a6c667ca85fff28ed6731d64e3c39ecb119","removed":[],"transactions":[{"abandoned":false,"address":"1Q3kQ1jsB2VyH83PJT1NXJqEaEcR6Yuknn","amount":-0.01788867,"bip125-replaceable":"no","blockhash":"0000000000000000000db4be4c2df08790e1027326832cc90889554bbebc69b7","blockindex":437,"blocktime":1572174214,"category":"send","confirmations":197,"fee":-0.00012924,"involvesWatchonly":true,"time":1572173721,"timereceived":1572173721,"txid":"29606e6780c69a39767b56dc758e6af31ced5232491ad62dcf25275684cb7701","vout":0,"walletconflicts":[]},{"address":"1JsAjr6d21j9T8EMsYnQ6GXf1mM523JAv1","amount":0.1995,"bip125-replaceable":"no","blockhash":"0000000000000000000e75b33bbb27e6af2fc3898108c93c03c293fd72a86c6f","blockindex":157,"blocktime":1572179171,"category":"receive","confirmations":190,"label":"","time":1572178251,"timereceived":1572178251,"txid":"da651c6addc8da7c4b2bec21d43022852a93a9f2882a827704b318eb2966b82e","vout":19,"walletconflicts":[]},{"abandoned":false,"address":"14RXkMTyH4NyK48DbhTQyMBoMb2UkbBEPr","amount":-0.0208,"bip125-replaceable":"no","blockhash":"0000000000000000000611bfe0b3f7612239264459f4f6e7169f8d1a67e1b08f","blockindex":286,"blocktime":1572189657,"category":"send","confirmations":178,"fee":-0.0002,"involvesWatchonly":true,"time":1572189100,"timereceived":1572189100,"txid":"8d10920ce70aeb6c7e61c8d47f3cd903fb69946edd08d8907472a90761965943","vout":0,"walletconflicts":[]},{"abandoned":false,"address":"361JVximBAqkLZERT7XB1rykgLePEHAP7B","amount":-0.01801791,"bip125-replaceable":"no","blockhash":"00000000000000000011e9293c1f07f9711e677389ac101b93116d239ac38c33","blockindex":274,"blocktime":1572173649,"category":"send","confirmations":198,"fee":-0.0000965,"involvesWatchonly":true,"label":"361JVximBAqkLZERT7XB1rykgLePEHAP7B","time":1572173458,"timereceived":1572173458,"txid":"7983cae1afeb7fe58e020878aaedea0fee15be9319bc49c81f3b9ad466782950","vout":0,"walletconflicts":[]},{"abandoned":false,"address":"1JsAjr6d21j9T8EMsYnQ6GXf1mM523JAv1","amount":-0.0003447,"bip125-replaceable":"no","blockhash":"00000000000000000011e9293c1f07f9711e677389ac101b93116d239ac38c33","blockindex":274,"blocktime":1572173649,"category":"send","confirmations":198,"fee":-0.0000965,"label":"","time":1572173458,"timereceived":1572173458,"txid":"7983cae1afeb7fe58e020878aaedea0fee15be9319bc49c81f3b9ad466782950","vout":1,"walletconflicts":[]},{"address":"361JVximBAqkLZERT7XB1rykgLePEHAP7B","amount":0.01801791,"bip125-replaceable":"no","blockhash":"00000000000000000011e9293c1f07f9711e677389ac101b93116d239ac38c33","blockindex":274,"blocktime":1572173649,"category":"receive","confirmations":198,"involvesWatchonly":true,"label":"361JVximBAqkLZERT7XB1rykgLePEHAP7B","time":1572173458,"timereceived":1572173458,"txid":"7983cae1afeb7fe58e020878aaedea0fee15be9319bc49c81f3b9ad466782950","vout":0,"walletconflicts":[]},{"address":"1JsAjr6d21j9T8EMsYnQ6GXf1mM523JAv1","amount":0.0003447,"bip125-replaceable":"no","blockhash":"00000000000000000011e9293c1f07f9711e677389ac101b93116d239ac38c33","blockindex":274,"blocktime":1572173649,"category":"receive","confirmations":198,"label":"","time":1572173458,"timereceived":1572173458,"txid":"7983cae1afeb7fe58e020878aaedea0fee15be9319bc49c81f3b9ad466782950","vout":1,"walletconflicts":[]},{"abandoned":false,"address":"3B3q1GTLQQ7Fspo6ATy3cd3tg5yu97hkve","amount":-0.021,"bip125-replaceable":"no","blockhash":"0000000000000000000debf11962f89e2ae08f8ff75803b0da6170af6c5c346b","blockindex":2618,"blocktime":1572188894,"category":"send","confirmations":179,"fee":-0.00016026,"involvesWatchonly":true,"label":"3B3q1GTLQQ7Fspo6ATy3cd3tg5yu97hkve","time":1572186009,"timereceived":1572186009,"txid":"54b159ac3a656bbaaf3bf0263b8deafad03b376ec0c2e9c715d0cf1caaf3495e","vout":0,"walletconflicts":[]},{"abandoned":false,"address":"1JsAjr6d21j9T8EMsYnQ6GXf1mM523JAv1","amount":-0.17868444,"bip125-replaceable":"no","blockhash":"0000000000000000000debf11962f89e2ae08f8ff75803b0da6170af6c5c346b","blockindex":2618,"blocktime":1572188894,"category":"send","confirmations":179,"fee":-0.00016026,"label":"","time":1572186009,"timereceived":1572186009,"txid":"54b159ac3a656bbaaf3bf0263b8deafad03b376ec0c2e9c715d0cf1caaf3495e","vout":1,"walletconflicts":[]},{"address":"3B3q1GTLQQ7Fspo6ATy3cd3tg5yu97hkve","amount":0.021,"bip125-replaceable":"no","blockhash":"0000000000000000000debf11962f89e2ae08f8ff75803b0da6170af6c5c346b","blockindex":2618,"blocktime":1572188894,"category":"receive","confirmations":179,"involvesWatchonly":true,"label":"3B3q1GTLQQ7Fspo6ATy3cd3tg5yu97hkve","time":1572186009,"timereceived":1572186009,"txid":"54b159ac3a656bbaaf3bf0263b8deafad03b376ec0c2e9c715d0cf1caaf3495e","vout":0,"walletconflicts":[]},{"address":"1JsAjr6d21j9T8EMsYnQ6GXf1mM523JAv1","amount":0.17868444,"bip125-replaceable":"no","blockhash":"0000000000000000000debf11962f89e2ae08f8ff75803b0da6170af6c5c346b","blockindex":2618,"blocktime":1572188894,"category":"receive","confirmations":179,"label":"","time":1572186009,"timereceived":1572186009,"txid":"54b159ac3a656bbaaf3bf0263b8deafad03b376ec0c2e9c715d0cf1caaf3495e","vout":1,"walletconflicts":[]},{"abandoned":false,"address":"3AC6k1Y54knEdkgWjX3TjmWGjDHtJCNZZY","amount":-0.17822795,"bip125-replaceable":"no","blockhash":"00000000000000000009a60478f29f4910e29224ea5ed63d77321ac8c624ec45","blockindex":2377,"blocktime":1572190637,"category":"send","confirmations":177,"fee":-0.00009985,"involvesWatchonly":true,"label":"3AC6k1Y54knEdkgWjX3TjmWGjDHtJCNZZY","time":1572189626,"timereceived":1572189626,"txid":"eabc01e45db89ea8cf623f8e22847e4023c69bed3c7d396d573b89dec3fe17a7","vout":0,"walletconflicts":[]},{"abandoned":false,"address":"1JsAjr6d21j9T8EMsYnQ6GXf1mM523JAv1","amount":-0.00035664,"bip125-replaceable":"no","blockhash":"00000000000000000009a60478f29f4910e29224ea5ed63d77321ac8c624ec45","blockindex":2377,"blocktime":1572190637,"category":"send","confirmations":177,"fee":-0.00009985,"label":"","time":1572189626,"timereceived":1572189626,"txid":"eabc01e45db89ea8cf623f8e22847e4023c69bed3c7d396d573b89dec3fe17a7","vout":1,"walletconflicts":[]},{"address":"3AC6k1Y54knEdkgWjX3TjmWGjDHtJCNZZY","amount":0.17822795,"bip125-replaceable":"no","blockhash":"00000000000000000009a60478f29f4910e29224ea5ed63d77321ac8c624ec45","blockindex":2377,"blocktime":1572190637,"category":"receive","confirmations":177,"involvesWatchonly":true,"label":"3AC6k1Y54knEdkgWjX3TjmWGjDHtJCNZZY","time":1572189626,"timereceived":1572189626,"txid":"eabc01e45db89ea8cf623f8e22847e4023c69bed3c7d396d573b89dec3fe17a7","vout":0,"walletconflicts":[]},{"address":"1JsAjr6d21j9T8EMsYnQ6GXf1mM523JAv1","amount":0.00035664,"bip125-replaceable":"no","blockhash":"00000000000000000009a60478f29f4910e29224ea5ed63d77321ac8c624ec45","blockindex":2377,"blocktime":1572190637,"category":"receive","confirmations":177,"label":"","time":1572189626,"timereceived":1572189626,"txid":"eabc01e45db89ea8cf623f8e22847e4023c69bed3c7d396d573b89dec3fe17a7","vout":1,"walletconflicts":[]},{"abandoned":false,"address":"1Q3kQ1jsB2VyH83PJT1NXJqEaEcR6Yuknn","amount":-0.17809412,"bip125-replaceable":"no","blockhash":"000000000000000000125e17a9540ac901d70e92e987d59a1cf87ca36ebca830","blockindex":1680,"blocktime":1572191122,"category":"send","confirmations":176,"fee":-0.00013383,"involvesWatchonly":true,"time":1572190821,"timereceived":1572190821,"txid":"d3579f7be169ea8fd1358d0eda85bad31ce8080a6020dcd224eac8a663dc9bf7","vout":0,"walletconflicts":[]},{"abandoned":false,"address":"326VCyLKV1w4SxeYs81jQU1SC11njcL1eG","amount":-0.039676,"bip125-replaceable":"no","blockhash":"0000000000000000000d61630db06ed5d3054a39bf71a706efeaa9e86866b9d4","blockindex":2193,"blocktime":1572053656,"category":"send","confirmations":380,"fee":-0.00005653,"involvesWatchonly":true,"label":"326VCyLKV1w4SxeYs81jQU1SC11njcL1eG","time":1572052431,"timereceived":1572052431,"txid":"37b57fb36312e21ec7d069a55ab9bffc6abc7fe3731ed38502c5329025a9edf9","vout":0,"walletconflicts":[]},{"abandoned":false,"address":"1JsAjr6d21j9T8EMsYnQ6GXf1mM523JAv1","amount":-0.01845911,"bip125-replaceable":"no","blockhash":"0000000000000000000d61630db06ed5d3054a39bf71a706efeaa9e86866b9d4","blockindex":2193,"blocktime":1572053656,"category":"send","confirmations":380,"fee":-0.00005653,"label":"","time":1572052431,"timereceived":1572052431,"txid":"37b57fb36312e21ec7d069a55ab9bffc6abc7fe3731ed38502c5329025a9edf9","vout":1,"walletconflicts":[]},{"address":"326VCyLKV1w4SxeYs81jQU1SC11njcL1eG","amount":0.039676,"bip125-replaceable":"no","blockhash":"0000000000000000000d61630db06ed5d3054a39bf71a706efeaa9e86866b9d4","blockindex":2193,"blocktime":1572053656,"category":"receive","confirmations":380,"involvesWatchonly":true,"label":"326VCyLKV1w4SxeYs81jQU1SC11njcL1eG","time":1572052431,"timereceived":1572052431,"txid":"37b57fb36312e21ec7d069a55ab9bffc6abc7fe3731ed38502c5329025a9edf9","vout":0,"walletconflicts":[]},{"address":"1JsAjr6d21j9T8EMsYnQ6GXf1mM523JAv1","amount":0.01845911,"bip125-replaceable":"no","blockhash":"0000000000000000000d61630db06ed5d3054a39bf71a706efeaa9e86866b9d4","blockindex":2193,"blocktime":1572053656,"category":"receive","confirmations":380,"label":"","time":1572052431,"timereceived":1572052431,"txid":"37b57fb36312e21ec7d069a55ab9bffc6abc7fe3731ed38502c5329025a9edf9","vout":1,"walletconflicts":[]}]}"#;
    let _res: ListSinceBlockRes = json::from_str(input).unwrap();
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/587
fn get_tx_details_coinbase_transaction() {
    /// Hash of coinbase transaction
    /// https://marty.explorer.dexstats.info/tx/ae3220b868c677c77f8c9bdbc49b42da512260b45af695e672b1c5090815566c
    const TX_HASH: &str = "ae3220b868c677c77f8c9bdbc49b42da512260b45af695e672b1c5090815566c";

    let client = electrum_client_for_test(MARTY_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(
        client.into(),
        Some("spice describe gravity federal blast come thank unfair canal monkey style afraid"),
        false,
    );

    let tx_details = get_tx_details_eq_for_both_versions(&coin, TX_HASH);
    assert!(tx_details.from.is_empty());
}

#[test]
fn test_electrum_rpc_client_error() {
    let client = electrum_client_for_test(&["electrum1.cipig.net:10060"]);

    let empty_hash = H256Json::default();
    let err = block_on_f01(client.get_verbose_transaction(&empty_hash)).unwrap_err();

    // use the static string instead because the actual error message cannot be obtain
    // by serde_json serialization
    let expected = r#"method: "blockchain.transaction.get", params: [String("0000000000000000000000000000000000000000000000000000000000000000"), Bool(true)] }, error: Response(electrum1.cipig.net:10060, Object {"code": Number(2), "message": String("daemon error: DaemonError({'code': -5, 'message': 'No such mempool or blockchain transaction. Use gettransaction for wallet transactions.'})")}) }"#;
    let actual = format!("{err}");

    assert!(actual.contains(expected));
}

#[test]
fn test_network_info_deserialization() {
    let network_info_kmd = r#"{
        "connections": 1,
        "localaddresses": [],
        "localservices": "0000000070000005",
        "networks": [
            {
                "limited": false,
                "name": "ipv4",
                "proxy": "",
                "proxy_randomize_credentials": false,
                "reachable": true
            },
            {
                "limited": false,
                "name": "ipv6",
                "proxy": "",
                "proxy_randomize_credentials": false,
                "reachable": true
            },
            {
                "limited": true,
                "name": "onion",
                "proxy": "",
                "proxy_randomize_credentials": false,
                "reachable": false
            }
        ],
        "protocolversion": 170007,
        "relayfee": 1e-06,
        "subversion": "/MagicBean:2.0.15-rc2/",
        "timeoffset": 0,
        "version": 2001526,
        "warnings": ""
    }"#;
    json::from_str::<NetworkInfo>(network_info_kmd).unwrap();

    let network_info_btc = r#"{
        "version": 180000,
        "subversion": "\/Satoshi:0.18.0\/",
        "protocolversion": 70015,
        "localservices": "000000000000040d",
        "localrelay": true,
        "timeoffset": 0,
        "networkactive": true,
        "connections": 124,
        "networks": [
            {
                "name": "ipv4",
                "limited": false,
                "reachable": true,
                "proxy": "",
                "proxy_randomize_credentials": false
            },
            {
                "name": "ipv6",
                "limited": false,
                "reachable": true,
                "proxy": "",
                "proxy_randomize_credentials": false
            },
            {
                "name": "onion",
                "limited": true,
                "reachable": false,
                "proxy": "",
                "proxy_randomize_credentials": false
            }
        ],
        "relayfee": 1.0e-5,
        "incrementalfee": 1.0e-5,
        "localaddresses": [
            {
                "address": "96.57.248.252",
                "port": 8333,
                "score": 618294
            }
        ],
        "warnings": ""
    }"#;
    json::from_str::<NetworkInfo>(network_info_btc).unwrap();
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/617
fn test_generate_transaction_relay_fee_is_used_when_dynamic_fee_is_lower() {
    let client = NativeClientImpl::default();

    static GET_RELAY_FEE_CALLED: AtomicBool = AtomicBool::new(false);
    NativeClient::get_relay_fee.mock_safe(|_| {
        GET_RELAY_FEE_CALLED.store(true, Ordering::Relaxed);
        MockResult::Return(Box::new(futures01::future::ok("1.0".parse().unwrap())))
    });
    let client = UtxoRpcClientEnum::Native(NativeClient(Arc::new(client)));
    let mut coin = utxo_coin_fields_for_test(client, None, false);
    coin.conf.force_min_relay_fee = true;
    let coin = utxo_coin_from_fields(coin);
    let unspents = vec![UnspentInfo {
        value: 1000000000,
        outpoint: OutPoint::default(),
        height: Default::default(),
        script: Vec::new().into(),
    }];

    let outputs = vec![TransactionOutput {
        script_pubkey: vec![].into(),
        value: 900000000,
    }];

    let builder = block_on(UtxoTxBuilder::new(&coin))
        .add_available_inputs(unspents)
        .add_outputs(outputs)
        .with_fee(ActualFeeRate::Dynamic(100));

    let generated = block_on(builder.build()).unwrap();
    assert_eq!(generated.0.outputs.len(), 2);

    // generated transaction fee must be equal to relay fee if calculated dynamic fee is lower than relay
    assert_eq!(generated.1.fee_amount, 22000000);
    assert_eq!(generated.1.received_by_me, 78000000);
    assert_eq!(generated.1.spent_by_me, 1000000000);
    assert!(GET_RELAY_FEE_CALLED.load(Ordering::Relaxed));
}

/// Test the transaction builder calculations (with random generated values)
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_generate_transaction_random_values() {
    use rand::{rngs::ThreadRng, Rng};

    let client = NativeClientImpl::default();
    let mut rng = rand::thread_rng();

    // tx_size for zcash, no shielded
    let est_tx_size = |n_inputs: usize, n_outputs: usize| {
        4 + 4
            + 1
            + (n_inputs as u64) * (1 + 1 + 72 + 1 + 33 + 32 + 4 + 4)
            + 1
            + (n_outputs as u64) * (1 + 25 + 8)
            + 4
            + 4
            + 8
            + 1
            + 1
            + 1
    };

    let make_random_vec_u64 = |rng: &mut ThreadRng, max_size: usize, max_value: u64| {
        let vsize = rng.gen_range(1, max_size);
        let mut v = vec![];
        for _i in 0..vsize {
            v.push(rng.gen_range(0, max_value))
        }
        v
    };

    NativeClient::get_relay_fee
        .mock_safe(|_| MockResult::Return(Box::new(futures01::future::ok("0.0".parse().unwrap()))));
    let client = UtxoRpcClientEnum::Native(NativeClient(Arc::new(client)));
    let mut coin = utxo_coin_fields_for_test(client, None, false);
    coin.conf.force_min_relay_fee = false;
    let coin = utxo_coin_from_fields(coin);

    for _i in 0..9999 {
        let input_vals = make_random_vec_u64(&mut rng, 100, 100_000);
        let output_vals = make_random_vec_u64(&mut rng, 100, 100_000);
        let dust = rng.gen_range(0, 1000);
        let fee_rate = rng.gen_range(0, 1000);

        let mut total_inputs = 0_u64;
        let mut unspents = vec![];
        for val in &input_vals {
            unspents.push(UnspentInfo {
                value: *val,
                outpoint: OutPoint::default(),
                height: Default::default(),
                script: Vec::new().into(),
            });
            total_inputs += *val;
        }

        let mut has_dust_output = false;
        let mut outputs = vec![];
        let mut total_outputs = 0_u64;
        for val in &output_vals {
            outputs.push(TransactionOutput {
                script_pubkey: "76a914124b0846223ef78130b8e544b9afc3b09988238688ac".into(),
                value: *val,
            });
            if *val < dust {
                has_dust_output = true;
            }
            total_outputs += *val;
        }

        let builder = block_on(UtxoTxBuilder::new(&coin))
            .add_available_inputs(unspents)
            .add_outputs(outputs.clone())
            .with_dust(dust)
            .with_fee(ActualFeeRate::Dynamic(fee_rate));

        let result = block_on(builder.build());
        if has_dust_output {
            let is_err_dust = matches!(
                result.unwrap_err().get_inner(),
                GenerateTxError::OutputValueLessThanDust { value: _, dust: _ }
            );
            assert!(is_err_dust);
            continue;
        }
        if let Err(ref err) = result {
            let tx_size_max = est_tx_size(input_vals.len(), output_vals.len() + 1);
            let tx_fee_max = fee_rate * tx_size_max / 1000;
            if matches!(
                err.get_inner(),
                GenerateTxError::NotEnoughUtxos {
                    sum_utxos: _,
                    required: _
                }
            ) {
                assert!(total_inputs < total_outputs + tx_fee_max);
                continue;
            }
            panic!("unexpected utxo builder err");
        }

        let generated = result.unwrap();

        // generated transaction has no change output but dust
        assert!(generated.0.outputs.len() >= output_vals.len() && generated.0.outputs.len() <= output_vals.len() + 1);
        let fact_inputs = generated.0.inputs.iter().fold(0u64, |acc, input| acc + input.amount);
        // total w/o change:
        let fact_outputs_no_change = generated
            .0
            .outputs
            .iter()
            .take(output_vals.len())
            .fold(0u64, |acc, output| acc + output.value);

        assert_eq!(generated.1.spent_by_me, fact_inputs);

        assert_eq!(total_outputs, fact_outputs_no_change);

        assert_eq!(
            generated.1.spent_by_me,
            generated.1.fee_amount + generated.1.received_by_me + total_outputs
        );

        let tx_size = est_tx_size(generated.0.inputs.len(), generated.0.outputs.len());
        let estimated_txfee = fee_rate * tx_size / 1000;
        const CHANGE_OUTPUT_SIZE: u64 = 1 + 25 + 8;
        let max_overpay = dust + fee_rate * CHANGE_OUTPUT_SIZE / 1000; // could be slight overpay due to dust change removed from tx
        if generated.1.fee_amount > estimated_txfee {
            println!(
                "overpay detected: generated.1.fee_amount={} estimated_txfee={}",
                generated.1.fee_amount, estimated_txfee
            );
        }
        assert!(generated.1.fee_amount >= estimated_txfee && generated.1.fee_amount <= estimated_txfee + max_overpay);

        let received_by_me = if generated.0.outputs.len() > output_vals.len() {
            generated.0.outputs.last().unwrap().value
        } else {
            0u64
        };
        assert_eq!(generated.1.received_by_me, received_by_me);
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/1037
fn test_generate_transaction_relay_fee_is_used_when_dynamic_fee_is_lower_and_deduct_from_output() {
    let client = NativeClientImpl::default();

    static GET_RELAY_FEE_CALLED: AtomicBool = AtomicBool::new(false);
    NativeClient::get_relay_fee.mock_safe(|_| {
        GET_RELAY_FEE_CALLED.store(true, Ordering::Relaxed);
        MockResult::Return(Box::new(futures01::future::ok("1.0".parse().unwrap())))
    });
    let client = UtxoRpcClientEnum::Native(NativeClient(Arc::new(client)));
    let mut coin = utxo_coin_fields_for_test(client, None, false);
    coin.conf.force_min_relay_fee = true;
    let coin = utxo_coin_from_fields(coin);
    let unspents = vec![UnspentInfo {
        value: 1000000000,
        outpoint: OutPoint::default(),
        height: Default::default(),
        script: Vec::new().into(),
    }];

    let outputs = vec![TransactionOutput {
        script_pubkey: vec![].into(),
        value: 1000000000,
    }];

    let tx_builder = block_on(UtxoTxBuilder::new(&coin))
        .add_available_inputs(unspents)
        .add_outputs(outputs)
        .with_fee_policy(FeePolicy::DeductFromOutput(0))
        .with_fee(ActualFeeRate::Dynamic(100));

    let generated = block_on(tx_builder.build()).unwrap();
    assert_eq!(generated.0.outputs.len(), 1);
    // `output (= 10.0) - tx_fee (= 186 byte * 100000000 / 1000)`
    assert_eq!(generated.0.outputs[0].value, 981400000);

    // generated transaction fee must be equal to relay fee if calculated dynamic fee is lower than relay fee
    assert_eq!(generated.1.fee_amount, 18600000);
    assert_eq!(generated.1.received_by_me, 0);
    assert_eq!(generated.1.spent_by_me, 1000000000);
    assert!(GET_RELAY_FEE_CALLED.load(Ordering::Relaxed));
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/617
fn test_generate_tx_fee_is_correct_when_dynamic_fee_is_larger_than_relay() {
    let client = NativeClientImpl::default();

    static GET_RELAY_FEE_CALLED: AtomicBool = AtomicBool::new(false);
    NativeClient::get_relay_fee.mock_safe(|_| {
        GET_RELAY_FEE_CALLED.store(true, Ordering::Relaxed);
        MockResult::Return(Box::new(futures01::future::ok("0.00001".parse().unwrap())))
    });
    let client = UtxoRpcClientEnum::Native(NativeClient(Arc::new(client)));
    let mut coin = utxo_coin_fields_for_test(client, None, false);
    coin.conf.force_min_relay_fee = true;
    let coin = utxo_coin_from_fields(coin);
    let unspents = vec![
        UnspentInfo {
            value: 1000000000,
            outpoint: OutPoint::default(),
            height: Default::default(),
            script: Vec::new().into(),
        };
        20
    ];

    let outputs = vec![TransactionOutput {
        script_pubkey: vec![].into(),
        value: 19000000000,
    }];

    let builder = block_on(UtxoTxBuilder::new(&coin))
        .add_available_inputs(unspents)
        .add_outputs(outputs)
        .with_fee(ActualFeeRate::Dynamic(1000));

    let generated = block_on(builder.build()).unwrap();

    assert_eq!(generated.0.outputs.len(), 2);
    assert_eq!(generated.0.inputs.len(), 20);

    // resulting signed transaction size would be 3032 bytes so fee is 3032 sat
    assert_eq!(generated.1.fee_amount, 3032);
    assert_eq!(generated.1.received_by_me, 999996968);
    assert_eq!(generated.1.spent_by_me, 20000000000);
    assert!(GET_RELAY_FEE_CALLED.load(Ordering::Relaxed));
}

#[test]
fn test_get_median_time_past_from_electrum_kmd() {
    let client = electrum_client_for_test(&[
        "electrum1.cipig.net:10001",
        "electrum2.cipig.net:10001",
        "electrum3.cipig.net:10001",
    ]);

    let mtp = block_on_f01(client.get_median_time_past(1773390, KMD_MTP_BLOCK_COUNT, CoinVariant::Standard)).unwrap();
    // the MTP is block time of 1773385 in this case
    assert_eq!(1583159915, mtp);
}

#[test]
fn test_get_median_time_past_from_electrum_btc() {
    let client = electrum_client_for_test(&[
        "electrum1.cipig.net:10000",
        "electrum2.cipig.net:10000",
        "electrum3.cipig.net:10000",
    ]);

    let mtp = block_on_f01(client.get_median_time_past(632858, KMD_MTP_BLOCK_COUNT, CoinVariant::Standard)).unwrap();
    assert_eq!(1591173041, mtp);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_get_median_time_past_from_native_has_median_in_get_block() {
    let client = native_client_for_test();
    NativeClientImpl::get_block_hash.mock_safe(|_, block_num| {
        assert_eq!(block_num, 632858);
        MockResult::Return(Box::new(futures01::future::ok(
            "00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695e3".into(),
        )))
    });

    NativeClientImpl::get_block.mock_safe(|_, block_hash| {
        assert_eq!(block_hash, "00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695e3".into());
        let block_data_str = r#"{"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695e3","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632858,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591174568,"mediantime":1591173041,"nonce":"1594651477","bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"}"#;
        let block_data = json::from_str(block_data_str).unwrap();
        MockResult::Return(
            Box::new(futures01::future::ok(block_data))
        )
    });

    let mtp = block_on_f01(client.get_median_time_past(632858, KMD_MTP_BLOCK_COUNT, CoinVariant::Standard)).unwrap();
    assert_eq!(1591173041, mtp);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_get_median_time_past_from_native_does_not_have_median_in_get_block() {
    use std::collections::HashMap;

    let blocks_json_str = r#"
    [
        {"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695e3","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632858,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591173090,"nonce":"1594651477","bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"},
        {"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695e4","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632857,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591173080,"nonce":"1594651477","bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"},
        {"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695e5","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632856,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591173070,"nonce":"1594651477","bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"},
        {"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695e6","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632855,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591173058,"nonce":"1594651477","bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"},
        {"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695e7","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632854,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591173050,"nonce":"1594651477","bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"},
        {"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695e8","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632853,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591173041,"nonce":"1594651477","bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"},
        {"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695e9","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632852,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591173040,"nonce":"1594651477","bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"},
        {"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695f0","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632851,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591173039,"nonce":"1594651477","bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"},
        {"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695f1","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632850,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591173038,"nonce":"1594651477","bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"},
        {"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695f2","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632849,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591173037,"nonce":"1594651477","bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"},
        {"hash":"00000000000000000002eb7892b8fdfd7b8e0f089e5cdf96436de680b7e695f3","confirmations":1,"strippedsize":833287,"size":1493229,"weight":3993090,"height":632848,"version":549453824,"versionHex":"20c00000","merkleroot":"7e20760d227465d2a84fbb2617b2962f77364daa66f06b48d1010fa27923b940","tx":[],"time":1591173030,"nonce":1594651477,"bits":"171297f6","difficulty":15138043247082.88,"chainwork":"00000000000000000000000000000000000000000fff2e35384d3c16f53adda4","nTx":1601,"previousblockhash":"00000000000000000009a54084d9f4eafa3ca07af646ff8fa9031d0ac72a92aa"}
    ]
    "#;

    let blocks: Vec<VerboseBlock> = json::from_str(blocks_json_str).unwrap();
    let mut block_hashes: HashMap<_, _> = blocks
        .iter()
        .map(|block| (block.height.unwrap() as u64, block.hash))
        .collect();
    let mut blocks: HashMap<_, _> = blocks.into_iter().map(|block| (block.hash, block)).collect();
    let client = native_client_for_test();

    NativeClientImpl::get_block_hash.mock_safe(move |_, block_num| {
        let hash = block_hashes.remove(&block_num).unwrap();
        MockResult::Return(Box::new(futures01::future::ok(hash)))
    });

    NativeClientImpl::get_block.mock_safe(move |_, block_hash| {
        let block = blocks.remove(&block_hash).unwrap();
        MockResult::Return(Box::new(futures01::future::ok(block)))
    });

    let mtp = block_on_f01(client.get_median_time_past(632858, KMD_MTP_BLOCK_COUNT, CoinVariant::Standard)).unwrap();
    assert_eq!(1591173041, mtp);
}

#[test]
fn test_cashaddresses_in_tx_details_by_hash() {
    const TX_HASH: &str = "0f2f6e0c8f440c641895023782783426c3aca1acc78d7c0db7751995e8aa5751";

    let conf = json!({
        "coin": "BCH",
        "pubtype": 0,
        "p2shtype": 5,
        "mm2": 1,
        "address_format":{"format":"cashaddress","network":"bchtest"},
    });
    let req = json!({
         "method": "electrum",
         "servers": electrum_servers_rpc(T_BCH_ELECTRUMS),
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let coin = block_on(utxo_standard_coin_with_priv_key(&ctx, "BCH", &conf, &params, priv_key)).unwrap();

    let tx_details = get_tx_details_eq_for_both_versions(&coin, TX_HASH);
    log!("{:?}", tx_details);

    assert!(tx_details
        .from
        .iter()
        .any(|addr| addr == "bchtest:qze8g4gx3z428jjcxzpycpxl7ke7d947gca2a7n2la"));
    assert!(tx_details
        .to
        .iter()
        .any(|addr| addr == "bchtest:qr39na5d25wdeecgw3euh9fkd4ygvd4pnsury96597"));
}

#[test]
fn test_address_from_str_with_cashaddress_activated() {
    let conf = json!({
        "coin": "BCH",
        "pubtype": 0,
        "p2shtype": 5,
        "mm2": 1,
        "address_format":{"format":"cashaddress","network":"bitcoincash"},
    });
    let req = json!({
         "method": "electrum",
         "servers": electrum_servers_rpc(T_BCH_ELECTRUMS),
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let coin = block_on(utxo_standard_coin_with_priv_key(&ctx, "BCH", &conf, &params, priv_key)).unwrap();

    // other error on parse
    let error = UtxoCommonOps::address_from_str(&coin, "bitcoincash:000000000000000000000000000000000000000000")
        .err()
        .unwrap();
    match error.into_inner() {
        AddrFromStrError::CannotDetermineFormat(_) => (),
        other => panic!(
            "Expected 'AddrFromStrError::CannotDetermineFormat' error, found: {}",
            other
        ),
    }
}

#[test]
fn test_address_from_str_with_legacy_address_activated() {
    let conf = json!({
        "coin": "BCH",
        "pubtype": 0,
        "p2shtype": 5,
        "mm2": 1,
    });
    let req = json!({
         "method": "electrum",
         "servers": electrum_servers_rpc(T_BCH_ELECTRUMS),
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let coin = block_on(utxo_standard_coin_with_priv_key(&ctx, "BCH", &conf, &params, priv_key)).unwrap();

    let error = UtxoCommonOps::address_from_str(&coin, "bitcoincash:qzxqqt9lh4feptf0mplnk58gnajfepzwcq9f2rxk55")
        .err()
        .unwrap();
    match error.into_inner() {
        AddrFromStrError::Unsupported(UnsupportedAddr::FormatMismatch {
            ticker,
            activated_format,
            used_format,
        }) => {
            assert_eq!(ticker, "BCH");
            assert_eq!(activated_format, "Legacy");
            assert_eq!(used_format, "CashAddress");
        },
        other => panic!("Expected 'UnsupportedAddr::FormatMismatch' error, found: {}", other),
    }

    // other error on parse
    let error = UtxoCommonOps::address_from_str(&coin, "0000000000000000000000000000000000")
        .err()
        .unwrap();
    match error.into_inner() {
        AddrFromStrError::CannotDetermineFormat(_) => (),
        other => panic!(
            "Expected 'AddrFromStrError::CannotDetermineFormat' error, found: {}",
            other
        ),
    }
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/673
fn test_network_info_negative_time_offset() {
    let info_str = r#"{"version":1140200,"subversion":"/Shibetoshi:1.14.2/","protocolversion":70015,"localservices":"0000000000000005","localrelay":true,"timeoffset":-1,"networkactive":true,"connections":12,"networks":[{"name":"ipv4","limited":false,"reachable":true,"proxy":"","proxy_randomize_credentials":false},{"name":"ipv6","limited":false,"reachable":true,"proxy":"","proxy_randomize_credentials":false},{"name":"onion","limited":false,"reachable":true,"proxy":"127.0.0.1:9050","proxy_randomize_credentials":true}],"relayfee":1.00000000,"incrementalfee":0.00001000,"localaddresses":[],"warnings":""}"#;
    let _info: NetworkInfo = json::from_str(info_str).unwrap();
}

#[test]
fn test_unavailable_electrum_proto_version() {
    ElectrumClientImpl::try_new_arc.mock_safe(
        |client_settings, block_headers_storage, streaming_manager, abortable_system, event_handlers| {
            MockResult::Return(ElectrumClientImpl::with_protocol_version(
                client_settings,
                block_headers_storage,
                streaming_manager,
                abortable_system,
                event_handlers,
                OrdRange::new(1.8, 1.9).unwrap(),
            ))
        },
    );

    let conf = json!({"coin":"RICK","asset":"RICK","rpcport":8923});
    let servers = ["electrum1.cipig.net:10020"];
    let req = json!({
         "method": "electrum",
         "servers": servers.iter().map(|server| json!({"url": server})).collect::<Vec<_>>(),
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let priv_key = Secp256k1Secret::from([1; 32]);
    let coin = block_on(utxo_standard_coin_with_priv_key(&ctx, "RICK", &conf, &params, priv_key)).unwrap();
    // Wait a little bit to make sure the servers are removed due to version mismatch.
    block_on(Timer::sleep(2.));
    if let UtxoRpcClientEnum::Electrum(ref electrum_client) = coin.as_ref().rpc_client {
        for server in servers {
            let error = block_on(electrum_client.get_block_count_from(server).compat())
                .err()
                .unwrap()
                .to_string();
            log!("{}", error);
            assert!(error.contains("Unknown server address"));
        }
    } else {
        panic!("Expected Electrum client");
    }
}

#[test]
#[ignore]
// The test provided to dimxy to recreate "stuck mempool" problem of komodod on RICK chain.
// Leaving this test here for a while because it might be still useful
fn test_spam_rick() {
    let conf = json!({"coin":"RICK","asset":"RICK","fname":"RICK (TESTCOIN)","rpcport":25435,"txversion":4,"overwintered":1,"mm2":1,"required_confirmations":1,"avg_blocktime":1,"protocol":{"type":"UTXO"}});
    let req = json!({
         "method": "enable",
         "coin": "RICK",
    });

    let key_pair = key_pair_from_seed("my_seed").unwrap();
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(utxo_standard_coin_with_priv_key(
        &ctx,
        "RICK",
        &conf,
        &params,
        key_pair.private().secret,
    ))
    .unwrap();

    let output = TransactionOutput {
        value: 1000000,
        script_pubkey: Builder::build_p2pkh(block_on(coin.as_ref().derivation_method.unwrap_single_addr()).hash())
            .to_bytes(),
    };
    let mut futures = vec![];
    for _ in 0..5 {
        futures.push(send_outputs_from_my_address_impl(coin.clone(), vec![output.clone()]));
    }
    let results = block_on(join_all(futures));
    for result in results {
        result.unwrap();
    }
}

#[test]
fn test_one_unavailable_electrum_proto_version() {
    // First mock with an unrealistically high version requirement that no server would support
    ElectrumClientImpl::try_new_arc.mock_safe(
        |client_settings, block_headers_storage, streaming_manager, abortable_system, event_handlers| {
            MockResult::Return(ElectrumClientImpl::with_protocol_version(
                client_settings,
                block_headers_storage,
                streaming_manager,
                abortable_system,
                event_handlers,
                OrdRange::new(7.4, 7.4).unwrap(),
            ))
        },
    );

    // Try to connect with the high version requirement - should fail
    let client = electrum_client_for_test(&["electrum1.cipig.net:10000"]);
    // When an electrum server doesn't support our protocol version range, it gets removed by the client,
    // wait a little bit to make sure this is the case.
    block_on(Timer::sleep(2.));
    let error = block_on_f01(client.get_block_count_from("electrum1.cipig.net:10000"))
        .unwrap_err()
        .to_string();
    log!("{}", error);
    assert!(error.contains("Unknown server address"));

    drop(client);
    log!("Run BTC coin to test the server.version loop");

    // Now reset the mock to a supported version
    ElectrumClientImpl::try_new_arc.mock_safe(
        |client_settings, block_headers_storage, streaming_manager, abortable_system, event_handlers| {
            MockResult::Return(ElectrumClientImpl::with_protocol_version(
                client_settings,
                block_headers_storage,
                streaming_manager,
                abortable_system,
                event_handlers,
                OrdRange::new(1.4, 1.4).unwrap(),
            ))
        },
    );

    let conf = json!({"coin":"BTC","asset":"BTC","rpcport":8332});
    let req = json!({
         "method": "electrum",
         "servers": [{"url":"electrum1.cipig.net:10000"}],
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let coin = block_on(utxo_standard_coin_with_priv_key(&ctx, "BTC", &conf, &params, priv_key)).unwrap();

    block_on(async { Timer::sleep(0.5).await });

    assert!(block_on_f01(coin.as_ref().rpc_client.get_block_count()).is_ok());
}

#[test]
fn test_qtum_generate_pod() {
    let priv_key = Secp256k1Secret::from([
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ]);
    let conf = json!({"coin":"tQTUM","rpcport":13889,"pubtype":120,"p2shtype":110});
    let req = json!({
        "method": "electrum",
        "servers": [{"url":"electrum1.cipig.net:10071"}, {"url":"electrum2.cipig.net:10071"}, {"url":"electrum3.cipig.net:10071"}],
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();

    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(qtum_coin_with_priv_key(&ctx, "tQTUM", &conf, &params, priv_key)).unwrap();
    let expected_res = "20086d757b34c01deacfef97a391f8ed2ca761c72a08d5000adc3d187b1007aca86a03bc5131b1f99b66873a12b51f8603213cdc1aa74c05ca5d48fe164b82152b";
    let address = Address::from_legacyaddress(
        "qcyBHeSct7Wr4mAw18iuQ1zW5mMFYmtmBE",
        &coin.as_ref().conf.address_prefixes,
    )
    .unwrap();
    let res = coin.generate_pod(address.hash().clone()).unwrap();
    assert_eq!(expected_res, res.to_string());
}

#[test]
fn test_qtum_add_delegation() {
    let keypair = key_pair_from_seed("asthma turtle lizard tone genuine tube hunt valley soap cloth urge alpha amazing frost faculty cycle mammal leaf normal bright topple avoid pulse buffalo").unwrap();
    let conf = json!({"coin":"tQTUM","rpcport":13889,"pubtype":120,"p2shtype":110, "mature_confirmations":1});
    let req = json!({
        "method": "electrum",
        "servers": [{"url":"electrum1.cipig.net:10071"}, {"url":"electrum2.cipig.net:10071"}, {"url":"electrum3.cipig.net:10071"}],
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(qtum_coin_with_priv_key(
        &ctx,
        "tQTUM",
        &conf,
        &params,
        keypair.private().secret,
    ))
    .unwrap();
    let address = Address::from_legacyaddress(
        "qcyBHeSct7Wr4mAw18iuQ1zW5mMFYmtmBE",
        &coin.as_ref().conf.address_prefixes,
    )
    .unwrap();
    let request = QtumDelegationRequest {
        validator_address: address.to_string(),
        fee: Some(10),
    };
    let res = block_on_f01(coin.add_delegation(request)).unwrap();
    // Eligible for delegation
    assert!(res.my_balance_change.is_negative());
    assert_eq!(res.total_amount, res.spent_by_me);
    assert!(res.spent_by_me > res.received_by_me);

    let request = QtumDelegationRequest {
        validator_address: "fake_address".to_string(),
        fee: Some(10),
    };
    let res = block_on_f01(coin.add_delegation(request));
    // Wrong address
    assert!(res.is_err());
}

#[test]
fn test_qtum_add_delegation_on_already_delegating() {
    let keypair = key_pair_from_seed("federal stay trigger hour exist success game vapor become comfort action phone bright ill target wild nasty crumble dune close rare fabric hen iron").unwrap();
    let conf = json!({"coin":"tQTUM","rpcport":13889,"pubtype":120,"p2shtype":110, "mature_confirmations":1});
    let req = json!({
        "method": "electrum",
        "servers": [{"url":"electrum1.cipig.net:10071"}, {"url":"electrum2.cipig.net:10071"}, {"url":"electrum3.cipig.net:10071"}],
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(qtum_coin_with_priv_key(
        &ctx,
        "tQTUM",
        &conf,
        &params,
        keypair.private().secret,
    ))
    .unwrap();
    let address = Address::from_legacyaddress(
        "qcyBHeSct7Wr4mAw18iuQ1zW5mMFYmtmBE",
        &coin.as_ref().conf.address_prefixes,
    )
    .unwrap();
    let request = QtumDelegationRequest {
        validator_address: address.to_string(),
        fee: Some(10),
    };
    let res = block_on_f01(coin.add_delegation(request));
    // Already Delegating
    assert!(res.is_err());
}

#[test]
fn test_qtum_get_delegation_infos() {
    let keypair =
        key_pair_from_seed("federal stay trigger hour exist success game vapor become comfort action phone bright ill target wild nasty crumble dune close rare fabric hen iron").unwrap();
    let conf = json!({"coin":"tQTUM","rpcport":13889,"pubtype":120,"p2shtype":110, "mature_confirmations":1});
    let req = json!({
        "method": "electrum",
        "servers": [{"url":"electrum1.cipig.net:10071"}, {"url":"electrum2.cipig.net:10071"}, {"url":"electrum3.cipig.net:10071"}],
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();

    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(qtum_coin_with_priv_key(
        &ctx,
        "tQTUM",
        &conf,
        &params,
        keypair.private().secret,
    ))
    .unwrap();
    let staking_infos = block_on_f01(coin.get_delegation_infos()).unwrap();
    match staking_infos.staking_infos_details {
        StakingInfosDetails::Qtum(staking_details) => {
            assert!(staking_details.am_i_staking);
            assert_eq!(staking_details.staker.unwrap(), "qcyBHeSct7Wr4mAw18iuQ1zW5mMFYmtmBE");
            // Will return false for segwit.
            assert!(staking_details.is_staking_supported);
        },
    };
}

#[test]
fn test_qtum_remove_delegation() {
    let keypair = key_pair_from_seed("federal stay trigger hour exist success game vapor become comfort action phone bright ill target wild nasty crumble dune close rare fabric hen iron").unwrap();
    let conf = json!({"coin":"tQTUM","rpcport":13889,"pubtype":120,"p2shtype":110, "mature_confirmations":1});
    let req = json!({
        "method": "electrum",
        "servers": [{"url":"electrum1.cipig.net:10071"}, {"url":"electrum2.cipig.net:10071"}, {"url":"electrum3.cipig.net:10071"}],
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(qtum_coin_with_priv_key(
        &ctx,
        "tQTUM",
        &conf,
        &params,
        keypair.private().secret,
    ))
    .unwrap();
    let res = block_on_f01(coin.remove_delegation());
    assert!(res.is_ok());
}

#[test]
fn test_qtum_my_balance() {
    QtumCoin::get_mature_unspent_ordered_list.mock_safe(move |coin, _address| {
        let fut = async move {
            let cache = coin.as_ref().recently_spent_outpoints.lock().await;
            // spendable balance (66.0)
            let mature = vec![
                UnspentInfo {
                    outpoint: OutPoint {
                        hash: 1.into(),
                        index: 0,
                    },
                    value: 5000000000,
                    height: Default::default(),
                    script: Vec::new().into(),
                },
                UnspentInfo {
                    outpoint: OutPoint {
                        hash: 1.into(),
                        index: 0,
                    },
                    value: 1600000000,
                    height: Default::default(),
                    script: Vec::new().into(),
                },
            ];
            // unspendable (2.0)
            let immature = vec![UnspentInfo {
                outpoint: OutPoint {
                    hash: 1.into(),
                    index: 0,
                },
                value: 200000000,
                height: Default::default(),
                script: Vec::new().into(),
            }];
            Ok((MatureUnspentList { mature, immature }, cache))
        };
        MockResult::Return(fut.boxed())
    });

    let conf = json!({"coin":"tQTUM","rpcport":13889,"pubtype":120,"p2shtype":110});
    let req = json!({
        "method": "electrum",
        "servers": [{"url":"electrum1.cipig.net:10071"}, {"url":"electrum2.cipig.net:10071"}, {"url":"electrum3.cipig.net:10071"}],
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();

    let priv_key = Secp256k1Secret::from([
        184, 199, 116, 240, 113, 222, 8, 199, 253, 143, 98, 185, 127, 26, 87, 38, 246, 206, 159, 27, 207, 20, 27, 112,
        184, 102, 137, 37, 78, 214, 113, 78,
    ]);

    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(qtum_coin_with_priv_key(&ctx, "tQTUM", &conf, &params, priv_key)).unwrap();

    let CoinBalance { spendable, unspendable } = block_on_f01(coin.my_balance()).unwrap();
    let expected_spendable = BigDecimal::from(66);
    let expected_unspendable = BigDecimal::from(2);
    assert_eq!(spendable, expected_spendable);
    assert_eq!(unspendable, expected_unspendable);
}

#[test]
fn test_qtum_my_balance_with_check_utxo_maturity_false() {
    const DISPLAY_BALANCE: u64 = 68;
    ElectrumClient::display_balance.mock_safe(move |_, _, _| {
        MockResult::Return(Box::new(futures01::future::ok(BigDecimal::from(DISPLAY_BALANCE))))
    });
    QtumCoin::get_all_unspent_ordered_list.mock_safe(move |_, _| {
        panic!(
            "'QtumCoin::get_all_unspent_ordered_list' is not expected to be called when `check_utxo_maturity` is false"
        )
    });

    let conf = json!({"coin":"tQTUM","rpcport":13889,"pubtype":120,"p2shtype":110});
    let req = json!({
        "method": "electrum",
        "servers": [{"url":"electrum1.cipig.net:10071"}, {"url":"electrum2.cipig.net:10071"}, {"url":"electrum3.cipig.net:10071"}],
        "check_utxo_maturity": false,
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();

    let priv_key = Secp256k1Secret::from([
        184, 199, 116, 240, 113, 222, 8, 199, 253, 143, 98, 185, 127, 26, 87, 38, 246, 206, 159, 27, 207, 20, 27, 112,
        184, 102, 137, 37, 78, 214, 113, 78,
    ]);

    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(qtum_coin_with_priv_key(&ctx, "tQTUM", &conf, &params, priv_key)).unwrap();

    let CoinBalance { spendable, unspendable } = block_on_f01(coin.my_balance()).unwrap();
    let expected_spendable = BigDecimal::from(DISPLAY_BALANCE);
    let expected_unspendable = BigDecimal::from(0);
    assert_eq!(spendable, expected_spendable);
    assert_eq!(unspendable, expected_unspendable);
}

fn test_get_mature_unspent_ordered_map_from_cache_impl(
    unspent_height: Option<u64>,
    cached_height: Option<u64>,
    cached_confs: u32,
    block_count: u64,
    expected_height: Option<u64>,
    expected_confs: u32,
) {
    const TX_HASH: &str = "b43f9ed47f7b97d4766b6f1614136fa0c55b9a52c97342428333521fa13ad714";
    let tx_hash: H256Json = <[u8; 32]>::from_hex(TX_HASH).unwrap().into();
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let mut verbose = block_on_f01(client.get_verbose_transaction(&tx_hash)).unwrap();
    verbose.confirmations = cached_confs;
    verbose.height = cached_height;

    // prepare mocks
    ElectrumClient::list_unspent.mock_safe(move |_, _, _| {
        let unspents = vec![UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_reversed_str(TX_HASH),
                index: 0,
            },
            value: 1000000000,
            height: unspent_height,
            script: Vec::new().into(),
        }];
        MockResult::Return(Box::new(futures01::future::ok(unspents)))
    });
    ElectrumClient::get_block_count
        .mock_safe(move |_| MockResult::Return(Box::new(futures01::future::ok(block_count))));
    UtxoStandardCoin::get_verbose_transactions_from_cache_or_rpc.mock_safe(move |_, tx_ids| {
        itertools::assert_equal(tx_ids, iter::once(tx_hash));
        let result: HashMap<_, _> = iter::once((tx_hash, VerboseTransactionFrom::Cache(verbose.clone()))).collect();
        MockResult::Return(Box::new(futures01::future::ok(result)))
    });
    static IS_UNSPENT_MATURE_CALLED: AtomicBool = AtomicBool::new(false);
    UtxoStandardCoin::is_unspent_mature.mock_safe(move |_, tx: &RpcTransaction| {
        // check if the transaction height and confirmations are expected
        assert_eq!(tx.height, expected_height);
        assert_eq!(tx.confirmations, expected_confs);
        IS_UNSPENT_MATURE_CALLED.store(true, Ordering::Relaxed);
        MockResult::Return(false)
    });

    // run test
    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Electrum(client), None, false);
    let (unspents, _) = block_on(coin.get_mature_unspent_ordered_list(
        &Address::from_legacyaddress("R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW", &KMD_PREFIXES).unwrap(),
    ))
    .expect("Expected an empty unspent list");
    // unspents should be empty because `is_unspent_mature()` always returns false
    assert!(IS_UNSPENT_MATURE_CALLED.load(Ordering::Relaxed));
    assert!(unspents.mature.is_empty());
    assert_eq!(unspents.immature.len(), 1);
}

#[test]
fn test_get_mature_unspents_ordered_map_from_cache() {
    let unspent_height = None;
    let cached_height = None;
    let cached_confs = 0;
    let block_count = 1000;
    let expected_height = None; // is unknown
    let expected_confs = 0; // is not changed because height is unknown
    test_get_mature_unspent_ordered_map_from_cache_impl(
        unspent_height,
        cached_height,
        cached_confs,
        block_count,
        expected_height,
        expected_confs,
    );

    let unspent_height = None;
    let cached_height = None;
    let cached_confs = 5;
    let block_count = 1000;
    let expected_height = None; // is unknown
    let expected_confs = 5; // is not changed because height is unknown
    test_get_mature_unspent_ordered_map_from_cache_impl(
        unspent_height,
        cached_height,
        cached_confs,
        block_count,
        expected_height,
        expected_confs,
    );

    let unspent_height = Some(998);
    let cached_height = None;
    let cached_confs = 0;
    let block_count = 1000;
    let expected_height = Some(998); // as the unspent_height
    let expected_confs = 3; // 1000 - 998 + 1
    test_get_mature_unspent_ordered_map_from_cache_impl(
        unspent_height,
        cached_height,
        cached_confs,
        block_count,
        expected_height,
        expected_confs,
    );

    let unspent_height = None;
    let cached_height = Some(998);
    let cached_confs = 0;
    let block_count = 1000;
    let expected_height = Some(998); // as the cached_height
    let expected_confs = 3; // 1000 - 998 + 1
    test_get_mature_unspent_ordered_map_from_cache_impl(
        unspent_height,
        cached_height,
        cached_confs,
        block_count,
        expected_height,
        expected_confs,
    );

    let unspent_height = Some(998);
    let cached_height = Some(997);
    let cached_confs = 0;
    let block_count = 1000;
    let expected_height = Some(998); // as the unspent_height
    let expected_confs = 3; // 1000 - 998 + 1
    test_get_mature_unspent_ordered_map_from_cache_impl(
        unspent_height,
        cached_height,
        cached_confs,
        block_count,
        expected_height,
        expected_confs,
    );

    // block_count < tx_height
    let unspent_height = None;
    let cached_height = Some(1000);
    let cached_confs = 1;
    let block_count = 999;
    let expected_height = Some(1000); // as the cached_height
    let expected_confs = 1; // is not changed because height cannot be calculated
    test_get_mature_unspent_ordered_map_from_cache_impl(
        unspent_height,
        cached_height,
        cached_confs,
        block_count,
        expected_height,
        expected_confs,
    );

    // block_count == tx_height
    let unspent_height = None;
    let cached_height = Some(1000);
    let cached_confs = 1;
    let block_count = 1000;
    let expected_height = Some(1000); // as the cached_height
    let expected_confs = 1; // 1000 - 1000 + 1
    test_get_mature_unspent_ordered_map_from_cache_impl(
        unspent_height,
        cached_height,
        cached_confs,
        block_count,
        expected_height,
        expected_confs,
    );

    // tx_height == 0
    let unspent_height = Some(0);
    let cached_height = None;
    let cached_confs = 1;
    let block_count = 1000;
    let expected_height = Some(0); // as the cached_height
    let expected_confs = 1; // is not changed because tx_height is expected to be not zero
    test_get_mature_unspent_ordered_map_from_cache_impl(
        unspent_height,
        cached_height,
        cached_confs,
        block_count,
        expected_height,
        expected_confs,
    );
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_native_client_unspents_filtered_using_tx_cache_single_tx_in_cache() {
    let client = native_client_for_test();
    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    let address: Address = Address::from_legacyaddress("RGfFZaaNV68uVe1uMf6Y37Y8E1i2SyYZBN", &KMD_PREFIXES).unwrap();
    let output_script = coin.script_for_address(&address).unwrap();
    block_on(coin.as_ref().recently_spent_outpoints.lock()).for_script_pubkey = output_script.clone().into();

    // https://morty.explorer.dexstats.info/tx/31c7aaae89ab1c39febae164a3190a86ed7c6c6f8c9dc98ec28d508b7929d347
    let tx: UtxoTx = "0400008085202f89027f57730fcbbc2c72fb18bcc3766a713044831a117bb1cade3ed88644864f7333020000006a47304402206e3737b2fcf078b61b16fa67340cc3e79c5d5e2dc9ffda09608371552a3887450220460a332aa1b8ad8f2de92d319666f70751078b221199951f80265b4f7cef8543012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff42b916a80430b80a77e114445b08cf120735447a524de10742fac8f6a9d4170f000000006a473044022004aa053edafb9d161ea8146e0c21ed1593aa6b9404dd44294bcdf920a1695fd902202365eac15dbcc5e9f83e2eed56a8f2f0e5aded36206f9c3fabc668fd4665fa2d012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff03547b16000000000017a9143e8ad0e2bf573d32cb0b3d3a304d9ebcd0c2023b870000000000000000166a144e2b3c0323ab3c2dc6f86dc5ec0729f11e42f56103970400000000001976a91450f4f098306f988d8843004689fae28c83ef16e888ac89c5925f000000000000000000000000000000".into();
    let spent_by_tx = vec![
        UnspentInfo {
            outpoint: tx.inputs[0].previous_output,
            value: 886737,
            height: Some(642293),
            script: output_script.clone(),
        },
        UnspentInfo {
            outpoint: tx.inputs[1].previous_output,
            value: 88843,
            height: Some(642293),
            script: output_script,
        },
    ];

    block_on(coin.as_ref().recently_spent_outpoints.lock()).add_spent(
        spent_by_tx.clone(),
        tx.hash(),
        tx.outputs.clone(),
    );
    NativeClient::list_unspent
        .mock_safe(move |_, _, _| MockResult::Return(Box::new(futures01::future::ok(spent_by_tx.clone()))));

    let (unspents_ordered, _) = block_on(coin.get_unspent_ordered_list(&address)).unwrap();
    // output 2 is change so it must be returned
    let expected_unspent = UnspentInfo {
        outpoint: OutPoint {
            hash: tx.hash(),
            index: 2,
        },
        value: tx.outputs[2].value,
        height: None,
        // Should be the same as: Some(output_script.clone()),
        script: tx.outputs[2].script_pubkey.clone().into(),
    };
    assert_eq!(vec![expected_unspent], unspents_ordered);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_native_client_unspents_filtered_using_tx_cache_single_several_chained_txs_in_cache() {
    let client = native_client_for_test();
    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    let address: Address = Address::from_legacyaddress("RGfFZaaNV68uVe1uMf6Y37Y8E1i2SyYZBN", &KMD_PREFIXES).unwrap();
    let output_script = coin.script_for_address(&address).unwrap();
    block_on(coin.as_ref().recently_spent_outpoints.lock()).for_script_pubkey = output_script.clone().into();

    // https://morty.explorer.dexstats.info/tx/31c7aaae89ab1c39febae164a3190a86ed7c6c6f8c9dc98ec28d508b7929d347
    let tx_0: UtxoTx = "0400008085202f89027f57730fcbbc2c72fb18bcc3766a713044831a117bb1cade3ed88644864f7333020000006a47304402206e3737b2fcf078b61b16fa67340cc3e79c5d5e2dc9ffda09608371552a3887450220460a332aa1b8ad8f2de92d319666f70751078b221199951f80265b4f7cef8543012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff42b916a80430b80a77e114445b08cf120735447a524de10742fac8f6a9d4170f000000006a473044022004aa053edafb9d161ea8146e0c21ed1593aa6b9404dd44294bcdf920a1695fd902202365eac15dbcc5e9f83e2eed56a8f2f0e5aded36206f9c3fabc668fd4665fa2d012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff03547b16000000000017a9143e8ad0e2bf573d32cb0b3d3a304d9ebcd0c2023b870000000000000000166a144e2b3c0323ab3c2dc6f86dc5ec0729f11e42f56103970400000000001976a91450f4f098306f988d8843004689fae28c83ef16e888ac89c5925f000000000000000000000000000000".into();
    let spent_by_tx_0 = vec![
        UnspentInfo {
            outpoint: tx_0.inputs[0].previous_output,
            value: 886737,
            height: Some(642293),
            script: output_script.clone(),
        },
        UnspentInfo {
            outpoint: tx_0.inputs[1].previous_output,
            value: 88843,
            height: Some(642293),
            script: output_script.clone(),
        },
    ];
    block_on(coin.as_ref().recently_spent_outpoints.lock()).add_spent(spent_by_tx_0.clone(), tx_0.hash(), tx_0.outputs);

    // https://morty.explorer.dexstats.info/tx/dbfc821e482747a3512ee6d5734f9df2aa73dab07e2fcd86abeadb462e795bf9
    let tx_1: UtxoTx = "0400008085202f890347d329798b508dc28ec99d8c6f6c7ced860a19a364e1bafe391cab89aeaac731020000006a47304402203ea8b380d0a7e64348869ef7c4c2bfa966fc7b148633003332fa8d0ab0c1bc5602202cc63fabdd2a6578c52d8f4f549069b16505f2ead48edc2b8de299be15aadf9a012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff1d1fd3a6b01710647a7f4a08c6de6075cb8e78d5069fa50f10c4a2a10ded2a95000000006a47304402203868945edc0f6dc2ee43d70a69ee4ec46ca188dc493173ce58924ba9bf6ee7a50220648ff99ce458ca72800758f6a1bd3800cd05ff9c3122f23f3653c25e09d22c79012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff7932150df8b4a1852b8b84b89b0d5322bf74665fb7f76a728369fd6895d3fd48000000006a4730440220127918c6f79c11f7f2376a6f3b750ed4c7103183181ad1218afcb2625ece9599022028c05e88d3a2f97cebd84a718cda33b62b48b18f16278fa8e531fd2155e61ee8012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff0329fd12000000000017a914cafb62e3e8bdb8db3735c39b92743ac6ebc9ef20870000000000000000166a14a7416b070c9bb98f4bafae55616f005a2a30bd6014b40c00000000001976a91450f4f098306f988d8843004689fae28c83ef16e888ac8cc5925f000000000000000000000000000000".into();
    let spent_by_tx_1 = vec![
        UnspentInfo {
            outpoint: tx_1.inputs[0].previous_output,
            value: 300803,
            height: Some(642293),
            script: output_script.clone(),
        },
        UnspentInfo {
            outpoint: tx_1.inputs[1].previous_output,
            value: 888544,
            height: Some(642293),
            script: output_script.clone(),
        },
        UnspentInfo {
            outpoint: tx_1.inputs[2].previous_output,
            value: 888642,
            height: Some(642293),
            script: output_script.clone(),
        },
    ];
    block_on(coin.as_ref().recently_spent_outpoints.lock()).add_spent(spent_by_tx_1.clone(), tx_1.hash(), tx_1.outputs);
    // https://morty.explorer.dexstats.info/tx/12ea22a7cde9efb66b76f9b84345ddfc4c34870e293bfa8eac68d7df83dffa4b
    let tx_2: UtxoTx = "0400008085202f8902f95b792e46dbeaab86cd2f7eb0da73aaf29d4f73d5e62e51a34727481e82fcdb020000006a4730440220347adefe33ed5afbbb8e5d453afd527319f9a50ab790023296a981da095ca4a2022029a68ef6fd5a4decf3793d4c33994eb8658408f3b14a6d439c4753b2dde954ee012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff75bd4348594f8ff2a216e5ad7533b37d47d2a2767b0b88d43972ad51895355e2000000006a473044022069b36c0f65d56e02bc179f7442806374c4163d07939090aba1da736abad9a77d022006dc39adf48e02033ae9d4a48540752ae3b3841e3ec60d2e86dececb88b9e518012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff03414111000000000017a914a153024c826a3a42c2e501eca5d7dacd3fc59976870000000000000000166a14db0e6f4d418d68dce8e5beb26cc5078e01e2e3ace2fe0800000000001976a91450f4f098306f988d8843004689fae28c83ef16e888ac8fc5925f000000000000000000000000000000".into();
    let spent_by_tx_2 = vec![
        UnspentInfo {
            outpoint: tx_2.inputs[0].previous_output,
            value: 832532,
            height: Some(642293),
            script: output_script.clone(),
        },
        UnspentInfo {
            outpoint: tx_2.inputs[1].previous_output,
            value: 888823,
            height: Some(642293),
            script: output_script,
        },
    ];
    block_on(coin.as_ref().recently_spent_outpoints.lock()).add_spent(
        spent_by_tx_2.clone(),
        tx_2.hash(),
        tx_2.outputs.clone(),
    );

    let mut unspents_to_return = spent_by_tx_0;
    unspents_to_return.extend(spent_by_tx_1);
    unspents_to_return.extend(spent_by_tx_2);

    NativeClient::list_unspent
        .mock_safe(move |_, _, _| MockResult::Return(Box::new(futures01::future::ok(unspents_to_return.clone()))));

    let (unspents_ordered, _) = block_on(coin.get_unspent_ordered_list(&address)).unwrap();

    // output 2 is change so it must be returned
    let expected_unspent = UnspentInfo {
        outpoint: OutPoint {
            hash: tx_2.hash(),
            index: 2,
        },
        value: tx_2.outputs[2].value,
        height: None,
        // Should be the same as: Some(output_script.clone()),
        script: tx_2.outputs[2].script_pubkey.clone().into(),
    };
    assert_eq!(vec![expected_unspent], unspents_ordered);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_native_client_unspents_p2pk_filtered_using_tx_cache_single_tx_in_cache() {
    let client = native_client_for_test();
    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    let address: Address = Address::from_legacyaddress("RWdLNGQ428ZmhbMs6sVi42KPUbiKYKhiLr", &KMD_PREFIXES).unwrap();
    let output_script = coin.script_for_address(&address).unwrap();
    block_on(coin.as_ref().recently_spent_outpoints.lock()).for_script_pubkey = output_script.into();

    // https://morty.explorer.dexstats.info/tx/e5b6a8c98bc802cf764430d79f2a8fdb1373ecf8bb0fb07e9ffea559083e9ead
    let tx: UtxoTx = "0400008085202f8902c828be61a42ca160a5cbf549b74cb3ac0a8011eb32924b61dd22cb8153dd0c9c000000004948304502210094e99bb9369d2b3c4f13b8ffd7c4abacbdb2b5ee06d9453b13c23d9a924e1b34022005e5939999537f8eb901b6a37fb0776d5f6196fc7a38b1ecc8b043e84ea420bb01ffffffff2ec39160776dc986b7619e07f077d719a3247e1c0ab2148213013a34d79fd56a000000006b483045022100909ec2d09276891d48765f101d6501ef0606af971309f86a814042421f420bc202200bc376f0186aeec215c4ad99d04d8e44354b6838044b26a6a195b36b86de2ed6012102b2e5b95daf6600d4b210ce5e0a9dae507df9c7b89618c4aea05045e5acc1e7eeffffffff01eceadcd4060000001976a914ea29e13cd4446800297a5883a48caddd6d12377688ac00000000becd09000000000000000000000000".into();
    let spent_by_tx = vec![
        UnspentInfo {
            outpoint: tx.inputs[0].previous_output,
            value: 100139000,
            height: Some(642293),
            script: "2103c9f7c3b8ff78beb991cf806a5c91561cfe68f530c9df2b1402e57621ecbcd6b0ac".into(),
        },
        UnspentInfo {
            outpoint: tx.inputs[1].previous_output,
            value: 29240917628,
            height: Some(642293),
            script: "76a914ea29e13cd4446800297a5883a48caddd6d12377688ac".into(),
        },
    ];

    block_on(coin.as_ref().recently_spent_outpoints.lock()).add_spent(
        spent_by_tx.clone(),
        tx.hash(),
        tx.outputs.clone(),
    );
    NativeClient::list_unspent
        .mock_safe(move |_, _, _| MockResult::Return(Box::new(futures01::future::ok(spent_by_tx.clone()))));

    let (unspents_ordered, _) = block_on(coin.get_unspent_ordered_list(&address)).unwrap();
    // output 0 is spent to self so it must be returned
    let expected_unspent = UnspentInfo {
        outpoint: OutPoint {
            hash: tx.hash(),
            index: 0,
        },
        value: tx.outputs[0].value,
        height: None,
        // Should be the same as: Some(output_script.clone()),
        script: tx.outputs[0].script_pubkey.clone().into(),
    };
    assert_eq!(vec![expected_unspent], unspents_ordered);
}

#[test]
fn validate_address_res_format() {
    let btc_017_and_above_response = json!({
      "isvalid": true,
      "address": "1JsAjr6d21j9T8EMsYnQ6GXf1mM523JAv1",
      "scriptPubKey": "76a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac",
      "isscript": false,
      "iswitness": false
    });

    let _: ValidateAddressRes = json::from_value(btc_017_and_above_response).unwrap();

    let btc_016_response = json!({
      "isvalid": true,
      "address": "RT9MpMyucqXiX8bZLimXBnrrn2ofmdGNKd",
      "scriptPubKey": "76a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac",
      "ismine": false,
      "iswatchonly": true,
      "isscript": false,
      "account": "RT9MpMyucqXiX8bZLimXBnrrn2ofmdGNKd",
      "timestamp": 0
    });

    let _: ValidateAddressRes = json::from_value(btc_016_response).unwrap();
}

#[test]
fn get_address_info_format() {
    let response = json!({
      "address": "Ld6814QT6fyChvvX3gmhNHbRDyiMBvPr9s",
      "scriptPubKey": "76a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac",
      "ismine": false,
      "solvable": false,
      "iswatchonly": true,
      "isscript": false,
      "iswitness": false,
      "label": "Ld6814QT6fyChvvX3gmhNHbRDyiMBvPr9s",
      "ischange": false,
      "timestamp": 0,
      "labels": [
        {
          "name": "Ld6814QT6fyChvvX3gmhNHbRDyiMBvPr9s",
          "purpose": "receive"
        }
      ]
    });

    let _: GetAddressInfoRes = json::from_value(response).unwrap();
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_native_is_address_imported_validate_address_is_mine() {
    let client = native_client_for_test();
    NativeClientImpl::validate_address.mock_safe(|_, _| {
        let result = ValidateAddressRes {
            is_valid: false,
            address: "".to_string(),
            script_pub_key: Default::default(),
            seg_id: None,
            is_mine: Some(true),
            is_watch_only: Some(false),
            is_script: false,
            account: None,
        };
        MockResult::Return(Box::new(futures01::future::ok(result)))
    });

    let address = "";
    let imported = block_on(client.is_address_imported(address)).unwrap();
    assert!(imported);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_native_is_address_imported_validate_address_is_watch_only() {
    let client = native_client_for_test();
    NativeClientImpl::validate_address.mock_safe(|_, _| {
        let result = ValidateAddressRes {
            is_valid: false,
            address: "".to_string(),
            script_pub_key: Default::default(),
            seg_id: None,
            is_mine: Some(false),
            is_watch_only: Some(true),
            is_script: false,
            account: None,
        };
        MockResult::Return(Box::new(futures01::future::ok(result)))
    });

    let address = "";
    let imported = block_on(client.is_address_imported(address)).unwrap();
    assert!(imported);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_native_is_address_imported_validate_address_false() {
    let client = native_client_for_test();
    NativeClientImpl::validate_address.mock_safe(|_, _| {
        let result = ValidateAddressRes {
            is_valid: false,
            address: "".to_string(),
            script_pub_key: Default::default(),
            seg_id: None,
            is_mine: Some(false),
            is_watch_only: Some(false),
            is_script: false,
            account: None,
        };
        MockResult::Return(Box::new(futures01::future::ok(result)))
    });

    let address = "";
    let imported = block_on(client.is_address_imported(address)).unwrap();
    assert!(!imported);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_native_is_address_imported_fallback_to_address_info_is_mine() {
    let client = native_client_for_test();
    NativeClientImpl::validate_address.mock_safe(|_, _| {
        let result = ValidateAddressRes {
            is_valid: false,
            address: "".to_string(),
            script_pub_key: Default::default(),
            seg_id: None,
            is_mine: None,
            is_watch_only: None,
            is_script: false,
            account: None,
        };
        MockResult::Return(Box::new(futures01::future::ok(result)))
    });

    NativeClientImpl::get_address_info.mock_safe(|_, _| {
        let result = GetAddressInfoRes {
            is_mine: true,
            is_watch_only: false,
        };
        MockResult::Return(Box::new(futures01::future::ok(result)))
    });

    let address = "";
    let imported = block_on(client.is_address_imported(address)).unwrap();
    assert!(imported);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_native_is_address_imported_fallback_to_address_info_is_watch_only() {
    let client = native_client_for_test();
    NativeClientImpl::validate_address.mock_safe(|_, _| {
        let result = ValidateAddressRes {
            is_valid: false,
            address: "".to_string(),
            script_pub_key: Default::default(),
            seg_id: None,
            is_mine: None,
            is_watch_only: None,
            is_script: false,
            account: None,
        };
        MockResult::Return(Box::new(futures01::future::ok(result)))
    });

    NativeClientImpl::get_address_info.mock_safe(|_, _| {
        let result = GetAddressInfoRes {
            is_mine: false,
            is_watch_only: true,
        };
        MockResult::Return(Box::new(futures01::future::ok(result)))
    });

    let address = "";
    let imported = block_on(client.is_address_imported(address)).unwrap();
    assert!(imported);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_native_is_address_imported_fallback_to_address_info_false() {
    let client = native_client_for_test();
    NativeClientImpl::validate_address.mock_safe(|_, _| {
        let result = ValidateAddressRes {
            is_valid: false,
            address: "".to_string(),
            script_pub_key: Default::default(),
            seg_id: None,
            is_mine: None,
            is_watch_only: None,
            is_script: false,
            account: None,
        };
        MockResult::Return(Box::new(futures01::future::ok(result)))
    });

    NativeClientImpl::get_address_info.mock_safe(|_, _| {
        let result = GetAddressInfoRes {
            is_mine: false,
            is_watch_only: false,
        };
        MockResult::Return(Box::new(futures01::future::ok(result)))
    });

    let address = "";
    let imported = block_on(client.is_address_imported(address)).unwrap();
    assert!(!imported);
}

/// Test if the [`NativeClient::find_output_spend`] handle the conflicting transactions correctly.
/// https://github.com/KomodoPlatform/atomicDEX-API/pull/775
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_find_output_spend_skips_conflicting_transactions() {
    const LIST_SINCE_BLOCK_JSON: &str = r#"{"transactions":[{"involvesWatchonly":true,"account":"","address":"RAsbVN52LC2hEp3UWWSLbV8pJ8CneKjW9F","category":"send","amount":-0.01537462,"vout":0,"fee":-0.00001000,"rawconfirmations":-1,"confirmations":-1,"txid":"220c337006b2581c3da734ef9f1106601e8538ebab823d0dd6719a4d4580fd04","walletconflicts":["a2144bee4eac4b41ab1aed2dd8f854785b3ddebd617d48696dd84e62d129544b"],"time":1607831631,"timereceived":1607831631,"vjoinsplit":[],"size":320},{"involvesWatchonly":true,"account":"","address":"RAsbVN52LC2hEp3UWWSLbV8pJ8CneKjW9F","category":"send","amount":-0.01537462,"vout":0,"fee":-0.00001000,"rawconfirmations":-1,"confirmations":-1,"txid":"6fb83afb1bf309515fa429814bf07552eea951656fdee913f3aa687d513cd720","walletconflicts":["4aad6471f59e5912349cd7679bc029bfbd5da54d34c235d20500249f98f549e4"],"time":1607831556,"timereceived":1607831556,"vjoinsplit":[],"size":320},{"account":"","address":"RT9MpMyucqXiX8bZLimXBnrrn2ofmdGNKd","category":"receive","amount":0.54623851,"vout":2,"rawconfirmations":1617,"confirmations":1617,"blockhash":"000000000c33a387d73180220a5a8f2fe6081bad9bdfc0dba5a9985abcee8294","blockindex":7,"blocktime":1607957613,"expiryheight":0,"txid":"45e4900a2b330800a356a74ce2a97370596ad3a25e689e3ed5c36e421d12bbf7","walletconflicts":[],"time":1607957175,"timereceived":1607957175,"vjoinsplit":[],"size":567},{"involvesWatchonly":true,"account":"","address":"RT9MpMyucqXiX8bZLimXBnrrn2ofmdGNKd","category":"send","amount":-0.00797200,"vout":0,"fee":-0.00001000,"rawconfirmations":-1,"confirmations":-1,"txid":"bfc99c06d1a060cdbeba05620dc1c6fdb7351eb4c04b7aae578688ca6aeaeafd","walletconflicts":[],"time":1607957792,"timereceived":1607957792,"vjoinsplit":[],"size":286}],"lastblock":"06082d363f78174fd13b126994210d3c3ad9d073ee3983ad59fe8b76e6e3e071"}"#;
    // in the json above this transaction is only one not conflicting
    const NON_CONFLICTING_TXID: &str = "45e4900a2b330800a356a74ce2a97370596ad3a25e689e3ed5c36e421d12bbf7";
    let expected_txid: H256Json = <[u8; 32]>::from_hex(NON_CONFLICTING_TXID).unwrap().into();
    NativeClientImpl::get_block_hash.mock_safe(|_, _| {
        // no matter what we return here
        let blockhash: H256Json =
            <[u8; 32]>::from_hex("000000000c33a387d73180220a5a8f2fe6081bad9bdfc0dba5a9985abcee8294")
                .unwrap()
                .into();
        MockResult::Return(Box::new(futures01::future::ok(blockhash)))
    });

    NativeClientImpl::list_since_block.mock_safe(|_, _| {
        let listsinceblockres: ListSinceBlockRes =
            json::from_str(LIST_SINCE_BLOCK_JSON).expect("Json is expected to be valid");
        MockResult::Return(Box::new(futures01::future::ok(listsinceblockres)))
    });

    static GET_RAW_TRANSACTION_BYTES_CALLED: AtomicU32 = AtomicU32::new(0);
    NativeClientImpl::get_raw_transaction_bytes.mock_safe(move |_, txid| {
        GET_RAW_TRANSACTION_BYTES_CALLED.fetch_add(1, Ordering::Relaxed);
        assert_eq!(*txid, expected_txid);
        // no matter what we return here
        let bytes: BytesJson = hex::decode("0400008085202f890347d329798b508dc28ec99d8c6f6c7ced860a19a364e1bafe391cab89aeaac731020000006a47304402203ea8b380d0a7e64348869ef7c4c2bfa966fc7b148633003332fa8d0ab0c1bc5602202cc63fabdd2a6578c52d8f4f549069b16505f2ead48edc2b8de299be15aadf9a012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff1d1fd3a6b01710647a7f4a08c6de6075cb8e78d5069fa50f10c4a2a10ded2a95000000006a47304402203868945edc0f6dc2ee43d70a69ee4ec46ca188dc493173ce58924ba9bf6ee7a50220648ff99ce458ca72800758f6a1bd3800cd05ff9c3122f23f3653c25e09d22c79012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff7932150df8b4a1852b8b84b89b0d5322bf74665fb7f76a728369fd6895d3fd48000000006a4730440220127918c6f79c11f7f2376a6f3b750ed4c7103183181ad1218afcb2625ece9599022028c05e88d3a2f97cebd84a718cda33b62b48b18f16278fa8e531fd2155e61ee8012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff0329fd12000000000017a914cafb62e3e8bdb8db3735c39b92743ac6ebc9ef20870000000000000000166a14a7416b070c9bb98f4bafae55616f005a2a30bd6014b40c00000000001976a91450f4f098306f988d8843004689fae28c83ef16e888ac8cc5925f000000000000000000000000000000").unwrap().into();
        MockResult::Return(Box::new(futures01::future::ok(bytes)))
    });
    let client = native_client_for_test();

    // no matter what arguments we will pass to the function because of the mocks above
    let tx: UtxoTx = "0400008085202f89027f57730fcbbc2c72fb18bcc3766a713044831a117bb1cade3ed88644864f7333020000006a47304402206e3737b2fcf078b61b16fa67340cc3e79c5d5e2dc9ffda09608371552a3887450220460a332aa1b8ad8f2de92d319666f70751078b221199951f80265b4f7cef8543012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff42b916a80430b80a77e114445b08cf120735447a524de10742fac8f6a9d4170f000000006a473044022004aa053edafb9d161ea8146e0c21ed1593aa6b9404dd44294bcdf920a1695fd902202365eac15dbcc5e9f83e2eed56a8f2f0e5aded36206f9c3fabc668fd4665fa2d012102d8c948c6af848c588517288168faa397d6ba3ea924596d03d1d84f224b5123c2ffffffff03547b16000000000017a9143e8ad0e2bf573d32cb0b3d3a304d9ebcd0c2023b870000000000000000166a144e2b3c0323ab3c2dc6f86dc5ec0729f11e42f56103970400000000001976a91450f4f098306f988d8843004689fae28c83ef16e888ac89c5925f000000000000000000000000000000".into();
    let vout = 0;
    let from_block = 0;
    let actual = block_on_f01(client.find_output_spend(
        tx.hash(),
        &tx.outputs[vout].script_pubkey,
        vout,
        BlockHashOrHeight::Height(from_block),
        TxHashAlgo::DSHA256,
    ));
    assert_eq!(actual, Ok(None));
    assert_eq!(GET_RAW_TRANSACTION_BYTES_CALLED.load(Ordering::Relaxed), 1);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_qtum_is_unspent_mature() {
    use crate::utxo::qtum::QtumBasedCoin;
    use rpc::v1::types::{ScriptType, SignedTransactionOutput, TransactionOutputScript};

    let mut coin_fields = utxo_coin_fields_for_test(UtxoRpcClientEnum::Native(native_client_for_test()), None, false);
    // Qtum's mature confirmations is 500 blocks
    coin_fields.conf.mature_confirmations = 500;
    let arc: UtxoArc = coin_fields.into();
    let coin = QtumCoin::from(arc);

    let empty_output = SignedTransactionOutput {
        value: Some(0.),
        n: 0,
        script: TransactionOutputScript {
            asm: "".into(),
            hex: "".into(),
            req_sigs: 0,
            script_type: ScriptType::NonStandard,
            addresses: vec![],
        },
    };
    let real_output = SignedTransactionOutput {
        value: Some(117.02430015),
        n: 1,
        script: TransactionOutputScript {
            asm: "03e71b9c152bb233ddfe58f20056715c51b054a1823e0aba108e6f1cea0ceb89c8 OP_CHECKSIG".into(),
            hex: "2103e71b9c152bb233ddfe58f20056715c51b054a1823e0aba108e6f1cea0ceb89c8ac".into(),
            req_sigs: 0,
            script_type: ScriptType::PubKey,
            addresses: vec![],
        },
    };

    let mut tx = RpcTransaction {
        hex: Default::default(),
        txid: "47d983175720ba2a67f36d0e1115a129351a2f340bdde6ecb6d6029e138fe920".into(),
        hash: None,
        size: Default::default(),
        vsize: Default::default(),
        version: 2,
        locktime: 0,
        vin: vec![],
        vout: vec![empty_output, real_output],
        blockhash: "c23882939ff695be36546ea998eb585e962b043396e4d91959477b9796ceb9e1".into(),
        confirmations: 421,
        rawconfirmations: None,
        time: 1590671504,
        blocktime: 1590671504,
        height: None,
    };

    // output is coinbase and has confirmations < QTUM_MATURE_CONFIRMATIONS
    assert!(!coin.is_qtum_unspent_mature(&tx));

    tx.confirmations = 501;
    // output is coinbase but has confirmations > QTUM_MATURE_CONFIRMATIONS
    assert!(coin.is_qtum_unspent_mature(&tx));

    tx.confirmations = 421;
    // remove empty output
    tx.vout.remove(0);
    // output is not coinbase
    assert!(coin.is_qtum_unspent_mature(&tx));
}

#[test]
#[ignore]
// TODO it fails at least when fee is 2055837 sat per kbyte, need to investigate
fn test_get_sender_trade_fee_dynamic_tx_fee() {
    let rpc_client = electrum_client_for_test(&["electrum1.cipig.net:10071"]);
    let mut coin_fields = utxo_coin_fields_for_test(
        UtxoRpcClientEnum::Electrum(rpc_client),
        Some("bob passphrase max taker vol with dynamic trade fee"),
        false,
    );
    coin_fields.tx_fee = FeeRate::Dynamic(EstimateFeeMethod::Standard);
    let coin = utxo_coin_from_fields(coin_fields);
    let my_balance = block_on_f01(coin.my_spendable_balance()).expect("!my_balance");
    let expected_balance = BigDecimal::from_str("2.22222").expect("!BigDecimal::from_str");
    assert_eq!(my_balance, expected_balance);

    let fee1 = block_on(coin.get_sender_trade_fee(
        TradePreimageValue::UpperBound(my_balance.clone()),
        FeeApproxStage::WithoutApprox,
    ))
    .expect("!get_sender_trade_fee");

    let value_without_fee = &my_balance - &fee1.amount.to_decimal();
    log!("value_without_fee {}", value_without_fee);
    let fee2 = block_on(coin.get_sender_trade_fee(
        TradePreimageValue::Exact(value_without_fee),
        FeeApproxStage::WithoutApprox,
    ))
    .expect("!get_sender_trade_fee");
    assert_eq!(fee1, fee2);

    // `2.21934443` value was obtained as a result of executing the `max_taker_vol` RPC call for this wallet
    let max_taker_vol = BigDecimal::from_str("2.21934443").expect("!BigDecimal::from_str");
    let fee3 =
        block_on(coin.get_sender_trade_fee(TradePreimageValue::Exact(max_taker_vol), FeeApproxStage::WithoutApprox))
            .expect("!get_sender_trade_fee");
    assert_eq!(fee1, fee3);
}

// validate an old tx with no output with the burn account
// TODO: remove when we disable such old style txns
#[test]
fn test_validate_old_fee_tx() {
    let rpc_client = electrum_client_for_test(MARTY_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Electrum(rpc_client), None, false);
    let tx_bytes = hex::decode("0400008085202f8901033aedb3c3c02fc76c15b393c7b1f638cfa6b4a1d502e00d57ad5b5305f12221000000006a473044022074879aabf38ef943eba7e4ce54c444d2d6aa93ac3e60ea1d7d288d7f17231c5002205e1671a62d8c031ac15e0e8456357e54865b7acbf49c7ebcba78058fd886b4bd012103242d9cb2168968d785f6914c494c303ff1c27ba0ad882dbc3c15cfa773ea953cffffffff0210270000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac4802d913000000001976a914902053231ef0541a7628c11acac40d30f2a127bd88ac008e3765000000000000000000000000000000").unwrap();
    let taker_fee_tx = coin.tx_enum_from_bytes(&tx_bytes).unwrap();
    let amount: MmNumber = "0.0001".parse::<BigDecimal>().unwrap().into();
    let dex_fee = DexFee::Standard(amount);
    let validate_fee_args = ValidateFeeArgs {
        fee_tx: &taker_fee_tx,
        expected_sender: &hex::decode("03242d9cb2168968d785f6914c494c303ff1c27ba0ad882dbc3c15cfa773ea953c").unwrap(),
        dex_fee: &dex_fee,
        min_block_number: 0,
        uuid: &[],
    };
    let result = block_on(coin.validate_fee(validate_fee_args));
    log!("result: {:?}", result);
    assert!(result.is_ok());
}

#[test]
fn test_validate_fee_wrong_sender() {
    let rpc_client = electrum_client_for_test(MARTY_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Electrum(rpc_client), None, false);
    // https://marty.explorer.dexstats.info/tx/99349d1c72ef396ecb39ab2989b888b02e22382249271c79cda8139825adc468
    let tx_bytes = hex::decode("0400008085202f8901033aedb3c3c02fc76c15b393c7b1f638cfa6b4a1d502e00d57ad5b5305f12221000000006a473044022074879aabf38ef943eba7e4ce54c444d2d6aa93ac3e60ea1d7d288d7f17231c5002205e1671a62d8c031ac15e0e8456357e54865b7acbf49c7ebcba78058fd886b4bd012103242d9cb2168968d785f6914c494c303ff1c27ba0ad882dbc3c15cfa773ea953cffffffff0210270000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac4802d913000000001976a914902053231ef0541a7628c11acac40d30f2a127bd88ac008e3765000000000000000000000000000000").unwrap();
    let taker_fee_tx = coin.tx_enum_from_bytes(&tx_bytes).unwrap();
    let amount: BigDecimal = "0.0001".parse().unwrap();
    let validate_fee_args = ValidateFeeArgs {
        fee_tx: &taker_fee_tx,
        expected_sender: &DEX_FEE_ADDR_RAW_PUBKEY,
        dex_fee: &DexFee::Standard(amount.into()),
        min_block_number: 0,
        uuid: &[],
    };
    let error = block_on(coin.validate_fee(validate_fee_args)).unwrap_err().into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains(INVALID_SENDER_ERR_LOG)),
        _ => panic!("Expected `WrongPaymentTx` wrong sender address, found {:?}", error),
    }
}

#[test]
fn test_validate_fee_min_block() {
    let rpc_client = electrum_client_for_test(MARTY_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Electrum(rpc_client), None, false);
    // https://marty.explorer.dexstats.info/tx/99349d1c72ef396ecb39ab2989b888b02e22382249271c79cda8139825adc468
    let tx_bytes = hex::decode("0400008085202f8901033aedb3c3c02fc76c15b393c7b1f638cfa6b4a1d502e00d57ad5b5305f12221000000006a473044022074879aabf38ef943eba7e4ce54c444d2d6aa93ac3e60ea1d7d288d7f17231c5002205e1671a62d8c031ac15e0e8456357e54865b7acbf49c7ebcba78058fd886b4bd012103242d9cb2168968d785f6914c494c303ff1c27ba0ad882dbc3c15cfa773ea953cffffffff0210270000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac4802d913000000001976a914902053231ef0541a7628c11acac40d30f2a127bd88ac008e3765000000000000000000000000000000").unwrap();
    let taker_fee_tx = coin.tx_enum_from_bytes(&tx_bytes).unwrap();
    let amount: BigDecimal = "0.0001".parse().unwrap();
    let sender_pub = hex::decode("03242d9cb2168968d785f6914c494c303ff1c27ba0ad882dbc3c15cfa773ea953c").unwrap();
    let validate_fee_args = ValidateFeeArgs {
        fee_tx: &taker_fee_tx,
        expected_sender: &sender_pub,
        dex_fee: &DexFee::Standard(amount.into()),
        min_block_number: 278455,
        uuid: &[],
    };
    let error = block_on(coin.validate_fee(validate_fee_args)).unwrap_err().into_inner();
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("confirmed before min_block")),
        _ => panic!("Expected `WrongPaymentTx` early confirmation, found {:?}", error),
    }
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/857
fn test_validate_fee_bch_70_bytes_signature() {
    let rpc_client = electrum_client_for_test(&[
        "electrum1.cipig.net:10055",
        "electrum2.cipig.net:10055",
        "electrum3.cipig.net:10055",
    ]);
    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Electrum(rpc_client), None, false);
    // https://blockchair.com/bitcoin-cash/transaction/ccee05a6b5bbc6f50d2a65a5a3a04690d3e2d81082ad57d3ab471189f53dd70d
    let tx_bytes = hex::decode("0100000002cae89775f264e50f14238be86a7184b7f77bfe26f54067b794c546ec5eb9c91a020000006b483045022100d6ed080f722a0637a37552382f462230cc438984bc564bdb4b7094f06cfa38fa022062304a52602df1fbb3bebac4f56e1632ad456f62d9031f4983f07e546c8ec4d8412102ae7dc4ef1b49aadeff79cfad56664105f4d114e1716bc4f930cb27dbd309e521ffffffff11f386a6fe8f0431cb84f549b59be00f05e78f4a8a926c5e023a0d5f9112e8200000000069463043021f17eb93ed20a6f2cd357eabb41a4ec6329000ddc6d5b42ecbe642c5d41b206a022026bc4920c4ce3af751283574baa8e4a3efd4dad0d8fe6ba3ddf5d75628d36fda412102ae7dc4ef1b49aadeff79cfad56664105f4d114e1716bc4f930cb27dbd309e521ffffffff0210270000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac57481c00000000001976a914bac11ce4cd2b1df2769c470d09b54f86df737e3c88ac035b4a60").unwrap();
    let taker_fee_tx = coin.tx_enum_from_bytes(&tx_bytes).unwrap();
    let amount: BigDecimal = "0.0001".parse().unwrap();
    let sender_pub = hex::decode("02ae7dc4ef1b49aadeff79cfad56664105f4d114e1716bc4f930cb27dbd309e521").unwrap();
    let validate_fee_args = ValidateFeeArgs {
        fee_tx: &taker_fee_tx,
        expected_sender: &sender_pub,
        dex_fee: &DexFee::Standard(amount.into()),
        min_block_number: 0,
        uuid: &[],
    };
    block_on(coin.validate_fee(validate_fee_args)).unwrap();
}

#[test]
fn firo_verbose_block_deserialize() {
    let json = json!({
       "hash":"e21ea157b142270ba479a0aeb5571144b2a06f66a693c20675c624a6f211de0a",
       "confirmations":1,
       "strippedsize":234913,
       "size":234913,
       "weight":234913,
       "height":348355,
       "version":536875008,
       "versionHex":"20001000",
       "merkleroot":"b7fa3ce26f5b493397302c260905ca6f8c9ade56cab7cb314dc6f8a1d4c69245",
       "tx":[
          "166d2e6c6b8e1f29192737be5b0df79f7ccb286a898a3bf7253aa091e1002756",
          "f0bcbf10f2aa20d6891c14fdf64eb336df2d4466ebbc6bd5349c61478be77bd3",
          "0305f0fed2286b4504907bd2588dec5205f0807f11d003489b6748437728b6dc",
          "17f69f35b125de65e140de9bffe873702a4550379fb0ae4fe371f703c739e268",
          "ca60309ee4f846f607295aabcea2d0680ca23a7fbb8699ad1b597255ad6c5a73",
          "5aec101f7b2452d293c1a1c3889861bc8e96081f3ecd328859bc005c14d2737e",
          "bd9a8a2fdbad3db6c38e6472fd2e50d452a98553c8a105cb10afc85b5eaadee0",
          "0a52a67bf6ca3784f81b828616cda6bdca314402cded278d98f94b546784a58d",
          "55e6f918b2e7af2886499919b1c4a2ba341180934a4691a1a7166d6dadfcf8b9",
          "7a2d8b10b3bfc3037ee884699ca4770d96575b2d39179801d760d1c86377ff58",
          "ded160f1ec3e978daa2d8adb0b611223946db1c1155522cf9f0796e6f6c081fe"
       ],
       "cbTx":{
          "version":2,
          "height":348355,
          "merkleRootMNList":"5bd9041001ba65e1aea7a8d3982bb7fc2a8a561a1898d4e176a2cc4d242107b0",
          "merkleRootQuorums":"bfe0f35ec169f3b96eb66097138e70d1e52a66a2fc31a057df6298bbbc790fce"
       },
       "time":1614002775,
       "mediantime":1614001062,
       "nonce":43516489,
       "bits":"1b6d4183",
       "difficulty":599.8302783653238,
       "chainwork":"000000000000000000000000000000000000000000000000bb39407cfc6d253a",
       "previousblockhash":"71b81ff345f062e5c6eacbda63f64295590667a8d72428e4e71011675fe531e1",
       "chainlock":true
    });
    let _block: VerboseBlock = json::from_value(json).unwrap();
}

#[test]
fn firo_lelantus_tx() {
    // https://explorer.firo.org/tx/06ed4b75010edcf404a315be70903473f44050c978bc37fbcee90e0b49114ba8
    let tx_hash = "06ed4b75010edcf404a315be70903473f44050c978bc37fbcee90e0b49114ba8".into();
    let electrum = electrum_client_for_test(&[
        "electrumx01.firo.org:50001",
        "electrumx02.firo.org:50001",
        "electrumx03.firo.org:50001",
    ]);
    let _tx = block_on_f01(electrum.get_verbose_transaction(&tx_hash)).unwrap();
}

#[test]
fn firo_lelantus_tx_details() {
    // https://explorer.firo.org/tx/06ed4b75010edcf404a315be70903473f44050c978bc37fbcee90e0b49114ba8
    let electrum = electrum_client_for_test(&[
        "electrumx01.firo.org:50001",
        "electrumx02.firo.org:50001",
        "electrumx03.firo.org:50001",
    ]);
    let coin = utxo_coin_for_test(electrum.into(), None, false);

    let tx_details = get_tx_details_eq_for_both_versions(
        &coin,
        "ad812911f5cba3eab7c193b6cd7020ea02fb5c25634ae64959c3171a6bd5a74d",
    );

    let expected_fee = TxFeeDetails::Utxo(UtxoFeeDetails {
        coin: Some(TEST_COIN_NAME.into()),
        amount: "0.00003793".parse().unwrap(),
    });
    assert_eq!(Some(expected_fee), tx_details.fee_details);

    let tx_details = get_tx_details_eq_for_both_versions(
        &coin,
        "06ed4b75010edcf404a315be70903473f44050c978bc37fbcee90e0b49114ba8",
    );

    let expected_fee = TxFeeDetails::Utxo(UtxoFeeDetails {
        coin: Some(TEST_COIN_NAME.into()),
        amount: "0.00045778".parse().unwrap(),
    });
    assert_eq!(Some(expected_fee), tx_details.fee_details);
}

#[test]
fn firo_spark_tx() {
    // https://explorer.firo.org/tx/c50e5a3f16744ac86bacae28d9251a29bf754d250592bce16a953cd961b584d5
    let tx_hash = "c50e5a3f16744ac86bacae28d9251a29bf754d250592bce16a953cd961b584d5".into();
    let electrum = electrum_client_for_test(&[
        "electrumx01.firo.org:50001",
        "electrumx02.firo.org:50001",
        "electrumx03.firo.org:50001",
    ]);
    let _tx = block_on_f01(electrum.get_verbose_transaction(&tx_hash)).unwrap();
}

#[test]
fn firo_spark_tx_details() {
    // https://explorer.firo.org/tx/c50e5a3f16744ac86bacae28d9251a29bf754d250592bce16a953cd961b584d5
    let electrum = electrum_client_for_test(&[
        "electrumx01.firo.org:50001",
        "electrumx02.firo.org:50001",
        "electrumx03.firo.org:50001",
    ]);
    let coin = utxo_coin_for_test(electrum.into(), None, false);

    let tx_details = get_tx_details_eq_for_both_versions(
        &coin,
        "c50e5a3f16744ac86bacae28d9251a29bf754d250592bce16a953cd961b584d5",
    );

    let expected_fee = TxFeeDetails::Utxo(UtxoFeeDetails {
        coin: Some(TEST_COIN_NAME.into()),
        amount: "0.00003603".parse().unwrap(),
    });
    assert_eq!(Some(expected_fee), tx_details.fee_details);

    let tx_details = get_tx_details_eq_for_both_versions(
        &coin,
        "3b3da29d2ff910ce15e274355b12ff89917fb98a80f746e4a0bbb669ab732250",
    );

    let expected_fee = TxFeeDetails::Utxo(UtxoFeeDetails {
        coin: Some(TEST_COIN_NAME.into()),
        amount: "0.00003603".parse().unwrap(),
    });
    assert_eq!(Some(expected_fee), tx_details.fee_details);
}

#[test]
fn test_generate_tx_doge_fee() {
    // Doge coin does not use fee rounding anymore, so this is not true now: 'a tx below 1kb is always 0,01 doge fee per kb'
    // That is, this test was fixed for lesser txfee.
    // See DINGO coin for the fee rounding.
    let config = json!({
        "coin": "DOGE",
        "name": "dogecoin",
        "fname": "Dogecoin",
        "rpcport": 22555,
        "pubtype": 30,
        "p2shtype": 22,
        "wiftype": 158,
        "txfee": 1000000,
        "force_min_relay_fee": true,
        "mm2": 1,
        "required_confirmations": 2,
        "avg_blocktime": 1,
        "protocol": {
            "type": "UTXO"
        }
    });
    let request = json!({
        "method": "electrum",
        "coin": "DOGE",
        "servers": [{"url": "electrum1.cipig.net:10060"},{"url": "electrum2.cipig.net:10060"},{"url": "electrum3.cipig.net:10060"}],
    });
    let ctx = MmCtxBuilder::default().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&request).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let doge = block_on(utxo_standard_coin_with_priv_key(
        &ctx, "DOGE", &config, &params, priv_key,
    ))
    .unwrap();

    let unspents = vec![UnspentInfo {
        outpoint: Default::default(),
        value: 1000000000000,
        height: None,
        script: Vec::new().into(),
    }];
    let outputs = vec![TransactionOutput {
        value: 100000000,
        script_pubkey: vec![0; 26].into(),
    }];
    let builder = block_on(UtxoTxBuilder::new(&doge))
        .add_available_inputs(unspents)
        .add_outputs(outputs);
    let (_, data) = block_on(builder.build()).unwrap();
    let expected_fee = 227000;
    assert_eq!(expected_fee, data.fee_amount);

    let unspents = vec![UnspentInfo {
        outpoint: Default::default(),
        value: 1000000000000,
        height: None,
        script: Vec::new().into(),
    }];
    let outputs = vec![
        TransactionOutput {
            value: 100000000,
            script_pubkey: vec![0; 26].into(),
        };
        40
    ];

    let builder = block_on(UtxoTxBuilder::new(&doge))
        .add_available_inputs(unspents)
        .add_outputs(outputs);
    let (_, data) = block_on(builder.build()).unwrap();
    let expected_fee = 1592000;
    assert_eq!(expected_fee, data.fee_amount);

    let unspents = vec![UnspentInfo {
        outpoint: Default::default(),
        value: 1000000000000,
        height: None,
        script: Vec::new().into(),
    }];
    let outputs = vec![
        TransactionOutput {
            value: 100000000,
            script_pubkey: vec![0; 26].into(),
        };
        60
    ];

    let builder = block_on(UtxoTxBuilder::new(&doge))
        .add_available_inputs(unspents)
        .add_outputs(outputs);
    let (_, data) = block_on(builder.build()).unwrap();
    let expected_fee = 2292000;
    assert_eq!(expected_fee, data.fee_amount);
}

#[test]
fn doge_mtp() {
    let electrum = electrum_client_for_test(&[
        "electrum1.cipig.net:10060",
        "electrum2.cipig.net:10060",
        "electrum3.cipig.net:10060",
    ]);
    let mtp = block_on_f01(electrum.get_median_time_past(3631820, NonZeroU64::new(11).unwrap(), CoinVariant::Standard))
        .unwrap();
    assert_eq!(mtp, 1614849084);
}

#[test]
fn test_parse_fixed_utxo_txfee_config() {
    let config = json!({
        "coin": "DOGE",
        "name": "dogecoin",
        "fname": "Dogecoin",
        "rpcport": 22555,
        "pubtype": 30,
        "p2shtype": 22,
        "wiftype": 158,
        "txfee": 1000000,
        "force_min_relay_fee": true,
        "mm2": 1,
        "required_confirmations": 2,
        "avg_blocktime": 1,
        "protocol": {
            "type": "UTXO"
        }
    });
    let request = json!({
        "method": "electrum",
        "coin": "DOGE",
        "servers": [{"url": "electrum1.cipig.net:10060"},{"url": "electrum2.cipig.net:10060"},{"url": "electrum3.cipig.net:10060"}],
    });
    let ctx = MmCtxBuilder::default().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&request).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let doge = block_on(utxo_standard_coin_with_priv_key(
        &ctx, "DOGE", &config, &params, priv_key,
    ))
    .unwrap();
    assert!(matches!(doge.as_ref().tx_fee, FeeRate::FixedPerKb(1000000_u64)));
}

#[test]
fn test_parse_fixed_dingo_txfee_config() {
    let config = json!({
        "coin": "DINGO",
        "name": "dingocoin",
        "fname": "Dingocoin",
        "sign_message_prefix": "Dingocoin Signed Message:\n",
        "rpcport": 34646,
        "pubtype": 30,
        "p2shtype": 22,
        "wiftype": 158,
        "txfee": 100000000,
        "dingo_fee": true,
        "force_min_relay_fee": true,
        "dust": 100000000,
        "mm2": 1,
        "required_confirmations": 5,
        "avg_blocktime": 60,
        "protocol": {
          "type": "UTXO"
        },
        "derivation_path": "m/44'/3'",
        "links": {
          "github": "https://github.com/dingocoin/dingocoin",
          "homepage": "https://dingocoin.com"
        }
    });
    let request = json!({
        "method": "electrum",
        "coin": "DINGO",
        "servers": [{"url": "elecx1.dingocoin.com:3342"},{"url": "elecx1.dingocoin.com:3339"},{"url": "elecx2.dingocoin.com:3342"},{"url": "elecx2.dingocoin.com:3339"},{"url": "delecx.twinkykms.com:3342"},{"url": "delecx.twinkykms.com:3339"}],
    });
    let ctx = MmCtxBuilder::default().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&request).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let dingo = block_on(utxo_standard_coin_with_priv_key(
        &ctx, "DINGO", &config, &params, priv_key,
    ))
    .unwrap();
    assert!(matches!(dingo.as_ref().tx_fee, FeeRate::FixedPerKbDingo(100000000_u64)));
}

#[test]
fn firo_mtp() {
    let electrum = electrum_client_for_test(&[
        "electrumx01.firo.org:50001",
        "electrumx02.firo.org:50001",
        "electrumx03.firo.org:50001",
    ]);
    let mtp = block_on_f01(electrum.get_median_time_past(356730, NonZeroU64::new(11).unwrap(), CoinVariant::Standard))
        .unwrap();
    assert_eq!(mtp, 1616492629);
}

#[test]
fn verus_mtp() {
    let electrum = electrum_client_for_test(&["el0.verus.io:17485", "el1.verus.io:17485", "el2.verus.io:17485"]);
    let mtp = block_on_f01(electrum.get_median_time_past(1480113, NonZeroU64::new(11).unwrap(), CoinVariant::Standard))
        .unwrap();
    assert_eq!(mtp, 1618579909);
}

#[test]
fn sys_mtp() {
    let electrum = electrum_client_for_test(&[
        "electrum1.cipig.net:10064",
        "electrum2.cipig.net:10064",
        "electrum3.cipig.net:10064",
    ]);
    let mtp = block_on_f01(electrum.get_median_time_past(1006678, NonZeroU64::new(11).unwrap(), CoinVariant::Standard))
        .unwrap();
    assert_eq!(mtp, 1620019628);
}

#[test]
fn btc_mtp() {
    let electrum = electrum_client_for_test(&[
        "electrum1.cipig.net:10000",
        "electrum2.cipig.net:10000",
        "electrum3.cipig.net:10000",
    ]);
    let mtp = block_on_f01(electrum.get_median_time_past(681659, NonZeroU64::new(11).unwrap(), CoinVariant::Standard))
        .unwrap();
    assert_eq!(mtp, 1620019527);
}

#[test]
fn rvn_mtp() {
    let electrum = electrum_client_for_test(&[
        "electrum1.cipig.net:10051",
        "electrum2.cipig.net:10051",
        "electrum3.cipig.net:10051",
    ]);
    let mtp =
        block_on_f01(electrum.get_median_time_past(1968120, NonZeroU64::new(11).unwrap(), CoinVariant::RVN)).unwrap();
    assert_eq!(mtp, 1633946264);
}

#[test]
fn pivx_mtp() {
    let electrum = electrum_client_for_test(&["electrum01.chainster.org:50001", "electrum02.chainster.org:50001"]);
    let mtp =
        block_on_f01(electrum.get_median_time_past(5014894, NonZeroU64::new(11).unwrap(), CoinVariant::PIVX)).unwrap();
    assert_eq!(mtp, 1754356500);
}

#[test]
fn qtum_mtp() {
    let electrum = electrum_client_for_test(&[
        "electrum1.cipig.net:10050",
        "electrum2.cipig.net:10050",
        "electrum3.cipig.net:10050",
    ]);
    let mtp =
        block_on_f01(electrum.get_median_time_past(681659, NonZeroU64::new(11).unwrap(), CoinVariant::Qtum)).unwrap();
    assert_eq!(mtp, 1598854128);
}

#[test]
fn zer_mtp() {
    let electrum = electrum_client_for_test(&[
        "electrum1.cipig.net:10065",
        "electrum2.cipig.net:10065",
        "electrum3.cipig.net:10065",
    ]);
    let mtp = block_on_f01(electrum.get_median_time_past(1130915, NonZeroU64::new(11).unwrap(), CoinVariant::Standard))
        .unwrap();
    assert_eq!(mtp, 1623240214);
}

#[test]
#[ignore]
fn test_tx_details_kmd_rewards() {
    let electrum = electrum_client_for_test(&[
        "electrum1.cipig.net:10001",
        "electrum2.cipig.net:10001",
        "electrum3.cipig.net:10001",
    ]);
    let mut fields = utxo_coin_fields_for_test(electrum.into(), None, false);
    fields.conf.ticker = "KMD".to_owned();
    fields.derivation_method = DerivationMethod::SingleAddress(
        Address::from_legacyaddress("RMGJ9tRST45RnwEKHPGgBLuY3moSYP7Mhk", &KMD_PREFIXES).unwrap(),
    );
    let coin = utxo_coin_from_fields(fields);

    let tx_details = get_tx_details_eq_for_both_versions(
        &coin,
        "535ffa3387d3fca14f4a4d373daf7edf00e463982755afce89bc8c48d8168024",
    );

    let expected_fee = TxFeeDetails::Utxo(UtxoFeeDetails {
        coin: Some("KMD".into()),
        amount: BigDecimal::from_str("0.00001").unwrap(),
    });
    assert_eq!(tx_details.fee_details, Some(expected_fee));

    let expected_kmd_rewards = KmdRewardsDetails {
        amount: BigDecimal::from_str("0.10431954").unwrap(),
        claimed_by_me: true,
    };
    assert_eq!(tx_details.kmd_rewards, Some(expected_kmd_rewards));
}

/// If the ticker is `KMD` AND no rewards were accrued due to a value less than 10 or for any other reasons,
/// then `TransactionDetails::kmd_rewards` has to be `Some(0)`, not `None`.
/// https://kmdexplorer.io/tx/f09e8894959e74c1e727ffa5a753a30bf2dc6d5d677cc1f24b7ee5bb64e32c7d
#[test]
#[ignore]
#[cfg(not(target_arch = "wasm32"))]
fn test_tx_details_kmd_rewards_claimed_by_other() {
    const TX_HASH: &str = "f09e8894959e74c1e727ffa5a753a30bf2dc6d5d677cc1f24b7ee5bb64e32c7d";

    let electrum = electrum_client_for_test(&[
        "electrum1.cipig.net:10001",
        "electrum2.cipig.net:10001",
        "electrum3.cipig.net:10001",
    ]);
    let mut fields = utxo_coin_fields_for_test(electrum.into(), None, false);
    fields.conf.ticker = "KMD".to_owned();
    fields.derivation_method = DerivationMethod::SingleAddress(
        Address::from_legacyaddress("RMGJ9tRST45RnwEKHPGgBLuY3moSYP7Mhk", &KMD_PREFIXES).unwrap(),
    );
    let coin = utxo_coin_from_fields(fields);

    let tx_details = get_tx_details_eq_for_both_versions(&coin, TX_HASH);

    let expected_fee = TxFeeDetails::Utxo(UtxoFeeDetails {
        coin: Some("KMD".into()),
        amount: BigDecimal::from_str("0.00001").unwrap(),
    });
    assert_eq!(tx_details.fee_details, Some(expected_fee));

    let expected_kmd_rewards = KmdRewardsDetails {
        amount: BigDecimal::from_str("0.00022428").unwrap(),
        claimed_by_me: false,
    };
    assert_eq!(tx_details.kmd_rewards, Some(expected_kmd_rewards));
}

#[test]
fn test_tx_details_bch_no_rewards() {
    const TX_HASH: &str = "eb13d926f15cbb896e0bcc7a1a77a4ec63504e57a1524c13a7a9b80f43ecb05c";

    let electrum = electrum_client_for_test(T_BCH_ELECTRUMS);
    let coin = utxo_coin_for_test(electrum.into(), None, false);

    let tx_details = get_tx_details_eq_for_both_versions(&coin, TX_HASH);
    let expected_fee = TxFeeDetails::Utxo(UtxoFeeDetails {
        coin: Some(TEST_COIN_NAME.into()),
        amount: BigDecimal::from_str("0.00000452").unwrap(),
    });
    assert_eq!(tx_details.fee_details, Some(expected_fee));
    assert_eq!(tx_details.kmd_rewards, None);
}

#[test]
fn test_update_kmd_rewards() {
    // 535ffa3387d3fca14f4a4d373daf7edf00e463982755afce89bc8c48d8168024
    const OUTDATED_TX_DETAILS: &str = r#"{"tx_hex":"0400008085202f8901afcadb73880bc1c9e7ce96b8274c2e2a4547415e649f425f98791685be009b73020000006b483045022100b8fbb77efea482b656ad16fc53c5a01d289054c2e429bf1d7bab16c3e822a83602200b87368a95c046b2ce6d0d092185138a3f234a7eb0d7f8227b196ef32358b93f012103b1e544ce2d860219bc91314b5483421a553a7b33044659eff0be9214ed58adddffffffff01dd15c293000000001976a91483762a373935ca241d557dfce89171d582b486de88ac99fe9960000000000000000000000000000000","tx_hash":"535ffa3387d3fca14f4a4d373daf7edf00e463982755afce89bc8c48d8168024","from":["RMGJ9tRST45RnwEKHPGgBLuY3moSYP7Mhk"],"to":["RMGJ9tRST45RnwEKHPGgBLuY3moSYP7Mhk"],"total_amount":"24.68539379","spent_by_me":"24.68539379","received_by_me":"24.78970333","my_balance_change":"0.10430954","block_height":2387532,"timestamp":1620705483,"fee_details":{"type":"Utxo","amount":"-0.10430954"},"coin":"KMD","internal_id":"535ffa3387d3fca14f4a4d373daf7edf00e463982755afce89bc8c48d8168024"}"#;

    let electrum = electrum_client_for_test(&[
        "electrum1.cipig.net:10001",
        "electrum2.cipig.net:10001",
        "electrum3.cipig.net:10001",
    ]);
    let mut fields = utxo_coin_fields_for_test(electrum.into(), None, false);
    fields.conf.ticker = "KMD".to_owned();
    fields.derivation_method = DerivationMethod::SingleAddress(
        Address::from_legacyaddress("RMGJ9tRST45RnwEKHPGgBLuY3moSYP7Mhk", &KMD_PREFIXES).unwrap(),
    );
    let coin = utxo_coin_from_fields(fields);

    let mut input_transactions = HistoryUtxoTxMap::default();
    let mut tx_details: TransactionDetails = json::from_str(OUTDATED_TX_DETAILS).unwrap();
    block_on(coin.update_kmd_rewards(&mut tx_details, &mut input_transactions)).expect("!update_kmd_rewards");

    let expected_rewards = KmdRewardsDetails {
        amount: BigDecimal::from_str("0.10431954").unwrap(),
        claimed_by_me: true,
    };
    assert_eq!(tx_details.kmd_rewards, Some(expected_rewards));

    let expected_fee_details = TxFeeDetails::Utxo(UtxoFeeDetails {
        coin: Some("KMD".into()),
        amount: BigDecimal::from_str("0.00001").unwrap(),
    });
    assert_eq!(tx_details.fee_details, Some(expected_fee_details));
}

#[test]
fn test_update_kmd_rewards_claimed_not_by_me() {
    // The custom 535ffa3387d3fca14f4a4d373daf7edf00e463982755afce89bc8c48d8168024 transaction with the additional 'from' address.
    const OUTDATED_TX_DETAILS: &str = r#"{"tx_hex":"0400008085202f8901afcadb73880bc1c9e7ce96b8274c2e2a4547415e649f425f98791685be009b73020000006b483045022100b8fbb77efea482b656ad16fc53c5a01d289054c2e429bf1d7bab16c3e822a83602200b87368a95c046b2ce6d0d092185138a3f234a7eb0d7f8227b196ef32358b93f012103b1e544ce2d860219bc91314b5483421a553a7b33044659eff0be9214ed58adddffffffff01dd15c293000000001976a91483762a373935ca241d557dfce89171d582b486de88ac99fe9960000000000000000000000000000000","tx_hash":"535ffa3387d3fca14f4a4d373daf7edf00e463982755afce89bc8c48d8168024","from":["RMGJ9tRST45RnwEKHPGgBLuY3moSYP7Mhk", "RMDc4fvQeekJwrXxuaw1R2b7CTPEuVguMP"],"to":["RMGJ9tRST45RnwEKHPGgBLuY3moSYP7Mhk"],"total_amount":"24.68539379","spent_by_me":"24.68539379","received_by_me":"24.78970333","my_balance_change":"0.10430954","block_height":2387532,"timestamp":1620705483,"fee_details":{"type":"Utxo","amount":"-0.10430954"},"coin":"KMD","internal_id":"535ffa3387d3fca14f4a4d373daf7edf00e463982755afce89bc8c48d8168024"}"#;

    let electrum = electrum_client_for_test(&[
        "electrum1.cipig.net:10001",
        "electrum2.cipig.net:10001",
        "electrum3.cipig.net:10001",
    ]);
    let mut fields = utxo_coin_fields_for_test(electrum.into(), None, false);
    fields.conf.ticker = "KMD".to_owned();
    fields.derivation_method = DerivationMethod::SingleAddress(
        Address::from_legacyaddress("RMGJ9tRST45RnwEKHPGgBLuY3moSYP7Mhk", &KMD_PREFIXES).unwrap(),
    );
    let coin = utxo_coin_from_fields(fields);

    let mut input_transactions = HistoryUtxoTxMap::default();
    let mut tx_details: TransactionDetails = json::from_str(OUTDATED_TX_DETAILS).unwrap();
    block_on(coin.update_kmd_rewards(&mut tx_details, &mut input_transactions)).expect("!update_kmd_rewards");

    let expected_rewards = KmdRewardsDetails {
        amount: BigDecimal::from_str("0.10431954").unwrap(),
        claimed_by_me: false,
    };
    assert_eq!(tx_details.kmd_rewards, Some(expected_rewards));

    let expected_fee_details = TxFeeDetails::Utxo(UtxoFeeDetails {
        coin: Some("KMD".into()),
        amount: BigDecimal::from_str("0.00001").unwrap(),
    });
    assert_eq!(tx_details.fee_details, Some(expected_fee_details));
}

/// https://github.com/KomodoPlatform/atomicDEX-API/issues/966
#[test]
fn test_parse_tx_with_huge_locktime() {
    let verbose = r#"{"hex":"0400008085202f89010c03a2b3d8f97139a623f0759224c657513752b705b5c689a256d52b8f8279f200000000d8483045022100fa07821f4739890fa3518c73ecb4917f4a8e7a1c7a803a0d0aea28f991f14f84022041ac557507d6c9786128828c7b2fca7d5c345ba57c8050e3edb29be0c1e5d2660120bdb3d550a68dfaeebe4c416e5750d20d27617bbfb29756843d605a0570ae787b004c6b63046576ba60b17521039ef1b42c635c32440099910bbe1c5e8b0c9373274c3f21cf1003750fc88d3499ac6782012088a914a4f9f1009dcb778bf1c26052258284b32c9075098821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68ffffffff014ddbf305000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88acf5b98899000000000000000000000000000000","txid":"3b666753b77e28da8a4d858339825315f32516cc147fa743329c7248bd0c6902","overwintered":true,"version":4,"versiongroupid":"892f2085","locktime":2575874549,"expiryheight":0,"vin":[{"txid":"f279828f2bd556a289c6b505b752375157c6249275f023a63971f9d8b3a2030c","vout":0,"scriptSig":{"asm":"3045022100fa07821f4739890fa3518c73ecb4917f4a8e7a1c7a803a0d0aea28f991f14f84022041ac557507d6c9786128828c7b2fca7d5c345ba57c8050e3edb29be0c1e5d266[ALL]bdb3d550a68dfaeebe4c416e5750d20d27617bbfb29756843d605a0570ae787b063046576ba60b17521039ef1b42c635c32440099910bbe1c5e8b0c9373274c3f21cf1003750fc88d3499ac6782012088a914a4f9f1009dcb778bf1c26052258284b32c9075098821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68","hex":"483045022100fa07821f4739890fa3518c73ecb4917f4a8e7a1c7a803a0d0aea28f991f14f84022041ac557507d6c9786128828c7b2fca7d5c345ba57c8050e3edb29be0c1e5d2660120bdb3d550a68dfaeebe4c416e5750d20d27617bbfb29756843d605a0570ae787b004c6b63046576ba60b17521039ef1b42c635c32440099910bbe1c5e8b0c9373274c3f21cf1003750fc88d3499ac6782012088a914a4f9f1009dcb778bf1c26052258284b32c9075098821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68"},"sequence":4294967295}],"vout":[{"value":0.99867469,"valueZat":99867469,"valueSat":99867469,"n":0,"scriptPubKey":{"asm":"OP_DUPOP_HASH160c3f710deb7320b0efa6edb14e3ebeeb9155fa90dOP_EQUALVERIFYOP_CHECKSIG","hex":"76a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac","reqSigs":1,"type":"pubkeyhash","addresses":["t1bjmkBWkzLWk3mHFoybXE5daGRY9pk1fxF"]}}],"vjoinsplit":[],"valueBalance":0.0,"valueBalanceZat":0,"vShieldedSpend":[],"vShieldedOutput":[],"blockhash":"0000077e33e838d9967427018a6e7049d8619ae556acb3e80c070990e90b67fc","height":1127478,"confirmations":2197,"time":1622825622,"blocktime":1622825622}"#;
    let verbose_tx: RpcTransaction = json::from_str(verbose).expect("!json::from_str");
    let _: UtxoTx = deserialize(verbose_tx.hex.as_slice()).unwrap();
}

#[test]
fn tbch_electroncash_verbose_tx() {
    let verbose = r#"{"blockhash":"00000000000d93dbc9c6e95c37044d584be959d24e514533b3a82f0f61dddc03","blocktime":1626262632,"confirmations":3708,"hash":"e64531613f909647651ac3f8fd72f3e6f72ac6e01c5a1d923884a10476f56a7f","height":1456230,"hex":"0100000002ebc10f58f220ec1bad5d634684ae649aa7bdd2f9c9081d36e5384e579caa95c2020000006a4730440220639ac218f572520c7d8addae74be6bfdefa9c86bc91474b6dedd7e117d232085022015a92f45f9ae5cee08c188e01fc614b77c461a41733649a55abfcc3e7ca207444121036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202cffffffffebc10f58f220ec1bad5d634684ae649aa7bdd2f9c9081d36e5384e579caa95c2030000006a47304402204c27a2c04df44f34bd71ec69cc0a24291a96f265217473affb3c3fce2dbd937202202c2ad2e6cfaac3901c807d9b048ccb2b5e7b0dbd922f2066e637f6bbf459313a4121036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202cffffffff040000000000000000406a04534c500001010453454e4420bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb70800000000000003e808000000000000f5fee80300000000000017a9146569d9a853a1934c642223a9432f18c3b3f2a64b87e8030000000000001976a9148cfffc2409d063437d6aa8b75a009b9ba51b71fc88ac67a84601000000001976a9148cfffc2409d063437d6aa8b75a009b9ba51b71fc88ac87caee60","locktime":1626262151,"size":477,"time":1626262632,"txid":"e64531613f909647651ac3f8fd72f3e6f72ac6e01c5a1d923884a10476f56a7f","version":1,"vin":[{"coinbase":null,"scriptSig":{"asm":"OP_PUSHBYTES_71 30440220639ac218f572520c7d8addae74be6bfdefa9c86bc91474b6dedd7e117d232085022015a92f45f9ae5cee08c188e01fc614b77c461a41733649a55abfcc3e7ca2074441 OP_PUSHBYTES_33 036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202c","hex":"4730440220639ac218f572520c7d8addae74be6bfdefa9c86bc91474b6dedd7e117d232085022015a92f45f9ae5cee08c188e01fc614b77c461a41733649a55abfcc3e7ca207444121036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202c"},"sequence":4294967295,"txid":"c295aa9c574e38e5361d08c9f9d2bda79a64ae8446635dad1bec20f2580fc1eb","vout":2},{"coinbase":null,"scriptSig":{"asm":"OP_PUSHBYTES_71 304402204c27a2c04df44f34bd71ec69cc0a24291a96f265217473affb3c3fce2dbd937202202c2ad2e6cfaac3901c807d9b048ccb2b5e7b0dbd922f2066e637f6bbf459313a41 OP_PUSHBYTES_33 036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202c","hex":"47304402204c27a2c04df44f34bd71ec69cc0a24291a96f265217473affb3c3fce2dbd937202202c2ad2e6cfaac3901c807d9b048ccb2b5e7b0dbd922f2066e637f6bbf459313a4121036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202c"},"sequence":4294967295,"txid":"c295aa9c574e38e5361d08c9f9d2bda79a64ae8446635dad1bec20f2580fc1eb","vout":3}],"vout":[{"n":0,"scriptPubKey":{"addresses":[],"asm":"OP_RETURN OP_PUSHBYTES_4 534c5000 OP_PUSHBYTES_1 01 OP_PUSHBYTES_4 53454e44 OP_PUSHBYTES_32 bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb7 OP_PUSHBYTES_8 00000000000003e8 OP_PUSHBYTES_8 000000000000f5fe","hex":"6a04534c500001010453454e4420bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb70800000000000003e808000000000000f5fe","type":"nulldata"},"value_coin":0.0,"value_satoshi":0},{"n":1,"scriptPubKey":{"addresses":["bchtest:ppjknkdg2wsexnryyg36jse0rrpm8u4xfv9hwa0rgl"],"asm":"OP_HASH160 OP_PUSHBYTES_20 6569d9a853a1934c642223a9432f18c3b3f2a64b OP_EQUAL","hex":"a9146569d9a853a1934c642223a9432f18c3b3f2a64b87","type":"scripthash"},"value_coin":0.00001,"value_satoshi":1000},{"n":2,"scriptPubKey":{"addresses":["bchtest:qzx0llpyp8gxxsmad25twksqnwd62xm3lsnnczzt66"],"asm":"OP_DUP OP_HASH160 OP_PUSHBYTES_20 8cfffc2409d063437d6aa8b75a009b9ba51b71fc OP_EQUALVERIFY OP_CHECKSIG","hex":"76a9148cfffc2409d063437d6aa8b75a009b9ba51b71fc88ac","type":"pubkeyhash"},"value_coin":0.00001,"value_satoshi":1000},{"n":3,"scriptPubKey":{"addresses":["bchtest:qzx0llpyp8gxxsmad25twksqnwd62xm3lsnnczzt66"],"asm":"OP_DUP OP_HASH160 OP_PUSHBYTES_20 8cfffc2409d063437d6aa8b75a009b9ba51b71fc OP_EQUALVERIFY OP_CHECKSIG","hex":"76a9148cfffc2409d063437d6aa8b75a009b9ba51b71fc88ac","type":"pubkeyhash"},"value_coin":0.21407847,"value_satoshi":21407847}]}"#;
    let _: RpcTransaction = json::from_str(verbose).expect("!json::from_str");
}

#[test]
fn tbch_electroncash_verbose_tx_unconfirmed() {
    let verbose = r#"{"blockhash":null,"blocktime":null,"confirmations":null,"hash":"e5c9ec5013fca3a62fdf880d1a98f1096a00d20ceaeb6a4cb88ddbea6f1e185a","height":null,"hex":"01000000017f6af57604a18438921d5a1ce0c62af7e6f372fdf8c31a654796903f613145e6030000006b483045022100c335dd0f22e047b806a9d84e02b70aab609093e960888f6f1878e605a173e3da02201c274ce4983d8e519a47c4bd17aeca897b084954ce7a9d77033100e06aa999304121036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202cffffffff0280969800000000001976a914eed5d3ad264ffc68fc0a6454e1696a30d8f405be88acbe0dae00000000001976a9148cfffc2409d063437d6aa8b75a009b9ba51b71fc88ac7a361261","locktime":1628583546,"size":226,"time":null,"txid":"e5c9ec5013fca3a62fdf880d1a98f1096a00d20ceaeb6a4cb88ddbea6f1e185a","version":1,"vin":[{"coinbase":null,"scriptSig":{"asm":"OP_PUSHBYTES_72 3045022100c335dd0f22e047b806a9d84e02b70aab609093e960888f6f1878e605a173e3da02201c274ce4983d8e519a47c4bd17aeca897b084954ce7a9d77033100e06aa9993041 OP_PUSHBYTES_33 036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202c","hex":"483045022100c335dd0f22e047b806a9d84e02b70aab609093e960888f6f1878e605a173e3da02201c274ce4983d8e519a47c4bd17aeca897b084954ce7a9d77033100e06aa999304121036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202c"},"sequence":4294967295,"txid":"e64531613f909647651ac3f8fd72f3e6f72ac6e01c5a1d923884a10476f56a7f","vout":3}],"vout":[{"n":0,"scriptPubKey":{"addresses":["bchtest:qrhdt5adye8lc68upfj9fctfdgcd3aq9hctf8ft6md"],"asm":"OP_DUP OP_HASH160 OP_PUSHBYTES_20 eed5d3ad264ffc68fc0a6454e1696a30d8f405be OP_EQUALVERIFY OP_CHECKSIG","hex":"76a914eed5d3ad264ffc68fc0a6454e1696a30d8f405be88ac","type":"pubkeyhash"},"value_coin":0.1,"value_satoshi":10000000},{"n":1,"scriptPubKey":{"addresses":["bchtest:qzx0llpyp8gxxsmad25twksqnwd62xm3lsnnczzt66"],"asm":"OP_DUP OP_HASH160 OP_PUSHBYTES_20 8cfffc2409d063437d6aa8b75a009b9ba51b71fc OP_EQUALVERIFY OP_CHECKSIG","hex":"76a9148cfffc2409d063437d6aa8b75a009b9ba51b71fc88ac","type":"pubkeyhash"},"value_coin":0.11406782,"value_satoshi":11406782}]}"#;
    let _: RpcTransaction = json::from_str(verbose).expect("!json::from_str");
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_to_p2pk_fails() {
    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    let withdraw_req = WithdrawRequest {
        amount: 1.into(),
        to: "03f8f8fa2062590ba9a0a7a86f937de22f540c015864aad35a2a9f6766de906265".to_string(),
        coin: TEST_COIN_NAME.into(),
        ..Default::default()
    };

    assert!(matches!(
        block_on_f01(coin.withdraw(withdraw_req)).unwrap_err().into_inner(),
        WithdrawError::InvalidAddress(..)
    ))
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_to_p2pkh() {
    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(|coin, _| {
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

    // Create a p2pkh address for the test coin
    let p2pkh_address = AddressBuilder::new(
        UtxoAddressFormat::Standard,
        *block_on(coin.as_ref().derivation_method.unwrap_single_addr()).checksum_type(),
        coin.as_ref().conf.address_prefixes.clone(),
        coin.as_ref().conf.bech32_hrp.clone(),
    )
    .as_pkh(
        block_on(coin.as_ref().derivation_method.unwrap_single_addr())
            .hash()
            .clone(),
    )
    .build()
    .expect("valid address props");

    let withdraw_req = WithdrawRequest {
        amount: 1.into(),
        to: p2pkh_address.to_string(),
        coin: TEST_COIN_NAME.into(),
        ..Default::default()
    };
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();
    let transaction: UtxoTx = deserialize(tx_details.tx.tx_hex().unwrap().as_slice()).unwrap();
    let output_script: Script = transaction.outputs[0].script_pubkey.clone().into();

    let expected_script = Builder::build_p2pkh(p2pkh_address.hash());

    assert_eq!(output_script, expected_script);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_to_p2sh() {
    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(|coin, _| {
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

    // Create a p2sh address for the test coin
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

    let withdraw_req = WithdrawRequest {
        amount: 1.into(),
        to: p2sh_address.to_string(),
        coin: TEST_COIN_NAME.into(),
        ..Default::default()
    };
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();
    let transaction: UtxoTx = deserialize(tx_details.tx.tx_hex().unwrap().as_slice()).unwrap();
    let output_script: Script = transaction.outputs[0].script_pubkey.clone().into();

    let expected_script = Builder::build_p2sh(p2sh_address.hash());

    assert_eq!(output_script, expected_script);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_to_p2wpkh() {
    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, true);

    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(|coin, _| {
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

    // Create a p2wpkh address for the test coin
    let p2wpkh_address = AddressBuilder::new(
        UtxoAddressFormat::Segwit,
        *block_on(coin.as_ref().derivation_method.unwrap_single_addr()).checksum_type(),
        NetworkAddressPrefixes::default(),
        coin.as_ref().conf.bech32_hrp.clone(),
    )
    .as_pkh(
        block_on(coin.as_ref().derivation_method.unwrap_single_addr())
            .hash()
            .clone(),
    )
    .build()
    .expect("valid address props");

    let withdraw_req = WithdrawRequest {
        amount: 1.into(),
        to: p2wpkh_address.to_string(),
        coin: TEST_COIN_NAME.into(),
        ..Default::default()
    };
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();
    let transaction: UtxoTx = deserialize(tx_details.tx.tx_hex().unwrap().as_slice()).unwrap();
    let output_script: Script = transaction.outputs[0].script_pubkey.clone().into();

    let expected_script = Builder::build_p2wpkh(p2wpkh_address.hash()).expect("valid p2wpkh script");

    assert_eq!(output_script, expected_script);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_withdraw_p2pk_balance() {
    let client = NativeClient(Arc::new(NativeClientImpl::default()));

    let coin = utxo_coin_for_test(UtxoRpcClientEnum::Native(client), None, false);

    UtxoStandardCoin::get_unspent_ordered_list.mock_safe(|coin, _| {
        let fut = async move {
            let cache = coin.as_ref().recently_spent_outpoints.lock().await;
            let unspents = vec![UnspentInfo {
                outpoint: OutPoint {
                    hash: 1.into(),
                    index: 0,
                },
                value: 1000000000,
                height: Default::default(),
                // Use a p2pk output script for this UTXO
                script: output_script_p2pk(
                    &coin
                        .as_ref()
                        .derivation_method
                        .unwrap_single_addr()
                        .await
                        .pubkey()
                        .unwrap(),
                ),
            }];
            Ok((unspents, cache))
        };
        MockResult::Return(fut.boxed())
    });

    // Create a dummy p2pkh address to withdraw the coins to.
    let my_p2pkh_address = block_on(coin.as_ref().derivation_method.unwrap_single_addr());

    let withdraw_req = WithdrawRequest {
        amount: 1.into(),
        to: my_p2pkh_address.to_string(),
        coin: TEST_COIN_NAME.into(),
        ..Default::default()
    };
    let tx_details = block_on_f01(coin.withdraw(withdraw_req)).unwrap();
    let transaction: UtxoTx = deserialize(tx_details.tx.tx_hex().unwrap().as_slice()).unwrap();

    // The change should be in a p2pkh script.
    let output_script: Script = transaction.outputs[1].script_pubkey.clone().into();
    let expected_script = Builder::build_p2pkh(my_p2pkh_address.hash());
    assert_eq!(output_script, expected_script);

    // And it should have this value (p2pk balance - amount sent - fees).
    assert_eq!(transaction.outputs[1].value, 899999755);
}

/// `UtxoStandardCoin` has to check UTXO maturity if `check_utxo_maturity` is `true`.
/// https://github.com/KomodoPlatform/atomicDEX-API/issues/1181
#[test]
fn test_utxo_standard_with_check_utxo_maturity_true() {
    /// Whether [`UtxoStandardCoin::get_mature_unspent_ordered_list`] is called or not.
    static GET_MATURE_UNSPENT_ORDERED_LIST_CALLED: AtomicBool = AtomicBool::new(false);

    UtxoStandardCoin::get_mature_unspent_ordered_list.mock_safe(|coin, _| {
        GET_MATURE_UNSPENT_ORDERED_LIST_CALLED.store(true, Ordering::Relaxed);
        let fut = async move {
            let cache = coin.as_ref().recently_spent_outpoints.lock().await;
            Ok((MatureUnspentList::default(), cache))
        };
        MockResult::Return(fut.boxed())
    });

    let conf = json!({"coin":"RICK","asset":"RICK","rpcport":25435,"txversion":4,"overwintered":1,"mm2":1,"protocol":{"type":"UTXO"}});
    let req = json!({
         "method": "electrum",
         "servers": doc_electrums(),
         "check_utxo_maturity": true,
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let coin = block_on(utxo_standard_coin_with_priv_key(&ctx, "RICK", &conf, &params, priv_key)).unwrap();

    let address = Address::from_legacyaddress("R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW", &KMD_PREFIXES).unwrap();
    // Don't use `block_on` here because it's used within a mock of [`GetUtxoListOps::get_mature_unspent_ordered_list`].
    block_on_f01(coin.get_unspent_ordered_list(&address).compat()).unwrap();
    assert!(GET_MATURE_UNSPENT_ORDERED_LIST_CALLED.load(Ordering::Relaxed));
}

/// `UtxoStandardCoin` hasn't to check UTXO maturity if `check_utxo_maturity` is not set.
/// https://github.com/KomodoPlatform/atomicDEX-API/issues/1181
#[test]
fn test_utxo_standard_without_check_utxo_maturity() {
    /// Whether [`UtxoStandardCoin::get_all_unspent_ordered_list`] is called or not.
    static GET_ALL_UNSPENT_ORDERED_LIST_CALLED: AtomicBool = AtomicBool::new(false);

    UtxoStandardCoin::get_all_unspent_ordered_list.mock_safe(|coin, _| {
        GET_ALL_UNSPENT_ORDERED_LIST_CALLED.store(true, Ordering::Relaxed);
        let fut = async move {
            let cache = coin.as_ref().recently_spent_outpoints.lock().await;
            Ok((Vec::new(), cache))
        };
        MockResult::Return(fut.boxed())
    });

    UtxoStandardCoin::get_mature_unspent_ordered_list.mock_safe(|_, _| {
        panic!("'UtxoStandardCoin::get_mature_unspent_ordered_list' is not expected to be called when `check_utxo_maturity` is not set")
    });

    let conf = json!({"coin":"RICK","asset":"RICK","rpcport":25435,"txversion":4,"overwintered":1,"mm2":1,"protocol":{"type":"UTXO"}});
    let req = json!({
         "method": "electrum",
         "servers": doc_electrums()
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let coin = block_on(utxo_standard_coin_with_priv_key(&ctx, "RICK", &conf, &params, priv_key)).unwrap();

    let address = Address::from_legacyaddress("R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW", &KMD_PREFIXES).unwrap();
    // Don't use `block_on` here because it's used within a mock of [`UtxoStandardCoin::get_all_unspent_ordered_list`].
    block_on_f01(coin.get_unspent_ordered_list(&address).compat()).unwrap();
    assert!(GET_ALL_UNSPENT_ORDERED_LIST_CALLED.load(Ordering::Relaxed));
}

/// `QtumCoin` has to check UTXO maturity if `check_utxo_maturity` is not set.
/// https://github.com/KomodoPlatform/atomicDEX-API/issues/1181
#[test]
fn test_qtum_without_check_utxo_maturity() {
    /// Whether [`QtumCoin::get_mature_unspent_ordered_list`] is called or not.
    static GET_MATURE_UNSPENT_ORDERED_LIST_CALLED: AtomicBool = AtomicBool::new(false);

    QtumCoin::get_mature_unspent_ordered_list.mock_safe(|coin, _| {
        GET_MATURE_UNSPENT_ORDERED_LIST_CALLED.store(true, Ordering::Relaxed);
        let fut = async move {
            let cache = coin.as_ref().recently_spent_outpoints.lock().await;
            Ok((MatureUnspentList::default(), cache))
        };
        MockResult::Return(fut.boxed())
    });

    let conf = json!({"coin":"tQTUM","rpcport":13889,"pubtype":120,"p2shtype":110});
    let req = json!({
        "method": "electrum",
        "servers": [
            {"url":"electrum1.cipig.net:10071"},
            {"url":"electrum2.cipig.net:10071"},
            {"url":"electrum3.cipig.net:10071"},
        ],
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let coin = block_on(qtum_coin_with_priv_key(&ctx, "tQTUM", &conf, &params, priv_key)).unwrap();

    let address = Address::from_legacyaddress(
        "qcyBHeSct7Wr4mAw18iuQ1zW5mMFYmtmBE",
        &coin.as_ref().conf.address_prefixes,
    )
    .unwrap();
    // Don't use `block_on` here because it's used within a mock of [`QtumCoin::get_mature_unspent_ordered_list`].
    block_on_f01(coin.get_unspent_ordered_list(&address).compat()).unwrap();
    assert!(GET_MATURE_UNSPENT_ORDERED_LIST_CALLED.load(Ordering::Relaxed));
}

/// The test is for splitting some mature unspent `QTUM` out points into 40 outputs with amount `1 QTUM` in each
#[test]
#[ignore]
fn test_split_qtum() {
    let priv_key = Secp256k1Secret::from([
        3, 98, 177, 3, 108, 39, 234, 144, 131, 178, 103, 103, 127, 80, 230, 166, 53, 68, 147, 215, 42, 216, 144, 72,
        172, 110, 180, 13, 123, 179, 10, 49,
    ]);
    let conf = json!({
      "coin": "tQTUM",
      "name": "qtumtest",
      "fname": "Qtum test",
      "rpcport": 13889,
      "pubtype": 120,
      "p2shtype": 110,
      "wiftype": 239,
      "txfee": 400000,
      "mm2": 1,
      "required_confirmations": 1,
      "mature_confirmations": 2000,
      "avg_blocktime": 0.53,
      "protocol": {
        "type": "QTUM"
      }
    });
    let req = json!({
        "method": "electrum",
        "servers": [
            {"url":"electrum1.cipig.net:10071"},
            {"url":"electrum2.cipig.net:10071"},
            {"url":"electrum3.cipig.net:10071"},
        ],
    });
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(qtum_coin_with_priv_key(&ctx, "tQTUM", &conf, &params, priv_key)).unwrap();
    let p2pkh_address = block_on(coin.as_ref().derivation_method.unwrap_single_addr());
    let script: Script = output_script(&p2pkh_address).expect("valid previous script must be built");
    let key_pair = coin.as_ref().priv_key_policy.activated_key_or_err().unwrap();
    let (unspents, _) = block_on(coin.get_mature_unspent_ordered_list(&p2pkh_address)).expect("Unspent list is empty");
    log!("Mature unspents vec = {:?}", unspents.mature);
    let outputs = vec![
        TransactionOutput {
            value: 100_000_000,
            script_pubkey: script.to_bytes(),
        };
        40
    ];
    let builder = block_on(UtxoTxBuilder::new(&coin))
        .add_available_inputs(unspents.mature)
        .add_outputs(outputs);
    let (unsigned, data) = block_on(builder.build()).unwrap();
    // fee_amount must be higher than the minimum fee
    assert!(data.fee_amount > 400_000);
    log!("Unsigned tx = {:?}", unsigned);
    let signature_version = match p2pkh_address.addr_format() {
        UtxoAddressFormat::Segwit => SignatureVersion::WitnessV0,
        _ => coin.as_ref().conf.signature_version,
    };
    let signed = sign_tx(unsigned, key_pair, signature_version, coin.as_ref().conf.fork_id).unwrap();
    log!("Signed tx = {:?}", signed);
    let res = block_on(coin.broadcast_tx(&signed)).unwrap();
    log!("Res = {:?}", res);
}

/// Test to validate the fix for https://github.com/KomodoPlatform/komodo-defi-framework/issues/2313 produces valid txfee for tx with many inputs
/// (as before the fix)
#[test]
fn test_raven_low_tx_fee_okay() {
    let config = json!({
        "coin": "RVN",
        "name": "raven",
        "fname": "RavenCoin",
        "sign_message_prefix": "Raven Signed Message:\n",
        "rpcport": 8766,
        "pubtype": 60,
        "p2shtype": 122,
        "wiftype": 128,
        "segwit": true,
        "txfee": 1000000,
        "mm2": 1,
        "required_confirmations": 3,
        "avg_blocktime": 60,
        "protocol": {
          "type": "UTXO"
        },
        "derivation_path": "m/44'/175'",
        "trezor_coin": "Ravencoin",
        "links": {
          "github": "https://github.com/RavenProject/Ravencoin",
          "homepage": "https://ravencoin.org"
        }
    });
    let request = json!({
        "method": "electrum",
        "coin": "RVN",
        "servers": [{"url": "electrum1.cipig.net:10060"},{"url": "electrum2.cipig.net:10060"},{"url": "electrum3.cipig.net:10060"}],
    });
    let ctx = MmCtxBuilder::default().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&request).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let raven = block_on(utxo_standard_coin_with_priv_key(
        &ctx, "RVN", &config, &params, priv_key,
    ))
    .unwrap();

    let unspents = vec![
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("be3f13e94d4c58293c2fbee40dd70714c3f833a10ab05b6a328b558bb72c38a7").unwrap(),
                index: 2,
            },
            value: 10618039482,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("2f2eb00dad863079fc20f0c5356bb72e18f3346c126cc3f2e3654360af930f85").unwrap(),
                index: 0,
            },
            value: 15105673480,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("4a806e97f1fa33439d58ce5fad32c5be1e1f1a59d742050a42f237b33f2196ab").unwrap(),
                index: 0,
            },
            value: 15376032861,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("c0f855886343247051bb42b39f75ff35690ad0fb67a08dba5e9f8b680f6fecf3").unwrap(),
                index: 0,
            },
            value: 29999000000,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("0e75a62d6bb49c6312a5a1f3635d4bfc39c3d1549a35dc07b253ec1b1dd3b835").unwrap(),
                index: 0,
            },
            value: 31916552049,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("921554ccd2e50729b521422d3ad22ae00b5721f888e35fca8d2c8ee7a7506490").unwrap(),
                index: 0,
            },
            value: 33542311009,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("9df4256f2e3d0a65745402e7233f309767a2a629755cb3841ff0f47ce90553be").unwrap(),
                index: 0,
            },
            value: 35133858231,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("bf3e69728fa9a41ab06da0e595da63bc0fbe055c04f0e7125c320b3255067a3b").unwrap(),
                index: 0,
            },
            value: 46177879500,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("c62efa3598fec9332746d0657b7bd2a1974efe637da549ddeb84c952535e214b").unwrap(),
                index: 2,
            },
            value: 155455117689,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("9b676bc6a81e4e801a37b48f11f3834c0b1fd49ff420e104563e0895f0517946").unwrap(),
                index: 2,
            },
            value: 251289432230,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("210525a94adc033a745bfae158d931464a720b60bd708d00415fa38d7aa1bed1").unwrap(),
                index: 0,
            },
            value: 260317094896,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("d78d731e8dfc9fc1591da45da7622b13a3e395a73fd3178e6b832cd30436ed14").unwrap(),
                index: 0,
            },
            value: 460964136766,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("02143bce641ef1f70354085dfdff6f1031db019df561aa09b06835fbcf41b8a4").unwrap(),
                index: 0,
            },
            value: 515274184960,
            height: None,
            script: Vec::new().into(),
        },
    ];
    let outputs = vec![
        TransactionOutput {
            value: 1742160278745,
            script_pubkey: "a9147484c59a11d053535314d5a1047005952f7fdf1e87".into(),
        },
        TransactionOutput {
            value: 0,
            script_pubkey: "6a140e7d2af72dc4363283f4b50e1cfe6775a1ad81c1".into(),
        },
        TransactionOutput {
            value: 119006034408,
            script_pubkey: "76a914124b0846223ef78130b8e544b9afc3b09988238688ac".into(),
        },
    ];
    let builder = block_on(UtxoTxBuilder::new(&raven))
        .add_available_inputs(unspents)
        .add_outputs(outputs);
    let (_, data) = block_on(builder.build()).unwrap();
    let expected_fee = 2065000;
    assert_eq!(expected_fee, data.fee_amount);
}

/// Test to validate the fix for https://github.com/KomodoPlatform/komodo-defi-framework/issues/2313 (code before the fix created tx with too low txfee )
#[test]
fn test_raven_low_tx_fee_error() {
    let config = json!({
        "coin": "RVN",
        "name": "raven",
        "fname": "RavenCoin",
        "sign_message_prefix": "Raven Signed Message:\n",
        "rpcport": 8766,
        "pubtype": 60,
        "p2shtype": 122,
        "wiftype": 128,
        "segwit": true,
        "txfee": 1000000,
        "mm2": 1,
        "required_confirmations": 3,
        "avg_blocktime": 60,
        "protocol": {
          "type": "UTXO"
        },
        "derivation_path": "m/44'/175'",
        "trezor_coin": "Ravencoin",
        "links": {
          "github": "https://github.com/RavenProject/Ravencoin",
          "homepage": "https://ravencoin.org"
        }
    });
    let request = json!({
        "method": "electrum",
        "coin": "RVN",
        "servers": [{"url": "electrum1.cipig.net:10060"},{"url": "electrum2.cipig.net:10060"},{"url": "electrum3.cipig.net:10060"}],
    });
    let ctx = MmCtxBuilder::default().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&request).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let raven = block_on(utxo_standard_coin_with_priv_key(
        &ctx, "RVN", &config, &params, priv_key,
    ))
    .unwrap();

    let unspents = vec![
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("fde4ef4f23edc53085460559702783f7128d4b9bacd6898ffae2234576e7feb9").unwrap(),
                index: 2,
            },
            value: 11014394719,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("2f2eb00dad863079fc20f0c5356bb72e18f3346c126cc3f2e3654360af930f85").unwrap(),
                index: 0,
            },
            value: 15105673480,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("4a806e97f1fa33439d58ce5fad32c5be1e1f1a59d742050a42f237b33f2196ab").unwrap(),
                index: 0,
            },
            value: 15376032861,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("c0f855886343247051bb42b39f75ff35690ad0fb67a08dba5e9f8b680f6fecf3").unwrap(),
                index: 0,
            },
            value: 29999000000,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("0e75a62d6bb49c6312a5a1f3635d4bfc39c3d1549a35dc07b253ec1b1dd3b835").unwrap(),
                index: 0,
            },
            value: 31916552049,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("921554ccd2e50729b521422d3ad22ae00b5721f888e35fca8d2c8ee7a7506490").unwrap(),
                index: 0,
            },
            value: 33542311009,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("9df4256f2e3d0a65745402e7233f309767a2a629755cb3841ff0f47ce90553be").unwrap(),
                index: 0,
            },
            value: 35133858231,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("bf3e69728fa9a41ab06da0e595da63bc0fbe055c04f0e7125c320b3255067a3b").unwrap(),
                index: 0,
            },
            value: 46177879500,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("c62efa3598fec9332746d0657b7bd2a1974efe637da549ddeb84c952535e214b").unwrap(),
                index: 2,
            },
            value: 155455117689,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("9b676bc6a81e4e801a37b48f11f3834c0b1fd49ff420e104563e0895f0517946").unwrap(),
                index: 2,
            },
            value: 251289432230,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("210525a94adc033a745bfae158d931464a720b60bd708d00415fa38d7aa1bed1").unwrap(),
                index: 0,
            },
            value: 260317094896,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("d78d731e8dfc9fc1591da45da7622b13a3e395a73fd3178e6b832cd30436ed14").unwrap(),
                index: 0,
            },
            value: 460964136766,
            height: None,
            script: Vec::new().into(),
        },
        UnspentInfo {
            outpoint: OutPoint {
                hash: H256::from_str("02143bce641ef1f70354085dfdff6f1031db019df561aa09b06835fbcf41b8a4").unwrap(),
                index: 0,
            },
            value: 515274184960,
            height: None,
            script: Vec::new().into(),
        },
    ];
    let outputs = vec![
        TransactionOutput {
            value: 1752628943415,
            script_pubkey: "a91417ad3c3cd6e32aede379ac0efa42e310ba30b81d87".into(),
        },
        TransactionOutput {
            value: 0,
            script_pubkey: "6a145786f27ae947255c21e47a3d3fe0d4e132f34e6c".into(),
        },
    ];
    let builder = block_on(UtxoTxBuilder::new(&raven))
        .add_available_inputs(unspents)
        .add_outputs(outputs);
    let (_, data) = block_on(builder.build()).unwrap();
    let expected_fee = 2031000;
    assert_eq!(expected_fee, data.fee_amount);
}

/// `QtumCoin` hasn't to check UTXO maturity if `check_utxo_maturity` is `false`.
/// https://github.com/KomodoPlatform/atomicDEX-API/issues/1181
#[test]
fn test_qtum_with_check_utxo_maturity_false() {
    /// Whether [`QtumCoin::get_all_unspent_ordered_list`] is called or not.
    static GET_ALL_UNSPENT_ORDERED_LIST_CALLED: AtomicBool = AtomicBool::new(false);

    QtumCoin::get_all_unspent_ordered_list.mock_safe(|coin, _address| {
        GET_ALL_UNSPENT_ORDERED_LIST_CALLED.store(true, Ordering::Relaxed);
        let fut = async move {
            let cache = coin.as_ref().recently_spent_outpoints.lock().await;
            let unspents = Vec::new();
            Ok((unspents, cache))
        };
        MockResult::Return(fut.boxed())
    });
    QtumCoin::get_mature_unspent_ordered_list.mock_safe(|_, _| {
        panic!(
            "'QtumCoin::get_mature_unspent_ordered_list' is not expected to be called when `check_utxo_maturity` is false"
        )
    });

    let conf = json!({"coin":"tQTUM","rpcport":13889,"pubtype":120,"p2shtype":110});
    let req = json!({
        "method": "electrum",
        "servers": [
            {"url":"electrum1.cipig.net:10071"},
            {"url":"electrum2.cipig.net:10071"},
            {"url":"electrum3.cipig.net:10071"},
        ],
        "check_utxo_maturity": false,
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let coin = block_on(qtum_coin_with_priv_key(&ctx, "tQTUM", &conf, &params, priv_key)).unwrap();

    let address = Address::from_legacyaddress(
        "qcyBHeSct7Wr4mAw18iuQ1zW5mMFYmtmBE",
        &coin.as_ref().conf.address_prefixes,
    )
    .unwrap();
    // Don't use `block_on` here because it's used within a mock of [`QtumCoin::get_all_unspent_ordered_list`].
    block_on_f01(coin.get_unspent_ordered_list(&address).compat()).unwrap();
    assert!(GET_ALL_UNSPENT_ORDERED_LIST_CALLED.load(Ordering::Relaxed));
}

#[test]
fn test_account_balance_rpc() {
    let mut addresses_map: HashMap<String, u64> = HashMap::new();
    let mut balances_by_der_path: HashMap<String, HDAddressBalance<CoinBalanceMap>> = HashMap::new();

    macro_rules! known_address {
        ($der_path:literal, $address:literal, $chain:expr, balance = $balance:literal) => {
            addresses_map.insert($address.to_string(), $balance);
            balances_by_der_path.insert(
                $der_path.to_string(),
                HDAddressBalance {
                    address: $address.to_string(),
                    derivation_path: RpcDerivationPath(DerivationPath::from_str($der_path).unwrap()),
                    chain: $chain,
                    balance: HashMap::from([(
                        TEST_COIN_NAME.to_string(),
                        CoinBalance::new(BigDecimal::from($balance)),
                    )]),
                },
            )
        };
    }

    macro_rules! get_balances {
        ($($der_paths:literal),*) => {
            [$($der_paths),*].iter().map(|der_path| balances_by_der_path.get(*der_path).unwrap().clone()).collect()
        };
    }

    {
        // Account#0, external addresses.
        known_address!(
            "m/44'/141'/0'/0/0",
            "RRqF4cYniMwYs66S4QDUUZ4GJQFQF69rBE",
            Bip44Chain::External,
            balance = 0
        );
        known_address!(
            "m/44'/141'/0'/0/1",
            "RSVLsjXc9LJ8fm9Jq7gXjeubfja3bbgSDf",
            Bip44Chain::External,
            balance = 0
        );
        known_address!(
            "m/44'/141'/0'/0/2",
            "RSSZjtgfnLzvqF4cZQJJEpN5gvK3pWmd3h",
            Bip44Chain::External,
            balance = 0
        );
        known_address!(
            "m/44'/141'/0'/0/3",
            "RU1gRFXWXNx7uPRAEJ7wdZAW1RZ4TE6Vv1",
            Bip44Chain::External,
            balance = 98
        );
        known_address!(
            "m/44'/141'/0'/0/4",
            "RUkEvRzb7mtwfVeKiSFEbYupLkcvU5KJBw",
            Bip44Chain::External,
            balance = 1
        );
        known_address!(
            "m/44'/141'/0'/0/5",
            "RP8deqVfjBbkvxbGbsQ2EGdamMaP1wxizR",
            Bip44Chain::External,
            balance = 0
        );
        known_address!(
            "m/44'/141'/0'/0/6",
            "RSvKMMegKGP5e2EanH7fnD4yNsxdJvLAmL",
            Bip44Chain::External,
            balance = 32
        );

        // Account#0, internal addresses.
        known_address!(
            "m/44'/141'/0'/1/0",
            "RLZxcZSYtKe74JZd1hBAmmD9PNHZqb72oL",
            Bip44Chain::Internal,
            balance = 13
        );
        known_address!(
            "m/44'/141'/0'/1/1",
            "RPj9JXUVnewWwVpxZDeqGB25qVqz5qJzwP",
            Bip44Chain::Internal,
            balance = 44
        );
        known_address!(
            "m/44'/141'/0'/1/2",
            "RSYdSLRYWuzBson2GDbWBa632q2PmFnCaH",
            Bip44Chain::Internal,
            balance = 10
        );

        // Account#1, internal addresses.
        known_address!(
            "m/44'/141'/1'/1/0",
            "RGo7sYzivPtzv8aRQ4A6vRJDxoqkRRBRhZ",
            Bip44Chain::Internal,
            balance = 0
        );
    }

    NativeClient::display_balances.mock_safe(move |_, addresses: Vec<Address>, _| {
        let result: Vec<_> = addresses
            .into_iter()
            .map(|address| {
                let address_str = address.to_string();
                let balance = addresses_map
                    .remove(&address_str)
                    .unwrap_or_else(|| panic!("Unexpected address: {}", address_str));
                (address, BigDecimal::from(balance))
            })
            .collect();
        MockResult::Return(Box::new(futures01::future::ok(result)))
    });

    let client = NativeClient(Arc::new(NativeClientImpl::default()));
    let mut fields = utxo_coin_fields_for_test(UtxoRpcClientEnum::Native(client), None, false);
    let mut hd_accounts = HDAccountsMap::new();
    hd_accounts.insert(0, UtxoHDAccount {
        account_id: 0,
        extended_pubkey: Secp256k1ExtendedPublicKey::from_str("xpub6DEHSksajpRPM59RPw7Eg6PKdU7E2ehxJWtYdrfQ6JFmMGBsrR6jA78ANCLgzKYm4s5UqQ4ydLEYPbh3TRVvn5oAZVtWfi4qJLMntpZ8uGJ").unwrap(),
        account_derivation_path: HDPathToAccount::from_str("m/44'/141'/0'").unwrap(),
        external_addresses_number: 7,
        internal_addresses_number: 3,
        derived_addresses: HDAddressesCache::default(),
    });
    hd_accounts.insert(1, UtxoHDAccount {
        account_id: 1,
        extended_pubkey: Secp256k1ExtendedPublicKey::from_str("xpub6DEHSksajpRPQq2FdGT6JoieiQZUpTZ3WZn8fcuLJhFVmtCpXbuXxp5aPzaokwcLV2V9LE55Dwt8JYkpuMv7jXKwmyD28WbHYjBH2zhbW2p").unwrap(),
        account_derivation_path: HDPathToAccount::from_str("m/44'/141'/1'").unwrap(),
        external_addresses_number: 0,
        internal_addresses_number: 1,
        derived_addresses: HDAddressesCache::default(),
    });
    fields.derivation_method = DerivationMethod::HDWallet(UtxoHDWallet {
        inner: HDWallet {
            hd_wallet_rmd160: "21605444b36ec72780bdf52a5ffbc18288893664".into(),
            hd_wallet_storage: HDWalletCoinStorage::default(),
            derivation_path: HDPathToCoin::from_str("m/44'/141'").unwrap(),
            accounts: HDAccountsMutex::new(hd_accounts),
            enabled_address: HDPathAccountToAddressId::default(),
            gap_limit: 3,
        },
        address_format: UtxoAddressFormat::Standard,
    });
    let coin = utxo_coin_from_fields(fields);

    // Request a balance of Account#0, external addresses, 1st page

    let params = AccountBalanceParams {
        account_index: 0,
        chain: Bip44Chain::External,
        limit: 3,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(1).unwrap()),
    };
    let actual = block_on(coin.account_balance_rpc(params)).expect("!account_balance_rpc");
    let expected = HDAccountBalanceResponse {
        account_index: 0,
        derivation_path: DerivationPath::from_str("m/44'/141'/0'").unwrap().into(),
        addresses: get_balances!("m/44'/141'/0'/0/0", "m/44'/141'/0'/0/1", "m/44'/141'/0'/0/2"),
        page_balance: HashMap::from([(coin.ticker().to_string(), CoinBalance::new(BigDecimal::from(0)))]),
        limit: 3,
        skipped: 0,
        total: 7,
        total_pages: 3,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(1).unwrap()),
    };
    assert_eq!(actual, expected);

    // Request a balance of Account#0, external addresses, 2nd page

    let params = AccountBalanceParams {
        account_index: 0,
        chain: Bip44Chain::External,
        limit: 3,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(2).unwrap()),
    };
    let actual = block_on(coin.account_balance_rpc(params)).expect("!account_balance_rpc");
    let expected = HDAccountBalanceResponse {
        account_index: 0,
        derivation_path: DerivationPath::from_str("m/44'/141'/0'").unwrap().into(),
        addresses: get_balances!("m/44'/141'/0'/0/3", "m/44'/141'/0'/0/4", "m/44'/141'/0'/0/5"),
        page_balance: HashMap::from([(coin.ticker().to_string(), CoinBalance::new(BigDecimal::from(99)))]),
        limit: 3,
        skipped: 3,
        total: 7,
        total_pages: 3,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(2).unwrap()),
    };
    assert_eq!(actual, expected);

    // Request a balance of Account#0, external addresses, 3rd page

    let params = AccountBalanceParams {
        account_index: 0,
        chain: Bip44Chain::External,
        limit: 3,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(3).unwrap()),
    };
    let actual = block_on(coin.account_balance_rpc(params)).expect("!account_balance_rpc");
    let expected = HDAccountBalanceResponse {
        account_index: 0,
        derivation_path: DerivationPath::from_str("m/44'/141'/0'").unwrap().into(),
        addresses: get_balances!("m/44'/141'/0'/0/6"),
        page_balance: HashMap::from([(coin.ticker().to_string(), CoinBalance::new(BigDecimal::from(32)))]),
        limit: 3,
        skipped: 6,
        total: 7,
        total_pages: 3,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(3).unwrap()),
    };
    assert_eq!(actual, expected);

    // Request a balance of Account#0, external addresses, page 4 (out of bound)

    let params = AccountBalanceParams {
        account_index: 0,
        chain: Bip44Chain::External,
        limit: 3,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(4).unwrap()),
    };
    let actual = block_on(coin.account_balance_rpc(params)).expect("!account_balance_rpc");
    let expected = HDAccountBalanceResponse {
        account_index: 0,
        derivation_path: DerivationPath::from_str("m/44'/141'/0'").unwrap().into(),
        addresses: Vec::new(),
        page_balance: HashMap::default(),
        limit: 3,
        skipped: 7,
        total: 7,
        total_pages: 3,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(4).unwrap()),
    };
    assert_eq!(actual, expected);

    // Request a balance of Account#0, internal addresses, where idx > 0

    let params = AccountBalanceParams {
        account_index: 0,
        chain: Bip44Chain::Internal,
        limit: 3,
        paging_options: PagingOptionsEnum::FromId(0),
    };
    let actual = block_on(coin.account_balance_rpc(params)).expect("!account_balance_rpc");
    let expected = HDAccountBalanceResponse {
        account_index: 0,
        derivation_path: DerivationPath::from_str("m/44'/141'/0'").unwrap().into(),
        addresses: get_balances!("m/44'/141'/0'/1/1", "m/44'/141'/0'/1/2"),
        page_balance: HashMap::from([(coin.ticker().to_string(), CoinBalance::new(BigDecimal::from(54)))]),
        limit: 3,
        skipped: 1,
        total: 3,
        total_pages: 1,
        paging_options: PagingOptionsEnum::FromId(0),
    };
    assert_eq!(actual, expected);

    // Request a balance of Account#1, external addresses, page 1 (out of bound)

    let params = AccountBalanceParams {
        account_index: 1,
        chain: Bip44Chain::External,
        limit: 3,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(1).unwrap()),
    };
    let actual = block_on(coin.account_balance_rpc(params)).expect("!account_balance_rpc");
    let expected = HDAccountBalanceResponse {
        account_index: 1,
        derivation_path: DerivationPath::from_str("m/44'/141'/1'").unwrap().into(),
        addresses: Vec::new(),
        page_balance: HashMap::default(),
        limit: 3,
        skipped: 0,
        total: 0,
        total_pages: 0,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(1).unwrap()),
    };
    assert_eq!(actual, expected);

    // Request a balance of Account#1, external addresses, page 1

    let params = AccountBalanceParams {
        account_index: 1,
        chain: Bip44Chain::Internal,
        limit: 3,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(1).unwrap()),
    };
    let actual = block_on(coin.account_balance_rpc(params)).expect("!account_balance_rpc");
    let expected = HDAccountBalanceResponse {
        account_index: 1,
        derivation_path: DerivationPath::from_str("m/44'/141'/1'").unwrap().into(),
        addresses: get_balances!("m/44'/141'/1'/1/0"),
        page_balance: HashMap::from([(coin.ticker().to_string(), CoinBalance::new(BigDecimal::from(0)))]),
        limit: 3,
        skipped: 0,
        total: 1,
        total_pages: 1,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(1).unwrap()),
    };
    assert_eq!(actual, expected);

    // Request a balance of Account#1, external addresses, where idx > 0 (out of bound)

    let params = AccountBalanceParams {
        account_index: 1,
        chain: Bip44Chain::Internal,
        limit: 3,
        paging_options: PagingOptionsEnum::FromId(0),
    };
    let actual = block_on(coin.account_balance_rpc(params)).expect("!account_balance_rpc");
    let expected = HDAccountBalanceResponse {
        account_index: 1,
        derivation_path: DerivationPath::from_str("m/44'/141'/1'").unwrap().into(),
        addresses: Vec::new(),
        page_balance: HashMap::default(),
        limit: 3,
        skipped: 1,
        total: 1,
        total_pages: 1,
        paging_options: PagingOptionsEnum::FromId(0),
    };
    assert_eq!(actual, expected);
}

#[test]
fn test_scan_for_new_addresses() {
    static ACCOUNT_ID: AtomicU32 = AtomicU32::new(0);
    static NEW_EXTERNAL_ADDRESSES_NUMBER: AtomicU32 = AtomicU32::new(0);
    static NEW_INTERNAL_ADDRESSES_NUMBER: AtomicU32 = AtomicU32::new(0);

    HDWalletMockStorage::update_external_addresses_number.mock_safe(
        |_, _, account_id, new_external_addresses_number| {
            assert_eq!(account_id, ACCOUNT_ID.load(Ordering::Relaxed));
            assert_eq!(
                new_external_addresses_number,
                NEW_EXTERNAL_ADDRESSES_NUMBER.load(Ordering::Relaxed)
            );
            MockResult::Return(Box::pin(futures::future::ok(())))
        },
    );

    HDWalletMockStorage::update_internal_addresses_number.mock_safe(
        |_, _, account_id, new_internal_addresses_number| {
            assert_eq!(account_id, ACCOUNT_ID.load(Ordering::Relaxed));
            assert_eq!(
                new_internal_addresses_number,
                NEW_INTERNAL_ADDRESSES_NUMBER.load(Ordering::Relaxed)
            );
            MockResult::Return(Box::pin(futures::future::ok(())))
        },
    );

    // The list of addresses that were checked using [`UtxoAddressScanner::is_address_used`].
    static CHECKED_ADDRESSES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    // The map of addresses for those [`NativeClient::display_balance`] called.
    let mut display_balances: HashMap<String, u64> = HashMap::new();
    // The expected list of the addresses that were checked using [`UtxoAddressScanner::is_address_used`].
    let mut expected_checked_addresses: Vec<String> = Vec::new();
    // The list of addresses with a non-empty transaction history.
    let mut non_empty_addresses: HashSet<String> = HashSet::new();
    // The map of results by the addresses.
    let mut balances_by_der_path: HashMap<String, HDAddressBalance<CoinBalanceMap>> = HashMap::new();

    macro_rules! new_address {
        ($der_path:literal, $address:literal, $chain:expr, balance = $balance:expr) => {{
            if let Some(balance) = $balance {
                display_balances.insert($address.to_string(), balance);
                non_empty_addresses.insert($address.to_string());
            }
            expected_checked_addresses.push($address.to_string());
            balances_by_der_path.insert(
                $der_path.to_string(),
                HDAddressBalance {
                    address: $address.to_string(),
                    derivation_path: RpcDerivationPath(DerivationPath::from_str($der_path).unwrap()),
                    chain: $chain,
                    balance: $balance.map_or_else(HashMap::default, |balance| {
                        HashMap::from([(TEST_COIN_NAME.to_string(), CoinBalance::new(BigDecimal::from(balance)))])
                    }),
                },
            );
        }};
    }

    macro_rules! unused_address {
        ($_der_path:literal, $address:literal) => {{
            let address = $address.to_string();
            expected_checked_addresses.push(address);
        }};
    }

    macro_rules! get_balances {
        ($($der_paths:literal),*) => {
            [$($der_paths),*].iter().map(|der_path| balances_by_der_path.get(*der_path).unwrap().clone()).collect()
        };
    }

    // Please note that the order of the `known` and `new` addresses is important.
    {
        // Account#0, external addresses.
        new_address!(
            "m/44'/141'/0'/0/3",
            "RU1gRFXWXNx7uPRAEJ7wdZAW1RZ4TE6Vv1",
            Bip44Chain::External,
            balance = Some(98)
        );
        unused_address!("m/44'/141'/0'/0/4", "RUkEvRzb7mtwfVeKiSFEbYupLkcvU5KJBw");
        unused_address!("m/44'/141'/0'/0/5", "RP8deqVfjBbkvxbGbsQ2EGdamMaP1wxizR");
        unused_address!("m/44'/141'/0'/0/6", "RSvKMMegKGP5e2EanH7fnD4yNsxdJvLAmL");
        unused_address!("m/44'/141'/0'/0/7", "RX76e9G7H4Xy6cYrtr1qGghxytAmWpv375"); // Stop searching for a non-empty address (gap_limit = 3).

        // Account#0, internal addresses.
        new_address!(
            "m/44'/141'/0'/1/1",
            "RPj9JXUVnewWwVpxZDeqGB25qVqz5qJzwP",
            Bip44Chain::Internal,
            balance = Some(98)
        );
        new_address!(
            "m/44'/141'/0'/1/2",
            "RSYdSLRYWuzBson2GDbWBa632q2PmFnCaH",
            Bip44Chain::Internal,
            balance = None::<u64>
        );
        new_address!(
            "m/44'/141'/0'/1/3",
            "RQstQeTUEZLh6c3YWJDkeVTTQoZUsfvNCr",
            Bip44Chain::Internal,
            balance = Some(14)
        );
        unused_address!("m/44'/141'/0'/1/4", "RT54m6pfj9scqwSLmYdfbmPcrpxnWGAe9J");
        unused_address!("m/44'/141'/0'/1/5", "RYWfEFxqA6zya9c891Dj7vxiDojCmuWR9T");
        unused_address!("m/44'/141'/0'/1/6", "RSkY6twW8knTcn6wGACUAG9crJHcuQ2kEH");
        unused_address!("m/44'/141'/0'/1/7", "RGRybU5awT9Chn9FeKZd8CEBREq5vNFDKJ"); // Stop searching for a non-empty address (gap_limit = 3).

        // Account#1, external addresses.
        new_address!(
            "m/44'/141'/1'/0/0",
            "RBQFLwJ88gVcnfkYvJETeTAB6AAYLow12K",
            Bip44Chain::External,
            balance = Some(9)
        );
        new_address!(
            "m/44'/141'/1'/0/1",
            "RCyy77sRWFa2oiFPpyimeTQfenM1aRoiZs",
            Bip44Chain::External,
            balance = Some(7)
        );
        new_address!(
            "m/44'/141'/1'/0/2",
            "RDnNa3pQmisfi42KiTZrfYfuxkLC91PoTJ",
            Bip44Chain::External,
            balance = None::<u64>
        );
        new_address!(
            "m/44'/141'/1'/0/3",
            "RQRGgXcGJz93CoAfQJoLgBz2r9HtJYMX3Z",
            Bip44Chain::External,
            balance = None::<u64>
        );
        new_address!(
            "m/44'/141'/1'/0/4",
            "RM6cqSFCFZ4J1LngLzqKkwo2ouipbDZUbm",
            Bip44Chain::External,
            balance = Some(11)
        );
        unused_address!("m/44'/141'/1'/0/5", "RX2fGBZjNZMNdNcnc5QBRXvmsXTvadvTPN");
        unused_address!("m/44'/141'/1'/0/6", "RJJ7muUETyp59vxVXna9KAZ9uQ1QSqmcjE");
        unused_address!("m/44'/141'/1'/0/7", "RYJ6vbhxFre5yChCMiJJFNTTBhAQbKM9AY");
        unused_address!("m/44'/141'/1'/0/8", "RWaND65Cucwc2Cs1djBUQ2z1rrxTopEjoG"); // Stop searching for a non-empty address (gap_limit = 3).

        // Account#1, internal addresses.
        unused_address!("m/44'/141'/1'/0/2", "RCjRDibDAXKYpVYSUeJXrbTzZ1UEKYAwJa");
        unused_address!("m/44'/141'/1'/0/3", "REs1NRzg8XjwN3v8Jp1wQUAyQb3TzeT8EB");
        unused_address!("m/44'/141'/1'/0/4", "RS4UZtkwZ8eYaTL1xodXgFNryJoTbPJYE5");
        unused_address!("m/44'/141'/1'/0/5", "RDzcAqivNqUCJA4auetoVE4hcmH2p4L1fB");
        // Stop searching for a non-empty address (gap_limit = 3).
    }

    NativeClient::display_balance.mock_safe(move |_, address: Address, _| {
        let address = address.to_string();
        let balance = display_balances
            .remove(&address)
            .unwrap_or_else(|| panic!("Unexpected address: {}", address));
        MockResult::Return(Box::new(futures01::future::ok(BigDecimal::from(balance))))
    });

    UtxoAddressScanner::is_address_used.mock_safe(move |_, address| {
        let address = address.to_string();
        CHECKED_ADDRESSES.lock().unwrap().push(address.clone());
        let is_used = non_empty_addresses.remove(&address);
        MockResult::Return(Box::pin(futures::future::ok(is_used)))
    });

    // This mock is required just not to fail on [`UtxoAddressScanner::init`].
    NativeClient::list_all_transactions
        .mock_safe(move |_, _| MockResult::Return(Box::new(futures01::future::ok(Vec::new()))));

    let client = NativeClient(Arc::new(NativeClientImpl::default()));
    let mut fields = utxo_coin_fields_for_test(UtxoRpcClientEnum::Native(client), None, false);
    let mut hd_accounts = HDAccountsMap::new();
    hd_accounts.insert(0, UtxoHDAccount {
        account_id: 0,
        extended_pubkey: Secp256k1ExtendedPublicKey::from_str("xpub6DEHSksajpRPM59RPw7Eg6PKdU7E2ehxJWtYdrfQ6JFmMGBsrR6jA78ANCLgzKYm4s5UqQ4ydLEYPbh3TRVvn5oAZVtWfi4qJLMntpZ8uGJ").unwrap(),
        account_derivation_path: HDPathToAccount::from_str("m/44'/141'/0'").unwrap(),
        external_addresses_number: 3,
        internal_addresses_number: 1,
        derived_addresses: HDAddressesCache::default(),
    });
    hd_accounts.insert(1, UtxoHDAccount {
        account_id: 1,
        extended_pubkey: Secp256k1ExtendedPublicKey::from_str("xpub6DEHSksajpRPQq2FdGT6JoieiQZUpTZ3WZn8fcuLJhFVmtCpXbuXxp5aPzaokwcLV2V9LE55Dwt8JYkpuMv7jXKwmyD28WbHYjBH2zhbW2p").unwrap(),
        account_derivation_path: HDPathToAccount::from_str("m/44'/141'/1'").unwrap(),
        external_addresses_number: 0,
        internal_addresses_number: 2,
        derived_addresses: HDAddressesCache::default(),
    });
    fields.derivation_method = DerivationMethod::HDWallet(UtxoHDWallet {
        inner: HDWallet {
            hd_wallet_rmd160: "21605444b36ec72780bdf52a5ffbc18288893664".into(),
            hd_wallet_storage: HDWalletCoinStorage::default(),
            derivation_path: HDPathToCoin::from_str("m/44'/141'").unwrap(),
            accounts: HDAccountsMutex::new(hd_accounts),
            enabled_address: HDPathAccountToAddressId::default(),
            gap_limit: 3,
        },
        address_format: UtxoAddressFormat::Standard,
    });
    let coin = utxo_coin_from_fields(fields);

    // Check balance of Account#0

    ACCOUNT_ID.store(0, Ordering::Relaxed);
    NEW_EXTERNAL_ADDRESSES_NUMBER.store(4, Ordering::Relaxed);
    NEW_INTERNAL_ADDRESSES_NUMBER.store(4, Ordering::Relaxed);

    let params = ScanAddressesParams {
        account_index: 0,
        gap_limit: Some(3),
    };
    let actual = block_on(coin.init_scan_for_new_addresses_rpc(params)).expect("!account_balance_rpc");
    let expected = ScanAddressesResponse {
        account_index: 0,
        derivation_path: DerivationPath::from_str("m/44'/141'/0'").unwrap().into(),
        new_addresses: get_balances!(
            "m/44'/141'/0'/0/3",
            "m/44'/141'/0'/1/1",
            "m/44'/141'/0'/1/2",
            "m/44'/141'/0'/1/3"
        ),
    };
    assert_eq!(actual, expected);

    // Check balance of Account#1

    ACCOUNT_ID.store(1, Ordering::Relaxed);
    NEW_EXTERNAL_ADDRESSES_NUMBER.store(5, Ordering::Relaxed);
    NEW_INTERNAL_ADDRESSES_NUMBER.store(2, Ordering::Relaxed);

    let params = ScanAddressesParams {
        account_index: 1,
        gap_limit: None,
    };
    let actual = block_on(coin.init_scan_for_new_addresses_rpc(params)).expect("!account_balance_rpc");
    let expected = ScanAddressesResponse {
        account_index: 1,
        derivation_path: DerivationPath::from_str("m/44'/141'/1'").unwrap().into(),
        new_addresses: get_balances!(
            "m/44'/141'/1'/0/0",
            "m/44'/141'/1'/0/1",
            "m/44'/141'/1'/0/2",
            "m/44'/141'/1'/0/3",
            "m/44'/141'/1'/0/4"
        ),
    };
    assert_eq!(actual, expected);

    let accounts = match coin.as_ref().derivation_method {
        DerivationMethod::HDWallet(UtxoHDWallet { ref inner, .. }) => block_on(inner.accounts.lock()).clone(),
        _ => unreachable!(),
    };
    assert_eq!(accounts[&0].external_addresses_number, 4);
    assert_eq!(accounts[&0].internal_addresses_number, 4);
    assert_eq!(accounts[&1].external_addresses_number, 5);
    assert_eq!(accounts[&1].internal_addresses_number, 2);
    assert_eq!(*CHECKED_ADDRESSES.lock().unwrap(), expected_checked_addresses);
}

#[test]
fn test_get_new_address() {
    static EXPECTED_CHECKED_ADDRESSES: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static CHECKED_ADDRESSES: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static NON_EMPTY_ADDRESSES: Mutex<Option<HashSet<String>>> = Mutex::new(None);

    macro_rules! expected_checked_addresses {
        ($($_der_path:literal, $addr:literal);*) => {
            CHECKED_ADDRESSES.lock().unwrap().clear();
            *EXPECTED_CHECKED_ADDRESSES.lock().unwrap() = vec![$($addr.to_string()),*];
        };
    }

    macro_rules! non_empty_addresses {
        ($($_der_path:literal, $addr:literal);*) => {
                *NON_EMPTY_ADDRESSES.lock().unwrap() = Some(vec![$($addr.to_string()),*].into_iter().collect());
        };
    }

    HDWalletMockStorage::update_external_addresses_number
        .mock_safe(|_, _, _account_id, _new_val| MockResult::Return(Box::pin(futures::future::ok(()))));
    HDWalletMockStorage::update_internal_addresses_number
        .mock_safe(|_, _, _account_id, _new_val| MockResult::Return(Box::pin(futures::future::ok(()))));

    // This mock is required just not to fail on [`UtxoStandardCoin::known_address_balance`].
    NativeClient::display_balance
        .mock_safe(move |_, _, _| MockResult::Return(Box::new(futures01::future::ok(BigDecimal::from(0)))));

    UtxoAddressScanner::is_address_used.mock_safe(move |_, address| {
        let address = address.to_string();
        CHECKED_ADDRESSES.lock().unwrap().push(address.clone());
        let is_used = NON_EMPTY_ADDRESSES.lock().unwrap().as_mut().unwrap().remove(&address);
        MockResult::Return(Box::pin(futures::future::ok(is_used)))
    });

    MockableConfirmAddress::confirm_address
        .mock_safe(move |_, _, _, _| MockResult::Return(Box::pin(futures::future::ok(()))));

    // This mock is required just not to fail on [`UtxoAddressScanner::init`].
    NativeClient::list_all_transactions
        .mock_safe(move |_, _| MockResult::Return(Box::new(futures01::future::ok(Vec::new()))));

    let client = NativeClient(Arc::new(NativeClientImpl::default()));
    let mut fields = utxo_coin_fields_for_test(UtxoRpcClientEnum::Native(client), None, false);
    let mut hd_accounts = HDAccountsMap::new();
    let hd_account_for_test = UtxoHDAccount {
        account_id: 0,
        extended_pubkey: Secp256k1ExtendedPublicKey::from_str("xpub6DEHSksajpRPM59RPw7Eg6PKdU7E2ehxJWtYdrfQ6JFmMGBsrR6jA78ANCLgzKYm4s5UqQ4ydLEYPbh3TRVvn5oAZVtWfi4qJLMntpZ8uGJ").unwrap(),
        account_derivation_path: HDPathToAccount::from_str("m/44'/141'/0'").unwrap(),
        external_addresses_number: 4,
        internal_addresses_number: 0,
        derived_addresses: HDAddressesCache::default(),
    };
    // Put multiple the same accounts for tests,
    // since every successful `get_new_address_rpc` changes the state of the account.
    hd_accounts.insert(0, hd_account_for_test.clone());
    hd_accounts.insert(1, hd_account_for_test.clone());
    hd_accounts.insert(2, hd_account_for_test);

    fields.derivation_method = DerivationMethod::HDWallet(UtxoHDWallet {
        inner: HDWallet {
            hd_wallet_rmd160: "21605444b36ec72780bdf52a5ffbc18288893664".into(),
            hd_wallet_storage: HDWalletCoinStorage::default(),
            derivation_path: HDPathToCoin::from_str("m/44'/141'").unwrap(),
            accounts: HDAccountsMutex::new(hd_accounts),
            enabled_address: HDPathAccountToAddressId::default(),
            gap_limit: 2,
        },
        address_format: UtxoAddressFormat::Standard,
    });
    fields.conf.trezor_coin = Some("Komodo".to_string());
    let coin = utxo_coin_from_fields(fields);

    // =======

    let confirm_address = MockableConfirmAddress;

    expected_checked_addresses!["m/44'/141'/0'/0/3", "RU1gRFXWXNx7uPRAEJ7wdZAW1RZ4TE6Vv1"];
    non_empty_addresses!["m/44'/141'/0'/0/3", "RU1gRFXWXNx7uPRAEJ7wdZAW1RZ4TE6Vv1"];
    let params = GetNewAddressParams {
        account_id: 0,
        chain: Some(Bip44Chain::External),
        gap_limit: None, // Will be used 2 from `UtxoHDWallet` by default.
    };
    block_on(coin.get_new_address_rpc(params, &confirm_address)).unwrap();
    assert_eq!(
        *CHECKED_ADDRESSES.lock().unwrap(),
        *EXPECTED_CHECKED_ADDRESSES.lock().unwrap()
    );

    // `m/44'/141'/1'/0/3` is empty, so `m/44'/141'/1'/0/2` will be checked.

    expected_checked_addresses!["m/44'/141'/1'/0/3", "RU1gRFXWXNx7uPRAEJ7wdZAW1RZ4TE6Vv1"];
    non_empty_addresses!["m/44'/141'/1'/0/2", "RSSZjtgfnLzvqF4cZQJJEpN5gvK3pWmd3h"];
    let params = GetNewAddressParams {
        account_id: 1,
        chain: Some(Bip44Chain::External),
        gap_limit: Some(1),
    };
    let err = block_on(coin.get_new_address_rpc(params, &confirm_address))
        .expect_err("get_new_address_rpc should have failed with 'EmptyAddressesLimitReached' error");
    let expected = GetNewAddressRpcError::EmptyAddressesLimitReached { gap_limit: 1 };
    assert_eq!(err.into_inner(), expected);
    assert_eq!(
        *CHECKED_ADDRESSES.lock().unwrap(),
        *EXPECTED_CHECKED_ADDRESSES.lock().unwrap()
    );

    // `m/44'/141'/1'/0/3` is empty, but `m/44'/141'/1'/0/2` is not.

    expected_checked_addresses![
        "m/44'/141'/1'/0/3", "RU1gRFXWXNx7uPRAEJ7wdZAW1RZ4TE6Vv1";
        "m/44'/141'/1'/0/2", "RSSZjtgfnLzvqF4cZQJJEpN5gvK3pWmd3h"
    ];
    non_empty_addresses!["m/44'/141'/1'/0/2", "RSSZjtgfnLzvqF4cZQJJEpN5gvK3pWmd3h"];
    let params = GetNewAddressParams {
        account_id: 1,
        chain: Some(Bip44Chain::External),
        gap_limit: Some(2),
    };
    block_on(coin.get_new_address_rpc(params, &confirm_address)).unwrap();
    assert_eq!(
        *CHECKED_ADDRESSES.lock().unwrap(),
        *EXPECTED_CHECKED_ADDRESSES.lock().unwrap()
    );

    // `m/44'/141'/2'/0/3` and `m/44'/141'/2'/0/2` are empty.

    expected_checked_addresses![
        "m/44'/141'/2'/0/3", "RU1gRFXWXNx7uPRAEJ7wdZAW1RZ4TE6Vv1";
        "m/44'/141'/2'/0/2", "RSSZjtgfnLzvqF4cZQJJEpN5gvK3pWmd3h"
    ];
    non_empty_addresses![];
    let params = GetNewAddressParams {
        account_id: 2,
        chain: Some(Bip44Chain::External),
        gap_limit: Some(2),
    };
    let err = block_on(coin.get_new_address_rpc(params, &confirm_address))
        .expect_err("get_new_address_rpc should have failed with 'EmptyAddressesLimitReached' error");
    let expected = GetNewAddressRpcError::EmptyAddressesLimitReached { gap_limit: 2 };
    assert_eq!(err.into_inner(), expected);
    assert_eq!(
        *CHECKED_ADDRESSES.lock().unwrap(),
        *EXPECTED_CHECKED_ADDRESSES.lock().unwrap()
    );

    // `gap_limit=0` means don't allow to generate new address if the last one is empty yet.

    expected_checked_addresses!["m/44'/141'/2'/0/3", "RU1gRFXWXNx7uPRAEJ7wdZAW1RZ4TE6Vv1"];
    non_empty_addresses![];
    let params = GetNewAddressParams {
        account_id: 2,
        chain: Some(Bip44Chain::External),
        gap_limit: Some(0),
    };
    let err = block_on(coin.get_new_address_rpc(params, &confirm_address))
        .expect_err("!get_new_address_rpc should have failed with 'EmptyAddressesLimitReached' error");
    let expected = GetNewAddressRpcError::EmptyAddressesLimitReached { gap_limit: 0 };
    assert_eq!(err.into_inner(), expected);
    assert_eq!(
        *CHECKED_ADDRESSES.lock().unwrap(),
        *EXPECTED_CHECKED_ADDRESSES.lock().unwrap()
    );

    // `(gap_limit=5) > (known_addresses_number=4)`, there should not be any network request.

    expected_checked_addresses![];
    non_empty_addresses![];
    let params = GetNewAddressParams {
        account_id: 2,
        chain: Some(Bip44Chain::External),
        gap_limit: Some(5),
    };
    block_on(coin.get_new_address_rpc(params, &confirm_address)).unwrap();
    assert_eq!(
        *CHECKED_ADDRESSES.lock().unwrap(),
        *EXPECTED_CHECKED_ADDRESSES.lock().unwrap()
    );

    // `known_addresses_number=0`, always allow.

    expected_checked_addresses![];
    non_empty_addresses![];
    let params = GetNewAddressParams {
        account_id: 0,
        chain: Some(Bip44Chain::Internal),
        gap_limit: Some(0),
    };
    block_on(coin.get_new_address_rpc(params, &confirm_address)).unwrap();
    assert_eq!(
        *CHECKED_ADDRESSES.lock().unwrap(),
        *EXPECTED_CHECKED_ADDRESSES.lock().unwrap()
    );

    // Check if `get_new_address_rpc` fails on the `HDAddressConfirm::confirm_address` error.

    MockableConfirmAddress::confirm_address.mock_safe(move |_, _, _, _| {
        MockResult::Return(Box::pin(futures::future::ready(MmError::err(
            HDConfirmAddressError::HwContextNotInitialized,
        ))))
    });

    expected_checked_addresses![];
    non_empty_addresses![];
    let params = GetNewAddressParams {
        account_id: 0,
        chain: Some(Bip44Chain::Internal),
        gap_limit: Some(2),
    };
    let err = block_on(coin.get_new_address_rpc(params, &confirm_address))
        .expect_err("!get_new_address_rpc should have failed with 'HwContextNotInitialized' error");
    assert_eq!(err.into_inner(), GetNewAddressRpcError::HwContextNotInitialized);
}

/// https://github.com/KomodoPlatform/atomicDEX-API/issues/1196
#[test]
fn test_electrum_balance_deserializing() {
    let serialized = r#"{"confirmed": 988937858554305, "unconfirmed": 18446720562229577551}"#;
    let actual: ElectrumBalance = json::from_str(serialized).unwrap();
    assert_eq!(actual.confirmed, 988937858554305i128);
    assert_eq!(actual.unconfirmed, 18446720562229577551i128);

    let serialized = r#"{"confirmed": -170141183460469231731687303715884105728, "unconfirmed": 170141183460469231731687303715884105727}"#;
    let actual: ElectrumBalance = json::from_str(serialized).unwrap();
    assert_eq!(actual.confirmed, i128::MIN);
    assert_eq!(actual.unconfirmed, i128::MAX);
}

#[test]
fn test_electrum_display_balances() {
    let rpc_client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    block_on(utxo_common_tests::test_electrum_display_balances(&rpc_client));
}

#[test]
fn test_for_non_existent_tx_hex_utxo_electrum() {
    // This test shouldn't wait till timeout!
    let timeout = wait_until_sec(120);
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(
        client.into(),
        Some("spice describe gravity federal blast come thank unfair canal monkey style afraid"),
        false,
    );
    // bad transaction hex
    let tx = hex::decode("0400008085202f8902bf17bf7d1daace52e08f732a6b8771743ca4b1cb765a187e72fd091a0aabfd52000000006a47304402203eaaa3c4da101240f80f9c5e9de716a22b1ec6d66080de6a0cca32011cd77223022040d9082b6242d6acf9a1a8e658779e1c655d708379862f235e8ba7b8ca4e69c6012102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ffffffffff023ca13c0e9e085dd13f481f193e8a3e8fd609020936e98b5587342d994f4d020000006b483045022100c0ba56adb8de923975052312467347d83238bd8d480ce66e8b709a7997373994022048507bcac921fdb2302fa5224ce86e41b7efc1a2e20ae63aa738dfa99b7be826012102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ffffffff0300e1f5050000000017a9141ee6d4c38a3c078eab87ad1a5e4b00f21259b10d87000000000000000016611400000000000000000000000000000000000000001b94d736000000001976a91405aab5342166f8594baf17a7d9bef5d56744332788ac2d08e35e000000000000000000000000000000").unwrap();
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: tx,
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    let actual = block_on_f01(coin.wait_for_confirmations(confirm_payment_input))
        .err()
        .unwrap();
    assert!(actual.contains(
        "Tx d342ff9da528a2e262bddf2b6f9a27d1beb7aeb03f0fc8d9eac2987266447e44 was not found on chain after 10 tries"
    ));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_native_display_balances() {
    let unspents = vec![
        NativeUnspent {
            address: "RG278CfeNPFtNztFZQir8cgdWexVhViYVy".to_owned(),
            amount: "4.77699".into(),
            ..NativeUnspent::default()
        },
        NativeUnspent {
            address: "RJeDDtDRtKUoL8BCKdH7TNCHqUKr7kQRsi".to_owned(),
            amount: "0.77699".into(),
            ..NativeUnspent::default()
        },
        NativeUnspent {
            address: "RQHn9VPHBqNjYwyKfJbZCiaxVrWPKGQjeF".to_owned(),
            amount: "0.99998".into(),
            ..NativeUnspent::default()
        },
        NativeUnspent {
            address: "RG278CfeNPFtNztFZQir8cgdWexVhViYVy".to_owned(),
            amount: "1".into(),
            ..NativeUnspent::default()
        },
    ];

    NativeClient::list_unspent_impl
        .mock_safe(move |_, _, _, _| MockResult::Return(Box::new(futures01::future::ok(unspents.clone()))));

    let rpc_client = native_client_for_test();

    let addresses = vec![
        Address::from_legacyaddress("RG278CfeNPFtNztFZQir8cgdWexVhViYVy", &KMD_PREFIXES).unwrap(),
        Address::from_legacyaddress("RYPz6Lr4muj4gcFzpMdv3ks1NCGn3mkDPN", &KMD_PREFIXES).unwrap(),
        Address::from_legacyaddress("RJeDDtDRtKUoL8BCKdH7TNCHqUKr7kQRsi", &KMD_PREFIXES).unwrap(),
        Address::from_legacyaddress("RQHn9VPHBqNjYwyKfJbZCiaxVrWPKGQjeF", &KMD_PREFIXES).unwrap(),
    ];
    let actual = block_on_f01(rpc_client.display_balances(addresses, TEST_COIN_DECIMALS)).unwrap();

    let expected: Vec<(Address, BigDecimal)> = vec![
        (
            Address::from_legacyaddress("RG278CfeNPFtNztFZQir8cgdWexVhViYVy", &KMD_PREFIXES).unwrap(),
            BigDecimal::try_from(5.77699).unwrap(),
        ),
        (
            Address::from_legacyaddress("RYPz6Lr4muj4gcFzpMdv3ks1NCGn3mkDPN", &KMD_PREFIXES).unwrap(),
            BigDecimal::from(0),
        ),
        (
            Address::from_legacyaddress("RJeDDtDRtKUoL8BCKdH7TNCHqUKr7kQRsi", &KMD_PREFIXES).unwrap(),
            BigDecimal::try_from(0.77699).unwrap(),
        ),
        (
            Address::from_legacyaddress("RQHn9VPHBqNjYwyKfJbZCiaxVrWPKGQjeF", &KMD_PREFIXES).unwrap(),
            BigDecimal::try_from(0.99998).unwrap(),
        ),
    ];
    assert_eq!(actual, expected);
}

#[test]
fn test_message_hash() {
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(
        client.into(),
        Some("spice describe gravity federal blast come thank unfair canal monkey style afraid"),
        false,
    );
    let expected = H256::from_reversed_str("5aef9b67485adba55a2cd935269e73f2f9876382f1eada02418797ae76c07e18");
    let result = coin.sign_message_hash("test");
    assert!(result.is_some());
    assert_eq!(H256::from(result.unwrap()), expected);
}

#[test]
fn test_sign_verify_message() {
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(
        client.into(),
        Some("spice describe gravity federal blast come thank unfair canal monkey style afraid"),
        false,
    );

    let message = "test";
    let signature = coin.sign_message(message, None).unwrap();
    assert_eq!(
        signature,
        "HzetbqVj9gnUOznon9bvE61qRlmjH5R+rNgkxu8uyce3UBbOu+2aGh7r/GGSVFGZjRnaYC60hdwtdirTKLb7bE4="
    );

    let address = "R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW";
    let is_valid = coin.verify_message(&signature, message, address).unwrap();
    assert!(is_valid);
}

#[test]
fn test_sign_verify_message_segwit() {
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(
        client.into(),
        Some("spice describe gravity federal blast come thank unfair canal monkey style afraid"),
        true,
    );

    let message = "test";
    let signature = coin.sign_message(message, None).unwrap();
    assert_eq!(
        signature,
        "HzetbqVj9gnUOznon9bvE61qRlmjH5R+rNgkxu8uyce3UBbOu+2aGh7r/GGSVFGZjRnaYC60hdwtdirTKLb7bE4="
    );

    let is_valid = coin
        .verify_message(&signature, message, "doc1qqk4t2dppvmu9jja0z7nan0h464n5gve8z592yd")
        .unwrap();
    assert!(is_valid);

    let is_valid = coin
        .verify_message(&signature, message, "R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW")
        .unwrap();
    assert!(is_valid);
}

#[test]
fn test_tx_enum_from_bytes() {
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    let coin = utxo_coin_for_test(client.into(), None, false);

    let tx_hex = hex::decode("01000000017b1eabe0209b1fe794124575ef807057c77ada2138ae4fa8d6c4de0398a14f3f00000000494830450221008949f0cb400094ad2b5eb399d59d01c14d73d8fe6e96df1a7150deb388ab8935022079656090d7f6bac4c9a94e0aad311a4268e082a725f8aeae0573fb12ff866a5f01ffffffff01f0ca052a010000001976a914cbc20a7664f2f69e5355aa427045bc15e7c6c77288ac00000000").unwrap();
    coin.tx_enum_from_bytes(&tx_hex).unwrap();

    let tx_hex = hex::decode("0100000002440f1a2929eb08c350cc8d2385c77c40411560c3b43b65efb5b06f997fc67672020000006b483045022100f82e88af256d2487afe0c30a166c9ecf6b7013e764e1407317c712d47f7731bd0220358a4d7987bfde2271599b5c4376d26f9ce9f1df2e04f5de8f89593352607110012103c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3edfffffffffb9c2fd7a19b55a4ffbda2ce5065d988a4f4efcf1ae567b4ddb6d97529c8fb0c000000006b483045022100dd75291db32dc859657a5eead13b85c340b4d508e57d2450ebfad76484f254130220727fcd65dda046ea62b449ab217da264dbf7c7ca7e63b39c8835973a152752c1012103c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3edffffffff03102700000000000017a9148d0ad41545dea44e914c419d33d422148c35a274870000000000000000166a149c0a919d4e9a23f0234df916a7dd21f9e2fdaa8f931d0000000000001976a9146d9d2b554d768232320587df75c4338ecc8bf37d88acbd8ff160").unwrap();
    coin.tx_enum_from_bytes(&tx_hex).unwrap();

    let tx_hex = hex::decode("0200000000010192a4497268107d7999e9551be733f5e0eab479be7d995a061a7bbdc43ef0e5ed0000000000feffffff02cd857a00000000001600145cb39bfcd68d520e29cadc990bceb5cd1562c507a0860100000000001600149a85cc05e9a722575feb770a217c73fd6145cf01024730440220030e0fb58889ab939c701f12d950f00b64836a1a33ec0d6697fd3053d469d244022053e33d72ef53b37b86eea8dfebbafffb0f919ef952dcb6ea6058b81576d8dc86012102225de6aed071dc29d0ca10b9f64a4b502e33e55b3c0759eedd8e333834c6a7d07a1f2000").unwrap();
    coin.tx_enum_from_bytes(&tx_hex).unwrap();

    let err = coin.tx_enum_from_bytes(&vec![0; 1000000]).unwrap_err().into_inner();
    assert!(matches!(err, TxMarshalingErr::CrossCheckFailed(_)));
}

#[test]
fn test_hd_utxo_tx_history() {
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);
    block_on(utxo_common_tests::test_hd_utxo_tx_history_impl(client));
}

#[test]
fn test_utxo_validate_valid_and_invalid_pubkey() {
    let conf = json!({"coin":"RICK","asset":"RICK","rpcport":25435,"txversion":4,"overwintered":1,"mm2":1,"protocol":{"type":"UTXO"}});
    let req = json!({
         "method": "electrum",
         "servers": doc_electrums(),
        "check_utxo_maturity": true,
    });

    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();

    let priv_key = Secp256k1Secret::from([1; 32]);
    let coin = block_on(utxo_standard_coin_with_priv_key(&ctx, "RICK", &conf, &params, priv_key)).unwrap();
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
fn test_block_header_utxo_loop() {
    use crate::utxo::utxo_builder::{block_header_utxo_loop, BlockHeaderUtxoLoopExtraArgs};
    use keys::hash::H256 as H256Json;

    static CURRENT_BLOCK_COUNT: AtomicU64 = AtomicU64::new(13);

    ElectrumClient::get_servers_with_latest_block_count.mock_safe(move |_| {
        let servers = DOC_ELECTRUM_ADDRS.iter().map(|url| url.to_string()).collect();
        MockResult::Return(Box::new(futures01::future::ok((
            servers,
            CURRENT_BLOCK_COUNT.load(Ordering::Relaxed),
        ))))
    });
    let expected_steps: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::with_capacity(14)));

    ElectrumClient::retrieve_headers_from.mock_safe({
        let expected_steps = expected_steps.clone();
        move |this, server_address, from_height, to_height| {
            let (expected_from, expected_to) = expected_steps.lock().unwrap().remove(0);
            assert_eq!(from_height, expected_from);
            assert_eq!(to_height, expected_to);
            MockResult::Continue((this, server_address, from_height, to_height))
        }
    });

    BlockHeaderUtxoLoopExtraArgs::default.mock_safe(move || {
        MockResult::Return(BlockHeaderUtxoLoopExtraArgs {
            chunk_size: 4,
            error_sleep: 1.,
            success_sleep: 0.8,
        })
    });

    let ctx = mm_ctx_with_custom_db();
    let priv_key_policy = PrivKeyBuildPolicy::IguanaPrivKey(H256Json::from([1u8; 32]));
    let servers: Vec<_> = DOC_ELECTRUM_ADDRS
        .iter()
        .map(|server| json!({ "url": server }))
        .collect();
    let req = json!({ "method": "electrum", "servers": servers });
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let conf = json!({"coin":"RICK", "asset":"RICK", "rpcport":8923});
    let builder = UtxoArcBuilder::new(&ctx, "RICK", &conf, &params, priv_key_policy, UtxoStandardCoin::from);
    let arc: UtxoArc = block_on(builder.build_utxo_fields()).unwrap().into();
    let client = match &arc.rpc_client {
        UtxoRpcClientEnum::Electrum(electrum) => electrum.clone(),
        UtxoRpcClientEnum::Native(_) => unreachable!(),
    };

    let (sync_status_notifier, _) = channel::<UtxoSyncStatus>(1);
    let loop_handle = UtxoSyncStatusLoopHandle::new(sync_status_notifier);

    let spv_conf = json::from_value(json!({
        "starting_block_header": {
            "height": 1,
            "hash": "0918169860eda78df99319a4d073d325017fbda08dd10375a6de8b6214cef3f5",
            "time": 1681404988,
            "bits": 537857807
        },
        "max_stored_block_headers": 15
    }));

    let weak_client = Arc::downgrade(&client.0);
    let loop_fut = async move { block_header_utxo_loop(weak_client, loop_handle, spv_conf.unwrap()).await };

    let test_fut = async move {
        *expected_steps.lock().unwrap() = vec![(2, 5), (6, 9), (10, 13), (14, 14)];
        CURRENT_BLOCK_COUNT.store(14, Ordering::Relaxed);
        Timer::sleep(3.).await;
        let get_headers_count = client
            .block_headers_storage()
            .get_last_block_height()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(get_headers_count, 14);
        assert!(expected_steps.lock().unwrap().is_empty());

        *expected_steps.lock().unwrap() = vec![(15, 18)];
        CURRENT_BLOCK_COUNT.store(18, Ordering::Relaxed);
        Timer::sleep(2.).await;
        let get_headers_count = client
            .block_headers_storage()
            .get_last_block_height()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(get_headers_count, 18);
        assert!(expected_steps.lock().unwrap().is_empty());

        *expected_steps.lock().unwrap() = vec![(19, 19)];
        CURRENT_BLOCK_COUNT.store(19, Ordering::Relaxed);
        Timer::sleep(2.).await;
        let get_headers_count = client
            .block_headers_storage()
            .get_last_block_height()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(get_headers_count, 19);
        assert!(expected_steps.lock().unwrap().is_empty());

        // Validate max_stored_block_headers
        // Since max_stored_block_headers = 15, headers from 2 - 4 shouldn't be in
        // storage anymore.
        for i in 2..=19 {
            let header = client.block_headers_storage().get_block_header(i).await.unwrap();
            if i >= 5 {
                assert!(header.is_some());
                break;
            }

            assert_eq!(header, None);
        }
        Timer::sleep(2.).await;
    };

    if let Either::Left(_) = block_on(futures::future::select(loop_fut.boxed(), test_fut.boxed())) {
        panic!("Loop shouldn't stop")
    };
}

#[test]
fn test_spv_conf_with_verification() {
    let verification_params = BlockHeaderValidationParams {
        difficulty_check: false,
        constant_difficulty: false,
        difficulty_algorithm: Some(DifficultyAlgorithm::BitcoinMainnet),
    };

    // Block header hash for BLOCK HEIGHT 4032
    let hash = "00000000ca4b69045a03d7b20624def97a5366418648d5005e82fd3b345d20d0".into();
    // test for good retarget_block_header_height
    let mut spv_conf = SPVConf {
        starting_block_header: SPVBlockHeader {
            height: 4032,
            hash,
            time: 1234466190,
            bits: BlockHeaderBits::Compact(486604799.into()),
        },
        max_stored_block_headers: None,
        validation_params: Some(verification_params.clone()),
    };
    assert!(spv_conf.validate("BTC").is_ok());

    // test for bad retarget_block_header_height
    // Block header hash for BLOCK HEIGHT 4037
    let hash = "0000000045c689dc49dee778a9fbca7b5bc48fceca9f05cde5fc8d667f00e7d2".into();
    spv_conf.starting_block_header = SPVBlockHeader {
        height: 4037,
        hash,
        time: 1234470475,
        bits: BlockHeaderBits::Compact(486604799.into()),
    };
    let validate = spv_conf.validate("BTC").err().unwrap();
    if let SPVError::WrongRetargetHeight { coin, expected_height } = validate {
        assert_eq!(coin, "BTC");
        assert_eq!(expected_height, 4032);
    }

    // test for bad max_stored_block_headers
    // Block header hash for BLOCK HEIGHT 4032
    let hash = "00000000ca4b69045a03d7b20624def97a5366418648d5005e82fd3b345d20d0".into();
    spv_conf = SPVConf {
        starting_block_header: SPVBlockHeader {
            height: 4032,
            hash,
            time: 1234466190,
            bits: BlockHeaderBits::Compact(486604799.into()),
        },
        max_stored_block_headers: NonZeroU64::new(2000),
        validation_params: Some(verification_params),
    };
    let validate = spv_conf.validate("BTC").err().unwrap();
    assert!(validate
        .to_string()
        .contains("max_stored_block_headers 2000 must be greater than retargeting interval"));
}

#[cfg(not(target_arch = "wasm32"))]
fn rick_blocker_5() -> BlockHeader {
    let header =
        "0400000028a4f1aa8be606c8bf8195b2e95d478a83314ff9ad7b017457d9e58d00d1710bb43f41db65677e3fdb83ddbd8cfb4a7ad2e110f74bc19726dc949576e003a1ecfbc2f4300c01f0b7820d00e3347c8da4ee614674376cbc45359daa54f9b5493e381b405d0f0f0f2001003cfb15008ad9f4fab1ff4076f8919f743193f007c0db28f5106e003b0000fd400500acba878991f600ed8c022758be9ff9752ef175e7530324df4d1b87f5a03ca5c2c3fce10b08743bd5ba03912703b8f305f7dd382487d437d9b1823cdc11a00f59a20b235ef57502a0a7ad6fc7d3d242e8f4477a01fb8834ac4dc6e2e40e4909f9edc0db07c0f98df40e5a61327311b005c98a727694ebaabcb366b92dda4af9e3f6e72c5461dd81d6daccbd1fca8ec17597df7585947b54deb83554859776b5bcefadfa566ff12c04ac624f9416e76beccec35694ae0ed11dc17a911f114225be62cf5b971628f364f57d8348d95fdc415b0d2a7a477ea130d3320108739edf761f85f81efd6c0e4eafa8166b05bd74af7928b0786b63ae499dba38065be13e7541b7f4e26727d0fa6887e265e09709b940ca87295ce5984de7d4058b5d340b162935fa46ee20cac955379e3c8fa1ff92fb354bb2a0fedf697b683a5875f4ed2bcef984d296b0c1e07a52920f1dd5a60140c7c1245a52ed196df3292db8bfff52923b0a8615b6a99a5fcf1e5f461f01a04b1c3bb517fe16553e1f8e8aa20bd3cc2cac6d3242a2ce373737b57cec4637907fd236e0d44d91d59533484ec23634b93645c10a858d83805d731f300aa27a162e172216d7fc21170b4d232767e4c66f9a871224f13480e89c2edb0e6e1ef5cf75d9203839cc0282fd7852319232057f30793bb5552d94ebf3ffcc67b73f44e80c3de79b9d8d7f0175939722054bc2ddfb84288dff8c7554f191d6ee1b65c40b75d4435712d4e88c64d6379ab7e578bcd8117501504faa7a3be3a6a2826fd7a3e5e9efb1d3642937f3a35be5793be8e1d4acf9dd2dcd356d6e4c7d0c8b87587b8ad901b9ce71792ae0bdae27811b52300e6809e4691bfc7f738252e7c197e228cce5fda6130f8f518e5059530b731fe8afbf51308aa8da3bd31b1d1eb22cca1a896aed281397925265cd861a7eadb80124363dec8cb508aea7c277f04b9841888dd932471349e651ce2622a59065932f463ffce6b19a975d6914336ab49394afd17dfb9a448157007ea1437b1483587bc7de0dec5103cafad76704e91e9ea2b0b9a8570b935d5c65478e7195b08161be4625b8d5fd3658e6164cf2d6898ecbf1f14945fdd75bb991a3d9ffac713a3a7a81a31a765b9c37a578976aa15e66c97c957f4651dc5fc492c2111d8724d375a8293a36e0ddcf2a01facf30401d8677611522882e1447e4c8be5fa9ad073fb3fdcc6f673981484089090fe4c05bfaae173503e0f99c7407b297852d216463924d365d26b4cd63401a46bd7ed969ddb235044eb2373645144976c7f713720c0238ade9d3aae1d2b153e82d093232d4b12b2108ec564ae0e855e09252f1434c28d90bb298ab6d1750498bf90d93c8797901911548b81af1ba185be52c0dff9c1b11812941d2d527c95c4382879298f364077710b5efd56d1bf39148aedc4fcd9e8bddb4c36a3f901dc11f9493d1fbdfe80c88fa8866c1465c939c0d71cb57e78822b5fc3023578aa2d6b9cd3ebaa54f22876b935f251183d8a68459cab30cd19bcb4e4c1e1a5a83e4687a4795dc23732e81b9f024f70db96e412831d26e61d4fa292a95648e0b614d9a148cd852df1bf26a34ea971e63f8c634133ab7b13ac8045f6d6e20af2313b38d12cb8cee54a7aba7a7cd7e8b1b5e0b0931d4665a0bb36b63f325161b571fdd4f159f470e443e9b0cfb193bf4eea5fa9715dc6132cb8ed97f7f097837471a5147d14f2066cd3dcd50460d70180a7a24e2b5b9ab20caf952d2ea1b51747afec975f76d0313a98e444f20938bf709530960f9fbf5af9857cbe3410d37f3cba10ff57642861586b7c1b1c57019602f1529df9d6e45ca2f7663519c58915e9e299d5beee73cb4553238566844f571374d3f6a247dd8ecbbc893";

    BlockHeader::try_from_string_with_coin_variant(header.to_string(), "RICK".into()).unwrap()
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_block_header_utxo_loop_with_reorg() {
    use crate::utxo::utxo_builder::{block_header_utxo_loop, BlockHeaderUtxoLoopExtraArgs};
    use futures::future::{Either, FutureExt};
    use keys::hash::H256 as H256Json;

    static CURRENT_BLOCK_COUNT: AtomicU64 = AtomicU64::new(3);
    static IS_MISMATCH_HEADER: AtomicBool = AtomicBool::new(true);
    let rick_headers = include_str!("../for_tests/RICK_HEADERS.json");
    let rick_headers: Vec<String> = serde_json::from_str(rick_headers).unwrap();
    let mut rick_headers_map = HashMap::new();
    for (idx, header) in rick_headers.into_iter().enumerate() {
        rick_headers_map.insert(
            (idx + 2) as u64,
            BlockHeader::try_from_string_with_coin_variant(header, "RICK".into()).unwrap(),
        );
    }

    ElectrumClient::get_servers_with_latest_block_count.mock_safe(move |_| {
        let servers = DOC_ELECTRUM_ADDRS.iter().map(|url| url.to_string()).collect();
        MockResult::Return(Box::new(futures01::future::ok((
            servers,
            CURRENT_BLOCK_COUNT.load(Ordering::Relaxed),
        ))))
    });

    let mut rick_headers_map_clone = rick_headers_map.clone();
    ElectrumClient::retrieve_headers_from.mock_safe(move |_this, _server_addr, from_height, to_height| {
        let header_map = rick_headers_map_clone
            .clone()
            .into_iter()
            .filter(|(index, _)| index >= &from_height && index <= &to_height)
            .collect::<HashMap<_, _>>();

        let mut header_vec = vec![];

        for i in from_height..=to_height {
            header_vec.push(header_map.get(&i).unwrap().clone());
        }
        // the first time headers from 5 is requested, we expected chain reorg error so we switch the bad header at
        // height 5 with a valid header so the next retrieval can validate it.
        if from_height == 5 && IS_MISMATCH_HEADER.load(Ordering::Relaxed) {
            IS_MISMATCH_HEADER.store(false, Ordering::Relaxed);
            if let Some(header) = rick_headers_map_clone.get_mut(&5) {
                *header = rick_blocker_5();
            }
        }

        MockResult::Return(Box::new(futures01::future::ok((
            header_map.into_iter().collect(),
            header_vec,
        ))))
    });

    BlockHeaderStorage::get_block_header.mock_safe(move |_this, height| {
        let res = rick_headers_map.get(&height).unwrap();
        MockResult::Return(Box::pin(futures::future::ok(Some(res.clone()))))
    });

    BlockHeaderUtxoLoopExtraArgs::default.mock_safe(move || {
        MockResult::Return(BlockHeaderUtxoLoopExtraArgs {
            chunk_size: 2,
            error_sleep: 1.,
            success_sleep: 0.8,
        })
    });

    let ctx = mm_ctx_with_custom_db();
    let priv_key_policy = PrivKeyBuildPolicy::IguanaPrivKey(H256Json::from([1u8; 32]));
    let servers: Vec<_> = DOC_ELECTRUM_ADDRS
        .iter()
        .map(|server| json!({ "url": server }))
        .collect();
    let req = json!({ "method": "electrum", "servers": servers });
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let conf = json!({"coin":"RICK", "asset":"RICK", "rpcport":8923});
    let builder = UtxoArcBuilder::new(&ctx, "RICK", &conf, &params, priv_key_policy, UtxoStandardCoin::from);
    let arc: UtxoArc = block_on(builder.build_utxo_fields()).unwrap().into();
    let client = match &arc.rpc_client {
        UtxoRpcClientEnum::Electrum(electrum) => electrum.clone(),
        UtxoRpcClientEnum::Native(_) => unreachable!(),
    };

    let (sync_status_notifier, _) = channel::<UtxoSyncStatus>(1);
    let loop_handle = UtxoSyncStatusLoopHandle::new(sync_status_notifier);

    let spv_conf = json::from_value(json!({
        "starting_block_header": {
            "height": 1,
            "hash": "0918169860eda78df99319a4d073d325017fbda08dd10375a6de8b6214cef3f5",
            "time": 1681404988,
            "bits": 537857807
        },
        "max_stored_block_headers": 100
    }));

    let weak_client = Arc::downgrade(&client.0);
    let loop_fut = async move { block_header_utxo_loop(weak_client, loop_handle, spv_conf.unwrap()).await };

    let test_fut = async move {
        Timer::sleep(2.).await;
        let get_headers_count = client
            .block_headers_storage()
            .get_last_block_height()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(get_headers_count, 3);

        CURRENT_BLOCK_COUNT.store(5, Ordering::Relaxed);
        Timer::sleep(2.).await;
        let get_headers_count = client
            .block_headers_storage()
            .get_last_block_height()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(get_headers_count, 5);

        CURRENT_BLOCK_COUNT.store(8, Ordering::Relaxed);
        Timer::sleep(2.).await;
        let get_headers_count = client
            .block_headers_storage()
            .get_last_block_height()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(get_headers_count, 8);

        CURRENT_BLOCK_COUNT.store(10, Ordering::Relaxed);
        Timer::sleep(2.).await;
        let get_headers_count = client
            .block_headers_storage()
            .get_last_block_height()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(get_headers_count, 10);
    };

    if let Either::Left(_) = block_on(futures::future::select(loop_fut.boxed(), test_fut.boxed())) {
        panic!("Loop shouldn't stop")
    };
}

#[test]
fn test_electrum_v14_block_hash() {
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS);

    // First verify BlockHeader hash implementation works correctly with a known reference block
    let headers =
        block_on_f01(client.blockchain_block_headers(841548, NonZeroU64::new(1).expect("Failed to create NonZeroU64")))
            .expect("Failed to fetch block headers");

    // Deserialize the reference block header
    let serialized = serialize(&CompactInteger::from(headers.count))
        .take()
        .into_iter()
        .chain(headers.hex.0)
        .collect::<Vec<_>>();
    let headers = Reader::new_with_coin_variant(&serialized, CoinVariant::RICK)
        .read_list::<BlockHeader>()
        .expect("Failed to deserialize headers");

    // Confirm BlockHeader hash matches the known hash value
    assert_eq!(
        headers[0].hash().reversed().to_string(),
        "0f0a6ce253b0536000636f85491db8030659064de8c27423b46ceef824d4ad28"
    );

    // Now get the latest block via V14 subscription to test its hash implementation
    let header =
        block_on_f01(client.blockchain_headers_subscribe()).expect("Failed to subscribe to blockchain headers");

    // Extract hash and height from V14 header
    let (hash, height) = match header {
        ElectrumBlockHeader::V14(header) => (header.hash(), header.height),
        _ => panic!("Expected ElectrumBlockHeader::V14"),
    };

    // Get the same block data to create a BlockHeader for comparison
    let headers =
        block_on_f01(client.blockchain_block_headers(height, NonZeroU64::new(1).expect("Failed to create NonZeroU64")))
            .expect("Failed to fetch block headers");

    // Create BlockHeader from the same block (using the implementation we just verified)
    let serialized = serialize(&CompactInteger::from(headers.count))
        .take()
        .into_iter()
        .chain(headers.hex.0)
        .collect::<Vec<_>>();
    let headers = Reader::new_with_coin_variant(&serialized, CoinVariant::RICK)
        .read_list::<BlockHeader>()
        .expect("Failed to deserialize headers");

    // Verify V14 header produces the same hash as our verified BlockHeader implementation
    assert_eq!(hash, headers[0].hash().into());
}

/// A utility test for debugging block header deserialization issues for any UTXO-based coin.
/// This test is ignored by default and must be run explicitly.
///
/// It scans a range of block heights, fetching headers in chunks. For each chunk, it reads
/// headers one by one from the data stream.
///
/// If it encounters a header that fails to parse, it will panic and print detailed information,
/// including the exact block height that failed and the raw hex of the entire chunk for context.
///
/// # How to Use:
/// 1.  Modify the constants in the `CONFIGURATION` section below.
/// 2.  Run the test with the `--ignored` flag: `cargo test -- --ignored test_scan_and_deserialize_block_headers`
///
/// # Debugging Note:
/// If a header at height `N` fails, the error might be caused by the deserializer reading
/// more data than expected from the header at height `N-1`. The full chunk hex provided
/// in the panic message is essential for debugging this scenario.
#[test]
#[ignore = "This is a utility test for debugging header deserialization and must be run explicitly"]
fn test_scan_and_deserialize_block_headers() {
    // ========================== CONFIGURATION ==========================
    /// The ticker of the coin to test (e.g., "NMC", "CHTA", "RVN").
    const COIN_TICKER: &str = "PIVX";
    /// A list of active Electrum servers for the specified coin.
    const ELECTRUM_URLS: &[&str] = &["electrum01.chainster.org:50001", "electrum02.chainster.org:50001"];
    /// The block height to start scanning from.
    const START_HEIGHT: u64 = 4903982;
    /// The block height to stop scanning at. Set to `None` to scan to the tip of the chain.
    const END_HEIGHT: Option<u64> = Some(4913982);
    /// The number of headers to fetch in a single RPC call.
    const CHUNK_SIZE: u64 = 100;
    // ===================================================================

    let client = electrum_client_for_test(ELECTRUM_URLS);
    let mut current_height = START_HEIGHT;

    loop {
        let mut num_to_fetch = CHUNK_SIZE;
        if let Some(end_h) = END_HEIGHT {
            if current_height > end_h {
                println!("Reached configured end height of {end_h}. Scan complete.");
                break;
            }
            let remaining = end_h.saturating_sub(current_height) + 1;
            num_to_fetch = num_to_fetch.min(remaining);
        }

        println!("Fetching {num_to_fetch} headers from height {current_height}");
        let headers_res =
            block_on_f01(client.blockchain_block_headers(current_height, NonZeroU64::new(num_to_fetch).unwrap()))
                .expect("Failed to get block headers");

        if headers_res.count == 0 {
            println!("Reached the end of the chain. No bad header found.");
            break;
        }

        // This is the correct approach, inspired by your original test.
        // We create a single reader for the entire raw byte stream of concatenated headers.
        let raw_chunk_bytes = &headers_res.hex.0;
        let mut reader = Reader::new_with_coin_variant(raw_chunk_bytes, COIN_TICKER.into());

        // We loop exactly `count` times, reading one header in each iteration.
        // The `read` method will correctly consume a variable number of bytes depending on the header's content.
        for i in 0..headers_res.count {
            let block_height_of_header = current_height + i;

            if let Err(e) = reader.read::<BlockHeader>() {
                // If a read fails, we've found the problematic header.
                // We panic with all the necessary context for debugging.
                let chunk_hex_str = hex::encode(raw_chunk_bytes);
                panic!("\n\n!!! Deserialization failed on header index {} (block height: {}) within the chunk starting at {} !!!\nDeserialization Error: {:?}\nRaw Chunk Hex: {}\n\n", i, block_height_of_header, current_height, e, chunk_hex_str);
            }
        }

        // If the loop completes, the entire chunk was successfully parsed.
        println!("Successfully deserialized chunk starting at height {current_height}.");
        current_height += headers_res.count;
    }

    println!("Scan finished successfully. No bad headers found in the specified range.");
}
