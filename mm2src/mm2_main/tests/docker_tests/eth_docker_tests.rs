use super::helpers::env::random_secp256k1_secret;
use super::helpers::eth::{
    erc20_coin_with_random_privkey, erc20_contract, erc20_contract_checksum, eth_coin_with_random_privkey,
    eth_coin_with_random_privkey_using_urls, fill_erc20, fill_eth, geth_account, geth_erc1155_contract,
    geth_erc721_contract, geth_maker_swap_v2, geth_nft_maker_swap_v2, geth_taker_swap_v2, geth_usdt_contract,
    swap_contract, swap_contract_checksum, usdt_coin_with_random_privkey, GETH_DEV_CHAIN_ID, GETH_NONCE_LOCK,
    GETH_RPC_URL, GETH_WEB3, MM_CTX, MM_CTX1,
};
use crate::common::Future01CompatExt;
use bitcrypto::{dhash160, sha256};
use coins::eth::erc20::get_erc20_token_info;
use coins::eth::gas_limit::ETH_MAX_TRADE_GAS;
use coins::eth::v2_activation::{eth_coin_from_conf_and_request_v2, EthActivationV2Request, EthNode};
use coins::eth::{
    eth_coin_from_conf_and_request, ChainSpec, EthCoin, EthCoinType, EthPrivKeyBuildPolicy, SignedEthTx,
    SwapV2Contracts,
};
use coins::hd_wallet::AddrToString;
use coins::nft::nft_structs::{Chain, ContractType, NftInfo};
use coins::{
    lp_coinfind, lp_register_coin, CoinProtocol, CoinWithDerivationMethod, CoinsContext, CommonSwapOpsV2,
    ConfirmPaymentInput, Eip1559Ops, FoundSwapTxSpend, MakerNftSwapOpsV2, MarketCoinOps, MmCoinEnum, NftSwapInfo,
    ParseCoinAssocTypes, ParseNftAssocTypes, PrivKeyBuildPolicy, RefundNftMakerPaymentArgs, RefundPaymentArgs,
    RegisterCoinParams, SearchForSwapTxSpendInput, SendNftMakerPaymentArgs, SendPaymentArgs, SpendNftMakerPaymentArgs,
    SpendPaymentArgs, SwapGasFeePolicy, SwapOps, SwapTxTypeWithSecretHash, ToBytes, Transaction,
    ValidateNftMakerPaymentArgs,
};
use coins::{
    DexFee, FundingTxSpend, GenTakerFundingSpendArgs, GenTakerPaymentSpendArgs, MakerCoinSwapOpsV2,
    RefundFundingSecretArgs, RefundMakerPaymentSecretArgs, RefundMakerPaymentTimelockArgs, RefundTakerPaymentArgs,
    SendMakerPaymentArgs, SendTakerFundingArgs, SpendMakerPaymentArgs, TakerCoinSwapOpsV2, TxPreimageWithSig,
    ValidateMakerPaymentArgs, ValidateTakerFundingArgs,
};
use common::{block_on, block_on_f01, now_sec};
use crypto::Secp256k1Secret;
use ethereum_types::U256;
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_number::{BigDecimal, BigUint};
use mm2_test_helpers::for_tests::{
    account_balance, active_swaps, check_recent_swaps, coins_needed_for_kickstart, disable_coin, enable_erc20_token_v2,
    enable_eth_coin_with_tokens_v2, erc20_dev_conf, eth_dev_conf, get_locked_amount, get_new_address, get_token_info,
    mm_dump, my_balance, my_swap_status, nft_dev_conf, start_swaps, task_enable_eth_with_tokens,
    wait_for_swap_finished, MarketMakerIt, Mm2TestConf, SwapV2TestContracts, TestNode,
};
use mm2_test_helpers::structs::{
    Bip44Chain, EnableCoinBalanceMap, EthWithTokensActivationResult, HDAccountAddressId, TokenInfo,
};
use num_traits::FromPrimitive;
use serde_json::{json, Value as Json};
use std::str::FromStr;
use std::thread;
use std::time::Duration;
use uuid::Uuid;
use web3::contract::{Contract, Options};
use web3::ethabi::Token;
use web3::types::{Address, H256};

const NFT_ETH: &str = "NFT_ETH";
const ETH: &str = "ETH";
const ERC20DEV: &str = "ERC20DEV";

/// ERC721_TEST_TOKEN has additional mint function
/// https://github.com/KomodoPlatform/etomic-swap/blob/public-mint-nft-functions/contracts/Erc721Token.sol (see public-mint-nft-functions branch)
const ERC721_TEST_ABI: &str = include_str!("../../../mm2_test_helpers/dummy_files/erc721_test_abi.json");
/// ERC1155_TEST_TOKEN has additional mint function
/// https://github.com/KomodoPlatform/etomic-swap/blob/public-mint-nft-functions/contracts/Erc1155Token.sol (see public-mint-nft-functions branch)
const ERC1155_TEST_ABI: &str = include_str!("../../../mm2_test_helpers/dummy_files/erc1155_test_abi.json");

fn wait_for_confirmation(tx_hash: H256) {
    thread::sleep(Duration::from_millis(2000));
    loop {
        match block_on(GETH_WEB3.eth().transaction_receipt(tx_hash)) {
            Ok(Some(r)) => match r.block_hash {
                Some(_) => break,
                None => thread::sleep(Duration::from_millis(100)),
            },
            _ => {
                thread::sleep(Duration::from_millis(100));
            },
        }
    }
}

fn mint_erc721(to_addr: Address, token_id: U256) {
    let _guard = GETH_NONCE_LOCK.lock().unwrap();
    let erc721_contract =
        Contract::from_json(GETH_WEB3.eth(), geth_erc721_contract(), ERC721_TEST_ABI.as_bytes()).unwrap();

    let options = Options {
        gas: Some(U256::from(ETH_MAX_TRADE_GAS)),
        ..Options::default()
    };

    let tx_hash = block_on(erc721_contract.call(
        "mint",
        (Token::Address(to_addr), Token::Uint(token_id)),
        geth_account(),
        options,
    ))
    .unwrap();
    wait_for_confirmation(tx_hash);

    let owner: Address =
        block_on(erc721_contract.query("ownerOf", Token::Uint(token_id), None, Options::default(), None)).unwrap();

    assert_eq!(
        owner, to_addr,
        "The ownership of the tokenID {token_id:?} does not match the expected address {to_addr:?}."
    );
}

fn geth_erc712_owner(token_id: U256) -> Address {
    let _guard = GETH_NONCE_LOCK.lock().unwrap();
    let erc721_contract =
        Contract::from_json(GETH_WEB3.eth(), geth_erc721_contract(), ERC721_TEST_ABI.as_bytes()).unwrap();
    block_on(erc721_contract.query("ownerOf", Token::Uint(token_id), None, Options::default(), None)).unwrap()
}

fn mint_erc1155(to_addr: Address, token_id: U256, amount: u32) {
    let _guard = GETH_NONCE_LOCK.lock().unwrap();
    let erc1155_contract =
        Contract::from_json(GETH_WEB3.eth(), geth_erc1155_contract(), ERC1155_TEST_ABI.as_bytes()).unwrap();

    let tx_hash = block_on(erc1155_contract.call(
        "mint",
        (
            Token::Address(to_addr),
            Token::Uint(token_id),
            Token::Uint(U256::from(amount)),
            Token::Bytes("".into()),
        ),
        geth_account(),
        Options::default(),
    ))
    .unwrap();
    wait_for_confirmation(tx_hash);

    // Check the balance of the token for the to_addr
    let balance: U256 = block_on(erc1155_contract.query(
        "balanceOf",
        (Token::Address(to_addr), Token::Uint(token_id)),
        None,
        Options::default(),
        None,
    ))
    .unwrap();

    // check that "balanceOf" from ERC11155 returns the exact amount of token without any decimals or scaling factors
    let balance_dec = balance.to_string().parse::<BigDecimal>().unwrap();
    assert_eq!(
        balance_dec,
        BigDecimal::from(amount),
        "The balance of tokenId {token_id:?} for address {to_addr:?} does not match the expected amount {amount:?}."
    );
}

fn geth_erc1155_balance(wallet_addr: Address, token_id: U256) -> U256 {
    let _guard = GETH_NONCE_LOCK.lock().unwrap();
    let erc1155_contract =
        Contract::from_json(GETH_WEB3.eth(), geth_erc1155_contract(), ERC1155_TEST_ABI.as_bytes()).unwrap();
    block_on(erc1155_contract.query(
        "balanceOf",
        (Token::Address(wallet_addr), Token::Uint(token_id)),
        None,
        Options::default(),
        None,
    ))
    .unwrap()
}

pub(crate) async fn fill_erc1155_info(eth_coin: &EthCoin, token_address: Address, token_id: u32, amount: u32) {
    let nft_infos_lock = eth_coin.nfts_infos.clone();
    let mut nft_infos = nft_infos_lock.lock().await;

    let erc1155_nft_info = NftInfo {
        token_address,
        token_id: BigUint::from(token_id),
        chain: Chain::Eth,
        contract_type: ContractType::Erc1155,
        amount: BigDecimal::from(amount),
    };
    let erc1155_address_str = token_address.addr_to_string();
    let erc1155_key = format!("{erc1155_address_str},{token_id}");
    nft_infos.insert(erc1155_key, erc1155_nft_info);
}

pub(crate) async fn fill_erc721_info(eth_coin: &EthCoin, token_address: Address, token_id: u32) {
    let nft_infos_lock = eth_coin.nfts_infos.clone();
    let mut nft_infos = nft_infos_lock.lock().await;

    let erc721_nft_info = NftInfo {
        token_address,
        token_id: BigUint::from(token_id),
        chain: Chain::Eth,
        contract_type: ContractType::Erc721,
        amount: BigDecimal::from(1),
    };
    let erc721_address_str = token_address.addr_to_string();
    let erc721_key = format!("{erc721_address_str},{token_id}");
    nft_infos.insert(erc721_key, erc721_nft_info);
}

#[derive(Clone, Copy, Debug)]
pub enum TestNftType {
    Erc1155 { token_id: u32, amount: u32 },
    Erc721 { token_id: u32 },
}

/// Generates a global NFT coin instance with a random private key and an initial 100 ETH balance.
/// Optionally mints a specified NFT (either ERC721 or ERC1155) to the global NFT address,
/// with details recorded in the `nfts_infos` field based on the provided `nft_type`.
fn global_nft_with_random_privkey(
    swap_v2_contracts: SwapV2Contracts,
    swap_contract_address: Address,
    fallback_swap_contract_address: Address,
    nft_type: Option<TestNftType>,
    nft_ticker: String,
    platform_ticker: String,
) -> EthCoin {
    // Register platform ETH coin in MM_CTX1 if not already registered.
    // Required because NFT coins call platform_coin() for get_swap_gas_fee_policy().
    if block_on(lp_coinfind(&MM_CTX1, &platform_ticker))
        .ok()
        .flatten()
        .is_none()
    {
        let eth_conf = eth_dev_conf();
        let eth_req = json!({
            "urls": [GETH_RPC_URL],
            "swap_contract_address": swap_contract_address,
            "swap_v2_contracts": {
                "maker_swap_v2_contract": swap_v2_contracts.maker_swap_v2_contract,
                "taker_swap_v2_contract": swap_v2_contracts.taker_swap_v2_contract,
                "nft_maker_swap_v2_contract": swap_v2_contracts.nft_maker_swap_v2_contract
            },
            "fallback_swap_contract": fallback_swap_contract_address
        });
        let platform_priv_key_policy = PrivKeyBuildPolicy::IguanaPrivKey(Secp256k1Secret::from([1u8; 32]));
        let platform_coin = block_on(eth_coin_from_conf_and_request(
            &MM_CTX1,
            &platform_ticker,
            &eth_conf,
            &eth_req,
            CoinProtocol::ETH {
                chain_id: GETH_DEV_CHAIN_ID,
            },
            platform_priv_key_policy,
        ))
        .unwrap();
        let coins_ctx = CoinsContext::from_ctx(&MM_CTX1).unwrap();
        // Ignore error if another parallel test already registered the platform
        let _ = block_on(coins_ctx.add_platform_with_tokens(platform_coin.into(), vec![], None));
    }

    let build_policy = EthPrivKeyBuildPolicy::IguanaPrivKey(random_secp256k1_secret());
    let node = EthNode {
        url: GETH_RPC_URL.to_string(),
        komodo_proxy: false,
    };
    let platform_request = EthActivationV2Request {
        nodes: vec![node],
        rpc_mode: Default::default(),
        swap_contract_address: Some(swap_contract_address),
        swap_v2_contracts: Some(swap_v2_contracts),
        fallback_swap_contract: Some(fallback_swap_contract_address),
        contract_supports_watchers: false,
        mm2: None,
        required_confirmations: None,
        priv_key_policy: Default::default(),
        enable_params: Default::default(),
        path_to_address: Default::default(),
        gap_limit: None,
        swap_gas_fee_policy: None,
    };
    let coin = block_on(eth_coin_from_conf_and_request_v2(
        &MM_CTX1,
        nft_ticker.as_str(),
        &nft_dev_conf(),
        platform_request,
        build_policy,
        ChainSpec::Evm {
            chain_id: GETH_DEV_CHAIN_ID,
        },
    ))
    .unwrap();

    let coin_type = EthCoinType::Nft {
        platform: platform_ticker,
    };
    // NFT coins use ETH decimals (18)
    let global_nft = block_on(coin.set_coin_type(coin_type, 18));
    let my_address = block_on(coin.my_addr());
    fill_eth(my_address, U256::from(10).pow(U256::from(20)));

    if let Some(nft_type) = nft_type {
        match nft_type {
            TestNftType::Erc1155 { token_id, amount } => {
                mint_erc1155(my_address, U256::from(token_id), amount);
                block_on(fill_erc1155_info(
                    &global_nft,
                    geth_erc1155_contract(),
                    token_id,
                    amount,
                ));
            },
            TestNftType::Erc721 { token_id } => {
                mint_erc721(my_address, U256::from(token_id));
                block_on(fill_erc721_info(&global_nft, geth_erc721_contract(), token_id));
            },
        }
    }
    global_nft
}

