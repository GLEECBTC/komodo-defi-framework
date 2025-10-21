use crate::proto::ProtoMessage;
use crate::TrezorResult;
use async_trait::async_trait;
use rand::RngCore;

mod protocol;
#[cfg(all(feature = "trezor-udp", not(target_arch = "wasm32"), not(target_os = "ios")))]
pub mod udp;
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "ios")))]
pub mod usb;
#[cfg(target_arch = "wasm32")]
pub mod webusb;

pub const TREZOR_DEVICES: [TrezorDevice; 3] = [
    // TREZOR v1
    // won't get opened, but we can show error at least
    TrezorDevice::new(0x534c, 0x0001),
    // TREZOR webusb Bootloader
    TrezorDevice::new(0x1209, 0x53c0),
    // TREZOR webusb Firmware
    TrezorDevice::new(0x1209, 0x53c1),
];

#[derive(Clone, Copy)]
pub struct TrezorDevice {
    pub vendor_id: u16,
    pub product_id: u16,
}

impl TrezorDevice {
    const fn new(vendor_id: u16, product_id: u16) -> TrezorDevice {
        TrezorDevice { vendor_id, product_id }
    }
}

/// The transport interface that is implemented by the different ways to communicate with a Trezor
/// device.
#[async_trait]
pub trait Transport {
    async fn session_begin(&mut self) -> TrezorResult<()>;
    async fn session_end(&mut self) -> TrezorResult<()>;

    async fn write_message(&mut self, message: ProtoMessage) -> TrezorResult<()>;
    async fn read_message(&mut self) -> TrezorResult<ProtoMessage>;

    #[cfg(target_arch = "wasm32")]
    async fn is_connected(&mut self) -> TrezorResult<bool>;
}

/// The Trezor session identifier.
/// https://docs.trezor.io/trezor-firmware/common/communication/sessions.html#session-lifecycle
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct SessionId([u8; 32]);

impl Default for SessionId {
    fn default() -> Self {
        SessionId::new()
    }
}

impl SessionId {
    /// Generate a new random `SessionId`.
    pub fn new() -> SessionId {
        let mut rng = rand::thread_rng();

        let mut bytes = [0; 32];
        rng.fill_bytes(&mut bytes);
        SessionId(bytes)
    }
}

impl AsRef<[u8]> for SessionId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Wrapper to abstract connectivity to usb and emulator devices
#[async_trait]
pub trait ConnectableDeviceWrapper {
    type TransportType: Transport + Sync + Send;

    async fn find_devices() -> TrezorResult<Vec<Self>>
    where
        Self: Sized;

    async fn connect(&self) -> TrezorResult<Self::TransportType>;
}
