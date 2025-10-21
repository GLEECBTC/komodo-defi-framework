use crate::transport::{send_event_recv_response, InternalError};
use common::executor::{spawn_local_abortable, AbortOnDropHandle};
use common::{deserialize_from_js, log::error, serialize_to_js, stringify_js_error};
use derive_more::Display;
use futures::channel::{mpsc, oneshot};
use futures::StreamExt;
use js_sys::{Array, Uint8Array};
use mm2_err_handle::prelude::*;
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Usb, UsbDevice, UsbDeviceRequestOptions, UsbInTransferResult};

pub type WebUsbResult<T> = Result<T, MmError<WebUsbError>>;
type EventResultSender<T> = oneshot::Sender<WebUsbResult<T>>;
type WebUsbEventSender = mpsc::UnboundedSender<WebUsbEvent>;
pub type DeviceEventSender = mpsc::UnboundedSender<DeviceEvent>;
type DeviceEventReceiver = mpsc::UnboundedReceiver<DeviceEvent>;

#[derive(Display)]
pub enum WebUsbError {
    #[display(fmt = "WebUSB is not available on this browser")]
    NotSupported,
    #[display(fmt = "Error requesting access permission: {_0}")]
    ErrorRequestingDevice(String),
    #[display(fmt = "Error getting devices: {_0}")]
    ErrorGettingDevices(String),
    #[display(fmt = "Error setting configuration: configurationNumber={configuration_number}, error='{error}'")]
    ErrorSettingConfiguration { configuration_number: u8, error: String },
    #[display(fmt = "Error claiming an interface: interfaceNumber={interface_number}, error='{error}'")]
    ErrorClaimingInterface { interface_number: u8, error: String },
    #[display(fmt = "Error opening a device: {_0}")]
    ErrorOpeningDevice(String),
    #[display(fmt = "Error resetting device: {_0}")]
    ErrorResettingDevice(String),
    #[display(fmt = "Error writing a chunk: {_0}")]
    ErrorWritingChunk(String),
    #[display(fmt = "Error reading a chunk: {_0}")]
    ErrorReadingChunk(String),
    #[display(fmt = "Type mismatch: expected '{expected}', found '{found}'")]
    TypeMismatch { expected: String, found: String },
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl InternalError for WebUsbError {
    fn internal(e: String) -> Self {
        WebUsbError::Internal(e)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceFilter {
    #[serde(rename = "vendorId")]
    pub vendor_id: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "productId")]
    pub product_id: Option<u16>,
}

impl DeviceFilter {
    pub const fn new(vendor_id: u16) -> DeviceFilter {
        DeviceFilter {
            vendor_id,
            product_id: None,
        }
    }