fn send_and_refund_eth_maker_payment_impl(swap_txfee_policy: SwapGasFeePolicy) {
    thread::sleep(Duration::from_secs(3));
    let eth_coin = eth_coin_with_random_privkey(swap_contract());
    assert!(block_on(eth_coin.set_swap_gas_fee_policy(swap_txfee_policy)).is_ok());

    let time_lock = now_sec() - 100;
    let other_pubkey = &[
        0x02, 0xc6, 0x6e, 0x7d, 0x89, 0x66, 0xb5, 0xc5, 0x55, 0xaf, 0x58, 0x05, 0x98, 0x9d, 0xa9, 0xfb, 0xf8, 0xdb,
        0x95, 0xe1, 0x56, 0x31, 0xce, 0x35, 0x8c, 0x3a, 0x17, 0x10, 0xc9, 0x62, 0x67, 0x90, 0x63,
    ];

    let send_payment_args = SendPaymentArgs {
        time_lock_duration: 100,
        time_lock,
        other_pubkey,
        secret_hash: &[0; 20],
        amount: 1.into(),
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let eth_maker_payment = block_on(eth_coin.send_maker_payment(send_payment_args)).unwrap();

    let confirm_input = ConfirmPaymentInput {
        payment_tx: eth_maker_payment.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(eth_coin.wait_for_confirmations(confirm_input)).unwrap();

    let refund_args = RefundPaymentArgs {
        payment_tx: &eth_maker_payment.tx_hex(),
        time_lock,
        other_pubkey,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: &[0; 20],
        },
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let payment_refund = block_on(eth_coin.send_maker_refunds_payment(refund_args)).unwrap();
    log!("Payment refund tx hash {:02x}", payment_refund.tx_hash_as_bytes());

    let confirm_input = ConfirmPaymentInput {
        payment_tx: payment_refund.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(eth_coin.wait_for_confirmations(confirm_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: other_pubkey,
        secret_hash: &[0; 20],
        tx: &eth_maker_payment.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
    };
    let search_tx = block_on(eth_coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();

    let expected = FoundSwapTxSpend::Refunded(payment_refund);
    assert_eq!(expected, search_tx);
}

#[test]
fn send_and_refund_eth_maker_payment_internal_gas_policy() {
    send_and_refund_eth_maker_payment_impl(SwapGasFeePolicy::Legacy);
}

#[test]
fn send_and_refund_eth_maker_payment_priority_fee() {
    send_and_refund_eth_maker_payment_impl(SwapGasFeePolicy::Medium);
}

fn send_and_spend_eth_maker_payment_impl(swap_txfee_policy: SwapGasFeePolicy) {
    let maker_eth_coin = eth_coin_with_random_privkey(swap_contract());
    let taker_eth_coin = eth_coin_with_random_privkey(swap_contract());

    assert!(block_on(maker_eth_coin.set_swap_gas_fee_policy(swap_txfee_policy.clone())).is_ok());
    assert!(block_on(taker_eth_coin.set_swap_gas_fee_policy(swap_txfee_policy)).is_ok());

    let time_lock = now_sec() + 1000;
    let maker_pubkey = maker_eth_coin.derive_htlc_pubkey(&[]);
    let taker_pubkey = taker_eth_coin.derive_htlc_pubkey(&[]);
    let secret = &[1; 32];
    let secret_hash_owned = dhash160(secret);
    let secret_hash = secret_hash_owned.as_slice();

    let send_payment_args = SendPaymentArgs {
        time_lock_duration: 1000,
        time_lock,
        other_pubkey: &taker_pubkey,
        secret_hash,
        amount: 1.into(),
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let eth_maker_payment = block_on(maker_eth_coin.send_maker_payment(send_payment_args)).unwrap();

    let confirm_input = ConfirmPaymentInput {
        payment_tx: eth_maker_payment.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(taker_eth_coin.wait_for_confirmations(confirm_input)).unwrap();

    let spend_args = SpendPaymentArgs {
        other_payment_tx: &eth_maker_payment.tx_hex(),
        time_lock,
        other_pubkey: &maker_pubkey,
        secret,
        secret_hash,
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let payment_spend = block_on(taker_eth_coin.send_taker_spends_maker_payment(spend_args)).unwrap();
    log!("Payment spend tx hash {:02x}", payment_spend.tx_hash_as_bytes());

    let confirm_input = ConfirmPaymentInput {
        payment_tx: payment_spend.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(taker_eth_coin.wait_for_confirmations(confirm_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: &taker_pubkey,
        secret_hash,
        tx: &eth_maker_payment.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
    };
    let search_tx = block_on(maker_eth_coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();

    let expected = FoundSwapTxSpend::Spent(payment_spend);
    assert_eq!(expected, search_tx);
}

#[test]
fn send_and_spend_eth_maker_payment_internal_gas_policy() {
    send_and_spend_eth_maker_payment_impl(SwapGasFeePolicy::Legacy);
}

#[test]
fn send_and_spend_eth_maker_payment_priority_fee() {
    send_and_spend_eth_maker_payment_impl(SwapGasFeePolicy::Medium);
}

fn send_and_refund_erc20_maker_payment_impl(swap_txfee_policy: SwapGasFeePolicy) {
    thread::sleep(Duration::from_secs(10));
    let erc20_coin = erc20_coin_with_random_privkey(swap_contract());
    assert!(block_on(erc20_coin.set_swap_gas_fee_policy(swap_txfee_policy)).is_ok());

    let time_lock = now_sec() - 100;
    let other_pubkey = &[
        0x02, 0xc6, 0x6e, 0x7d, 0x89, 0x66, 0xb5, 0xc5, 0x55, 0xaf, 0x58, 0x05, 0x98, 0x9d, 0xa9, 0xfb, 0xf8, 0xdb,
        0x95, 0xe1, 0x56, 0x31, 0xce, 0x35, 0x8c, 0x3a, 0x17, 0x10, 0xc9, 0x62, 0x67, 0x90, 0x63,
    ];
    let secret_hash = &[1; 20];

    let send_payment_args = SendPaymentArgs {
        time_lock_duration: 100,
        time_lock,
        other_pubkey,
        secret_hash,
        amount: 1.into(),
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: now_sec() + 60,
    };
    let eth_maker_payment = block_on(erc20_coin.send_maker_payment(send_payment_args)).unwrap();

    let confirm_input = ConfirmPaymentInput {
        payment_tx: eth_maker_payment.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(erc20_coin.wait_for_confirmations(confirm_input)).unwrap();

    let refund_args = RefundPaymentArgs {
        payment_tx: &eth_maker_payment.tx_hex(),
        time_lock,
        other_pubkey,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: secret_hash,
        },
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let payment_refund = block_on(erc20_coin.send_maker_refunds_payment(refund_args)).unwrap();
    log!("Payment refund tx hash {:02x}", payment_refund.tx_hash_as_bytes());

    let confirm_input = ConfirmPaymentInput {
        payment_tx: payment_refund.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(erc20_coin.wait_for_confirmations(confirm_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: other_pubkey,
        secret_hash,
        tx: &eth_maker_payment.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
    };
    let search_tx = block_on(erc20_coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();

    let expected = FoundSwapTxSpend::Refunded(payment_refund);
    assert_eq!(expected, search_tx);
}

#[test]
fn send_and_refund_erc20_maker_payment_internal_gas_policy() {
    send_and_refund_erc20_maker_payment_impl(SwapGasFeePolicy::Legacy);
}

#[test]
fn send_and_refund_erc20_maker_payment_priority_fee() {
    send_and_refund_erc20_maker_payment_impl(SwapGasFeePolicy::Medium);
}

fn send_and_spend_erc20_maker_payment_impl(swap_txfee_policy: SwapGasFeePolicy) {
    thread::sleep(Duration::from_secs(7));
    let maker_erc20_coin = erc20_coin_with_random_privkey(swap_contract());
    let taker_erc20_coin = erc20_coin_with_random_privkey(swap_contract());

    assert!(block_on(maker_erc20_coin.set_swap_gas_fee_policy(swap_txfee_policy.clone())).is_ok());
    assert!(block_on(taker_erc20_coin.set_swap_gas_fee_policy(swap_txfee_policy)).is_ok());

    let time_lock = now_sec() + 1000;
    let maker_pubkey = maker_erc20_coin.derive_htlc_pubkey(&[]);
    let taker_pubkey = taker_erc20_coin.derive_htlc_pubkey(&[]);
    let secret = &[2; 32];
    let secret_hash_owned = dhash160(secret);
    let secret_hash = secret_hash_owned.as_slice();

    let send_payment_args = SendPaymentArgs {
        time_lock_duration: 1000,
        time_lock,
        other_pubkey: &taker_pubkey,
        secret_hash,
        amount: 1.into(),
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: now_sec() + 60,
    };
    let eth_maker_payment = block_on(maker_erc20_coin.send_maker_payment(send_payment_args)).unwrap();

    let confirm_input = ConfirmPaymentInput {
        payment_tx: eth_maker_payment.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(taker_erc20_coin.wait_for_confirmations(confirm_input)).unwrap();

    let spend_args = SpendPaymentArgs {
        other_payment_tx: &eth_maker_payment.tx_hex(),
        time_lock,
        other_pubkey: &maker_pubkey,
        secret,
        secret_hash,
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let payment_spend = block_on(taker_erc20_coin.send_taker_spends_maker_payment(spend_args)).unwrap();
    log!("Payment spend tx hash {:02x}", payment_spend.tx_hash_as_bytes());

    let confirm_input = ConfirmPaymentInput {
        payment_tx: payment_spend.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(taker_erc20_coin.wait_for_confirmations(confirm_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: &taker_pubkey,
        secret_hash,
        tx: &eth_maker_payment.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
    };
    let search_tx = block_on(maker_erc20_coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();

    let expected = FoundSwapTxSpend::Spent(payment_spend);
    assert_eq!(expected, search_tx);
}

#[test]
fn send_and_spend_erc20_maker_payment_internal_gas_policy() {
    send_and_spend_erc20_maker_payment_impl(SwapGasFeePolicy::Legacy);
}

#[test]
fn send_and_spend_erc20_maker_payment_priority_fee() {
    send_and_spend_erc20_maker_payment_impl(SwapGasFeePolicy::Medium);
}

#[test]
fn send_and_spend_erc721_maker_payment() {
    let token_id = 1u32;
    let time_lock = now_sec() + 1000;
    let activation = NftActivationV2Args::init();
    let setup = setup_test(
        token_id,
        None,
        ContractType::Erc721,
        geth_erc721_contract(),
        time_lock,
        activation,
    );

    let maker_payment = send_nft_maker_payment(&setup, 1.into());
    log!(
        "Maker sent ERC721 NFT payment, tx hash: {:02x}",
        maker_payment.tx_hash()
    );

    wait_for_confirmations(&setup.maker_global_nft, &maker_payment, 200);
    validate_nft_maker_payment(&setup, &maker_payment, 1.into());

    let spend_tx = spend_nft_maker_payment(&setup, &maker_payment, &ContractType::Erc721);
    log!(
        "Taker spent ERC721 NFT Maker payment, tx hash: {:02x}",
        spend_tx.tx_hash()
    );

    wait_for_confirmations(&setup.taker_global_nft, &spend_tx, 200);
    let new_owner = geth_erc712_owner(U256::from(token_id));
    let taker_address = block_on(setup.taker_global_nft.my_addr());
    assert_eq!(new_owner, taker_address);
}

#[test]
fn send_and_spend_erc1155_maker_payment() {
    let token_id = 1u32;
    let amount = 3u32;
    let time_lock = now_sec() + 1000;
    let activation = NftActivationV2Args::init();
    let setup = setup_test(
        token_id,
        Some(amount),
        ContractType::Erc1155,
        geth_erc1155_contract(),
        time_lock,
        activation,
    );

    let maker_address = block_on(setup.maker_global_nft.my_addr());
    let maker_balance = geth_erc1155_balance(maker_address, U256::from(token_id));
    assert_eq!(U256::from(amount), maker_balance);

    let swap_amount = 2u32;
    let maker_payment = send_nft_maker_payment(&setup, swap_amount.into());
    log!(
        "Maker sent ERC1155 NFT payment, tx hash: {:02x}",
        maker_payment.tx_hash()
    );

    wait_for_confirmations(&setup.maker_global_nft, &maker_payment, 100);

    validate_nft_maker_payment(&setup, &maker_payment, swap_amount.into());

    let spend_tx = spend_nft_maker_payment(&setup, &maker_payment, &ContractType::Erc1155);
    log!(
        "Taker spent ERC1155 NFT Maker payment, tx hash: {:02x}",
        spend_tx.tx_hash()
    );

    wait_for_confirmations(&setup.taker_global_nft, &spend_tx, 100);

    let taker_address = block_on(setup.taker_global_nft.my_addr());
    let taker_balance = geth_erc1155_balance(taker_address, U256::from(token_id));
    assert_eq!(U256::from(swap_amount), taker_balance);

    let maker_new_balance = geth_erc1155_balance(maker_address, U256::from(token_id));
    assert_eq!(U256::from(1u32), maker_new_balance);
}

#[test]
fn test_nonce_several_urls() {
    // Use one working and one failing URL.
    let coin = eth_coin_with_random_privkey_using_urls(swap_contract(), &[GETH_RPC_URL, "http://127.0.0.1:0"]);
    let my_address = block_on(coin.derivation_method().single_addr_or_err()).unwrap();
    let (old_nonce, _) = block_on_f01(coin.clone().get_addr_nonce(my_address.inner())).unwrap();

    // Send a payment to increase the nonce.
    block_on_f01(coin.send_to_address(my_address.inner(), 200000000.into())).unwrap();

    let (new_nonce, _) = block_on_f01(coin.get_addr_nonce(my_address.inner())).unwrap();
    assert_eq!(old_nonce + 1, new_nonce);
}

#[test]
fn test_nonce_lock() {
    use futures::future::join_all;

    let coin = eth_coin_with_random_privkey(swap_contract());
    let my_address = block_on(coin.derivation_method().single_addr_or_err()).unwrap();
    let futures = (0..5).map(|_| coin.send_to_address(my_address.inner(), 200000000.into()).compat());
    let results = block_on(join_all(futures));

    // make sure all transactions are successful
    for result in results {
        result.unwrap();
    }
}

/// Test to validate duplicate nonces for legacy token activation
/// https://github.com/KomodoPlatform/komodo-defi-framework/issues/2573
#[test]
fn test_nonce_erc20_lock() {
    use futures::future::join_all;

    let swap_addresses = SwapAddresses::init();
    let swap_contract_address = swap_addresses.swap_contract_address.addr_to_string();

    let eth_conf = eth_dev_conf();
    let erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    let eth_ticker = eth_conf["coin"].as_str().unwrap().to_owned();
    let erc20_ticker = erc20_conf["coin"].as_str().unwrap().to_owned();
    let erc20_contract = erc20_conf["protocol"]["protocol_data"]["contract_address"]
        .as_str()
        .unwrap()
        .to_owned();
    let ctx = MmCtxBuilder::new()
        .with_conf(json!({"coins":[eth_conf, erc20_conf]}))
        .into_mm_arc();

    let (eth_coin, privkey) =
        eth_coin_v2_activation_with_random_privkey(&ctx, &eth_ticker, &eth_conf, swap_addresses, false);
    block_on(lp_register_coin(
        &ctx,
        MmCoinEnum::EthCoinVariant(eth_coin.clone()),
        RegisterCoinParams {
            ticker: eth_ticker.clone(),
        },
    ))
    .unwrap();

    // Use legacy "enable" RPC for token to validate this issue
    let req_erc20 = json!({
        "method": "enable",
        "coin": erc20_ticker,
        "swap_contract_address": swap_contract_address,
        "urls": [ GETH_RPC_URL ]
    });
    let eth_token = block_on(eth_coin_from_conf_and_request(
        &ctx,
        &erc20_ticker,
        &erc20_conf,
        &req_erc20,
        CoinProtocol::ERC20 {
            platform: eth_ticker.clone(),
            contract_address: erc20_contract,
        },
        PrivKeyBuildPolicy::IguanaPrivKey(privkey),
    ))
    .unwrap();

    let my_address = block_on(eth_coin.derivation_method().single_addr_or_err()).unwrap();

    let futures = vec![
        eth_coin.send_to_address(my_address.inner(), 100.into()).compat(),
        eth_token.send_to_address(my_address.inner(), 1.into()).compat(),
        eth_token.send_to_address(my_address.inner(), 2.into()).compat(),
        eth_coin.send_to_address(my_address.inner(), 200.into()).compat(),
    ];
    let results = block_on(join_all(futures));

    // make sure all transactions are successful
    for result in results {
        result.unwrap();
    }
}

#[test]
fn send_and_refund_erc721_maker_payment_timelock() {
    let token_id = 2u32;
    let time_lock_to_refund = now_sec() - 1000;
    let activation = NftActivationV2Args::init();
    let setup = setup_test(
        token_id,
        None,
        ContractType::Erc721,
        geth_erc721_contract(),
        time_lock_to_refund,
        activation,
    );

    let maker_payment_to_refund = send_nft_maker_payment(&setup, 1.into());
    log!(
        "Maker sent ERC721 NFT payment, tx hash: {:02x}",
        maker_payment_to_refund.tx_hash()
    );

    wait_for_confirmations(&setup.maker_global_nft, &maker_payment_to_refund, 150);
    let current_owner = geth_erc712_owner(U256::from(token_id));
    assert_eq!(current_owner, geth_nft_maker_swap_v2());

    let refund_timelock_tx = refund_nft_maker_payment(
        &setup,
        &maker_payment_to_refund,
        &ContractType::Erc721,
        RefundType::Timelock,
    );
    log!(
        "Maker refunded ERC721 NFT payment after timelock, tx hash: {:02x}",
        refund_timelock_tx.tx_hash()
    );

    wait_for_confirmations(&setup.maker_global_nft, &refund_timelock_tx, 150);
    let current_owner = geth_erc712_owner(U256::from(token_id));
    let maker_address = block_on(setup.maker_global_nft.my_addr());
    assert_eq!(current_owner, maker_address);
}

#[test]
fn send_and_refund_erc1155_maker_payment_timelock() {
    let token_id = 2u32;
    let amount = 3u32;
    let time_lock_to_refund = now_sec() - 1000;
    let activation = NftActivationV2Args::init();
    let setup = setup_test(
        token_id,
        Some(amount),
        ContractType::Erc1155,
        geth_erc1155_contract(),
        time_lock_to_refund,
        activation,
    );

    let maker_address = block_on(setup.maker_global_nft.my_addr());
    let balance = geth_erc1155_balance(maker_address, U256::from(token_id));
    assert_eq!(U256::from(amount), balance);

    let swap_amount = 2u32;
    let maker_payment_to_refund = send_nft_maker_payment(&setup, swap_amount.into());
    log!(
        "Maker sent ERC1155 NFT payment, tx hash: {:02x}",
        maker_payment_to_refund.tx_hash()
    );

    wait_for_confirmations(&setup.maker_global_nft, &maker_payment_to_refund, 150);

    let swap_contract_balance = geth_erc1155_balance(geth_nft_maker_swap_v2(), U256::from(token_id));
    assert_eq!(U256::from(swap_amount), swap_contract_balance);
    let balance = geth_erc1155_balance(maker_address, U256::from(token_id));
    assert_eq!(U256::from(1u32), balance);

    let refund_timelock_tx = refund_nft_maker_payment(
        &setup,
        &maker_payment_to_refund,
        &ContractType::Erc1155,
        RefundType::Timelock,
    );
    log!(
        "Maker refunded ERC1155 NFT payment after timelock, tx hash: {:02x}",
        refund_timelock_tx.tx_hash()
    );

    wait_for_confirmations(&setup.maker_global_nft, &refund_timelock_tx, 150);

    let balance = geth_erc1155_balance(maker_address, U256::from(token_id));
    assert_eq!(U256::from(amount), balance);
}

#[test]
fn send_and_refund_erc721_maker_payment_secret() {
    let token_id = 3u32;
    let time_lock_to_refund = now_sec() + 1000;
    let activation = NftActivationV2Args::init();
    let setup = setup_test(
        token_id,
        None,
        ContractType::Erc721,
        geth_erc721_contract(),
        time_lock_to_refund,
        activation,
    );

    let maker_payment_to_refund = send_nft_maker_payment(&setup, 1.into());
    log!(
        "Maker sent ERC721 NFT payment, tx hash: {:02x}",
        maker_payment_to_refund.tx_hash()
    );

    wait_for_confirmations(&setup.maker_global_nft, &maker_payment_to_refund, 150);
    let current_owner = geth_erc712_owner(U256::from(token_id));
    assert_eq!(current_owner, geth_nft_maker_swap_v2());

    let refund_secret_tx = refund_nft_maker_payment(
        &setup,
        &maker_payment_to_refund,
        &ContractType::Erc721,
        RefundType::Secret,
    );
    log!(
        "Maker refunded ERC721 NFT payment using Taker secret, tx hash: {:02x}",
        refund_secret_tx.tx_hash()
    );

    wait_for_confirmations(&setup.maker_global_nft, &refund_secret_tx, 150);
    let current_owner = geth_erc712_owner(U256::from(token_id));
    let maker_address = block_on(setup.maker_global_nft.my_addr());
    assert_eq!(current_owner, maker_address);
}

#[test]
fn send_and_refund_erc1155_maker_payment_secret() {
    let token_id = 3u32;
    let amount = 3u32;
    let time_lock_to_refund = now_sec() + 1000;
    let activation = NftActivationV2Args::init();
    let setup = setup_test(
        token_id,
        Some(amount),
        ContractType::Erc1155,
        geth_erc1155_contract(),
        time_lock_to_refund,
        activation,
    );

    let maker_address = block_on(setup.maker_global_nft.my_addr());
    let balance = geth_erc1155_balance(maker_address, U256::from(token_id));
    assert_eq!(U256::from(amount), balance);

    let swap_amount = 2u32;
    let maker_payment_to_refund = send_nft_maker_payment(&setup, swap_amount.into());
    log!(
        "Maker sent ERC1155 NFT payment, tx hash: {:02x}",
        maker_payment_to_refund.tx_hash()
    );

    wait_for_confirmations(&setup.maker_global_nft, &maker_payment_to_refund, 100);

    let swap_contract_balance = geth_erc1155_balance(geth_nft_maker_swap_v2(), U256::from(token_id));
    assert_eq!(U256::from(swap_amount), swap_contract_balance);
    let balance = geth_erc1155_balance(maker_address, U256::from(token_id));
    assert_eq!(U256::from(1u32), balance);

    let refund_secret_tx = refund_nft_maker_payment(
        &setup,
        &maker_payment_to_refund,
        &ContractType::Erc1155,
        RefundType::Secret,
    );
    log!(
        "Maker refunded ERC1155 NFT payment using Taker secret, tx hash: {:02x}",
        refund_secret_tx.tx_hash()
    );

    wait_for_confirmations(&setup.maker_global_nft, &refund_secret_tx, 100);

    let balance = geth_erc1155_balance(maker_address, U256::from(token_id));
    assert_eq!(U256::from(amount), balance);
}

struct NftTestSetup {
    maker_global_nft: EthCoin,
    taker_global_nft: EthCoin,
    nft_swap_info: TestNftSwapInfo<EthCoin>,
    maker_secret: Vec<u8>,
    maker_secret_hash: Vec<u8>,
    taker_secret: Vec<u8>,
    taker_secret_hash: Vec<u8>,
    time_lock: u64,
}

/// Structure representing necessary NFT info for Swap
pub struct TestNftSwapInfo<Coin: ParseNftAssocTypes + ?Sized> {
    /// The address of the NFT token
    pub token_address: Coin::ContractAddress,
    /// The ID of the NFT token.
    pub token_id: Vec<u8>,
    /// The type of smart contract that governs this NFT
    pub contract_type: Coin::ContractType,
}

struct NftActivationV2Args {
    swap_contract_address: Address,
    fallback_swap_contract_address: Address,
    swap_v2_contracts: SwapV2Contracts,
    nft_ticker: String,
    platform_ticker: String,
}

impl NftActivationV2Args {
    fn init() -> Self {
        Self {
            swap_contract_address: swap_contract(),
            fallback_swap_contract_address: swap_contract(),
            swap_v2_contracts: SwapV2Contracts {
                maker_swap_v2_contract: geth_maker_swap_v2(),
                taker_swap_v2_contract: geth_taker_swap_v2(),
                nft_maker_swap_v2_contract: geth_nft_maker_swap_v2(),
            },
            nft_ticker: NFT_ETH.to_string(),
            platform_ticker: "ETH".to_string(),
        }
    }
}

fn setup_test(
    token_id: u32,
    amount: Option<u32>,
    contract_type: ContractType,
    token_contract: Address,
    time_lock: u64,
    activation: NftActivationV2Args,
) -> NftTestSetup {
    let nft_type = match contract_type {
        ContractType::Erc721 => TestNftType::Erc721 { token_id },
        ContractType::Erc1155 => TestNftType::Erc1155 {
            token_id,
            amount: amount.unwrap(),
        },
    };

    let maker_global_nft = global_nft_with_random_privkey(
        activation.swap_v2_contracts,
        activation.swap_contract_address,
        activation.fallback_swap_contract_address,
        Some(nft_type),
        activation.nft_ticker.clone(),
        activation.platform_ticker.clone(),
    );
    let taker_global_nft = global_nft_with_random_privkey(
        activation.swap_v2_contracts,
        activation.swap_contract_address,
        activation.fallback_swap_contract_address,
        None,
        activation.nft_ticker,
        activation.platform_ticker,
    );
    let maker_secret = vec![1; 32];
    let maker_secret_hash = sha256(&maker_secret).to_vec();
    let taker_secret = vec![0; 32];
    let taker_secret_hash = sha256(&taker_secret).to_vec();

    let token_id = BigUint::from(token_id).to_bytes();

    let nft_swap_info = TestNftSwapInfo {
        token_address: token_contract,
        token_id,
        contract_type,
    };

    NftTestSetup {
        maker_global_nft,
        taker_global_nft,
        nft_swap_info,
        maker_secret,
        maker_secret_hash,
        taker_secret,
        taker_secret_hash,
        time_lock,
    }
}

fn send_nft_maker_payment(setup: &NftTestSetup, amount: BigDecimal) -> SignedEthTx {
    let nft_swap_info = NftSwapInfo {
        token_address: &setup.nft_swap_info.token_address,
        token_id: &setup.nft_swap_info.token_id,
        contract_type: &setup.nft_swap_info.contract_type,
    };
    let send_payment_args = SendNftMakerPaymentArgs::<EthCoin> {
        time_lock: setup.time_lock,
        taker_secret_hash: &setup.taker_secret_hash,
        maker_secret_hash: &setup.maker_secret_hash,
        amount,
        taker_pub: &setup.taker_global_nft.derive_htlc_pubkey_v2(&[]),
        swap_unique_data: &[],
        nft_swap_info: &nft_swap_info,
    };
    block_on(setup.maker_global_nft.send_nft_maker_payment_v2(send_payment_args)).unwrap()
}

fn wait_for_confirmations(coin: &EthCoin, tx: &SignedEthTx, wait_seconds: u64) {
    let confirm_input = ConfirmPaymentInput {
        payment_tx: tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + wait_seconds,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_input)).unwrap();
}

fn validate_nft_maker_payment(setup: &NftTestSetup, maker_payment: &SignedEthTx, amount: BigDecimal) {
    let nft_swap_info = NftSwapInfo {
        token_address: &setup.nft_swap_info.token_address,
        token_id: &setup.nft_swap_info.token_id,
        contract_type: &setup.nft_swap_info.contract_type,
    };
    let validate_args = ValidateNftMakerPaymentArgs {
        maker_payment_tx: maker_payment,
        time_lock: setup.time_lock,
        taker_secret_hash: &setup.taker_secret_hash,
        maker_secret_hash: &setup.maker_secret_hash,
        amount,
        taker_pub: &setup.taker_global_nft.derive_htlc_pubkey_v2(&[]),
        maker_pub: &setup.maker_global_nft.derive_htlc_pubkey_v2(&[]),
        swap_unique_data: &[],
        nft_swap_info: &nft_swap_info,
    };
    block_on(setup.maker_global_nft.validate_nft_maker_payment_v2(validate_args)).unwrap()
}

fn spend_nft_maker_payment(
    setup: &NftTestSetup,
    maker_payment: &SignedEthTx,
    contract_type: &ContractType,
) -> SignedEthTx {
    let spend_payment_args = SpendNftMakerPaymentArgs {
        maker_payment_tx: maker_payment,
        taker_secret_hash: &setup.taker_secret_hash,
        maker_secret_hash: &setup.maker_secret_hash,
        maker_secret: &setup.maker_secret,
        maker_pub: &setup.maker_global_nft.derive_htlc_pubkey_v2(&[]),
        swap_unique_data: &[],
        contract_type,
    };
    block_on(setup.taker_global_nft.spend_nft_maker_payment_v2(spend_payment_args)).unwrap()
}

fn refund_nft_maker_payment(
    setup: &NftTestSetup,
    maker_payment: &SignedEthTx,
    contract_type: &ContractType,
    refund_type: RefundType,
) -> SignedEthTx {
    let refund_args = RefundNftMakerPaymentArgs {
        maker_payment_tx: maker_payment,
        taker_secret_hash: &setup.taker_secret_hash,
        maker_secret_hash: &setup.maker_secret_hash,
        taker_secret: &setup.taker_secret,
        swap_unique_data: &[],
        contract_type,
    };
    match refund_type {
        RefundType::Timelock => {
            block_on(setup.maker_global_nft.refund_nft_maker_payment_v2_timelock(refund_args)).unwrap()
        },
        RefundType::Secret => block_on(setup.maker_global_nft.refund_nft_maker_payment_v2_secret(refund_args)).unwrap(),
    }
}

enum RefundType {
    Timelock,
    Secret,
}

#[derive(Copy, Clone)]
struct SwapAddresses {
    swap_v2_contracts: SwapV2Contracts,
    swap_contract_address: Address,
    fallback_swap_contract_address: Address,
}

#[allow(dead_code)]
/// Needed for Geth taker or maker swap v2 tests
impl SwapAddresses {
    fn init() -> Self {
        Self {
            swap_contract_address: swap_contract(),
            fallback_swap_contract_address: swap_contract(),
            swap_v2_contracts: SwapV2Contracts {
                maker_swap_v2_contract: geth_maker_swap_v2(),
                taker_swap_v2_contract: geth_taker_swap_v2(),
                nft_maker_swap_v2_contract: geth_nft_maker_swap_v2(),
            },
        }
    }
}

/// ERC20 test token decimals (our test token has 8 decimals)
const ERC20_TOKEN_DECIMALS: u8 = 8;

/// Needed for eth or erc20 v2 activation in Geth tests
fn eth_coin_v2_activation_with_random_privkey(
    ctx: &MmArc,
    ticker: &str,
    conf: &Json,
    swap_addr: SwapAddresses,
    erc20: bool,
) -> (EthCoin, Secp256k1Secret) {
    let priv_key = random_secp256k1_secret();
    let build_policy = EthPrivKeyBuildPolicy::IguanaPrivKey(priv_key);
    let node = EthNode {
        url: GETH_RPC_URL.to_string(),
        komodo_proxy: false,
    };
    let platform_request = EthActivationV2Request {
        nodes: vec![node],
        rpc_mode: Default::default(),
        swap_contract_address: Some(swap_addr.swap_contract_address),
        swap_v2_contracts: Some(swap_addr.swap_v2_contracts),
        fallback_swap_contract: Some(swap_addr.fallback_swap_contract_address),
        contract_supports_watchers: false,
        mm2: None,
        required_confirmations: None,
        priv_key_policy: Default::default(),
        enable_params: Default::default(),
        path_to_address: Default::default(),
        gap_limit: None,
        swap_gas_fee_policy: None,
    };
    let coin = block_on(eth_coin_from_conf_and_request_v2(
        ctx,
        ticker,
        conf,
        platform_request,
        build_policy,
        ChainSpec::Evm {
            chain_id: GETH_DEV_CHAIN_ID,
        },
    ))
    .unwrap();
    let my_address = block_on(coin.my_addr());
    fill_eth(my_address, U256::from(10).pow(U256::from(20)));
    fill_erc20(my_address, U256::from(10000000000u64));
    if erc20 {
        let coin_type = EthCoinType::Erc20 {
            platform: ETH.to_string(),
            token_addr: erc20_contract(),
        };
        let coin = block_on(coin.set_coin_type(coin_type, ERC20_TOKEN_DECIMALS));
        return (coin, priv_key);
    }
    (coin, priv_key)
}

#[test]
fn send_and_refund_taker_funding_by_secret_eth() {
    let swap_addr = SwapAddresses::init();
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ETH, &eth_dev_conf(), swap_addr, false);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ETH, &eth_dev_conf(), swap_addr, false);

    let taker_secret = &[0; 32];
    let taker_secret_hash = sha256(taker_secret).to_vec();
    let maker_secret = &[1; 32];
    let maker_secret_hash = sha256(maker_secret).to_vec();
    let funding_time_lock = now_sec() + 3000;
    let payment_time_lock = now_sec() + 1000;

    let dex_fee = &DexFee::Standard("0.00001".into());
    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let maker_pub = &maker_coin.derive_htlc_pubkey_v2(&[]);
    let payment_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        maker_pub: maker_pub.as_bytes(),
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount: trading_amount.clone(),
        swap_unique_data: &[],
    };

    let funding_tx = block_on(taker_coin.send_taker_funding(payment_args)).unwrap();
    log!("Taker sent ETH funding, tx hash: {:02x}", funding_tx.tx_hash());

    let refund_args = RefundFundingSecretArgs {
        funding_tx: &funding_tx,
        funding_time_lock,
        payment_time_lock,
        maker_pubkey: maker_pub,
        taker_secret,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        dex_fee,
        premium_amount: Default::default(),
        trading_amount,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let funding_tx_refund = block_on(taker_coin.refund_taker_funding_secret(refund_args)).unwrap();
    log!(
        "Taker refunded ETH funding by secret, tx hash: {:02x}",
        funding_tx_refund.tx_hash()
    );

    wait_for_confirmations(&taker_coin, &funding_tx_refund, 30);
}

#[test]
fn send_and_refund_taker_funding_by_secret_erc20() {
    let swap_addr = SwapAddresses::init();
    let erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ERC20DEV, &erc20_conf, swap_addr, true);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ERC20DEV, &erc20_conf, swap_addr, true);

    let taker_secret = &[0; 32];
    let taker_secret_hash = sha256(taker_secret).to_vec();
    let maker_secret = [1; 32];
    let maker_secret_hash = sha256(&maker_secret).to_vec();

    let funding_time_lock = now_sec() + 3000;
    let payment_time_lock = now_sec() + 1000;

    let dex_fee = &DexFee::Standard("0.00001".into());
    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let maker_pub = &maker_coin.derive_htlc_pubkey_v2(&[]);
    let payment_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        maker_pub: maker_pub.as_bytes(),
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount: trading_amount.clone(),
        swap_unique_data: &[],
    };

    let funding_tx = block_on(taker_coin.send_taker_funding(payment_args)).unwrap();
    log!("Taker sent ERC20 funding, tx hash: {:02x}", funding_tx.tx_hash());
    wait_for_confirmations(&taker_coin, &funding_tx, 30);

    let refund_args = RefundFundingSecretArgs {
        funding_tx: &funding_tx,
        funding_time_lock,
        payment_time_lock,
        maker_pubkey: maker_pub,
        taker_secret,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        dex_fee,
        premium_amount: Default::default(),
        trading_amount,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let funding_tx_refund = block_on(taker_coin.refund_taker_funding_secret(refund_args)).unwrap();
    log!(
        "Taker refunded ERC20 funding by secret, tx hash: {:02x}",
        funding_tx_refund.tx_hash()
    );
    wait_for_confirmations(&taker_coin, &funding_tx_refund, 30);
}

#[test]
fn send_and_refund_taker_funding_exceed_pre_approve_timelock_eth() {
    let swap_addr = SwapAddresses::init();
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ETH, &eth_dev_conf(), swap_addr, false);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ETH, &eth_dev_conf(), swap_addr, false);

    let taker_secret = [0; 32];
    let taker_secret_hash = sha256(&taker_secret).to_vec();
    let maker_secret = [1; 32];
    let maker_secret_hash = sha256(&maker_secret).to_vec();

    // if TakerPaymentState is `PaymentSent` then timestamp should exceed payment pre-approve lock time (funding_time_lock)
    let funding_time_lock = now_sec() - 3000;
    let payment_time_lock = now_sec() + 1000;

    let dex_fee = &DexFee::Standard("0.00001".into());
    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let maker_pub = &maker_coin.derive_htlc_pubkey_v2(&[]);
    let payment_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        maker_pub: maker_pub.as_bytes(),
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount: trading_amount.clone(),
        swap_unique_data: &[],
    };
    let funding_tx = block_on(taker_coin.send_taker_funding(payment_args)).unwrap();
    log!("Taker sent ETH funding, tx hash: {:02x}", funding_tx.tx_hash());
    wait_for_confirmations(&taker_coin, &funding_tx, 30);

    let tx_type_with_secret_hash = SwapTxTypeWithSecretHash::TakerPaymentV2 {
        maker_secret_hash: &maker_secret_hash,
        taker_secret_hash: &taker_secret_hash,
    };

    let refund_args = RefundTakerPaymentArgs {
        payment_tx: &funding_tx.to_bytes(),
        time_lock: payment_time_lock,
        maker_pub: maker_pub.as_bytes(),
        tx_type_with_secret_hash,
        swap_unique_data: &[],
        watcher_reward: false,
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount,
    };
    let funding_tx_refund = block_on(taker_coin.refund_taker_funding_timelock(refund_args)).unwrap();
    log!(
        "Taker refunded ETH funding after pre-approval lock time was exceeded, tx hash: {:02x}",
        funding_tx_refund.tx_hash()
    );

    wait_for_confirmations(&taker_coin, &funding_tx_refund, 30);
}

#[test]
fn taker_send_approve_and_spend_eth() {
    let swap_addr = SwapAddresses::init();
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ETH, &eth_dev_conf(), swap_addr, false);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ETH, &eth_dev_conf(), swap_addr, false);

    let taker_secret = &[0; 32];
    let taker_secret_hash = sha256(taker_secret).to_vec();
    let maker_secret = &[1; 32];
    let maker_secret_hash = sha256(maker_secret).to_vec();
    let funding_time_lock = now_sec() + 3000;
    let payment_time_lock = now_sec() + 600;

    let maker_address = block_on(maker_coin.my_addr());

    let dex_fee = &DexFee::Standard("0.00001".into());
    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let maker_pub = &maker_coin.derive_htlc_pubkey_v2(&[]);
    let payment_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        maker_pub: maker_pub.as_bytes(),
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount: trading_amount.clone(),
        swap_unique_data: &[],
    };
    let taker_coin_start_block = block_on(taker_coin.current_block().compat()).unwrap();
    let funding_tx = block_on(taker_coin.send_taker_funding(payment_args)).unwrap();
    log!("Taker sent ETH funding, tx hash: {:02x}", funding_tx.tx_hash());
    wait_for_confirmations(&taker_coin, &funding_tx, 30);

    let taker_pub = &taker_coin.derive_htlc_pubkey_v2(&[]);
    let validate = ValidateTakerFundingArgs {
        funding_tx: &funding_tx,
        funding_time_lock,
        payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        taker_pub,
        dex_fee,
        premium_amount: Default::default(),
        trading_amount: trading_amount.clone(),
        swap_unique_data: &[],
    };
    block_on(maker_coin.validate_taker_funding(validate)).unwrap();

    let approve_args = GenTakerFundingSpendArgs {
        funding_tx: &funding_tx,
        maker_pub,
        taker_pub,
        funding_time_lock,
        taker_secret_hash: &taker_secret_hash,
        taker_payment_time_lock: funding_time_lock,
        maker_secret_hash: &maker_secret_hash,
    };
    let preimage = TxPreimageWithSig {
        preimage: funding_tx.clone(),
        signature: taker_coin.parse_signature(&[0u8; 65]).unwrap(),
    };
    let taker_approve_tx =
        block_on(taker_coin.sign_and_send_taker_funding_spend(&preimage, &approve_args, &[])).unwrap();
    log!(
        "Taker approved ETH payment, tx hash: {:02x}",
        taker_approve_tx.tx_hash()
    );
    wait_for_confirmations(&taker_coin, &taker_approve_tx, 30);

    let check_taker_approved_tx = block_on(maker_coin.search_for_taker_funding_spend(&funding_tx, 0u64, &[]))
        .unwrap()
        .unwrap();
    match check_taker_approved_tx {
        FundingTxSpend::TransferredToTakerPayment(tx) => {
            assert_eq!(tx, funding_tx);
        },
        FundingTxSpend::RefundedTimelock(_) | FundingTxSpend::RefundedSecret { .. } => {
            panic!("Wrong FundingTxSpend variant, expected TransferredToTakerPayment")
        },
    };

    let spend_args = GenTakerPaymentSpendArgs {
        taker_tx: &funding_tx,
        time_lock: payment_time_lock,
        maker_secret_hash: &maker_secret_hash,
        maker_pub,
        maker_address: &maker_address,
        taker_pub,
        dex_fee,
        premium_amount: Default::default(),
        trading_amount,
    };
    let spend_tx =
        block_on(maker_coin.sign_and_broadcast_taker_payment_spend(None, &spend_args, maker_secret, &[])).unwrap();
    log!("Maker spent ETH payment, tx hash: {:02x}", spend_tx.tx_hash());
    wait_for_confirmations(&maker_coin, &spend_tx, 30);
    let found_spend_tx =
        block_on(taker_coin.find_taker_payment_spend_tx(&taker_approve_tx, taker_coin_start_block, payment_time_lock))
            .unwrap();
    let extracted_maker_secret = block_on(taker_coin.extract_secret_v2(&[], &found_spend_tx)).unwrap();
    assert_eq!(maker_secret, &extracted_maker_secret);
}

#[test]
fn taker_send_approve_and_spend_erc20() {
    let swap_addr = SwapAddresses::init();
    let erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ERC20DEV, &erc20_conf, swap_addr, true);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ERC20DEV, &erc20_conf, swap_addr, true);

    let taker_secret = [0; 32];
    let taker_secret_hash = sha256(&taker_secret).to_vec();
    let maker_secret = [1; 32];
    let maker_secret_hash = sha256(&maker_secret).to_vec();
    let funding_time_lock = now_sec() + 3000;
    let payment_time_lock = now_sec() + 600;

    let maker_address = block_on(maker_coin.my_addr());

    let dex_fee = &DexFee::Standard("0.00001".into());
    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let maker_pub = &maker_coin.derive_htlc_pubkey_v2(&[]);
    let payment_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        maker_pub: maker_pub.as_bytes(),
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount: trading_amount.clone(),
        swap_unique_data: &[],
    };
    let taker_coin_start_block = block_on(taker_coin.current_block().compat()).unwrap();
    let funding_tx = block_on(taker_coin.send_taker_funding(payment_args)).unwrap();
    log!("Taker sent ERC20 funding, tx hash: {:02x}", funding_tx.tx_hash());
    wait_for_confirmations(&taker_coin, &funding_tx, 30);

    let taker_pub = &taker_coin.derive_htlc_pubkey_v2(&[]);
    let validate = ValidateTakerFundingArgs {
        funding_tx: &funding_tx,
        funding_time_lock,
        payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        taker_pub,
        dex_fee,
        premium_amount: Default::default(),
        trading_amount: trading_amount.clone(),
        swap_unique_data: &[],
    };
    block_on(maker_coin.validate_taker_funding(validate)).unwrap();

    let approve_args = GenTakerFundingSpendArgs {
        funding_tx: &funding_tx,
        maker_pub,
        taker_pub,
        funding_time_lock,
        taker_secret_hash: &taker_secret_hash,
        taker_payment_time_lock: funding_time_lock,
        maker_secret_hash: &maker_secret_hash,
    };
    let preimage = TxPreimageWithSig {
        preimage: funding_tx.clone(),
        signature: taker_coin.parse_signature(&[0u8; 65]).unwrap(),
    };
    let taker_approve_tx =
        block_on(taker_coin.sign_and_send_taker_funding_spend(&preimage, &approve_args, &[])).unwrap();
    log!(
        "Taker approved ERC20 payment, tx hash: {:02x}",
        taker_approve_tx.tx_hash()
    );
    wait_for_confirmations(&taker_coin, &taker_approve_tx, 30);

    let check_taker_approved_tx = block_on(maker_coin.search_for_taker_funding_spend(&funding_tx, 0u64, &[]))
        .unwrap()
        .unwrap();
    match check_taker_approved_tx {
        FundingTxSpend::TransferredToTakerPayment(tx) => {
            assert_eq!(tx, funding_tx);
        },
        FundingTxSpend::RefundedTimelock(_) | FundingTxSpend::RefundedSecret { .. } => {
            panic!("Wrong FundingTxSpend variant, expected TransferredToTakerPayment")
        },
    };

    let spend_args = GenTakerPaymentSpendArgs {
        taker_tx: &funding_tx,
        time_lock: payment_time_lock,
        maker_secret_hash: &maker_secret_hash,
        maker_pub,
        maker_address: &maker_address,
        taker_pub,
        dex_fee,
        premium_amount: Default::default(),
        trading_amount,
    };
    let spend_tx =
        block_on(maker_coin.sign_and_broadcast_taker_payment_spend(None, &spend_args, &maker_secret, &[])).unwrap();
    log!("Maker spent ERC20 payment, tx hash: {:02x}", spend_tx.tx_hash());
    let found_spend_tx =
        block_on(taker_coin.find_taker_payment_spend_tx(&taker_approve_tx, taker_coin_start_block, payment_time_lock))
            .unwrap();
    let extracted_maker_secret = block_on(taker_coin.extract_secret_v2(&[], &found_spend_tx)).unwrap();
    assert_eq!(maker_secret, extracted_maker_secret);
}

#[test]
fn send_and_refund_taker_funding_exceed_payment_timelock_eth() {
    let swap_addr = SwapAddresses::init();
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ETH, &eth_dev_conf(), swap_addr, false);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ETH, &eth_dev_conf(), swap_addr, false);

    let taker_secret = [0; 32];
    let taker_secret_hash = sha256(&taker_secret).to_vec();
    let maker_secret = [1; 32];
    let maker_secret_hash = sha256(&maker_secret).to_vec();
    let funding_time_lock = now_sec() + 3000;
    let payment_time_lock = now_sec() - 1000;

    let dex_fee = &DexFee::Standard("0.00001".into());
    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let maker_pub = &maker_coin.derive_htlc_pubkey_v2(&[]);
    let payment_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        maker_pub: maker_pub.as_bytes(),
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount: trading_amount.clone(),
        swap_unique_data: &[],
    };
    let funding_tx = block_on(taker_coin.send_taker_funding(payment_args)).unwrap();
    log!("Taker sent ETH funding, tx hash: {:02x}", funding_tx.tx_hash());
    wait_for_confirmations(&taker_coin, &funding_tx, 30);

    let taker_pub = &taker_coin.derive_htlc_pubkey_v2(&[]);
    let approve_args = GenTakerFundingSpendArgs {
        funding_tx: &funding_tx,
        maker_pub,
        taker_pub,
        funding_time_lock,
        taker_secret_hash: &taker_secret_hash,
        taker_payment_time_lock: funding_time_lock,
        maker_secret_hash: &maker_secret_hash,
    };
    let preimage = TxPreimageWithSig {
        preimage: funding_tx.clone(),
        signature: taker_coin.parse_signature(&[0u8; 65]).unwrap(),
    };
    let taker_approve_tx =
        block_on(taker_coin.sign_and_send_taker_funding_spend(&preimage, &approve_args, &[])).unwrap();
    log!(
        "Taker approved ETH payment, tx hash: {:02x}",
        taker_approve_tx.tx_hash()
    );
    wait_for_confirmations(&taker_coin, &taker_approve_tx, 30);

    let tx_type_with_secret_hash = SwapTxTypeWithSecretHash::TakerPaymentV2 {
        maker_secret_hash: &maker_secret_hash,
        taker_secret_hash: &taker_secret_hash,
    };
    let refund_args = RefundTakerPaymentArgs {
        payment_tx: &funding_tx.to_bytes(),
        time_lock: payment_time_lock,
        maker_pub: maker_pub.as_bytes(),
        tx_type_with_secret_hash,
        swap_unique_data: &[],
        watcher_reward: false,
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount,
    };
    let funding_tx_refund = block_on(taker_coin.refund_taker_funding_timelock(refund_args)).unwrap();
    log!(
        "Taker refunded ETH funding after payment lock time was exceeded, tx hash: {:02x}",
        funding_tx_refund.tx_hash()
    );
    wait_for_confirmations(&taker_coin, &funding_tx_refund, 30);
}

#[test]
fn send_and_refund_taker_funding_exceed_payment_timelock_erc20() {
    let swap_addr = SwapAddresses::init();
    let erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ERC20DEV, &erc20_conf, swap_addr, true);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ERC20DEV, &erc20_conf, swap_addr, true);

    let taker_secret = [0; 32];
    let taker_secret_hash = sha256(&taker_secret).to_vec();
    let maker_secret = [1; 32];
    let maker_secret_hash = sha256(&maker_secret).to_vec();
    let funding_time_lock = now_sec() + 29;
    let payment_time_lock = now_sec() + 15;

    let dex_fee = &DexFee::Standard("0.00001".into());
    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let maker_pub = &maker_coin.derive_htlc_pubkey_v2(&[]);
    let payment_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        maker_pub: maker_pub.as_bytes(),
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount: trading_amount.clone(),
        swap_unique_data: &[],
    };
    let funding_tx = block_on(taker_coin.send_taker_funding(payment_args)).unwrap();
    log!("Taker sent ERC20 funding, tx hash: {:02x}", funding_tx.tx_hash());
    wait_for_confirmations(&taker_coin, &funding_tx, 30);
    thread::sleep(Duration::from_secs(16));

    let taker_pub = &taker_coin.derive_htlc_pubkey_v2(&[]);
    let approve_args = GenTakerFundingSpendArgs {
        funding_tx: &funding_tx,
        maker_pub,
        taker_pub,
        funding_time_lock,
        taker_secret_hash: &taker_secret_hash,
        taker_payment_time_lock: funding_time_lock,
        maker_secret_hash: &maker_secret_hash,
    };
    let preimage = TxPreimageWithSig {
        preimage: funding_tx.clone(),
        signature: taker_coin.parse_signature(&[0u8; 65]).unwrap(),
    };
    let taker_approve_tx =
        block_on(taker_coin.sign_and_send_taker_funding_spend(&preimage, &approve_args, &[])).unwrap();
    log!(
        "Taker approved ERC20 payment, tx hash: {:02x}",
        taker_approve_tx.tx_hash()
    );
    wait_for_confirmations(&taker_coin, &taker_approve_tx, 30);

    let tx_type_with_secret_hash = SwapTxTypeWithSecretHash::TakerPaymentV2 {
        maker_secret_hash: &maker_secret_hash,
        taker_secret_hash: &taker_secret_hash,
    };
    let refund_args = RefundTakerPaymentArgs {
        payment_tx: &funding_tx.to_bytes(),
        time_lock: payment_time_lock,
        maker_pub: maker_pub.as_bytes(),
        tx_type_with_secret_hash,
        swap_unique_data: &[],
        watcher_reward: false,
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount,
    };
    let funding_tx_refund = block_on(taker_coin.refund_taker_funding_timelock(refund_args)).unwrap();
    log!(
        "Taker refunded ERC20 funding after payment lock time was exceeded, tx hash: {:02x}",
        funding_tx_refund.tx_hash()
    );
    wait_for_confirmations(&taker_coin, &funding_tx_refund, 30);
}

