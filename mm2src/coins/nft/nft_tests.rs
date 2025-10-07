use crate::hd_wallet::AddrToString;
use crate::nft::nft_structs::{
    Chain, NftFromMoralis, NftListFilters, NftTransferHistoryFilters, NftTransferHistoryFromMoralis, PhishingDomainReq,
    PhishingDomainRes, SpamContractReq, SpamContractRes, TransferMeta,
};
use crate::nft::storage::db_test_helpers::{get_nft_ctx, nft, nft_list, nft_transfer_history};
use crate::nft::storage::{NftListStorageOps, NftTransferHistoryStorageOps, RemoveNftResult};
use crate::nft::{
    check_moralis_ipfs_bafy, get_domain_from_url, is_malicious, process_metadata_for_spam_link,
    process_text_for_spam_link,
};
use common::cross_test;
use ethereum_types::Address;
use mm2_net::transport::send_post_request_to_uri;
use mm2_number::{BigDecimal, BigUint};
use std::num::NonZeroUsize;
use std::str::FromStr;

const MORALIS_API_ENDPOINT_TEST: &str = "https://moralis-proxy.komodian.info/api/v2";
const TEST_WALLET_ADDR_EVM: &str = "0x394d86994f954ed931b86791b62fe64f4c5dac37";
const BLOCKLIST_API_ENDPOINT: &str = "https://nft.antispam.dragonhound.info";
const TOKEN_ADD: &str = "0xfd913a305d70a60aac4faac70c739563738e1f81";
const TOKEN_ID: &str = "214300044414";
const TX_HASH: &str = "0x1e9f04e9b571b283bde02c98c2a97da39b2bb665b57c1f2b0b733f9b681debbe";
const LOG_INDEX: u32 = 495;

#[cfg(not(target_arch = "wasm32"))]
use mm2_net::native_http::send_request_to_uri;

common::cfg_wasm32! {
    use wasm_bindgen_test::*;
    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
    use mm2_net::wasm::http::send_request_to_uri;
}

cross_test!(test_is_malicious, {
    let token_uri = "https://btrgtrhbyjuyj.xyz/BABYDOGE.json";
    assert!(is_malicious(token_uri).unwrap());

    let token_uri1 = "https://btrgtrhbyjuyj.com/BABYDOGE.json%00";
    assert!(is_malicious(token_uri1).unwrap());
});

cross_test!(test_moralis_ipfs_bafy, {
    let uri = "https://ipfs.moralis.io:2053/ipfs/bafybeifnek24coy5xj5qabdwh24dlp5omq34nzgvazkfyxgnqms4eidsiq/1.json";
    let res_uri = check_moralis_ipfs_bafy(Some(uri));
    let expected = "https://ipfs.io/ipfs/bafybeifnek24coy5xj5qabdwh24dlp5omq34nzgvazkfyxgnqms4eidsiq/1.json";
    assert_eq!(expected, res_uri.unwrap());
});

cross_test!(test_get_domain_from_url, {
    let image_url = "https://public.nftstatic.com/static/nft/res/4df0a5da04174e1e9be04b22a805f605.png";
    let res_domain = get_domain_from_url(Some(image_url));
    let expected = "public.nftstatic.com";
    assert_eq!(expected, res_domain.unwrap());
});

cross_test!(test_invalid_moralis_ipfs_link, {
    let uri = "example.com/bafy?1=ipfs.moralis.io&e=https://";
    let res_uri = check_moralis_ipfs_bafy(Some(uri));
    assert_eq!(uri, res_uri.unwrap());
});

