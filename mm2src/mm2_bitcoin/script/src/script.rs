//! Serialized script, used inside transaction inputs and outputs.

use bytes::Bytes;
use keys::{self, AddressHashEnum, Public};
use std::convert::TryInto;
use std::{fmt, ops};
use {Error, Opcode};

/// Maximum number of public keys per multisig
pub const MAX_PUBKEYS_PER_MULTISIG: usize = 20;

/// Classified script type
#[derive(PartialEq, Debug)]
pub enum ScriptType {
    NonStandard,
    PubKey,
    PubKeyHash,
    ScriptHash,
    Multisig,
    NullData,
    WitnessScript,
    WitnessKey,
    Taproot,
    // Qtum specific
    CallSender,
    CreateSender,
    Call,
    Create,
    ColdStaking,
    // Komodo smart chains specific
    CryptoCondition,
}

/// Address from Script
#[derive(PartialEq, Debug)]
pub struct ScriptAddress {
    /// The type of the address.
    pub kind: keys::AddressScriptType,
    /// Public key hash.
    pub hash: AddressHashEnum,
}

impl ScriptAddress {
    /// Creates P2PKH-type ScriptAddress
    pub fn new_p2pkh(hash: AddressHashEnum) -> Self {
        ScriptAddress {
            kind: keys::AddressScriptType::P2PKH,
            hash,
        }
    }

    /// Creates P2SH-type ScriptAddress
    pub fn new_p2sh(hash: AddressHashEnum) -> Self {
        ScriptAddress {
            kind: keys::AddressScriptType::P2SH,
            hash,
        }
    }

    /// Creates P2WPKH-type ScriptAddress
    pub fn new_p2wpkh(hash: AddressHashEnum) -> Self {
        ScriptAddress {
            kind: keys::AddressScriptType::P2WPKH,
            hash,
        }
    }

    /// Creates P2WSH-type ScriptAddress
    pub fn new_p2wsh(hash: AddressHashEnum) -> Self {
        ScriptAddress {
            kind: keys::AddressScriptType::P2WSH,
            hash,
        }
    }
}

/// Serialized script, used inside transaction inputs and outputs.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Script {
    data: Bytes,
}

impl From<&'static str> for Script {
    fn from(s: &'static str) -> Self {
        Script::new(s.into())
    }
}

impl From<Bytes> for Script {
    fn from(s: Bytes) -> Self {
        Script::new(s)
    }
}

impl From<Vec<u8>> for Script {
    fn from(v: Vec<u8>) -> Self {
        Script::new(v.into())
    }
}

impl From<Script> for Bytes {
    fn from(script: Script) -> Self {
        script.data
    }
}

impl Script {
    /// Script constructor.
    pub fn new(data: Bytes) -> Self {
        Script { data }
    }

    pub fn to_bytes(&self) -> Bytes {
        self.data.clone()
    }

    pub fn as_slice(&self) -> &[u8] {
        self.data.as_slice()
    }

    /// Is empty script
    pub fn is_empty(&self) -> bool {
        self.data.len() == 0
    }

    /// Extra-fast test for pay-to-public-key-hash (P2PKH) scripts.
    pub fn is_pay_to_public_key_hash(&self) -> bool {
        self.data.len() == 25
            && self.data[0] == Opcode::OP_DUP as u8
            && self.data[1] == Opcode::OP_HASH160 as u8
            && self.data[2] == Opcode::OP_PUSHBYTES_20 as u8
            && self.data[23] == Opcode::OP_EQUALVERIFY as u8
            && self.data[24] == Opcode::OP_CHECKSIG as u8
    }

    /// Extra-fast test for pay-to-public-key (P2PK) scripts.
    pub fn is_pay_to_public_key(&self) -> bool {
        if self.data.is_empty() {
            return false;
        }

        let len = match self.data[0] {
            x if x == Opcode::OP_PUSHBYTES_33 as u8 => 35,
            x if x == Opcode::OP_PUSHBYTES_65 as u8 => 67,
            _ => return false,
        };

        self.data.len() == len && self.data[len - 1] == Opcode::OP_CHECKSIG as u8
    }

    /// Extra-fast test for pay-to-script-hash (P2SH) scripts.
    pub fn is_pay_to_script_hash(&self) -> bool {
        self.data.len() == 23
            && self.data[0] == Opcode::OP_HASH160 as u8
            && self.data[1] == Opcode::OP_PUSHBYTES_20 as u8
            && self.data[22] == Opcode::OP_EQUAL as u8
    }

    /// Extra-fast test for pay-to-witness-key-hash scripts.
    pub fn is_pay_to_witness_key_hash(&self) -> bool {
        self.data.len() == 22 && self.data[0] == Opcode::OP_0 as u8 && self.data[1] == Opcode::OP_PUSHBYTES_20 as u8
    }

