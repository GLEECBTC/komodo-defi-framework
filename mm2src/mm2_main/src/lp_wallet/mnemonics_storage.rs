use crypto::EncryptedData;
use derive_more::Display;
use mm2_core::mm_ctx::{MmArc, WALLET_FILE_EXTENSION};
use mm2_err_handle::prelude::*;
use mm2_io::fs::{ensure_file_is_writable, list_files_by_extension};
use std::path::PathBuf;

type WalletsStorageResult<T> = Result<T, MmError<WalletsStorageError>>;

#[derive(Debug, Deserialize, Display, Serialize)]
pub enum WalletsStorageError {
    #[display(fmt = "Error writing to file: {_0}")]
    FsWriteError(String),
    #[display(fmt = "Error reading from file: {_0}")]
    FsReadError(String),
    #[display(fmt = "Invalid wallet name: {_0}")]
    InvalidWalletName(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

fn wallet_file_path(ctx: &MmArc, wallet_name: &str) -> Result<PathBuf, String> {
    let wallet_name_trimmed = wallet_name.trim();
    if wallet_name_trimmed.is_empty() {
        return Err("Wallet name cannot be empty or consist only of whitespace.".to_string());
    }

    if !wallet_name_trimmed
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ')
    {
        return Err(format!(
            "Invalid wallet name: '{wallet_name_trimmed}'. Only alphanumeric characters, spaces, dash and underscore are allowed."
        ));
    }

    Ok(ctx
        .db_root()
        .join(format!("{wallet_name_trimmed}.{WALLET_FILE_EXTENSION}")))
}

/// Saves the passphrase to a file associated with the given wallet name.
///
/// # Returns
/// Result indicating success or an error.
pub(super) async fn save_encrypted_passphrase(
    ctx: &MmArc,
    wallet_name: &str,
    encrypted_passphrase_data: &EncryptedData,
) -> WalletsStorageResult<()> {
    let wallet_path = wallet_file_path(ctx, wallet_name).map_to_mm(WalletsStorageError::InvalidWalletName)?;
    ensure_file_is_writable(&wallet_path).map_to_mm(WalletsStorageError::FsWriteError)?;
    mm2_io::fs::write_json(encrypted_passphrase_data, &wallet_path, true)
        .await
        .mm_err(|e| WalletsStorageError::FsWriteError(e.to_string()))
}

/// Reads the encrypted passphrase data from the file associated with the given wallet name, if available.
///
/// This function is responsible for retrieving the encrypted passphrase data from a file for a specific wallet.
/// The data is expected to be in the format of `EncryptedData`, which includes
/// all necessary components for decryption, such as the encryption algorithm, key derivation
///
/// # Returns
/// `WalletsStorageResult<Option<EncryptedData>>` - The encrypted passphrase data or an error if the
/// reading process fails. An `Ok(None)` is returned if the wallet file does not exist.
///
/// # Errors
/// Returns a `WalletsStorageError` if the file cannot be read or the data cannot be deserialized into
/// `EncryptedData`.
pub(super) async fn read_encrypted_passphrase(
    ctx: &MmArc,
    wallet_name: &str,
) -> WalletsStorageResult<Option<EncryptedData>> {
    let wallet_path = wallet_file_path(ctx, wallet_name).map_to_mm(WalletsStorageError::InvalidWalletName)?;
    mm2_io::fs::read_json(&wallet_path).await.mm_err(|e| {
        WalletsStorageError::FsReadError(format!(
            "Error reading passphrase from file {}: {}",
            wallet_path.display(),
            e
        ))
    })
}

pub(super) async fn read_all_wallet_names(ctx: &MmArc) -> WalletsStorageResult<impl Iterator<Item = String>> {
    let wallet_names = list_files_by_extension(&ctx.db_root(), WALLET_FILE_EXTENSION, false)
        .await
        .mm_err(|e| WalletsStorageError::FsReadError(format!("Error reading wallets directory: {e}")))?;
    Ok(wallet_names)
}

/// Deletes the wallet file associated with the given wallet name.
pub(super) async fn delete_wallet(ctx: &MmArc, wallet_name: &str) -> WalletsStorageResult<()> {
    let wallet_path = wallet_file_path(ctx, wallet_name).map_to_mm(WalletsStorageError::InvalidWalletName)?;
    mm2_io::fs::remove_file_async(&wallet_path)
        .await
        .mm_err(|e| WalletsStorageError::FsWriteError(e.to_string()))
}
