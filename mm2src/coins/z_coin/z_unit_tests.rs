use super::*;
use crate::utxo::rpc_clients::ElectrumClient;
use crate::utxo::rpc_clients::UtxoRpcClientOps;
use crate::z_coin::storage::WalletDbShared;
use crate::CoinProtocol;
use crate::DexFeeBurnDestination;
use common::executor::spawn_abortable;
use core::convert::AsRef;
use ff::{Field, PrimeField};
use futures::channel::mpsc::channel;
use futures::lock::Mutex as AsyncMutex;
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_net::transport::slurp_url_with_headers;
use mm2_test_helpers::for_tests::zombie_conf;
use mocktopus::mocking::*;
use rand::rngs::OsRng;
use rand::RngCore;
use std::fs::{self, create_dir};
use std::path::Path;
use url::Url;
use zcash_primitives::merkle_tree::CommitmentTree;
use zcash_primitives::merkle_tree::IncrementalWitness;
use zcash_primitives::sapling::Node;
use zcash_primitives::sapling::Rseed;
use zcash_primitives::transaction::components::amount::DEFAULT_FEE;

const GITHUB_CLIENT_USER_AGENT: &str = "mm2";

/// Download zcash params from komodo repo
async fn fetch_and_save_params(param: &str, fname: &Path) -> Result<(), String> {
    let url = Url::parse(&format!("{DOWNLOAD_URL}/")).unwrap().join(param).unwrap();
    println!("downloading zcash params {url}...");
    let data = slurp_url_with_headers(
        url.as_str(),
        vec![(http::header::USER_AGENT.as_str(), GITHUB_CLIENT_USER_AGENT)],
    )
    .await
    .map_err(|err| format!("could not download zcash params: {err}"))?
    .2;
    println!("saving zcash params to file {}...", fname.display());
    fs::write(fname, data).map_err(|err| format!("could not save zcash params: {err}"))
}

/// download zcash params, if not exist
pub(super) async fn download_parameters_for_tests(z_params_path: &Path) {
    let sapling_spend_fname = z_params_path.join(SAPLING_SPEND_NAME);
    let sapling_output_fname = z_params_path.join(SAPLING_OUTPUT_NAME);
    if !sapling_spend_fname.exists()
        || !sapling_output_fname.exists()
        || !verify_checksum_zcash_params(&sapling_spend_fname, &sapling_output_fname).is_ok_and(|r| r)
    {
        let _ = create_dir(z_params_path);
        fetch_and_save_params(SAPLING_SPEND_NAME, sapling_spend_fname.as_path())
            .await
            .unwrap();
        fetch_and_save_params(SAPLING_OUTPUT_NAME, sapling_output_fname.as_path())
            .await
            .unwrap();
    }
}

pub(super) async fn create_test_sync_connector<'a>(
    builder: &ZCoinBuilder<'a>,
) -> (AsyncMutex<SaplingSyncConnector>, WalletDbShared) {
    let wallet_db = WalletDbShared::new(builder, None, true).await.unwrap(); // Note: assuming we have a spending key in the builder
    let (_, sync_watcher) = channel(1);
    let (on_tx_gen_notifier, _) = channel(1);
    let abort_handle = spawn_abortable(futures::future::ready(()));
    let first_sync_block = FirstSyncBlock {
        requested: 0,
        is_pre_sapling: false,
        actual: 0,
    };
    let sync_state_connector =
        SaplingSyncConnector::new_mutex_wrapped(sync_watcher, on_tx_gen_notifier, abort_handle, first_sync_block);
    (sync_state_connector, wallet_db)
}

