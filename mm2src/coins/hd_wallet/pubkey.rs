#[cfg(feature = "enable-sia")]
use crate::siacoin::sia_hd_wallet::Ed25519ExtendedPublicKey;
use crate::CoinProtocol;

use super::*;
use async_trait::async_trait;
use bip32::Prefix;
use crypto::hw_rpc_task::HwConnectStatuses;
use crypto::trezor::trezor_rpc_task::{TrezorRpcTaskProcessor, TryIntoUserAction};
use crypto::trezor::utxo::IGNORE_XPUB_MAGIC;
use crypto::trezor::ProcessTrezorResponse;
use crypto::trezor::TrezorMessageType;
use crypto::{CryptoCtx, DerivationPath, EcdsaCurve, HardwareWalletArc, XPub, XPubConverter};
use mm2_core::mm_ctx::MmArc;
use rpc_task::{RpcTask, RpcTaskHandleShared};
use std::sync::Arc;

const SHOW_PUBKEY_ON_DISPLAY: bool = false;

/// A trait that should be implemented by any extended public key type
/// to allow it to work with the HD wallet traits.
pub trait ExtendedPublicKeyOps: FromStr + Sized {
    /// Derives a child extended public key from the current one.
    fn derive_child(&self, child_number: ChildNumber) -> Result<Self, Bip32Error>;
    /// Converts the extended public key to a string.
    fn to_string(&self, prefix: Prefix) -> String;
}

impl ExtendedPublicKeyOps for Secp256k1ExtendedPublicKey {
    fn derive_child(&self, child_number: ChildNumber) -> Result<Self, Bip32Error> {
        self.derive_child(child_number)
    }

    fn to_string(&self, prefix: Prefix) -> String {
        self.to_string(prefix)
    }
}

#[cfg(feature = "enable-sia")]
impl ExtendedPublicKeyOps for Ed25519ExtendedPublicKey {
    fn derive_child(&self, child_number: ChildNumber) -> Result<Self, Bip32Error> {
        self.derive_child(child_number)
    }

    fn to_string(&self, prefix: Prefix) -> String {
        self.to_string(prefix)
    }
}

/// This trait should be implemented for coins
/// to support extracting extended public keys from any depth.
/// The extraction can be from either an internal or external wallet.
#[async_trait]
pub trait ExtractExtendedPubkey {
    type ExtendedPublicKey;

    async fn extract_extended_pubkey<XPubExtractor>(
        &self,
        xpub_extractor: Option<XPubExtractor>,
        derivation_path: DerivationPath,
    ) -> MmResult<Self::ExtendedPublicKey, HDExtractPubkeyError>
    where
        XPubExtractor: HDXPubExtractor + Send;
}

/// A trait for extracting an extended public key from an external source.
#[async_trait]
pub trait HDXPubExtractor: Sync {
    async fn extract_xpub(
        &self,
        trezor_coin: String,
        derivation_path: DerivationPath,
    ) -> MmResult<XPub, HDExtractPubkeyError>;
}

/// The task for extracting an extended public key from an external source.
pub enum RpcTaskXPubExtractor<Task: RpcTask> {
    Trezor {
        hw_ctx: HardwareWalletArc,
        task_handle: RpcTaskHandleShared<Task>,
        statuses: HwConnectStatuses<Task::InProgressStatus, Task::AwaitingStatus>,
        trezor_message_type: TrezorMessageType,
    },
}

#[async_trait]
impl<Task> HDXPubExtractor for RpcTaskXPubExtractor<Task>
where
    Task: RpcTask,
    Task::UserAction: TryIntoUserAction + Send,
{
    async fn extract_xpub(
        &self,
        trezor_coin: String,
        derivation_path: DerivationPath,
    ) -> MmResult<XPub, HDExtractPubkeyError> {
        match self {
            RpcTaskXPubExtractor::Trezor {
                hw_ctx,
                task_handle,
                statuses,
                trezor_message_type,
            } => match trezor_message_type {
                TrezorMessageType::Bitcoin => {
                    Self::extract_utxo_xpub_from_trezor(
                        hw_ctx,
                        task_handle.clone(),
                        statuses,
                        trezor_coin,
                        derivation_path,
                    )
                    .await
                },
                TrezorMessageType::Ethereum => {
                    Self::extract_eth_xpub_from_trezor(hw_ctx, task_handle.clone(), statuses, derivation_path).await
                },
            },
        }
    }
}

