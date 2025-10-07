use crate::decrypt::decrypt_data;
use crate::encrypt::encrypt_data;
use crate::key_derivation::{derive_encryption_authentication_keys, KeyDerivationDetails, KeyDerivationError};
use crate::EncryptedData;
use derive_more::Display;
use mm2_err_handle::prelude::*;

#[allow(dead_code)]
pub(crate) const ENCRYPTION_PATH: &str = "SLIP-0021/Master encryption key/";
#[allow(dead_code)]
pub(crate) const AUTHENTICATION_PATH: &str = "SLIP-0021/Authentication key/";

#[derive(Debug, Display, PartialEq)]
#[allow(dead_code)]
pub enum SLIP21Error {
    #[display(fmt = "Error deriving key: {_0}")]
    KeyDerivationError(String),
    #[display(fmt = "Error encrypting mnemonic: {_0}")]
    EncryptionFailed(String),
    #[display(fmt = "Error decrypting mnemonic: {_0}")]
    DecryptionFailed(String),
}

impl From<KeyDerivationError> for SLIP21Error {
    fn from(e: KeyDerivationError) -> Self {
        SLIP21Error::KeyDerivationError(e.to_string())
    }
}

/// Encrypts data using SLIP-0021 derived keys.
///
/// # Returns
/// `MmResult<EncryptedData, EncryptionError>` - The encrypted data along with metadata for decryption, or an error.
#[allow(dead_code)]
pub fn encrypt_with_slip21(
    data: &[u8],
    master_secret: &[u8; 64],
    derivation_path: &str,
) -> MmResult<EncryptedData, SLIP21Error> {
    let encryption_path = ENCRYPTION_PATH.to_string() + derivation_path;
    let authentication_path = AUTHENTICATION_PATH.to_string() + derivation_path;

    // Derive encryption and authentication keys using SLIP-0021
    let (key_aes, key_hmac) =
        derive_encryption_authentication_keys(master_secret, &encryption_path, &authentication_path).map_mm_err()?;

    let key_derivation_details = KeyDerivationDetails::SLIP0021 {
        encryption_path,
        authentication_path,
    };

    encrypt_data(data, key_derivation_details, &key_aes, &key_hmac)
        .mm_err(|e| SLIP21Error::EncryptionFailed(e.to_string()))
}

/// Decrypts data encrypted with SLIP-0021 derived keys.
///
/// # Returns
/// `MmResult<Vec<u8>, DecryptionError>` - The decrypted data, or an error.
#[allow(dead_code)]
pub fn decrypt_with_slip21(encrypted_data: &EncryptedData, master_secret: &[u8; 64]) -> MmResult<Vec<u8>, SLIP21Error> {
    let (encryption_path, authentication_path) = match &encrypted_data.key_derivation_details {
        KeyDerivationDetails::SLIP0021 {
            encryption_path,
            authentication_path,
        } => (encryption_path, authentication_path),
        _ => {
            return MmError::err(SLIP21Error::KeyDerivationError(
                "Key derivation details should be SLIP0021!".to_string(),
            ))
        },
    };

    // Derive encryption and authentication keys using SLIP-0021
    let (key_aes, key_hmac) =
        derive_encryption_authentication_keys(master_secret, encryption_path, authentication_path).map_mm_err()?;

    decrypt_data(encrypted_data, &key_aes, &key_hmac).mm_err(|e| SLIP21Error::DecryptionFailed(e.to_string()))
}

#[cfg(any(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use common::cross_test;
    use std::convert::TryInto;

    common::cfg_wasm32! {
        use wasm_bindgen_test::*;
        wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
    }

    cross_test!(test_encrypt_decrypt_with_slip21, {
        let data = b"Example data to encrypt and decrypt using SLIP-0021";
        let master_secret = hex::decode("c76c4ac4f4e4a00d6b274d5c39c700bb4a7ddc04fbc6f78e85ca75007b5b495f74a9043eeb77bdd53aa6fc3a0e31462270316fa04b8c19114c8798706cd02ac8").unwrap().try_into().unwrap();
        let derivation_path = "test/path";

        // Encrypt the data
        let encrypted_data_result = encrypt_with_slip21(data, &master_secret, derivation_path);
        assert!(encrypted_data_result.is_ok());
        let encrypted_data = encrypted_data_result.unwrap();

        // Decrypt the data
        let decrypted_data_result = decrypt_with_slip21(&encrypted_data, &master_secret);
        assert!(decrypted_data_result.is_ok());
        let decrypted_data = decrypted_data_result.unwrap();

        // Verify if decrypted data matches the original data
        assert_eq!(data.to_vec(), decrypted_data);
    });
}
