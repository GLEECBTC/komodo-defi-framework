//! TODO consider refactoring [`UsbDevice::connect`] not to spawn a thread for every device.
//! We can spawn it once on [`UsbContext::new`], but we have to set the read/write chunk timeout to 0.5s or smaller.

use super::{send_event_recv_response, InternalError};
use common::log::error;
use common::{block_on, some_or_return_ok_none};
use derive_more::Display;
use futures::channel::{mpsc, oneshot};
use futures::StreamExt;
use mm2_err_handle::prelude::*;
use rusb::Error as RusbError;
use rusb::UsbContext as RusbContext;
use std::thread;
use std::time::{Duration, Instant};

pub const READ_ENDPOINT_MASK: u8 = 0x80;

pub type UsbResult<T> = Result<T, MmError<UsbError>>;

type EventResultSender<T> = oneshot::Sender<UsbResult<T>>;
type DeviceEventSender = mpsc::UnboundedSender<internal::DeviceEvent>;
type DeviceEventReceiver = mpsc::UnboundedReceiver<internal::DeviceEvent>;

#[derive(Display)]
pub enum UsbError {
    DeviceDisconnected,
    ErrorInitializingSession(rusb::Error),
    ErrorGettingDevices(rusb::Error),
    ErrorOpeningDevice(rusb::Error),
    ErrorWritingChunk(rusb::Error),
    ErrorReadingChunk(rusb::Error),
    Timeout,
    Internal(String),
}

impl InternalError for UsbError {
    fn internal(e: String) -> Self {
        UsbError::Internal(e)
    }
}

#[derive(Clone)]
pub struct GetDevicesFilters {
    pub config_id: u8,
    pub interface_id: u8,
    pub interface_descriptor: u8,
    pub interface_class_code: u8,
}

pub struct UsbContext {
    context: rusb::Context,
}

impl UsbContext {
    pub fn new() -> UsbResult<UsbContext> {
        let inner = rusb::Context::new().map_to_mm(UsbError::ErrorInitializingSession)?;
        Ok(UsbContext { context: inner })
    }

    pub fn get_devices(&self, filters: GetDevicesFilters) -> UsbResult<Vec<UsbAvailableDevice>> {
        let found_devices = self.devices()?;
        let mut devices = Vec::with_capacity(found_devices.len());
        for device in found_devices.iter() {
            let device_descriptor = match device.device_descriptor() {
                Ok(descr) => descr,
                Err(e) => {
                    error!("Error getting device descriptor: {}", e);
                    continue;
                },
            };
            let interface_info = match Self::device_interface_by_filters(&device, &device_descriptor, &filters) {
                Ok(Some(interface_info)) => interface_info,
                Ok(None) => continue,
                Err(e) => {
                    error!("Error checking device interface: {}", e);
                    continue;
                },
            };
            let device_info = UsbDeviceInfo {
                vendor_id: device_descriptor.vendor_id(),
                product_id: device_descriptor.product_id(),
                bus: device.bus_number(),
                address: device.address(),
                interface_info,
            };
            devices.push(UsbAvailableDevice { device, device_info });
        }
        Ok(devices)
    }

    /// `libusb_get_device_list` is a blocking function, but it's blocking time is negligible on many OS.
    fn devices(&self) -> UsbResult<rusb::DeviceList<rusb::Context>> {
        self.context.devices().map_to_mm(UsbError::ErrorGettingDevices)
    }

    fn device_interface_by_filters(
        device: &rusb::Device<rusb::Context>,
        device_descriptor: &rusb::DeviceDescriptor,
        filters: &GetDevicesFilters,
    ) -> UsbResult<Option<UsbDeviceInterfaceInfo>> {
        if device_descriptor.num_configurations() <= filters.config_id {
            return Ok(None);
        }
        let config_descriptor = device
            .config_descriptor(filters.config_id)
            .map_to_mm(UsbError::ErrorGettingDevices)?;
        let interface = some_or_return_ok_none!(config_descriptor
            .interfaces()
            .find(|i| i.number() == filters.interface_id));
        let descriptor = some_or_return_ok_none!(interface
            .descriptors()
            .find(|d| d.setting_number() == filters.interface_descriptor));
        if descriptor.class_code() != filters.interface_class_code {
            return Ok(None);
        }
        // Get the first endpoint.
        let endpoint = some_or_return_ok_none!(descriptor.endpoint_descriptors().next());
        Ok(Some(UsbDeviceInterfaceInfo {
            interface_number: filters.interface_id,
            endpoint_number: endpoint.number(),
        }))
    }
}

/// An available transport for connecting with a device.
pub struct UsbAvailableDevice {
    device: rusb::Device<rusb::Context>,
    device_info: UsbDeviceInfo,
}

impl UsbAvailableDevice {
    pub fn connect(&self) -> UsbResult<UsbDevice> {
        // This is a non-blocking function; no requests are sent over the bus.
        let mut device_handle = self.device.open().map_to_mm(UsbError::ErrorOpeningDevice)?;
        // Claiming of interfaces is a purely logical operation.
        // It does not cause any requests to be sent over the bus.
        // Interface claiming is used to instruct the underlying operating system that your application wishes to take ownership of the interface.
        device_handle
            .claim_interface(self.device_info.interface_info.interface_number)
            .map_to_mm(UsbError::ErrorOpeningDevice)?;

        let (event_tx, event_rx) = mpsc::unbounded();
        let thread_handle = thread::spawn(move || Self::event_loop(event_rx, device_handle));
        Ok(UsbDevice {
            event_tx,
            device_info: self.device_info,
            thread_handle,
        })
    }

