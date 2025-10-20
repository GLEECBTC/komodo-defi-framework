use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHasher};
use common::drop_mutability;
use derive_more::Display;
use hmac::{Hmac, Mac};
use mm2_err_handle::mm_error::MmResult;
use mm2_err_handle::prelude::*;
use sha2::Sha512;
use std::convert::{TryFrom, TryInto};

const ARGON2_ALGORITHM: &str = "argon2id";
const ARGON2ID_VERSION: u32 = 19;
const ARGON2ID_M_COST: u32 = 65536;
const ARGON2ID_T_COST: u32 = 2;
const ARGON2ID_P_COST: u32 = 1;
const ARGON2ID_OUTPUT_LEN: usize = 32;

#[allow(dead_code)]
type HmacSha512 = Hmac<Sha512>;

#[derive(Debug, Display, PartialEq)]
pub enum KeyDerivationError {
    #[display(fmt = "Error hashing password: {_0}")]
    PasswordHashingFailed(String),
    #[display(fmt = "Error initializing HMAC")]
    HmacInitialization,
    #[display(fmt = "Invalid key length")]
    InvalidKeyLength,
    #[display(fmt = "Not supported: {_0}")]
    NotSupported(String),
}

impl From<argon2::password_hash::Error> for KeyDerivationError {
    fn from(e: argon2::password_hash::Error) -> Self {
        KeyDerivationError::PasswordHashingFailed(e.to_string())
    }
}

/// Parameters for the Argon2 key derivation function.
///
/// This struct defines the configuration parameters used by Argon2, one of the
/// most secure and widely used key derivation functions, especially for
/// password hashing.
#[derive(Serialize, Deserialize, Debug)]
pub struct Argon2Params {
    /// The specific variant of the Argon2 algorithm used (e.g., Argon2id).
    algorithm: String,

    /// The version of the Argon2 algorithm (e.g., 0x13 19 in u32) for the latest version).
    version: u32,

    /// The memory cost parameter defining the memory usage of the algorithm.
    /// Expressed in kibibytes (KiB).
    m_cost: u32,

    /// The time cost parameter defining the execution time and number of
    /// iterations of the algorithm.
    t_cost: u32,

    /// The parallelism cost parameter defining the number of parallel threads.
    p_cost: u32,

    /// The size of the output hash in bytes.
    output_len: usize,
}

impl Default for Argon2Params {
    fn default() -> Self {
        Argon2Params {
            algorithm: ARGON2_ALGORITHM.to_string(),
            version: ARGON2ID_VERSION,
            m_cost: ARGON2ID_M_COST,
            t_cost: ARGON2ID_T_COST,
            p_cost: ARGON2ID_P_COST,
            output_len: ARGON2ID_OUTPUT_LEN,
        }
    }
}

/// Enum representing different key derivation details.
///
/// This enum allows for flexible specification of various key derivation
/// algorithms and their parameters, making it easier to extend and support
/// multiple algorithms in the future.
#[derive(Serialize, Deserialize, Debug)]
pub enum KeyDerivationDetails {
    /// Argon2 algorithm for key derivation.
    Argon2 {
        /// The parameters for the Argon2 key derivation function.
        params: Argon2Params,
        /// The salt used in the key derivation process for the AES key.
        /// Stored as a Base64-encoded string.
        salt_aes: String,
        /// The salt used in the key derivation process for the HMAC key.
        /// This is applicable if HMAC is used for ensuring data integrity and authenticity.
        /// Stored as a Base64-encoded string.
        salt_hmac: String,
    },
    /// Algorithm for deriving a hierarchy of symmetric keys from a master secret according to [SLIP-0021](https://github.com/satoshilabs/slips/blob/master/slip-0021.md).
    SLIP0021 {
        encryption_path: String,
        authentication_path: String,
    },
    // Placeholder for future algorithms.
}

fn build_argon2_instance(params: &Argon2Params) -> Result<Argon2<'_>, KeyDerivationError> {
    let argon2_params = argon2::Params::new(params.m_cost, params.t_cost, params.p_cost, Some(params.output_len))
        .map_err(|e| KeyDerivationError::PasswordHashingFailed(format!("Invalid Argon2 parameters: {e}")))?;

    let algorithm = argon2::Algorithm::new(&params.algorithm)
        .map_err(|e| KeyDerivationError::PasswordHashingFailed(format!("Unknown Argon2 algorithm: {e}")))?;

    let version = argon2::Version::try_from(params.version)
        .map_err(|e| KeyDerivationError::PasswordHashingFailed(format!("Unknown Argon2 version: {e}")))?;

    Ok(Argon2::new(algorithm, version, argon2_params))
}

