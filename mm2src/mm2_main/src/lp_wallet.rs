use common::password_policy::{password_policy, PasswordPolicyError};
use common::HttpStatusCode;
use crypto::{
    decrypt_mnemonic, encrypt_mnemonic, generate_mnemonic, CryptoCtx, CryptoInitError, EncryptedData, MnemonicError,
};
use derive_more::Display;
use enum_derives::EnumFromStringify;
use http::StatusCode;
use itertools::Itertools;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use serde::de::DeserializeOwned;
use serde_json::{self as json, Value as Json};

cfg_wasm32! {
    use crate::lp_wallet::mnemonics_wasm_db::{WalletsDb, WalletsDBError};
    use mm2_core::mm_ctx::from_ctx;
    use mm2_db::indexed_db::{ConstructibleDb, DbLocked, InitDbResult};
    use mnemonics_wasm_db::{delete_wallet, read_all_wallet_names, read_encrypted_passphrase, save_encrypted_passphrase};
    use std::sync::Arc;

    type WalletsDbLocked<'a> = DbLocked<'a, WalletsDb>;
}

cfg_native! {
    use mnemonics_storage::{delete_wallet, read_all_wallet_names, read_encrypted_passphrase, save_encrypted_passphrase, WalletsStorageError};
}
#[cfg(not(target_arch = "wasm32"))]
mod mnemonics_storage;
#[cfg(target_arch = "wasm32")]
mod mnemonics_wasm_db;

type WalletInitResult<T> = Result<T, MmError<WalletInitError>>;

#[derive(Debug, Deserialize, Display, EnumFromStringify, Serialize)]
pub enum WalletInitError {
    #[display(fmt = "Error deserializing '{field}' config field: {error}")]
    ErrorDeserializingConfig {
        field: String,
        error: String,
    },
    #[display(fmt = "The '{field}' field not found in the config")]
    FieldNotFoundInConfig {
        field: String,
    },
    #[display(fmt = "Wallets storage error: {_0}")]
    WalletsStorageError(String),
    #[display(
        fmt = "Passphrase doesn't match the one from file, please create a new wallet if you want to use a new passphrase"
    )]
    PassphraseMismatch,
    #[display(fmt = "Error generating or decrypting mnemonic: {_0}")]
    MnemonicError(String),
    #[display(fmt = "Error initializing crypto context: {_0}")]
    CryptoInitError(String),
    #[display(fmt = "Password does not meet policy requirements: {_0}")]
    #[from_stringify("PasswordPolicyError")]
    PasswordPolicyViolation(String),
    InternalError(String),
}

impl From<MnemonicError> for WalletInitError {
    fn from(e: MnemonicError) -> Self {
        WalletInitError::MnemonicError(e.to_string())
    }
}

impl From<CryptoInitError> for WalletInitError {
    fn from(e: CryptoInitError) -> Self {
        WalletInitError::CryptoInitError(e.to_string())
    }
}