cross_test!(test_check_for_spam_links, {
    let mut spam_text = Some("https://arweave.net".to_string());
    assert!(process_text_for_spam_link(&mut spam_text, true).unwrap());
    let url_redacted = "URL redacted for user protection";
    assert_eq!(url_redacted, spam_text.unwrap());

    let mut spam_text = Some("ftp://123path ".to_string());
    assert!(process_text_for_spam_link(&mut spam_text, true).unwrap());
    let url_redacted = "URL redacted for user protection";
    assert_eq!(url_redacted, spam_text.unwrap());

    let mut spam_text = Some("/192.168.1.1/some.example.org?type=A".to_string());
    assert!(process_text_for_spam_link(&mut spam_text, true).unwrap());
    let url_redacted = "URL redacted for user protection";
    assert_eq!(url_redacted, spam_text.unwrap());

    let mut spam_text = Some(r"C:\Users\path\".to_string());
    assert!(process_text_for_spam_link(&mut spam_text, true).unwrap());
    let url_redacted = "URL redacted for user protection";
    assert_eq!(url_redacted, spam_text.unwrap());

    let mut valid_text = Some("Hello my name is NFT (The best ever!)".to_string());
    assert!(!process_text_for_spam_link(&mut valid_text, true).unwrap());
    assert_eq!("Hello my name is NFT (The best ever!)", valid_text.unwrap());

    let mut nft = nft();
    assert!(process_metadata_for_spam_link(&mut nft, true).unwrap());
    let meta_redacted = "{\"name\":\"URL redacted for user protection\",\"image\":\"https://tikimetadata.s3.amazonaws.com/tiki_box.png\"}";
    assert_eq!(meta_redacted, nft.common.metadata.unwrap())
});

cross_test!(test_moralis_requests, {
    let uri_nft_list = format!("{MORALIS_API_ENDPOINT_TEST}/{TEST_WALLET_ADDR_EVM}/nft?chain=POLYGON&format=decimal");
    let response_nft_list = send_request_to_uri(uri_nft_list.as_str(), None).await.unwrap();
    let nfts_list = response_nft_list["result"].as_array().unwrap();
    for nft_json in nfts_list {
        let nft_moralis: NftFromMoralis = serde_json::from_str(&nft_json.to_string()).unwrap();
        assert_eq!(TEST_WALLET_ADDR_EVM, nft_moralis.common.owner_of.addr_to_string());
    }

    let uri_history =
        format!("{MORALIS_API_ENDPOINT_TEST}/{TEST_WALLET_ADDR_EVM}/nft/transfers?chain=POLYGON&format=decimal");
    let response_transfer_history = send_request_to_uri(uri_history.as_str(), None).await.unwrap();
    let mut transfer_list = response_transfer_history["result"].as_array().unwrap().clone();
    assert!(!transfer_list.is_empty());
    let first_transfer = transfer_list.remove(transfer_list.len() - 1);
    let transfer_moralis: NftTransferHistoryFromMoralis = serde_json::from_str(&first_transfer.to_string()).unwrap();
    assert_eq!(
        TEST_WALLET_ADDR_EVM,
        transfer_moralis.common.to_address.addr_to_string()
    );

    let uri_meta = format!(
        "{MORALIS_API_ENDPOINT_TEST}/nft/0xed55e4477b795eaa9bb4bca24df42214e1a05c18/1111777?chain=POLYGON&format=decimal"
    );
    let response_meta = send_request_to_uri(uri_meta.as_str(), None).await.unwrap();
    let nft_moralis: NftFromMoralis = serde_json::from_str(&response_meta.to_string()).unwrap();
    assert_eq!(42563567, nft_moralis.block_number.0);
});

cross_test!(test_antispam_scan_endpoints, {
    let req_spam = SpamContractReq {
        network: Chain::Eth,
        addresses: "0x0ded8542fc8b2b4e781b96e99fee6406550c9b7c,0x8d1355b65da254f2cc4611453adfa8b7a13f60ee".to_string(),
    };
    let uri_contract = format!("{BLOCKLIST_API_ENDPOINT}/api/blocklist/contract/scan");
    let req_json = serde_json::to_string(&req_spam).unwrap();
    let contract_scan_res = send_post_request_to_uri(uri_contract.as_str(), req_json).await.unwrap();
    let spam_res: SpamContractRes = serde_json::from_slice(&contract_scan_res).unwrap();
    assert!(spam_res
        .result
        .get(&Address::from_str("0x0ded8542fc8b2b4e781b96e99fee6406550c9b7c").unwrap())
        .unwrap());
    assert!(spam_res
        .result
        .get(&Address::from_str("0x8d1355b65da254f2cc4611453adfa8b7a13f60ee").unwrap())
        .unwrap());

    let req_phishing = PhishingDomainReq {
        domains: "disposal-account-case-1f677.web.app,defi8090.vip".to_string(),
    };
    let req_json = serde_json::to_string(&req_phishing).unwrap();
    let uri_domain = format!("{BLOCKLIST_API_ENDPOINT}/api/blocklist/domain/scan");
    let domain_scan_res = send_post_request_to_uri(uri_domain.as_str(), req_json).await.unwrap();
    let phishing_res: PhishingDomainRes = serde_json::from_slice(&domain_scan_res).unwrap();
    assert!(phishing_res.result.get("disposal-account-case-1f677.web.app").unwrap());
});

// Disabled on Linux: https://github.com/KomodoPlatform/komodo-defi-framework/issues/2367
cross_test!(
    test_camo,
    {
        use crate::nft::nft_structs::UriMeta;

        let hex_token_uri = hex::encode("https://tikimetadata.s3.amazonaws.com/tiki_box.json");
        let uri_decode = format!("{BLOCKLIST_API_ENDPOINT}/url/decode/{hex_token_uri}");
        let decode_res = send_request_to_uri(&uri_decode, None).await.unwrap();
        let uri_meta: UriMeta = serde_json::from_value(decode_res).unwrap();
        assert_eq!(
            uri_meta.raw_image_url.unwrap(),
            "https://tikimetadata.s3.amazonaws.com/tiki_box.png"
        );
    },
    target_os = "macos",
    target_os = "windows"
);

cross_test!(test_add_get_nfts, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let nft_list = nft_list();
    storage.add_nfts_to_list(chain, nft_list, 28056726).await.unwrap();

    let token_id = BigUint::from_str(TOKEN_ID).unwrap();
    let nft = storage
        .get_nft(&chain, TOKEN_ADD.to_string(), token_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(nft.block_number, 28056721);
});

cross_test!(test_last_nft_block, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let nft_list = nft_list();
    storage.add_nfts_to_list(chain, nft_list, 28056726).await.unwrap();

    let last_block = NftListStorageOps::get_last_block_number(&storage, &chain)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(last_block, 28056726);
});

