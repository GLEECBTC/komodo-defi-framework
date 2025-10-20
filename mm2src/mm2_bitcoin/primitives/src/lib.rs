#![expect(clippy::assign_op_pattern)]
#![expect(clippy::ptr_offset_with_cast)]
#![expect(clippy::manual_div_ceil)]

extern crate bitcoin_hashes;
extern crate byteorder;
extern crate rustc_hex as hex;
extern crate uint;

use uint::construct_uint;

construct_uint! {
    pub struct U256(4);
}

pub mod bytes;
pub mod compact;
pub mod hash;
