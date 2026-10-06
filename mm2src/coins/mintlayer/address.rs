use crate::mintlayer::MintlayerNetwork;
use crate::ToBytes;
use bech32::{FromBase32, ToBase32, Variant};
use crypto::privkey::key_pair_from_secret;
use crypto::{Bip44Chain, DerivationPath, GlobalHDAccountArc};
use std::str::FromStr;
use thiserror::Error;

const BIP44_PURPOSE: u32 = 44;
const MINTLAYER_MAINNET_COIN_TYPE: u32 = 0x4D4C;
const MINTLAYER_TEST_COIN_TYPE: u32 = 1;

const PUBLIC_KEY_HOLDER_SECP256K1_SCHNORR_TAG: u8 = 0;
const DESTINATION_PUBLIC_KEY_HASH_TAG: u8 = 1;
const PUBLIC_KEY_HASH_SIZE: usize = 20;
const COMPRESSED_PUBLIC_KEY_SIZE: usize = 33;

#[derive(Clone, Debug, Error, PartialEq)]
pub enum MintlayerAddressError {
    #[error("Invalid Mintlayer derivation path '{path}': {error}")]
    InvalidDerivationPath { path: String, error: String },
    #[error("Failed to derive Mintlayer private key: {0}")]
    KeyDerivation(String),
    #[error("Invalid compressed secp256k1 public key length: expected {expected} bytes, found {actual}")]
    InvalidCompressedPublicKeyLength { expected: usize, actual: usize },
    #[error("Invalid compressed secp256k1 public key: {0}")]
    InvalidCompressedPublicKey(String),
    #[error("Failed to encode Mintlayer address: {0}")]
    AddressEncoding(String),
    #[error("Failed to decode Mintlayer address: {0}")]
    AddressDecoding(String),
    #[error("Unexpected Mintlayer address prefix: expected '{expected}', found '{actual}'")]
    UnexpectedHrp { expected: String, actual: String },
    #[error("Mintlayer address must use Bech32m encoding")]
    UnexpectedVariant,
    #[error("Invalid Mintlayer destination payload length: expected {expected} bytes, found {actual}")]
    InvalidDestinationPayloadLength { expected: usize, actual: usize },
    #[error("Unsupported Mintlayer destination tag: {0}")]
    UnsupportedDestinationTag(u8),
}

/// Returns the BIP44 coin type used by Mintlayer Core.
///
/// Mainnet uses the ASCII-derived value `0x4D4C` (`19788`), while all
/// non-mainnet networks use the test coin type `1`.
pub fn mintlayer_coin_type(network: MintlayerNetwork) -> u32 {
    match network {
        MintlayerNetwork::Mainnet => MINTLAYER_MAINNET_COIN_TYPE,
        MintlayerNetwork::Testnet | MintlayerNetwork::Regtest | MintlayerNetwork::Signet => MINTLAYER_TEST_COIN_TYPE,
    }
}

/// Returns the human-readable prefix used for a PublicKeyHash destination.
pub fn mintlayer_public_key_hash_hrp(network: MintlayerNetwork) -> &'static str {
    match network {
        MintlayerNetwork::Mainnet => "mtc",
        MintlayerNetwork::Testnet => "tmt",
        MintlayerNetwork::Regtest => "rmt",
        MintlayerNetwork::Signet => "smt",
    }
}

/// Builds the Mintlayer BIP44 path:
///
/// `m/44'/coin_type'/account'/chain/address_index`
pub fn mintlayer_derivation_path(
    network: MintlayerNetwork,
    account: u32,
    chain: Bip44Chain,
    address_index: u32,
) -> Result<DerivationPath, MintlayerAddressError> {
    let chain_index = match chain {
        Bip44Chain::External => 0,
        Bip44Chain::Internal => 1,
    };

    let path = format!(
        "m/{BIP44_PURPOSE}'/{}'/{}'/{chain_index}/{address_index}",
        mintlayer_coin_type(network),
        account
    );

    DerivationPath::from_str(&path).map_err(|error| MintlayerAddressError::InvalidDerivationPath {
        path,
        error: error.to_string(),
    })
}