#[allow(clippy::too_many_arguments)]
async fn z_coin_from_conf_and_params_for_tests(
    ctx: &MmArc,
    ticker: &str,
    conf: &Json,
    params: &ZcoinActivationParams,
    priv_key_policy: PrivKeyBuildPolicy,
    protocol_info: ZcoinProtocolInfo,
    spending_key: &str,
) -> Result<ZCoin, MmError<ZCoinBuildError>> {
    use zcash_client_backend::encoding::decode_extended_spending_key;
    let z_spending_key =
        decode_extended_spending_key(z_mainnet_constants::HRP_SAPLING_EXTENDED_SPENDING_KEY, spending_key)
            .unwrap()
            .unwrap();

    let builder = ZCoinBuilder::new(
        ctx,
        ticker,
        conf,
        params,
        priv_key_policy,
        Some(z_spending_key),
        protocol_info,
    )?;

    builder.build().await
}

/// Build asset `ZCoin` for unit tests.
async fn z_coin_from_spending_key_for_unit_test(spending_key: &str) -> (MmArc, ZCoin) {
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let mut conf = zombie_conf();
    let params = ZcoinActivationParams {
        mode: ZcoinRpcMode::UnitTests,
        ..Default::default()
    };
    let pk_data = [1; 32];
    let protocol_info = match serde_json::from_value::<CoinProtocol>(conf["protocol"].take()).unwrap() {
        CoinProtocol::ZHTLC(protocol_info) => protocol_info,
        other_protocol => panic!("Failed to get protocol from config: {:?}", other_protocol),
    };

    let coin = z_coin_from_conf_and_params_for_tests(
        &ctx,
        "ZOMBIE",
        &conf,
        &params,
        PrivKeyBuildPolicy::IguanaPrivKey(pk_data.into()),
        protocol_info,
        spending_key,
    )
    .await
    .unwrap();
    (ctx, coin)
}

fn add_test_spend<P: Parameters, R: RngCore>(coin: &ZCoin, tx_builder: &mut ZTxBuilder<P, R>, amount: u64) {
    let extsk = coin.z_fields.z_spending_key.clone();
    let extfvk = coin.z_fields.evk.clone();
    let to = extfvk.default_address().unwrap().1;
    let mut rng = OsRng;
    let note1 = to
        .create_note(amount, Rseed::BeforeZip212(jubjub::Fr::random(&mut rng)))
        .unwrap();
    let cmu1 = Node::new(note1.cmu().to_repr());
    let mut tree = CommitmentTree::empty();
    tree.append(cmu1).unwrap();
    let witness1 = IncrementalWitness::from_tree(&tree);

    tx_builder
        .add_sapling_spend(extsk, *to.diversifier(), note1, witness1.path().unwrap())
        .unwrap();
}

async fn validate_fee_caller(
    coin: &ZCoin,
    dex_params: (PaymentAddress, u64),
    burn_params: Option<(PaymentAddress, u64)>,
    dex_fee: &DexFee,
) -> ValidatePaymentResult<()> {
    let uuid = &[1; 16];
    let mut z_outputs = vec![];
    let mut tx_builder = ZTxBuilder::new(coin.consensus_params(), BlockHeight::from_u32(1));

    add_test_spend(
        coin,
        &mut tx_builder,
        dex_params.1
            + if let Some(ref burn_params) = burn_params {
                burn_params.1
            } else {
                0
            }
            + u64::from(DEFAULT_FEE),
    );

    let dex_fee_out = ZOutput {
        to_addr: dex_params.0,
        amount: Amount::from_u64(dex_params.1).unwrap(),
        viewing_key: Some(DEX_FEE_OVK),
        memo: Some(MemoBytes::from_bytes(uuid).expect("uuid length < 512")),
    };
    z_outputs.push(dex_fee_out);

    // add output to the dex burn address:
    if let Some(burn_params) = burn_params {
        let dex_burn_out = ZOutput {
            to_addr: burn_params.0,
            amount: Amount::from_u64(burn_params.1).unwrap(),
            viewing_key: Some(DEX_FEE_OVK),
            memo: Some(MemoBytes::from_bytes(uuid).expect("uuid length < 512")),
        };
        z_outputs.push(dex_burn_out);
    }
    for z_out in z_outputs {
        tx_builder
            .add_sapling_output(z_out.viewing_key, z_out.to_addr, z_out.amount, z_out.memo)
            .unwrap();
    }
    let (tx, _) = async_blocking({
        let prover = coin.z_fields.z_tx_prover.clone();
        move || tx_builder.build(BranchId::Sapling, prover.as_ref())
    })
    .await
    .unwrap();

    let tx: TransactionEnum = tx.into();
    let tx_ret = tx.clone();
    ElectrumClient::get_verbose_transaction.mock_safe(move |_, txid| {
        let bytes: BytesJson = tx_ret.tx_hex().into();
        MockResult::Return(Box::new(futures01::future::ok(RpcTransaction {
            txid: *txid,
            hash: None,
            blockhash: H256Json::default(),
            confirmations: 0,
            time: 0,
            blocktime: 0,
            hex: bytes,
            vout: vec![],
            vin: vec![],
            size: None,
            version: 4,
            locktime: 0,
            vsize: None,
            rawconfirmations: None,
            height: None,
        })))
    });
    let validate_fee_args = ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: &[],
        dex_fee,
        min_block_number: 1,
        uuid: &[1; 16],
    };
    coin.validate_fee(validate_fee_args).await
}

