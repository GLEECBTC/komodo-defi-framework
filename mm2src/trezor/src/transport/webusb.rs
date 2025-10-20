use crate::proto::ProtoMessage;
use crate::transport::protocol::{Link, Protocol, ProtocolV1};
use crate::transport::{Transport, TrezorDevice, TREZOR_DEVICES};
use crate::{TrezorError, TrezorResult};
use async_trait::async_trait;
use common::log::warn;
use hw_common::transport::webusb_driver::{DeviceFilter, WebUsbDevice, WebUsbWrapper};
use mm2_err_handle::prelude::*;

pub use hw_common::transport::webusb_driver::WebUsbDeviceInfo;

const T1HID_VENDOR: u16 = 0x534c;
const CONFIGURATION_ID: u8 = 1;
const DEBUG_ENDPOINT_ID: u8 = 2;

impl From<TrezorDevice> for DeviceFilter {
    fn from(d: TrezorDevice) -> Self {
        DeviceFilter::with_product_id(d.vendor_id, d.product_id)
    }
}

pub struct WebUsbTransport {
    protocol: ProtocolV1<WebUsbLink>,
}

#[async_trait]
impl Transport for WebUsbTransport {
    async fn session_begin(&mut self) -> TrezorResult<()> {
        self.protocol.session_begin().await
    }

    async fn session_end(&mut self) -> TrezorResult<()> {
        self.protocol.session_end().await
    }

    async fn write_message(&mut self, message: ProtoMessage) -> TrezorResult<()> {
        self.protocol.write(message).await
    }

    async fn read_message(&mut self) -> TrezorResult<ProtoMessage> {
        self.protocol.read().await
    }

    async fn is_connected(&mut self) -> TrezorResult<bool> {
        self.protocol.link().is_connected().await
    }
}

struct WebUsbLink {
    device: WebUsbDevice,
    interface_number: u8,
    endpoint_number: u8,
    debug: bool,
}

#[async_trait]
impl Link for WebUsbLink {
    async fn write_chunk(&mut self, chunk: Vec<u8>) -> TrezorResult<()> {
        if !self.device.is_open().await.map_mm_err()? {
            return MmError::err(TrezorError::DeviceDisconnected);
        }

        self.device.write_chunk(self.endpoint_number, chunk).await.map_mm_err()
    }

    async fn read_chunk(&mut self, chunk_len: u32) -> TrezorResult<Vec<u8>> {
        if !self.device.is_open().await.map_mm_err()? {
            return MmError::err(TrezorError::DeviceDisconnected);
        }

        self.device
            .read_chunk(self.endpoint_number, chunk_len)
            .await
            .map_mm_err()
    }
}

impl WebUsbLink {
    #[allow(dead_code)]
    pub fn is_debug(&self) -> bool {
        self.debug
    }

    pub async fn is_connected(&self) -> TrezorResult<bool> {
        self.device.is_open().await.mm_err(TrezorError::from)
    }

    /// Configure the WebUSB device.
    async fn establish_connection(&self, first: bool) -> TrezorResult<()> {
        self.device.open().await.map_mm_err()?;
        if first {
            self.device.select_configuration(CONFIGURATION_ID).await.map_mm_err()?;
            if let Err(e) = self.device.reset_device().await {
                // Reset fails on ChromeOS and Windows.
                warn!("{}", e);
            }
        }
        self.device.claim_interface(self.interface_number).await.map_mm_err()?;
        Ok(())
    }
}

pub struct WebUsbAvailableDevice(WebUsbLink);

impl WebUsbAvailableDevice {
    pub async fn connect(self) -> TrezorResult<WebUsbTransport> {
        let first = true;
        self.0.establish_connection(first).await?;
        Ok(WebUsbTransport {
            protocol: ProtocolV1 { link: self.0 },
        })
    }

    fn from_webusb_device(device: WebUsbDevice) -> Result<WebUsbAvailableDevice, String> {
        let (interface_number, endpoint_number) = match device.device_info.interface {
            Some(ref interface) => (interface.interface_number, interface.endpoint_number),
            None => return Err(format!("Unknown interface: {:?}", device.device_info)),
        };
        Ok(WebUsbAvailableDevice(WebUsbLink {
            device,
            interface_number,
            endpoint_number,
            debug: endpoint_number == DEBUG_ENDPOINT_ID,
        }))
    }
}

pub struct FoundDevices {
    pub available: Vec<WebUsbAvailableDevice>,
    pub not_supported: Vec<WebUsbDeviceInfo>,
}

/// The implementation is inspired by (WebUsbPlugin::_listDevices)[https://github.com/trezor/trezor-link/blob/master/src/lowlevel/webusb.js#L74].
///
/// # Usage
///
/// This function **must** be called via a user gesture like a touch or mouse click.
pub async fn find_devices() -> TrezorResult<FoundDevices> {
    let wrapper = WebUsbWrapper::new().map_mm_err()?;
    wrapper
        .request_device(TREZOR_DEVICES.iter().copied().map(DeviceFilter::from).collect())
        .await
        .map_mm_err()?;
    let devices_iter = wrapper.get_devices().await.map_mm_err()?.into_iter().filter(is_trezor);
    let mut available = Vec::new();
    let mut not_supported = Vec::new();
    for device in devices_iter {
        if is_hid(&device) {
            not_supported.push(device.device_info);
            continue;
        }
        match WebUsbAvailableDevice::from_webusb_device(device) {
            Ok(device) => available.push(device),
            Err(e) => {
                warn!("Skip the device: {}", e);
                continue;
            },
        }
    }
    Ok(FoundDevices {
        available,
        not_supported,
    })
}

fn is_trezor(device: &WebUsbDevice) -> bool {
    let (vendor_id, product_id) = (device.device_info.vendor_id, device.device_info.product_id);
    TREZOR_DEVICES
        .iter()
        .any(|expected| vendor_id == expected.vendor_id && product_id == expected.product_id)
}

fn is_hid(device: &WebUsbDevice) -> bool {
    device.device_info.vendor_id == T1HID_VENDOR
}
