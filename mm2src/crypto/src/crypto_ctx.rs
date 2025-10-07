use crate::global_hd_ctx::{GlobalHDAccountArc, GlobalHDAccountCtx};
use crate::hw_client::{HwDeviceInfo, HwProcessingError, HwPubkey, TrezorConnectProcessor};
use crate::hw_ctx::{HardwareWalletArc, HardwareWalletCtx};
use crate::hw_error::HwError;
#[cfg(target_arch = "wasm32")]
use crate::metamask_ctx::{MetamaskArc, MetamaskCtx, MetamaskError};
use crate::privkey::{key_pair_from_seed, PrivKeyError};
use crate::shared_db_id::{shared_db_id_from_seed, SharedDbIdError};
use arrayref::array_ref;
use common::bits256;
use common::log::info;
use derive_more::Display;
use keys::{KeyPair, Public as PublicKey, Secret as Secp256k1Secret};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::common_errors::InternalError;
use mm2_err_handle::prelude::*;
use parking_lot::RwLock;
use primitives::hash::H160;
use rpc_task::RpcTaskError;
use std::ops::Deref;
use std::sync::Arc;

pub type CryptoInitResult<T> = Result<T, MmError<CryptoInitError>>;

#[derive(Debug, Display)]
pub enum CryptoInitError {
    NotInitialized,
    InitializedAlready,
    #[display(fmt = "Passphrase cannot be an empty string")]
    EmptyPassphrase,
    #[display(fmt = "Invalid passphrase: '{_0}'")]
    InvalidPassphrase(PrivKeyError),
    Internal(String),
}

impl From<PrivKeyError> for CryptoInitError {
    fn from(e: PrivKeyError) -> Self {
        CryptoInitError::InvalidPassphrase(e)
    }
}

impl From<SharedDbIdError> for CryptoInitError {
    fn from(e: SharedDbIdError) -> Self {
        match e {
            SharedDbIdError::EmptyPassphrase => CryptoInitError::EmptyPassphrase,
            SharedDbIdError::Internal(internal) => CryptoInitError::Internal(internal),
        }
    }
}