#[test]
fn send_and_refund_taker_funding_exceed_pre_approve_timelock_erc20() {
    let swap_addr = SwapAddresses::init();
    let erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ERC20DEV, &erc20_conf, swap_addr, true);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ERC20DEV, &erc20_conf, swap_addr, true);

    let taker_secret = [0; 32];
    let taker_secret_hash = sha256(&taker_secret).to_vec();
    let maker_secret = [1; 32];
    let maker_secret_hash = sha256(&maker_secret).to_vec();

    // if TakerPaymentState is `PaymentSent` then timestamp should exceed payment pre-approve lock time (funding_time_lock)
    let funding_time_lock = now_sec() + 29;
    let payment_time_lock = now_sec() + 1000;

    let dex_fee = &DexFee::Standard("0.00001".into());
    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let maker_pub = &maker_coin.derive_htlc_pubkey_v2(&[]);
    let payment_args = SendTakerFundingArgs {
        funding_time_lock,
        payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        maker_pub: maker_pub.as_bytes(),
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount: trading_amount.clone(),
        swap_unique_data: &[],
    };
    let funding_tx = block_on(taker_coin.send_taker_funding(payment_args)).unwrap();
    log!("Taker sent ERC20 funding, tx hash: {:02x}", funding_tx.tx_hash());
    wait_for_confirmations(&taker_coin, &funding_tx, 30);
    thread::sleep(Duration::from_secs(29));

    let tx_type_with_secret_hash = SwapTxTypeWithSecretHash::TakerPaymentV2 {
        maker_secret_hash: &maker_secret_hash,
        taker_secret_hash: &taker_secret_hash,
    };

    let refund_args = RefundTakerPaymentArgs {
        payment_tx: &funding_tx.to_bytes(),
        time_lock: payment_time_lock,
        maker_pub: maker_pub.as_bytes(),
        tx_type_with_secret_hash,
        swap_unique_data: &[],
        watcher_reward: false,
        dex_fee,
        premium_amount: BigDecimal::default(),
        trading_amount,
    };
    let funding_tx_refund = block_on(taker_coin.refund_taker_funding_timelock(refund_args)).unwrap();
    log!(
        "Taker refunded ERC20 funding after pre-approval lock time was exceeded, tx hash: {:02x}",
        funding_tx_refund.tx_hash()
    );
    wait_for_confirmations(&taker_coin, &funding_tx_refund, 30);
}

