#![allow(non_local_definitions)]

use crate::privkey::{bip39_seed_from_passphrase, key_pair_from_secret, PrivKeyError};
use crate::{mm2_internal_der_path, Bip32Error, CryptoInitError, CryptoInitResult};
use bip32::{DerivationPath, ExtendedPrivateKey};
use common::drop_mutability;
use keys::{KeyPair, Secret as Secp256k1Secret};
use mm2_err_handle::prelude::*;
use std::ops::Deref;
use std::sync::Arc;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub(super) type Mm2InternalKeyPair = KeyPair;

#[derive(Clone)]
pub struct GlobalHDAccountArc(Arc<GlobalHDAccountCtx>);

impl Deref for GlobalHDAccountArc {
    type Target = GlobalHDAccountCtx;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Bip39Seed(pub [u8; 64]);

pub struct GlobalHDAccountCtx {
    bip39_seed: Bip39Seed,
    bip39_secp_priv_key: ExtendedPrivateKey<secp256k1::SecretKey>,
}

impl GlobalHDAccountCtx {
    pub fn new(passphrase: &str) -> CryptoInitResult<(Mm2InternalKeyPair, GlobalHDAccountCtx)> {
        let bip39_seed = bip39_seed_from_passphrase(passphrase).map_mm_err()?;
        let bip39_secp_priv_key: ExtendedPrivateKey<secp256k1::SecretKey> = ExtendedPrivateKey::new(bip39_seed.0)
            .map_to_mm(|e| PrivKeyError::InvalidPrivKey(e.to_string()))
            .map_mm_err()?;

        let derivation_path = mm2_internal_der_path();

        let mut internal_priv_key = bip39_secp_priv_key.clone();
        for child in derivation_path {
            internal_priv_key = internal_priv_key
                .derive_child(child)
                .map_to_mm(|e| CryptoInitError::InvalidPassphrase(PrivKeyError::InvalidPrivKey(e.to_string())))
                .map_mm_err()?;
        }

        let mm2_internal_key_pair = key_pair_from_secret(internal_priv_key.private_key().as_ref()).map_mm_err()?;

        let global_hd_ctx = GlobalHDAccountCtx {
            bip39_seed,
            bip39_secp_priv_key,
        };
        Ok((mm2_internal_key_pair, global_hd_ctx))
    }

    #[inline]
    pub fn into_arc(self) -> GlobalHDAccountArc {
        GlobalHDAccountArc(Arc::new(self))
    }

    /// Returns the root BIP39 seed.
    pub fn root_seed(&self) -> &Bip39Seed {
        &self.bip39_seed
    }

    /// Returns the root BIP39 seed as bytes.
    pub fn root_seed_bytes(&self) -> &[u8] {
        &self.bip39_seed.0
    }

    /// Returns the root BIP39 private key.
    pub fn root_priv_key(&self) -> &ExtendedPrivateKey<secp256k1::SecretKey> {
        &self.bip39_secp_priv_key
    }

    /// Derives a `secp256k1::SecretKey` from [`HDAccountCtx::bip39_secp_priv_key`]
    /// at the given `m/purpose'/coin_type'/account_id'/chain/address_id` derivation path,
    /// where:
    /// * `m/purpose'/coin_type'` is specified by `derivation_path`.
    /// * `account_id = 0`, `chain = 0`.
    /// * `address_id = HDAccountCtx::hd_account`.
    ///
    /// Returns the `secp256k1::Private` Secret 256-bit key
    pub fn derive_secp256k1_secret(&self, derivation_path: &DerivationPath) -> MmResult<Secp256k1Secret, Bip32Error> {
        derive_secp256k1_secret(self.bip39_secp_priv_key.clone(), derivation_path)
    }
}

pub fn derive_secp256k1_secret(
    bip39_secp_priv_key: ExtendedPrivateKey<secp256k1::SecretKey>,
    derivation_path: &DerivationPath,
) -> MmResult<Secp256k1Secret, Bip32Error> {
    let mut priv_key = bip39_secp_priv_key;
    for child in derivation_path.iter() {
        priv_key = priv_key.derive_child(child)?;
    }
    drop_mutability!(priv_key);

    let secret = *priv_key.private_key().as_ref();
    Ok(Secp256k1Secret::from(secret))
}