    /// Parse witness program. Returns Some(witness program version, code) or None if not a witness program.
    pub fn parse_witness_program(&self) -> Option<(u8, &[u8])> {
        if self.data.len() < 4 || self.data.len() > 42 || self.data.len() != self.data[1] as usize + 2 {
            return None;
        }
        let witness_version = match Opcode::from_u8(self.data[0]) {
            Some(Opcode::OP_0) => 0,
            Some(x) if x >= Opcode::OP_1 && x <= Opcode::OP_16 => (x as u8) - (Opcode::OP_1 as u8) + 1,
            _ => return None,
        };
        let witness_program = &self.data[2..];
        Some((witness_version, witness_program))
    }

    /// Extra-fast test for pay-to-witness-script-hash scripts.
    pub fn is_pay_to_witness_script_hash(&self) -> bool {
        self.data.len() == 34 && self.data[0] == Opcode::OP_0 as u8 && self.data[1] == Opcode::OP_PUSHBYTES_32 as u8
    }

    /// Extra-fast test for multisig scripts.
    pub fn is_multisig_script(&self) -> bool {
        if self.data.len() < 3 {
            return false;
        }

        let siglen = match self.get_opcode(0) {
            Ok(Opcode::OP_0) => 0,
            Ok(o) if o >= Opcode::OP_1 && o <= Opcode::OP_16 => o as u8 - (Opcode::OP_1 as u8 - 1),
            _ => return false,
        };

        let keylen = match self.get_opcode(self.data.len() - 2) {
            Ok(Opcode::OP_0) => 0,
            Ok(o) if o >= Opcode::OP_1 && o <= Opcode::OP_16 => o as u8 - (Opcode::OP_1 as u8 - 1),
            _ => return false,
        };

        if siglen > keylen {
            return false;
        }

        if self.data[self.data.len() - 1] != Opcode::OP_CHECKMULTISIG as u8 {
            return false;
        }

        let mut pc = 1;
        let mut keys = 0;
        while pc < self.len() - 2 {
            let instruction = match self.get_instruction_at(pc) {
                Ok(i) => i,
                _ => return false,
            };

            match instruction.opcode {
                Opcode::OP_PUSHBYTES_33 | Opcode::OP_PUSHBYTES_65 => keys += 1,
                _ => return false,
            }

            pc += instruction.step;
        }

        keys == keylen
    }

    pub fn is_null_data_script(&self) -> bool {
        // TODO: optimise it
        !self.data.is_empty() && self.data[0] == Opcode::OP_RETURN as u8 && self.subscript(1).is_push_only()
    }

    pub fn subscript(&self, from: usize) -> Script {
        self.data[from..].to_vec().into()
    }

    pub fn find_and_delete(&self, data: &[u8]) -> Script {
        let mut result = Vec::new();
        let mut current = 0;
        let len = data.len();
        let end = self.data.len();

        if len > end || len == 0 {
            return self.data.to_vec().into();
        }

        while current < end - len {
            if &self.data[current..current + len] != data {
                result.push(self.data[current]);
                current += 1;
            } else {
                current += len;
            }
        }

        result.extend_from_slice(&self.data[current..]);
        result.into()
    }

    pub fn get_opcode(&self, position: usize) -> Result<Opcode, Error> {
        Opcode::from_u8(self.data[position]).ok_or(Error::BadOpcode)
    }