#[test]
fn send_maker_payment_and_refund_timelock_eth() {
    let swap_addr = SwapAddresses::init();
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ETH, &eth_dev_conf(), swap_addr, false);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ETH, &eth_dev_conf(), swap_addr, false);

    let taker_secret = [0; 32];
    let taker_secret_hash = sha256(&taker_secret).to_vec();
    let maker_secret = [1; 32];
    let maker_secret_hash = sha256(&maker_secret).to_vec();
    let payment_time_lock = now_sec() - 1000;

    let taker_pub = &taker_coin.derive_htlc_pubkey_v2(&[]);

    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let payment_args = SendMakerPaymentArgs {
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        amount: trading_amount.clone(),
        taker_pub,
        swap_unique_data: &[],
    };
    let payment_tx = block_on(maker_coin.send_maker_payment_v2(payment_args)).unwrap();
    log!("Maker sent ETH payment, tx hash: {:02x}", payment_tx.tx_hash());
    wait_for_confirmations(&maker_coin, &payment_tx, 30);

    let tx_type_with_secret_hash = SwapTxTypeWithSecretHash::MakerPaymentV2 {
        maker_secret_hash: &maker_secret_hash,
        taker_secret_hash: &taker_secret_hash,
    };
    let refund_args = RefundMakerPaymentTimelockArgs {
        payment_tx: &payment_tx.to_bytes(),
        time_lock: payment_time_lock,
        taker_pub: &taker_pub.to_bytes(),
        tx_type_with_secret_hash,
        swap_unique_data: &[],
        watcher_reward: false,
        amount: trading_amount,
    };
    let payment_tx_refund = block_on(maker_coin.refund_maker_payment_v2_timelock(refund_args)).unwrap();
    log!(
        "Maker refunded ETH payment after timelock, tx hash: {:02x}",
        payment_tx_refund.tx_hash()
    );
    wait_for_confirmations(&maker_coin, &payment_tx_refund, 30);
}