/// Derives AES and HMAC keys from a given password and salts for mnemonic encryption/decryption.
///
/// # Returns
/// A tuple containing the AES key and HMAC key as byte arrays, or a `MnemonicError` in case of failure.
pub(crate) fn derive_keys_for_mnemonic(
    password: &str,
    key_details: &KeyDerivationDetails,
) -> MmResult<([u8; 32], [u8; 32]), KeyDerivationError> {
    match key_details {
        KeyDerivationDetails::Argon2 {
            params,
            salt_aes,
            salt_hmac,
        } => {
            let argon2 = build_argon2_instance(params)?;

            let salt_aes = SaltString::from_b64(salt_aes)
                .map_err(|e| KeyDerivationError::PasswordHashingFailed(format!("Invalid AES salt: {e}")))?;
            let salt_hmac = SaltString::from_b64(salt_hmac)
                .map_err(|e| KeyDerivationError::PasswordHashingFailed(format!("Invalid HMAC salt: {e}")))?;

            // Derive AES Key
            let aes_password_hash = argon2.hash_password(password.as_bytes(), &salt_aes)?;
            let key_aes_output = aes_password_hash.serialize().hash().ok_or_else(|| {
                KeyDerivationError::PasswordHashingFailed("Error finding AES key hashing output".to_string())
            })?;
            let key_aes = key_aes_output
                .as_bytes()
                .try_into()
                .map_err(|_| KeyDerivationError::PasswordHashingFailed("Invalid AES key length".to_string()))?;

            // Derive HMAC Key
            let hmac_password_hash = argon2.hash_password(password.as_bytes(), &salt_hmac)?;
            let key_hmac_output = hmac_password_hash.serialize().hash().ok_or_else(|| {
                KeyDerivationError::PasswordHashingFailed("Error finding HMAC key hashing output".to_string())
            })?;
            let key_hmac = key_hmac_output
                .as_bytes()
                .try_into()
                .map_err(|_| KeyDerivationError::PasswordHashingFailed("Invalid HMAC key length".to_string()))?;

            Ok((key_aes, key_hmac))
        },
        KeyDerivationDetails::SLIP0021 { .. } => MmError::err(KeyDerivationError::NotSupported(
            "SLIP-0021 key derivation is not supported for mnemonic keys".to_string(),
        )),
    }
}

/// Splits a path into its components and derives a key for each component.
fn derive_key_from_path(master_node: &[u8], path: &str) -> MmResult<[u8; 32], KeyDerivationError> {
    if master_node.len() < 64 {
        return MmError::err(KeyDerivationError::InvalidKeyLength);
    }

    let mut current_key_material = master_node.to_vec();

    for segment in path.split('/').filter(|s| !s.is_empty()) {
        let mut mac = HmacSha512::new_from_slice(&current_key_material[..32])
            .map_err(|_| KeyDerivationError::HmacInitialization)?;
        mac.update(b"\x00");
        mac.update(segment.as_bytes());
        drop_mutability!(mac);

        let hmac_result = mac.finalize().into_bytes();
        current_key_material = hmac_result.to_vec();
    }
    drop_mutability!(current_key_material);

    current_key_material[32..64]
        .try_into()
        .map_to_mm(|_| KeyDerivationError::InvalidKeyLength)
}

/// Derives encryption and authentication keys from the master private key using [SLIP-0021](https://github.com/satoshilabs/slips/blob/master/slip-0021.md).
///
/// # Returns
/// A tuple containing the encryption and authentication keys as byte arrays, or a [`KeyDerivationError`] in case of failure.
#[allow(dead_code)]
pub(crate) fn derive_encryption_authentication_keys(
    master_secret: &[u8; 64],
    encryption_path: &str,
    authentication_path: &str,
) -> MmResult<([u8; 32], [u8; 32]), KeyDerivationError> {
    const MASTER_NODE_HMAC_KEY: &[u8] = b"Symmetric key seed";

    // Generate the master node `m` according to SLIP-0021.
    let mut mac =
        HmacSha512::new_from_slice(MASTER_NODE_HMAC_KEY).map_to_mm(|_| KeyDerivationError::HmacInitialization)?;
    mac.update(master_secret);
    drop_mutability!(mac);
    let master_key_material = mac.finalize().into_bytes();

    // Derive encryption key
    let encryption_key = derive_key_from_path(&master_key_material, encryption_path)?;

    // Derive authentication key
    let authentication_key = derive_key_from_path(&master_key_material, authentication_path)?;

    Ok((encryption_key, authentication_key))
}

#[cfg(any(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use crate::slip21::{AUTHENTICATION_PATH, ENCRYPTION_PATH};
    use common::cross_test;

    common::cfg_wasm32! {
        use wasm_bindgen_test::*;
        wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
    }

    // https://github.com/satoshilabs/slips/blob/master/slip-0021.md#example
    cross_test!(test_slip_0021_key_derivation, {
        let master_secret = hex::decode("c76c4ac4f4e4a00d6b274d5c39c700bb4a7ddc04fbc6f78e85ca75007b5b495f74a9043eeb77bdd53aa6fc3a0e31462270316fa04b8c19114c8798706cd02ac8").unwrap();

        let expected_encryption_key =
            hex::decode("ea163130e35bbafdf5ddee97a17b39cef2be4b4f390180d65b54cf05c6a82fde").unwrap();
        let expected_authentication_key =
            hex::decode("47194e938ab24cc82bfa25f6486ed54bebe79c40ae2a5a32ea6db294d81861a6").unwrap();

        // Directly derive the encryption and authentication keys from the master secret
        let (derived_encryption_key, derived_authentication_key) = derive_encryption_authentication_keys(
            &master_secret.try_into().expect("Invalid master secret"),
            ENCRYPTION_PATH,
            AUTHENTICATION_PATH,
        )
        .expect("Key derivation failed");

        // Verify the derived keys against the expected values
        assert_eq!(
            derived_encryption_key,
            expected_encryption_key.as_slice(),
            "Derived encryption key does not match expected value"
        );
        assert_eq!(
            derived_authentication_key,
            expected_authentication_key.as_slice(),
            "Derived authentication key does not match expected value"
        );
    });
}