    pub fn get_instruction(&self, index: usize) -> Option<Result<Instruction<'_>, Error>> {
        self.iter()
            .enumerate()
            .find_map(|(idx, instr)| if idx == index { Some(instr) } else { None })
    }

    pub fn get_instruction_at(&self, position: usize) -> Result<Instruction<'_>, Error> {
        let opcode = self.get_opcode(position)?;
        let instruction = match opcode {
            Opcode::OP_PUSHDATA1 | Opcode::OP_PUSHDATA2 | Opcode::OP_PUSHDATA4 => {
                let len = match opcode {
                    Opcode::OP_PUSHDATA1 => 1,
                    Opcode::OP_PUSHDATA2 => 2,
                    _ => 4,
                };

                let slice = self.take(position + 1, len)?;
                let n = read_usize(slice, len)?;
                let bytes = self.take(position + 1 + len, n)?;
                Instruction {
                    opcode,
                    step: len + n + 1,
                    data: Some(bytes),
                }
            },
            o if o <= Opcode::OP_PUSHBYTES_75 => {
                let bytes = self.take(position + 1, opcode as usize)?;
                Instruction {
                    opcode: o,
                    step: opcode as usize + 1,
                    data: Some(bytes),
                }
            },
            _ => Instruction {
                opcode,
                step: 1,
                data: None,
            },
        };

        Ok(instruction)
    }

    #[inline]
    pub fn take(&self, offset: usize, len: usize) -> Result<&[u8], Error> {
        if offset + len > self.data.len() {
            Err(Error::BadOpcode)
        } else {
            Ok(&self.data[offset..offset + len])
        }
    }

    /// Returns Script without OP_CODESEPARATOR opcodes
    pub fn without_separators(&self) -> Script {
        let mut pc = 0;
        let mut result = Vec::new();

        while pc < self.len() {
            match self.get_instruction_at(pc) {
                Ok(instruction) => {
                    if instruction.opcode != Opcode::OP_CODESEPARATOR {
                        result.extend_from_slice(&self[pc..pc + instruction.step]);
                    }

                    pc += instruction.step;
                },
                _ => {
                    result.push(self[pc]);
                    pc += 1;
                },
            }
        }

        result.into()
    }

    /// Returns true if script contains only push opcodes
    pub fn is_push_only(&self) -> bool {
        let mut pc = 0;
        while pc < self.len() {
            let instruction = match self.get_instruction_at(pc) {
                Ok(i) => i,
                _ => return false,
            };

            if instruction.opcode > Opcode::OP_16 {
                return false;
            }

            pc += instruction.step;
        }
        true
    }

    pub fn script_type(&self) -> ScriptType {
        if self.is_pay_to_public_key() {
            ScriptType::PubKey
        } else if self.is_pay_to_public_key_hash() {
            ScriptType::PubKeyHash
        } else if self.is_pay_to_script_hash() {
            ScriptType::ScriptHash
        } else if self.is_multisig_script() {
            ScriptType::Multisig
        } else if self.is_null_data_script() {
            ScriptType::NullData
        } else if self.is_pay_to_witness_key_hash() {
            ScriptType::WitnessKey
        } else if self.is_pay_to_witness_script_hash() {
            ScriptType::WitnessScript
            // TODO add Call
        } else {
            ScriptType::NonStandard
        }
    }

    pub fn iter(&self) -> Instructions<'_> {
        Instructions {
            position: 0,
            script: self,
        }
    }

    pub fn opcodes(&self) -> Opcodes<'_> {
        Opcodes {
            position: 0,
            script: self,
        }
    }

    pub fn sigops_count(&self, serialized_script: bool) -> usize {
        let mut last_opcode = Opcode::OP_0;
        let mut total = 0;
        for opcode in self.opcodes() {
            let opcode = match opcode {
                Ok(opcode) => opcode,
                // If we push an invalid element, all previous CHECKSIGs are counted
                _ => return total,
            };

            match opcode {
                Opcode::OP_CHECKSIG | Opcode::OP_CHECKSIGVERIFY => {
                    total += 1;
                },
                Opcode::OP_CHECKMULTISIG | Opcode::OP_CHECKMULTISIGVERIFY => {
                    if serialized_script && last_opcode.is_within_op_n() {
                        total += last_opcode.decode_op_n() as usize;
                    } else {
                        total += MAX_PUBKEYS_PER_MULTISIG;
                    }
                },
                _ => (),
            };

            last_opcode = opcode;
        }

        total
    }

    pub fn num_signatures_required(&self) -> u8 {
        if self.is_multisig_script() {
            return match self.data[0] {
                x if x == Opcode::OP_0 as u8 => 0,
                x => x - (Opcode::OP_1 as u8) + 1,
            };
        }
        1
    }

    pub fn extract_destinations(&self) -> Result<Vec<ScriptAddress>, keys::Error> {
        match self.script_type() {
            ScriptType::NonStandard => Ok(vec![]),
            ScriptType::PubKey => {
                Public::from_slice(match self.data[0] {
                    x if x == Opcode::OP_PUSHBYTES_33 as u8 => &self.data[1..34],
                    x if x == Opcode::OP_PUSHBYTES_65 as u8 => &self.data[1..66],
                    _ => unreachable!(), // because we are relying on script_type() checks here
                })
                .map(|public| {
                    vec![ScriptAddress::new_p2pkh(AddressHashEnum::AddressHash(
                        public.address_hash(),
                    ))]
                })
            },
            ScriptType::PubKeyHash => {
                let bytes = self.data.get(3..23).ok_or(keys::Error::InvalidAddress)?;
                let hash: [u8; 20] = bytes.try_into().map_err(|_| keys::Error::InvalidAddress)?;
                let address_hash = AddressHashEnum::AddressHash(hash.into());
                Ok(vec![ScriptAddress::new_p2pkh(address_hash)])
            },
            ScriptType::ScriptHash => {
                let bytes = self.data.get(2..22).ok_or(keys::Error::InvalidAddress)?;
                let hash: [u8; 20] = bytes.try_into().map_err(|_| keys::Error::InvalidAddress)?;
                let address_hash = AddressHashEnum::AddressHash(hash.into());
                Ok(vec![ScriptAddress::new_p2sh(address_hash)])
            },
            ScriptType::Multisig => {
                let mut addresses: Vec<ScriptAddress> = Vec::new();
                let mut pc = 1;
                while pc < self.len() - 2 {
                    let instruction = self
                        .get_instruction_at(pc)
                        .expect("this method depends on previous check in script_type()");
                    let data = instruction
                        .data
                        .expect("this method depends on previous check in script_type()");
                    let address = Public::from_slice(data)?.address_hash();
                    addresses.push(ScriptAddress::new_p2pkh(AddressHashEnum::AddressHash(address)));
                    pc += instruction.step;
                }
                Ok(addresses)
            },
            ScriptType::NullData => Ok(vec![]),
            ScriptType::WitnessScript => {
                let bytes = self.data.get(2..34).ok_or(keys::Error::InvalidAddress)?;
                let hash: [u8; 32] = bytes.try_into().map_err(|_| keys::Error::InvalidAddress)?;
                let address_hash = AddressHashEnum::WitnessScriptHash(hash.into());
                Ok(vec![ScriptAddress::new_p2wsh(address_hash)])
            },
            ScriptType::WitnessKey => {
                let bytes = self.data.get(2..22).ok_or(keys::Error::InvalidAddress)?;
                let hash: [u8; 20] = bytes.try_into().map_err(|_| keys::Error::InvalidAddress)?;
                let address_hash = AddressHashEnum::AddressHash(hash.into());
                Ok(vec![ScriptAddress::new_p2wpkh(address_hash)])
            },
            ScriptType::Taproot => {
                Ok(vec![]) // TODO
            },
            ScriptType::CallSender => {
                Ok(vec![]) // TODO
            },
            ScriptType::CreateSender => {
                Ok(vec![]) // TODO
            },
            ScriptType::Call => {
                Ok(vec![]) // TODO
            },
            ScriptType::Create => {
                Ok(vec![]) // TODO
            },
            ScriptType::ColdStaking => {
                Ok(vec![]) // TODO
            },
            ScriptType::CryptoCondition => {
                Ok(vec![]) // TODO
            },
        }
    }

    pub fn pay_to_script_hash_sigops(&self, prev_out: &Script) -> usize {
        if !prev_out.is_pay_to_script_hash() {
            return 0;
        }

        if self.data.is_empty() || !self.is_push_only() {
            return 0;
        }

        let script: Script = self
            .iter()
            .last()
            .expect("self.data.is_empty() == false; qed")
            .expect("self.data.is_push_only()")
            .data
            .expect("self.data.is_push_only()")
            .to_vec()
            .into();

        script.sigops_count(true)
    }

    /// Extracts the signature from a scriptSig at instruction 0.
    ///
    /// Usable for P2PK and P2PKH scripts.
    pub fn extract_signature(&self) -> Result<Vec<u8>, String> {
        match self.get_instruction(0) {
            Some(Ok(instruction)) => match instruction.data {
                Some(bytes) if !bytes.is_empty() => Ok(bytes.to_vec()),
                Some(_) | None => Err(format!("No data at instruction 0 of script {self:?}")),
            },
            Some(Err(e)) => Err(format!("Error {e} on getting instruction 0 of script {self:?}")),
            None => Err(format!("None instruction 0 of script {self:?}")),
        }
    }

    /// Checks if a scriptSig is a script that spends a P2PK output.
    pub fn does_script_spend_p2pk(&self) -> bool {
        // P2PK scriptSig is just a single signature. The script should consist of a single push bytes
        // instruction with the data as the signature.
        self.extract_signature().is_ok() && self.get_instruction(1).is_none()
    }
}

