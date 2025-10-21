use crate::hw_client::{
    HwClient, HwConnectionStatus, HwDeviceInfo, HwProcessingError, HwPubkey, TrezorConnectProcessor,
};
use crate::hw_error::HwError;
use crate::trezor::TrezorSession;
use crate::{mm2_internal_der_path, HwWalletType};
use bitcrypto::dhash160;
use common::log::warn;
use hw_common::primitives::{EcdsaCurve, Secp256k1ExtendedPublicKey};
use keys::Public as PublicKey;
use mm2_err_handle::prelude::*;
use primitives::hash::{H160, H264};
use rpc_task::RpcTaskError;
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use trezor::utxo::IGNORE_XPUB_MAGIC;
use trezor::{ProcessTrezorResponse, TrezorRequestProcessor};

const MM2_INTERNAL_ECDSA_CURVE: EcdsaCurve = EcdsaCurve::Secp256k1;
const MM2_TREZOR_INTERNAL_COIN: &str = "Komodo";
const SHOW_PUBKEY_ON_DISPLAY: bool = false;

#[derive(Clone)]
pub struct HardwareWalletArc(Arc<HardwareWalletCtx>);

impl Deref for HardwareWalletArc {
    type Target = HardwareWalletCtx;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl HardwareWalletArc {
    pub fn new(ctx: HardwareWalletCtx) -> HardwareWalletArc {
        HardwareWalletArc(Arc::new(ctx))
    }
}

pub struct HardwareWalletCtx {
    /// The pubkey derived from `mm2_internal_der_path`.
    pub(crate) hw_internal_pubkey: H264,
    pub(crate) hw_wallet_type: HwWalletType,
    /// Please avoid locking multiple mutexes.
    /// The mutex hasn't to be locked while the client is used
    /// because every variant of `HwClient` uses an internal mutex to operate with the device.
    /// But it has to be locked while the client is initialized.
    pub(crate) hw_wallet: HwClient,
    /// Whether the Hardware Wallet is connected.
    pub(crate) hw_wallet_connected: AtomicBool,
}

impl HardwareWalletCtx {
    pub(crate) async fn init_with_trezor(
        processor: Arc<dyn TrezorConnectProcessor<Error = RpcTaskError>>,
    ) -> MmResult<(HwDeviceInfo, HardwareWalletArc), HwProcessingError<RpcTaskError>> {
        let mut trezor = HwClient::trezor(processor.clone()).await?;

        let (hw_device_info, hw_internal_pubkey) = {
            let processor = processor.as_base_shared();
            let (device_info, mut session) = trezor.init_new_session(processor).await.map_mm_err()?;
            let hw_internal_pubkey = HardwareWalletCtx::trezor_mm_internal_pubkey(&mut session)
                .await
                .map_mm_err()?;
            (HwDeviceInfo::Trezor(device_info), hw_internal_pubkey)
        };

        let hw_wallet = HwClient::Trezor(trezor);
        let hw_ctx = HardwareWalletArc::new(HardwareWalletCtx {
            hw_internal_pubkey,
            hw_wallet_type: hw_wallet.hw_wallet_type(),
            hw_wallet,
            hw_wallet_connected: AtomicBool::new(true),
        });
        Ok((hw_device_info, hw_ctx))
    }

    pub fn hw_wallet_type(&self) -> HwWalletType {
        self.hw_wallet_type
    }

    /// Returns a Trezor session.
    pub async fn trezor(
        &self,
        processor: Arc<dyn TrezorRequestProcessor<Error = RpcTaskError>>,
    ) -> MmResult<TrezorSession<'_>, HwError> {
        if !self.hw_wallet_connected.load(Ordering::Relaxed) {
            return MmError::err(HwError::DeviceDisconnected);
        }

        let HwClient::Trezor(ref trezor) = self.hw_wallet;
        let session = trezor.session(processor).await;
        self.check_if_connected(session).await
    }

