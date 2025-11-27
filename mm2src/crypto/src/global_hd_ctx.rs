#![allow(non_local_definitions)]

use crate::privkey::{bip39_seed_from_mnemonic, key_pair_from_secret, PrivKeyError};
use crate::{mm2_internal_der_path, Bip32Error, CryptoInitResult};
use bip32::{DerivationPath, ExtendedPrivateKey};
use common::drop_mutability;
use ed25519_dalek_bip32::ExtendedSigningKey;
use keys::{KeyPair, Secret as Secp256k1Secret};
use mm2_err_handle::prelude::*;
use std::ops::Deref;
use std::sync::Arc;
use zeroize::{Zeroize, ZeroizeOnDrop};
// Ed25519DerivationPath represents the same exact thing as bip32::DerivationPath, but is a different type
// used within the ed25519_dalek_bip32 library.
// We should consider our own wrapper around both types to avoid confusion.
use ed25519_dalek_bip32::DerivationPath as Ed25519DerivationPath;

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
    /// The master extended private key `m`, as defined within the BIP32 standard.
    bip39_secp_priv_key: ExtendedPrivateKey<secp256k1::SecretKey>,
    /// The master extended private key `m`, as defined within the SLIP-10 standard.
    ///
    /// ## Security Considerations
    /// - ed25519 key derivation via SLIP-10 does not support deriving children from a public key alone.
    /// - Any type or abstraction that acts like an `xpub` must embed private key material, which can easily
    ///   lead to misuse if consumers assume it is safe to expose or serialize.
    ed25519_master_priv_key: ExtendedSigningKey,
}