pub struct Instructions<'a> {
    position: usize,
    script: &'a Script,
}

pub struct Opcodes<'a> {
    position: usize,
    script: &'a Script,
}

impl<'a> Iterator for Instructions<'a> {
    type Item = Result<Instruction<'a>, Error>;

    fn next(&mut self) -> Option<Result<Instruction<'a>, Error>> {
        if self.script.len() <= self.position {
            return None;
        }

        let instruction = match self.script.get_instruction_at(self.position) {
            Ok(x) => x,
            Err(e) => {
                self.position = self.script.len();
                return Some(Err(e));
            },
        };

        self.position += instruction.step;

        Some(Ok(instruction))
    }
}

impl Iterator for Opcodes<'_> {
    type Item = Result<Opcode, Error>;

    fn next(&mut self) -> Option<Result<Opcode, Error>> {
        if self.script.len() <= self.position {
            return None;
        }

        let instruction = match self.script.get_instruction_at(self.position) {
            Ok(x) => x,
            Err(e) => return Some(Err(e)),
        };

        self.position += instruction.step;

        Some(Ok(instruction.opcode))
    }
}

impl ops::Deref for Script {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

pub struct Instruction<'a> {
    pub opcode: Opcode,
    pub step: usize,
    pub data: Option<&'a [u8]>,
}