cross_test!(test_nft_list, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let nft_list = nft_list();
    storage.add_nfts_to_list(chain, nft_list, 28056726).await.unwrap();

    let nft_list = storage
        .get_nft_list(vec![chain], false, 1, Some(NonZeroUsize::new(3).unwrap()), None)
        .await
        .unwrap();
    assert_eq!(nft_list.nfts.len(), 1);
    let nft = nft_list.nfts.first().unwrap();
    assert_eq!(nft.block_number, 28056721);
    assert_eq!(nft_list.skipped, 2);
    assert_eq!(nft_list.total, 4);
});

cross_test!(test_remove_nft, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let nft_list = nft_list();
    storage.add_nfts_to_list(chain, nft_list, 28056726).await.unwrap();

    let token_id = BigUint::from_str(TOKEN_ID).unwrap();
    let remove_rslt = storage
        .remove_nft_from_list(&chain, TOKEN_ADD.to_string(), token_id, 28056800)
        .await
        .unwrap();
    assert_eq!(remove_rslt, RemoveNftResult::NftRemoved);
    let list_len = storage
        .get_nft_list(vec![chain], true, 1, None, None)
        .await
        .unwrap()
        .nfts
        .len();
    assert_eq!(list_len, 3);
    let last_scanned_block = storage.get_last_scanned_block(&chain).await.unwrap().unwrap();
    assert_eq!(last_scanned_block, 28056800);
});

cross_test!(test_nft_amount, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let mut nft = nft();
    storage
        .add_nfts_to_list(chain, vec![nft.clone()], 25919780)
        .await
        .unwrap();

    nft.common.amount -= BigDecimal::from(1);
    storage.update_nft_amount(&chain, nft.clone(), 25919800).await.unwrap();
    let amount = storage
        .get_nft_amount(&chain, nft.common.token_address.addr_to_string(), nft.token_id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(amount, "1");
    let last_scanned_block = storage.get_last_scanned_block(&chain).await.unwrap().unwrap();
    assert_eq!(last_scanned_block, 25919800);

    nft.common.amount += BigDecimal::from(1);
    nft.block_number = 25919900;
    storage
        .update_nft_amount_and_block_number(&chain, nft.clone())
        .await
        .unwrap();
    let amount = storage
        .get_nft_amount(&chain, nft.common.token_address.addr_to_string(), nft.token_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(amount, "2");
    let last_scanned_block = storage.get_last_scanned_block(&chain).await.unwrap().unwrap();
    assert_eq!(last_scanned_block, 25919900);
});

cross_test!(test_refresh_metadata, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let new_symbol = "NEW_SYMBOL";
    let mut nft = nft();
    storage
        .add_nfts_to_list(chain, vec![nft.clone()], 25919780)
        .await
        .unwrap();
    nft.common.symbol = Some(new_symbol.to_string());
    drop_mutability!(nft);
    let token_add = nft.common.token_address.addr_to_string();
    let token_id = nft.token_id.clone();
    storage.refresh_nft_metadata(&chain, nft).await.unwrap();
    let nft_upd = storage.get_nft(&chain, token_add, token_id).await.unwrap().unwrap();
    assert_eq!(new_symbol.to_string(), nft_upd.common.symbol.unwrap());
});

