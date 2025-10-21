/// This source file was borrowed from Trezor repo (https://raw.githubusercontent.com/trezor/trezor-firmware/07ba960ab4aa5aa3ddf16ae74c3658782d491250/rust/trezor-client/src/transport/udp.rs)
/// and modified to integrate into this project.
/// Adds udp transport to interact with trezor emulator.
/// To build emulator use this repo: https://github.com/trezor/trezor-firmware, build with build-docker.sh for the desired branch (tag)
/// Tested with the legacy emulator (for Trezor One).
/// After building the emulator find it as ./legacy/firmware/trezor.elf file.
/// Start it (no params needed) and initialize it with Trezor Suite: create a wallet, find a receive address.
/// You need the bridge for connecting from the Suite, it can be downloaded from trezor.io.
/// Do not use pin for the created wallet.
/// Be aware that when you rebuild the firmware the emulator flash memory file emulator.img is recreated (so save it before rebuilding code)
use super::{
    protocol::{Link, Protocol, ProtocolV1},
    ProtoMessage, Transport,
};
use crate::transport::ConnectableDeviceWrapper;
use crate::{TrezorError, TrezorResult};
use async_std::{io, net::UdpSocket};
use mm2_err_handle::prelude::*;
use std::time::Duration;

// A collection of constants related to the Emulator Ports.
mod constants {
    pub(super) const DEFAULT_HOST: &str = "127.0.0.1";
    pub(super) const DEFAULT_PORT: &str = "21324";
    pub(super) const DEFAULT_DEBUG_PORT: &str = "21325";
    pub(super) const LOCAL_LISTENER: &str = "127.0.0.1:0";
}

use async_trait::async_trait;
use constants::{DEFAULT_DEBUG_PORT, DEFAULT_HOST, DEFAULT_PORT, LOCAL_LISTENER};

/// The chunk size for the serial protocol.
const CHUNK_SIZE: usize = 64;

const READ_TIMEOUT_MS: u64 = 100000;
const WRITE_TIMEOUT_MS: u64 = 100000;

/// A device found by the `find_devices()` method.  It can be connected to using the `connect()`
/// method.
pub struct UdpAvailableDevice {
    //pub model: Model,
    pub debug: bool,
    transport: UdpTransport,
}

impl UdpAvailableDevice {
    /// Connect to the device.
    async fn connect(&self) -> TrezorResult<UdpTransport> {
        let transport = UdpTransport::connect(self).await?;
        Ok(transport)
    }
}

async fn find_devices() -> TrezorResult<Vec<UdpAvailableDevice>> {
    let debug = false;
    let dest = format!(
        "{}:{}",
        DEFAULT_HOST,
        if debug { DEFAULT_DEBUG_PORT } else { DEFAULT_PORT }
    );

    let link = UdpLink::open(&dest).await?;

    if link.ping().await? {
        Ok(vec![UdpAvailableDevice {
            // model: Model::TrezorEmulator,
            debug,
            transport: UdpTransport {
                protocol: ProtocolV1 { link },
            },
        }])
    } else {
        Ok(vec![])
    }
}

/// An actual serial HID USB link to a device over which bytes can be sent.
struct UdpLink {
    pub socket: UdpSocket,
    pub device: (String, String),
}
// No need to implement drop as every member is owned

#[async_trait]
impl Link for UdpLink {
    async fn write_chunk(&mut self, chunk: Vec<u8>) -> TrezorResult<()> {
        debug_assert_eq!(CHUNK_SIZE, chunk.len());
        io::timeout(Duration::from_millis(WRITE_TIMEOUT_MS), async move {
            self.socket.send(&chunk).await
        })
        .await
        .map_to_mm(|_e| TrezorError::UnderlyingError(String::from("write timeout")))?;
        Ok(())
    }

    async fn read_chunk(&mut self, chunk_len: u32) -> TrezorResult<Vec<u8>> {
        let mut chunk = vec![0; chunk_len as usize];
        io::timeout(Duration::from_millis(READ_TIMEOUT_MS), async move {
            let n = self.socket.recv(&mut chunk).await?;
            if n == chunk_len as usize {
                Ok(chunk)
            } else {
                Err(io::Error::other("invalid read size"))
            }
        })
        .await
        .map_to_mm(|_e| TrezorError::UnderlyingError(String::from("read timeout")))
    }
}

impl UdpLink {
    async fn open(path: &str) -> TrezorResult<UdpLink> {
        let mut parts = path.split(':');
        let link = Self {
            socket: UdpSocket::bind(LOCAL_LISTENER).await?,
            device: (
                parts.next().expect("Incorrect Path").to_owned(),
                parts.next().expect("Incorrect Path").to_owned(),
            ),
        };
        link.socket.connect(path).await?;
        Ok(link)
    }

    // Ping the port and compare against expected response
    async fn ping(&self) -> TrezorResult<bool> {
        let mut resp = [0; CHUNK_SIZE];
        self.socket.send("PINGPING".as_bytes()).await?;
        let size = self.socket.recv(&mut resp).await?;
        Ok(&resp[..size] == "PONGPONG".as_bytes())
    }
}

/// An implementation of the Transport interface for UDP devices.
// #[derive(Debug)]
pub struct UdpTransport {
    protocol: ProtocolV1<UdpLink>,
}

impl UdpTransport {
    /// Connect to a device over the UDP transport.
    async fn connect(device: &UdpAvailableDevice) -> TrezorResult<UdpTransport> {
        let transport = &device.transport;
        let path = format!(
            "{}:{}",
            transport.protocol.link.device.0, transport.protocol.link.device.1
        );
        let link = UdpLink::open(&path).await?;
        Ok(UdpTransport {
            protocol: ProtocolV1 { link },
        })
    }
}

#[async_trait]
impl Transport for UdpTransport {
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

#[async_trait]
impl ConnectableDeviceWrapper for UdpAvailableDevice {
    type TransportType = UdpTransport;

    async fn find_devices() -> TrezorResult<Vec<Self>>
    where
        Self: Sized,
    {
        find_devices().await
    }

    async fn connect(&self) -> TrezorResult<Self::TransportType> {
        UdpAvailableDevice::connect(self).await
    }
}