    pub async fn trezor_connection_status(&self) -> HwConnectionStatus {
        if !self.hw_wallet_connected.load(Ordering::Relaxed) {
            return HwConnectionStatus::Unreachable;
        }

        let HwClient::Trezor(ref trezor) = self.hw_wallet;
        let session = match trezor.try_session_if_not_occupied() {
            // No 'processor' in the returned session, so it is only for checking conn
            Some(session) => session,
            // If we got `None`, the session mutex is occupied by another task,
            // so for now we can consider the Trezor device as connected.
            None => return HwConnectionStatus::Connected,
        };

        match self.check_if_connected(session).await {
            Ok(_session) => HwConnectionStatus::Connected,
            Err(_) => HwConnectionStatus::Unreachable,
        }
    }

    pub fn secp256k1_pubkey(&self) -> PublicKey {
        PublicKey::Compressed(self.hw_internal_pubkey)
    }

    /// Returns `RIPEMD160(SHA256(x))` where x is a pubkey extracted from the Hardware wallet.
    pub fn rmd160(&self) -> H160 {
        h160_from_h264(&self.hw_internal_pubkey)
    }

    /// Returns serializable/deserializable Hardware wallet pubkey.
    pub fn hw_pubkey(&self) -> HwPubkey {
        hw_pubkey_from_h264(&self.hw_internal_pubkey)
    }

    pub(crate) async fn trezor_mm_internal_pubkey(
        trezor_session: &mut TrezorSession<'_>,
    ) -> MmResult<H264, HwProcessingError<RpcTaskError>> {
        let path = mm2_internal_der_path();
        let processor = trezor_session
            .processor
            .as_ref()
            .or_mm_err(|| HwProcessingError::InternalError("No processor in session object".to_string()))?
            .clone();
        let mm2_internal_xpub = trezor_session
            .get_public_key(
                path,
                MM2_TREZOR_INTERNAL_COIN.to_string(),
                MM2_INTERNAL_ECDSA_CURVE,
                SHOW_PUBKEY_ON_DISPLAY,
                IGNORE_XPUB_MAGIC,
            )
            .await
            .map_mm_err()?
            .process(processor)
            .await
            .map_mm_err()?;
        let extended_pubkey = Secp256k1ExtendedPublicKey::from_str(&mm2_internal_xpub)
            .map_to_mm(HwError::from)
            .map_mm_err()?;
        Ok(H264::from(extended_pubkey.public_key().serialize()))
    }

    #[cfg(target_arch = "wasm32")]
    async fn check_if_connected<'a>(&self, mut session: TrezorSession<'a>) -> MmResult<TrezorSession<'a>, HwError> {
        match session.is_connected().await {
            Ok(true) => Ok(session),
            Ok(false) => {
                self.handle_hw_error(&HwError::DeviceDisconnected);
                MmError::err(HwError::DeviceDisconnected)
            },
            Err(e) => {
                self.handle_hw_error(&e);
                Err(e.map(HwError::from))
            },
        }
    }

    /// Currently, we use the heavy [`TrezorSession::ping`] method although it may lead
    /// to the Button/PIN press request if the device is **locked**.
    /// So even if we cancel the Button/PIN request,
    /// the display will have time to draw something, that is, blink.
    ///
    /// TODO figure out how to implement [`Transport::is_connected`] for the native `UsbTransport`.
    #[cfg(not(target_arch = "wasm32"))]
    async fn check_if_connected<'a>(&self, mut session: TrezorSession<'a>) -> MmResult<TrezorSession<'a>, HwError> {
        match session.ping().await {
            Ok(resp) => {
                resp.cancel_if_not_ready().await;
                Ok(session)
            },
            Err(e) => {
                self.handle_hw_error(&e);
                Err(e.map(HwError::from))
            },
        }
    }

    /// If either the [`HardwareWalletCtx::hw_wallet`] client failed on a connection check,
    /// we can't use it anymore.
    fn handle_hw_error(&self, error: &dyn fmt::Display) {
        warn!("Error checking Trezor device status. The device is no longer available: '{error}'");
        self.hw_wallet_connected.store(false, Ordering::Relaxed);
    }
}

/// Applies `RIPEMD160(SHA256(h264))` to the given `h264`.
fn h160_from_h264(h264: &H264) -> H160 {
    dhash160(h264.as_slice())
}

/// Converts `H264` into a serializable/deserializable Hardware wallet pubkey.
fn hw_pubkey_from_h264(h264: &H264) -> HwPubkey {
    HwPubkey::from(h160_from_h264(h264).take())
}