impl<Task> RpcTaskXPubExtractor<Task>
where
    Task: RpcTask,
    Task::UserAction: TryIntoUserAction + Send,
{
    pub fn new_trezor_extractor(
        ctx: &MmArc,
        task_handle: RpcTaskHandleShared<Task>,
        statuses: HwConnectStatuses<Task::InProgressStatus, Task::AwaitingStatus>,
        coin_protocol: CoinProtocol,
    ) -> MmResult<RpcTaskXPubExtractor<Task>, HDExtractPubkeyError> {
        let crypto_ctx = CryptoCtx::from_ctx(ctx).map_mm_err()?;
        let hw_ctx = crypto_ctx
            .hw_ctx()
            .or_mm_err(|| HDExtractPubkeyError::HwContextNotInitialized)?;

        let trezor_message_type = match coin_protocol {
            CoinProtocol::UTXO { .. } => TrezorMessageType::Bitcoin,
            CoinProtocol::QTUM => TrezorMessageType::Bitcoin,
            CoinProtocol::ETH { .. } | CoinProtocol::ERC20 { .. } => TrezorMessageType::Ethereum,
            _ => return Err(MmError::new(HDExtractPubkeyError::CoinDoesntSupportTrezor)),
        };
        Ok(RpcTaskXPubExtractor::Trezor {
            hw_ctx,
            task_handle,
            statuses,
            trezor_message_type,
        })
    }

    async fn extract_utxo_xpub_from_trezor(
        hw_ctx: &HardwareWalletArc,
        task_handle: RpcTaskHandleShared<Task>,
        statuses: &HwConnectStatuses<Task::InProgressStatus, Task::AwaitingStatus>,
        trezor_coin: String,
        derivation_path: DerivationPath,
    ) -> MmResult<XPub, HDExtractPubkeyError> {
        let pubkey_processor = TrezorRpcTaskProcessor::new(task_handle, statuses.to_trezor_request_statuses());
        let pubkey_processor = Arc::new(pubkey_processor);
        let mut trezor_session = hw_ctx.trezor(pubkey_processor.clone()).await.map_mm_err()?;
        let xpub = trezor_session
            .get_public_key(
                derivation_path,
                trezor_coin,
                EcdsaCurve::Secp256k1,
                SHOW_PUBKEY_ON_DISPLAY,
                IGNORE_XPUB_MAGIC,
            )
            .await
            .map_mm_err()?
            .process(pubkey_processor.clone())
            .await
            .map_mm_err()?;
        // Despite we pass `IGNORE_XPUB_MAGIC` to the [`TrezorSession::get_public_key`] method,
        // Trezor sometimes returns pubkeys with magic prefixes like `dgub` prefix for DOGE coin.
        // So we need to replace the magic prefix manually.
        XPubConverter::replace_magic_prefix(xpub).mm_err(HDExtractPubkeyError::from)
    }

    async fn extract_eth_xpub_from_trezor(
        hw_ctx: &HardwareWalletArc,
        task_handle: RpcTaskHandleShared<Task>,
        statuses: &HwConnectStatuses<Task::InProgressStatus, Task::AwaitingStatus>,
        derivation_path: DerivationPath,
    ) -> MmResult<XPub, HDExtractPubkeyError> {
        let pubkey_processor = TrezorRpcTaskProcessor::new(task_handle, statuses.to_trezor_request_statuses());
        let pubkey_processor = Arc::new(pubkey_processor);
        let mut trezor_session = hw_ctx.trezor(pubkey_processor.clone()).await.map_mm_err()?;
        trezor_session
            .get_eth_public_key(&derivation_path, SHOW_PUBKEY_ON_DISPLAY)
            .await
            .map_mm_err()?
            .process(pubkey_processor)
            .await
            .mm_err(HDExtractPubkeyError::from)
    }
}