#[test]
fn send_maker_payment_and_refund_timelock_erc20() {
    let swap_addr = SwapAddresses::init();
    let erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ERC20DEV, &erc20_conf, swap_addr, true);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ERC20DEV, &erc20_conf, swap_addr, true);

    let taker_secret = [0; 32];
    let taker_secret_hash = sha256(&taker_secret).to_vec();
    let maker_secret = [1; 32];
    let maker_secret_hash = sha256(&maker_secret).to_vec();
    let payment_time_lock = now_sec() - 1000;

    let taker_pub = &taker_coin.derive_htlc_pubkey_v2(&[]);

    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    // Pre-approve the ERC20 token for maker swap v2 contract since the payment time_lock
    // is in the past (for refund testing) and handle_allowance would timeout immediately.
    let approve_tx =
        block_on_f01(maker_coin.approve(swap_addr.swap_v2_contracts.maker_swap_v2_contract, U256::max_value()))
            .unwrap();
    wait_for_confirmations(&maker_coin, &approve_tx, 30);

    let payment_args = SendMakerPaymentArgs {
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        amount: trading_amount.clone(),
        taker_pub,
        swap_unique_data: &[],
    };
    let payment_tx = block_on(maker_coin.send_maker_payment_v2(payment_args)).unwrap();
    log!("Maker sent ERC20 payment, tx hash: {:02x}", payment_tx.tx_hash());
    wait_for_confirmations(&maker_coin, &payment_tx, 30);

    let tx_type_with_secret_hash = SwapTxTypeWithSecretHash::MakerPaymentV2 {
        maker_secret_hash: &maker_secret_hash,
        taker_secret_hash: &taker_secret_hash,
    };
    let refund_args = RefundMakerPaymentTimelockArgs {
        payment_tx: &payment_tx.to_bytes(),
        time_lock: payment_time_lock,
        taker_pub: &taker_pub.to_bytes(),
        tx_type_with_secret_hash,
        swap_unique_data: &[],
        watcher_reward: false,
        amount: trading_amount,
    };
    let payment_tx_refund = block_on(maker_coin.refund_maker_payment_v2_timelock(refund_args)).unwrap();
    log!(
        "Maker refunded ERC20 payment after timelock, tx hash: {:02x}",
        payment_tx_refund.tx_hash()
    );
    wait_for_confirmations(&maker_coin, &payment_tx_refund, 30);
}

