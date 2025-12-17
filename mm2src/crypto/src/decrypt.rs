use crate::EncryptedData;
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use aes::Aes256;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use derive_more::Display;
use hmac::{Hmac, Mac};
use mm2_err_handle::prelude::*;
use sha2::Sha256;

type Aes256CbcDec = cbc::Decryptor<Aes256>;

#[derive(Debug, Display, PartialEq)]
pub enum DecryptionError {
    #[display(fmt = "AES cipher error: {_0}")]
    AESCipherError(String),
    #[display(fmt = "Error decoding string: {_0}")]
    DecodeError(String),
    #[display(fmt = "HMAC error: {_0}")]
    HMACError(String),
    Internal(String),
}

impl From<base64::DecodeError> for DecryptionError {
    fn from(e: base64::DecodeError) -> Self {
        DecryptionError::DecodeError(e.to_string())
    }
}

/// Decrypts the provided encrypted data using AES-256-CBC decryption and HMAC for integrity check.
///
/// This function performs several operations:
/// - It decodes the Base64-encoded values of the IV, ciphertext, and HMAC tag from the `EncryptedData`.
/// - It verifies the HMAC tag before decrypting to ensure the integrity of the data.
/// - It creates an AES-256-CBC cipher instance and decrypts the ciphertext with the provided key and the decoded IV.
///
/// # Returns
/// `MmResult<Vec<u8>, DecryptionError>` - The result is either a byte vector containing the decrypted data,
/// or a [`DecryptionError`] in case of failure.
///
/// # Errors
/// This function can return various errors related to Base64 decoding, HMAC verification, and AES decryption.
pub fn decrypt_data(
    encrypted_data: &EncryptedData,
    key_aes: &[u8; 32],
    key_hmac: &[u8; 32],
) -> MmResult<Vec<u8>, DecryptionError> {
    // Decode the Base64-encoded values
    let iv = STANDARD.decode(&encrypted_data.iv)?;
    let mut ciphertext = STANDARD.decode(&encrypted_data.ciphertext)?;
    let tag = STANDARD.decode(&encrypted_data.tag)?;

    // Verify HMAC tag before decrypting
    let mut mac = Hmac::<Sha256>::new_from_slice(key_hmac).map_to_mm(|e| DecryptionError::Internal(e.to_string()))?;
    mac.update(&ciphertext);
    mac.update(&iv);
    mac.verify_slice(&tag)
        .map_to_mm(|e| DecryptionError::HMACError(e.to_string()))?;

    // Decrypt the ciphertext and return the result
    Aes256CbcDec::new(key_aes.into(), iv.as_slice().into())
        .decrypt_padded_mut::<Pkcs7>(&mut ciphertext)
        .map_to_mm(|e| DecryptionError::AESCipherError(e.to_string()))
        .map(|plaintext| plaintext.to_vec())
}