/// Converts a compressed secp256k1 public key into a Mintlayer
/// PublicKeyHash destination address.
///
/// Mintlayer serializes the public key as:
///
/// `0x00 || compressed_public_key`
///
/// It then calculates BLAKE2b-512, takes the first 20 bytes, serializes the
/// PublicKeyHash destination as:
///
/// `0x01 || public_key_hash`
///
/// and encodes the payload using Bech32m.
pub fn mintlayer_address_from_compressed_public_key(
    network: MintlayerNetwork,
    compressed_public_key: &[u8],
) -> Result<String, MintlayerAddressError> {
    if compressed_public_key.len() != COMPRESSED_PUBLIC_KEY_SIZE {
        return Err(MintlayerAddressError::InvalidCompressedPublicKeyLength {
            expected: COMPRESSED_PUBLIC_KEY_SIZE,
            actual: compressed_public_key.len(),
        });
    }

    let public_key = secp256k1::PublicKey::from_slice(compressed_public_key)
        .map_err(|error| MintlayerAddressError::InvalidCompressedPublicKey(error.to_string()))?;

    let normalized_public_key = public_key.serialize();

    let mut encoded_public_key = Vec::with_capacity(1 + COMPRESSED_PUBLIC_KEY_SIZE);
    encoded_public_key.push(PUBLIC_KEY_HOLDER_SECP256K1_SCHNORR_TAG);
    encoded_public_key.extend_from_slice(&normalized_public_key);

    // Mintlayer's Blake2b32 implementation calculates full BLAKE2b-512 and
    // truncates the resulting digest, rather than configuring BLAKE2b with
    // a shorter output length.
    let public_key_digest = blake2b_simd::Params::new().hash_length(64).hash(&encoded_public_key);

    let mut destination_payload = Vec::with_capacity(1 + PUBLIC_KEY_HASH_SIZE);
    destination_payload.push(DESTINATION_PUBLIC_KEY_HASH_TAG);
    destination_payload.extend_from_slice(&public_key_digest.as_bytes()[..PUBLIC_KEY_HASH_SIZE]);

    bech32::encode(
        mintlayer_public_key_hash_hrp(network),
        destination_payload.to_base32(),
        Variant::Bech32m,
    )
    .map_err(|error| MintlayerAddressError::AddressEncoding(error.to_string()))
}

/// Validates a Mintlayer PublicKeyHash address for the selected network.
pub fn validate_mintlayer_address(network: MintlayerNetwork, address: &str) -> Result<(), MintlayerAddressError> {
    let (hrp, data, variant) =
        bech32::decode(address).map_err(|error| MintlayerAddressError::AddressDecoding(error.to_string()))?;

    let expected_hrp = mintlayer_public_key_hash_hrp(network);
    if !hrp.eq_ignore_ascii_case(expected_hrp) {
        return Err(MintlayerAddressError::UnexpectedHrp {
            expected: expected_hrp.to_string(),
            actual: hrp,
        });
    }

    if variant != Variant::Bech32m {
        return Err(MintlayerAddressError::UnexpectedVariant);
    }

    let payload =
        Vec::<u8>::from_base32(&data).map_err(|error| MintlayerAddressError::AddressDecoding(error.to_string()))?;
    let expected_payload_length = 1 + PUBLIC_KEY_HASH_SIZE;
    if payload.len() != expected_payload_length {
        return Err(MintlayerAddressError::InvalidDestinationPayloadLength {
            expected: expected_payload_length,
            actual: payload.len(),
        });
    }

    if payload[0] != DESTINATION_PUBLIC_KEY_HASH_TAG {
        return Err(MintlayerAddressError::UnsupportedDestinationTag(payload[0]));
    }

    Ok(())
}