#[derive(Debug, Display)]
pub enum CryptoCtxError {
    #[display(fmt = "'CryptoCtx' is not initialized")]
    NotInitialized,
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

#[derive(Debug)]
pub enum HwCtxInitError<ProcessorError> {
    InitializingAlready,
    UnexpectedPubkey {
        actual_pubkey: HwPubkey,
        expected_pubkey: HwPubkey,
    },
    HwError(HwError),
    ProcessorError(ProcessorError),
    InternalError(String),
}

impl<ProcessorError> From<HwProcessingError<ProcessorError>> for HwCtxInitError<ProcessorError> {
    fn from(e: HwProcessingError<ProcessorError>) -> Self {
        match e {
            HwProcessingError::HwError(hw_error) => HwCtxInitError::HwError(hw_error),
            HwProcessingError::ProcessorError(processor_error) => HwCtxInitError::ProcessorError(processor_error),
            HwProcessingError::InternalError(internal_error) => HwCtxInitError::InternalError(internal_error),
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
pub enum MetamaskCtxInitError {
    InitializingAlready,
    MetamaskError(MetamaskError),
}

#[cfg(target_arch = "wasm32")]
impl From<MetamaskError> for MetamaskCtxInitError {
    fn from(value: MetamaskError) -> Self {
        MetamaskCtxInitError::MetamaskError(value)
    }
}

pub struct CryptoCtx {
    /// secp256k1 key pair derived from either:
    /// * Iguana passphrase,
    ///   cf. `key_pair_from_seed`;
    /// * BIP39 passphrase at `mm2_internal_der_path`,
    ///   cf. [`GlobalHDAccountCtx::new`].
    secp256k1_key_pair: KeyPair,
    key_pair_policy: KeyPairPolicy,
    /// Can be initialized on [`CryptoCtx::init_hw_ctx_with_trezor`].
    hw_ctx: RwLock<InitializationState<HardwareWalletArc>>,
    #[cfg(target_arch = "wasm32")]
    metamask_ctx: RwLock<InitializationState<MetamaskArc>>,
}

impl CryptoCtx {
    pub fn is_init(ctx: &MmArc) -> MmResult<bool, InternalError> {
        match CryptoCtx::from_ctx(ctx).split_mm() {
            Ok(_) => Ok(true),
            Err((CryptoCtxError::NotInitialized, _trace)) => Ok(false),
            Err((other, trace)) => MmError::err_with_trace(InternalError(other.to_string()), trace),
        }
    }

    pub fn from_ctx(ctx: &MmArc) -> MmResult<Arc<CryptoCtx>, CryptoCtxError> {
        let ctx_field = ctx
            .crypto_ctx
            .lock()
            .map_to_mm(|poison| CryptoCtxError::Internal(poison.to_string()))?;
        let ctx = match ctx_field.deref() {
            Some(ctx) => ctx,
            None => return MmError::err(CryptoCtxError::NotInitialized),
        };
        ctx.clone()
            .downcast()
            .map_err(|_| MmError::new(CryptoCtxError::Internal("Error casting the context field".to_owned())))
    }

    #[inline]
    pub fn key_pair_policy(&self) -> &KeyPairPolicy {
        &self.key_pair_policy
    }

    /// This is our public ID, allowing us to be different from other peers.
    /// This should also be our public key which we'd use for P2P message verification.
    #[inline]
    pub fn mm2_internal_public_id(&self) -> bits256 {
        // Compressed public key is going to be 33 bytes.
        let public = self.mm2_internal_pubkey();
        // First byte is a prefix, https://davidederosa.com/basic-blockchain-programming/elliptic-curve-keys/.
        bits256 {
            bytes: *array_ref!(public, 1, 32),
        }
    }

    /// Returns `secp256k1` key-pair.
    /// It can be used for mm2 internal purposes such as signing P2P messages.
    ///
    /// To activate coins, consider matching [`CryptoCtx::key_pair_ctx`] manually.
    ///
    /// # Security
    ///
    /// If [`CryptoCtx::key_pair_ctx`] is `Iguana`, then the returning key-pair is used to activate coins.
    /// Please use this method carefully.
    #[inline]
    pub fn mm2_internal_key_pair(&self) -> &KeyPair {
        &self.secp256k1_key_pair
    }

    /// Returns `secp256k1` public key.
    /// It can be used for mm2 internal purposes such as P2P peer ID.
    ///
    /// To activate coins, consider matching [`CryptoCtx::key_pair_ctx`] manually.
    ///
    /// # Security
    ///
    /// If [`CryptoCtx::key_pair_ctx`] is `Iguana`, then the returning key-pair can be also used
    /// at the activated coins.
    /// Please use this method carefully.
    #[inline]
    pub fn mm2_internal_pubkey(&self) -> PublicKey {
        *self.secp256k1_key_pair.public()
    }

    /// Returns `secp256k1` public key hex.
    /// It can be used for mm2 internal purposes such as P2P peer ID.
    ///
    /// To activate coins, consider matching [`CryptoCtx::key_pair_ctx`] manually.
    ///
    /// # Security
    ///
    /// If [`CryptoCtx::key_pair_ctx`] is `Iguana`, then the returning public key can be also used
    /// at the activated coins.
    /// Please use this method carefully.
    #[inline]
    pub fn mm2_internal_pubkey_hex(&self) -> String {
        hex::encode(&*self.mm2_internal_pubkey())
    }

    /// Returns `secp256k1` private key as `H256` bytes.
    /// It can be used for mm2 internal purposes such as signing P2P messages.
    ///
    /// To activate coins, consider matching [`CryptoCtx::key_pair_ctx`] manually.
    ///
    /// # Security
    ///
    /// If [`CryptoCtx::key_pair_ctx`] is `Iguana`, then the returning private is used to activate coins.
    /// Please use this method carefully.
    #[inline]
    pub fn mm2_internal_privkey_secret(&self) -> Secp256k1Secret {
        self.secp256k1_key_pair.private().secret
    }

    /// Returns `secp256k1` private key as `[u8]` slice.
    /// It can be used for mm2 internal purposes such as signing P2P messages.
    /// Please consider using [`CryptoCtx::mm2_internal_privkey_bytes`] instead.
    ///
    /// If you don't need to borrow the secret bytes, consider using [`CryptoCtx::mm2_internal_privkey_bytes`] instead.
    /// To activate coins, consider matching [`CryptoCtx::key_pair_ctx`] manually.
    ///
    /// # Security
    ///
    /// If [`CryptoCtx::key_pair_ctx`] is `Iguana`, then the returning private is used to activate coins.
    /// Please use this method carefully.
    #[inline]
    pub fn mm2_internal_privkey_slice(&self) -> &[u8] {
        self.secp256k1_key_pair.private().secret.as_slice()
    }

    #[inline]
    pub fn hw_ctx(&self) -> Option<HardwareWalletArc> {
        self.hw_ctx.read().to_option().cloned()
    }

    #[cfg(target_arch = "wasm32")]
    pub fn metamask_ctx(&self) -> Option<MetamaskArc> {
        self.metamask_ctx.read().to_option().cloned()
    }

    /// Returns an `RIPEMD160(SHA256(x))` where x is secp256k1 pubkey that identifies a Hardware Wallet device or an HD master private key.
    #[inline]
    pub fn hw_wallet_rmd160(&self) -> Option<H160> {
        self.hw_ctx.read().to_option().map(|hw_ctx| hw_ctx.rmd160())
    }

    pub fn init_with_iguana_passphrase(ctx: MmArc, passphrase: &str) -> CryptoInitResult<Arc<CryptoCtx>> {
        Self::init_crypto_ctx_with_policy_builder(ctx, passphrase, KeyPairPolicyBuilder::Iguana)
    }

    pub fn init_with_global_hd_account(ctx: MmArc, passphrase: &str) -> CryptoInitResult<Arc<CryptoCtx>> {
        let builder = KeyPairPolicyBuilder::GlobalHDAccount;
        Self::init_crypto_ctx_with_policy_builder(ctx, passphrase, builder)
    }

    pub async fn init_hw_ctx_with_trezor(
        &self,
        processor: Arc<dyn TrezorConnectProcessor<Error = RpcTaskError>>,
        expected_pubkey: Option<HwPubkey>,
    ) -> MmResult<(HwDeviceInfo, HardwareWalletArc), HwCtxInitError<RpcTaskError>> {
        {
            let mut state = self.hw_ctx.write();
            if let InitializationState::Initializing = state.deref() {
                return MmError::err(HwCtxInitError::InitializingAlready);
            }

            *state = InitializationState::Initializing;
        }

        let result = init_check_hw_ctx_with_trezor(processor, expected_pubkey).await;
        let new_state = match result {
            Ok((_, ref hw_ctx)) => InitializationState::Ready(hw_ctx.clone()),
            Err(_) => InitializationState::NotInitialized,
        };

        *self.hw_ctx.write() = new_state;
        result.mm_err(HwCtxInitError::from)
    }

    /// # Todo
    ///
    /// Consider taking `processor: Processor` and `expected_address: Option<String>`
    /// the same way as `CryptoCtx::init_hw_ctx_with_trezor`.
    #[cfg(target_arch = "wasm32")]
    pub async fn init_metamask_ctx(&self, project_name: String) -> MmResult<MetamaskArc, MetamaskCtxInitError> {
        {
            let mut state = self.metamask_ctx.write();
            if let InitializationState::Initializing = state.deref() {
                return MmError::err(MetamaskCtxInitError::InitializingAlready);
            }

            *state = InitializationState::Initializing;
        }

        let metamask_ctx = MetamaskCtx::init(project_name).await.map_mm_err()?;
        let metamask_arc = MetamaskArc::new(metamask_ctx);

        *self.metamask_ctx.write() = InitializationState::Ready(metamask_arc.clone());
        Ok(metamask_arc)
    }

    pub fn reset_hw_ctx(&self) {
        let mut state = self.hw_ctx.write();
        *state = InitializationState::NotInitialized;
    }

    #[cfg(target_arch = "wasm32")]
    pub fn reset_metamask_ctx(&self) {
        let mut state = self.metamask_ctx.write();
        *state = InitializationState::NotInitialized;
    }

    fn init_crypto_ctx_with_policy_builder(
        ctx: MmArc,
        passphrase: &str,
        policy_builder: KeyPairPolicyBuilder,
    ) -> CryptoInitResult<Arc<CryptoCtx>> {
        let mut ctx_field = ctx
            .crypto_ctx
            .lock()
            .map_to_mm(|poison| CryptoInitError::Internal(poison.to_string()))?;
        if ctx_field.is_some() {
            return MmError::err(CryptoInitError::InitializedAlready);
        }

        if passphrase.is_empty() {
            return MmError::err(CryptoInitError::EmptyPassphrase);
        }

        let (secp256k1_key_pair, key_pair_policy) = policy_builder.build(passphrase)?;
        let rmd160 = secp256k1_key_pair.public().address_hash();
        let shared_db_id = shared_db_id_from_seed(passphrase).map_mm_err()?;

        let crypto_ctx = CryptoCtx {
            secp256k1_key_pair,
            key_pair_policy,
            hw_ctx: RwLock::new(InitializationState::NotInitialized),
            #[cfg(target_arch = "wasm32")]
            metamask_ctx: RwLock::new(InitializationState::NotInitialized),
        };

        let result = Arc::new(crypto_ctx);
        *ctx_field = Some(result.clone());
        drop(ctx_field);

        ctx.rmd160
            .set(rmd160)
            .map_to_mm(|_| CryptoInitError::Internal("Already Initialized".to_string()))?;
        ctx.shared_db_id
            .set(shared_db_id)
            .map_to_mm(|_| CryptoInitError::Internal("Already Initialized".to_string()))?;

        info!("Public key hash: {rmd160}");
        info!("Shared Database ID: {shared_db_id}");
        Ok(result)
    }
}

enum KeyPairPolicyBuilder {
    Iguana,
    GlobalHDAccount,
}

impl KeyPairPolicyBuilder {
    /// [`KeyPairPolicyBuilder::build`] is fired if all checks pass **only**.
    fn build(self, passphrase: &str) -> CryptoInitResult<(KeyPair, KeyPairPolicy)> {
        match self {
            KeyPairPolicyBuilder::Iguana => {
                let secp256k1_key_pair = key_pair_from_seed(passphrase).map_mm_err()?;
                Ok((secp256k1_key_pair, KeyPairPolicy::Iguana))
            },
            KeyPairPolicyBuilder::GlobalHDAccount => {
                let (mm2_internal_key_pair, global_hd_ctx) = GlobalHDAccountCtx::new(passphrase).map_mm_err()?;
                let key_pair_policy = KeyPairPolicy::GlobalHDAccount(global_hd_ctx.into_arc());
                Ok((mm2_internal_key_pair, key_pair_policy))
            },
        }
    }
}

#[derive(Clone)]
pub enum KeyPairPolicy {
    Iguana,
    GlobalHDAccount(GlobalHDAccountArc),
}

async fn init_check_hw_ctx_with_trezor(
    processor: Arc<dyn TrezorConnectProcessor<Error = RpcTaskError>>,
    expected_pubkey: Option<HwPubkey>,
) -> MmResult<(HwDeviceInfo, HardwareWalletArc), HwCtxInitError<RpcTaskError>> {
    let (hw_device_info, hw_ctx) = HardwareWalletCtx::init_with_trezor(processor).await.map_mm_err()?;
    let expected_pubkey = match expected_pubkey {
        Some(expected) => expected,
        None => return Ok((hw_device_info, hw_ctx)),
    };
    let actual_pubkey = hw_ctx.hw_pubkey();

    // Check whether the connected Trezor device has an expected pubkey.
    if actual_pubkey != expected_pubkey {
        return MmError::err(HwCtxInitError::UnexpectedPubkey {
            actual_pubkey,
            expected_pubkey,
        });
    }
    Ok((hw_device_info, hw_ctx))
}

enum InitializationState<Feature> {
    NotInitialized,
    Initializing,
    Ready(Feature),
}

impl<Feature> InitializationState<Feature> {
    fn to_option(&self) -> Option<&Feature> {
        match self {
            InitializationState::Ready(feature) => Some(feature),
            _ => None,
        }
    }
}