#[test]
fn send_maker_payment_and_refund_secret_eth() {
    let swap_addr = SwapAddresses::init();
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ETH, &eth_dev_conf(), swap_addr, false);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ETH, &eth_dev_conf(), swap_addr, false);

    let taker_secret = &[0; 32];
    let taker_secret_hash = sha256(taker_secret).to_vec();
    let maker_secret = &[1; 32];
    let maker_secret_hash = sha256(maker_secret).to_vec();
    let payment_time_lock = now_sec() + 1000;

    let taker_pub = &taker_coin.derive_htlc_pubkey_v2(&[]);

    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let payment_args = SendMakerPaymentArgs {
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        amount: trading_amount.clone(),
        taker_pub,
        swap_unique_data: &[],
    };
    let payment_tx = block_on(maker_coin.send_maker_payment_v2(payment_args)).unwrap();
    log!("Maker sent ETH payment, tx hash: {:02x}", payment_tx.tx_hash());
    wait_for_confirmations(&maker_coin, &payment_tx, 30);

    let refund_args = RefundMakerPaymentSecretArgs {
        maker_payment_tx: &payment_tx,
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        taker_secret,
        taker_pub,
        swap_unique_data: &[],
        amount: trading_amount,
    };
    let payment_tx_refund = block_on(maker_coin.refund_maker_payment_v2_secret(refund_args)).unwrap();
    log!(
        "Maker refunded ETH payment using taker secret, tx hash: {:02x}",
        payment_tx_refund.tx_hash()
    );
    wait_for_confirmations(&maker_coin, &payment_tx_refund, 30);
}

#[test]
fn send_maker_payment_and_refund_secret_erc20() {
    let swap_addr = SwapAddresses::init();
    let erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ERC20DEV, &erc20_conf, swap_addr, true);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ERC20DEV, &erc20_conf, swap_addr, true);

    let taker_secret = &[0; 32];
    let taker_secret_hash = sha256(taker_secret).to_vec();
    let maker_secret = &[1; 32];
    let maker_secret_hash = sha256(maker_secret).to_vec();
    let payment_time_lock = now_sec() + 1000;

    let taker_pub = &taker_coin.derive_htlc_pubkey_v2(&[]);

    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let payment_args = SendMakerPaymentArgs {
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        amount: trading_amount.clone(),
        taker_pub,
        swap_unique_data: &[],
    };
    let payment_tx = block_on(maker_coin.send_maker_payment_v2(payment_args)).unwrap();
    log!("Maker sent ERC20 payment, tx hash: {:02x}", payment_tx.tx_hash());
    wait_for_confirmations(&maker_coin, &payment_tx, 30);

    let refund_args = RefundMakerPaymentSecretArgs {
        maker_payment_tx: &payment_tx,
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        taker_secret,
        taker_pub,
        swap_unique_data: &[],
        amount: trading_amount,
    };
    let payment_tx_refund = block_on(maker_coin.refund_maker_payment_v2_secret(refund_args)).unwrap();
    log!(
        "Maker refunded ERC20 payment using taker secret, tx hash: {:02x}",
        payment_tx_refund.tx_hash()
    );
    wait_for_confirmations(&maker_coin, &payment_tx_refund, 30);
}

#[test]
fn send_and_spend_maker_payment_eth() {
    let swap_addr = SwapAddresses::init();
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ETH, &eth_dev_conf(), swap_addr, false);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ETH, &eth_dev_conf(), swap_addr, false);

    let taker_secret = [0; 32];
    let taker_secret_hash = sha256(&taker_secret).to_vec();
    let maker_secret = [1; 32];
    let maker_secret_hash = sha256(&maker_secret).to_vec();
    let payment_time_lock = now_sec() + 1000;

    let maker_pub = &maker_coin.derive_htlc_pubkey_v2(&[]);
    let taker_pub = &taker_coin.derive_htlc_pubkey_v2(&[]);

    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let payment_args = SendMakerPaymentArgs {
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        amount: trading_amount.clone(),
        taker_pub,
        swap_unique_data: &[],
    };
    let payment_tx = block_on(maker_coin.send_maker_payment_v2(payment_args)).unwrap();
    log!("Maker sent ETH payment, tx hash: {:02x}", payment_tx.tx_hash());
    wait_for_confirmations(&maker_coin, &payment_tx, 30);

    let validation_args = ValidateMakerPaymentArgs {
        maker_payment_tx: &payment_tx,
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        amount: trading_amount.clone(),
        maker_pub,
        swap_unique_data: &[],
    };
    block_on(taker_coin.validate_maker_payment_v2(validation_args)).unwrap();
    log!("Taker validated maker ETH payment");

    let spend_args = SpendMakerPaymentArgs {
        maker_payment_tx: &payment_tx,
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        maker_secret,
        maker_pub,
        swap_unique_data: &[],
        amount: trading_amount,
    };
    let spend_tx = block_on(taker_coin.spend_maker_payment_v2(spend_args)).unwrap();
    log!("Taker spent maker ETH payment, tx hash: {:02x}", spend_tx.tx_hash());
    wait_for_confirmations(&taker_coin, &spend_tx, 30);
}

#[test]
fn send_and_spend_maker_payment_erc20() {
    let swap_addr = SwapAddresses::init();
    let erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    let (taker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ERC20DEV, &erc20_conf, swap_addr, true);
    let (maker_coin, _) = eth_coin_v2_activation_with_random_privkey(&MM_CTX, ERC20DEV, &erc20_conf, swap_addr, true);

    let taker_secret = [0; 32];
    let taker_secret_hash = sha256(&taker_secret).to_vec();
    let maker_secret = [1; 32];
    let maker_secret_hash = sha256(&maker_secret).to_vec();
    let payment_time_lock = now_sec() + 1000;

    let maker_pub = &maker_coin.derive_htlc_pubkey_v2(&[]);
    let taker_pub = &taker_coin.derive_htlc_pubkey_v2(&[]);

    let trading_amount = BigDecimal::from_str("0.0001").unwrap();

    let payment_args = SendMakerPaymentArgs {
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        amount: trading_amount.clone(),
        taker_pub,
        swap_unique_data: &[],
    };
    let payment_tx = block_on(maker_coin.send_maker_payment_v2(payment_args)).unwrap();
    log!("Maker sent ERC20 payment, tx hash: {:02x}", payment_tx.tx_hash());
    wait_for_confirmations(&maker_coin, &payment_tx, 30);

    let validation_args = ValidateMakerPaymentArgs {
        maker_payment_tx: &payment_tx,
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        amount: trading_amount.clone(),
        maker_pub,
        swap_unique_data: &[],
    };
    block_on(taker_coin.validate_maker_payment_v2(validation_args)).unwrap();
    log!("Taker validated maker ERC20 payment");

    let spend_args = SpendMakerPaymentArgs {
        maker_payment_tx: &payment_tx,
        time_lock: payment_time_lock,
        taker_secret_hash: &taker_secret_hash,
        maker_secret_hash: &maker_secret_hash,
        maker_secret,
        maker_pub,
        swap_unique_data: &[],
        amount: trading_amount,
    };
    let spend_tx = block_on(taker_coin.spend_maker_payment_v2(spend_args)).unwrap();
    log!("Taker spent maker ERC20 payment, tx hash: {:02x}", spend_tx.tx_hash());
    wait_for_confirmations(&taker_coin, &spend_tx, 30);
}