/// Derives a Mintlayer key from the global KDF HD context and returns its
/// PublicKeyHash address.
pub fn derive_mintlayer_address(
    global_hd_ctx: &GlobalHDAccountArc,
    network: MintlayerNetwork,
    account: u32,
    chain: Bip44Chain,
    address_index: u32,
) -> Result<String, MintlayerAddressError> {
    let derivation_path = mintlayer_derivation_path(network, account, chain, address_index)?;

    let secret = global_hd_ctx
        .derive_secp256k1_secret(&derivation_path)
        .map_err(|error| MintlayerAddressError::KeyDerivation(error.to_string()))?;

    let key_pair = key_pair_from_secret(&secret.take())
        .map_err(|error| MintlayerAddressError::KeyDerivation(error.to_string()))?;
    let compressed_public_key = key_pair.public().to_bytes();

    mintlayer_address_from_compressed_public_key(network, &compressed_public_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFICIAL_PUBLIC_KEY: &str = "03bf6f8d52dade77f95e9c6c9488fd8492a99c09ff23095caffb2e6409d1746ade";
    const OFFICIAL_MAINNET_ADDRESS: &str = "mtc1qyumjs84s5nqgcp6nw9kwde9mn7akph6hgtulsdk";

    #[test]
    fn network_coin_types_match_mintlayer_core() {
        assert_eq!(
            mintlayer_coin_type(MintlayerNetwork::Mainnet),
            MINTLAYER_MAINNET_COIN_TYPE
        );
        assert_eq!(mintlayer_coin_type(MintlayerNetwork::Testnet), MINTLAYER_TEST_COIN_TYPE);
        assert_eq!(mintlayer_coin_type(MintlayerNetwork::Regtest), MINTLAYER_TEST_COIN_TYPE);
        assert_eq!(mintlayer_coin_type(MintlayerNetwork::Signet), MINTLAYER_TEST_COIN_TYPE);
    }

    #[test]
    fn network_hrps_match_mintlayer_core() {
        assert_eq!(mintlayer_public_key_hash_hrp(MintlayerNetwork::Mainnet), "mtc");
        assert_eq!(mintlayer_public_key_hash_hrp(MintlayerNetwork::Testnet), "tmt");
        assert_eq!(mintlayer_public_key_hash_hrp(MintlayerNetwork::Regtest), "rmt");
        assert_eq!(mintlayer_public_key_hash_hrp(MintlayerNetwork::Signet), "smt");
    }

    #[test]
    fn builds_official_mainnet_receive_path() {
        let path = mintlayer_derivation_path(MintlayerNetwork::Mainnet, 0, Bip44Chain::External, 0).unwrap();

        assert_eq!(path.to_string(), "m/44'/19788'/0'/0/0");
    }

    #[test]
    fn builds_testnet_internal_path() {
        let path = mintlayer_derivation_path(MintlayerNetwork::Testnet, 7, Bip44Chain::Internal, 12).unwrap();

        assert_eq!(path.to_string(), "m/44'/1'/7'/1/12");
    }

    #[test]
    fn official_mainnet_address_vector_matches_mintlayer_core() {
        let public_key = hex::decode(OFFICIAL_PUBLIC_KEY).unwrap();
        let address = mintlayer_address_from_compressed_public_key(MintlayerNetwork::Mainnet, &public_key).unwrap();

        assert_eq!(address, OFFICIAL_MAINNET_ADDRESS);
    }

    #[test]
    fn validates_official_mainnet_address() {
        assert!(validate_mintlayer_address(MintlayerNetwork::Mainnet, OFFICIAL_MAINNET_ADDRESS).is_ok());
    }

    #[test]
    fn rejects_address_from_another_network() {
        assert!(matches!(
            validate_mintlayer_address(MintlayerNetwork::Testnet, OFFICIAL_MAINNET_ADDRESS),
            Err(MintlayerAddressError::UnexpectedHrp { .. })
        ));
    }

    #[test]
    fn rejects_legacy_bech32_variant() {
        let (_, data, _) = bech32::decode(OFFICIAL_MAINNET_ADDRESS).unwrap();
        let address = bech32::encode(
            mintlayer_public_key_hash_hrp(MintlayerNetwork::Mainnet),
            data,
            Variant::Bech32,
        )
        .unwrap();

        assert_eq!(
            validate_mintlayer_address(MintlayerNetwork::Mainnet, &address),
            Err(MintlayerAddressError::UnexpectedVariant)
        );
    }

    #[test]
    fn rejects_unsupported_destination_tag() {
        let mut payload = vec![0_u8; 1 + PUBLIC_KEY_HASH_SIZE];
        payload[0] = 2;
        let address = bech32::encode(
            mintlayer_public_key_hash_hrp(MintlayerNetwork::Mainnet),
            payload.to_base32(),
            Variant::Bech32m,
        )
        .unwrap();

        assert_eq!(
            validate_mintlayer_address(MintlayerNetwork::Mainnet, &address),
            Err(MintlayerAddressError::UnsupportedDestinationTag(2))
        );
    }

    #[test]
    fn rejects_incorrect_destination_payload_length() {
        let payload = vec![DESTINATION_PUBLIC_KEY_HASH_TAG; PUBLIC_KEY_HASH_SIZE];
        let address = bech32::encode(
            mintlayer_public_key_hash_hrp(MintlayerNetwork::Mainnet),
            payload.to_base32(),
            Variant::Bech32m,
        )
        .unwrap();

        assert_eq!(
            validate_mintlayer_address(MintlayerNetwork::Mainnet, &address),
            Err(MintlayerAddressError::InvalidDestinationPayloadLength {
                expected: 1 + PUBLIC_KEY_HASH_SIZE,
                actual: PUBLIC_KEY_HASH_SIZE,
            })
        );
    }

    #[test]
    fn rejects_incorrect_public_key_length() {
        let error = mintlayer_address_from_compressed_public_key(MintlayerNetwork::Mainnet, &[2; 32]).unwrap_err();

        assert_eq!(
            error,
            MintlayerAddressError::InvalidCompressedPublicKeyLength {
                expected: COMPRESSED_PUBLIC_KEY_SIZE,
                actual: 32,
            }
        );
    }

    #[test]
    fn rejects_invalid_compressed_public_key() {
        let invalid_public_key = [0_u8; COMPRESSED_PUBLIC_KEY_SIZE];
        let error =
            mintlayer_address_from_compressed_public_key(MintlayerNetwork::Mainnet, &invalid_public_key).unwrap_err();

        assert!(matches!(error, MintlayerAddressError::InvalidCompressedPublicKey(_)));
    }
}