cross_test!(test_update_nft_spam_by_token_address, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let nft_list = nft_list();
    storage.add_nfts_to_list(chain, nft_list, 28056726).await.unwrap();

    storage
        .update_nft_spam_by_token_address(&chain, TOKEN_ADD.to_string(), true)
        .await
        .unwrap();
    let nfts = storage
        .get_nfts_by_token_address(chain, TOKEN_ADD.to_string())
        .await
        .unwrap();
    for nft in nfts {
        assert!(nft.common.possible_spam);
    }
});

cross_test!(test_exclude_nft_spam, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let nft_list = nft_list();
    storage.add_nfts_to_list(chain, nft_list, 28056726).await.unwrap();

    let filters = NftListFilters {
        exclude_spam: true,
        exclude_phishing: false,
    };
    let nft_list = storage
        .get_nft_list(vec![chain], true, 1, None, Some(filters))
        .await
        .unwrap();
    assert_eq!(nft_list.nfts.len(), 3);
});

cross_test!(test_get_animation_external_domains, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let nft_list = nft_list();
    storage.add_nfts_to_list(chain, nft_list, 28056726).await.unwrap();

    let domains = storage.get_animation_external_domains(&chain).await.unwrap();
    assert_eq!(2, domains.len());
    assert!(domains.contains("tikimetadata.s3.amazonaws.com"));
    assert!(domains.contains("public.nftstatic.com"));
});

cross_test!(test_update_nft_phishing_by_domain, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let nft_list = nft_list();
    storage.add_nfts_to_list(chain, nft_list, 28056726).await.unwrap();

    let domains = vec![
        "tikimetadata.s3.amazonaws.com".to_string(),
        "public.nftstatic.com".to_string(),
    ];
    for domain in domains.into_iter() {
        storage
            .update_nft_phishing_by_domain(&chain, domain, true)
            .await
            .unwrap();
    }
    let nfts = storage
        .get_nft_list(vec![chain], true, 1, None, None)
        .await
        .unwrap()
        .nfts;
    for nft in nfts.into_iter() {
        assert!(nft.possible_phishing);
    }
});

cross_test!(test_exclude_nft_phishing_spam, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let nft_list = nft_list();
    storage.add_nfts_to_list(chain, nft_list, 28056726).await.unwrap();

    storage
        .update_nft_phishing_by_domain(&chain, "tikimetadata.s3.amazonaws.com".to_string(), true)
        .await
        .unwrap();
    let filters = NftListFilters {
        exclude_spam: true,
        exclude_phishing: true,
    };
    let nfts = storage
        .get_nft_list(vec![chain], true, 1, None, Some(filters))
        .await
        .unwrap()
        .nfts;
    assert_eq!(nfts.len(), 2);
});

cross_test!(test_clear_nft, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let nft = nft();
    storage.add_nfts_to_list(chain, vec![nft], 28056726).await.unwrap();

    storage.clear_nft_data(&chain).await.unwrap();
    test_clear_nft_target(&storage, &chain).await;
});

cross_test!(test_clear_all_nft, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftListStorageOps::init(&storage, &chain).await.unwrap();
    let nft = nft();
    storage.add_nfts_to_list(chain, vec![nft], 28056726).await.unwrap();

    storage.clear_all_nft_data().await.unwrap();
    test_clear_nft_target(&storage, &chain).await;
});

#[cfg(not(target_arch = "wasm32"))]
async fn test_clear_nft_target<S: NftListStorageOps>(storage: &S, chain: &Chain) {
    let is_initialized = NftListStorageOps::is_initialized(storage, chain).await.unwrap();
    assert!(!is_initialized);

    let is_err = storage.get_nft_list(vec![*chain], false, 10, None, None).await.is_err();
    assert!(is_err);

    let is_err = storage.get_last_scanned_block(chain).await.is_err();
    assert!(is_err);
}

