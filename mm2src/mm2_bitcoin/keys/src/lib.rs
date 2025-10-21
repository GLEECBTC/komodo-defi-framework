//! Bitcoin keys.

extern crate bech32;
extern crate bitcrypto as crypto;
extern crate bs58;
extern crate derive_more;
extern crate lazy_static;
extern crate primitives;
extern crate rustc_hex as hex;
extern crate secp256k1;
extern crate serde;
#[macro_use]
extern crate serde_derive;

mod address;
mod address_prefixes;
mod cashaddress;
mod display;
mod error;
mod keypair;
mod legacyaddress;
mod network;
mod private;
mod public;
mod segwitaddress;
mod signature;

pub use primitives::{bytes, hash};

pub use address::{Address, AddressBuilder, AddressBuilderOption, AddressFormat, AddressScriptType};
pub use address_prefixes::prefixes;
pub use address_prefixes::{AddressPrefix, NetworkAddressPrefixes};
pub use cashaddress::{CashAddrType, CashAddress, NetworkPrefix};
pub use display::DisplayLayout;
pub use error::Error;
pub use keypair::KeyPair;
pub use legacyaddress::LegacyAddress;
pub use network::Network;
pub use private::Private;
pub use public::Public;
pub use segwitaddress::SegwitAddress;
pub use signature::{CompactSignature, Signature};

use hash::{H160, H256};
use lazy_static::lazy_static;
use secp256k1::{Secp256k1, SignOnly, VerifyOnly};
use std::fmt;

/// 32 bytes long secret key
pub type Secret = H256;
/// 32 bytes long signable message
pub type Message = H256;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum AddressHashEnum {
    /// 20 bytes long hash derived from public `ripemd160(sha256(public/script))` used in P2PKH, P2SH, P2WPKH
    AddressHash(H160),
    /// 32 bytes long hash derived from script `sha256(script)` used in P2WSH
    WitnessScriptHash(H256),
}

impl AddressHashEnum {
    pub fn default_address_hash() -> Self {
        AddressHashEnum::AddressHash(H160::default())
    }

    pub fn default_witness_script_hash() -> Self {
        AddressHashEnum::WitnessScriptHash(H256::default())
    }

    pub fn copy_from_slice(&mut self, src: &[u8]) {
        match self {
            AddressHashEnum::AddressHash(h) => h.copy_from_slice(src),
            AddressHashEnum::WitnessScriptHash(s) => s.copy_from_slice(src),
        }
    }

    pub fn to_vec(&self) -> Vec<u8> {
        match self {
            AddressHashEnum::AddressHash(h) => h.to_vec(),
            AddressHashEnum::WitnessScriptHash(s) => s.to_vec(),
        }
    }

    pub fn is_address_hash(&self) -> bool {
        matches!(*self, AddressHashEnum::AddressHash(_))
    }

    pub fn is_witness_script_hash(&self) -> bool {
        matches!(*self, AddressHashEnum::WitnessScriptHash(_))
    }
}

impl fmt::Display for AddressHashEnum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AddressHashEnum::AddressHash(h) => f.write_str(&h.to_string()),
            AddressHashEnum::WitnessScriptHash(s) => f.write_str(&s.to_string()),
        }
    }
}

impl From<H160> for AddressHashEnum {
    fn from(hash: H160) -> Self {
        AddressHashEnum::AddressHash(hash)
    }
}

lazy_static! {
    pub static ref SECP_VERIFY: Secp256k1<VerifyOnly> = Secp256k1::verification_only();
    pub static ref SECP_SIGN: Secp256k1<SignOnly> = Secp256k1::signing_only();
}