#[test]
fn test_eth_erc20_hd() {
    const PASSPHRASE: &str = "tank abandon bind salon remove wisdom net size aspect direct source fossil";

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);
    let swap_contract = swap_contract_checksum();

    // Withdraw from HD account 0, change address 0, index 0
    let path_to_address = HDAccountAddressId::default();
    let conf = Mm2TestConf::seednode_with_hd_account(PASSPHRASE, &coins);
    let mm_hd = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();
    let (_mm_dump_log, _mm_dump_dashboard) = mm_hd.mm_dump();
    log!("Alice log path: {}", mm_hd.log_path.display());

    let eth_enable = block_on(task_enable_eth_with_tokens(
        &mm_hd,
        "ETH",
        &["ERC20DEV"],
        Some(&swap_contract),
        &[GETH_RPC_URL],
        60,
        Some(path_to_address),
    ));
    let activation_result = match eth_enable {
        EthWithTokensActivationResult::HD(hd) => hd,
        _ => panic!("Expected EthWithTokensActivationResult::HD"),
    };
    let balance = match activation_result.wallet_balance {
        EnableCoinBalanceMap::HD(hd) => hd,
        _ => panic!("Expected EnableCoinBalance::HD"),
    };
    let account = balance.accounts.first().expect("Expected account at index 0");
    assert_eq!(
        account.addresses[0].address,
        "0x1737F1FaB40c6Fd3dc729B51C0F97DB3297CCA93"
    );
    assert_eq!(account.addresses[0].balance.len(), 2);
    assert!(account.addresses[0].balance.contains_key("ETH"));
    assert!(account.addresses[0].balance.contains_key("ERC20DEV"));

    block_on(mm_hd.stop()).unwrap();

    // Enable HD account 0, change address 0, index 1
    let path_to_address = HDAccountAddressId {
        account_id: 0,
        chain: Bip44Chain::External,
        address_id: 1,
    };
    let conf = Mm2TestConf::seednode_with_hd_account(PASSPHRASE, &coins);
    let mm_hd = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();
    let (_mm_dump_log, _mm_dump_dashboard) = mm_hd.mm_dump();
    log!("Alice log path: {}", mm_hd.log_path.display());

    let eth_enable = block_on(task_enable_eth_with_tokens(
        &mm_hd,
        "ETH",
        &["ERC20DEV"],
        Some(&swap_contract),
        &[GETH_RPC_URL],
        60,
        Some(path_to_address),
    ));
    let activation_result = match eth_enable {
        EthWithTokensActivationResult::HD(hd) => hd,
        _ => panic!("Expected EthWithTokensActivationResult::HD"),
    };
    let balance = match activation_result.wallet_balance {
        EnableCoinBalanceMap::HD(hd) => hd,
        _ => panic!("Expected EnableCoinBalance::HD"),
    };
    let account = balance.accounts.first().expect("Expected account at index 0");
    assert_eq!(
        account.addresses[1].address,
        "0xDe841899aB4A22E23dB21634e54920aDec402397"
    );
    assert_eq!(account.addresses[0].balance.len(), 2);
    assert!(account.addresses[0].balance.contains_key("ETH"));
    assert!(account.addresses[0].balance.contains_key("ERC20DEV"));

    let get_new_address = block_on(get_new_address(&mm_hd, "ETH", 0, Some(Bip44Chain::External)));
    assert!(get_new_address.new_address.balance.contains_key("ETH"));
    // Make sure balance is returned for any token enabled with ETH as platform coin
    assert!(get_new_address.new_address.balance.contains_key("ERC20DEV"));
    assert_eq!(
        get_new_address.new_address.address,
        "0x4249E165a68E4FF9C41B1C3C3b4245c30ecB43CC"
    );
    // Make sure that the address is also added to tokens
    let account_balance = block_on(account_balance(&mm_hd, "ERC20DEV", 0, Bip44Chain::External, None));
    assert_eq!(
        account_balance.addresses[2].address,
        "0x4249E165a68E4FF9C41B1C3C3b4245c30ecB43CC"
    );

    block_on(mm_hd.stop()).unwrap();

    // Enable HD account 77, change address 0, index 7
    let path_to_address = HDAccountAddressId {
        account_id: 77,
        chain: Bip44Chain::External,
        address_id: 7,
    };
    let conf = Mm2TestConf::seednode_with_hd_account(PASSPHRASE, &coins);
    let mm_hd = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();
    let (_mm_dump_log, _mm_dump_dashboard) = mm_hd.mm_dump();
    log!("Alice log path: {}", mm_hd.log_path.display());

    let eth_enable = block_on(task_enable_eth_with_tokens(
        &mm_hd,
        "ETH",
        &["ERC20DEV"],
        Some(&swap_contract),
        &[GETH_RPC_URL],
        60,
        Some(path_to_address),
    ));
    let activation_result = match eth_enable {
        EthWithTokensActivationResult::HD(hd) => hd,
        _ => panic!("Expected EthWithTokensActivationResult::HD"),
    };
    let balance = match activation_result.wallet_balance {
        EnableCoinBalanceMap::HD(hd) => hd,
        _ => panic!("Expected EnableCoinBalance::HD"),
    };
    let account = balance.accounts.first().expect("Expected account at index 0");
    assert_eq!(
        account.addresses[7].address,
        "0xa420a4DBd8C50e6240014Db4587d2ec8D0cE0e6B"
    );
    assert_eq!(account.addresses[0].balance.len(), 2);
    assert!(account.addresses[0].balance.contains_key("ETH"));
    assert!(account.addresses[0].balance.contains_key("ERC20DEV"));

    block_on(mm_hd.stop()).unwrap();
}

#[test]
fn test_enable_custom_erc20() {
    const PASSPHRASE: &str = "tank abandon bind salon remove wisdom net size aspect direct source fossil";

    let coins = json!([eth_dev_conf()]);
    let swap_contract = swap_contract_checksum();

    let path_to_address = HDAccountAddressId::default();
    let conf = Mm2TestConf::seednode_with_hd_account(PASSPHRASE, &coins);
    let mm_hd = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();
    let (_mm_dump_log, _mm_dump_dashboard) = mm_hd.mm_dump();
    log!("Alice log path: {}", mm_hd.log_path.display());

    // Enable platform coin in HD mode
    block_on(task_enable_eth_with_tokens(
        &mm_hd,
        "ETH",
        &[],
        Some(&swap_contract),
        &[GETH_RPC_URL],
        60,
        Some(path_to_address.clone()),
    ));

    // Test `get_token_info` rpc, we also use it to get the token symbol to use it as the ticker
    let protocol = erc20_dev_conf(&erc20_contract_checksum())["protocol"].clone();
    let TokenInfo::ERC20(custom_token_info) = block_on(get_token_info(&mm_hd, protocol.clone())).info;
    let ticker = custom_token_info.symbol;
    assert_eq!(ticker, "QTC");
    assert_eq!(custom_token_info.decimals, 8);

    // Enable the custom token in HD mode
    block_on(enable_erc20_token_v2(
        &mm_hd,
        &ticker,
        Some(protocol.clone()),
        60,
        Some(path_to_address.clone()),
    ))
    .unwrap();

    // Test that the custom token is wallet only by using it in a swap
    let buy = block_on(mm_hd.rpc(&json!({
        "userpass": mm_hd.userpass,
        "method": "buy",
        "base": "ETH",
        "rel": ticker,
        "price": "1",
        "volume": "1",
    })))
    .unwrap();
    assert!(!buy.0.is_success(), "buy success, but should fail: {}", buy.1);
    assert!(
        buy.1.contains(&format!(
            "'{ticker}' is a wallet only asset and can't be used in orders."
        )),
        "Expected error message indicating that the token is wallet only, but got: {}",
        buy.1
    );

    // Enabling the same custom token using a different ticker should fail
    let err = block_on(enable_erc20_token_v2(
        &mm_hd,
        "ERC20DEV",
        Some(protocol.clone()),
        60,
        Some(path_to_address),
    ))
    .unwrap_err();
    let expected_error_type = "CustomTokenError";
    assert_eq!(err["error_type"], expected_error_type);
    let expected_error_data = json!({
        "TokenWithSameContractAlreadyActivated": {
            "ticker": ticker,
            "contract_address": protocol["protocol_data"]["contract_address"]
        }
    });
    assert_eq!(err["error_data"], expected_error_data);

    // Disable the custom token
    block_on(disable_coin(&mm_hd, &ticker, true));
}

#[test]
fn test_enable_custom_erc20_with_duplicate_contract_in_config() {
    const PASSPHRASE: &str = "tank abandon bind salon remove wisdom net size aspect direct source fossil";

    let erc20_dev_conf = erc20_dev_conf(&erc20_contract_checksum());
    let coins = json!([eth_dev_conf(), erc20_dev_conf]);
    let swap_contract = swap_contract_checksum();

    let path_to_address = HDAccountAddressId::default();
    let conf = Mm2TestConf::seednode_with_hd_account(PASSPHRASE, &coins);
    let mm_hd = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();
    let (_mm_dump_log, _mm_dump_dashboard) = mm_hd.mm_dump();
    log!("Alice log path: {}", mm_hd.log_path.display());

    // Enable platform coin in HD mode
    block_on(task_enable_eth_with_tokens(
        &mm_hd,
        "ETH",
        &[],
        Some(&swap_contract),
        &[GETH_RPC_URL],
        60,
        Some(path_to_address.clone()),
    ));

    let protocol = erc20_dev_conf["protocol"].clone();
    // Enable the custom token in HD mode.
    // Since the contract is already in the coins config, this should fail with an error
    // that specifies the ticker in config so that the user can enable the right coin.
    let err = block_on(enable_erc20_token_v2(
        &mm_hd,
        "QTC",
        Some(protocol.clone()),
        60,
        Some(path_to_address.clone()),
    ))
    .unwrap_err();
    let expected_error_type = "CustomTokenError";
    assert_eq!(err["error_type"], expected_error_type);
    let expected_error_data = json!({
        "DuplicateContractInConfig": {
            "ticker_in_config": "ERC20DEV"
        }
    });
    assert_eq!(err["error_data"], expected_error_data);

    // Another way is to use the `get_token_info` RPC and use the config ticker to enable the token.
    let custom_token_info = block_on(get_token_info(&mm_hd, protocol));
    assert!(custom_token_info.config_ticker.is_some());
    let config_ticker = custom_token_info.config_ticker.unwrap();
    assert_eq!(config_ticker, "ERC20DEV");
    // Parameters passed here are for normal enabling of a coin in config and not for a custom token
    block_on(enable_erc20_token_v2(
        &mm_hd,
        &config_ticker,
        None,
        60,
        Some(path_to_address),
    ))
    .unwrap();

    // Disable the custom token, this to check that it was enabled correctly
    block_on(disable_coin(&mm_hd, &config_ticker, true));
}

#[test]
fn test_v2_eth_erc20_kickstart() {
    test_v2_eth_eth_kickstart_impl("ETH", "ERC20DEV", 2500.0, 2500.0, 0.01)
}

#[test]
fn test_v2_erc20_eth_kickstart() {
    test_v2_eth_eth_kickstart_impl("ERC20DEV", "ETH", 0.0004, 0.0004, 100.0)
}

