use crate::key_derivation::KeyDerivationDetails;
use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use aes::Aes256;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use common::drop_mutability;
use derive_more::Display;
use hmac::{Hmac, Mac};
use mm2_err_handle::prelude::*;
use sha2::Sha256;

const ENCRYPTED_DATA_VERSION: u8 = 1;

type Aes256CbcEnc = cbc::Encryptor<Aes256>;

#[derive(Debug, Display, PartialEq)]
pub enum EncryptionError {
    #[display(fmt = "Error generating random bytes: {_0}")]
    UnableToGenerateRandomBytes(String),
    #[display(fmt = "AES cipher error: {_0}")]
    AESCipherError(String),
    Internal(String),
}

/// Enum representing different encryption algorithms.
#[derive(Serialize, Deserialize, Debug)]
pub enum EncryptionAlgorithm {
    /// AES-256-CBC algorithm.
    AES256CBC,
    // Placeholder for future algorithms.
}

/// `EncryptedData` represents encrypted data for a wallet.
///
/// This struct encapsulates all essential components required to securely encrypt
/// and subsequently decrypt a wallet mnemonic and data. It is designed to be self-contained,
/// meaning it includes not only the encrypted data but also all the necessary metadata
/// and parameters for decryption. This makes the struct portable and convenient for
/// use in various scenarios, allowing decryption of the mnemonic in different
/// environments or applications, provided the correct password or seed is supplied.
///
/// The `EncryptedData` struct is typically used for wallet encryption in blockchain-based applications,
/// providing a robust and comprehensive approach to securing sensitive mnemonic data.
#[derive(Serialize, Deserialize, Debug)]
pub struct EncryptedData {
    /// Version of the encrypted data format.
    /// This version value allows future changes to this struct while maintaining backward compatibility.
    pub version: u8,

    /// The encryption algorithm used to encrypt the mnemonic.
    /// Example: "AES-256-CBC".
    pub encryption_algorithm: EncryptionAlgorithm,

    /// Detailed information about the key derivation process. This includes
    /// the specific algorithm used (e.g., Argon2) and its parameters.
    pub key_derivation_details: KeyDerivationDetails,

    /// The initialization vector (IV) used in the AES encryption process.
    /// The IV ensures that the encryption process produces unique ciphertext
    /// for the same plaintext and key when encrypted multiple times.
    /// Stored as a Base64-encoded string.
    pub iv: String,

    /// The encrypted mnemonic data. This is the ciphertext generated
    /// using the specified encryption algorithm, key, and IV.
    /// Stored as a Base64-encoded string.
    pub ciphertext: String,

    /// The HMAC tag used for verifying the integrity and authenticity of the encrypted data.
    /// This tag is crucial for validating that the data has not been tampered with.
    /// Stored as a Base64-encoded string.
    pub tag: String,
}

/// Encrypts the provided data using AES-256-CBC encryption and HMAC for integrity check.
///
/// This function performs several operations:
/// - It generates an Initialization Vector (IV) for the AES encryption.
/// - It creates an AES-256-CBC cipher instance and encrypts the data with the provided key and the generated IV.
/// - It creates an HMAC tag for verifying the integrity of the encrypted data.
/// - It constructs an [`EncryptedData`] instance containing all the necessary components for decryption.
///
/// # Returns
/// `MmResult<EncryptedData, EncryptionError>` - The result is either an [`EncryptedData`]
/// struct containing all the necessary components for decryption, or an [`EncryptionError`] in case of failure.
///
/// # Errors
/// This function can return various errors related to IV generation, AES encryption, HMAC creation, and data encoding.
pub fn encrypt_data(
    data: &[u8],
    key_derivation_details: KeyDerivationDetails,
    key_aes: &[u8; 32],
    key_hmac: &[u8; 32],
) -> MmResult<EncryptedData, EncryptionError> {
    // Generate IV
    let mut iv = [0u8; 16];
    common::os_rng(&mut iv).map_to_mm(|e| EncryptionError::UnableToGenerateRandomBytes(e.to_string()))?;
    drop_mutability!(iv);

    // Create an AES-256-CBC cipher instance, encrypt the data with the key and the IV and get the ciphertext
    let msg_len = data.len();
    let buffer_len = msg_len + 16 - (msg_len % 16);
    let mut buffer = vec![0u8; buffer_len];
    buffer[..msg_len].copy_from_slice(data);
    let ciphertext = Aes256CbcEnc::new(key_aes.into(), &iv.into())
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, msg_len)
        .map_to_mm(|e| EncryptionError::AESCipherError(e.to_string()))?;

    // Create HMAC tag
    let mut mac = Hmac::<Sha256>::new_from_slice(key_hmac).map_to_mm(|e| EncryptionError::Internal(e.to_string()))?;
    mac.update(ciphertext);
    mac.update(&iv);
    let tag = mac.finalize().into_bytes();

    let encrypted_data = EncryptedData {
        version: ENCRYPTED_DATA_VERSION,
        encryption_algorithm: EncryptionAlgorithm::AES256CBC,
        key_derivation_details,
        iv: STANDARD.encode(iv),
        ciphertext: STANDARD.encode(ciphertext),
        tag: STANDARD.encode(tag),
    };

    Ok(encrypted_data)
}