#[cfg(target_arch = "wasm32")]
async fn test_clear_nft_target<S: NftListStorageOps>(storage: &S, chain: &Chain) {
    let nft_list = storage.get_nft_list(vec![*chain], true, 1, None, None).await.unwrap();
    assert!(nft_list.nfts.is_empty());
}

cross_test!(test_add_get_transfers, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    let token_id = BigUint::from_str(TOKEN_ID).unwrap();
    let transfer1 = storage
        .get_transfers_by_token_addr_id(chain, TOKEN_ADD.to_string(), token_id)
        .await
        .unwrap()
        .first()
        .unwrap()
        .clone();
    assert_eq!(transfer1.block_number, 28056721);
    let transfer2 = storage
        .get_transfer_by_tx_hash_log_index_token_id(
            &chain,
            TX_HASH.to_string(),
            LOG_INDEX,
            BigUint::from_str("214300047253").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transfer2.block_number, 28056726);
    let transfer_from = storage.get_transfers_from_block(chain, 28056721).await.unwrap();
    assert_eq!(transfer_from.len(), 3);
});

cross_test!(test_last_transfer_block, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    let last_block = NftTransferHistoryStorageOps::get_last_block_number(&storage, &chain)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(last_block, 28056726);
});

cross_test!(test_transfer_history, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    let transfer_history = storage
        .get_transfer_history(vec![chain], false, 1, Some(NonZeroUsize::new(3).unwrap()), None)
        .await
        .unwrap();
    assert_eq!(transfer_history.transfer_history.len(), 1);
    let transfer = transfer_history.transfer_history.first().unwrap();
    assert_eq!(transfer.block_number, 28056721);
    assert_eq!(transfer_history.skipped, 2);
    assert_eq!(transfer_history.total, 4);
});

cross_test!(test_transfer_history_filters, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    let filters = NftTransferHistoryFilters {
        receive: true,
        send: false,
        from_date: None,
        to_date: None,
        exclude_spam: false,
        exclude_phishing: false,
    };

    let filters1 = NftTransferHistoryFilters {
        receive: false,
        send: false,
        from_date: None,
        to_date: Some(1677166110),
        exclude_spam: false,
        exclude_phishing: false,
    };

    let filters2 = NftTransferHistoryFilters {
        receive: false,
        send: false,
        from_date: Some(1677166110),
        to_date: Some(1683627417),
        exclude_spam: false,
        exclude_phishing: false,
    };

    let transfer_history = storage
        .get_transfer_history(vec![chain], true, 1, None, Some(filters))
        .await
        .unwrap();
    assert_eq!(transfer_history.transfer_history.len(), 4);
    let transfer = transfer_history.transfer_history.first().unwrap();
    assert_eq!(transfer.block_number, 28056726);

    let transfer_history1 = storage
        .get_transfer_history(vec![chain], true, 1, None, Some(filters1))
        .await
        .unwrap();
    assert_eq!(transfer_history1.transfer_history.len(), 1);
    let transfer1 = transfer_history1.transfer_history.first().unwrap();
    assert_eq!(transfer1.block_number, 25919780);

    let transfer_history2 = storage
        .get_transfer_history(vec![chain], true, 1, None, Some(filters2))
        .await
        .unwrap();
    assert_eq!(transfer_history2.transfer_history.len(), 2);
    let transfer_0 = transfer_history2.transfer_history.first().unwrap();
    assert_eq!(transfer_0.block_number, 28056721);
    let transfer_1 = transfer_history2.transfer_history.get(1).unwrap();
    assert_eq!(transfer_1.block_number, 25919780);
});

cross_test!(test_get_update_transfer_meta, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    let vec_token_add_id = storage.get_transfers_with_empty_meta(chain).await.unwrap();
    assert_eq!(vec_token_add_id.len(), 2);

    let token_add = "0x5c7d6712dfaf0cb079d48981781c8705e8417ca0".to_string();
    let transfer_meta = TransferMeta {
        token_address: token_add.clone(),
        token_id: Default::default(),
        token_uri: None,
        token_domain: None,
        collection_name: None,
        image_url: None,
        image_domain: None,
        token_name: Some("Tiki box".to_string()),
    };
    storage
        .update_transfers_meta_by_token_addr_id(&chain, transfer_meta, true)
        .await
        .unwrap();
    let transfer_upd = storage
        .get_transfers_by_token_addr_id(chain, token_add, Default::default())
        .await
        .unwrap();
    let transfer_upd = transfer_upd.first().unwrap();
    assert_eq!(transfer_upd.token_name, Some("Tiki box".to_string()));
    assert!(transfer_upd.common.possible_spam);
});

