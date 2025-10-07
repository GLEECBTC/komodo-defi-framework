use crate::decrypt::decrypt_data;
use crate::encrypt::encrypt_data;
use crate::key_derivation::{derive_keys_for_mnemonic, Argon2Params, KeyDerivationDetails, KeyDerivationError};
use crate::EncryptedData;
use argon2::password_hash::SaltString;
use bip39::{Language, Mnemonic};
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;

const DEFAULT_WORD_COUNT: u64 = 12;

#[derive(Debug, Display, PartialEq)]
pub enum MnemonicError {
    #[display(fmt = "BIP39 mnemonic error: {_0}")]
    BIP39Error(String),
    #[display(fmt = "Error deriving key: {_0}")]
    KeyDerivationError(String),
    #[display(fmt = "Error decoding string: {_0}")]
    DecodeError(String),
    #[display(fmt = "Error encrypting mnemonic: {_0}")]
    EncryptionError(String),
    #[display(fmt = "Error decrypting mnemonic: {_0}")]
    DecryptionError(String),
    Internal(String),
}

impl From<bip39::Error> for MnemonicError {
    fn from(e: bip39::Error) -> Self {
        MnemonicError::BIP39Error(e.to_string())
    }
}

impl From<KeyDerivationError> for MnemonicError {
    fn from(e: KeyDerivationError) -> Self {
        MnemonicError::KeyDerivationError(e.to_string())
    }
}

/// Generates a new mnemonic passphrase.
///
/// This function creates a new mnemonic passphrase using a specified word count and randomness source.
/// The generated mnemonic is intended for use as a wallet mnemonic.
///
/// # Returns
/// `MmInitResult<String>` - The generated mnemonic passphrase or an error if generation fails.
///
/// # Errors
/// Returns `MmInitError::Internal` if mnemonic generation fails.
pub fn generate_mnemonic(ctx: &MmArc) -> MmResult<Mnemonic, MnemonicError> {
    let mut rng = bip39::rand_core::OsRng;
    let word_count = ctx.conf["word_count"].as_u64().unwrap_or(DEFAULT_WORD_COUNT) as usize;
    let mnemonic = Mnemonic::generate_in_with(&mut rng, Language::English, word_count)?;
    Ok(mnemonic)
}

/// Encrypts a mnemonic phrase using a specified password.
///
/// This function performs several operations:
/// - It generates salts for AES and HMAC key derivation.
/// - It derives the keys using the Argon2 algorithm.
/// - It encrypts the mnemonic using AES-256-CBC.
/// - It creates an HMAC tag for verifying the integrity and authenticity of the encrypted data.
///
/// # Returns
/// `MmResult<EncryptedData, MnemonicError>` - The result is either an `EncryptedData`
/// struct containing all the necessary components for decryption, or a `MnemonicError` in case of failure.
///
/// # Errors
/// This function can return various errors related to key derivation, encryption, and data encoding.
pub fn encrypt_mnemonic(mnemonic: &str, password: &str) -> MmResult<EncryptedData, MnemonicError> {
    use argon2::password_hash::rand_core::OsRng;

    // Generate salt for AES key
    let salt_aes = SaltString::generate(&mut OsRng);

    // Generate salt for HMAC key
    let salt_hmac = SaltString::generate(&mut OsRng);

    let key_derivation_details = KeyDerivationDetails::Argon2 {
        params: Argon2Params::default(),
        salt_aes: salt_aes.as_str().to_string(),
        salt_hmac: salt_hmac.as_str().to_string(),
    };

    // Derive AES and HMAC keys
    let (key_aes, key_hmac) = derive_keys_for_mnemonic(password, &key_derivation_details).map_mm_err()?;

    encrypt_data(mnemonic.as_bytes(), key_derivation_details, &key_aes, &key_hmac)
        .mm_err(|e| MnemonicError::EncryptionError(e.to_string()))
}