fn read_usize(data: &[u8], size: usize) -> Result<usize, Error> {
    if data.len() < size {
        return Err(Error::BadOpcode);
    }

    let result = data
        .iter()
        .take(size)
        .enumerate()
        .fold(0, |acc, (i, x)| acc + ((*x as usize) << (i * 8)));
    Ok(result)
}

impl fmt::Display for Script {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut pc = 0;

        while pc < self.len() {
            let instruction = match self.get_instruction_at(pc) {
                Ok(i) => i,
                Err(e) => return e.fmt(f),
            };

            match instruction.data {
                Some(data) => writeln!(f, "{:?} 0x{:?}", instruction.opcode, Bytes::from(data.to_vec()))?,
                None => writeln!(f, "{:?}", instruction.opcode)?,
            }

            pc += instruction.step;
        }

        Ok(())
    }
}

pub type ScriptWitness = Vec<Bytes>;

/// Passed bytes array is a commitment script?
/// https://github.com/bitcoin/bips/blob/master/bip-0141.mediawiki#Commitment_structure
pub fn is_witness_commitment_script(script: &[u8]) -> bool {
    script.len() >= 36
        && script[0] == Opcode::OP_RETURN as u8
        && script[1] == 0x24
        && script[2] == 0xAA
        && script[3] == 0x21
        && script[4] == 0xA9
        && script[5] == 0xED
}

#[cfg(test)]
mod tests {
    use super::{Script, ScriptAddress, ScriptType};
    use crypto::ChecksumType;
    use keys::{prefixes::BTC_PREFIXES, Address, Public};
    use {Builder, Error, Opcode};

    /// Maximum number of bytes pushable to the stack
    const MAX_SCRIPT_ELEMENT_SIZE: usize = 52;

    #[test]
    fn test_is_pay_to_script_hash() {
        let script: Script = "a9143b80842f4ea32806ce5e723a255ddd6490cfd28d87".into();
        let script2: Script = "a9143b80842f4ea32806ce5e723a255ddd6490cfd28d88".into();
        assert!(script.is_pay_to_script_hash());
        assert!(!script2.is_pay_to_script_hash());
    }

    #[test]
    fn test_is_pay_to_witness_key_hash() {
        let script: Script = "00140000000000000000000000000000000000000000".into();
        let script2: Script = "01140000000000000000000000000000000000000000".into();
        assert!(script.is_pay_to_witness_key_hash());
        assert!(!script2.is_pay_to_witness_key_hash());
    }

    #[test]
    fn test_is_pay_to_witness_script_hash() {
        let script: Script = "00203b80842f4ea32806ce5e723a255ddd6490cfd28dac38c58bf9254c0577330693".into();
        let script2: Script = "01203b80842f4ea32806ce5e723a255ddd6490cfd28dac38c58bf9254c0577330693".into();
        assert!(script.is_pay_to_witness_script_hash());
        assert!(!script2.is_pay_to_witness_script_hash());
    }

    #[test]
    fn test_script_debug() {
        use std::fmt::Write;

        let script = Builder::default()
            .push_num(3.into())
            .push_num(2.into())
            .push_opcode(Opcode::OP_ADD)
            .into_script();
        let s = "Script { data: 0103010293 }";
        let mut res = String::new();
        write!(&mut res, "{script:?}").unwrap();
        assert_eq!(s.to_string(), res);
    }

    #[test]
    fn test_script_display() {
        let script = Builder::default()
            .push_num(3.into())
            .push_num(2.into())
            .push_opcode(Opcode::OP_ADD)
            .into_script();
        let s = r#"OP_PUSHBYTES_1 0x03
OP_PUSHBYTES_1 0x02
OP_ADD
"#;
        assert_eq!(script.to_string(), s.to_string());
    }

    #[test]
    fn test_script_without_op_codeseparator() {
        let script: Script =
            "ab00270025512102e485fdaa062387c0bbb5ab711a093b6635299ec155b7b852fce6b992d5adbfec51ae".into();
        let scr_goal: Script =
            "00270025512102e485fdaa062387c0bbb5ab711a093b6635299ec155b7b852fce6b992d5adbfec51ae".into();
        assert_eq!(script.without_separators(), scr_goal);
    }