cross_test!(test_update_transfer_spam_by_token_address, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    storage
        .update_transfer_spam_by_token_address(&chain, TOKEN_ADD.to_string(), true)
        .await
        .unwrap();
    let transfers = storage
        .get_transfers_by_token_address(chain, TOKEN_ADD.to_string())
        .await
        .unwrap();
    for transfers in transfers {
        assert!(transfers.common.possible_spam);
    }
});

cross_test!(test_get_token_addresses, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    let token_addresses = storage.get_token_addresses(chain).await.unwrap();
    assert_eq!(token_addresses.len(), 2);
});

cross_test!(test_exclude_transfer_spam, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    let filters = NftTransferHistoryFilters {
        receive: true,
        send: true,
        from_date: None,
        to_date: None,
        exclude_spam: true,
        exclude_phishing: false,
    };
    let transfer_history = storage
        .get_transfer_history(vec![chain], true, 1, None, Some(filters))
        .await
        .unwrap();
    assert_eq!(transfer_history.transfer_history.len(), 3);
});

cross_test!(test_get_domains, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    let domains = storage.get_domains(&chain).await.unwrap();
    assert_eq!(2, domains.len());
    assert!(domains.contains("tikimetadata.s3.amazonaws.com"));
    assert!(domains.contains("public.nftstatic.com"));
});

cross_test!(test_update_transfer_phishing_by_domain, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    let domains = vec![
        "tikimetadata.s3.amazonaws.com".to_string(),
        "public.nftstatic.com".to_string(),
    ];
    for domain in domains.into_iter() {
        storage
            .update_transfer_phishing_by_domain(&chain, domain, true)
            .await
            .unwrap();
    }
    let transfers = storage
        .get_transfer_history(vec![chain], true, 1, None, None)
        .await
        .unwrap()
        .transfer_history;
    for transfer in transfers.into_iter() {
        assert!(transfer.possible_phishing);
    }
});

cross_test!(test_exclude_transfer_phishing_spam, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    storage
        .update_transfer_phishing_by_domain(&chain, "tikimetadata.s3.amazonaws.com".to_string(), true)
        .await
        .unwrap();
    let filters = NftTransferHistoryFilters {
        receive: true,
        send: true,
        from_date: None,
        to_date: None,
        exclude_spam: false,
        exclude_phishing: true,
    };
    let transfers = storage
        .get_transfer_history(vec![chain], true, 1, None, Some(filters))
        .await
        .unwrap()
        .transfer_history;
    assert_eq!(transfers.len(), 2);

    let filters1 = NftTransferHistoryFilters {
        receive: true,
        send: true,
        from_date: None,
        to_date: None,
        exclude_spam: true,
        exclude_phishing: true,
    };
    let transfers = storage
        .get_transfer_history(vec![chain], true, 1, None, Some(filters1))
        .await
        .unwrap()
        .transfer_history;
    assert_eq!(transfers.len(), 1);
});

cross_test!(test_clear_history, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    storage.clear_history_data(&chain).await.unwrap();
    test_clear_history_target(&storage, &chain).await;
});

cross_test!(test_clear_all_history, {
    let chain = Chain::Bsc;
    let nft_ctx = get_nft_ctx(&chain).await;
    let storage = nft_ctx.lock_db().await.unwrap();
    NftTransferHistoryStorageOps::init(&storage, &chain).await.unwrap();
    let transfers = nft_transfer_history();
    storage.add_transfers_to_history(chain, transfers).await.unwrap();

    storage.clear_all_history_data().await.unwrap();
    test_clear_history_target(&storage, &chain).await;
});

#[cfg(not(target_arch = "wasm32"))]
async fn test_clear_history_target<S: NftTransferHistoryStorageOps>(storage: &S, chain: &Chain) {
    let is_init = NftTransferHistoryStorageOps::is_initialized(storage, chain)
        .await
        .unwrap();
    assert!(!is_init);
}

#[cfg(target_arch = "wasm32")]
async fn test_clear_history_target<S: NftTransferHistoryStorageOps>(storage: &S, chain: &Chain) {
    let transfer_list = storage
        .get_transfer_history(vec![*chain], true, 1, None, None)
        .await
        .unwrap();
    assert!(transfer_list.transfer_history.is_empty());
}
