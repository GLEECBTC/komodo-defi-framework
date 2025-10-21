use crate::proto::ProtoMessage;
use crate::transport::protocol::{Link, Protocol, ProtocolV1};
use crate::transport::{ConnectableDeviceWrapper, Transport, TREZOR_DEVICES};
use crate::TrezorResult;

use async_trait::async_trait;
use hw_common::transport::libusb::{
    GetDevicesFilters, UsbAvailableDevice as UsbAvailableDeviceImpl, UsbContext, UsbDevice,
};
use mm2_err_handle::prelude::MmResultExt;
use std::time::Duration;

pub use hw_common::transport::libusb::UsbDeviceInfo;

// TODO these timeouts should be optional and depend on the context of use.
const READ_TIMEOUT: Duration = Duration::from_secs(600);
const WRITE_TIMEOUT: Duration = Duration::from_secs(600);

// The following constants are imported from
// https://github.com/tezedge/tezedge-client/blob/master/trezor_api/src/transport/usb.rs
const CONFIG_ID: u8 = 0;
const INTERFACE: u8 = 0;
const INTERFACE_DESCRIPTOR: u8 = 0;
const LIBUSB_CLASS_VENDOR_SPEC: u8 = 0xff;

pub struct UsbTransport {
    protocol: ProtocolV1<UsbLink>,
}

#[async_trait]
impl Transport for UsbTransport {
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
}

struct UsbLink {
    device: UsbDevice,
}

#[async_trait]
impl Link for UsbLink {
    async fn write_chunk(&mut self, chunk: Vec<u8>) -> TrezorResult<()> {
        // don't try to reconnect since libusb requires to enumerate all devices, ope and, claim interface again
        Ok(self.device.write_chunk(chunk, WRITE_TIMEOUT).await.map_mm_err()?)
    }

    async fn read_chunk(&mut self, chunk_len: u32) -> TrezorResult<Vec<u8>> {
        // don't try to reconnect since libusb requires to enumerate all devices, ope and, claim interface again
        Ok(self
            .device
            .read_chunk(chunk_len as usize, READ_TIMEOUT)
            .await
            .map_mm_err()?)
    }
}

async fn find_devices() -> TrezorResult<Vec<UsbAvailableDevice>> {
    let context = UsbContext::new().map_mm_err()?;
    let filters = GetDevicesFilters {
        config_id: CONFIG_ID,
        interface_id: INTERFACE,
        interface_descriptor: INTERFACE_DESCRIPTOR,
        interface_class_code: LIBUSB_CLASS_VENDOR_SPEC,
    };
    Ok(context
        .get_devices(filters)
        .map_mm_err()?
        .into_iter()
        .filter(is_trezor)
        .map(UsbAvailableDevice)
        .collect())
}

pub struct UsbAvailableDevice(UsbAvailableDeviceImpl);

impl UsbAvailableDevice {
    /// Please note [`hw_common::transport::libusb::UsbAvailableDevice::connect`] spawns a thread.
    async fn connect(&self) -> TrezorResult<UsbTransport> {
        let link = UsbLink {
            device: self.0.connect().map_mm_err()?,
        };
        Ok(UsbTransport {
            protocol: ProtocolV1 { link },
        })
    }

    pub fn device_info(&self) -> &UsbDeviceInfo {
        self.0.device_info()
    }
}

fn is_trezor(device: &UsbAvailableDeviceImpl) -> bool {
    let device_info = device.device_info();
    let (vendor_id, product_id) = (device_info.vendor_id, device_info.product_id);
    TREZOR_DEVICES
        .iter()
        .any(|expected| vendor_id == expected.vendor_id && product_id == expected.product_id)
}

#[async_trait]
impl ConnectableDeviceWrapper for UsbAvailableDevice {
    type TransportType = UsbTransport;

    async fn find_devices() -> TrezorResult<Vec<Self>>
    where
        Self: Sized,
    {
        find_devices().await
    }

    async fn connect(&self) -> TrezorResult<Self::TransportType> {
        UsbAvailableDevice::connect(self).await
    }
}