#[derive(Debug, Deserialize, Display, Serialize)]
pub enum ReadPassphraseError {
    #[display(fmt = "Wallets storage error: {_0}")]
    WalletsStorageError(String),
    #[display(fmt = "Error decrypting passphrase: {_0}")]
    DecryptionError(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<ReadPassphraseError> for WalletInitError {
    fn from(e: ReadPassphraseError) -> Self {
        match e {
            ReadPassphraseError::WalletsStorageError(e) => WalletInitError::WalletsStorageError(e),
            ReadPassphraseError::DecryptionError(e) => WalletInitError::MnemonicError(e),
            ReadPassphraseError::Internal(e) => WalletInitError::InternalError(e),
        }
    }
}

#[cfg(target_arch = "wasm32")]
struct WalletsContext {
    wallets_db: ConstructibleDb<WalletsDb>,
}

#[cfg(target_arch = "wasm32")]
impl WalletsContext {
    /// Obtains a reference to this crate context, creating it if necessary.
    fn from_ctx(ctx: &MmArc) -> Result<Arc<WalletsContext>, String> {
        Ok(try_s!(from_ctx(&ctx.wallets_ctx, move || {
            Ok(WalletsContext {
                #[cfg(target_arch = "wasm32")]
                wallets_db: ConstructibleDb::new_global_db(ctx),
            })
        })))
    }

    pub async fn wallets_db(&self) -> InitDbResult<WalletsDbLocked<'_>> {
        self.wallets_db.get_or_initialize().await
    }
}

// Utility function for deserialization to reduce repetition
fn deserialize_config_field<T: DeserializeOwned>(ctx: &MmArc, field: &str) -> WalletInitResult<T> {
    json::from_value::<T>(ctx.conf[field].clone()).map_to_mm(|e| WalletInitError::ErrorDeserializingConfig {
        field: field.to_owned(),
        error: e.to_string(),
    })
}

// Utility function to handle passphrase encryption and saving
async fn encrypt_and_save_passphrase(
    ctx: &MmArc,
    wallet_name: &str,
    passphrase: &str,
    wallet_password: &str,
) -> WalletInitResult<()> {
    let encrypted_passphrase_data = encrypt_mnemonic(passphrase, wallet_password).map_mm_err()?;
    save_encrypted_passphrase(ctx, wallet_name, &encrypted_passphrase_data)
        .await
        .mm_err(|e| WalletInitError::WalletsStorageError(e.to_string()))
}

/// A convenience wrapper that calls [`try_load_wallet_passphrase`] for the currently active wallet.
async fn try_load_active_wallet_passphrase(
    ctx: &MmArc,
    wallet_password: &str,
) -> MmResult<Option<String>, ReadPassphraseError> {
    let wallet_name = ctx
        .wallet_name
        .get()
        .ok_or(ReadPassphraseError::Internal(
            "`wallet_name` not initialized yet!".to_string(),
        ))?
        .clone()
        .ok_or_else(|| {
            ReadPassphraseError::Internal("Cannot read stored passphrase: no active wallet is set.".to_string())
        })?;

    try_load_wallet_passphrase(ctx, &wallet_name, wallet_password).await
}

/// Loads (reads from storage and decrypts) a passphrase for a specific wallet by name.
///
/// Returns `Ok(None)` if the passphrase is not found in storage. This is an expected
/// outcome for a new wallet or when using a legacy config where the passphrase is not saved.
async fn try_load_wallet_passphrase(
    ctx: &MmArc,
    wallet_name: &str,
    wallet_password: &str,
) -> MmResult<Option<String>, ReadPassphraseError> {
    let encrypted = read_encrypted_passphrase(ctx, wallet_name)
        .await
        .mm_err(|e| ReadPassphraseError::WalletsStorageError(e.to_string()))?;

    match encrypted {
        Some(encrypted_passphrase) => {
            let mnemonic = decrypt_mnemonic(&encrypted_passphrase, wallet_password)
                .mm_err(|e| ReadPassphraseError::DecryptionError(e.to_string()))?;
            Ok(Some(mnemonic))
        },
        None => Ok(None),
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum Passphrase {
    Encrypted(EncryptedData),
    Decrypted(String),
}

fn deserialize_wallet_config(ctx: &MmArc) -> WalletInitResult<(Option<String>, Option<Passphrase>)> {
    let passphrase = deserialize_config_field::<Option<Passphrase>>(ctx, "passphrase")?;
    // New approach for passphrase, `wallet_name` is needed in the config to enable multi-wallet support.
    // In this case the passphrase will be generated if not provided.
    // The passphrase will then be encrypted and saved whether it was generated or provided.
    let wallet_name = deserialize_config_field::<Option<String>>(ctx, "wallet_name")?;
    Ok((wallet_name, passphrase))
}

/// Passphrase is not provided. Generate, encrypt and save passphrase if not already saved.
async fn retrieve_or_create_passphrase(
    ctx: &MmArc,
    wallet_name: &str,
    wallet_password: &str,
) -> WalletInitResult<Option<String>> {
    match try_load_active_wallet_passphrase(ctx, wallet_password)
        .await
        .map_mm_err()?
    {
        Some(passphrase_from_file) => {
            // If an existing passphrase is found, return it
            Ok(Some(passphrase_from_file))
        },
        None => {
            if wallet_password.is_empty() {
                return MmError::err(WalletInitError::PasswordPolicyViolation(
                    "`wallet_password` cannot be empty".to_string(),
                ));
            }
            let is_weak_password_accepted = ctx.conf["allow_weak_password"].as_bool().unwrap_or(false);
            if !is_weak_password_accepted {
                password_policy(wallet_password)?;
            }
            // If no passphrase is found, generate a new one
            let new_passphrase = generate_mnemonic(ctx).map_mm_err()?.to_string();
            // Encrypt and save the new passphrase
            encrypt_and_save_passphrase(ctx, wallet_name, &new_passphrase, wallet_password)
                .await
                .map_mm_err()?;
            Ok(Some(new_passphrase))
        },
    }
}

/// Passphrase is provided in plaintext. Encrypt and save passphrase if not already saved.
async fn confirm_or_encrypt_and_store_passphrase(
    ctx: &MmArc,
    wallet_name: &str,
    passphrase: &str,
    wallet_password: &str,
) -> WalletInitResult<Option<String>> {
    match try_load_active_wallet_passphrase(ctx, wallet_password)
        .await
        .map_mm_err()?
    {
        Some(passphrase_from_file) if passphrase == passphrase_from_file => {
            // If an existing passphrase is found and it matches the provided passphrase, return it
            Ok(Some(passphrase_from_file))
        },
        None => {
            if wallet_password.is_empty() {
                return MmError::err(WalletInitError::PasswordPolicyViolation(
                    "`wallet_password` cannot be empty".to_string(),
                ));
            }
            let is_weak_password_accepted = ctx.conf["allow_weak_password"].as_bool().unwrap_or(false);
            if !is_weak_password_accepted {
                password_policy(wallet_password)?;
            }
            // If no passphrase is found in the file, encrypt and save the provided passphrase
            encrypt_and_save_passphrase(ctx, wallet_name, passphrase, wallet_password)
                .await
                .map_mm_err()?;
            Ok(Some(passphrase.to_string()))
        },
        _ => {
            // If an existing passphrase is found and it does not match the provided passphrase, return an error
            Err(WalletInitError::PassphraseMismatch.into())
        },
    }
}

/// Encrypted passphrase is provided. Decrypt and save encrypted passphrase if not already saved.
async fn decrypt_validate_or_save_passphrase(
    ctx: &MmArc,
    wallet_name: &str,
    encrypted_passphrase_data: EncryptedData,
    wallet_password: &str,
) -> WalletInitResult<Option<String>> {
    // Decrypt the provided encrypted passphrase
    let decrypted_passphrase = decrypt_mnemonic(&encrypted_passphrase_data, wallet_password).map_mm_err()?;

    match try_load_active_wallet_passphrase(ctx, wallet_password)
        .await
        .map_mm_err()?
    {
        Some(passphrase_from_file) if decrypted_passphrase == passphrase_from_file => {
            // If an existing passphrase is found and it matches the decrypted passphrase, return it
            Ok(Some(decrypted_passphrase))
        },
        None => {
            save_encrypted_passphrase(ctx, wallet_name, &encrypted_passphrase_data)
                .await
                .mm_err(|e| WalletInitError::WalletsStorageError(e.to_string()))?;
            Ok(Some(decrypted_passphrase))
        },
        _ => {
            // If an existing passphrase is found and it does not match the decrypted passphrase, return an error
            Err(WalletInitError::PassphraseMismatch.into())
        },
    }
}

async fn process_wallet_with_name(
    ctx: &MmArc,
    wallet_name: &str,
    passphrase: Option<Passphrase>,
    wallet_password: &str,
) -> WalletInitResult<Option<String>> {
    match passphrase {
        None => retrieve_or_create_passphrase(ctx, wallet_name, wallet_password).await,
        Some(Passphrase::Decrypted(passphrase)) => {
            confirm_or_encrypt_and_store_passphrase(ctx, wallet_name, &passphrase, wallet_password).await
        },
        Some(Passphrase::Encrypted(encrypted_data)) => {
            decrypt_validate_or_save_passphrase(ctx, wallet_name, encrypted_data, wallet_password).await
        },
    }
}

async fn process_passphrase_logic(
    ctx: &MmArc,
    wallet_name: Option<&str>,
    passphrase: Option<Passphrase>,
) -> WalletInitResult<Option<String>> {
    match (wallet_name, passphrase) {
        (None, None) => Ok(None),
        // Legacy approach for passphrase, no `wallet_name` is needed in the config, in this case the passphrase is not encrypted and saved.
        (None, Some(Passphrase::Decrypted(passphrase))) => Ok(Some(passphrase)),
        // Importing an encrypted passphrase without a wallet name is not supported since it's not possible to save the passphrase.
        (None, Some(Passphrase::Encrypted(_))) => Err(WalletInitError::FieldNotFoundInConfig {
            field: "wallet_name".to_owned(),
        }
        .into()),

        (Some(wallet_name), passphrase_option) => {
            let wallet_password = deserialize_config_field::<String>(ctx, "wallet_password")?;
            process_wallet_with_name(ctx, wallet_name, passphrase_option, &wallet_password).await
        },
    }
}

fn initialize_crypto_context(ctx: &MmArc, passphrase: &str) -> WalletInitResult<()> {
    // This defaults to false to maintain backward compatibility.
    match ctx.enable_hd() {
        true => CryptoCtx::init_with_global_hd_account(ctx.clone(), passphrase).map_mm_err()?,
        false => CryptoCtx::init_with_iguana_passphrase(ctx.clone(), passphrase).map_mm_err()?,
    };
    Ok(())
}

/// Initializes and manages the wallet passphrase.
///
/// This function handles several scenarios based on the configuration:
/// - Deserializes the passphrase and wallet name from the configuration.
/// - If both wallet name and passphrase are `None`, the function sets up the context for "no login mode"
///   This mode can be entered after the function's execution, allowing access to Komodo DeFi Framework
///   functionalities that don't require a passphrase (e.g., viewing the orderbook).
/// - If a wallet name is provided without a passphrase, it first checks for the existence of a
///   passphrase file associated with the wallet. If no file is found, it generates a new passphrase,
///   encrypts it, and saves it, enabling multi-wallet support.
/// - If a passphrase is provided (with or without a wallet name), it uses the provided passphrase
///   and handles encryption and storage as needed.
/// - Initializes the cryptographic context based on the `enable_hd` configuration.
///
/// # Returns
/// `MmInitResult<()>` - Result indicating success or failure of the initialization process.
///
/// # Errors
/// Returns `MmInitError` if deserialization fails or if there are issues in passphrase handling.
///
pub(crate) async fn initialize_wallet_passphrase(ctx: &MmArc) -> WalletInitResult<()> {
    let (wallet_name, passphrase) = deserialize_wallet_config(ctx)?;
    ctx.wallet_name
        .set(wallet_name.clone())
        .map_to_mm(|_| WalletInitError::InternalError("Already Initialized".to_string()))
        .map_mm_err()?;

    let passphrase = process_passphrase_logic(ctx, wallet_name.as_deref(), passphrase)
        .await
        .map_mm_err()?;
    if let Some(passphrase) = passphrase {
        initialize_crypto_context(ctx, &passphrase).map_mm_err()?;
    }

    Ok(())
}

/// `MnemonicFormat` is an enum representing the format of a mnemonic.
///
/// It has two variants:
/// - `Encrypted`: This variant represents an encrypted mnemonic. It does not carry any associated data.
/// - `PlainText`: This variant represents a plaintext mnemonic. It carries the password to decrypt the mnemonic in string format.
#[derive(Debug, Deserialize)]
#[serde(tag = "format", content = "password", rename_all = "lowercase")]
pub enum MnemonicFormat {
    Encrypted,
    PlainText(String),
}

/// `GetMnemonicRequest` is a struct representing a request to get a mnemonic.
///
/// It contains a single field, `mnemonic_format`, which is an instance of the `MnemonicFormat` enum.
/// The `#[serde(flatten)]` attribute is used so that the fields of the `MnemonicFormat` enum are included
/// directly in the `GetMnemonicRequest` when it is deserialized, rather than nested under a
/// `mnemonic_format` field.
///
/// # Examples
///
/// For a `GetMnemonicRequest` where the `MnemonicFormat` is `Encrypted`, the JSON representation would be:
/// ```json
/// {
///   "format": "encrypted"
/// }
/// ```
///
/// For a `GetMnemonicRequest` where the `MnemonicFormat` is `PlainText` with a password of "password123", the JSON representation would be:
/// ```json
/// {
///   "format": "plaintext",
///   "password": "password123"
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct GetMnemonicRequest {
    #[serde(flatten)]
    pub mnemonic_format: MnemonicFormat,
}

/// `MnemonicForRpc` is an enum representing the format of a mnemonic for RPC communication.
///
/// It has two variants:
/// - `Encrypted`: This variant represents an encrypted mnemonic. It carries the [`EncryptedData`] struct.
/// - `PlainText`: This variant represents a plaintext mnemonic. It carries the mnemonic as a `String`.
#[derive(Serialize)]
#[serde(tag = "format", rename_all = "lowercase")]
pub enum MnemonicForRpc {
    Encrypted { encrypted_mnemonic_data: EncryptedData },
    PlainText { mnemonic: String },
}

impl From<EncryptedData> for MnemonicForRpc {
    fn from(encrypted_mnemonic_data: EncryptedData) -> Self {
        MnemonicForRpc::Encrypted {
            encrypted_mnemonic_data,
        }
    }
}

impl From<String> for MnemonicForRpc {
    fn from(mnemonic: String) -> Self {
        MnemonicForRpc::PlainText { mnemonic }
    }
}

/// [`GetMnemonicResponse`] is a struct representing the response to a get mnemonic request.
///
/// It contains a single field, `mnemonic`, which is an instance of the [`MnemonicForRpc`] enum.
/// The `#[serde(flatten)]` attribute is used so that the fields of the [`MnemonicForRpc`] enum are included
/// directly in the [`GetMnemonicResponse`] when it is serialized, rather than nested under a
/// `mnemonic` field.
///
/// # Examples
///
/// For a [`GetMnemonicResponse`] where the [`MnemonicForRpc`] is `Encrypted` with some [`EncryptedData`], the JSON representation would be:
/// ```json
/// {
///   "format": "encrypted",
///   "encrypted_mnemonic_data": {
///     // EncryptedData fields go here
///   }
/// }
/// ```
///
/// For a `GetMnemonicResponse` where the `MnemonicForRpc` is `PlainText` with a mnemonic of "your_mnemonic_here", the JSON representation would be:
/// ```json
/// {
///   "format": "plaintext",
///   "mnemonic": "your_mnemonic_here"
/// }
/// ```
#[derive(Serialize)]
pub struct GetMnemonicResponse {
    #[serde(flatten)]
    pub mnemonic: MnemonicForRpc,
}

#[derive(Debug, Display, Serialize, SerializeErrorType, EnumFromStringify)]
#[serde(tag = "error_type", content = "error_data")]
pub enum MnemonicRpcError {
    #[display(fmt = "Invalid request error: {_0}")]
    InvalidRequest(String),
    #[display(fmt = "Wallets storage error: {_0}")]
    WalletsStorageError(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
    #[display(fmt = "Invalid password error: {_0}")]
    #[from_stringify("MnemonicError")]
    InvalidPassword(String),
    #[display(fmt = "Password does not meet policy requirements: {_0}")]
    #[from_stringify("PasswordPolicyError")]
    PasswordPolicyViolation(String),
}

impl HttpStatusCode for MnemonicRpcError {
    fn status_code(&self) -> StatusCode {
        match self {
            MnemonicRpcError::InvalidRequest(_)
            | MnemonicRpcError::InvalidPassword(_)
            | MnemonicRpcError::PasswordPolicyViolation(_) => StatusCode::BAD_REQUEST,
            MnemonicRpcError::WalletsStorageError(_) | MnemonicRpcError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<WalletsStorageError> for MnemonicRpcError {
    fn from(e: WalletsStorageError) -> Self {
        MnemonicRpcError::WalletsStorageError(e.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
impl From<WalletsDBError> for MnemonicRpcError {
    fn from(e: WalletsDBError) -> Self {
        MnemonicRpcError::WalletsStorageError(e.to_string())
    }
}

impl From<ReadPassphraseError> for MnemonicRpcError {
    fn from(e: ReadPassphraseError) -> Self {
        match e {
            ReadPassphraseError::DecryptionError(e) => MnemonicRpcError::InvalidPassword(e),
            ReadPassphraseError::WalletsStorageError(e) => MnemonicRpcError::WalletsStorageError(e),
            ReadPassphraseError::Internal(e) => MnemonicRpcError::Internal(e),
        }
    }
}

/// Retrieves the wallet mnemonic in the requested format.
///
/// # Returns
///
/// A `Result` type containing:
///
/// * [`Ok`]([`GetMnemonicResponse`]) - The wallet mnemonic in the requested format.
/// * [`MmError`]<[`MnemonicRpcError>`]> - Returns specific [`MnemonicRpcError`] variants for different failure scenarios.
///
/// # Errors
///
/// This function will return an error in the following situations:
///
/// * The wallet name is not found in the context.
/// * The wallet is initialized without a name.
/// * The wallet passphrase file is not found for `MnemonicFormat::Encrypted`.
/// * The wallet mnemonic file is not found for `MnemonicFormat::PlainText`.
///
/// # Examples
///
/// ```rust
/// let ctx = MmArc::new(MmCtx::default());
/// let req = GetMnemonicRequest {
///     mnemonic_format: MnemonicFormat::Encrypted,
/// };
/// let result = get_mnemonic_rpc(ctx, req).await;
/// match result {
///     Ok(response) => println!("Mnemonic: {:?}", response.mnemonic),
///     Err(e) => println!("Error: {:?}", e),
/// }
/// ```
pub async fn get_mnemonic_rpc(ctx: MmArc, req: GetMnemonicRequest) -> MmResult<GetMnemonicResponse, MnemonicRpcError> {
    match req.mnemonic_format {
        MnemonicFormat::Encrypted => {
            let wallet_name = ctx
                .wallet_name
                .get()
                .ok_or(MnemonicRpcError::Internal(
                    "`wallet_name` not initialized yet!".to_string(),
                ))?
                .as_ref()
                .ok_or_else(|| {
                    MnemonicRpcError::Internal(
                        "Cannot get encrypted mnemonic: This operation requires an active named wallet.".to_string(),
                    )
                })?;
            let encrypted_mnemonic = read_encrypted_passphrase(&ctx, wallet_name)
                .await
                .map_mm_err()?
                .ok_or_else(|| MnemonicRpcError::InvalidRequest("Wallet mnemonic file not found".to_string()))?;
            Ok(GetMnemonicResponse {
                mnemonic: encrypted_mnemonic.into(),
            })
        },
        MnemonicFormat::PlainText(wallet_password) => {
            let plaintext_mnemonic = try_load_active_wallet_passphrase(&ctx, &wallet_password)
                .await
                .map_mm_err()?
                .ok_or_else(|| MnemonicRpcError::InvalidRequest("Wallet mnemonic file not found".to_string()))?;
            Ok(GetMnemonicResponse {
                mnemonic: plaintext_mnemonic.into(),
            })
        },
    }
}

/// The response to `get_wallet_names_rpc`, returns all created wallet names and the currently activated wallet name.
#[derive(Serialize)]
pub struct GetWalletNamesResponse {
    wallet_names: Vec<String>,
    activated_wallet: Option<String>,
}

/// Retrieves all created wallets and the currently activated wallet.
pub async fn get_wallet_names_rpc(ctx: MmArc, _req: Json) -> MmResult<GetWalletNamesResponse, MnemonicRpcError> {
    // We want to return wallet names in the same order for both native and wasm32 targets.
    let wallets = read_all_wallet_names(&ctx).await.map_mm_err()?.sorted().collect();
    // Note: `ok_or` is used here on `Constructible<Option<String>>` to handle the case where the wallet name is not set.
    // `wallet_name` can be `None` in the case of no-login mode.
    let activated_wallet = ctx.wallet_name.get().ok_or(MnemonicRpcError::Internal(
        "`wallet_name` not initialized yet!".to_string(),
    ))?;

    Ok(GetWalletNamesResponse {
        wallet_names: wallets,
        activated_wallet: activated_wallet.clone(),
    })
}

/// `ChangeMnemonicPasswordReq ` represents a request to update the password for Menmonic.
/// It includes the current password and the new password to be set.
#[derive(Debug, Deserialize)]
pub struct ChangeMnemonicPasswordReq {
    /// Current mnemonic password.
    pub current_password: String,
    /// New mnemonic password.
    pub new_password: String,
}

/// RPC function to handle a request for changing mnemonic password.
pub async fn change_mnemonic_password(ctx: MmArc, req: ChangeMnemonicPasswordReq) -> MmResult<(), MnemonicRpcError> {
    if req.new_password.is_empty() {
        return MmError::err(MnemonicRpcError::PasswordPolicyViolation(
            "`new_password` cannot be empty".to_string(),
        ));
    }
    let is_weak_password_accepted = ctx.conf["allow_weak_password"].as_bool().unwrap_or(false);
    if !is_weak_password_accepted {
        password_policy(&req.new_password)?;
    }
    let wallet_name = ctx
        .wallet_name
        .get()
        .ok_or(MnemonicRpcError::Internal(
            "`wallet_name` not initialized yet!".to_string(),
        ))?
        .as_ref()
        .ok_or_else(|| MnemonicRpcError::Internal("`wallet_name` cannot be None!".to_string()))?;
    // read mnemonic for a wallet_name using current user's password.
    let mnemonic = try_load_active_wallet_passphrase(&ctx, &req.current_password)
        .await
        .map_mm_err()?
        .ok_or(MmError::new(MnemonicRpcError::Internal(format!(
            "{wallet_name}: wallet mnemonic file not found"
        ))))?;
    // encrypt mnemonic with new passphrase.
    let encrypted_data = encrypt_mnemonic(&mnemonic, &req.new_password).map_mm_err()?;
    // save new encrypted mnemonic data with new password
    save_encrypted_passphrase(&ctx, wallet_name, &encrypted_data)
        .await
        .map_mm_err()?;

    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct DeleteWalletRequest {
    /// The name of the wallet to be deleted.
    pub wallet_name: String,
    /// The password to confirm wallet deletion.
    pub password: String,
}

/// Deletes a wallet. Requires password confirmation.
/// The active wallet cannot be deleted.
pub async fn delete_wallet_rpc(ctx: MmArc, req: DeleteWalletRequest) -> MmResult<(), MnemonicRpcError> {
    let active_wallet = ctx
        .wallet_name
        .get()
        .ok_or(MnemonicRpcError::Internal(
            "`wallet_name` not initialized yet!".to_string(),
        ))?
        .as_ref();

    if active_wallet == Some(&req.wallet_name) {
        return MmError::err(MnemonicRpcError::InvalidRequest(format!(
            "Cannot delete wallet '{}' as it is currently active.",
            req.wallet_name
        )));
    }

    // Verify the password by attempting to decrypt the mnemonic.
    let maybe_mnemonic = try_load_wallet_passphrase(&ctx, &req.wallet_name, &req.password)
        .await
        .map_mm_err()?;

    match maybe_mnemonic {
        Some(_) => {
            // Password is correct, proceed with deletion.
            delete_wallet(&ctx, &req.wallet_name).await.map_mm_err()?;
            Ok(())
        },
        None => {
            // This case implies no mnemonic file was found for the given wallet.
            MmError::err(MnemonicRpcError::InvalidRequest(format!(
                "Wallet '{}' not found.",
                req.wallet_name
            )))
        },
    }
}