    pub fn device_info(&self) -> &UsbDeviceInfo {
        &self.device_info
    }

    fn event_loop(mut event_rx: DeviceEventReceiver, device_handle: rusb::DeviceHandle<rusb::Context>) {
        while let Some(event) = block_on(event_rx.next()) {
            match event {
                internal::DeviceEvent::WriteChunk {
                    endpoint_number,
                    chunk,
                    timeout,
                    result_tx,
                } => {
                    result_tx
                        .send(Self::on_write_chunk(&device_handle, chunk, endpoint_number, timeout))
                        .ok();
                },
                internal::DeviceEvent::ReadChunk {
                    endpoint_number,
                    chunk_len,
                    timeout,
                    result_tx,
                } => {
                    result_tx
                        .send(Self::on_read_chunk(&device_handle, endpoint_number, chunk_len, timeout))
                        .ok();
                },
            }
        }
    }

    fn on_write_chunk(
        device_handle: &rusb::DeviceHandle<rusb::Context>,
        chunk: Vec<u8>,
        endpoint: u8,
        mut timeout: Duration,
    ) -> UsbResult<()> {
        if chunk.is_empty() {
            return MmError::err(UsbError::ErrorWritingChunk(RusbError::InvalidParam));
        }

        let mut sent_bytes = 0;
        loop {
            let started_at = Instant::now();

            // If an error variant is returned, no bytes were written.
            sent_bytes += device_handle
                .write_interrupt(endpoint, &chunk[sent_bytes..], timeout)
                .map_to_mm(|e| match e {
                    RusbError::NoDevice => UsbError::DeviceDisconnected,
                    RusbError::Timeout => UsbError::Timeout,
                    e => UsbError::ErrorWritingChunk(e),
                })?;

            let ended_at = Instant::now();
            if sent_bytes == chunk.len() {
                return Ok(());
            }
            // If the timeout has not expired yet, try to continue writing the chunk.
            timeout = match timeout.checked_sub(ended_at - started_at) {
                Some(time_left) => time_left,
                None => return MmError::err(UsbError::Timeout),
            };
        }
    }

    fn on_read_chunk(
        device_handle: &rusb::DeviceHandle<rusb::Context>,
        endpoint: u8,
        chunk_len: usize,
        mut timeout: Duration,
    ) -> UsbResult<Vec<u8>> {
        let mut chunk = vec![0; chunk_len];

        loop {
            let started_at = Instant::now();

            // If an error variant is returned, no bytes were written.
            let read_bytes = device_handle
                .read_interrupt(endpoint, &mut chunk, timeout)
                .map_to_mm(|e| match e {
                    RusbError::NoDevice => UsbError::DeviceDisconnected,
                    RusbError::Timeout => UsbError::Timeout,
                    e => UsbError::ErrorReadingChunk(e),
                })?;

            let ended_at = Instant::now();
            if read_bytes != 0 {
                return Ok(chunk[0..read_bytes].to_vec());
            }

            // If the timeout has not expired yet, try to continue reading the chunk.
            timeout = match timeout.checked_sub(ended_at - started_at) {
                Some(time_left) => time_left,
                None => return MmError::err(UsbError::Timeout),
            };
        }
    }
}

pub struct UsbDevice {
    event_tx: DeviceEventSender,
    device_info: UsbDeviceInfo,
    #[allow(dead_code)]
    thread_handle: thread::JoinHandle<()>,
}

impl UsbDevice {
    pub async fn write_chunk(&self, chunk: Vec<u8>, timeout: Duration) -> UsbResult<()> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DeviceEvent::WriteChunk {
            endpoint_number: self.endpoint_number(),
            chunk,
            timeout,
            result_tx,
        };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    pub async fn read_chunk(&self, chunk_len: usize, timeout: Duration) -> UsbResult<Vec<u8>> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DeviceEvent::ReadChunk {
            endpoint_number: READ_ENDPOINT_MASK | self.endpoint_number(),
            chunk_len,
            timeout,
            result_tx,
        };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    fn endpoint_number(&self) -> u8 {
        self.device_info.interface_info.endpoint_number
    }
}

#[derive(Clone, Copy, Debug)]
pub struct UsbDeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub bus: u8,
    pub address: u8,
    pub interface_info: UsbDeviceInterfaceInfo,
}

#[derive(Clone, Copy, Debug)]
pub struct UsbDeviceInterfaceInfo {
    pub interface_number: u8,
    pub endpoint_number: u8,
}

mod internal {
    use super::*;

    pub(super) enum DeviceEvent {
        WriteChunk {
            endpoint_number: u8,
            chunk: Vec<u8>,
            timeout: Duration,
            result_tx: EventResultSender<()>,
        },
        ReadChunk {
            endpoint_number: u8,
            chunk_len: usize,
            timeout: Duration,
            result_tx: EventResultSender<Vec<u8>>,
        },
    }
}