    pub const fn with_product_id(vendor_id: u16, product_id: u16) -> DeviceFilter {
        DeviceFilter {
            vendor_id,
            product_id: Some(product_id),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WebUsbDeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial_number: Option<String>,
    pub interface: Option<DeviceInterfaceInfo>,
}

impl WebUsbDeviceInfo {
    fn from_usb_device(usb_device: &UsbDevice) -> WebUsbDeviceInfo {
        let interface = match device_interface(usb_device) {
            Ok(js_value) => deserialize_from_js::<DeviceInterfaceInfo>(js_value).ok(),
            Err(e) => {
                error!("Error getting device interface: {}", stringify_js_error(&e));
                None
            },
        };
        WebUsbDeviceInfo {
            vendor_id: usb_device.vendor_id(),
            product_id: usb_device.product_id(),
            serial_number: usb_device.serial_number(),
            interface,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceInterfaceInfo {
    pub interface_number: u8,
    pub endpoint_number: u8,
}

pub struct WebUsbWrapper {
    event_tx: WebUsbEventSender,
    /// This abort handle is needed to drop the spawned future at [`WebUsbWrapper::new`] immediately.
    _abort_handle: AbortOnDropHandle,
}

impl WebUsbWrapper {
    pub fn new() -> WebUsbResult<WebUsbWrapper> {
        let usb = get_webusb().or_mm_err(|| WebUsbError::NotSupported)?;
        let (event_tx, mut event_rx) = mpsc::unbounded();
        let fut = async move {
            while let Some(event) = event_rx.next().await {
                match event {
                    WebUsbEvent::RequestDevice { filters, result_tx } => {
                        result_tx
                            .send(WebUsbWrapper::on_request_device(&usb, filters).await)
                            .ok();
                    },
                    WebUsbEvent::GetDevices { result_tx } => {
                        result_tx.send(WebUsbWrapper::on_get_devices(&usb).await).ok();
                    },
                }
            }
        };

        let abort_handle = spawn_local_abortable(fut);
        Ok(WebUsbWrapper {
            event_tx,
            _abort_handle: abort_handle,
        })
    }

    pub async fn request_device(&self, filters: Vec<DeviceFilter>) -> WebUsbResult<()> {
        let (result_tx, result_rx) = oneshot::channel();
        send_event_recv_response(
            &self.event_tx,
            WebUsbEvent::RequestDevice { filters, result_tx },
            result_rx,
        )
        .await
    }

    pub async fn get_devices(&self) -> WebUsbResult<Vec<WebUsbDevice>> {
        let (result_tx, result_rx) = oneshot::channel();
        send_event_recv_response(&self.event_tx, WebUsbEvent::GetDevices { result_tx }, result_rx).await
    }

    async fn on_request_device(usb: &Usb, filters: Vec<DeviceFilter>) -> WebUsbResult<()> {
        let filters_js_value = serialize_to_js(&filters)
            .map_to_mm(|e| WebUsbError::Internal(format!("DeviceFilter::serialize should never fail: {e}")))?;

        let request_options = UsbDeviceRequestOptions::new(&filters_js_value);
        if let Err(e) = JsFuture::from(usb.request_device(&request_options)).await {
            return MmError::err(WebUsbError::ErrorRequestingDevice(stringify_js_error(&e)));
        }
        Ok(())
    }

    async fn on_get_devices(usb: &Usb) -> WebUsbResult<Vec<WebUsbDevice>> {
        let devices = JsFuture::from(usb.get_devices())
            .await
            .map_to_mm(|e| WebUsbError::ErrorGettingDevices(stringify_js_error(&e)))?;
        let devices_array: Array = devices.dyn_into().map_to_mm(|found| WebUsbError::TypeMismatch {
            expected: "Array".to_owned(),
            found: format!("{found:?}"),
        })?;

        let mut result_devices = Vec::with_capacity(devices_array.length() as usize);
        for device_js in devices_array.iter() {
            let device: UsbDevice = device_js.dyn_into().map_to_mm(|found| WebUsbError::TypeMismatch {
                expected: "UsbDevice".to_owned(),
                found: format!("{found:?}"),
            })?;
            let device_info = WebUsbDeviceInfo::from_usb_device(&device);

            let (device_event_tx, device_event_rx) = mpsc::unbounded();
            let abort_handle = spawn_local_abortable(WebUsbDevice::event_loop(device, device_event_rx));

            result_devices.push(WebUsbDevice {
                event_tx: device_event_tx,
                device_info,
                _abort_handle: abort_handle,
            });
        }
        Ok(result_devices)
    }
}

pub struct WebUsbDevice {
    pub event_tx: DeviceEventSender,
    pub device_info: WebUsbDeviceInfo,
    /// This abort handle is needed to drop the spawned future at [`WebUsbWrapper::on_get_devices`] immediately.
    _abort_handle: AbortOnDropHandle,
}

impl WebUsbDevice {
    pub async fn select_configuration(&self, configuration_number: u8) -> WebUsbResult<()> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = DeviceEvent::SelectConfiguration {
            configuration_number,
            result_tx,
        };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    pub async fn claim_interface(&self, interface_number: u8) -> WebUsbResult<()> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = DeviceEvent::ClaimInterface {
            interface_number,
            result_tx,
        };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    pub async fn open(&self) -> WebUsbResult<()> {
        let (result_tx, result_rx) = oneshot::channel();
        send_event_recv_response(&self.event_tx, DeviceEvent::Open { result_tx }, result_rx).await
    }

    pub async fn is_open(&self) -> WebUsbResult<bool> {
        let (result_tx, result_rx) = oneshot::channel();
        send_event_recv_response(&self.event_tx, DeviceEvent::IsOpen { result_tx }, result_rx).await
    }

    pub async fn reset_device(&self) -> WebUsbResult<()> {
        let (result_tx, result_rx) = oneshot::channel();
        send_event_recv_response(&self.event_tx, DeviceEvent::ResetDevice { result_tx }, result_rx).await
    }

    pub async fn write_chunk(&self, endpoint_number: u8, chunk: Vec<u8>) -> WebUsbResult<()> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = DeviceEvent::WriteChunk {
            endpoint_number,
            chunk,
            result_tx,
        };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    pub async fn read_chunk(&self, endpoint_number: u8, chunk_len: u32) -> WebUsbResult<Vec<u8>> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = DeviceEvent::ReadChunk {
            endpoint_number,
            chunk_len,
            result_tx,
        };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    async fn event_loop(device: UsbDevice, mut event_rx: DeviceEventReceiver) {
        while let Some(event) = event_rx.next().await {
            match event {
                DeviceEvent::SelectConfiguration {
                    configuration_number,
                    result_tx,
                } => {
                    result_tx
                        .send(WebUsbDevice::on_select_configuration(&device, configuration_number).await)
                        .ok();
                },
                DeviceEvent::ClaimInterface {
                    interface_number,
                    result_tx,
                } => {
                    result_tx
                        .send(WebUsbDevice::on_claim_interface(&device, interface_number).await)
                        .ok();
                },
                DeviceEvent::Open { result_tx } => {
                    result_tx.send(WebUsbDevice::on_open(&device).await).ok();
                },
                DeviceEvent::IsOpen { result_tx } => {
                    result_tx.send(WebUsbDevice::on_is_open(&device)).ok();
                },
                DeviceEvent::ResetDevice { result_tx } => {
                    result_tx.send(WebUsbDevice::on_reset_device(&device).await).ok();
                },
                DeviceEvent::WriteChunk {
                    endpoint_number,
                    chunk,
                    result_tx,
                } => {
                    result_tx
                        .send(
                            WebUsbDevice::on_write_chunk(&device, endpoint_number, Uint8Array::from(chunk.as_slice()))
                                .await,
                        )
                        .ok();
                },
                DeviceEvent::ReadChunk {
                    endpoint_number,
                    chunk_len,
                    result_tx,
                } => {
                    result_tx
                        .send(WebUsbDevice::on_read_chunk(&device, endpoint_number, chunk_len).await)
                        .ok();
                },
            }
        }
    }

    async fn on_select_configuration(device: &UsbDevice, configuration_number: u8) -> WebUsbResult<()> {
        JsFuture::from(device.select_configuration(configuration_number))
            .await
            .map_to_mm(|e| WebUsbError::ErrorSettingConfiguration {
                configuration_number,
                error: stringify_js_error(&e),
            })?;
        Ok(())
    }

    async fn on_claim_interface(device: &UsbDevice, interface_number: u8) -> WebUsbResult<()> {
        JsFuture::from(device.claim_interface(interface_number))
            .await
            .map_to_mm(|e| WebUsbError::ErrorClaimingInterface {
                interface_number,
                error: stringify_js_error(&e),
            })?;
        Ok(())
    }

    async fn on_open(device: &UsbDevice) -> WebUsbResult<()> {
        JsFuture::from(device.open())
            .await
            .map_to_mm(|e| WebUsbError::ErrorOpeningDevice(stringify_js_error(&e)))?;
        Ok(())
    }

    fn on_is_open(device: &UsbDevice) -> WebUsbResult<bool> {
        Ok(device.opened())
    }

    async fn on_reset_device(device: &UsbDevice) -> WebUsbResult<()> {
        JsFuture::from(device.reset())
            .await
            .map_to_mm(|e| WebUsbError::ErrorResettingDevice(stringify_js_error(&e)))?;
        Ok(())
    }

    async fn on_write_chunk(device: &UsbDevice, endpoint_number: u8, chunk: Uint8Array) -> WebUsbResult<()> {
        let data = device
            .transfer_out_with_u8_array(endpoint_number, &chunk)
            .map_to_mm(|e| WebUsbError::ErrorWritingChunk(stringify_js_error(&e)))?;
        JsFuture::from(data)
            .await
            .map_to_mm(|e| WebUsbError::ErrorWritingChunk(stringify_js_error(&e)))?;

        Ok(())
    }

    async fn on_read_chunk(device: &UsbDevice, endpoint_number: u8, chunk_len: u32) -> WebUsbResult<Vec<u8>> {
        let buffer = loop {
            let js_value = JsFuture::from(device.transfer_in(endpoint_number, chunk_len))
                .await
                .map_to_mm(|e| WebUsbError::ErrorReadingChunk(stringify_js_error(&e)))?;
            let result: UsbInTransferResult = js_value.dyn_into().map_to_mm(|found| WebUsbError::TypeMismatch {
                expected: "UsbInTransferResult".to_owned(),
                found: format!("{found:?}"),
            })?;
            let data_view = match result.data() {
                Some(data) => data,
                None => continue,
            };
            if data_view.byte_length() == 0 {
                continue;
            }
            break data_view.buffer();
        };
        let bytes = Uint8Array::new(&buffer);
        let chunk = bytes.to_vec();
        Ok(chunk)
    }
}

enum WebUsbEvent {
    RequestDevice {
        filters: Vec<DeviceFilter>,
        result_tx: EventResultSender<()>,
    },
    GetDevices {
        result_tx: EventResultSender<Vec<WebUsbDevice>>,
    },
}

pub enum DeviceEvent {
    SelectConfiguration {
        configuration_number: u8,
        result_tx: EventResultSender<()>,
    },
    ClaimInterface {
        interface_number: u8,
        result_tx: EventResultSender<()>,
    },
    Open {
        result_tx: EventResultSender<()>,
    },
    IsOpen {
        result_tx: EventResultSender<bool>,
    },
    ResetDevice {
        result_tx: EventResultSender<()>,
    },
    WriteChunk {
        endpoint_number: u8,
        chunk: Vec<u8>,
        result_tx: EventResultSender<()>,
    },
    ReadChunk {
        endpoint_number: u8,
        chunk_len: u32,
        result_tx: EventResultSender<Vec<u8>>,
    },
}

/// [Navigator::usb](https://rustwasm.github.io/wasm-bindgen/api/web_sys/struct.Navigator.html#method.usb)
/// crushes if a browser doesn't support WebUSB.
/// Use this wrapper instead.
#[wasm_bindgen(inline_js = "export function get_webusb() { return navigator.usb; }")]
extern "C" {
    fn get_webusb() -> Option<Usb>;
}

/// https://github.com/LedgerHQ/ledgerjs/blob/v6.9.0/packages/hw-transport-webusb/src/TransportWebUSB.ts#L131
#[wasm_bindgen(inline_js = r"
  export function device_interface(device) {
    const interfaces = device.configurations[0].interfaces;
    for (let i = 0; i < interfaces.length; ++i) {
        const ifaceAlt = interfaces[i].alternates.find((a) => a.interfaceClass === 255);
        if (!ifaceAlt) { continue; }
        return { interface_number: interfaces[i].interfaceNumber, endpoint_number: ifaceAlt.endpoints[0].endpointNumber }
    }
    return null;
  }
")]
extern "C" {
    #[wasm_bindgen(catch)]
    fn device_interface(device: &UsbDevice) -> Result<JsValue, JsValue>;
}
