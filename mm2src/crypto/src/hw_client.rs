use crate::hw_error::HwError;
#[cfg(not(target_arch = "wasm32"))]
use crate::hw_error::HwResult;
use async_trait::async_trait;
#[cfg(not(target_os = "ios"))]
use common::custom_futures::timeout::FutureTimerExt;
use derive_more::Display;
use futures::FutureExt;
use mm2_err_handle::prelude::*;
use rpc::v1::types::H160 as H160Json;
use rpc_task::RpcTaskError;
use std::sync::Arc;
use std::time::Duration;
use trezor::client::TrezorClient;
use trezor::device_info::TrezorDeviceInfo;
use trezor::{TrezorError, TrezorProcessingError, TrezorRequestProcessor};

pub type HwPubkey = H160Json;

#[derive(Display)]
pub enum HwProcessingError<E> {
    HwError(HwError),
    ProcessorError(E),
    InternalError(String),
}

impl<E> From<HwError> for HwProcessingError<E> {
    fn from(e: HwError) -> Self {
        HwProcessingError::HwError(e)
    }
}

impl<E> From<TrezorError> for HwProcessingError<E> {
    fn from(e: TrezorError) -> Self {
        HwProcessingError::HwError(HwError::from(e))
    }
}

impl<E> From<TrezorProcessingError<E>> for HwProcessingError<E> {
    fn from(e: TrezorProcessingError<E>) -> Self {
        match e {
            TrezorProcessingError::TrezorError(trezor) => HwProcessingError::from(trezor),
            TrezorProcessingError::ProcessorError(processor) => HwProcessingError::ProcessorError(processor),
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
pub enum HwWalletType {
    Trezor,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HwDeviceInfo {
    Trezor(TrezorDeviceInfo),
}

#[derive(Debug, Serialize)]
pub enum HwConnectionStatus {
    Connected,
    /// `Unreachable` means that the device is either disconnected or is in an incorrect state,
    /// so it should be reinitialized.
    Unreachable,
}

#[async_trait]
pub trait TrezorConnectProcessor: TrezorRequestProcessor {
    async fn on_connect(&self) -> MmResult<Duration, HwProcessingError<Self::Error>>;

    async fn on_connected(&self) -> MmResult<(), HwProcessingError<Self::Error>>;

    async fn on_connection_failed(&self) -> MmResult<(), HwProcessingError<Self::Error>>;

    /// Helper to upcast to super trait object
    fn as_base_shared(&self) -> Arc<dyn TrezorRequestProcessor<Error = Self::Error>>;
}

#[derive(Clone)]
pub enum HwClient {
    Trezor(TrezorClient),
}

impl From<TrezorClient> for HwClient {
    fn from(trezor: TrezorClient) -> Self {
        HwClient::Trezor(trezor)
    }
}

impl HwClient {
    pub fn hw_wallet_type(&self) -> HwWalletType {
        match self {
            HwClient::Trezor(_) => HwWalletType::Trezor,
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn trezor(
        processor: Arc<dyn TrezorConnectProcessor<Error = RpcTaskError>>,
    ) -> MmResult<TrezorClient, HwProcessingError<RpcTaskError>> {
        let timeout = processor.on_connect().await.map_mm_err()?;

        let fut = async move {
            // `find_devices` in a browser leads to a popup that asks the user which device he wants to connect.
            // So we shouldn't ask in a loop like we do natively.
            let mut devices = trezor::transport::webusb::find_devices()
                .boxed()
                .timeout(timeout)
                .await
                .map_to_mm(|_| HwError::ConnectionTimedOut { timeout })
                .map_mm_err()?
                .map_mm_err()?;
            if devices.available.is_empty() {
                return MmError::err(HwProcessingError::HwError(HwError::NoTrezorDeviceAvailable));
            }
            let device = devices.available.remove(0);
            device.connect().await.map_mm_err()
        };

        match fut.await {
            Ok(transport) => {
                processor.on_connected().await.map_mm_err()?;
                Ok(TrezorClient::from_transport(transport))
            },
            Err(e) => {
                processor.on_connection_failed().await.map_mm_err()?;
                Err(e)
            },
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "ios")))]
    pub(crate) async fn trezor(
        processor: Arc<dyn TrezorConnectProcessor<Error = RpcTaskError>>,
    ) -> MmResult<TrezorClient, HwProcessingError<RpcTaskError>> {
        use common::custom_futures::timeout::TimeoutError;
        use common::executor::Timer;
        use trezor::transport::ConnectableDeviceWrapper;

        async fn try_to_connect<C>() -> HwResult<Option<TrezorClient>>
        where
            C: ConnectableDeviceWrapper + 'static,
        {
            let mut devices = C::find_devices().await.map_mm_err()?;
            if devices.is_empty() {
                return Ok(None);
            }
            if devices.len() != 1 {
                return MmError::err(HwError::CannotChooseDevice { count: devices.len() });
            }
            let device = devices.remove(0);
            let transport = device.connect().await.map_mm_err()?;
            let trezor = TrezorClient::from_transport(transport);
            Ok(Some(trezor))
        }

        let fut = async move {
            loop {
                if let Some(trezor) = try_to_connect::<trezor::transport::usb::UsbAvailableDevice>().await? {
                    return Ok(trezor);
                }

                #[cfg(feature = "trezor-udp")]
                // try also to connect to emulator over UDP
                if let Some(trezor) = try_to_connect::<trezor::transport::udp::UdpAvailableDevice>().await? {
                    return Ok(trezor);
                }

                Timer::sleep(1.).await;
            }
        };

        let timeout = processor.on_connect().await.map_mm_err()?;
        let result: Result<HwResult<TrezorClient>, TimeoutError> = fut.boxed().timeout(timeout).await;
        match result {
            Ok(Ok(trezor)) => {
                processor.on_connected().await.map_mm_err()?;
                Ok(trezor)
            },
            Ok(Err(hw_err)) => {
                processor.on_connection_failed().await.map_mm_err()?;
                Err(hw_err.map(HwProcessingError::from))
            },
            Err(_timed_out) => {
                processor.on_connection_failed().await.map_mm_err()?;
                MmError::err(HwProcessingError::HwError(HwError::ConnectionTimedOut { timeout }))
            },
        }
    }

    #[cfg(target_os = "ios")]
    pub(crate) async fn trezor(
        _processor: Arc<dyn TrezorConnectProcessor<Error = RpcTaskError>>,
    ) -> MmResult<TrezorClient, HwProcessingError<RpcTaskError>> {
        MmError::err(HwProcessingError::HwError(HwError::Internal(
            "Not supported on iOS!".into(),
        )))
    }
}
