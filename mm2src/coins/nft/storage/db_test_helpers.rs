use crate::nft::nft_structs::{
    Chain, ContractType, Nft, NftCommon, NftCtx, NftTransferCommon, NftTransferHistory, TransferStatus, UriMeta,
};
use ethereum_types::Address;
use mm2_number::{BigDecimal, BigUint};
#[cfg(not(target_arch = "wasm32"))]
use mm2_test_helpers::for_tests::mm_ctx_with_custom_async_db;
#[cfg(target_arch = "wasm32")]
use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;
use std::str::FromStr;
use std::sync::Arc;

pub(crate) fn nft() -> Nft {
    Nft {
        common: NftCommon {
            token_address: Address::from_str("0x5c7d6712dfaf0cb079d48981781c8705e8417ca0").unwrap(),
            amount: BigDecimal::from_str("2").unwrap(),
            owner_of: Address::from_str("0xf622a6c52c94b500542e2ae6bcad24c53bc5b6a2").unwrap(),
            token_hash: Some("b34ddf294013d20a6d70691027625839".to_string()),
            collection_name: None,
            symbol: None,
            token_uri: Some("https://tikimetadata.s3.amazonaws.com/tiki_box.json".to_string()),
            token_domain: None,
            metadata: Some(
                "{\"name\":\"https://arweave.net\",\"image\":\"https://tikimetadata.s3.amazonaws.com/tiki_box.png\"}"
                    .to_string(),
            ),
            last_token_uri_sync: Some("2023-02-07T17:10:08.402Z".to_string()),
            last_metadata_sync: Some("2023-02-07T17:10:16.858Z".to_string()),
            minter_address: Some("ERC1155 tokens don't have a single minter".to_string()),
            possible_spam: true,
        },
        chain: Chain::Bsc,
        token_id: Default::default(),
        block_number_minted: Some(25465916),
        block_number: 25919780,
        contract_type: ContractType::Erc1155,
        possible_phishing: false,
        uri_meta: UriMeta {
            image_url: Some("https://tikimetadata.s3.amazonaws.com/tiki_box.png".to_string()),
            raw_image_url: Some("https://tikimetadata.s3.amazonaws.com/tiki_box.png".to_string()),
            token_name: None,
            description: Some("Born to usher in Bull markets.".to_string()),
            attributes: None,
            animation_url: None,
            animation_domain: None,
            external_url: None,
            external_domain: None,
            image_details: None,
            image_domain: Some("tikimetadata.s3.amazonaws.com".to_string()),
        },
    }
}