    #[test]
    fn test_script_is_multisig() {
        let script: Script = "524104a882d414e478039cd5b52a92ffb13dd5e6bd4515497439dffd691a0f12af9575fa349b5694ed3155b136f09e63975a1700c9f4d4df849323dac06cf3bd6458cd41046ce31db9bdd543e72fe3039a1f1c047dab87037c36a669ff90e28da1848f640de68c2fe913d363a51154a0c62d7adea1b822d05035077418267b1a1379790187410411ffd36c70776538d079fbae117dc38effafb33304af83ce4894589747aee1ef992f63280567f52f5ba870678b4ab4ff6c8ea600bd217870a8b4f1f09f3a8e8353ae".into();
        let not: Script = "ab00270025512102e485fdaa062387c0bbb5ab711a093b6635299ec155b7b852fce6b992d5adbfec51ae".into();
        assert!(script.is_multisig_script());
        assert!(!not.is_multisig_script());
    }

    // https://github.com/libbtc/libbtc/blob/998badcdac95a226a8f8c00c8f6abbd8a77917c1/test/tx_tests.c#L640
    #[test]
    fn test_script_type() {
        assert_eq!(
            ScriptType::PubKeyHash,
            Script::from("76a914aab76ba4877d696590d94ea3e02948b55294815188ac").script_type()
        );
        assert_eq!(ScriptType::Multisig, Script::from("522102004525da5546e7603eefad5ef971e82f7dad2272b34e6b3036ab1fe3d299c22f21037d7f2227e6c646707d1c61ecceb821794124363a2cf2c1d2a6f28cf01e5d6abe52ae").script_type());
        assert_eq!(
            ScriptType::ScriptHash,
            Script::from("a9146262b64aec1f4a4c1d21b32e9c2811dd2171fd7587").script_type()
        );
        assert_eq!(ScriptType::PubKey, Script::from("4104ae1a62fe09c5f51b13905f07f06b99a2f7159b2225f374cd378d71302fa28414e7aab37397f554a7df5f142c21c1b7303b8a0626f1baded5c72a704f7e6cd84cac").script_type());
    }

    #[test]
    fn test_sigops_count() {
        assert_eq!(
            1usize,
            Script::from("76a914aab76ba4877d696590d94ea3e02948b55294815188ac").sigops_count(false)
        );
        assert_eq!(2usize, Script::from("522102004525da5546e7603eefad5ef971e82f7dad2272b34e6b3036ab1fe3d299c22f21037d7f2227e6c646707d1c61ecceb821794124363a2cf2c1d2a6f28cf01e5d6abe52ae").sigops_count(true));
        assert_eq!(20usize, Script::from("522102004525da5546e7603eefad5ef971e82f7dad2272b34e6b3036ab1fe3d299c22f21037d7f2227e6c646707d1c61ecceb821794124363a2cf2c1d2a6f28cf01e5d6abe52ae").sigops_count(false));
        assert_eq!(
            0usize,
            Script::from("a9146262b64aec1f4a4c1d21b32e9c2811dd2171fd7587").sigops_count(false)
        );
        assert_eq!(1usize, Script::from("4104ae1a62fe09c5f51b13905f07f06b99a2f7159b2225f374cd378d71302fa28414e7aab37397f554a7df5f142c21c1b7303b8a0626f1baded5c72a704f7e6cd84cac").sigops_count(false));
    }

    #[test]
    fn test_sigops_count_b73() {
        let max_block_sigops = 20000;
        let block_sigops = 0;
        let mut script =
            vec![Opcode::OP_CHECKSIG as u8; max_block_sigops - block_sigops + MAX_SCRIPT_ELEMENT_SIZE + 1 + 5 + 1];
        script[max_block_sigops - block_sigops] = Opcode::OP_PUSHDATA4 as u8;
        let overmax = MAX_SCRIPT_ELEMENT_SIZE + 1;
        script[max_block_sigops - block_sigops + 1] = overmax as u8;
        script[max_block_sigops - block_sigops + 2] = (overmax >> 8) as u8;
        script[max_block_sigops - block_sigops + 3] = (overmax >> 16) as u8;
        script[max_block_sigops - block_sigops + 4] = (overmax >> 24) as u8;
        let script: Script = script.into();
        assert_eq!(script.sigops_count(false), 20001);
    }