#[test]
fn derive_z_key_from_mm_seed() {
    use crypto::privkey::key_pair_from_seed;
    use zcash_client_backend::encoding::encode_extended_spending_key;

    let seed = "spice describe gravity federal blast come thank unfair canal monkey style afraid";
    let secp_keypair = key_pair_from_seed(seed).unwrap();
    let z_spending_key = ExtendedSpendingKey::master(&*secp_keypair.private().secret);
    let encoded = encode_extended_spending_key(z_mainnet_constants::HRP_SAPLING_EXTENDED_SPENDING_KEY, &z_spending_key);
    assert_eq!(encoded, "secret-extended-key-main1qqqqqqqqqqqqqqytwz2zjt587n63kyz6jawmflttqu5rxavvqx3lzfs0tdr0w7g5tgntxzf5erd3jtvva5s52qx0ms598r89vrmv30r69zehxy2r3vesghtqd6dfwdtnauzuj8u8eeqfx7qpglzu6z54uzque6nzzgnejkgq569ax4lmk0v95rfhxzxlq3zrrj2z2kqylx2jp8g68lqu6alczdxd59lzp4hlfuj3jp54fp06xsaaay0uyass992g507tdd7psua5w6q76dyq3");

    let (_, address) = z_spending_key.default_address().unwrap();
    let encoded_addr = encode_payment_address(z_mainnet_constants::HRP_SAPLING_PAYMENT_ADDRESS, &address);
    assert_eq!(
        encoded_addr,
        "zs182ht30wnnnr8jjhj2j9v5dkx3qsknnr5r00jfwk2nczdtqy7w0v836kyy840kv2r8xle5gcl549"
    );

    let seed = "also shoot benefit prefer juice shell elder veteran woman mimic image kidney";
    let secp_keypair = key_pair_from_seed(seed).unwrap();
    let z_spending_key = ExtendedSpendingKey::master(&*secp_keypair.private().secret);
    let encoded = encode_extended_spending_key(z_mainnet_constants::HRP_SAPLING_EXTENDED_SPENDING_KEY, &z_spending_key);
    assert_eq!(encoded, "secret-extended-key-main1qqqqqqqqqqqqqq8jnhc9stsqwts6pu5ayzgy4szplvy03u227e50n3u8e6dwn5l0q5s3s8xfc03r5wmyh5s5dq536ufwn2k89ngdhnxy64sd989elwas6kr7ygztsdkw6k6xqyvhtu6e0dhm4mav8rus0fy8g0hgy9vt97cfjmus0m2m87p4qz5a00um7gwjwk494gul0uvt3gqyjujcclsqry72z57kr265jsajactgfn9m3vclqvx8fsdnwp4jwj57ffw560vvwks9g9hpu");

    let (_, address) = z_spending_key.default_address().unwrap();
    let encoded_addr = encode_payment_address(z_mainnet_constants::HRP_SAPLING_PAYMENT_ADDRESS, &address);
    assert_eq!(
        encoded_addr,
        "zs1funuwrjr2stlr6fnhkdh7fyz3p7n0p8rxase9jnezdhc286v5mhs6q3myw0phzvad5mvqgfxpam"
    );
}

