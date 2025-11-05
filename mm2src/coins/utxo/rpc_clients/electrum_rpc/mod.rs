use sha2::{Digest, Sha256};

mod client;
mod connection;
mod connection_manager;
mod constants;
mod event_handlers;
mod rpc_responses;
#[cfg(not(target_arch = "wasm32"))]
mod tcp_stream;

pub use client::{ElectrumClient, ElectrumClientImpl, ElectrumClientSettings};
pub use connection::ElectrumConnectionSettings;
pub use constants::*;
pub use rpc_responses::*;

#[inline]
pub fn electrum_script_hash(script: &[u8]) -> Vec<u8> {
    let mut sha = Sha256::new();
    sha.update(script);
    sha.finalize().iter().rev().copied().collect()
}