    #[test]
    fn test_sigops_count_b74() {
        let max_block_sigops = 20000;
        let block_sigops = 0;
        let mut script =
            vec![Opcode::OP_CHECKSIG as u8; max_block_sigops - block_sigops + MAX_SCRIPT_ELEMENT_SIZE + 42];
        script[max_block_sigops - block_sigops + 1] = Opcode::OP_PUSHDATA4 as u8;
        script[max_block_sigops - block_sigops + 2] = 0xfe;
        script[max_block_sigops - block_sigops + 3] = 0xff;
        script[max_block_sigops - block_sigops + 4] = 0xff;
        script[max_block_sigops - block_sigops + 5] = 0xff;
        let script: Script = script.into();
        assert_eq!(script.sigops_count(false), 20001);
    }

    #[test]
    fn test_script_empty_find_and_delete() {
        let s: Script = vec![Opcode::OP_0 as u8].into();
        let result = s.find_and_delete(&[]);
        assert_eq!(s, result);
    }

    #[test]
    fn test_extract_destinations_pub_key_compressed() {
        let pubkey_bytes = [0; 33];
        let address = Public::from_slice(&pubkey_bytes).unwrap().address_hash();
        let script = Builder::default()
            .push_data(&pubkey_bytes)
            .push_opcode(Opcode::OP_CHECKSIG)
            .into_script();
        assert_eq!(script.script_type(), ScriptType::PubKey);
        assert_eq!(
            script.extract_destinations(),
            Ok(vec![ScriptAddress::new_p2pkh(address.into()),])
        );
    }

    #[test]
    fn test_extract_destinations_pub_key_normal() {
        let pubkey_bytes = [0; 65];
        let address = Public::from_slice(&pubkey_bytes).unwrap().address_hash();
        let script = Builder::default()
            .push_data(&pubkey_bytes)
            .push_opcode(Opcode::OP_CHECKSIG)
            .into_script();
        assert_eq!(script.script_type(), ScriptType::PubKey);
        assert_eq!(
            script.extract_destinations(),
            Ok(vec![ScriptAddress::new_p2pkh(address.into()),])
        );
    }

    #[test]
    fn test_extract_destinations_pub_key_hash() {
        let address = Address::from_legacyaddress("13NMTpfNVVJQTNH4spP4UeqBGqLdqDo27S", &BTC_PREFIXES)
            .unwrap()
            .hash()
            .clone();
        let script = Builder::build_p2pkh(&address);
        assert_eq!(script.script_type(), ScriptType::PubKeyHash);
        assert_eq!(
            script.extract_destinations(),
            Ok(vec![ScriptAddress::new_p2pkh(address),])
        );
    }

    #[test]
    fn test_extract_destinations_script_hash() {
        let address = Address::from_legacyaddress("13NMTpfNVVJQTNH4spP4UeqBGqLdqDo27S", &BTC_PREFIXES)
            .unwrap()
            .hash()
            .clone();
        let script = Builder::build_p2sh(&address);
        assert_eq!(script.script_type(), ScriptType::ScriptHash);
        assert_eq!(
            script.extract_destinations(),
            Ok(vec![ScriptAddress::new_p2sh(address),])
        );
    }

    #[test]
    fn test_extract_destinations_witness_pub_key_hash() {
        let address_hash =
            Address::from_segwitaddress("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4", ChecksumType::DSHA256)
                .unwrap()
                .hash()
                .clone();
        let script = Builder::build_p2wpkh(&address_hash).expect("build p2wpkh ok");
        assert_eq!(script.script_type(), ScriptType::WitnessKey);
        assert_eq!(
            script.extract_destinations(),
            Ok(vec![ScriptAddress::new_p2wpkh(address_hash),])
        );
    }

    #[test]
    fn test_extract_destinations_witness_script_hash() {
        let address_hash = Address::from_segwitaddress(
            "bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3",
            ChecksumType::DSHA256,
        )
        .unwrap()
        .hash()
        .clone();
        let script = Builder::build_p2wsh(&address_hash).expect("build p2wsh ok");
        assert_eq!(script.script_type(), ScriptType::WitnessScript);
        assert_eq!(
            script.extract_destinations(),
            Ok(vec![ScriptAddress::new_p2wsh(address_hash),])
        );
    }

    #[test]
    fn test_extract_destinations_multisig() {
        let pubkey1_bytes = [0; 33];
        let address1 = Public::from_slice(&pubkey1_bytes).unwrap().address_hash();
        let pubkey2_bytes = [1; 65];
        let address2 = Public::from_slice(&pubkey2_bytes).unwrap().address_hash();
        let script = Builder::default()
            .push_opcode(Opcode::OP_2)
            .push_data(&pubkey1_bytes)
            .push_data(&pubkey2_bytes)
            .push_opcode(Opcode::OP_2)
            .push_opcode(Opcode::OP_CHECKMULTISIG)
            .into_script();
        assert_eq!(script.script_type(), ScriptType::Multisig);
        assert_eq!(
            script.extract_destinations(),
            Ok(vec![
                ScriptAddress::new_p2pkh(address1.into()),
                ScriptAddress::new_p2pkh(address2.into()),
            ])
        );
    }

