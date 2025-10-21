use super::*;
use crate::hd_wallet::{HDAccountsMap, HDAccountsMutex, HDAddressesCache, HDWallet, HDWalletCoinStorage};
use crate::my_tx_history_v2::{
    my_tx_history_v2_impl, CoinWithTxHistoryV2, MyTxHistoryDetails, MyTxHistoryRequestV2, MyTxHistoryResponseV2,
    MyTxHistoryTarget,
};
use crate::tx_history_storage::TxHistoryStorageBuilder;
use crate::utxo::rpc_clients::{ElectrumClient, UtxoRpcClientOps};
use crate::utxo::tx_cache::dummy_tx_cache::DummyVerboseCache;
use crate::utxo::tx_cache::UtxoVerboseCacheOps;
use crate::utxo::utxo_hd_wallet::UtxoHDAccount;
use crate::utxo::utxo_tx_history_v2::{utxo_history_loop, UtxoTxHistoryOps};
use crate::{compare_transaction_details, UtxoStandardCoin};
use common::custom_futures::repeatable::{Ready, Retry};
use common::executor::{spawn, Timer};
use common::jsonrpc_client::JsonRpcErrorType;
use common::log::info;
use common::PagingOptionsEnum;
use crypto::privkey::key_pair_from_seed;
use crypto::HDPathToAccount;
use itertools::Itertools;
use keys::prefixes::*;
use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;
use std::convert::TryFrom;
use std::num::NonZeroUsize;
use std::str::FromStr;
use std::time::Duration;

pub(super) const TEST_COIN_NAME: &str = "DOC";
// Made-up hrp for DOC to test p2wpkh script
pub(super) const TEST_COIN_HRP: &str = "doc";
pub(super) const TEST_COIN_DECIMALS: u8 = 8;

const DOC_HD_TX_HISTORY_STR: &str = include_str!("../for_tests/DOC_HD_tx_history_fixtures.json");

lazy_static! {
    static ref DOC_HD_TX_HISTORY_MAP: HashMap<String, TransactionDetails> = parse_tx_history_map(DOC_HD_TX_HISTORY_STR);
}

fn parse_tx_history(history_str: &'static str) -> Vec<TransactionDetails> {
    json::from_str(history_str).unwrap()
}

fn parse_tx_history_map(history_str: &'static str) -> HashMap<String, TransactionDetails> {
    parse_tx_history(history_str)
        .into_iter()
        .map(|tx| (format!("{:02x}", tx.internal_id), tx))
        .collect()
}

pub(super) fn utxo_coin_fields_for_test(
    rpc_client: UtxoRpcClientEnum,
    force_seed: Option<&str>,
    is_segwit_coin: bool,
) -> UtxoCoinFields {
    let checksum_type = ChecksumType::DSHA256;
    let default_seed = "spice describe gravity federal blast come thank unfair canal monkey style afraid";
    let seed = match force_seed {
        Some(s) => s.into(),
        None => match std::env::var("BOB_PASSPHRASE") {
            Ok(p) => {
                if p.is_empty() {
                    default_seed.into()
                } else {
                    p
                }
            },
            Err(_) => default_seed.into(),
        },
    };
    let key_pair = key_pair_from_seed(&seed).unwrap();
    let prefixes = if is_segwit_coin {
        NetworkAddressPrefixes::default()
    } else {
        NetworkAddressPrefixes {
            p2pkh: [60].into(),
            p2sh: AddressPrefix::default(),
        }
    };
    let hrp = if is_segwit_coin {
        Some(TEST_COIN_HRP.to_string())
    } else {
        None
    };
    let addr_format = if is_segwit_coin {
        UtxoAddressFormat::Segwit
    } else {
        UtxoAddressFormat::Standard
    };
    let my_address = AddressBuilder::new(addr_format, checksum_type, prefixes, hrp)
        .as_pkh_from_pk(*key_pair.public())
        .build()
        .expect("valid address props");
    let my_script_pubkey = Builder::build_p2pkh(my_address.hash()).to_bytes();

    let priv_key_policy = PrivKeyPolicy::Iguana(key_pair);
    let derivation_method = DerivationMethod::SingleAddress(my_address);

    let bech32_hrp = if is_segwit_coin {
        Some(TEST_COIN_HRP.to_string())
    } else {
        None
    };

    UtxoCoinFields {
        conf: UtxoCoinConf {
            is_pos: false,
            is_posv: false,
            requires_notarization: false.into(),
            overwintered: true,
            segwit: true,
            tx_version: 4,
            default_address_format: UtxoAddressFormat::Standard,
            asset_chain: true,
            address_prefixes: NetworkAddressPrefixes {
                p2pkh: [60].into(),
                p2sh: [85].into(),
            },
            sign_message_prefix: Some(String::from("Komodo Signed Message:\n")),
            bech32_hrp,
            ticker: TEST_COIN_NAME.into(),
            wif_prefix: 0,
            tx_fee_volatility_percent: DEFAULT_DYNAMIC_FEE_VOLATILITY_PERCENT,
            version_group_id: 0x892f2085,
            consensus_branch_id: 0x76b809bb,
            zcash: true,
            checksum_type,
            fork_id: 0,
            signature_version: SignatureVersion::Base,
            required_confirmations: 1.into(),
            force_min_relay_fee: false,
            mtp_block_count: NonZeroU64::new(11).unwrap(),
            estimate_fee_mode: None,
            mature_confirmations: MATURE_CONFIRMATIONS_DEFAULT,
            estimate_fee_blocks: 1,
            trezor_coin: None,
            spv_conf: None,
            derivation_path: None,
            avg_blocktime: None,
        },
        decimals: TEST_COIN_DECIMALS,
        dust_amount: UTXO_DUST_AMOUNT,
        tx_fee: FeeRate::FixedPerKb(1000),
        rpc_client,
        priv_key_policy,
        derivation_method,
        history_sync_state: Mutex::new(HistorySyncState::NotEnabled),
        tx_cache: DummyVerboseCache.into_shared(),
        recently_spent_outpoints: AsyncMutex::new(RecentlySpentOutPoints::new(my_script_pubkey)),
        tx_hash_algo: TxHashAlgo::DSHA256,
        check_utxo_maturity: false,
        block_headers_status_notifier: None,
        block_headers_status_watcher: None,
        ctx: MmWeak::default(),
        abortable_system: AbortableQueue::default(),
    }
}