fn test_v2_eth_eth_kickstart_impl(base: &str, rel: &str, maker_price: f64, taker_price: f64, volume: f64) {
    // Initialize swap addresses and configurations
    let swap_addresses = SwapAddresses::init();
    let swap_v2_contracts = SwapV2TestContracts {
        maker_swap_v2_contract: swap_addresses.swap_v2_contracts.maker_swap_v2_contract.addr_to_string(),
        taker_swap_v2_contract: swap_addresses.swap_v2_contracts.taker_swap_v2_contract.addr_to_string(),
        nft_maker_swap_v2_contract: swap_addresses
            .swap_v2_contracts
            .nft_maker_swap_v2_contract
            .addr_to_string(),
    };
    let swap_contract_address = swap_addresses.swap_contract_address.addr_to_string();
    let erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    let erc20_ticker = erc20_conf.get("coin").unwrap().as_str().unwrap();
    let node = TestNode {
        url: GETH_RPC_URL.to_string(),
    };

    // Helper function for activating coins
    let enable_coin_with_tokens = |mm: &MarketMakerIt, coin: &str, tokens: &[&str]| {
        log!(
            "{:?}",
            block_on(enable_eth_coin_with_tokens_v2(
                mm,
                coin,
                tokens,
                &swap_contract_address,
                swap_v2_contracts.clone(),
                None,
                std::slice::from_ref(&node)
            ))
        );
    };

    // Top-up Bob and Alice
    let (_, bob_priv_key) =
        eth_coin_v2_activation_with_random_privkey(&MM_CTX, ETH, &eth_dev_conf(), swap_addresses, false);
    let (_, alice_priv_key) =
        eth_coin_v2_activation_with_random_privkey(&MM_CTX1, ETH, &eth_dev_conf(), swap_addresses, false);
    let coins = json!([eth_dev_conf(), erc20_conf]);

    // Start Bob
    let mut bob_conf = Mm2TestConf::seednode_trade_v2(&format!("0x{}", hex::encode(bob_priv_key)), &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf.clone(), bob_conf.rpc_password.clone(), None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    log!("Bob log path: {}", mm_bob.log_path.display());

    // Start Alice
    let mut alice_conf = Mm2TestConf::light_node_trade_v2(
        &format!("0x{}", hex::encode(alice_priv_key)),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf.clone(), alice_conf.rpc_password.clone(), None).unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
    log!("Alice log path: {}", mm_alice.log_path.display());

    enable_coin_with_tokens(&mm_bob, ETH, &[erc20_ticker]);
    enable_coin_with_tokens(&mm_alice, ETH, &[erc20_ticker]);

    let bob_base_balance_0 = block_on(my_balance(&mm_bob, base));
    let alice_rel_balance_0 = block_on(my_balance(&mm_alice, rel));
    let bob_rel_balance_0 = block_on(my_balance(&mm_bob, rel));
    let alice_base_balance_0 = block_on(my_balance(&mm_alice, base));
    log!("bob_base_balance_0={} {}", bob_base_balance_0.balance, base);
    log!("alice_rel_balance_0={} {}", alice_rel_balance_0.balance, rel);
    log!("bob_rel_balance_0={} {}", bob_rel_balance_0.balance, rel);
    log!("alice_base_balance_0={} {}", alice_base_balance_0.balance, base);

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[(base, rel)],
        maker_price,
        taker_price,
        volume,
    ));
    log!("{:?}", uuids);
    let parsed_uuids: Vec<Uuid> = uuids.iter().map(|u| u.parse().unwrap()).collect();
    for uuid in uuids.iter() {
        log_swap_status_before_stop(&mm_bob, uuid, "Maker");
        log_swap_status_before_stop(&mm_alice, uuid, "Taker");
    }
    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();

    // Restart Bob and Alice
    bob_conf.conf["dbdir"] = mm_bob.folder.join("DB").to_str().unwrap().into();
    bob_conf.conf["log"] = mm_bob.folder.join("mm2_dup.log").to_str().unwrap().into();

    let mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    log!("Bob log path: {}", mm_bob.log_path.display());

    alice_conf.conf["dbdir"] = mm_alice.folder.join("DB").to_str().unwrap().into();
    alice_conf.conf["log"] = mm_alice.folder.join("mm2_dup.log").to_str().unwrap().into();
    alice_conf.conf["seednodes"] = vec![mm_bob.ip.to_string()].into();

    let mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
    log!("Alice log path: {}", mm_alice.log_path.display());

    verify_coins_needed_for_kickstart(&mm_bob, &[base, rel]);
    verify_coins_needed_for_kickstart(&mm_alice, &[base, rel]);

    enable_coin_with_tokens(&mm_bob, ETH, &[erc20_ticker]);
    enable_coin_with_tokens(&mm_alice, ETH, &[erc20_ticker]);

    // give swaps 1 second to restart
    thread::sleep(Duration::from_secs(1));

    verify_active_swaps(&mm_bob, &parsed_uuids);
    verify_active_swaps(&mm_alice, &parsed_uuids);

    // coins must be virtually locked after kickstart until swap transactions are sent
    verify_locked_amount(&mm_alice, "Taker", rel);
    verify_locked_amount(&mm_bob, "Maker", base);
    for uuid in uuids {
        block_on(wait_for_swap_finished(&mm_bob, &uuid, 240));
        block_on(wait_for_swap_finished(&mm_alice, &uuid, 30));

        let maker_swap_status = block_on(my_swap_status(&mm_bob, &uuid));
        log!("{:?}", maker_swap_status);

        let taker_swap_status = block_on(my_swap_status(&mm_alice, &uuid));
        log!("{:?}", taker_swap_status);
    }
    block_on(check_recent_swaps(&mm_bob, 1));
    block_on(check_recent_swaps(&mm_alice, 1));

    let bob_base_balance_1 = block_on(my_balance(&mm_bob, base));
    let alice_rel_balance_1 = block_on(my_balance(&mm_alice, rel));
    let bob_rel_balance_1 = block_on(my_balance(&mm_bob, rel));
    let alice_base_balance_1 = block_on(my_balance(&mm_alice, base));
    log!("bob_base_balance_1={} {}", bob_base_balance_1.balance, base);
    log!("alice_rel_balance_1={} {}", alice_rel_balance_1.balance, rel);
    log!("bob_rel_balance_1={} {}", bob_rel_balance_1.balance, rel);
    log!("alice_base_balance_1={} {}", alice_base_balance_1.balance, base);

    // check buy/sell balance difference, with tx fee and tolerance
    let check_balance =
        |coin: &str, bal_0: &BigDecimal, bal_1: &BigDecimal, maker_volume: f64, price: Option<f64>, action: &str| {
            let is_token = coins
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c.get("coin").unwrap().as_str().unwrap() == coin)
                .unwrap()["protocol"]["type"]
                .as_str()
                .unwrap()
                == "ERC20";
            let volume = if let Some(price) = price {
                BigDecimal::from_f64(maker_volume).unwrap() * BigDecimal::from_f64(price).unwrap()
            } else {
                BigDecimal::from_f64(maker_volume).unwrap()
            };

            // Set low/high interval for swap total gas fees (if this is a platform coin):
            let (gas_fee_low, gas_fee_high) = if !is_token {
                (
                    BigDecimal::from_f64(0.0).unwrap(),
                    BigDecimal::from_f64(0.0005).unwrap(),
                )
            } else {
                (BigDecimal::from_f64(0.0).unwrap(), BigDecimal::from_f64(0.0).unwrap())
            };

            let vol_tol = BigDecimal::from_f64(0.00001).unwrap();

            if action == "sell" {
                // Check low/high border for the swap result, for sell, as swap volume plus gas fee and plus/minus tolerance
                let low_border = &volume + &gas_fee_low - &vol_tol;
                let high_border = &volume + &gas_fee_high + &vol_tol;
                assert!(
                    bal_0 - bal_1 >= low_border,
                    "{} >= {} {action} {}",
                    bal_0 - bal_1,
                    low_border,
                    coin
                );
                assert!(
                    bal_0 - bal_1 <= high_border,
                    "{} <= {} {action} {}",
                    bal_0 - bal_1,
                    high_border,
                    coin
                );
            } else {
                // Check low/high border for the swap result, for buy, as swap volume minus gas fee and plus/minus tolerance
                let low_border = &volume - &gas_fee_high - &vol_tol;
                let high_border = &volume - &gas_fee_low + &vol_tol;
                assert!(
                    bal_1 - bal_0 >= low_border,
                    "{} >= {} {action} {}",
                    bal_1 - bal_0,
                    low_border,
                    coin
                );
                assert!(
                    bal_1 - bal_0 <= high_border,
                    "{} <= {} {action} {}",
                    bal_1 - bal_0,
                    high_border,
                    coin
                );
            };
        };

    check_balance(
        base,
        &bob_base_balance_0.balance,
        &bob_base_balance_1.balance,
        volume,
        None,
        "sell",
    );
    check_balance(
        rel,
        &alice_rel_balance_0.balance,
        &alice_rel_balance_1.balance,
        volume * 1.02, // 2% DEX fee
        Some(taker_price),
        "sell",
    );
    check_balance(
        rel,
        &bob_rel_balance_0.balance,
        &bob_rel_balance_1.balance,
        volume,
        Some(taker_price),
        "buy",
    );
    check_balance(
        base,
        &alice_base_balance_0.balance,
        &alice_base_balance_1.balance,
        volume,
        None,
        "buy",
    );

    // Disabling coins on both nodes should be successful at this point
    block_on(disable_coin(&mm_bob, ETH, false));
    block_on(disable_coin(&mm_alice, ETH, false));
}

fn log_swap_status_before_stop(mm: &MarketMakerIt, uuid: &str, role: &str) {
    let status = block_on(my_swap_status(mm, uuid));
    log!("{} swap {} status before stop: {:?}", role, uuid, status);
}

fn verify_coins_needed_for_kickstart(mm: &MarketMakerIt, expected_coins: &[&str]) {
    let mut coins_needed = block_on(coins_needed_for_kickstart(mm));
    coins_needed.sort();
    let mut expected_coins = expected_coins.to_vec();
    expected_coins.sort();
    assert_eq!(coins_needed, expected_coins);
}

fn verify_active_swaps(mm: &MarketMakerIt, expected_uuids: &[Uuid]) {
    let active_swaps = block_on(active_swaps(mm));
    assert_eq!(active_swaps.uuids, expected_uuids);
}

fn verify_locked_amount(mm: &MarketMakerIt, role: &str, coin: &str) {
    let locked = block_on(get_locked_amount(mm, coin));
    log!("{} {} locked amount: {:?}", role, coin, locked.locked_amount);
    assert_eq!(locked.coin, coin);
}

// ================================
// USDT (Non-Standard ERC20) Tests
// ================================
// These tests verify that SafeERC20 in the V1 EtomicSwap contract
// correctly handles USDT's non-standard transfer/transferFrom functions
// which don't return a boolean value.

fn send_and_spend_usdt_maker_payment_impl(swap_txfee_policy: SwapGasFeePolicy) {
    let maker_usdt_coin = usdt_coin_with_random_privkey(swap_contract());
    let taker_usdt_coin = usdt_coin_with_random_privkey(swap_contract());

    assert!(block_on(maker_usdt_coin.set_swap_gas_fee_policy(swap_txfee_policy.clone())).is_ok());
    assert!(block_on(taker_usdt_coin.set_swap_gas_fee_policy(swap_txfee_policy)).is_ok());

    let time_lock = now_sec() + 1000;
    let maker_pubkey = maker_usdt_coin.derive_htlc_pubkey(&[]);
    let taker_pubkey = taker_usdt_coin.derive_htlc_pubkey(&[]);
    let secret = &[2; 32];
    let secret_hash_owned = dhash160(secret);
    let secret_hash = secret_hash_owned.as_slice();

    let send_payment_args = SendPaymentArgs {
        time_lock_duration: 1000,
        time_lock,
        other_pubkey: &taker_pubkey,
        secret_hash,
        amount: BigDecimal::from_str("10").unwrap(),
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: now_sec() + 60,
    };
    let usdt_maker_payment = block_on(maker_usdt_coin.send_maker_payment(send_payment_args)).unwrap();
    log!(
        "USDT maker payment tx hash {:02x}",
        usdt_maker_payment.tx_hash_as_bytes()
    );

    let confirm_input = ConfirmPaymentInput {
        payment_tx: usdt_maker_payment.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(taker_usdt_coin.wait_for_confirmations(confirm_input)).unwrap();

    let spend_args = SpendPaymentArgs {
        other_payment_tx: &usdt_maker_payment.tx_hex(),
        time_lock,
        other_pubkey: &maker_pubkey,
        secret,
        secret_hash,
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let payment_spend = block_on(taker_usdt_coin.send_taker_spends_maker_payment(spend_args)).unwrap();
    log!("USDT payment spend tx hash {:02x}", payment_spend.tx_hash_as_bytes());

    let confirm_input = ConfirmPaymentInput {
        payment_tx: payment_spend.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(taker_usdt_coin.wait_for_confirmations(confirm_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: &taker_pubkey,
        secret_hash,
        tx: &usdt_maker_payment.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
    };
    let search_tx = block_on(maker_usdt_coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();

    let expected = FoundSwapTxSpend::Spent(payment_spend);
    assert_eq!(expected, search_tx);
}

#[test]
fn send_and_spend_usdt_maker_payment_legacy_gas_policy() {
    send_and_spend_usdt_maker_payment_impl(SwapGasFeePolicy::Legacy);
}

#[test]
fn send_and_spend_usdt_maker_payment_priority_fee() {
    send_and_spend_usdt_maker_payment_impl(SwapGasFeePolicy::Medium);
}

fn send_and_refund_usdt_maker_payment_impl(swap_txfee_policy: SwapGasFeePolicy) {
    let usdt_coin = usdt_coin_with_random_privkey(swap_contract());
    assert!(block_on(usdt_coin.set_swap_gas_fee_policy(swap_txfee_policy)).is_ok());

    // Use a past time_lock to allow immediate refund
    let time_lock = now_sec() - 100;
    let other_pubkey = &[
        0x02, 0xc6, 0x6e, 0x7d, 0x89, 0x66, 0xb5, 0xc5, 0x55, 0xaf, 0x58, 0x05, 0x98, 0x9d, 0xa9, 0xfb, 0xf8, 0xdb,
        0x95, 0xe1, 0x56, 0x31, 0xce, 0x35, 0x8c, 0x3a, 0x17, 0x10, 0xc9, 0x62, 0x67, 0x90, 0x63,
    ];
    let secret_hash = &[1; 20];

    let send_payment_args = SendPaymentArgs {
        time_lock_duration: 100,
        time_lock,
        other_pubkey,
        secret_hash,
        amount: BigDecimal::from_str("10").unwrap(), // 10 USDT
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: now_sec() + 60,
    };
    let usdt_maker_payment = block_on(usdt_coin.send_maker_payment(send_payment_args)).unwrap();
    log!(
        "USDT maker payment tx hash {:02x}",
        usdt_maker_payment.tx_hash_as_bytes()
    );

    let confirm_input = ConfirmPaymentInput {
        payment_tx: usdt_maker_payment.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(usdt_coin.wait_for_confirmations(confirm_input)).unwrap();

    let refund_args = RefundPaymentArgs {
        payment_tx: &usdt_maker_payment.tx_hex(),
        time_lock,
        other_pubkey,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: secret_hash,
        },
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let payment_refund = block_on(usdt_coin.send_maker_refunds_payment(refund_args)).unwrap();
    log!("USDT payment refund tx hash {:02x}", payment_refund.tx_hash_as_bytes());

    let confirm_input = ConfirmPaymentInput {
        payment_tx: payment_refund.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: now_sec() + 60,
        check_every: 1,
    };
    block_on_f01(usdt_coin.wait_for_confirmations(confirm_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: other_pubkey,
        secret_hash,
        tx: &usdt_maker_payment.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &Some(swap_contract().as_bytes().into()),
        swap_unique_data: &[],
    };
    let search_tx = block_on(usdt_coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();

    let expected = FoundSwapTxSpend::Refunded(payment_refund);
    assert_eq!(expected, search_tx);
}

#[test]
fn send_and_refund_usdt_maker_payment_legacy_gas_policy() {
    send_and_refund_usdt_maker_payment_impl(SwapGasFeePolicy::Legacy);
}

#[test]
fn send_and_refund_usdt_maker_payment_priority_fee() {
    send_and_refund_usdt_maker_payment_impl(SwapGasFeePolicy::Medium);
}

/// Test that get_erc20_token_info correctly fetches USDT token info from chain,
/// verifying that the non-standard decimals() return type (uint256 instead of uint8) is handled.
/// This is critical because USDT's decimals() returns uint256, not the standard uint8.
#[test]
fn test_usdt_get_token_info() {
    // Use ETH coin as web3 provider to query the USDT contract
    let eth_coin = eth_coin_with_random_privkey(swap_contract());
    let usdt_address = geth_usdt_contract();

    // Call get_erc20_token_info which internally calls decimals() on the contract
    // This verifies that the uint256 return type from USDT's decimals() is correctly parsed
    let token_info = block_on(get_erc20_token_info(&eth_coin, usdt_address)).unwrap();

    assert_eq!(token_info.symbol, "USDT");
    assert_eq!(token_info.decimals, 6);
}