pub(crate) fn nft_list() -> Vec<Nft> {
    let nft = Nft {
        common: NftCommon {
            token_address: Address::from_str("0x5c7d6712dfaf0cb079d48981781c8705e8417ca0").unwrap(),
            amount: BigDecimal::from_str("2").unwrap(),
            owner_of: Address::from_str("0xf622a6c52c94b500542e2ae6bcad24c53bc5b6a2").unwrap(),
            token_hash: Some("b34ddf294013d20a6d70691027625839".to_string()),
            collection_name: None,
            symbol: None,
            token_uri: Some("https://tikimetadata.s3.amazonaws.com/tiki_box.json".to_string()),
            token_domain: None,
            metadata: Some("{\"name\":\"Tiki box\"}".to_string()),
            last_token_uri_sync: Some("2023-02-07T17:10:08.402Z".to_string()),
            last_metadata_sync: Some("2023-02-07T17:10:16.858Z".to_string()),
            minter_address: Some("ERC1155 tokens don't have a single minter".to_string()),
            possible_spam: false,
        },
        chain: Chain::Bsc,
        token_id: Default::default(),
        block_number_minted: Some(25465916),
        block_number: 25919780,
        contract_type: ContractType::Erc1155,
        possible_phishing: false,
        uri_meta: UriMeta {
            image_url: Some("https://tikimetadata.s3.amazonaws.com/tiki_box.png".to_string()),
            raw_image_url: None,
            token_name: None,
            description: Some("Born to usher in Bull markets.".to_string()),
            attributes: None,
            animation_url: None,
            animation_domain: Some("tikimetadata.s3.amazonaws.com".to_string()),
            external_url: None,
            external_domain: None,
            image_details: None,
            image_domain: None,
        },
    };

    let nft1 = Nft {
        common: NftCommon {
            token_address: Address::from_str("0xfd913a305d70a60aac4faac70c739563738e1f81").unwrap(),
            amount: BigDecimal::from_str("1").unwrap(),
            owner_of: Address::from_str("0xf622a6c52c94b500542e2ae6bcad24c53bc5b6a2").unwrap(),
            token_hash: Some("c5d1cfd75a0535b0ec750c0156e6ddfe".to_string()),
            collection_name: Some("Binance NFT Mystery Box-Back to Blockchain Future".to_string()),
            symbol: Some("BMBBBF".to_string()),
            token_uri: Some("https://public.nftstatic.com/static/nft/BSC/BMBBBF/214300047252".to_string()),
            token_domain: Some("public.nftstatic.com".to_string()),
            metadata: Some(
                "{\"image\":\"https://public.nftstatic.com/static/nft/res/4df0a5da04174e1e9be04b22a805f605.png\"}"
                    .to_string(),
            ),
            last_token_uri_sync: Some("2023-02-16T16:35:52.392Z".to_string()),
            last_metadata_sync: Some("2023-02-16T16:36:04.283Z".to_string()),
            minter_address: Some("0xdbdeb0895f3681b87fb3654b5cf3e05546ba24a9".to_string()),
            possible_spam: true,
        },
        chain: Chain::Bsc,
        token_id: BigUint::from_str("214300047252").unwrap(),
        block_number_minted: Some(25721963),
        block_number: 28056726,
        contract_type: ContractType::Erc721,
        possible_phishing: false,
        uri_meta: UriMeta {
            image_url: Some(
                "https://public.nftstatic.com/static/nft/res/4df0a5da04174e1e9be04b22a805f605.png".to_string(),
            ),
            raw_image_url: None,
            token_name: Some("Nebula Nodes".to_string()),
            description: Some("Interchain nodes".to_string()),
            attributes: None,
            animation_url: None,
            animation_domain: None,
            external_url: None,
            external_domain: None,
            image_details: None,
            image_domain: None,
        },
    };

    let nft2 = Nft {
        common: NftCommon {
            token_address: Address::from_str("0xfd913a305d70a60aac4faac70c739563738e1f81").unwrap(),
            amount: BigDecimal::from_str("1").unwrap(),
            owner_of: Address::from_str("0xf622a6c52c94b500542e2ae6bcad24c53bc5b6a2").unwrap(),
            token_hash: Some("c5d1cfd75a0535b0ec750c0156e6ddfe".to_string()),
            collection_name: Some("Binance NFT Mystery Box-Back to Blockchain Future".to_string()),
            symbol: Some("BMBBBF".to_string()),
            token_uri: Some("https://public.nftstatic.com/static/nft/BSC/BMBBBF/214300047252".to_string()),
            token_domain: None,
            metadata: Some(
                "{\"image\":\"https://public.nftstatic.com/static/nft/res/4df0a5da04174e1e9be04b22a805f605.png\"}"
                    .to_string(),
            ),
            last_token_uri_sync: Some("2023-02-16T16:35:52.392Z".to_string()),
            last_metadata_sync: Some("2023-02-16T16:36:04.283Z".to_string()),
            minter_address: Some("0xdbdeb0895f3681b87fb3654b5cf3e05546ba24a9".to_string()),
            possible_spam: false,
        },
        chain: Chain::Bsc,
        token_id: BigUint::from_str("214300047253").unwrap(),
        block_number_minted: Some(25721963),
        block_number: 28056726,
        contract_type: ContractType::Erc721,
        possible_phishing: false,
        uri_meta: UriMeta {
            image_url: Some(
                "https://public.nftstatic.com/static/nft/res/4df0a5da04174e1e9be04b22a805f605.png".to_string(),
            ),
            raw_image_url: None,
            token_name: Some("Nebula Nodes".to_string()),
            description: Some("Interchain nodes".to_string()),
            attributes: None,
            animation_url: None,
            animation_domain: None,
            external_url: None,
            external_domain: None,
            image_details: None,
            image_domain: Some("public.nftstatic.com".to_string()),
        },
    };

    let nft3 = Nft {
        common: NftCommon {
            token_address: Address::from_str("0xfd913a305d70a60aac4faac70c739563738e1f81").unwrap(),
            amount: BigDecimal::from_str("1").unwrap(),
            owner_of: Address::from_str("0xf622a6c52c94b500542e2ae6bcad24c53bc5b6a2").unwrap(),
            token_hash: Some("125f8f4e952e107c257960000b4b250e".to_string()),
            collection_name: Some("Binance NFT Mystery Box-Back to Blockchain Future".to_string()),
            symbol: Some("BMBBBF".to_string()),
            token_uri: Some("https://public.nftstatic.com/static/nft/BSC/BMBBBF/214300044414".to_string()),
            token_domain: None,
            metadata: Some(
                "{\"image\":\"https://public.nftstatic.com/static/nft/res/4df0a5da04174e1e9be04b22a805f605.png\"}"
                    .to_string(),
            ),
            last_token_uri_sync: Some("2023-02-19T19:12:09.471Z".to_string()),
            last_metadata_sync: Some("2023-02-19T19:12:18.080Z".to_string()),
            minter_address: Some("0xdbdeb0895f3681b87fb3654b5cf3e05546ba24a9".to_string()),
            possible_spam: false,
        },
        chain: Chain::Bsc,
        token_id: BigUint::from_str("214300044414").unwrap(),
        block_number_minted: Some(25810308),
        block_number: 28056721,
        contract_type: ContractType::Erc721,
        possible_phishing: false,
        uri_meta: UriMeta {
            image_url: Some(
                "https://public.nftstatic.com/static/nft/res/4df0a5da04174e1e9be04b22a805f605.png".to_string(),
            ),
            raw_image_url: None,
            token_name: Some("Nebula Nodes".to_string()),
            description: Some("Interchain nodes".to_string()),
            attributes: None,
            animation_url: None,
            animation_domain: None,
            external_url: None,
            external_domain: Some("public.nftstatic.com".to_string()),
            image_details: None,
            image_domain: None,
        },
    };
    vec![nft, nft1, nft2, nft3]
}

