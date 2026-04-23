use super::config::ResolvedTronGaslessProvider;
use super::error::TronGasfreeError;
use crate::eth::tron::TronAddress;
use common::u256_to_hex;
use ethereum_types::{H256, U256};
use mm2_err_handle::prelude::*;
use mm2_eth::eip712::{CustomTypes, Eip712, ObjectType, PropertyType, EIP712_DOMAIN};
use mm2_eth::eip712_encode::hash_typed_data;
use serde::Serialize;
use std::collections::HashMap;

pub const GASFREE_DOMAIN_NAME: &str = "GasFreeController";
pub const GASFREE_DOMAIN_VERSION: &str = "V1.0.0";
pub const PERMIT_TRANSFER_PRIMARY_TYPE: &str = "PermitTransfer";
pub const GASFREE_PERMIT_VERSION: u64 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermitTransferData {
    pub token: TronAddress,
    pub user: TronAddress,
    pub receiver: TronAddress,
    pub value: U256,
    pub max_fee: U256,
    pub deadline: U256,
    pub nonce: U256,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GasfreeDomain {
    pub name: String,
    pub version: String,
    pub chain_id: String,
    pub verifying_contract: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermitTransferMessage {
    pub token: String,
    pub service_provider: String,
    pub user: String,
    pub receiver: String,
    pub value: String,
    pub max_fee: String,
    pub deadline: String,
    pub version: String,
    pub nonce: String,
}

pub fn build_permit_transfer_typed_data(
    provider: &ResolvedTronGaslessProvider,
    transfer: &PermitTransferData,
) -> MmResult<Eip712<GasfreeDomain, PermitTransferMessage>, TronGasfreeError> {
    let domain = GasfreeDomain {
        name: GASFREE_DOMAIN_NAME.to_string(),
        version: GASFREE_DOMAIN_VERSION.to_string(),
        chain_id: u256_to_hex(U256::from(provider.network().chain_id())),
        verifying_contract: format!("{:#x}", provider.verifying_contract().to_evm_address()),
    };

    let message = PermitTransferMessage {
        token: format!("{:#x}", transfer.token.to_evm_address()),
        service_provider: format!("{:#x}", provider.service_provider().to_evm_address()),
        user: format!("{:#x}", transfer.user.to_evm_address()),
        receiver: format!("{:#x}", transfer.receiver.to_evm_address()),
        value: u256_to_hex(transfer.value),
        max_fee: u256_to_hex(transfer.max_fee),
        deadline: u256_to_hex(transfer.deadline),
        version: u256_to_hex(U256::from(GASFREE_PERMIT_VERSION)),
        nonce: u256_to_hex(transfer.nonce),
    };

    Ok(Eip712 {
        types: permit_transfer_types(),
        domain,
        primary_type: PERMIT_TRANSFER_PRIMARY_TYPE.to_string(),
        message,
    })
}

pub fn hash_permit_transfer_typed_data(
    provider: &ResolvedTronGaslessProvider,
    transfer: &PermitTransferData,
) -> MmResult<H256, TronGasfreeError> {
    let typed_data = build_permit_transfer_typed_data(provider, transfer)?;
    hash_typed_data(typed_data).map_to_mm(|e: web3::Error| TronGasfreeError::Internal(e.to_string()))
}

fn permit_transfer_types() -> CustomTypes {
    let mut domain = ObjectType::domain();
    domain
        .property("name", PropertyType::String)
        .property("version", PropertyType::String)
        .property("chainId", PropertyType::Uint256)
        .property("verifyingContract", PropertyType::Address);

    let mut permit_transfer = ObjectType::new(PERMIT_TRANSFER_PRIMARY_TYPE);
    permit_transfer
        .property("token", PropertyType::Address)
        .property("serviceProvider", PropertyType::Address)
        .property("user", PropertyType::Address)
        .property("receiver", PropertyType::Address)
        .property("value", PropertyType::Uint256)
        .property("maxFee", PropertyType::Uint256)
        .property("deadline", PropertyType::Uint256)
        .property("version", PropertyType::Uint256)
        .property("nonce", PropertyType::Uint256);

    let mut types = HashMap::new();
    types.insert(EIP712_DOMAIN.to_string(), domain.properties);
    types.insert(PERMIT_TRANSFER_PRIMARY_TYPE.to_string(), permit_transfer.properties);
    types
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::tron::gasfree::test_helpers::test_provider;
    use crate::eth::tron::Network;
    use bitcrypto::keccak256;
    use ethabi::Token;
    use ethereum_types::H160;
    use std::str::FromStr;

    fn transfer(
        token: &str,
        user: &str,
        receiver: &str,
        value: u64,
        max_fee: u64,
        deadline: u64,
        nonce: u64,
    ) -> PermitTransferData {
        PermitTransferData {
            token: TronAddress::from_str(token).unwrap(),
            user: TronAddress::from_str(user).unwrap(),
            receiver: TronAddress::from_str(receiver).unwrap(),
            value: U256::from(value),
            max_fee: U256::from(max_fee),
            deadline: U256::from(deadline),
            nonce: U256::from(nonce),
        }
    }

    fn typed_data_hash_hex(provider: &ResolvedTronGaslessProvider, transfer: &PermitTransferData) -> String {
        format!("0x{:02x}", hash_permit_transfer_typed_data(provider, transfer).unwrap())
    }

    fn domain_separator_hex(provider: &ResolvedTronGaslessProvider) -> String {
        let typed_data = build_permit_transfer_typed_data(
            provider,
            &transfer(
                "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
                "TMVQGm1qAQYVdetCeGRRkTWYYrLXuHK2HC",
                "TJM1BE5wq1VdHh3gwjUeyaVkvZp9DVYCfC",
                1,
                1,
                1,
                0,
            ),
        )
        .unwrap();
        let type_hash =
            keccak256(b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
        let name_hash = keccak256(typed_data.domain.name.as_bytes());
        let version_hash = keccak256(typed_data.domain.version.as_bytes());
        let chain_id = U256::from_str(typed_data.domain.chain_id.trim_start_matches("0x")).unwrap();
        let verifying_contract =
            H160::from_slice(&hex::decode(typed_data.domain.verifying_contract.trim_start_matches("0x")).unwrap());
        let encoded = ethabi::encode(&[
            Token::FixedBytes(type_hash.to_vec()),
            Token::FixedBytes(name_hash.to_vec()),
            Token::FixedBytes(version_hash.to_vec()),
            Token::Uint(chain_id),
            Token::Address(verifying_contract),
        ]);
        format!("0x{}", hex::encode(keccak256(&encoded)))
    }

    #[test]
    fn official_domain_separator_vectors_match() {
        assert_eq!(
            domain_separator_hex(&test_provider(Network::Nile, "TDbJyQ6g1Lx9BAfEEeN5S5TMjjDRAVFCaA")),
            "0x31a0a46f427dd040c91835228e4555951bde0a894cae6239869bb680ebc6ebea"
        );
        assert_eq!(
            domain_separator_hex(&test_provider(Network::Mainnet, "TLntW9Z59LYY5KEi9cmwk3PKjQga828ird")),
            "0x82f2b33881ada15cfdfa98b393db0e6f80fc9a27a4883ad62943ec5da825c9e8"
        );
    }

    #[test]
    fn official_permit_transfer_hash_vectors_match() {
        let nile = test_provider(Network::Nile, "TDbJyQ6g1Lx9BAfEEeN5S5TMjjDRAVFCaA");
        let mainnet = test_provider(Network::Mainnet, "TLntW9Z59LYY5KEi9cmwk3PKjQga828ird");
        let vectors = [
            (
                &mainnet,
                transfer(
                    "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
                    "TFDP1vFeSYPT6FUznL7zUjhg5X7p2AA8vw",
                    "TSPrmJetAMo6S6RxMd4tswzeRCFVegBNig",
                    20_000_000,
                    20_000_000,
                    1_740_641_152,
                    1,
                ),
                "0x4e0e1444d20768c286b9de66064e4e7311b5160871c8c0292ffeac9a16265622",
            ),
            (
                &nile,
                transfer(
                    "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
                    "TMVQGm1qAQYVdetCeGRRkTWYYrLXuHK2HC",
                    "TJM1BE5wq1VdHh3gwjUeyaVkvZp9DVYCfC",
                    10_000,
                    2_000,
                    1_726_207_632,
                    2,
                ),
                "0xb1226f3a0b690b04e2c39fac3b58352ed68943a12a54b58035045215aaf0b9b1",
            ),
            (
                &nile,
                transfer(
                    "TLBaRhANQoJFTqre9Nf1mjuwNWjCJeYqUL",
                    "TDvSsdrNM5eeXNL3czpa6AxLDHZA9nwe9K",
                    "TLFXfejEMgivFDR2x8qBpukMXd56spmFhz",
                    20_000,
                    2_000,
                    1_726_507_632,
                    3,
                ),
                "0x25c20423c18719438f4d40e6b8fec40ede6b73fb3fa702453ea9bd17dd154fb5",
            ),
            (
                &nile,
                transfer(
                    "TVSvjZdyDSNocHm7dP3jvCmMNsCnMTPa5W",
                    "TKTX96CBxr5kvhjsDHcqoiPWZageGxoTW3",
                    "TX7WF4tRGQehC9W88XEEKBhQRkLmAtZqKo",
                    100_000,
                    2_000,
                    1_729_507_632,
                    5,
                ),
                "0x3d103a6a3407dfe7540696131d7cafc3d41d7d8649b93a95daeee041e66238ce",
            ),
            (
                &nile,
                transfer(
                    "TWrZRHY9aKQZcyjpovdH6qeCEyYZrRQDZt",
                    "TCo75zcxTuWn5nnFqZUeK5socdVnG11f2T",
                    "TCN4biEVzzfyUgN1NM8iysp4bYx6mx2gPv",
                    100_000,
                    2_000,
                    1_729_517_632,
                    15,
                ),
                "0xa1a612e946ad2fecc8bcd2f93f987c38a06ca4807db7af30442e9308a20234ea",
            ),
            (
                &nile,
                transfer(
                    "TDnDyfMigx5nch7cCrtzGSwTXkUBnQJ9Pg",
                    "TWYSVbUy6eTu6ZrFWRUimgDy9SinkggVKL",
                    "TVkoisqxn1SbET8ztcnjqRGAY4npxqDcmv",
                    100_000,
                    2_000,
                    1_729_907_632,
                    50,
                ),
                "0xc78d11f0afc5397f9329861888a6724b66fe370f0189e67c39bfad4eeb7ec2a9",
            ),
        ];

        for (provider, transfer, expected_hash) in vectors {
            assert_eq!(typed_data_hash_hex(provider, &transfer), expected_hash);
        }
    }
}
