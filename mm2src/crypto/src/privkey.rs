/******************************************************************************
 * Copyright © 2023 Pampex LTD and TillyHK LTD                                *
 *                                                                            *
 * See the CONTRIBUTOR-LICENSE-AGREEMENT, COPYING, LICENSE-COPYRIGHT-NOTICE   *
 * and DEVELOPER-CERTIFICATE-OF-ORIGIN files in the LEGAL directory in        *
 * the top-level directory of this distribution for the individual copyright  *
 * holder information and the developer policies on copyright and licensing.  *
 *                                                                            *
 * Unless otherwise agreed in a custom licensing agreement, no part of the    *
 * Komodo DeFi Framework software, including this file may be copied, modified, propagated*
 * or distributed except according to the terms contained in the              *
 * LICENSE-COPYRIGHT-NOTICE file.                                             *
 *                                                                            *
 * Removal or modification of this copyright notice is prohibited.            *
 *                                                                            *
 ******************************************************************************/
//
//  LP_utxos.c
//  marketmaker
//

use crate::global_hd_ctx::Bip39Seed;
use bitcrypto::{sha256, ChecksumType};
use derive_more::Display;
use keys::{Error as KeysError, KeyPair, Private, Secret as Secp256k1Secret};
use mm2_err_handle::prelude::*;
use rustc_hex::FromHexError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub type PrivKeyResult<T> = Result<T, MmError<PrivKeyError>>;

#[derive(Debug, Display, Serialize)]
pub enum PrivKeyError {
    #[display(fmt = "Provided WIF passphrase has invalid checksum!")]
    WifPassphraseInvalidChecksum,
    #[display(fmt = "Error parsing passphrase: {_0}")]
    ErrorParsingPassphrase(String),
    #[display(fmt = "Invalid private key: {_0}")]
    InvalidPrivKey(String),
    #[display(fmt = "We only support compressed keys at the moment")]
    ExpectedCompressedKeys,
}

impl From<FromHexError> for PrivKeyError {
    fn from(e: FromHexError) -> Self {
        PrivKeyError::ErrorParsingPassphrase(e.to_string())
    }
}

impl From<KeysError> for PrivKeyError {
    fn from(e: KeysError) -> Self {
        PrivKeyError::InvalidPrivKey(e.to_string())
    }
}

impl std::error::Error for PrivKeyError {}

fn private_from_seed(seed: &str) -> PrivKeyResult<Private> {
    match seed.parse() {
        Ok(private) => return Ok(private),
        Err(e) => {
            if let KeysError::InvalidChecksum = e {
                return MmError::err(PrivKeyError::WifPassphraseInvalidChecksum);
            }
        }, // else ignore other errors, assume the passphrase is not WIF
    }

    match seed.strip_prefix("0x") {
        Some(stripped) => {
            let hash: Secp256k1Secret = stripped.parse()?;
            Ok(Private {
                prefix: 0,
                secret: hash,
                compressed: true,
                checksum_type: ChecksumType::DSHA256,
            })
        },
        None => Ok(private_from_seed_hash(seed)),
    }
}

pub(crate) fn private_from_seed_hash(seed: &str) -> Private {
    let hash = sha256(seed.as_bytes());
    Private {
        prefix: 0,
        secret: secp_privkey_from_hash(hash),
        compressed: true,
        checksum_type: ChecksumType::DSHA256,
    }
}

/// Mutates the arbitrary hash to become a valid secp256k1 private key
pub fn secp_privkey_from_hash(mut hash: Secp256k1Secret) -> Secp256k1Secret {
    hash[0] &= 248;
    hash[31] &= 127;
    hash[31] |= 64;
    hash
}

pub fn key_pair_from_seed(seed: &str) -> PrivKeyResult<KeyPair> {
    let private = private_from_seed(seed).map_mm_err()?;
    if !private.compressed {
        return MmError::err(PrivKeyError::ExpectedCompressedKeys);
    }
    let pair = KeyPair::from_private(private)?;
    // Just a sanity check. We rely on the public key being 33 bytes (aka compressed).
    assert_eq!(pair.public().len(), 33);
    Ok(pair)
}

pub fn key_pair_from_secret(secret: &[u8; 32]) -> PrivKeyResult<KeyPair> {
    let private = Private {
        prefix: 0,
        secret: secret.into(),
        compressed: true,
        checksum_type: ChecksumType::DSHA256,
    };
    Ok(KeyPair::from_private(private)?)
}

pub fn bip39_seed_from_passphrase(passphrase: &str) -> PrivKeyResult<Bip39Seed> {
    let mnemonic = bip39::Mnemonic::parse_in_normalized(bip39::Language::English, passphrase)
        .map_to_mm(|e| PrivKeyError::ErrorParsingPassphrase(e.to_string()))?;
    let seed = mnemonic.to_seed_normalized("");
    Ok(Bip39Seed(seed))
}

#[derive(Clone, Copy, Debug)]
pub struct SerializableSecp256k1Keypair {
    inner: KeyPair,
}

impl PartialEq for SerializableSecp256k1Keypair {
    fn eq(&self, other: &Self) -> bool {
        self.inner.public() == other.inner.public()
    }
}

impl Eq for SerializableSecp256k1Keypair {}

impl SerializableSecp256k1Keypair {
    pub fn new(key: [u8; 32]) -> PrivKeyResult<Self> {
        Ok(SerializableSecp256k1Keypair {
            inner: key_pair_from_secret(&key)?,
        })
    }

    pub fn key_pair(&self) -> &KeyPair {
        &self.inner
    }

    pub fn public_slice(&self) -> &[u8] {
        self.inner.public_slice()
    }

    pub fn priv_key(&self) -> [u8; 32] {
        self.inner.private().secret.take()
    }

    pub fn random() -> Self {
        SerializableSecp256k1Keypair {
            inner: KeyPair::random_compressed(),
        }
    }

    pub fn into_inner(self) -> KeyPair {
        self.inner
    }
}

impl From<KeyPair> for SerializableSecp256k1Keypair {
    fn from(inner: KeyPair) -> Self {
        SerializableSecp256k1Keypair { inner }
    }
}

impl Serialize for SerializableSecp256k1Keypair {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.priv_key().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SerializableSecp256k1Keypair {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let priv_key = <[u8; 32]>::deserialize(deserializer)?;
        SerializableSecp256k1Keypair::new(priv_key).map_err(serde::de::Error::custom)
    }
}

#[test]
fn serializable_secp256k1_keypair_test() {
    use serde_json::{self as json};

    let key_pair = KeyPair::random_compressed();
    let serializable = SerializableSecp256k1Keypair { inner: key_pair };
    let serialized = json::to_string(&serializable).unwrap();
    println!("{serialized}");
    let deserialized = json::from_str(&serialized).unwrap();
    assert_eq!(serializable, deserialized);

    let invalid_privkey: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0xba, 0xae,
        0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x41,
    ];
    let invalid_privkey_serialized = json::to_string(&invalid_privkey).unwrap();
    let err = json::from_str::<SerializableSecp256k1Keypair>(&invalid_privkey_serialized).unwrap_err();
    println!("{err}");
}
