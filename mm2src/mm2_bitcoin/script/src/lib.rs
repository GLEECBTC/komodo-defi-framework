#[cfg(not(target_arch = "wasm32"))]
extern crate bitcoin as ext_bitcoin;
extern crate bitcrypto as crypto;
extern crate blake2b_simd;
extern crate chain;
extern crate keys;
extern crate log;
extern crate primitives;
extern crate serde;
extern crate serialization as ser;

mod builder;
mod error;
mod flags;
mod num;
mod opcode;
mod script;
mod sign;
mod stack;
mod verify;

pub use primitives::{bytes, hash};

pub use self::builder::Builder;
pub use self::error::Error;
pub use self::flags::VerificationFlags;
pub use self::num::Num;
pub use self::opcode::Opcode;
pub use self::script::{is_witness_commitment_script, Script, ScriptAddress, ScriptType, ScriptWitness};
pub use self::sign::{SignatureVersion, SignerHashAlgo, TransactionInputSigner, UnsignedTransactionInput};
pub use self::stack::Stack;
pub use self::verify::{NoopSignatureChecker, SignatureChecker, TransactionSignatureChecker};