pub(crate) fn nft_transfer_history() -> Vec<NftTransferHistory> {
    let transfer = NftTransferHistory {
        common: NftTransferCommon {
            block_hash: Some("0xcb41654fc5cf2bf5d7fd3f061693405c74d419def80993caded0551ecfaeaae5".to_string()),
            transaction_hash: "0x9c16b962f63eead1c5d2355cc9037dde178b14b53043c57eb40c27964d22ae6a".to_string(),
            transaction_index: Some(57),
            log_index: 139,
            value: Default::default(),
            transaction_type: Some("Single".to_string()),
            token_address: Address::from_str("0x5c7d6712dfaf0cb079d48981781c8705e8417ca0").unwrap(),
            from_address: Address::from_str("0x4ff0bbc9b64d635a4696d1a38554fb2529c103ff").unwrap(),
            to_address: Address::from_str("0xf622a6c52c94b500542e2ae6bcad24c53bc5b6a2").unwrap(),
            amount: BigDecimal::from_str("1").unwrap(),
            verified: Some(1),
            operator: Some("0x4ff0bbc9b64d635a4696d1a38554fb2529c103ff".to_string()),
            possible_spam: false,
        },
        chain: Chain::Bsc,
        token_id: Default::default(),
        block_number: 25919780,
        block_timestamp: 1677166110,
        contract_type: ContractType::Erc1155,
        token_uri: None,
        token_domain: Some("tikimetadata.s3.amazonaws.com".to_string()),
        collection_name: None,
        image_url: None,
        image_domain: None,
        token_name: None,
        status: TransferStatus::Receive,
        possible_phishing: false,
        fee_details: None,
        confirmations: 0,
    };

    let transfer1 = NftTransferHistory {
        common: NftTransferCommon {
            block_hash: Some("0x3d68b78391fb3cf8570df27036214f7e9a5a6a45d309197936f51d826041bfe7".to_string()),
            transaction_hash: "0x1e9f04e9b571b283bde02c98c2a97da39b2bb665b57c1f2b0b733f9b681debbe".to_string(),
            transaction_index: Some(198),
            log_index: 495,
            value: Default::default(),
            transaction_type: Some("Batch".to_string()),
            token_address: Address::from_str("0xfd913a305d70a60aac4faac70c739563738e1f81").unwrap(),
            from_address: Address::from_str("0x6fad0ec6bb76914b2a2a800686acc22970645820").unwrap(),
            to_address: Address::from_str("0xf622a6c52c94b500542e2ae6bcad24c53bc5b6a2").unwrap(),
            amount: BigDecimal::from_str("1").unwrap(),
            verified: Some(1),
            operator: None,
            possible_spam: true,
        },
        chain: Chain::Bsc,
        token_id: BigUint::from_str("214300047252").unwrap(),
        block_number: 28056726,
        block_timestamp: 1683627432,
        contract_type: ContractType::Erc721,
        token_uri: None,
        token_domain: Some("public.nftstatic.com".to_string()),
        collection_name: None,
        image_url: None,
        image_domain: None,
        token_name: None,
        status: TransferStatus::Receive,
        possible_phishing: false,
        fee_details: None,
        confirmations: 0,
    };

    // Same as transfer1 (identical tx hash and log index) but with different token_id, meaning that transfer1 and transfer2 are part of one batch/multi token transaction
    let transfer2 = NftTransferHistory {
        common: NftTransferCommon {
            block_hash: Some("0x3d68b78391fb3cf8570df27036214f7e9a5a6a45d309197936f51d826041bfe7".to_string()),
            transaction_hash: "0x1e9f04e9b571b283bde02c98c2a97da39b2bb665b57c1f2b0b733f9b681debbe".to_string(),
            transaction_index: Some(198),
            log_index: 495,
            value: Default::default(),
            transaction_type: Some("Batch".to_string()),
            token_address: Address::from_str("0xfd913a305d70a60aac4faac70c739563738e1f81").unwrap(),
            from_address: Address::from_str("0x6fad0ec6bb76914b2a2a800686acc22970645820").unwrap(),
            to_address: Address::from_str("0xf622a6c52c94b500542e2ae6bcad24c53bc5b6a2").unwrap(),
            amount: BigDecimal::from_str("1").unwrap(),
            verified: Some(1),
            operator: None,
            possible_spam: false,
        },
        chain: Chain::Bsc,
        token_id: BigUint::from_str("214300047253").unwrap(),
        block_number: 28056726,
        block_timestamp: 1683627432,
        contract_type: ContractType::Erc721,
        token_uri: None,
        token_domain: None,
        collection_name: None,
        image_url: None,
        image_domain: Some("public.nftstatic.com".to_string()),
        token_name: None,
        status: TransferStatus::Receive,
        possible_phishing: false,
        fee_details: None,
        confirmations: 0,
    };

    let transfer3 = NftTransferHistory {
        common: NftTransferCommon {
            block_hash: Some("0x326db41c5a4fd5f033676d95c590ced18936ef2ef6079e873b23af087fd966c6".to_string()),
            transaction_hash: "0x981bad702cc6e088f0e9b5e7287ff7a3487b8d269103cee3b9e5803141f63f91".to_string(),
            transaction_index: Some(83),
            log_index: 201,
            value: Default::default(),
            transaction_type: Some("Single".to_string()),
            token_address: Address::from_str("0xfd913a305d70a60aac4faac70c739563738e1f81").unwrap(),
            from_address: Address::from_str("0x6fad0ec6bb76914b2a2a800686acc22970645820").unwrap(),
            to_address: Address::from_str("0xf622a6c52c94b500542e2ae6bcad24c53bc5b6a2").unwrap(),
            amount: BigDecimal::from_str("1").unwrap(),
            verified: Some(1),
            operator: None,
            possible_spam: false,
        },
        chain: Chain::Bsc,
        token_id: BigUint::from_str("214300044414").unwrap(),
        block_number: 28056721,
        block_timestamp: 1683627417,
        contract_type: ContractType::Erc721,
        token_uri: None,
        token_domain: None,
        collection_name: Some("Binance NFT Mystery Box-Back to Blockchain Future".to_string()),
        image_url: Some("https://public.nftstatic.com/static/nft/res/4df0a5da04174e1e9be04b22a805f605.png".to_string()),
        image_domain: Some("tikimetadata.s3.amazonaws.com".to_string()),
        token_name: Some("Nebula Nodes".to_string()),
        status: TransferStatus::Receive,
        possible_phishing: false,
        fee_details: None,
        confirmations: 0,
    };
    vec![transfer, transfer1, transfer2, transfer3]
}

pub(crate) async fn get_nft_ctx(_chain: &Chain) -> Arc<NftCtx> {
    #[cfg(not(target_arch = "wasm32"))]
    let ctx = mm_ctx_with_custom_async_db().await;
    #[cfg(target_arch = "wasm32")]
    let ctx = mm_ctx_with_custom_db();
    NftCtx::from_ctx(&ctx).unwrap()
}