pub(super) fn utxo_coin_from_fields(coin: UtxoCoinFields) -> UtxoStandardCoin {
    let arc: UtxoArc = coin.into();
    arc.into()
}

pub(super) async fn wait_for_tx_history_finished<Coin>(
    ctx: &MmArc,
    coin: &Coin,
    target: MyTxHistoryTarget,
    expected_txs: usize,
    timeout_s: u64,
) -> MyTxHistoryResponseV2<MyTxHistoryDetails, BytesJson>
where
    Coin: CoinWithTxHistoryV2 + MmCoin,
{
    let req = MyTxHistoryRequestV2 {
        coin: coin.ticker().to_owned(),
        limit: u32::MAX as usize,
        paging_options: PagingOptionsEnum::PageNumber(NonZeroUsize::new(1).unwrap()),
        target,
    };

    // Let the storage to be initialized for the given coin.
    Timer::sleep(1.).await;

    repeatable!(async {
        let response = my_tx_history_v2_impl(ctx.clone(), coin, req.clone()).await.unwrap();
        info!("LIMIT: {}", response.transactions.len());
        if response.transactions.len() >= expected_txs {
            return Ready(response);
        }
        Retry(())
    })
    .repeat_every(Duration::from_secs(3))
    .with_timeout_ms(timeout_s * 1000)
    .await
    .unwrap()
}

pub(super) fn get_doc_hd_transactions_ordered(tx_hashes: &[&str]) -> Vec<TransactionDetails> {
    tx_hashes
        .iter()
        .map(|tx_hash| {
            DOC_HD_TX_HISTORY_MAP
                .get(*tx_hash)
                .unwrap_or_else(|| panic!("No such {:?} TX in the file", tx_hash))
                .clone()
        })
        .sorted_by(compare_transaction_details)
        .collect()
}

pub(super) async fn test_electrum_display_balances(rpc_client: &ElectrumClient) {
    let addresses = vec![
        Address::from_legacyaddress("RG278CfeNPFtNztFZQir8cgdWexVhViYVy", &KMD_PREFIXES).unwrap(),
        Address::from_legacyaddress("RYPz6Lr4muj4gcFzpMdv3ks1NCGn3mkDPN", &KMD_PREFIXES).unwrap(),
        Address::from_legacyaddress("RJeDDtDRtKUoL8BCKdH7TNCHqUKr7kQRsi", &KMD_PREFIXES).unwrap(),
        Address::from_legacyaddress("RQHn9VPHBqNjYwyKfJbZCiaxVrWPKGQjeF", &KMD_PREFIXES).unwrap(),
    ];
    let actual = rpc_client.display_balances(addresses, 8).compat().await.unwrap();

    let expected: Vec<(Address, BigDecimal)> = vec![
        (
            Address::from_legacyaddress("RG278CfeNPFtNztFZQir8cgdWexVhViYVy", &KMD_PREFIXES).unwrap(),
            BigDecimal::try_from(5.77699).unwrap(),
        ),
        (
            Address::from_legacyaddress("RYPz6Lr4muj4gcFzpMdv3ks1NCGn3mkDPN", &KMD_PREFIXES).unwrap(),
            BigDecimal::try_from(3.33).unwrap(),
        ),
        (
            Address::from_legacyaddress("RJeDDtDRtKUoL8BCKdH7TNCHqUKr7kQRsi", &KMD_PREFIXES).unwrap(),
            BigDecimal::try_from(0.77699).unwrap(),
        ),
        (
            Address::from_legacyaddress("RQHn9VPHBqNjYwyKfJbZCiaxVrWPKGQjeF", &KMD_PREFIXES).unwrap(),
            BigDecimal::try_from(16.55398).unwrap(),
        ),
    ];
    assert_eq!(actual, expected);

    let invalid_hashes = vec![
        "0128a4ea8c5775039d39a192f8490b35b416f2f194cb6b6ee91a41d01233c3b5".to_owned(),
        "!INVALID!".to_owned(),
        "457206aa039ed77b223e4623c19152f9aa63aa7845fe93633920607500766931".to_owned(),
    ];

    let rpc_err = rpc_client
        .scripthash_get_balances(invalid_hashes)
        .compat()
        .await
        .unwrap_err();
    match rpc_err.error {
        JsonRpcErrorType::Response(_, json_err) => {
            let expected = json!({"code": 1, "message": "!INVALID! is not a valid script hash"});
            assert_eq!(json_err, expected);
        },
        ekind => panic!("Unexpected `JsonRpcErrorType`: {:?}", ekind),
    }
}

