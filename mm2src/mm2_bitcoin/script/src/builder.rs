//! Script builder

use bytes::Bytes;
use keys::{AddressHashEnum, Error, Public};
use {Num, Opcode, Script};

/// Script builder
#[derive(Default)]
pub struct Builder {
    data: Bytes,
}

impl Builder {
    /// Builds p2pkh script pubkey
    pub fn build_p2pkh(address: &AddressHashEnum) -> Script {
        Builder::default()
            .push_opcode(Opcode::OP_DUP)
            .push_opcode(Opcode::OP_HASH160)
            .push_data(&address.to_vec())
            .push_opcode(Opcode::OP_EQUALVERIFY)
            .push_opcode(Opcode::OP_CHECKSIG)
            .into_script()
    }

    /// Builds p2pk script pubkey
    pub fn build_p2pk(pubkey: &Public) -> Script {
        Builder::default()
            .push_data(pubkey)
            .push_opcode(Opcode::OP_CHECKSIG)
            .into_script()
    }

    /// Builds p2sh script pubkey
    pub fn build_p2sh(address: &AddressHashEnum) -> Script {
        Builder::default()
            .push_opcode(Opcode::OP_HASH160)
            .push_data(&address.to_vec())
            .push_opcode(Opcode::OP_EQUAL)
            .into_script()
    }

    /// Builds p2wpkh script pubkey
    pub fn build_p2wpkh(address_hash: &AddressHashEnum) -> Result<Script, Error> {
        match address_hash {
            AddressHashEnum::AddressHash(wpkh_hash) => Ok(Builder::default()
                .push_opcode(Opcode::OP_0)
                .push_data(wpkh_hash.as_ref())
                .into_script()),
            AddressHashEnum::WitnessScriptHash(_) => Err(Error::WitnessHashMismatched),
        }
    }

    /// Builds p2wsh script pubkey
    pub fn build_p2wsh(address_hash: &AddressHashEnum) -> Result<Script, Error> {
        match address_hash {
            AddressHashEnum::WitnessScriptHash(wsh_hash) => Ok(Builder::default()
                .push_opcode(Opcode::OP_0)
                .push_data(wsh_hash.as_ref())
                .into_script()),
            AddressHashEnum::AddressHash(_) => Err(Error::WitnessHashMismatched),
        }
    }

    /// Builds op_return script
    pub fn build_nulldata(bytes: &[u8]) -> Script {
        Builder::default()
            .push_opcode(Opcode::OP_RETURN)
            .push_data(bytes)
            .into_script()
    }

    /// Pushes opcode to the end of script
    pub fn push_opcode(mut self, opcode: Opcode) -> Self {
        self.data.push(opcode as u8);
        self
    }

    /// Appends bool push operation to the end of script
    pub fn push_bool(mut self, value: bool) -> Self {
        if value {
            self.data.push(Opcode::OP_1 as u8);
        } else {
            self.data.push(Opcode::OP_0 as u8);
        }
        self
    }

    /// Appends num push operation to the end of script
    pub fn push_num(self, num: Num) -> Self {
        self.push_data(&num.to_bytes())
    }

    /// Appends data push operation to the end of script
    pub fn push_data(mut self, data: &[u8]) -> Self {
        let len = data.len();
        if len < Opcode::OP_PUSHDATA1 as usize {
            self.data.push(len as u8);
        } else if len < 0x100 {
            self.data.push(Opcode::OP_PUSHDATA1 as u8);
            self.data.push(len as u8);
        } else if len < 0x10000 {
            self.data.push(Opcode::OP_PUSHDATA2 as u8);
            self.data.push(len as u8);
            self.data.push((len >> 8) as u8);
        } else if (len as i64) < 0x1_0000_0000 {
            self.data.push(Opcode::OP_PUSHDATA4 as u8);
            self.data.push(len as u8);
            self.data.push((len >> 8) as u8);
            self.data.push((len >> 16) as u8);
            self.data.push((len >> 24) as u8);
        } else {
            panic!("Cannot push more than 0x1_0000_0000 bytes");
        }

        self.data.extend_from_slice(data);
        self
    }

    /// Appends `OP_RETURN` operation to the end of script
    pub fn return_bytes(mut self, bytes: &[u8]) -> Self {
        let len = bytes.len();
        if !(1..=75).contains(&len) {
            panic!("Canot push {} bytes", len);
        }

        let opcode: Opcode = Opcode::from_u8(((Opcode::OP_PUSHBYTES_1 as usize) + len - 1) as u8)
            .expect("value is within [OP_PUSHBYTES_1; OP_PUSHBYTES_75] interval; qed");

        self.data.push(Opcode::OP_RETURN as u8);
        self.data.push(opcode as u8);
        self.data.extend_from_slice(bytes);
        self
    }

    /// Pushes invalid opcode to the end of script
    pub fn push_invalid_opcode(mut self) -> Self {
        self.data.push(0xff);
        self
    }

    /// Builds final script
    pub fn into_script(self) -> Script {
        Script::new(self.data)
    }

    /// Builds final script bytes
    pub fn into_bytes(self) -> Bytes {
        self.data
    }
}
