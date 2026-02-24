//! TRON transaction signing utilities.
//!
//! TRON signs the SHA256 hash of `TransactionRaw` protobuf bytes and stores
//! signatures as `r(32) || s(32) || v(1)` where `v` must be 0 or 1.
//!

use super::proto::{Transaction, TransactionRaw};
use bitcrypto::sha256;
use derive_more::Display;
use ethereum_types::H256;
use ethkey::{sign, Secret};
use prost::Message;

#[derive(Debug, Display)]
pub enum TronSignError {
    #[display(fmt = "Invalid signature length: {}", _0)]
    InvalidSignatureLength(usize),
    #[display(fmt = "Invalid recovery id: {}", _0)]
    InvalidRecoveryId(u8),
    #[display(fmt = "Signing failed: {}", _0)]
    SigningFailed(String),
}

impl From<ethkey::Error> for TronSignError {
    fn from(err: ethkey::Error) -> Self {
        TronSignError::SigningFailed(err.to_string())
    }
}

/// Normalize recovery id to TRON format (`0/1`).
///
/// Some signers expose Ethereum legacy values (`27/28`), so we accept both and
/// convert to `0/1`. Any other value is rejected as invalid.
fn normalize_tron_v(v: u8) -> Result<u8, TronSignError> {
    match v {
        0 | 1 => Ok(v),
        27 | 28 => Ok(v - 27),
        invalid => Err(TronSignError::InvalidRecoveryId(invalid)),
    }
}

/// Compute TRON txid as SHA-256 over protobuf-encoded `TransactionRaw` bytes.
fn tron_tx_id_from_raw(raw: &TransactionRaw) -> H256 {
    let raw_bytes = raw.encode_to_vec();
    H256::from(sha256(&raw_bytes).take())
}

/// Sign `TransactionRaw` with secp256k1 and return `(tx_id, signed_transaction)`.
///
/// TRON stores signatures in `r(32) || s(32) || v(1)` format where `v` must be `0/1`.
/// We normalize legacy Ethereum-style `v` values (`27/28`) to `0/1` for compatibility.
pub fn sign_tron_transaction(raw: &TransactionRaw, secret: &Secret) -> Result<(H256, Transaction), TronSignError> {
    let tx_id = tron_tx_id_from_raw(raw);
    let signature = sign(secret, &tx_id)?;
    let mut signature_bytes = signature.to_vec();
    if signature_bytes.len() != 65 {
        return Err(TronSignError::InvalidSignatureLength(signature_bytes.len()));
    }

    signature_bytes[64] = normalize_tron_v(signature_bytes[64])?;

    let signed = Transaction {
        raw_data: Some(raw.clone()),
        signature: vec![signature_bytes],
    };

    Ok((tx_id, signed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::cross_test;
    use ethereum_types::H256;
    use ethkey::{public_to_address, verify_address, KeyPair, Signature};
    use prost::Message;
    use std::str::FromStr;

    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::*;

    /// Golden TRON TransferContract `raw_data_hex` fixture from TRON developer docs:
    /// https://developers.tron.network/docs/tron-protocol-transaction
    ///
    /// This same vector is also validated in `tron/proto.rs` golden tests.
    const GOLDEN_TRANSFER_RAW_DATA_HEX: &str = concat!(
        "0a020add",
        "22086c2763abadf9ed29",
        "40c8d5deea822e",
        "5a65",
        "0801",
        "1261",
        "0a2d",
        "747970652e676f6f676c65617069732e636f6d",
        "2f70726f746f636f6c2e",
        "5472616e73666572436f6e7472616374",
        "1230",
        "0a15",
        "418840e6c55b9ada326d211d818c34a994aeced808",
        "1215",
        "41d3136787e667d1e055d2cd5db4b5f6c880563049",
        "1864",
        "70ac89dbea822e",
    );

    /// Expected txid for `GOLDEN_TRANSFER_RAW_DATA_HEX`:
    /// SHA256(raw_data_bytes) = 9f62a65d0616c749643c4e2620b7877efd0f04dd5b2b4cd14004570d39858d7e
    const GOLDEN_TRANSFER_TXID_HEX: &str = "9f62a65d0616c749643c4e2620b7877efd0f04dd5b2b4cd14004570d39858d7e";

    fn decode_golden_transfer_raw() -> TransactionRaw {
        let raw_bytes = hex::decode(GOLDEN_TRANSFER_RAW_DATA_HEX).unwrap();
        TransactionRaw::decode(raw_bytes.as_slice()).unwrap()
    }

    cross_test!(
        test_sign_tron_transaction_verifies_signer_and_rejects_tampered_digest,
        {
            let raw = decode_golden_transfer_raw();
            let key_pair = KeyPair::from_secret_slice(&[1u8; 32]).expect("valid test key pair");
            let expected_address = public_to_address(key_pair.public());

            let (tx_id, signed) = sign_tron_transaction(&raw, key_pair.secret()).unwrap();
            assert_eq!(tx_id, tron_tx_id_from_raw(&raw));
            assert_eq!(tx_id, H256::from_slice(&hex::decode(GOLDEN_TRANSFER_TXID_HEX).unwrap()));

            assert_eq!(signed.raw_data, Some(raw.clone()));
            assert_eq!(signed.signature.len(), 1);
            assert_eq!(signed.signature[0].len(), 65);
            assert!(signed.signature[0][64] <= 1);

            let signature = Signature::from_str(&hex::encode(&signed.signature[0])).expect("valid signature hex");
            assert!(verify_address(&expected_address, &signature, &tx_id).expect("verification should execute"));

            let mut tampered_raw = raw.clone();
            tampered_raw.timestamp += 1;
            let tampered_tx_id = tron_tx_id_from_raw(&tampered_raw);
            assert!(
                !verify_address(&expected_address, &signature, &tampered_tx_id).expect("verification should execute")
            );
        }
    );
}