#[test]
fn test_interpret_memo_string() {
    use std::str::FromStr;
    use zcash_primitives::memo::Memo;

    let actual = interpret_memo_string("68656c6c6f207a63617368").unwrap();
    let expected = Memo::from_str("68656c6c6f207a63617368").unwrap().encode();
    assert_eq!(actual, expected);

    let actual = interpret_memo_string("A custom memo").unwrap();
    let expected = Memo::from_str("A custom memo").unwrap().encode();
    assert_eq!(actual, expected);

    let actual = interpret_memo_string("0x68656c6c6f207a63617368").unwrap();
    let expected = MemoBytes::from_bytes(&hex::decode("68656c6c6f207a63617368").unwrap()).unwrap();
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn test_validate_zcoin_dex_fee() {
    let (_ctx, coin) = z_coin_from_spending_key_for_unit_test("secret-extended-key-main1qvqstxphqyqqpqqnh3hstqpdjzkpadeed6u7fz230jmm2mxl0aacrtu9vt7a7rmr2w5az5u79d24t0rudak3newknrz5l0m3dsd8m4dffqh5xwyldc5qwz8pnalrnhlxdzf900x83jazc52y25e9hvyd4kepaze6nlcvk8sd8a4qjh3e9j5d6730t7ctzhhrhp0zljjtwuptadnksxf8a8y5axwdhass5pjaxg0hzhg7z25rx0rll7a6txywl32s6cda0s5kexr03uqdtelwe").await;

    let std_fee = DexFee::Standard("0.001".into());
    let with_burn = DexFee::WithBurn {
        fee_amount: "0.0075".into(),
        burn_amount: "0.0025".into(),
        burn_destination: DexFeeBurnDestination::PreBurnAccount,
    };
    assert!(
        validate_fee_caller(&coin, (coin.z_fields.dex_fee_addr.clone(), 100000), None, &std_fee)
            .await
            .is_ok()
    );
    assert!(validate_fee_caller(
        &coin,
        (coin.z_fields.dex_fee_addr.clone(), 750000),
        Some((coin.z_fields.dex_burn_addr.clone(), 250000)),
        &with_burn
    )
    .await
    .is_ok());
    // try reverted addresses
    assert!(validate_fee_caller(
        &coin,
        (coin.z_fields.dex_burn_addr.clone(), 750000),
        Some((coin.z_fields.dex_fee_addr.clone(), 250000)),
        &with_burn
    )
    .await
    .is_err());
    let other_addr = decode_payment_address(
        coin.z_fields.consensus_params.hrp_sapling_payment_address(),
        "zs182ht30wnnnr8jjhj2j9v5dkx3qsknnr5r00jfwk2nczdtqy7w0v836kyy840kv2r8xle5gcl549",
    )
    .expect("valid z address format")
    .expect("valid z address");
    // try invalid dex address
    assert!(validate_fee_caller(
        &coin,
        (other_addr.clone(), 750000),
        Some((coin.z_fields.dex_burn_addr.clone(), 250000)),
        &with_burn
    )
    .await
    .is_err());
    // try invalid burn address
    assert!(validate_fee_caller(
        &coin,
        (coin.z_fields.dex_fee_addr.clone(), 750000),
        Some((other_addr.clone(), 250000)),
        &with_burn
    )
    .await
    .is_err());
}