/// TODO move this test to `mm2_tests.rs`
/// when [Trezor Daemon Emulator](https://github.com/trezor/trezord-go#emulator-support) is integrated.
pub(super) async fn test_hd_utxo_tx_history_impl(rpc_client: ElectrumClient) {
    let ctx = mm_ctx_with_custom_db();
    let hd_account_for_test = UtxoHDAccount {
        account_id: 0,
        extended_pubkey: Secp256k1ExtendedPublicKey::from_str("xpub6DEHSksajpRPM59RPw7Eg6PKdU7E2ehxJWtYdrfQ6JFmMGBsrR6jA78ANCLgzKYm4s5UqQ4ydLEYPbh3TRVvn5oAZVtWfi4qJLMntpZ8uGJ").unwrap(),
        account_derivation_path: HDPathToAccount::from_str("m/44'/141'/0'").unwrap(),
        external_addresses_number: 10,
        internal_addresses_number: 0,
        derived_addresses: HDAddressesCache::default(),
    };

    let mut hd_accounts = HDAccountsMap::new();
    hd_accounts.insert(0, hd_account_for_test);

    let mut fields = utxo_coin_fields_for_test(rpc_client.into(), None, false);
    fields.ctx = ctx.weak();
    fields.conf.ticker = "DOC".to_string();
    fields.derivation_method = DerivationMethod::HDWallet(UtxoHDWallet {
        inner: HDWallet {
            hd_wallet_rmd160: "6d9d2b554d768232320587df75c4338ecc8bf37d".into(),
            hd_wallet_storage: HDWalletCoinStorage::default(),
            derivation_path: HDPathToCoin::from_str("m/44'/141'").unwrap(),
            accounts: HDAccountsMutex::new(hd_accounts),
            enabled_address: HDPathAccountToAddressId::default(),
            gap_limit: 20,
        },
        address_format: UtxoAddressFormat::Standard,
    });

    let coin = utxo_coin_from_fields(fields);
    let current_balances = coin.my_addresses_balances().await.unwrap();
    let storage = TxHistoryStorageBuilder::new(&ctx).build().unwrap();
    spawn(utxo_history_loop(
        coin.clone(),
        storage,
        ctx.metrics.clone(),
        ctx.event_stream_manager.clone(),
        current_balances.clone(),
    ));

    let target = MyTxHistoryTarget::AccountId { account_id: 0 };
    let tx_history = wait_for_tx_history_finished(&ctx, &coin, target, 1, 20).await;

    let actual: Vec<_> = tx_history.transactions.into_iter().map(|tx| tx.details).collect();
    let expected =
        get_doc_hd_transactions_ordered(&["2f8b4178b56d0a9f0ad31afcbef6ff267a0bf655dcff72de530107a4c93407b6"]);
    assert_eq!(actual, expected);

    // Activate new `RYM6yDMn8vdqtkYKLzY5dNe7p3T6YmMWvq` address.
    match coin.as_ref().derivation_method {
        DerivationMethod::HDWallet(ref hd_wallet) => {
            let mut accounts = hd_wallet.inner.accounts.lock().await;
            accounts.get_mut(&0).unwrap().external_addresses_number += 1
        },
        _ => unimplemented!(),
    }

    let storage = TxHistoryStorageBuilder::new(&ctx).build().unwrap();
    spawn(utxo_history_loop(
        coin.clone(),
        storage,
        ctx.metrics.clone(),
        ctx.event_stream_manager.clone(),
        current_balances,
    ));

    // Wait for the TX history loop to fetch Transactions of the activated address.
    let target = MyTxHistoryTarget::AccountId { account_id: 0 };
    let tx_history = wait_for_tx_history_finished(&ctx, &coin, target, 2, 20).await;

    let actual: Vec<_> = tx_history.transactions.into_iter().map(|tx| tx.details).collect();
    let expected = get_doc_hd_transactions_ordered(&[
        // New transaction:
        "2f8b4178b56d0a9f0ad31afcbef6ff267a0bf655dcff72de530107a4c93407b6",
        "071200b4b2967cfe3522b8a6713b8bdcd09f74a17d575fad87b4e97bc442f404",
    ]);
    assert_eq!(actual, expected);
}