impl GlobalHDAccountCtx {
    pub fn new(mnemonic_str: &str) -> CryptoInitResult<(Mm2InternalKeyPair, GlobalHDAccountCtx)> {
        let bip39_seed = bip39_seed_from_mnemonic(mnemonic_str).map_mm_err()?;
        let bip39_secp_priv_key: ExtendedPrivateKey<secp256k1::SecretKey> = ExtendedPrivateKey::new(bip39_seed.0)
            .map_to_mm(PrivKeyError::Secp256k1MasterKey)
            .map_mm_err()?;

        let ed25519_master_priv_key = ExtendedSigningKey::from_seed(&bip39_seed.0)
            .map_to_mm(PrivKeyError::Ed25519MasterKey)
            .map_mm_err()?;

        // The derivation path for an "internal key". See CryptoCtx::mm2_internal_key_pair comment.
        let derivation_path = mm2_internal_der_path();

        let mut internal_priv_key = bip39_secp_priv_key.clone();
        for child in derivation_path {
            internal_priv_key = internal_priv_key
                .derive_child(child)
                .map_to_mm(PrivKeyError::Secp256k1InternalKey)
                .map_mm_err()?
        }

        let mm2_internal_key_pair = key_pair_from_secret(internal_priv_key.private_key().as_ref()).map_mm_err()?;

        let global_hd_ctx = GlobalHDAccountCtx {
            bip39_seed,
            bip39_secp_priv_key,
            ed25519_master_priv_key,
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

    pub fn derive_ed25519_signing_key(
        &self,
        derivation_path: &Ed25519DerivationPath,
    ) -> MmResult<ExtendedSigningKey, PrivKeyError> {
        self.ed25519_master_priv_key
            .derive(derivation_path)
            .map_to_mm(|e| PrivKeyError::Ed25519DeriveKey(e, derivation_path.clone()))
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

// https://github.com/satoshilabs/slips/blob/master/slip-0010.md#test-vector-1-for-ed25519
#[test]
fn test_slip_10_ed25519_vector_1() {
    use std::str::FromStr;

    let ed25519_master_priv_key =
        ExtendedSigningKey::from_seed(&hex::decode("000102030405060708090a0b0c0d0e0f").unwrap()).unwrap();

    // master xpriv aka "m"
    let known_chain_code = hex::decode("90046a93de5380a72b5e45010748567d5ea02bbf6522f979e05c0d8d8ca9fffb").unwrap();
    let known_priv_key = hex::decode("2b4be7f19ee27bbf30c667b642d5f4aa69fd169872f8fc3059c08ebae2eb19e7").unwrap();
    let known_pub_key = hex::decode("a4b2856bfec510abab89753fac1ac0e1112364e7d250545963f135f2a33188ed").unwrap();
    assert_eq!(known_chain_code, ed25519_master_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, ed25519_master_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        ed25519_master_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );

    let path = Ed25519DerivationPath::from_str("m/0'").unwrap();
    let child_priv_key = ed25519_master_priv_key.derive(&path).unwrap();
    let known_chain_code = hex::decode("8b59aa11380b624e81507a27fedda59fea6d0b779a778918a2fd3590e16e9c69").unwrap();
    let known_priv_key = hex::decode("68e0fe46dfb67e368c75379acec591dad19df3cde26e63b93a8e704f1dade7a3").unwrap();
    let known_pub_key = hex::decode("8c8a13df77a28f3445213a0f432fde644acaa215fc72dcdf300d5efaa85d350c").unwrap();
    assert_eq!(known_chain_code, child_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, child_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        child_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );

    let path = Ed25519DerivationPath::from_str("m/0'/1'").unwrap();
    let child_priv_key = ed25519_master_priv_key.derive(&path).unwrap();
    let known_chain_code = hex::decode("a320425f77d1b5c2505a6b1b27382b37368ee640e3557c315416801243552f14").unwrap();
    let known_priv_key = hex::decode("b1d0bad404bf35da785a64ca1ac54b2617211d2777696fbffaf208f746ae84f2").unwrap();
    let known_pub_key = hex::decode("1932a5270f335bed617d5b935c80aedb1a35bd9fc1e31acafd5372c30f5c1187").unwrap();
    assert_eq!(known_chain_code, child_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, child_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        child_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );

    let path = Ed25519DerivationPath::from_str("m/0'/1'/2'").unwrap();
    let child_priv_key = ed25519_master_priv_key.derive(&path).unwrap();
    let known_chain_code = hex::decode("2e69929e00b5ab250f49c3fb1c12f252de4fed2c1db88387094a0f8c4c9ccd6c").unwrap();
    let known_priv_key = hex::decode("92a5b23c0b8a99e37d07df3fb9966917f5d06e02ddbd909c7e184371463e9fc9").unwrap();
    let known_pub_key = hex::decode("ae98736566d30ed0e9d2f4486a64bc95740d89c7db33f52121f8ea8f76ff0fc1").unwrap();
    assert_eq!(known_chain_code, child_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, child_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        child_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );

    let path = Ed25519DerivationPath::from_str("m/0'/1'/2'/2'").unwrap();
    let child_priv_key = ed25519_master_priv_key.derive(&path).unwrap();
    let known_chain_code = hex::decode("8f6d87f93d750e0efccda017d662a1b31a266e4a6f5993b15f5c1f07f74dd5cc").unwrap();
    let known_priv_key = hex::decode("30d1dc7e5fc04c31219ab25a27ae00b50f6fd66622f6e9c913253d6511d1e662").unwrap();
    let known_pub_key = hex::decode("8abae2d66361c879b900d204ad2cc4984fa2aa344dd7ddc46007329ac76c429c").unwrap();
    assert_eq!(known_chain_code, child_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, child_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        child_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );

    let path = Ed25519DerivationPath::from_str("m/0'/1'/2'/2'/1000000000'").unwrap();
    let child_priv_key = ed25519_master_priv_key.derive(&path).unwrap();
    let known_chain_code = hex::decode("68789923a0cac2cd5a29172a475fe9e0fb14cd6adb5ad98a3fa70333e7afa230").unwrap();
    let known_priv_key = hex::decode("8f94d394a8e8fd6b1bc2f3f49f5c47e385281d5c17e65324b0f62483e37e8793").unwrap();
    let known_pub_key = hex::decode("3c24da049451555d51a7014a37337aa4e12d41e485abccfa46b47dfb2af54b7a").unwrap();
    assert_eq!(known_chain_code, child_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, child_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        child_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );
}

// https://github.com/satoshilabs/slips/blob/master/slip-0010.md#test-vector-2-for-ed25519
#[test]
fn test_slip_10_ed25519_vector_2() {
    use std::convert::TryInto;
    use std::str::FromStr;

    // Ed25519DerivationPath represents the same exact thing as bip32::DerivationPath, but is a different type
    // used within the ed25519_dalek_bip32 library.
    // We should consider our own wrapper around both types to avoid confusion.
    use ed25519_dalek_bip32::DerivationPath as Ed25519DerivationPath;

    let seed_bytes : [u8;64] = hex::decode("fffcf9f6f3f0edeae7e4e1dedbd8d5d2cfccc9c6c3c0bdbab7b4b1aeaba8a5a29f9c999693908d8a8784817e7b7875726f6c696663605d5a5754514e4b484542").unwrap().try_into().unwrap();
    let seed = Bip39Seed(seed_bytes);
    let ed25519_master_priv_key = ExtendedSigningKey::from_seed(&seed.0).unwrap();

    // master xpriv aka "m"
    let known_chain_code = hex::decode("ef70a74db9c3a5af931b5fe73ed8e1a53464133654fd55e7a66f8570b8e33c3b").unwrap();
    let known_priv_key = hex::decode("171cb88b1b3c1db25add599712e36245d75bc65a1a5c9e18d76f9f2b1eab4012").unwrap();
    let known_pub_key = hex::decode("8fe9693f8fa62a4305a140b9764c5ee01e455963744fe18204b4fb948249308a").unwrap();
    assert_eq!(known_chain_code, ed25519_master_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, ed25519_master_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        ed25519_master_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );

    let path = Ed25519DerivationPath::from_str("m/0'").unwrap();
    let child_priv_key = ed25519_master_priv_key.derive(&path).unwrap();
    let known_chain_code = hex::decode("0b78a3226f915c082bf118f83618a618ab6dec793752624cbeb622acb562862d").unwrap();
    let known_priv_key = hex::decode("1559eb2bbec5790b0c65d8693e4d0875b1747f4970ae8b650486ed7470845635").unwrap();
    let known_pub_key = hex::decode("86fab68dcb57aa196c77c5f264f215a112c22a912c10d123b0d03c3c28ef1037").unwrap();
    assert_eq!(known_chain_code, child_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, child_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        child_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );

    let path = Ed25519DerivationPath::from_str("m/0'/2147483647'").unwrap();
    let child_priv_key = ed25519_master_priv_key.derive(&path).unwrap();
    let known_chain_code = hex::decode("138f0b2551bcafeca6ff2aa88ba8ed0ed8de070841f0c4ef0165df8181eaad7f").unwrap();
    let known_priv_key = hex::decode("ea4f5bfe8694d8bb74b7b59404632fd5968b774ed545e810de9c32a4fb4192f4").unwrap();
    let known_pub_key = hex::decode("5ba3b9ac6e90e83effcd25ac4e58a1365a9e35a3d3ae5eb07b9e4d90bcf7506d").unwrap();
    assert_eq!(known_chain_code, child_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, child_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        child_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );

    let path = Ed25519DerivationPath::from_str("m/0'/2147483647'").unwrap();
    let child_priv_key = ed25519_master_priv_key.derive(&path).unwrap();
    let known_chain_code = hex::decode("138f0b2551bcafeca6ff2aa88ba8ed0ed8de070841f0c4ef0165df8181eaad7f").unwrap();
    let known_priv_key = hex::decode("ea4f5bfe8694d8bb74b7b59404632fd5968b774ed545e810de9c32a4fb4192f4").unwrap();
    let known_pub_key = hex::decode("5ba3b9ac6e90e83effcd25ac4e58a1365a9e35a3d3ae5eb07b9e4d90bcf7506d").unwrap();
    assert_eq!(known_chain_code, child_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, child_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        child_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );

    let path = Ed25519DerivationPath::from_str("m/0'/2147483647'/1'").unwrap();
    let child_priv_key = ed25519_master_priv_key.derive(&path).unwrap();
    let known_chain_code = hex::decode("73bd9fff1cfbde33a1b846c27085f711c0fe2d66fd32e139d3ebc28e5a4a6b90").unwrap();
    let known_priv_key = hex::decode("3757c7577170179c7868353ada796c839135b3d30554bbb74a4b1e4a5a58505c").unwrap();
    let known_pub_key = hex::decode("2e66aa57069c86cc18249aecf5cb5a9cebbfd6fadeab056254763874a9352b45").unwrap();
    assert_eq!(known_chain_code, child_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, child_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        child_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );

    let path = Ed25519DerivationPath::from_str("m/0'/2147483647'/1'/2147483646'").unwrap();
    let child_priv_key = ed25519_master_priv_key.derive(&path).unwrap();
    let known_chain_code = hex::decode("0902fe8a29f9140480a00ef244bd183e8a13288e4412d8389d140aac1794825a").unwrap();
    let known_priv_key = hex::decode("5837736c89570de861ebc173b1086da4f505d4adb387c6a1b1342d5e4ac9ec72").unwrap();
    let known_pub_key = hex::decode("e33c0f7d81d843c572275f287498e8d408654fdf0d1e065b84e2e6f157aab09b").unwrap();
    assert_eq!(known_chain_code, child_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, child_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        child_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );

    let path = Ed25519DerivationPath::from_str("m/0'/2147483647'/1'/2147483646'/2'").unwrap();
    let child_priv_key = ed25519_master_priv_key.derive(&path).unwrap();
    let known_chain_code = hex::decode("5d70af781f3a37b829f0d060924d5e960bdc02e85423494afc0b1a41bbe196d4").unwrap();
    let known_priv_key = hex::decode("551d333177df541ad876a60ea71f00447931c0a9da16f227c11ea080d7391b8d").unwrap();
    let known_pub_key = hex::decode("47150c75db263559a70d5778bf36abbab30fb061ad69f69ece61a72b0cfa4fc0").unwrap();
    assert_eq!(known_chain_code, child_priv_key.chain_code.to_vec());
    assert_eq!(known_priv_key, child_priv_key.signing_key.to_bytes().to_vec());
    assert_eq!(
        known_pub_key,
        child_priv_key.signing_key.verifying_key().to_bytes().to_vec()
    );
}
