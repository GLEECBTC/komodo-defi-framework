#[cfg(not(target_arch = "wasm32"))]
use super::*;
#[cfg(not(target_arch = "wasm32"))]
use crate::CoinsContext;
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
#[cfg(not(target_arch = "wasm32"))]
use mm2_test_helpers::for_tests::{eth_sepolia_conf, ETH_SEPOLIA_SWAP_CONTRACT};

lazy_static! {
    static ref MM_CTX: MmArc = MmCtxBuilder::new().into_mm_arc();
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn eth_coin_for_test(
    coin_type: EthCoinType,
    urls: &[&str],
    fallback_swap_contract: Option<Address>,
    chain_id: u64,
) -> (MmArc, EthCoin) {
    let key_pair = KeyPair::from_secret_slice(
        &hex::decode("809465b17d0a4ddb3e4c69e8f23c2cabad868f51f8bed5c765ad1d6516c3306f").unwrap(),
    )
    .unwrap();
    eth_coin_from_keypair(
        coin_type,
        urls,
        fallback_swap_contract,
        key_pair,
        chain_id,
        eth_sepolia_conf(),
    )
}

/// Helper to create an EthCoin for testing with the given parameters
#[cfg(not(target_arch = "wasm32"))]
fn make_eth_coin(
    ctx: &MmArc,
    coin_type: EthCoinType,
    ticker: String,
    key_pair: KeyPair,
    chain_id: u64,
    fallback_swap_contract: Option<Address>,
    web3_instances: Vec<Web3Instance>,
) -> EthCoin {
    let platform_ticker = match &coin_type {
        EthCoinType::Eth => "ETH".to_string(),
        EthCoinType::Erc20 { platform, .. } | EthCoinType::Nft { platform } => platform.to_string(),
    };
    let my_address = ChainTaggedAddress::new(key_pair.address(), ChainFamily::Evm);
    let coin_conf = coin_conf(ctx, &ticker);
    let max_eth_tx_type = get_conf_param_or_from_plaform_coin(ctx, &coin_conf, &coin_type, MAX_ETH_TX_TYPE_SUPPORTED)
        .expect("valid max_eth_tx_type config");
    let gas_price_adjust = get_conf_param_or_from_plaform_coin(ctx, &coin_conf, &coin_type, GAS_PRICE_ADJUST)
        .expect("expected valid gas adjust config");
    let estimate_gas_mult = get_conf_param_or_from_plaform_coin(ctx, &coin_conf, &coin_type, ESTIMATE_GAS_MULT)
        .expect("expected valid estimate gas mult config");
    let gas_limit: EthGasLimit = get_conf_param_or_from_plaform_coin(ctx, &coin_conf, &coin_type, EthGasLimit::key())
        .expect("expected valid gas_limit config")
        .unwrap_or_default();
    let gas_limit_v2: EthGasLimitV2 =
        get_conf_param_or_from_plaform_coin(ctx, &coin_conf, &coin_type, EthGasLimitV2::key())
            .expect("expected valid gas_limit config")
            .unwrap_or_default();
    let swap_gas_fee_policy: SwapGasFeePolicy =
        get_conf_param_or_from_plaform_coin(ctx, &coin_conf, &coin_type, SWAP_GAS_FEE_POLICY)
            .expect("valid swap_gas_fee_policy config")
            .unwrap_or_default();

    EthCoin(Arc::new(EthCoinImpl {
        coin_type,
        chain_spec: ChainSpec::Evm { chain_id },
        decimals: 18,
        history_sync_state: Mutex::new(HistorySyncState::NotEnabled),
        sign_message_prefix: Some(String::from("Ethereum Signed Message:\n")),
        priv_key_policy: key_pair.into(),
        derivation_method: Arc::new(DerivationMethod::SingleAddress(my_address)),
        swap_contract_address: Address::from_str(ETH_SEPOLIA_SWAP_CONTRACT).unwrap(),
        swap_v2_contracts: None,
        fallback_swap_contract,
        contract_supports_watchers: false,
        ticker,
        web3_instances: AsyncMutex::new(web3_instances),
        rpc_client: None,
        ctx: ctx.weak(),
        required_confirmations: 1.into(),
        swap_gas_fee_policy: Mutex::new(swap_gas_fee_policy),
        trezor_coin: None,
        logs_block_range: DEFAULT_LOGS_BLOCK_RANGE,
        address_nonce_locks: PerNetNonceLocks::get_net_locks(platform_ticker),
        max_eth_tx_type,
        gas_price_adjust,
        erc20_tokens_infos: Default::default(),
        nfts_infos: Arc::new(Default::default()),
        gas_limit,
        gas_limit_v2,
        estimate_gas_mult,
        abortable_system: AbortableQueue::default(),
    }))
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn eth_coin_from_keypair(
    coin_type: EthCoinType,
    urls: &[&str],
    fallback_swap_contract: Option<Address>,
    key_pair: KeyPair,
    chain_id: u64,
    coin_conf_json: Json,
) -> (MmArc, EthCoin) {
    let conf = json!({ "coins": [coin_conf_json] });
    let ctx = MmCtxBuilder::new().with_conf(conf).into_mm_arc();

    // For ERC20/NFT tokens, register the platform coin first so platform_coin() lookups work
    if let EthCoinType::Erc20 { ref platform, .. } | EthCoinType::Nft { ref platform } = coin_type {
        let platform_coin = make_eth_coin(
            &ctx,
            EthCoinType::Eth,
            platform.clone(),
            key_pair.clone(),
            chain_id,
            fallback_swap_contract,
            vec![],
        );
        let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
        common::block_on(coins_ctx.add_platform_with_tokens(platform_coin.into(), vec![], None)).unwrap();
    }

    let web3_instances: Vec<_> = urls
        .iter()
        .map(|url| {
            let node = HttpTransportNode {
                uri: url.parse().unwrap(),
                komodo_proxy: false,
            };
            Web3Instance(Web3::new(Web3Transport::new_http(node)))
        })
        .collect();

    let ticker = match &coin_type {
        EthCoinType::Erc20 { .. } => "JST".to_string(),
        EthCoinType::Nft { platform } => platform.to_string(),
        _ => coin_type.to_string(),
    };

    let eth_coin = make_eth_coin(
        &ctx,
        coin_type,
        ticker,
        key_pair,
        chain_id,
        fallback_swap_contract,
        web3_instances,
    );
    (ctx, eth_coin)
}