    #[test]
    fn test_num_signatures_required() {
        let script = Builder::default()
            .push_opcode(Opcode::OP_3)
            .push_data(&[0; 33])
            .push_data(&[0; 65])
            .push_data(&[0; 65])
            .push_data(&[0; 65])
            .push_opcode(Opcode::OP_4)
            .push_opcode(Opcode::OP_CHECKMULTISIG)
            .into_script();
        assert_eq!(script.script_type(), ScriptType::Multisig);
        assert_eq!(script.num_signatures_required(), 3);

        let script = Builder::default()
            .push_opcode(Opcode::OP_HASH160)
            .push_data(&[0; 20])
            .push_opcode(Opcode::OP_EQUAL)
            .into_script();
        assert_eq!(script.script_type(), ScriptType::ScriptHash);
        assert_eq!(script.num_signatures_required(), 1);
    }

    #[test]
    fn test_get_instruction() {
        // Builder::default()
        // 	.push_opcode(Opcode::OP_4)
        // 	.push_opcode(Opcode::OP_HASH160)
        // 	.push_data(&[0; 20])
        // 	.push_opcode(Opcode::_F9) // Bad opcode - 0xf9
        // 	.push_data(&[1; 20])
        // 	.push_opcode(Opcode::OP_EQUAL)
        // is the same as following:
        let script: Script =
            "54a9140000000000000000000000000000000000000000f914010101010101010101010101010101010101010187".into();

        match script.get_instruction(0) {
            Some(Ok(instr)) => assert_eq!(instr.opcode, Opcode::OP_4),
            _ => panic!("Expected Some(Ok)"),
        }
        match script.get_instruction(1) {
            Some(Ok(instr)) => assert_eq!(instr.opcode, Opcode::OP_HASH160),
            _ => panic!("Expected Some(Ok)"),
        }
        match script.get_instruction(2) {
            Some(Ok(instr)) => assert_eq!(instr.opcode, Opcode::OP_PUSHBYTES_20),
            _ => panic!("Expected Some(Ok)"),
        }
        match script.get_instruction(3) {
            Some(Err(e)) => assert_eq!(e, Error::BadOpcode),
            _ => panic!("Expected error"),
        }
        assert!(script.get_instruction(4).is_none());
        assert!(script.get_instruction(5).is_none());
        assert!(script.get_instruction(10).is_none());
        assert!(script.get_instruction(1245).is_none());
        assert!(script.get_instruction(99187829).is_none());
    }

    #[test]
    fn test_script_iter() {
        // Builder::default()
        // 	.push_opcode(Opcode::OP_4)
        // 	.push_opcode(Opcode::OP_HASH160)
        // 	.push_data(&[0; 20])
        // 	.push_opcode(Opcode::_F9) // Bad opcode - 0xf9
        // 	.push_data(&[1; 20])
        // 	.push_opcode(Opcode::OP_EQUAL)
        // is the same as following:
        let script: Script =
            "54a9140000000000000000000000000000000000000000f914010101010101010101010101010101010101010187".into();

        let mut max_idx = 0;
        for (idx, r) in script.iter().enumerate() {
            assert!(idx < 4, "Loop must stop on error");
            match idx {
                3 => assert!(r.is_err()),
                _ => assert!(r.is_ok()),
            }
            max_idx = idx;
        }
        assert_eq!(max_idx, 3);
    }

    #[test]
    fn test_does_script_spend_p2pk() {
        let script_sig = Script::from("473044022071edae37cf518e98db3f7637b9073a7a980b957b0c7b871415dbb4898ec3ebdc022031b402a6b98e64ffdf752266449ca979a9f70144dba77ed7a6a25bfab11648f6012103ad6f89abc2e5beaa8a3ac28e22170659b3209fe2ddf439681b4b8f31508c36fa");
        assert!(!script_sig.does_script_spend_p2pk());
        // The scriptSig of the input spent from: https://mempool.space/tx/1db6251a9afce7025a2061a19e63c700dffc3bec368bd1883decfac353357a9d
        let script_sig = Script::from("483045022078e86c021003cca23842d4b2862dfdb68d2478a98c08c10dcdffa060e55c72be022100f6a41da12cdc2e350045f4c97feeab76a7c0ab937bd8a9e507293ce6d37c9cc201");
        assert!(script_sig.does_script_spend_p2pk());
    }
}
