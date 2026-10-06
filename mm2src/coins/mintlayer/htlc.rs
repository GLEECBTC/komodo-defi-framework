use mintlayer_sdk::crypto::types::{OutputTimeLock, TxOutput};
use mintlayer_sdk::crypto::{self, Amount, Network};
use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq)]
pub enum MintlayerHtlcError {
    #[error("Mintlayer HTLC secret hash must contain exactly 20 bytes, found {0}")]
    InvalidSecretHashLength(usize),
    #[error("Mintlayer HTLC SDK operation failed: {0}")]
    Sdk(String),
}

/// Converts KDF's 20-byte DHASH160 secret hash into the hexadecimal
/// representation expected by the Mintlayer SDK.
///
/// Mintlayer consensus defines HtlcSecret::hash() as
/// RIPEMD160(SHA256(secret)), matching KDF SecretHashAlgo::DHASH160.
pub fn mintlayer_htlc_secret_hash_hex(secret_hash: &[u8]) -> Result<String, MintlayerHtlcError> {
    if secret_hash.len() != 20 {
        return Err(MintlayerHtlcError::InvalidSecretHashLength(secret_hash.len()));
    }

    Ok(hex::encode(secret_hash))
}

/// Converts KDF's absolute UNIX refund timestamp into Mintlayer's
/// absolute HTLC timelock representation.
pub fn mintlayer_htlc_until_time(time_lock: u64) -> OutputTimeLock {
    crypto::encode_lock_until_time(time_lock)
}