/// Decrypts an encrypted mnemonic phrase using a specified password.
///
/// This function performs the reverse operations of `encrypt_mnemonic`. It:
/// - Decodes and re-creates the necessary salts, IV, and ciphertext from the `EncryptedData`.
/// - Derives the AES and HMAC keys using the Argon2 algorithm.
/// - Verifies the integrity and authenticity of the data using the HMAC tag.
/// - Decrypts the mnemonic using AES-256-CBC.
///
/// # Returns
/// `MmResult<Mnemonic, MnemonicError>` - The result is either a `Mnemonic` instance if decryption is successful,
/// or a `MnemonicError` in case of failure.
///
/// # Errors
/// This function can return various errors related to decoding, key derivation, encryption, and HMAC verification.
pub fn decrypt_mnemonic(encrypted_data: &EncryptedData, password: &str) -> MmResult<String, MnemonicError> {
    // Re-create the keys from the password and salts
    let (key_aes, key_hmac) =
        derive_keys_for_mnemonic(password, &encrypted_data.key_derivation_details).map_mm_err()?;

    // Decrypt the ciphertext
    let decrypted_data =
        decrypt_data(encrypted_data, &key_aes, &key_hmac).mm_err(|e| MnemonicError::DecryptionError(e.to_string()))?;

    // Convert decrypted data back to a string
    let mnemonic_str = String::from_utf8(decrypted_data)
        .map_to_mm(|e| MnemonicError::DecodeError(e.to_string()))
        .map_mm_err()?;
    Ok(mnemonic_str)
}

#[cfg(any(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use common::cross_test;

    common::cfg_wasm32! {
        use wasm_bindgen_test::*;
        wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
    }

    cross_test!(test_encrypt_decrypt_mnemonic, {
        let mnemonic = "tank abandon bind salon remove wisdom net size aspect direct source fossil";
        let password = "password";

        // Encrypt the mnemonic
        let encrypted_data = encrypt_mnemonic(mnemonic, password);
        assert!(encrypted_data.is_ok());
        let encrypted_data = encrypted_data.unwrap();

        // Decrypt the mnemonic
        let decrypted_mnemonic = decrypt_mnemonic(&encrypted_data, password);
        assert!(decrypted_mnemonic.is_ok());
        let decrypted_mnemonic = decrypted_mnemonic.unwrap();

        // Verify if decrypted mnemonic matches the original
        assert_eq!(decrypted_mnemonic, mnemonic);
    });

    cross_test!(test_encrypt_decrypt_non_bip39_mnemonic, {
        let mnemonic = "Helloworld";
        let password = "Helloworld";

        // Encrypt the mnemonic
        let encrypted_data = encrypt_mnemonic(mnemonic, password);
        assert!(encrypted_data.is_ok());
        let encrypted_data = encrypted_data.unwrap();

        // Decrypt the mnemonic
        let decrypted_mnemonic = decrypt_mnemonic(&encrypted_data, password);
        assert!(decrypted_mnemonic.is_ok());
        let decrypted_mnemonic = decrypted_mnemonic.unwrap();

        // Verify if decrypted mnemonic matches the original
        assert_eq!(decrypted_mnemonic, mnemonic);
    });

    cross_test!(test_mnemonic_with_last_byte_zero, {
        let mnemonic = "tank abandon bind salon remove wisdom net size aspect direct source fossil\0".to_string();
        let password = "password";

        // Encrypt the mnemonic
        let encrypted_data = encrypt_mnemonic(&mnemonic, password);
        assert!(encrypted_data.is_ok());
        let encrypted_data = encrypted_data.unwrap();

        // Decrypt the mnemonic
        let decrypted_mnemonic_str = decrypt_mnemonic(&encrypted_data, password);
        assert!(decrypted_mnemonic_str.is_ok());
        let decrypted_mnemonic = Mnemonic::parse_normalized(&decrypted_mnemonic_str.unwrap());
        assert!(decrypted_mnemonic.is_err());

        // Verify that the error is due to parsing and not padding
        assert!(decrypted_mnemonic
            .unwrap_err()
            .to_string()
            .contains("mnemonic contains an unknown word (word 11)"));
    });
}