/// Builds a native Mintlayer coin HTLC output from KDF swap parameters.
pub fn build_mintlayer_htlc_output(
    amount_atoms: u128,
    secret_hash: &[u8],
    spend_address: &str,
    refund_address: &str,
    time_lock: u64,
    network: Network,
) -> Result<TxOutput, MintlayerHtlcError> {
    let secret_hash_hex = mintlayer_htlc_secret_hash_hex(secret_hash)?;

    crypto::encode_output_htlc(
        Amount::from_atoms(amount_atoms),
        None,
        &secret_hash_hex,
        spend_address,
        refund_address,
        mintlayer_htlc_until_time(time_lock),
        network,
    )
    .map_err(|error| MintlayerHtlcError::Sdk(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mintlayer::mintlayer_address_from_compressed_public_key;
    use crate::mintlayer::MintlayerNetwork;

    const SECRET_HASH: [u8; 20] = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x10, 0x20,
        0x30, 0x40,
    ];
    const TIME_LOCK: u64 = 1_800_000_000;

    const SPEND_PUBLIC_KEY: [u8; 33] = [
        0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b, 0x07, 0x02,
        0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17, 0x98,
    ];

    const REFUND_PUBLIC_KEY: [u8; 33] = [
        0x02, 0xc6, 0x04, 0x7f, 0x94, 0x41, 0xed, 0x7d, 0x6d, 0x30, 0x45, 0x40, 0x6e, 0x95, 0xc0, 0x7c, 0xd8, 0x5c,
        0x77, 0x8e, 0x4b, 0x8c, 0xef, 0x3c, 0xa7, 0xab, 0xac, 0x09, 0xb9, 0x5c, 0x70, 0x9e, 0xe5,
    ];

    #[test]
    fn accepts_exact_kdf_dhash160_and_rejects_other_lengths() {
        assert_eq!(
            mintlayer_htlc_secret_hash_hex(&SECRET_HASH).unwrap(),
            hex::encode(SECRET_HASH)
        );

        assert_eq!(
            mintlayer_htlc_secret_hash_hex(&[0_u8; 32]),
            Err(MintlayerHtlcError::InvalidSecretHashLength(32))
        );
    }

    #[test]
    fn builds_native_htlc_with_absolute_time_lock() {
        let spend_address =
            mintlayer_address_from_compressed_public_key(MintlayerNetwork::Mainnet, &SPEND_PUBLIC_KEY).unwrap();

        let refund_address =
            mintlayer_address_from_compressed_public_key(MintlayerNetwork::Mainnet, &REFUND_PUBLIC_KEY).unwrap();

        let output = build_mintlayer_htlc_output(
            100_000_000,
            &SECRET_HASH,
            &spend_address,
            &refund_address,
            TIME_LOCK,
            Network::Mainnet,
        )
        .unwrap();

        match output {
            TxOutput::Htlc(value, htlc) => {
                assert_eq!(
                    value,
                    mintlayer_sdk::crypto::types::OutputValue::Coin(Amount::from_atoms(100_000_000))
                );
                assert_eq!(htlc.secret_hash.as_bytes(), &SECRET_HASH);
                assert_eq!(
                    htlc.spend_key,
                    crypto::encode_destination(&spend_address, Network::Mainnet).unwrap()
                );
                assert_eq!(
                    htlc.refund_key,
                    crypto::encode_destination(&refund_address, Network::Mainnet).unwrap()
                );
                assert_eq!(htlc.refund_timelock, mintlayer_htlc_until_time(TIME_LOCK));
            },
            other => panic!("expected native Mintlayer HTLC output, got {:?}", other),
        }
    }

    #[test]
    fn spends_native_htlc_and_extracts_original_secret() {
        use mintlayer_sdk::crypto::types::{DecodeAll, Encode, HtlcSecret, PrivateKey, H256};
        use mintlayer_sdk::crypto::{
            encode_input_for_utxo, encode_outpoint_source_id, encode_signed_transaction, encode_transaction,
            encode_witness_htlc_spend, extract_htlc_secret, pubkey_to_pubkeyhash_address, public_key_from_private_key,
            SigHashType, SourceId, TxAdditionalInfo,
        };

        const FIXED_SIGNING_PRIVKEY: &str = "00b88adfb44da2c1fd5f12f7996bd147f45bd0b8917fa8842d4c901b965d5dad1f";

        const SECRET_BYTES: [u8; 32] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11,
            0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        ];

        let network = Network::Mainnet;

        let fixed_key = <PrivateKey as DecodeAll>::decode_all(&mut &hex::decode(FIXED_SIGNING_PRIVKEY).unwrap()[..])
            .expect("fixed public test private key must decode");

        let spend_address = pubkey_to_pubkeyhash_address(&public_key_from_private_key(&fixed_key), network);

        let refund_address =
            mintlayer_address_from_compressed_public_key(MintlayerNetwork::Mainnet, &REFUND_PUBLIC_KEY).unwrap();

        let secret = HtlcSecret::new(SECRET_BYTES);
        let secret_hash = secret.hash();

        let htlc_output = build_mintlayer_htlc_output(
            100_000_000,
            secret_hash.as_bytes(),
            &spend_address,
            &refund_address,
            TIME_LOCK,
            network,
        )
        .unwrap();

        let source_id = encode_outpoint_source_id(H256::from_slice(&[0_u8; 32]), SourceId::Transaction);

        let input = encode_input_for_utxo(source_id.clone(), 0);

        let transaction = encode_transaction(vec![input], vec![htlc_output.clone()], 0).unwrap();

        let input_utxos = [Some(htlc_output)];

        let witness = encode_witness_htlc_spend(
            SigHashType::all(),
            &fixed_key,
            &spend_address,
            &transaction,
            &input_utxos,
            0,
            HtlcSecret::new(SECRET_BYTES),
            &TxAdditionalInfo::new(),
            500_000,
            network,
        )
        .expect("native Mintlayer HTLC spend witness must be produced");

        let signed = encode_signed_transaction(transaction, vec![witness]).unwrap();

        let extracted = extract_htlc_secret(&signed, source_id, 0)
            .expect("secret must be extractable from native Mintlayer HTLC spend");

        assert_eq!(extracted, HtlcSecret::new(SECRET_BYTES));
        assert_eq!(
            extracted.encode(),
            SECRET_BYTES.to_vec(),
            "extracted secret must be the original 32-byte pre-image"
        );
    }

    #[test]
    fn refunds_native_htlc_without_revealing_secret() {
        use mintlayer_sdk::crypto::types::{DecodeAll, HtlcSecret, PrivateKey, H256};
        use mintlayer_sdk::crypto::{
            encode_input_for_utxo, encode_outpoint_source_id, encode_signed_transaction, encode_transaction,
            encode_witness_htlc_refund_single_sig, extract_htlc_secret, pubkey_to_pubkeyhash_address,
            public_key_from_private_key, Error, SigHashType, SourceId, TxAdditionalInfo,
        };

        const FIXED_REFUND_PRIVKEY: &str = "00b88adfb44da2c1fd5f12f7996bd147f45bd0b8917fa8842d4c901b965d5dad1f";

        const SECRET_BYTES: [u8; 32] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11,
            0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        ];

        let network = Network::Mainnet;

        let refund_key = <PrivateKey as DecodeAll>::decode_all(&mut &hex::decode(FIXED_REFUND_PRIVKEY).unwrap()[..])
            .expect("fixed public test refund key must decode");

        let refund_address = pubkey_to_pubkeyhash_address(&public_key_from_private_key(&refund_key), network);

        let spend_address =
            mintlayer_address_from_compressed_public_key(MintlayerNetwork::Mainnet, &SPEND_PUBLIC_KEY).unwrap();

        let secret = HtlcSecret::new(SECRET_BYTES);
        let secret_hash = secret.hash();

        let htlc_output = build_mintlayer_htlc_output(
            100_000_000,
            secret_hash.as_bytes(),
            &spend_address,
            &refund_address,
            TIME_LOCK,
            network,
        )
        .unwrap();

        let source_id = encode_outpoint_source_id(H256::from_slice(&[0_u8; 32]), SourceId::Transaction);

        let input = encode_input_for_utxo(source_id.clone(), 0);

        let transaction = encode_transaction(vec![input], vec![htlc_output.clone()], 0).unwrap();

        let input_utxos = [Some(htlc_output)];

        let witness = encode_witness_htlc_refund_single_sig(
            SigHashType::all(),
            &refund_key,
            &refund_address,
            &transaction,
            &input_utxos,
            0,
            &TxAdditionalInfo::new(),
            500_000,
            network,
        )
        .expect("native Mintlayer HTLC refund witness must be produced");

        let signed = encode_signed_transaction(transaction, vec![witness]).unwrap();

        let result = extract_htlc_secret(&signed, source_id, 0);

        assert!(
            matches!(result, Err(Error::UnexpectedHtlcSpendType)),
            "refund must not expose an HTLC secret, got {:?}",
            result
        );
    }
}
