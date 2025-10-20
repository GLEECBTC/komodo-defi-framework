//! Bitcoin transaction.
//! https://en.bitcoin.it/wiki/Protocol_documentation#tx

use bytes::Bytes;
use constants::{LOCKTIME_THRESHOLD, SEQUENCE_FINAL};
use crypto::{dhash256, sha256};
#[cfg(not(target_arch = "wasm32"))]
use ext_bitcoin::blockdata::transaction::{OutPoint as ExtOutpoint, Transaction as ExtTransaction, TxIn, TxOut};
#[cfg(not(target_arch = "wasm32"))]
use ext_bitcoin::hash_types::Txid;
#[cfg(not(target_arch = "wasm32"))]
use ext_bitcoin::{PackedLockTime, Sequence, Witness};
use hash::{CipherText, EncCipherText, OutCipherText, ZkProof, ZkProofSapling, H256, H512, H64};
use hex::FromHex;
use ser::{deserialize, serialize, serialize_with_flags, SERIALIZE_TRANSACTION_WITNESS};
use ser::{CompactInteger, Deserializable, Error, Reader, Serializable, Stream};
use std::fmt::Formatter;
use std::io;
use std::io::Read;

/// Must be zero.
const WITNESS_MARKER: u8 = 0;
/// Must be nonzero.
const WITNESS_FLAG: u8 = 1;
/// Maximum supported list size (inputs, outputs, etc.)
const MAX_LIST_SIZE: usize = 8192;

#[derive(Clone, Copy, Debug, Default, Deserializable, Eq, Hash, PartialEq, Serializable)]
pub struct OutPoint {
    pub hash: H256,
    pub index: u32,
}

impl OutPoint {
    pub fn null() -> Self {
        OutPoint {
            hash: H256::default(),
            index: u32::MAX,
        }
    }

    pub fn is_null(&self) -> bool {
        self.hash.is_zero() && self.index == u32::MAX
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<OutPoint> for ExtOutpoint {
    fn from(outpoint: OutPoint) -> Self {
        ExtOutpoint {
            txid: Txid::from_hash(outpoint.hash.to_sha256d()),
            vout: outpoint.index,
        }
    }
}

#[derive(Debug, PartialEq, Default, Clone)]
pub struct TransactionInput {
    pub previous_output: OutPoint,
    pub script_sig: Bytes,
    pub sequence: u32,
    pub script_witness: Vec<Bytes>,
}

impl TransactionInput {
    pub fn coinbase(script_sig: Bytes) -> Self {
        TransactionInput {
            previous_output: OutPoint::null(),
            script_sig,
            sequence: SEQUENCE_FINAL,
            script_witness: vec![],
        }
    }

    pub fn is_final(&self) -> bool {
        self.sequence == SEQUENCE_FINAL
    }

    pub fn has_witness(&self) -> bool {
        !self.script_witness.is_empty()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<TransactionInput> for TxIn {
    fn from(txin: TransactionInput) -> Self {
        TxIn {
            previous_output: txin.previous_output.into(),
            script_sig: txin.script_sig.take().into(),
            sequence: Sequence(txin.sequence),
            witness: Witness::from_vec(txin.script_witness.into_iter().map(|s| s.take()).collect()),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Serializable, Deserializable)]
pub struct TransactionOutput {
    pub value: u64,
    pub script_pubkey: Bytes,
}

impl Default for TransactionOutput {
    fn default() -> Self {
        TransactionOutput {
            value: 0xffffffffffffffffu64,
            script_pubkey: Bytes::default(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<TransactionOutput> for TxOut {
    fn from(txout: TransactionOutput) -> Self {
        TxOut {
            value: txout.value,
            script_pubkey: txout.script_pubkey.take().into(),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Serializable, Deserializable)]
pub struct ShieldedSpend {
    pub cv: H256,
    pub anchor: H256,
    pub nullifier: H256,
    pub rk: H256,
    pub zkproof: ZkProofSapling,
    pub spend_auth_sig: H512,
}

#[derive(Debug, PartialEq, Clone, Serializable, Deserializable)]
pub struct ShieldedOutput {
    pub cv: H256,
    pub cmu: H256,
    pub ephemeral_key: H256,
    pub enc_cipher_text: EncCipherText,
    pub out_cipher_text: OutCipherText,
    pub zkproof: ZkProofSapling,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, PartialEq, Clone)]
pub enum JoinSplitProof {
    PHGR(ZkProof),
    Groth(ZkProofSapling),
}

impl Serializable for JoinSplitProof {
    fn serialize(&self, stream: &mut Stream) {
        match self {
            JoinSplitProof::PHGR(p) => stream.append(p),
            JoinSplitProof::Groth(p) => stream.append(p),
        };
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct JoinSplit {
    pub v_pub_old: H64,
    pub v_pub_new: H64,
    pub anchor: H256,
    pub nullifiers: [H256; 2],
    pub commitments: [H256; 2],
    pub ephemeral_key: H256,
    pub random_seed: H256,
    pub macs: [H256; 2],
    pub zkproof: JoinSplitProof,
    pub ciphertexts: [CipherText; 2],
}

// TODO Make it more optimal later by adding fixed-size array support to serialization_derive crate
impl Serializable for JoinSplit {
    fn serialize(&self, stream: &mut Stream) {
        stream
            .append(&self.v_pub_old)
            .append(&self.v_pub_new)
            .append(&self.anchor)
            .append(&self.nullifiers[0])
            .append(&self.nullifiers[1])
            .append(&self.commitments[0])
            .append(&self.commitments[1])
            .append(&self.ephemeral_key)
            .append(&self.random_seed)
            .append(&self.macs[0])
            .append(&self.macs[1])
            .append(&self.zkproof)
            .append(&self.ciphertexts[0])
            .append(&self.ciphertexts[1]);
    }
}

fn deserialize_join_split<T>(reader: &mut Reader<T>, use_groth: bool) -> Result<JoinSplit, Error>
where
    T: io::Read,
{
    Ok(JoinSplit {
        v_pub_old: reader.read()?,
        v_pub_new: reader.read()?,
        anchor: reader.read()?,
        nullifiers: [reader.read()?, reader.read()?],
        commitments: [reader.read()?, reader.read()?],
        ephemeral_key: reader.read()?,
        random_seed: reader.read()?,
        macs: [reader.read()?, reader.read()?],
        zkproof: if use_groth {
            let proof: ZkProofSapling = reader.read()?;
            JoinSplitProof::Groth(proof)
        } else {
            let proof: ZkProof = reader.read()?;
            JoinSplitProof::PHGR(proof)
        },
        ciphertexts: [reader.read()?, reader.read()?],
    })
}

#[derive(Debug, PartialEq, Default, Clone)]
pub struct Transaction {
    pub version: i32,
    pub n_time: Option<u32>,
    pub overwintered: bool,
    pub version_group_id: u32,
    pub inputs: Vec<TransactionInput>,
    pub outputs: Vec<TransactionOutput>,
    pub lock_time: u32,
    pub expiry_height: u32,
    pub shielded_spends: Vec<ShieldedSpend>,
    pub shielded_outputs: Vec<ShieldedOutput>,
    pub join_splits: Vec<JoinSplit>,
    pub value_balance: i64,
    pub join_split_pubkey: H256,
    pub join_split_sig: H512,
    pub binding_sig: H512,
    pub zcash: bool,
    pub posv: bool,
    /// https://github.com/navcoin/navcoin-core/blob/556250920fef9dc3eddd28996329ba316de5f909/src/primitives/transaction.h#L497
    pub str_d_zeel: Option<String>,
    pub tx_hash_algo: TxHashAlgo,
    /// https://github.com/firoorg/firo/blob/bf7c7fa555bd00db970c5c3e64e7452ef20c800e/src/primitives/transaction.h#L389
    pub v_extra_payload: Option<Vec<u8>>, // only available for special transaction types
}

impl From<&'static str> for Transaction {
    fn from(s: &'static str) -> Self {
        deserialize(&s.from_hex::<Vec<u8>>().unwrap() as &[u8]).unwrap()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<Transaction> for ExtTransaction {
    fn from(tx: Transaction) -> Self {
        ExtTransaction {
            version: tx.version,
            lock_time: PackedLockTime(tx.lock_time),
            input: tx.inputs.into_iter().map(|i| i.into()).collect(),
            output: tx.outputs.into_iter().map(|o| o.into()).collect(),
        }
    }
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TxHashAlgo {
    #[default]
    DSHA256,
    SHA256,
}

/// Represents the error returned when transaction has no outputs
#[derive(Debug)]
pub struct TxHasNoOutputs {}

impl std::fmt::Display for TxHasNoOutputs {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tx has no outputs")
    }
}

impl Transaction {
    pub fn hash(&self) -> H256 {
        let serialized = &serialize(self);
        match self.tx_hash_algo {
            TxHashAlgo::DSHA256 => dhash256(serialized),
            TxHashAlgo::SHA256 => sha256(serialized),
        }
    }

    pub fn witness_hash(&self) -> H256 {
        dhash256(&serialize_with_flags(self, SERIALIZE_TRANSACTION_WITNESS))
    }

    pub fn inputs(&self) -> &[TransactionInput] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[TransactionOutput] {
        &self.outputs
    }

    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty() || self.outputs.is_empty()
    }

    pub fn is_null(&self) -> bool {
        self.inputs.iter().any(|input| input.previous_output.is_null())
    }

    pub fn is_coinbase(&self) -> bool {
        self.inputs.len() == 1 && self.inputs[0].previous_output.is_null()
    }

    pub fn is_final(&self) -> bool {
        // if lock_time is 0, transaction is final
        if self.lock_time == 0 {
            return true;
        }
        // setting all sequence numbers to 0xffffffff disables the time lock, so if you want to use locktime,
        // at least one input must have a sequence number below the maximum.
        self.inputs.iter().all(TransactionInput::is_final)
    }

    pub fn is_final_in_block(&self, block_height: u32, block_time: u32) -> bool {
        if self.lock_time == 0 {
            return true;
        }

        let max_lock_time = if self.lock_time < LOCKTIME_THRESHOLD {
            block_height
        } else {
            block_time
        };

        if self.lock_time < max_lock_time {
            return true;
        }

        self.inputs.iter().all(TransactionInput::is_final)
    }

    pub fn has_witness(&self) -> bool {
        self.inputs.iter().any(TransactionInput::has_witness)
    }

    pub fn total_spends(&self) -> u64 {
        let mut result = 0u64;
        for output in self.outputs.iter() {
            if u64::MAX - result < output.value {
                return u64::MAX;
            }
            result += output.value;
        }
        result
    }

    /// Returns reference to first output of the transaction or error if outputs are empty
    #[inline]
    pub fn first_output(&self) -> Result<&TransactionOutput, TxHasNoOutputs> {
        self.outputs.first().ok_or(TxHasNoOutputs {})
    }
}

impl Serializable for TransactionInput {
    fn serialize(&self, stream: &mut Stream) {
        stream
            .append(&self.previous_output)
            .append(&self.script_sig)
            .append(&self.sequence);
    }
}

impl Deserializable for TransactionInput {
    fn deserialize<T>(reader: &mut Reader<T>) -> Result<Self, Error>
    where
        Self: Sized,
        T: io::Read,
    {
        Ok(TransactionInput {
            previous_output: reader.read()?,
            script_sig: reader.read()?,
            sequence: reader.read()?,
            script_witness: vec![],
        })
    }
}

impl Serializable for Transaction {
    fn serialize(&self, stream: &mut Stream) {
        let include_transaction_witness = stream.include_transaction_witness() && self.has_witness();
        match include_transaction_witness {
            false => {
                let mut header = self.version;
                if self.overwintered {
                    header |= 1 << 31;
                }
                stream.append(&header);

                if self.overwintered {
                    stream.append(&self.version_group_id);
                }

                if !self.posv {
                    if let Some(n_time) = self.n_time {
                        stream.append(&n_time);
                    }
                }

                stream
                    .append_list(&self.inputs)
                    .append_list(&self.outputs)
                    .append(&self.lock_time);

                if self.posv {
                    if let Some(n_time) = self.n_time {
                        stream.append(&n_time);
                    }
                }

                if self.overwintered && self.version >= 3 {
                    stream.append(&self.expiry_height);
                    if self.version >= 4 {
                        stream
                            .append(&self.value_balance)
                            .append_list(&self.shielded_spends)
                            .append_list(&self.shielded_outputs);
                    }
                }
                if self.zcash {
                    if self.version == 2 || self.overwintered {
                        stream.append_list(&self.join_splits);
                        if !self.join_splits.is_empty() {
                            stream.append(&self.join_split_pubkey).append(&self.join_split_sig);
                        }
                    }

                    if self.version >= 4
                        && self.overwintered
                        && !(self.shielded_outputs.is_empty() && self.shielded_spends.is_empty())
                    {
                        stream.append(&self.binding_sig);
                    }
                }
                if let Some(ref string) = self.str_d_zeel {
                    let len: CompactInteger = string.len().into();
                    stream.append(&len);
                    stream.append_slice(string.as_bytes());
                }
                if let Some(ref payload) = self.v_extra_payload {
                    if !payload.is_empty() {
                        let len: CompactInteger = payload.len().into();
                        stream.append(&len);
                        stream.append_slice(payload);
                    }
                }
            },
            true => {
                stream
                    .append(&self.version)
                    .append(&WITNESS_MARKER)
                    .append(&WITNESS_FLAG)
                    .append_list(&self.inputs)
                    .append_list(&self.outputs);
                for input in &self.inputs {
                    stream.append_list(&input.script_witness);
                }
                stream.append(&self.lock_time);
            },
        };
    }
}

#[derive(Eq, PartialEq)]
pub enum TxType {
    StandardWithWitness,
    Zcash,
    PosWithNTime,
    PosvWithNTime,
}

impl TxType {
    fn uses_witness(&self) -> bool {
        matches!(self, TxType::StandardWithWitness | TxType::PosvWithNTime)
    }
}

pub fn deserialize_tx<T>(reader: &mut Reader<T>, tx_type: TxType) -> Result<Transaction, Error>
where
    T: io::Read,
{
    let header: i32 = reader.read()?;
    let overwintered: bool = (header >> 31) != 0;
    let version = if overwintered { header & 0x7FFFFFFF } else { header };

    // Check if we might have a version 3 (Spark) transaction using the 0xFFFF mask
    let spark_version = header & 0xFFFF;
    let maybe_spark = spark_version == 3;

    let mut version_group_id = 0;
    if overwintered {
        version_group_id = reader.read()?;
    }

    let mut n_time = if tx_type == TxType::PosWithNTime {
        Some(reader.read()?)
    } else {
        None
    };
    let mut inputs: Vec<TransactionInput> = reader.read_list_max(MAX_LIST_SIZE)?;
    let read_witness = if inputs.is_empty() && !overwintered && tx_type.uses_witness() {
        let witness_flag: u8 = reader.read()?;
        if witness_flag != WITNESS_FLAG {
            return Err(Error::MalformedData);
        }

        inputs = reader.read_list_max(MAX_LIST_SIZE)?;
        true
    } else {
        false
    };
    let outputs = reader.read_list_max(MAX_LIST_SIZE)?;
    if outputs.is_empty() && tx_type.uses_witness() {
        return Err(Error::Custom("Transaction has no output".into()));
    }
    if read_witness {
        for input in inputs.iter_mut() {
            input.script_witness = reader.read_list_max(MAX_LIST_SIZE)?;
        }
    }

    let lock_time = reader.read()?;

    let mut posv = false;
    n_time = if tx_type == TxType::PosvWithNTime && !maybe_spark {
        posv = true;
        Some(reader.read()?)
    } else {
        n_time
    };

    let mut expiry_height = 0;
    let mut value_balance = 0;
    let mut shielded_spends = vec![];
    let mut shielded_outputs = vec![];
    if overwintered && version >= 3 {
        expiry_height = reader.read()?;
        if version >= 4 {
            value_balance = reader.read()?;
            shielded_spends = reader.read_list_max(MAX_LIST_SIZE)?;
            shielded_outputs = reader.read_list_max(MAX_LIST_SIZE)?;
        }
    }

    let mut join_splits = vec![];
    let mut join_split_pubkey = H256::default();
    let mut join_split_sig = H512::default();
    let mut binding_sig = H512::default();
    let zcash = overwintered || tx_type == TxType::Zcash;
    if zcash {
        if version == 2 || overwintered {
            let len: usize = reader.read::<CompactInteger>()?.into();
            if len > 0 {
                if len > MAX_LIST_SIZE {
                    return Err(Error::MalformedData);
                };
                let use_groth = version > 2;
                for _ in 0..len {
                    join_splits.push(deserialize_join_split(reader, use_groth)?);
                }
                join_split_pubkey = reader.read()?;
                join_split_sig = reader.read()?;
            }
        }

        if overwintered && version >= 4 && !(shielded_spends.is_empty() && shielded_outputs.is_empty()) {
            binding_sig = reader.read()?;
        }
    };

    let str_d_zeel = if tx_type == TxType::PosWithNTime && !reader.is_finished() {
        let len: CompactInteger = reader.read()?;
        let mut buf = vec![0; len.into()];
        reader.read_slice(&mut buf)?;
        let string = std::str::from_utf8(&buf).map_err(|_| Error::MalformedData)?;
        Some(string.into())
    } else {
        None
    };

    // Check for extra payload if it might be a Spark transaction
    let v_extra_payload = if maybe_spark && !reader.is_finished() {
        let len: CompactInteger = reader.read()?;
        let mut buf = vec![0; len.into()];
        reader.read_slice(&mut buf)?;
        Some(buf)
    } else {
        None
    };

    Ok(Transaction {
        version,
        n_time,
        overwintered,
        version_group_id,
        expiry_height,
        value_balance,
        inputs,
        outputs,
        lock_time,
        binding_sig,
        join_split_pubkey,
        join_split_sig,
        join_splits,
        shielded_spends,
        shielded_outputs,
        zcash,
        posv,
        str_d_zeel,
        tx_hash_algo: TxHashAlgo::DSHA256,
        v_extra_payload,
    })
}

impl Deserializable for Transaction {
    fn deserialize<T>(reader: &mut Reader<T>) -> Result<Self, Error>
    where
        Self: Sized,
        T: io::Read,
    {
        // read the entire buffer to get it's copy for different cases
        // it works properly only when buffer contains only 1 transaction bytes
        // it breaks block serialization, but block serialization is not required for AtomicDEX
        // specific use case
        let mut buffer = vec![];
        reader.read_to_end(&mut buffer)?;

        if let Ok(t) = deserialize_tx(&mut Reader::from_read(buffer.as_slice()), TxType::PosvWithNTime) {
            return Ok(t);
        }
        if let Ok(t) = deserialize_tx(&mut Reader::from_read(buffer.as_slice()), TxType::StandardWithWitness) {
            return Ok(t);
        }
        if let Ok(t) = deserialize_tx(&mut Reader::from_read(buffer.as_slice()), TxType::PosWithNTime) {
            return Ok(t);
        }
        deserialize_tx(&mut Reader::from_read(buffer.as_slice()), TxType::Zcash)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_arch = "wasm32"))]
    use super::ExtTransaction;
    use super::{Bytes, OutPoint, Transaction, TransactionInput, TransactionOutput};
    use hash::{H256, H512};
    use hex::ToHex;
    use ser::{deserialize, serialize, serialize_with_flags, Serializable, SERIALIZE_TRANSACTION_WITNESS};
    use TxHashAlgo;

    // real transaction from block 80000
    // https://blockchain.info/rawtx/5a4ebf66822b0b2d56bd9dc64ece0bc38ee7844a23ff1d7320a88c5fdb2ad3e2
    // https://blockchain.info/rawtx/5a4ebf66822b0b2d56bd9dc64ece0bc38ee7844a23ff1d7320a88c5fdb2ad3e2?format=hex
    #[test]
    fn test_transaction_reader() {
        let t: Transaction = "0100000001a6b97044d03da79c005b20ea9c0e1a6d9dc12d9f7b91a5911c9030a439eed8f5000000004948304502206e21798a42fae0e854281abd38bacd1aeed3ee3738d9e1446618c4571d1090db022100e2ac980643b0b82c0e88ffdfec6b64e3e6ba35e7ba5fdd7d5d6cc8d25c6b241501ffffffff0100f2052a010000001976a914404371705fa9bd789a2fcd52d2c580b65d35549d88ac00000000".into();
        assert_eq!(t.version, 1);
        assert_eq!(t.lock_time, 0);
        assert_eq!(t.inputs.len(), 1);
        assert_eq!(t.outputs.len(), 1);
        let tx_input = &t.inputs[0];
        assert_eq!(tx_input.sequence, 4294967295);
        assert_eq!(tx_input.script_sig, "48304502206e21798a42fae0e854281abd38bacd1aeed3ee3738d9e1446618c4571d1090db022100e2ac980643b0b82c0e88ffdfec6b64e3e6ba35e7ba5fdd7d5d6cc8d25c6b241501".into());
        let tx_output = &t.outputs[0];
        assert_eq!(tx_output.value, 5000000000);
        assert_eq!(
            tx_output.script_pubkey,
            "76a914404371705fa9bd789a2fcd52d2c580b65d35549d88ac".into()
        );
        assert!(!t.has_witness());
    }

    #[test]
    fn test_transaction_reader_v7() {
        let raw = "0700000001f87575693f4c038018628ff89f64571f0b9b48cd91a09b984d7eb018f4753bfa000000006a47304402202a3c612b11db1be51ae47fc1c23cc73e7fb14f08f10b3e71e5778d7adad494e90220636ca2580324452d8596cea7b2ebc31d796787108a7f74b676e3f136cb2c56b9012102e75e70baceb8cd5ae2bdc893d018512aafc8aac403ae8c14da66fa3ede87fcc3ffffffff0148b6eb0b000000001976a914139df01a608671fcf24db66d2d02bf2d4274e1f888ac00000000";
        let t: Transaction = raw.into();

        assert_eq!(t.version, 7);
        assert_eq!(t.lock_time, 0);
        assert_eq!(t.inputs.len(), 1);
        assert_eq!(t.outputs.len(), 1);

        let serialized = serialize(&t);
        assert_eq!(Bytes::from(raw), serialized);
    }

    // https://github.com/zcash/zips/blob/master/zip-0243.rst#test-vector-1
    #[test]
    fn test_transaction_serde_overwintered_sapling() {
        let raw = "0400008085202f890002e7719811893e0000095200ac6551ac636565b2835a0805750200025151481cdd86b3cc4318442117623ceb0500031b3d1a027c2c40590958b7eb13d742a997738c46a458965baf276ba92f272c721fe01f7e9c8e36d6a5e29d4e30a73594bf5098421c69378af1e40f64e125946f62c2fa7b2fecbcb64b6968912a6381ce3dc166d56a1d62f5a8d7551db5fd9313e8c7203d996af7d477083756d59af80d06a745f44ab023752cb5b406ed8985e18130ab33362697b0e4e4c763ccb8f676495c222f7fba1e31defa3d5a57efc2e1e9b01a035587d5fb1a38e01d94903d3c3e0ad3360c1d3710acd20b183e31d49f25c9a138f49b1a537edcf04be34a9851a7af9db6990ed83dd64af3597c04323ea51b0052ad8084a8b9da948d320dadd64f5431e61ddf658d24ae67c22c8d1309131fc00fe7f235734276d38d47f1e191e00c7a1d48af046827591e9733a97fa6b679f3dc601d008285edcbdae69ce8fc1be4aac00ff2711ebd931de518856878f73476f21a482ec9378365c8f7393c94e2885315eb4671098b79535e790fe53e29fef2b3766697ac32b4f473f468a008e72389fc03880d780cb07fcfaabe3f1a15825b7acb4d6b57a61bc68f242b52e4fbf85cf1a09cc45b6d6bb3a391578f499486a7afd04a0d9c74c2995d96b4de37b36046a1ef6d190b916b1111c92887311a20da8aba18d1dbebbc862ded42435e92476930d069896cff30eb414f727b89e001afa2fb8dc3436d75a4a6f26572504b192232ecb9f0c02411e52596bc5e90457e745939ffedbd12863ce71a02af117d417adb3d15cc54dcb1fce467500c6b8fb86b12b56da9c382857deecc40a98d5f2935395ee4762dd21afdbb5d47fa9a6dd984d567db2857b927b7fae2db587105415d4642789d38f50b8dbcc129cab3d17d19f3355bcf73cecb8cb8a5da01307152f13936a270572670dc82d39026c6cb4cd4b0f7f5aa2a4f5a5341ec5dd715406f2fdd2afa733f5f641c8c21862a1bafce2609d9eecfa158cfb5cd79f88008e315dc7d8388e76c1782fd2795d18a763624c25fa959cc97489ce75745824b77868c53239cfbdf73caec65604037314faaceb56218c6bd30f8374ac13386793f21a9fb80ad03bc0cda4a44946c00e1b102c78f11876b7065212183199fb5979ca77d2c24c738fe5145f02602053bb4c2f6556df6ed4b4ddd3d9a69f53357d7767f4f5ccbdbc596631277f8fecd08cb056b95e3025b9792fff7f244fc716269b926d62e9596fa825c6bf21aff9e68625a192440ea06828123d97884806f15fa08da52754a1095e3ff1abd5ce4fddfccfc3a6128aef784a64610a89d1a7099216d0814d3a2d452431c32d411ac1cce82ad0229407bbc48985675e3f874a4533f1d63a84dfa3e0f460fe2f57e34fbc75423c3737f5b2a0615f5722db041a3ef66fa483afd3c2e19e59444a64add6df1d963f5dd5b5010d3d025f0287c4cf19c75f33d51ddddba5d657b43ee8da645443814cc7329f3e9b4e54c236c29af3923101756d9fa4bd0f7d2ddaacb6b0f86a2658e0a07a05ac5b950051cd24c47a88d13d659ba2a46ca1830816d09cd7646f76f716abec5de07fe9b523410806ea6f288f8736c23357c85f45791e1708029d9824d90704607f387a03e49bf9836574431345a7877efaa8a08e73081ef8d62cb780a010fa3207ee2f0408097d563da1b2146819edf88d33e7753664fb71d122a6e36998fbd467f75b780149ae8808f4e68f50c0536acddf6f1aeab016b6bc1ec144b4e59aeb77eef49d00e5fbb67101cdd41e6bc9cf641a52fca98be915f8440a410d74cb30e15914f01bc6bc2307b488d2556d7b7380ea4ffd712f6b02fe806b94569cd4059f396bf29b99d0a40e5e1711ca944f72d436a102fca4b97693da0b086fe9d2e7162470d02e0f05d4bec9512bfb3f38327296efaa74328b118c27402c70c3a90b49ad4bbc68e37c0aa7d9b3fe17799d73b841e751713a02943905aae0803fd69442eb7681ec2a05600054e92eed555028f21b6a155268a2dd6640a69301a52a38d4d9f9f957ae35af7167118141ce4c9be0a6a492fe79f1581a155fa3a2b9dafd82e650b386ad3a08cb6b83131ac300b0846354a7eef9c410e4b62c47c5426907dfc6685c5c99b7141ac626ab4761fd3f41e728e1a28f89db89ffdeca364dd2f0f0739f0534556483199c71f189341ac9b78a269164206a0ea1ce73bfb2a942e7370b247c046f8e75ef8e3f8bd821cf577491864e20e6d08fd2e32b555c92c661f19588b72a89599710a88061253ca285b6304b37da2b5294f5cb354a894322848ccbdc7c2545b7da568afac87ffa005c312241c2d57f4b45d6419f0d2e2c5af33ae243785b325cdab95404fc7aed70525cddb41872cfcc214b13232edc78609753dbff930eb0dc156612b9cb434bc4b693392deb87c530435312edcedc6a961133338d786c4a3e103f60110a16b1337129704bf4754ff6ba9fbe65951e610620f71cda8fc877625f2c5bb04cbe1228b1e886f4050afd8fe94e97d2e9e85c6bb748c0042d3249abb1342bb0eebf62058bf3de080d94611a3750915b5dc6c0b3899d41222bace760ee9c8818ded599e34c56d7372af1eb86852f2a732104bdb750739de6c2c6e0f9eb7cb17f1942bfc9f4fd6ebb6b4cdd4da2bca26fac4578e9f543405acc7d86ff59158bd0cba3aef6f4a8472d144d99f8b8d1dedaa9077d4f01d4bb27bbe31d88fbefac3dcd4797563a26b1d61fcd9a464ab21ed550fe6fa09695ba0b2f10eea6468cc6e20a66f826e3d14c5006f0563887f5e1289be1b2004caca8d3f34d6e84bf59c1e04619a7c23a996941d889e4622a9b9b1d59d5e319094318cd405ba27b7e2c084762d31453ec4549a4d97729d033460fcf89d6494f2ffd789e98082ea5ce9534b3acd60fe49e37e4f666931677319ed89f85588741b3128901a93bd78e4be0225a9e2692c77c969ed0176bdf9555948cbd5a332d045de6ba6bf4490adfe7444cd467a09075417fc0200000000000000000000000000000000062e49f008c51ad4227439c1b4476ccd8e97862dab7be1e8d399c05ef27c6e22ee273e15786e394c8f1be31682a30147963ac8da8d41d804258426a3f70289b8ad19d8de13be4eebe3bd4c8a6f55d6e0c373d456851879f5fbc282db9e134806bff71e11bc33ab75dd6ca067fb73a043b646a7cf39cab4928386786d2f24141ee120fdc34d6764eafc66880ee0204f53cc1167ed20b43a52dea3ca7cff8ef35cd8e6d7c111a68ef44bcd0c1513ad47ca61c659cc5d325b440f6b9f59aff66879bb6688fd2859362b182f207b3175961f6411a493bffd048e7d0d87d82fe6f990a2b0a25f5aa0111a6e68f37bf6f3ac2d26b84686e569d58d99c1383597fad81193c4c1b16e6a90e2d507cdfe6fbdaa86163e9cf5de3100fbca7e8da047b090db9f37952fbfee76af61668190bd52ed490e677b515d014384af07219c7c0ee7fc7bfc79f325644e4df4c0d7db08e9f0bd024943c705abff8994bfa605cfbc7ed746a7d3f7c37d9e8bdc433b7d79e08a12f738a8f0dbddfef2f2657ef3e47d1b0fd11e6a13311fb799c79c641d9da43b33e7ad012e28255398789262275f1175be8462c01491c4d842406d0ec4282c9526174a09878fe8fdde33a29604e5e5e7b2a025d6650b97dbb52befb59b1d30a57433b0a351474444099daa371046613260cf3354cfcdada663ece824ffd7e44393886a86165ddddf2b4c41773554c86995269408b11e6737a4c447586f69173446d8e48bf84cbc000a807899973eb93c5e819aad669413f8387933ad1584aa35e43f4ecd1e2d0407c0b1b89920ffdfdb9bea51ac95b557af71b89f903f5d9848f14fcbeb1837570f544d6359eb23faf38a0822da36ce426c4a2fbeffeb0a8a2e297a9d19ba15024590e3329d9fa9261f9938a4032dd34606c9cf9f3dd33e576f05cd1dd6811c6298757d77d9e810abdb226afcaa4346a6560f8932b3181fd355d5d391976183f8d99388839632d6354f666d09d3e5629ea19737388613d38a34fd0f6e50ee5a0cc9677177f50028c141378187bd2819403fc534f80076e9380cb4964d3b6b45819d3b8e9caf54f051852d671bf8c1ffde2d1510756418cb4810936aa57e6965d6fb656a760b7f19adf96c173488552193b147ee58858033dac7cd0eb204c06490bbdedf5f7571acb2ebe76acef3f2a01ee987486dfe6c3f0a5e234c127258f97a28fb5d164a8176be946b8097d0e317287f33bf9c16f9a545409ce29b1f4273725fc0df02a04ebae178b3414fb0a82d50deb09fcf4e6ee9d180ff4f56ff3bc1d3601fc2dc90d814c3256f4967d3a8d64c83fea339c51f5a8e5801fbb97835581b602465dee04b5922c2761b54245bec0c9eef2db97d22b2b3556cc969fbb13d06509765a52b3fac54b93f421bf08e18d52ddd52cc1c8ca8adfaccab7e5cc2f4573fbbf8239bb0b8aedbf8dad16282da5c9125dba1c059d0df8abf621078f02d6c4bc86d40845ac1d59710c45f07d585eb48b32fc0167ba256e73ca3b9311c62d109497957d8dbe10aa3e866b40c0baa2bc492c19ad1e6372d9622bf163fbffeaeee796a3cd9b6fbbfa4d792f34d7fd6e763cd5859dd26833d21d9bc5452bd19515dff9f4995b35bc0c1f876e6ad11f2452dc9ae85aec01fc56f8cbfda75a7727b75ebbd6bbffb43b63a3b1b671e40feb0db002974a3c3b1a788567231bf6399ff89236981149d423802d2341a3bedb9ddcbac1fe7b6435e1479c72e7089d029e7fbbaf3cf37e9b9a6b776791e4c5e6fda57e8d5f14c8c35a2d270846b9dbe005cda16af4408f3ab06a916eeeb9c9594b70424a4c1d171295b6763b22f47f80b53ccbb904bd68fd65fbd3fbdea1035e98c21a7dbc91a9b5bc7690f05ec317c97f8764eb48e911d428ec8d861b708e8298acb62155145155ae95f0a1d1501034753146e22d05f586d7f6b4fe12dad9a17f5db70b1db96b8d9a83edadc966c8a5466b61fc998c31f1070d9a5c9a6d268d304fe6b8fd3b4010348611abdcbd49fe4f85b623c7828c71382e1034ea67bc8ae97404b0c50b2a04f559e49950afcb0ef462a2ae024b0f0224dfd73684b88c7fbe92d02b68f759c4752663cd7b97a14943649305521326bde085630864629291bae25ff8822a14c4b666a9259ad0dc42a8290ac7bc7f53a16f379f758e5de750f04fd7cad47701c8597f97888bea6fa0bf2999956fbfd0ee68ec36e4688809ae231eb8bc4369f5fe1573f57e099d9c09901bf39caac48dc11956a8ae905ead86954547c448ae43d315e669c4242da565938f417bf43ce7b2b30b1cd4018388e1a910f0fc41fb0877a5925e466819d375b0a912d4fe843b76ef6f223f0f7c894f38f7ab780dfd75f669c8c06cffa0000000000000000000000000000000043eb47565a50e3b1fa45ad61ce9a1c4727b7aaa53562f523e73952bbf33d8a4104078ade3eaaa49699a69fdf1c5ac7732146ee5e1d6b6ca9b9180f964cc9d0878ae1373524d7d510e58227df6de9d30d271867640177b0f1856e28d5c8afb095ef6184fed651589022eeaea4c0ce1fa6f085092b04979489172b3ef8194a798df5724d6b05f1ae000013a08d612bca8a8c31443c10346dbf61de8475c0bbec5104b47556af3d514458e2321d146071789d2335934a680614e83562f82dfd405b54a45eb32c165448d4d5d61ca2859585369f53f1a137e9e82b67b8fdaf01bda54a317311896ae10280a032440c420a421e944d1e952b70d5826cd3b08b7db9630fe4fd5f22125de840fcc40b98038af11d55be25432597b4b65b9ec1c7a8bbfd052cbf7e1c1785314934b262d5853754f1f17771cfb7503072655753fa3f54ecc587e9f83b581916092df26e63e18994cb0db91a0bbdc7b6119b32222adf5e61d8d8ae89dae4954b54813bb33f08d562ba513fee1b09c0fcd516055419474dd7fda038a89c84ea7b9468287f0eb0c10c4b132520194d3d8d5351fc10d09c15c8cc101aa1663bbf17b84111f38bb439f07353bdea3596d15e713e1e2e7d3f1c383135b47fa7f81f46df7a902a404699ec912f5656c35b85763e4de583aecaa1dfd5d2677d9c8ffee877f63f40a5ca0d67f6e554124739f805af876aeede53aa8b0f8e5604a73c30cbd09dad963d6f8a5dcc40def40797342113ba206fae8ebe4f3bc3caf69259e462eff9ba8b3f4bfaa1300c26925a8729cd32915bfc966086f0d5560bbe32a598c22adfb48cef72ba5d4287c0cefbacfd8ce195b4963c34a94bba7a175dae4bbe3ef4863d53708915090f47a068e227433f9e49d3aa09e356d8d66d0c0121e91a3c4aa3f27fa1b63396e2b41db908fdab8b18cc7304e94e970568f9421c0dbbbaf84598d972b0534f48a5e52670436aaa776ed2482ad703430201e53443c36dcfd34a0cb6637876105e79bf3bd58ec148cb64970e3223a91f71dfcfd5a04b667fbaf3d4b3b908b9828820dfecdd753750b5f9d2216e56c615272f854464c0ca4b1e85aedd038292c4e1a57744ebba010b9ebfbb011bd6f0b78805025d27f3c17746bae116c15d9f471f0f6288a150647b2afe9df7cccf01f5cde5f04680bbfed87f6cf429fb27ad6babe791766611cf5bc20e48bef119259b9b8a0e39c3df28cb9582ea338601cdc481b32fb82adeebb3dade25d1a3df20c37e712506b5d996c49a9f0f30ddcb91fe9004e1e83294a6c9203d94e8dc2cbb449de4155032604e47997016b304fd437d8235045e255a19b743a0a9f2e336b44cae307bb3987bd3e4e777fbb34c0ab8cc3d67466c0a88dd4ccad18a07a8d1068df5b629e5718d0f6df5c957cf71bb00a5178f175caca944e635c5159f738e2402a2d21aa081e10e456afb00b9f62416c8b9c0f7228f510729e0be3f305313d77f7379dc2af24869c6c74ee4471498861d192f0ff0f508285dab6b6a36ccf7d12256cc76b95503720ac672d08268d2cf7773b6ba2a5f664847bf707f2fc10c98f2f006ec22ccb5a8c8b7c40c7c2d49a6639b9f2ce33c25c04bc461e744dfa536b00d94baddf4f4d14044c695a33881477df124f0fcf206a9fb2e65e304cdbf0c4d2390170c130ab849c2f22b5cdd3921640c8cf1976ae1010b0dfd9cb2543e45f99749cc4d61f2e8aabfe98bd905fa39951b33ea769c45ab9531c57209862ad12fd76ba4807e65417b6cd12fa8ec916f013ebb8706a96effeda06c4be24b04846392e9d1e6930eae01fa21fbd700583fb598b92c8f4eb8a61aa6235db60f2841cf3a1c6ab54c67066844711d091eb931a1bd6281aedf2a0e8fab18817202a9be06402ed9cc720c16bfe881e4df4255e87afb7fc62f38116bbe03cd8a3cb11a27d568414782f47b1a44c97c680467694bc9709d32916c97e8006cbb07ba0e4180a3738038c374c4cce8f32959afb25f303f5815c4533124acf9d18940e77522ac5dc4b9570aae8f47b7f57fd8767bea1a24ae7bed65b4afdc8f1278c30e2db98fd172730ac6bbed4f1127cd32b04a95b205526cfcb4c4e1cc955175b3e8de1f5d81b18669692350aaa1a1d797617582e54d7a5b57a683b32fb1098062dad7b0c2eb518f6862e83db25e3dbaf7aed504de932acb99d735992ce62bae9ef893ff6acc0ffcf8e3483e146b9d49dd8c7835f43a37dca0787e3ec9f6605223d5ba7ae0ab9025b73bc03f7fac36c009a56d4d95d1e81d3b3ebca7e54cc1a12d127b57c8138976e791013b015f06a624f521b6ee04ec980893c7e5e01a336203594094f82833d7445fe2d09130f63511da54832de9136b39f4599f5aa5dfbb45da60cdceab7eefde89be63f3f7c0d2324847cce1405def7c469b0e272494e5df54f568656cb9c8818d92b72b8bc34db7bb3112487e746eefe4e808bbb287d99bf07d00dabededc5e5f074ffeae0cba7da3a516c173be1c513323e119f635e8209a074b216b7023fadc2d25949c90037e71e3e550726d210a2c688342e52440635e9cc14afe10102621a9c9accb782e9e4a5fa87f0a956f5b";
        let t: Transaction = raw.into();
        assert_eq!(t.version, 4);
        assert!(t.overwintered);
        assert!(!t.has_witness());
        assert_eq!(t.inputs.len(), 0);
        assert_eq!(t.outputs.len(), 2);
        assert_eq!(t.shielded_spends.len(), 3);
        assert_eq!(t.shielded_outputs.len(), 1);
        assert_eq!(t.join_splits.len(), 2);
        assert!(t.zcash);

        let serialized = serialize(&t);
        assert_eq!(Bytes::from(raw), serialized);
    }

    // https://github.com/artemii235/SuperNET/issues/342
    // Some CHIPS transactions have txversion 2 but no join splits
    // http://chips.komodochainz.info/api/getrawtransaction?txid=a23bc182ceacf7fa631f013e6a49e532f88f33db4a825f0684d069e8d3fa6c41&decrypt=0
    #[test]
    fn test_chips_tx() {
        let raw = "020000003688cdc6f58544a136f08cd2f6a955441dafd7a990fc8987f07f240f20e7d153cb11000000484730440220212d55322ea4099baf9460231dbaee956930a9ef4fe3b7c48df4439b512bdeec0220281330266c6ed28f0c07c59b51fb291a2cb819ffdd762f2c376043609f578c3501feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca8841000000048473044022007e98691ac94393c398961735207dcea8777b369e1cf80c994b873b361c2c99402205e0e77e1214983a88136d04420f59e936186b266cf7bb7136936f15d8d4cafbc01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c6040000004847304402207b769e5edebfe358d4b8432a1b49a55fb7fda68738b592c6beda3c9bead5a25402207a7c584f517c81fcfa8d664dd39276b9b511556341db4a7fda77212d9cc0e03101feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c60a00000048473044022011fb466e3f5e41d6b8986c26187700525525bce3b0b82e509eb0b3f73148212b02206df27c40df4b5bfa24a6d3ade17446c6e4cf4a00b58fc69b262054bf314c5e2f01feffffff88cdc6f58544a136f08cd2f6a955441dafd7a990fc8987f07f240f20e7d153cb1d00000049483045022100bb1e8c318f9f9f11364d105b893893422264e75cc60c852652ecd4490c3e825702202367a1664a90e5956ea3504e77b78a651d82c7942d0ee8fa96113aabca924bc901feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca884070000004847304402202bfbac9c8ecd388b269708ca05b3e67d5e10c714e367719e6558f4df3167bb77022041385673dd7bb139e5e3e0483e5ea6af7fe49ba6bb458a5e57c17156c004a3bb01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c61200000049483045022100f39f0f024b1a98f7d6a2df9c452bd7fc3bec85504a1dc0c75a80bc5ee9dd27ee02202f2e0fddc727e2970443b32179c002eef2c6a65aae6a0b3770b611c43aedbc7501feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca884000000004847304402205591e075d9b09685b87e8d58b8abdd207cf00a5ea6d343c67c44f50da476af6c02200916ff80b67d8271e8a96e0ea06db82d7ad780409db9a937cf45da926743f16e01feffffff88cdc6f58544a136f08cd2f6a955441dafd7a990fc8987f07f240f20e7d153cb050000004847304402205eb4404d70709c1ab861e1f497c93a1c8094b355eb6cee968d7fa3383989ad590220658adb9ef08a85849098607f235b426db292ef9e548a3fb20be964465ca981bf01feffffff88cdc6f58544a136f08cd2f6a955441dafd7a990fc8987f07f240f20e7d153cb1300000049483045022100e5910f6f245e0b079ef42e5437679e2648a45038de86e988e319559e38df01db0220480bbb57bb408cdf36bd4ca0cb4a4fdcbb670a921fc15abf9f8ba002d1c983b201feffffffedcbe806af5f3aeea510a2257ecc386c7ce3a46587c33d9a1c1d098edbbdac7b1a0000004948304502210080fc3c3e7b7f2248345806d6256b6eadbccea899f9ccf0af85db3e29d9663252022025764f9891c01a38edc3fa40acf1a8887ac8b143ed485e54685e98f7621f5ddf01feffffff88cdc6f58544a136f08cd2f6a955441dafd7a990fc8987f07f240f20e7d153cb020000004847304402202eee2ec7e2881ebbf346ac249e50b9300ac77729bc85a85bd4b356e6843cfb63022008b40c544583d781fbf7335bd8ff86c72af0f130b2886d615d52897559347c8001feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca884130000004948304502210097f1ad88807d8e078406c7020feb763e95bbd2c9db9b6520ad035ddb7be744ac022016ea51c14cc866ec0d4974e7e6eefe963812d11ca1b18412b5bdc5f14cb234e501feffffff93a064ad67dcb4b6b62a873254a7a7e99ccd2ac22a20afa7934f2ee1b7feaab20300000049483045022100bb006fe1d5cc3d9a2c965cd682e1bada78375b3679d16923bc8dd3d75c8bc42d02205b664654ea1b692e127582649e14d6643393a1695c8c6e03152db0f95d3234e901feffffff3e320cdac2a21aa82a5f1109676e4e2cfcfe8490f5d10e520f077d8b0aa3ffa80700000049483045022100f91c3bc5a4da127f0edbff186da1630a55d81a7e23f28757c98b957fba10115402202140c0498da21c17ff4c369de9004d801988fb27fb67ea8a71b9b5e475314dba01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c60200000049483045022100ed4a197e2abd7c3b3f7b06beed9958ae2bfb8e697900936e1ef043cecb3b6ee202206df202dae2993dbf27a7a6d46480e10ca39207e4d5d1a319df87e05427a1f6cb01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c61d00000049483045022100a98b630bba8a2aee9b7117b48fe5296b6c0544012ac1603a1231e95f9f9d012e02205f99d45289a947edd267b71ce23217136e191f584ed2fcd99252a2dc39ad632e01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c6170000004847304402204229e93ee6c9d2c7a61fa4373bf3c9b2f83d93e00008ff22ab46779c3c675ddd02204cacd5b94ecd9365773d646eeb682188ae5c976f83c719d4bc72fd18c6ed15c801feffffff88cdc6f58544a136f08cd2f6a955441dafd7a990fc8987f07f240f20e7d153cb0b00000049483045022100c53b15a61c65096083292c062dfe752edeaa8de852b8a1f746c984e72f6e022a022006b7e4aeed701150bec809a9d4a994145e839bc87e0578f9431921cd6097592001feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c60c0000004847304402200224cb015011f3db6994cc96a0ca25d1fcdfdcb86bf64ad73549a86648c7e7c6022025b229f30ebcf8ca6cd87d7ef006eaaecfcbd8eda8acb618e9661f3a322270e501feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca8840e0000004847304402204b5fe1947944e59e5705203b839cf4b52aaeffc3fa3a9d5b5c1d5397d76e3b9a02204b62801a39b4476a2b25adca77195bb159e85cbaf05dddcad8b081850158986501feffffff88cdc6f58544a136f08cd2f6a955441dafd7a990fc8987f07f240f20e7d153cb0c00000048473044022004bccf69cb4924e38f5d42b03e7d83bc2f20dddd03c3e1e26caa5e4182cb1c11022015943d3e9aed5c7ad771f17dbf6cf4bebc7e045443d4e63c1219578c5a3dfb9f01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c61300000048473044022046dd8b748935d89c8f2d0a555fd2655ca9e99506c5ed5eb5038fc7bf93c95a8502201e98ad259b1ea6598ae1c38b93807348fbdbce0d85f23cc294245a7d1f81730c01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c60800000049483045022100b8cb28788d6387f425629c742329eadf9b427161a597a09f94195b23418d92d102201e5badcd0cc778f226eb9284a50a544714dd8643daa826a28b2e3c21c4c38c0a01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c60d00000048473044022036ad533a558e18071e81305ab662e65c86b630e42ac69ee89d465af3f55fb8a102200e2f932ddf42f2d50de951b7ea331bdba4e013ccc46bf1af64bb2875cbd0eb5201feffffff88cdc6f58544a136f08cd2f6a955441dafd7a990fc8987f07f240f20e7d153cb1c0000004847304402203be0d84780038093dddaa5fd4dfabaac8cb97cb7c6c32cecc42240028842ac5b022008f48e363f4a46cec1e7f4395149bc38b4d91614ae13038b67cab35068df46d801feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca8841b0000004847304402201aeff3d96874e9c4a0890f054970ff0fd0ea879b13e875580d98b06ba4a328ac0220441ee724e33b7ab860c062a4572d8e804630b8583a96c6c707089f1500a25e2d01feffffff93a064ad67dcb4b6b62a873254a7a7e99ccd2ac22a20afa7934f2ee1b7feaab20000000049483045022100b0b168d58e17ec786542de020baa845089ba6763ef08bb21502cdc7a0cac531e022033b2cf852f29afcaf4b950b6d92fb49f7a00d9867a28c15dd1838eb341ff248801feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca8841800000048473044022007e267d17ef56ccfe56047c1ad62d7eb36bada154280e95de809ca831a61dc8b02205b2b264adcafaf3eeea2e3679093a860139434371a5ee865e4fcfa78f05abd4801feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c609000000494830450221009714a50b44d39cd59f1200dc67c2c02ef23b230b2d54195939e1f1fcd8e176440220719e3fc7900f50aef00967591a773db1f697a06bc457fac62c161be43e51277801feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c603000000484730440220048c9f7bee3732a7af64feaaf50f986976b9cf68bc67785a7664ba82369a6926022034a9cdb40c3af4191cab0ac9f1c698aa188e8ec5665d11fbcf999a76b644655501feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c6140000004847304402203aee19f809d05eb18c7c0c491d8742493f4711e537513f4af90fda3fe968e94502203896a15bcb13423fd80399a103fcdcbf254958897366c1d49dad7cac1283e6cf01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c61a0000004847304402200156377c553347980b62ec07ef6cbda4a56f6b7fa35f12b198a515c667b3da7802202a2fe14888221c75de931291cc97994198d1ead7baaaeec84cce1de84c649fc801feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c61c0000004948304502210096a57c4dcc3e07e653db8cdf785ff124e2a33360a056b036c041cbf697f1eee202204a691f529fea65cea09e688c571d1131c10fdae38505efa8ac93938e1f798e6901feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca884010000004948304502210093bbbe402d37137a7f2ac4cd31a8c27af05d47cabb9e77f4adc93fb25a5e000e02200e3ca9abde41b46016581cdcd9c4ccfc96d2eb40ba55b62b5b28fb111e6fbc0901feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c60100000048473044022015c69ac6f59d81f06aad0a24f85253cfb098d7c036afb3cf923d20ac1f30f5a10220585f1ce46096fa489ed5759c8bcd55b02166686bb6499eb1156e9a22c79fc68401feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c60e000000494830450221008db34667c2ff0b73c8c7d96a7be69f4967108cf442fb4612b4c2905a0525158402200eec1af1954473e6e4cd5d2acaeb9c51f9b7c9a951079d1ce851b08a4053148801feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c60500000049483045022100e163833f7f757e1f067adf9ab98d1f9db2d4b65c3d5447ea5fd861433e3a25240220762a998c45dbae90cfbc2236c2f4f44edeedb6339c70c325c88b4a77986188a901feffffff88cdc6f58544a136f08cd2f6a955441dafd7a990fc8987f07f240f20e7d153cb1600000049483045022100fe54fca2ddec5146b346787e43d9ba5431ed299bad01c23aa2618a6954c5dd87022046ebfe7d183a2248cfe60e41798c5e722d101c5c9a9884c7f76427d126baa5be01feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca88412000000484730440220075c4fb5c9aa29b41388bcc1763d50e11eaf3b5de095eda124f4bfab91bf273502203b09e60aa6de4772e603af5968e296e9374defc963d283bb5d9237a3f1e2773201feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca8841400000048473044022069e55af13c855836276182e14d681dabeeed5d42adcea871b6e839ca10cb450402201780cb6d139254bc07ba66ef53a5a8c527dfeb854e9d7385dbfb82ee5180d47a01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c61000000048473044022024410f4b60aad3279bd492051a15fb9e60c1694515512eefcd62343bcc9b7b4902200dccd00fa8f199932d8461e665743c826764ee145d3e38b22de4f5199eca3c5101feffffff88cdc6f58544a136f08cd2f6a955441dafd7a990fc8987f07f240f20e7d153cb19000000494830450221008dddaa8b4b031080cc5fa00a81c1c16e185887432824412a9fb8ecf681cbdb2402207cd5aff78c245bdb9980d3da0833c87cdab35cf0a3edede4f396d181fb179d8c01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c61800000049483045022100af56a39ed2e9d84821ca0da33e0107b2149738902b54250353c1b85c9ccb9bec02200260aa14677473c08c3f976071ddcefbec067d563acf48451137b22b83b4095001feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c60b00000049483045022100a6a514337a3cbf91ac95555b5d4e3cc342e8210b293c6228e6af58977b49fd1d0220667427a6fba74791389324cadecd9bbd69dde2af9adc9878de3fea5c18c6340b01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c606000000484730440220291146e6f74d3bdf472d5db27145b937435ecc242d417f17755744938baaf68802206b84bf80fe2912128234ae681d6b4b7c361a74a743ef6dfd5278da08e8e1b83b01feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c61b00000049483045022100c134a7c54aa4b378542a2f097863035c65a0a18a47fe5dbbd5bd5dea25d7269b02205d0ea88f52c90cd26aec208231a7114d5604ba6ce7015bc5f342ce908b2e006a01feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca88403000000494830450221009dfd91ceb134d05096c58630ca783ec63b79f7a28265cf036acfeaa206f3ab8702204e2660d8223ae422eba36c7aee934bf0255a617c0696373cd4e4edf98c19de9701feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c61500000049483045022100a32a639ecd654827e98b96fba3178fb4dfefe4b698623106690570764c81e768022009aa53edd6e2d07d35450dc7580c606964730f165ca1e741b3047bd323d98d9801feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c60700000049483045022100deabb0b694d4672663cf6b9c99ccb00d5f14a4593d81a2f606bf3aab1d0e2f3f0220108a6df56ee76884cf6289cf431efbf36c184f023e35634f3b0bc6976cfe3fc101feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c600000000484730440220301eb19148cfb9d1cb7e22acef271558479deb4ad28508fe5274f022215f708a02203c52d53627e81466b2bc907ce91253d8d463728fd139de120d7ca15cd7373b5701feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c61e0000006b483045022100d4041d5eeacd33c903a6154aaa3505687ba5c902d0e3749f4cfb82a17d1f7a8002204845d824e853e564cd6ca496a8359124735010e6842a94147e6fee5458c0b276012102858904a2a1a0b44df4c937b65ee1f5b66186ab87a751858cf270dee1d5031f18feffffff391dbed35e0b9e980310389379f1e69ed1a3419fac2c3adaf73c7a03474727c60f00000048473044022037b58995339770de6569d56fdad073b28005efde4cabbbe54a346353a5dd912202205a608bb8c7f43f8e71046a86be50cd3545c7ec031cddbb39e15d4a30b16e59ba01feffffff71ed3c90632fa8b3cde51c81355a7736eac3480bf0ab382551823968d8eca8841d000000484730440220122b82b2c0743418e1e66d2fd883347808da253863be736060aa26a20e0e8fdf02204c52f47b42dff36416051cdf2530a73d47996b9824fb98af4feafe8f76f3ba7d01feffffff01d06d4405000000001976a914ed56f2feb905938b45d2e009ec14add0e7d0157288ac226e2a00";
        let t: Transaction = raw.into();
        assert_eq!(t.version, 2);
        assert!(!t.overwintered);
        assert!(!t.has_witness());
        assert_eq!(t.inputs.len(), 54);
        assert_eq!(t.outputs.len(), 1);
        assert_eq!(t.shielded_spends.len(), 0);
        assert_eq!(t.shielded_outputs.len(), 0);
        assert_eq!(t.join_splits.len(), 0);
        assert!(!t.zcash);

        let serialized = serialize(&t);
        assert_eq!(Bytes::from(raw), serialized);
    }

    // https://github.com/artemii235/SuperNET/issues/342
    // DASH transactions have txversion 2 but no join splits
    // https://api.blockcypher.com/v1/dash/main/txs/d49504f35a67e1ec8b2eabba80cc0929a8845d255656049599b96cad54016215?limit=50&includeHex=true
    #[test]
    fn test_dash_tx() {
        let raw = "020000000f766b14021dcac10393def8b3848648b323d5dc6911b79c049ae85d8f680a7c06000000006b483045022100c59b5a0019a7fd1d39a8a9a51c4c704c20b90ef925b5c0c6d752d2d38829d10602203a25c07c7007ea420e4be1e133c386bceb9b327fb063e0aa1ecc99e31b23868a0121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffff392fe2195f4386e262b5a7bbcd678c4d39cb6eb467ac67b00f7838bb0ab02317000000006a47304402200f3ec53f2d497ea65652fcdc9a9be44d3e6b7a03f01cec1a1a7963ac780a8df6022035fc2cc170b572e5e3e55de417507e98be40a02be98f19e08e1f47e09b92953b0121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffff3ea478172c3362377b0922ec240fab0efa8d5259dbda663025fb030dbf98bb1e000000006a473044022009d8b6cd31dc564c89fb113177698769d3cb7c4cb0c7b85e37a06363c011909a0220075bca6ba83144daf7b27d1fbb3d268b8451448000f9f5250e1bf7edbd10bc980121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffffc8a26222e346959c5af54826f5d881012ae40dc0efa4d9293faa82718c32e62a010000006a47304402207a9959072e9d96856cf9cb11cc0fc4f851890cb24b7d960b3a8cfc65ccdda22602201a295abf40cf461c2afc5126708dfcec753eac9d56f599c6bb2cd50cd86954d20121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffff753f5200a83aed46f6d3f7ececadaca7feab73d8869abbeb894dc3adca3ca233000000006b48304502210085848455f07db8313d45f1b82a56b46ef3a914403166ce08083802ce624f34f6022003066377af5256bc23fcc3c377f57a27d56882b81eae5cf49cba0041a0b7d37c0121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffff73b55ca45b63deff02c63e5ecc4c90bc38df048be4187e2ade72754b516ce03a010000006b483045022100c0e1c59cca9880af7b6ed26889e65fc45e30dd1c56a634be69ff8f52539d89fa02201ef5ad60d6d8bd398e23b036ec30e46c6b912b492330333493548492d717c8810121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffffe96705773a9c74e2b00a5189750352c48f29d9b657d5376b66030828739b8747000000006a4730440220737c29d304ae8c7dad7752ae86fda6b3ab0b48886cdb59930d8736482954abcf02204f8a057143a99077e8dc4c4ae3abcc8141b15312a531dc8fc242f4263d4a4bf80121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffff5fd4ab80dc2e428296786a05753348d3ff6e37afb869ad05a1950ac1a170f85a010000006b483045022100ab3d8919e944be10cdfdefac794547c90298579c2beb0fdeb2d7b27a8a22ac2c0220740d4374edb29557ea3215e44cb4d9d6853a7c81c01be9dc95055382b09bfb5f0121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffff7e944d32282a7a22c77c9150445a03e8f01b7a7ea8a1aca76d4c2711826ffe69010000006a47304402206c8d9422c7014b17fc2c28c91471b7b72eae2b474d2ad879d502c7f2a1a587b9022078ab5329fb1d5b3721feffeecfe049e7abeeb8d73ddfdac98520241126014c380121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffffa7eb559328c2ee7803e3265eecc9359e9d1d0226c9687e52ffa58d2e87c0636e000000006a473044022055d2bc798d849fb72da63f193225e0e0a3fd46d03c72275e3315d9b1374333e00220151cfbfb1e8774ce793a5ad4d3566d730723c9adbb55595a72f75f294e82e8c30121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffff26c2272f0e621015171f280f7fd68774da0c4c806f810afb2787ba6317ae44d8010000006a4730440220681d867bab4b351dfd6c90e33bc9f12a96d8a27ec646a48ee4a43432bbe01d1a02202f92653beb96e8db7dd7934992654813aae537ea073e6ab7e77f20897942e4320121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffff0e9875013fad0dffb0434057df5ac6cab88b9c2198f62ce1a137ec6a142150e3000000006a47304402204a578d26172c8b8548e4bf653e9dca3f98c81d99e6db8d0a6ea905451bcacb8002202acaf1f8e21d5dd7f3cbe24810c81f2de391aaf3c5b96eac787bc78cead068360121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffffb26f49180dfb0047320bca50c501b05788d4a5f807197410734817a08d742ce7000000006b4830450221008b600c65ca7e9051636ec7f9ab7ece4a5e494539722ee06cf73c0f608be06f26022060606134d63f706e6d8fe15a6c54cdbc7738bdde1f0e86ad8c8c5f5999ec0db10121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffffee8f01057ead1a71d2fbe87442cc7c3817149c4de95682d94ec66ba174a80dfa010000006a473044022001a108cd275353da0cb11bb0b379fe29db52dc020671485d1a6bfd4fdc7658ea0220125fe028f97500dd80c80b0cfa7637c7fa4dc0f94f4f3daf865d3fdcf6ba7df80121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffff9ebfe0b6c3316e6c3cd5471744b5b019c525fd9b59b2865c6c4d422fe63277fc000000006a4730440220109050a4041a7a097b42ddb94c2fbc5551e92367e0f0658dd244d4c34cd2ce7702206501d9327cb3c25988e6a899334aa96523432f59cb72a63669168567debd48fd0121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8feffffff019505a003000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88acc2e70f00";
        let t: Transaction = raw.into();
        assert_eq!(t.version, 2);
        assert!(!t.overwintered);
        assert!(!t.has_witness());
        assert_eq!(t.inputs.len(), 15);
        assert_eq!(t.outputs.len(), 1);
        assert_eq!(t.shielded_spends.len(), 0);
        assert_eq!(t.shielded_outputs.len(), 0);
        assert_eq!(t.join_splits.len(), 0);
        assert!(!t.zcash);

        let serialized = serialize(&t);
        assert_eq!(Bytes::from(raw), serialized);
    }

    // http://explore.myce.world/api/getrawtransaction?txid=248b2cadff69bb58f3232b914d32588cd9cd014d4f3dc29cd39d1914bf1d7f43&decrypt=0
    // MYCE has txversion = 3, but no Zcash upgrades
    #[test]
    fn test_transaction_serde_tx_version_3_not_overwintered() {
        let raw = "030000000145f09710b0d6ff73a52bffdd1661f2f001783fb6f947ecf253462359dca19e990100000049483045022100e2f6183e2008e6b0aa31f728f289c66436bf4d4be7aedfe0c3f582e60d16443e0220741548d2cee78a2b39a8e1146b131a69211da025ff0859dba60e38b12a46a0b501ffffffff026c39ea0b000000001976a9142b79bc408688f48858083de027a1b42ed3e39da188ac380265d9450000001976a914066baabb56dc1588afd7fa83e0ffd4729aee89d588ac00000000";
        let t: Transaction = raw.into();
        assert_eq!(t.version, 3);
        assert!(!t.overwintered);
        assert!(!t.has_witness());
        assert_eq!(t.inputs.len(), 1);
        assert_eq!(t.outputs.len(), 2);
        assert_eq!(t.shielded_spends.len(), 0);
        assert_eq!(t.shielded_outputs.len(), 0);
        assert_eq!(t.join_splits.len(), 0);
        let serialized = serialize(&t);
        assert_eq!(Bytes::from(raw), serialized);
    }

    // https://chainz.cryptoid.info/ecc/tx.dws?816906122e12c5b56a38f169aa2bdccb1e90f4e0d78a3777b60b262883132602.htm
    // Deserialization of this ECC transaction failed
    // ECC is PoS coin having nTime field in transaction
    #[test]
    fn test_transaction_serde_ecc() {
        let bytes: Vec<u8> = vec![
            1, 0, 0, 0, 70, 254, 168, 92, 1, 170, 99, 80, 219, 121, 123, 10, 150, 232, 96, 154, 102, 242, 208, 96, 100,
            59, 114, 52, 38, 97, 143, 194, 239, 6, 154, 4, 232, 82, 124, 189, 240, 0, 0, 0, 0, 106, 71, 48, 68, 2, 32,
            75, 18, 92, 56, 109, 69, 254, 77, 185, 43, 157, 13, 166, 30, 129, 30, 185, 72, 161, 125, 37, 134, 120, 218,
            213, 146, 229, 8, 117, 133, 164, 38, 2, 32, 40, 91, 86, 89, 107, 96, 15, 202, 12, 124, 168, 252, 75, 139,
            191, 93, 216, 144, 212, 58, 159, 166, 64, 202, 72, 155, 182, 222, 42, 140, 167, 128, 1, 33, 3, 148, 13,
            224, 176, 222, 92, 35, 122, 18, 78, 113, 66, 51, 158, 172, 225, 229, 41, 119, 44, 212, 117, 176, 232, 66,
            250, 100, 75, 202, 254, 73, 204, 254, 255, 255, 255, 2, 193, 198, 45, 0, 0, 0, 0, 0, 25, 118, 169, 20, 131,
            5, 22, 126, 249, 90, 27, 30, 154, 205, 246, 52, 167, 104, 108, 183, 105, 147, 64, 106, 136, 172, 127, 132,
            30, 0, 0, 0, 0, 0, 25, 118, 169, 20, 195, 247, 16, 222, 183, 50, 11, 14, 250, 110, 219, 20, 227, 235, 238,
            185, 21, 95, 169, 13, 136, 172, 238, 100, 32, 0,
        ];
        let t: Transaction = deserialize(bytes.as_slice()).unwrap();
        assert_eq!(t.version, 1);
        assert!(!t.overwintered);
        assert!(!t.has_witness());
        assert_eq!(t.inputs.len(), 1);
        assert_eq!(t.outputs.len(), 2);
        assert_eq!(t.shielded_spends.len(), 0);
        assert_eq!(t.shielded_outputs.len(), 0);
        assert_eq!(t.join_splits.len(), 0);
        let serialized = serialize(&t);
        assert_eq!(Bytes::from(bytes), serialized);
    }

    // Deserialization of this NAV transaction failed
    // NAV is PoS coin having nTime field in transaction
    // NAV coin tx also has strDZeel field that is not supported as of now
    // https://github.com/navcoin/navcoin-core/blob/85690b907f423fab48ee41dd1782f3ee9040d68d/src/primitives/transaction.h#L414
    #[test]
    fn test_transaction_serde_nav() {
        let bytes: Vec<u8> = vec![
            3, 0, 0, 0, 13, 96, 152, 92, 2, 20, 58, 107, 102, 116, 164, 26, 174, 199, 16, 166, 39, 126, 103, 203, 187,
            192, 176, 219, 43, 192, 73, 93, 118, 26, 134, 41, 28, 131, 123, 227, 220, 0, 0, 0, 0, 107, 72, 48, 69, 2,
            33, 0, 174, 215, 242, 173, 170, 178, 139, 171, 71, 204, 106, 251, 240, 134, 193, 51, 146, 91, 26, 42, 127,
            55, 199, 24, 179, 104, 243, 129, 216, 0, 7, 161, 2, 32, 124, 16, 163, 154, 229, 128, 110, 209, 126, 131,
            158, 197, 56, 183, 219, 22, 180, 14, 253, 114, 164, 98, 222, 137, 198, 145, 147, 91, 225, 132, 183, 56, 1,
            33, 3, 27, 184, 59, 88, 236, 19, 14, 40, 224, 166, 213, 210, 172, 242, 235, 1, 176, 211, 241, 103, 14, 2,
            29, 71, 211, 29, 184, 168, 88, 33, 157, 168, 254, 255, 255, 255, 85, 253, 74, 79, 211, 120, 236, 109, 192,
            55, 203, 24, 96, 189, 156, 22, 227, 112, 74, 210, 217, 189, 130, 89, 76, 62, 204, 212, 95, 91, 175, 250, 1,
            0, 0, 0, 72, 71, 48, 68, 2, 32, 110, 46, 42, 223, 247, 151, 62, 91, 112, 45, 109, 158, 199, 116, 13, 53,
            155, 181, 34, 41, 40, 178, 212, 255, 22, 217, 222, 138, 69, 208, 187, 55, 2, 32, 21, 234, 176, 205, 2, 222,
            232, 108, 28, 245, 211, 133, 46, 62, 145, 17, 75, 45, 69, 171, 113, 113, 247, 160, 189, 229, 87, 139, 217,
            125, 22, 139, 1, 254, 255, 255, 255, 1, 60, 143, 6, 192, 7, 0, 0, 0, 25, 118, 169, 20, 195, 247, 16, 222,
            183, 50, 11, 14, 250, 110, 219, 20, 227, 235, 238, 185, 21, 95, 169, 13, 136, 172, 64, 143, 45, 0, 253, 88,
            1, 71, 57, 50, 106, 117, 65, 47, 83, 104, 110, 69, 87, 69, 120, 116, 48, 82, 47, 90, 57, 100, 118, 50, 77,
            55, 77, 119, 88, 79, 122, 56, 115, 88, 82, 78, 111, 57, 53, 107, 81, 84, 57, 80, 86, 97, 53, 52, 98, 73,
            77, 73, 111, 82, 77, 55, 47, 100, 68, 78, 112, 104, 82, 78, 90, 51, 52, 97, 108, 73, 47, 76, 70, 88, 53,
            120, 80, 86, 75, 71, 100, 74, 116, 117, 90, 51, 115, 109, 122, 84, 84, 75, 76, 89, 109, 78, 75, 53, 104,
            117, 72, 87, 74, 66, 106, 81, 71, 108, 116, 50, 90, 69, 100, 69, 82, 67, 119, 122, 77, 74, 115, 75, 82, 72,
            90, 107, 104, 48, 43, 103, 67, 116, 114, 79, 53, 75, 116, 84, 89, 119, 79, 75, 66, 75, 108, 74, 75, 89,
            113, 107, 66, 120, 97, 80, 107, 47, 68, 76, 52, 110, 121, 53, 113, 98, 88, 57, 90, 57, 66, 74, 98, 104, 52,
            122, 105, 109, 70, 116, 70, 75, 77, 43, 100, 47, 102, 55, 54, 68, 43, 105, 117, 106, 87, 102, 100, 85, 88,
            103, 79, 107, 86, 67, 97, 116, 101, 68, 115, 79, 47, 108, 50, 72, 50, 79, 66, 86, 88, 70, 76, 100, 49, 113,
            110, 87, 106, 75, 98, 98, 85, 79, 49, 88, 51, 80, 75, 120, 122, 105, 106, 97, 117, 90, 68, 68, 107, 76, 90,
            49, 113, 72, 47, 83, 66, 88, 107, 43, 52, 101, 118, 43, 102, 52, 51, 109, 83, 100, 85, 67, 116, 57, 112,
            75, 86, 79, 49, 107, 68, 97, 88, 69, 67, 104, 51, 71, 100, 119, 88, 105, 100, 111, 56, 102, 121, 48, 51,
            66, 78, 49, 55, 82, 118, 66, 115, 78, 111, 54, 76, 102, 57, 113, 48, 107, 65, 76, 77, 97, 101, 97, 122, 99,
            70, 102, 122, 57, 65, 112, 67, 108, 87, 51, 47, 70, 118, 65, 121, 115, 101, 84, 119, 54, 113, 43, 65, 61,
            61,
        ];
        println!("{}", bytes.to_hex::<String>());
        let t: Transaction = deserialize(bytes.as_slice()).unwrap();
        assert_eq!(t.version, 3);
        assert!(!t.overwintered);
        assert!(!t.has_witness());
        assert_eq!(t.inputs.len(), 2);
        assert_eq!(t.outputs.len(), 1);
        assert_eq!(t.shielded_spends.len(), 0);
        assert_eq!(t.shielded_outputs.len(), 0);
        assert_eq!(t.join_splits.len(), 0);
        let serialized = serialize(&t);
        assert_eq!(Bytes::from(bytes), serialized);
    }

    // https://kmdexplorer.io/tx/88893f05764f5a781f2e555a5b492c064f2269a4a44c51afdbe98fab54361bb5
    // KMD transaction having opreturn output
    #[test]
    fn test_kmd_transaction_with_opreturn_output() {
        let raw = "0100000001ebca38fa14b1ec029c3e08a2e87940c1f796b1588674b4c386f09626ee702576010000006a4730440220070963b9460d9bafe7865563574594fc3f823e5cdf7c49a5642dade76502547f022023fd90d41e34e514237f4b5967f83c9af27673d6de2eae3d88079a988fa5be3e012103668e3368c9fb67d8fc808a5fe74d5a8d21b6eed726838122d5f7716fb3328998ffffffff03e87006060000000017a914fef59ae800bb89050d25f67be432b231097e1849878758c100000000001976a91473122bcec852f394e51496e39fca5111c3d7ae5688ac00000000000000000a6a08303764643135633400000000";
        let t: Transaction = raw.into();
        assert_eq!(t.version, 1);
        assert!(!t.overwintered);
        assert!(!t.has_witness());
        assert_eq!(t.inputs.len(), 1);
        assert_eq!(t.outputs.len(), 3);
        assert_eq!(t.shielded_spends.len(), 0);
        assert_eq!(t.shielded_outputs.len(), 0);
        assert_eq!(t.join_splits.len(), 0);
        let serialized = serialize(&t);
        assert_eq!(Bytes::from(raw), serialized);
    }

    #[test]
    fn test_transaction_hash() {
        let t: Transaction = "0100000001a6b97044d03da79c005b20ea9c0e1a6d9dc12d9f7b91a5911c9030a439eed8f5000000004948304502206e21798a42fae0e854281abd38bacd1aeed3ee3738d9e1446618c4571d1090db022100e2ac980643b0b82c0e88ffdfec6b64e3e6ba35e7ba5fdd7d5d6cc8d25c6b241501ffffffff0100f2052a010000001976a914404371705fa9bd789a2fcd52d2c580b65d35549d88ac00000000".into();
        let hash = H256::from_reversed_str("5a4ebf66822b0b2d56bd9dc64ece0bc38ee7844a23ff1d7320a88c5fdb2ad3e2");
        assert_eq!(t.hash(), hash);
    }

    #[test]
    fn test_transaction_serialized_len() {
        let raw_tx: &'static str = "0100000001a6b97044d03da79c005b20ea9c0e1a6d9dc12d9f7b91a5911c9030a439eed8f5000000004948304502206e21798a42fae0e854281abd38bacd1aeed3ee3738d9e1446618c4571d1090db022100e2ac980643b0b82c0e88ffdfec6b64e3e6ba35e7ba5fdd7d5d6cc8d25c6b241501ffffffff0100f2052a010000001976a914404371705fa9bd789a2fcd52d2c580b65d35549d88ac00000000";
        let tx: Transaction = raw_tx.into();
        assert_eq!(tx.serialized_size(), raw_tx.len() / 2);
    }

    #[test]
    fn test_transaction_reader_with_witness() {
        // test case from https://github.com/bitcoin/bips/blob/master/bip-0143.mediawiki
        let actual: Transaction = "01000000000102fff7f7881a8099afa6940d42d1e7f6362bec38171ea3edf433541db4e4ad969f00000000494830450221008b9d1dc26ba6a9cb62127b02742fa9d754cd3bebf337f7a55d114c8e5cdd30be022040529b194ba3f9281a99f2b1c0a19c0489bc22ede944ccf4ecbab4cc618ef3ed01eeffffffef51e1b804cc89d182d279655c3aa89e815b1b309fe287d9b2b55d57b90ec68a0100000000ffffffff02202cb206000000001976a9148280b37df378db99f66f85c95a783a76ac7a6d5988ac9093510d000000001976a9143bde42dbee7e4dbe6a21b2d50ce2f0167faa815988ac000247304402203609e17b84f6a7d30c80bfa610b5b4542f32a8a0d5447a12fb1366d7f01cc44a0220573a954c4518331561406f90300e8f3358f51928d43c212a8caed02de67eebee0121025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee635711000000".into();
        let expected = Transaction {
			version: 1,
			n_time: None,
			overwintered: false,
			expiry_height: 0,
			binding_sig: H512::default(),
			join_split_pubkey: H256::default(),
			join_split_sig: H512::default(),
			join_splits: vec![],
			shielded_spends: vec![],
			shielded_outputs: vec![],
			value_balance: 0,
			version_group_id: 0,
			inputs: vec![TransactionInput {
				previous_output: OutPoint {
					hash: "fff7f7881a8099afa6940d42d1e7f6362bec38171ea3edf433541db4e4ad969f".into(),
					index: 0,
				},
				script_sig: "4830450221008b9d1dc26ba6a9cb62127b02742fa9d754cd3bebf337f7a55d114c8e5cdd30be022040529b194ba3f9281a99f2b1c0a19c0489bc22ede944ccf4ecbab4cc618ef3ed01".into(),
				sequence: 0xffffffee,
				script_witness: vec![],
			}, TransactionInput {
				previous_output: OutPoint {
					hash: "ef51e1b804cc89d182d279655c3aa89e815b1b309fe287d9b2b55d57b90ec68a".into(),
					index: 1,
				},
				script_sig: "".into(),
				sequence: 0xffffffff,
				script_witness: vec![
					"304402203609e17b84f6a7d30c80bfa610b5b4542f32a8a0d5447a12fb1366d7f01cc44a0220573a954c4518331561406f90300e8f3358f51928d43c212a8caed02de67eebee01".into(),
					"025476c2e83188368da1ff3e292e7acafcdb3566bb0ad253f62fc70f07aeee6357".into(),
				],
			}],
			outputs: vec![TransactionOutput {
				value: 0x0000000006b22c20,
				script_pubkey: "76a9148280b37df378db99f66f85c95a783a76ac7a6d5988ac".into(),
			}, TransactionOutput {
				value: 0x000000000d519390,
				script_pubkey: "76a9143bde42dbee7e4dbe6a21b2d50ce2f0167faa815988ac".into(),
			}],
			lock_time: 0x00000011,
			zcash: false,
            posv: false,
            str_d_zeel: None,
            tx_hash_algo: TxHashAlgo::DSHA256,
            v_extra_payload: None,
		};
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_serialization_with_flags() {
        // tBTC txid: 245f0a072bed336be95cb2b5a7fb080cc4b57b95e1db7c3c4152d58705e3a72e
        let transaction_without_witness: Transaction =
            "0100000001d834ce92ae8493013133a217d6a5f42e9ccc303c23a34bf49a9394381659bd4500000000d7473044022000f1f6f036d14e1f68ca250c0f99d89385bed112254832416d965d07f6ca4c930220256ea7a5a459f406434a4cd7b9cba747c623cbb06e315e944b13152a8085851701205132177f18e6234f24c7489e5db9bcb506d3fa066c554845c8de456a61ebbb4a004c6b6304cc38cb60b1752102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ac6782012088a914c9db7afe749d2e94ee842ed2ec966e96e3508f6a882103c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3edac68ffffffff01df250000000000001976a9146d9d2b554d768232320587df75c4338ecc8bf37d88accc38cb60"
                .into();
        assert_eq!(
            serialize_with_flags(&transaction_without_witness, 0),
            serialize_with_flags(&transaction_without_witness, SERIALIZE_TRANSACTION_WITNESS)
        );

        // tBTC txid: 303d1797bd67895dab9289e6729886518d6e1ef34f15e49fbaaa3204db832b7f
        let transaction_with_witness: Transaction = "01000000000101b62722419e6529830e548eb7944b0bb8af2dfd102c30d9accc5be57da6276f320100000000ffffffff02e8030000000000001976a9146d9d2b554d768232320587df75c4338ecc8bf37d88ac3f7b00000000000016001405aab5342166f8594baf17a7d9bef5d567443327024730440220229efa569066639fd94d737d3981c7e1fe39e4605637b8c46c28413abb3e759b022034f2b1ea17481c6fb34c9074f8d1ab1c7cc1a2dcdfe7060c53a31fd5fc606504012102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f33fcec160".into();
        assert!(
            serialize_with_flags(&transaction_with_witness, 0)
                != serialize_with_flags(&transaction_with_witness, SERIALIZE_TRANSACTION_WITNESS)
        );
    }

    #[test]
    fn test_witness_hash_differs() {
        // tBTC txid: 245f0a072bed336be95cb2b5a7fb080cc4b57b95e1db7c3c4152d58705e3a72e
        let transaction_without_witness: Transaction =
            "0100000001d834ce92ae8493013133a217d6a5f42e9ccc303c23a34bf49a9394381659bd4500000000d7473044022000f1f6f036d14e1f68ca250c0f99d89385bed112254832416d965d07f6ca4c930220256ea7a5a459f406434a4cd7b9cba747c623cbb06e315e944b13152a8085851701205132177f18e6234f24c7489e5db9bcb506d3fa066c554845c8de456a61ebbb4a004c6b6304cc38cb60b1752102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ac6782012088a914c9db7afe749d2e94ee842ed2ec966e96e3508f6a882103c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3edac68ffffffff01df250000000000001976a9146d9d2b554d768232320587df75c4338ecc8bf37d88accc38cb60"
                .into();
        assert_eq!(
            transaction_without_witness.hash(),
            transaction_without_witness.witness_hash()
        );

        // tBTC txid: 303d1797bd67895dab9289e6729886518d6e1ef34f15e49fbaaa3204db832b7f
        let transaction_with_witness: Transaction = "01000000000101b62722419e6529830e548eb7944b0bb8af2dfd102c30d9accc5be57da6276f320100000000ffffffff02e8030000000000001976a9146d9d2b554d768232320587df75c4338ecc8bf37d88ac3f7b00000000000016001405aab5342166f8594baf17a7d9bef5d567443327024730440220229efa569066639fd94d737d3981c7e1fe39e4605637b8c46c28413abb3e759b022034f2b1ea17481c6fb34c9074f8d1ab1c7cc1a2dcdfe7060c53a31fd5fc606504012102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f33fcec160".into();
        assert!(transaction_with_witness.hash() != transaction_with_witness.witness_hash());
    }

    // BLK is PoS coin having nTime field in transaction
    #[test]
    fn blk_transaction() {
        let bytes: Vec<u8> = vec![
            1, 0, 0, 0, 162, 103, 223, 92, 2, 79, 49, 147, 29, 108, 253, 170, 24, 82, 136, 31, 202, 238, 165, 154, 222,
            15, 158, 154, 96, 150, 241, 135, 100, 57, 134, 205, 59, 158, 195, 187, 15, 13, 0, 0, 0, 107, 72, 48, 69, 2,
            33, 0, 241, 134, 33, 141, 133, 127, 125, 248, 164, 161, 111, 29, 251, 121, 92, 252, 149, 4, 154, 69, 79,
            88, 127, 240, 164, 50, 188, 50, 172, 90, 143, 96, 2, 32, 79, 50, 70, 183, 118, 116, 15, 50, 225, 17, 124,
            159, 80, 46, 221, 193, 119, 101, 103, 87, 97, 232, 200, 16, 26, 141, 152, 250, 118, 221, 90, 128, 1, 33, 2,
            243, 240, 208, 177, 243, 186, 213, 250, 153, 106, 191, 167, 111, 157, 109, 1, 123, 168, 144, 58, 18, 100,
            15, 42, 213, 185, 153, 37, 209, 197, 242, 75, 254, 255, 255, 255, 152, 209, 17, 248, 100, 77, 208, 109, 91,
            63, 188, 17, 50, 85, 64, 148, 201, 222, 117, 40, 176, 243, 134, 218, 1, 68, 203, 63, 131, 29, 220, 202, 0,
            0, 0, 0, 107, 72, 48, 69, 2, 33, 0, 253, 170, 126, 29, 64, 103, 227, 98, 71, 131, 253, 101, 78, 135, 207,
            193, 211, 154, 116, 64, 213, 152, 136, 251, 197, 164, 155, 134, 107, 237, 34, 241, 2, 32, 16, 6, 149, 129,
            154, 146, 137, 189, 250, 125, 163, 247, 238, 70, 34, 58, 227, 247, 198, 93, 108, 60, 210, 213, 128, 167,
            131, 201, 210, 172, 206, 27, 1, 33, 2, 8, 204, 95, 204, 180, 120, 66, 94, 251, 138, 12, 183, 109, 21, 90,
            210, 214, 172, 22, 178, 147, 29, 149, 41, 253, 105, 157, 87, 234, 50, 160, 76, 254, 255, 255, 255, 2, 80,
            164, 22, 1, 0, 0, 0, 0, 25, 118, 169, 20, 68, 133, 246, 191, 218, 151, 98, 0, 55, 226, 89, 137, 136, 58,
            126, 146, 73, 127, 96, 208, 136, 172, 192, 183, 226, 64, 0, 0, 0, 0, 25, 118, 169, 20, 195, 247, 16, 222,
            183, 50, 11, 14, 250, 110, 219, 20, 227, 235, 238, 185, 21, 95, 169, 13, 136, 172, 90, 91, 39, 0,
        ];
        println!("{}", bytes.to_hex::<String>());
        let t: Transaction = deserialize(bytes.as_slice()).unwrap();
        let serialized = serialize(&t);
        assert_eq!(Bytes::from(bytes), serialized);
    }

    #[test]
    fn malformed_transaction() {
        // due to compact integer representation the number of inputs calculated will be huge
        // resulting into capacity overflow error
        let bytes: Vec<u8> = vec![
            1, 0, 0, 0, 70, 255, 168, 92, 255, 170, 99, 80, 219, 121, 123, 10, 150, 232, 96, 154, 102, 242, 208, 96,
            100, 59, 114, 52, 38, 97, 143, 194, 239, 6, 154, 4, 232, 82, 124, 189, 240, 0, 0, 0, 0, 106, 71, 48, 68, 2,
            32, 75, 18, 92, 56, 109, 69, 254, 77, 185, 43, 157, 13, 166, 30, 129, 30, 185, 72, 161, 125, 37, 134, 120,
            218, 213, 146, 229, 8, 117, 133, 164, 38, 2, 32, 40, 91, 86, 89, 107, 96, 15, 202, 12, 124, 168, 252, 75,
            139, 191, 93, 216, 144, 212, 58, 159, 166, 64, 202, 72, 155, 182, 222, 42, 140, 167, 128, 1, 33, 3, 148,
            13, 224, 176, 222, 92, 35, 122, 18, 78, 113, 66, 51, 158, 172, 225, 229, 41, 119, 44, 212, 117, 176, 232,
            66, 250, 100, 75, 202, 254, 73, 204, 254, 255, 255, 255, 2, 193, 198, 45, 0, 0, 0, 0, 0, 25, 118, 169, 20,
            131, 5, 22, 126, 249, 90, 27, 30, 154, 205, 246, 52, 167, 104, 108, 183, 105, 147, 64, 106, 136, 172, 127,
            132, 30, 0, 0, 0, 0, 0, 25, 118, 169, 20, 195, 247, 16, 222, 183, 50, 11, 14, 250, 110, 219, 20, 227, 235,
            238, 185, 21, 95, 169, 13, 136, 172, 238, 100, 32, 0,
        ];
        let res: Result<Transaction, _> = deserialize(bytes.as_slice());
        res.unwrap_err();
    }

    #[test]
    fn biggest_btc_transaction() {
        let transaction = include_str!("for_tests/biggest_btc_tx_hex");
        let t: Transaction = transaction.into();
        assert_eq!(5569, t.inputs.len());
        assert_eq!(1, t.outputs.len());
        assert_eq!(serialize(&t).to_hex::<String>(), transaction);
    }

    #[test]
    fn firo_lelantus() {
        let transaction =
            include_str!("for_tests/firo_06ed4b75010edcf404a315be70903473f44050c978bc37fbcee90e0b49114ba8");
        let t: Transaction = transaction.into();
        assert_eq!(2, t.outputs.len());
        assert_eq!(serialize(&t).to_hex::<String>(), transaction);
    }

    #[test]
    fn firo_spark() {
        let transaction =
            include_str!("for_tests/firo_c50e5a3f16744ac86bacae28d9251a29bf754d250592bce16a953cd961b584d5");
        let t: Transaction = transaction.into();
        assert_eq!(2, t.outputs.len());
        assert_eq!(serialize(&t).to_hex::<String>(), transaction);
    }

    #[test]
    // https://kmdexplorer.io/tx/687acd73ad23ce93e7ddabeece8eb228a0a0e15e4d265f7c717d7458ddce9bdd
    fn kmd_687acd73ad23ce93e7ddabeece8eb228a0a0e15e4d265f7c717d7458ddce9bdd() {
        let transaction = "02000000000202ecf451020000001976a9140eaccdb0d80773734ebd6deab0b2d8ac1eed1e9188ac83349800000000001976a9145177f8b427e5f47342a4b8ab5dac770815d4389e88ac00000000010000000000000000c5629c52020000005d45bfb8839a898af4d22c3569923cd2017e4f574fd425ed6a35c9d2f746be10fecf8072db89387c1ae4b6a131dcb60541e512375c9250801133a7f599350f11435b0705a0a7bb1712fb224fe8d750b2881771481b6276bd680e75361e86124579575403b75b3ff60ee7c08a43ecf2a1a67e156e1bd2dc7c247f1ec4a712958c0c7b4f88c3aaee1661a5c26ed119ce08b25b65030967d078b6ebd5202878d27d64ceeb51139a3f86f40184f659878f0692cd863f76e8a0c18af2c09e1973176b87821b2b32e033833d28b1cfcc8ba2486aea8991de4cbb6e2634295956e341c35143c133f28ca978d3115b63c24e642b9f08cc61728e63a2cd51832a97c9482dfefedd12ad549d6791d6ccc4bb196ec433780680d8f9d5938e4c09cfe3701038031fbdede7125e5f92531d3374cc59eefbbd021118d0d7c992a9fe95b0027331e602154164bcf599f93ec90357c27ab68723d56ba812c9c05d2b4f053d077807f0c20a07f5cdc652851786fd3a2b5f9342a14c8cdba7dbac3bdb31ff64884853c64da56faf00bbad0b5010eb62a65fc5738a3a9bef7f14e7ce14c093efc0bdf7eef728030e45b1bf0419f025fe06918452ace10cc63be5e08f64e326f27989eac30de1de030850bab2d19f5f91073b45cf9be99d46f9f233856c75dd9ef4b8cf608e7853220317956376c1c3533ddca185af1b5da30e8cc95a0cb28463be840e24cc64e517c3030a413f7f670279e55dc2dac5092190c339e0769a25b50beba74098b7dfd9b232020d59ac11b915c4deb3cd6cd8e103952133a0da027837f8f4b8a35db033152fb85f7d001a14f0487fd9bd52c32ff3369bf4cfa0e4b7330d6ec94ea958cf5fa6c3481011e4e2e459deef5021b6e0c277bf1e11fc8fc0c7a5d779e34402d8a917d952520bb2492b36c5939a9c71920a12014794c50164e16dc1fb0dd9b47fd10c759c1c4f990ae17f04eae205b86a94ae8755cf5ea68bf9ee4963e3acc4b7211c9250772b9ddcf9cee78b37b4d398606e0b61d66798dc1fd409821dbd6e318853c4c9b5e0daa896dd4fb09cfad8ed081b26f4599b6124171588979035bfead194bb607a7fba38528d7a5c20439ba74b2720e5e50b1d3f34649ad8736599a395a8af4c1014997ae8de5ef3e42fcf3511859283b98674bcc73b3defc8423f82cbed23bf7cfcb1019c15d144b44001a61ab10f527369c55255528957d6a38077fa974ed14139e71adb2a1b17d6a377029189cc2e9a7196e367f7cdbc9af18cfc52ca381895cbe194a672bf5b988cec8fc53fc1eb6c357305e2f5aa43485408a3bfc0353ed8f277f0365af9acd0de3a6ab9e0eefc49d0edac4ac4e9aa42b19d38d9149d36c9cb7e5438d86c2df399ff91222fc7725b1868aa06bc2e109e4cedb2e4b071a667abc2e4c974edecac6626c14485dd36f6e53bb0ddb6bcbb7013310965de93302cc196aa26ab06c988cdc751eab3f621fb77316682a058dcd101b70d30fc5c12fc09ec356c6c25b9cda8d65dabc02f3760b90a48f3ffb16e54c1a98aebaccbf7cf20ba0df37c5480e475a1c451d41c63a1d9a59a11a35a92b9e6990ba0813103fc87efbecec6010f82d66ea25a5f9a52e3212c03e3ed9828acd1bdcf3d6aa6bc554cdb203acc2de30fdb27aa01b340a2bf59e85da1e031a5cb54b26854a5d9ecfb79f6b6f7c4835e11b9f8193c50030a3595965125cdbe8e0b07a7705b4afc5b839598c885b3adbbe66993ee13df0132150f647cbbe752d646088ad6cc0b5cc3cf19596edc6919baa8ee9f99c14af63f55c834542d48560d1d73f68edfb36df56ba129427fe17c11c813b072dda7453354746ff94c6616a5397fb5c6bfcf5b0b1c1daa81c97f6a6b9a2bb2e83a864860df321791b2aa8b0657f90e691d182233a049f1a1a8f3e353b9289a781fcc8a497772ca3fc0abc41bb38cbdba69752d9330d282d9a8d19449c2090139c00f4aaadef83cf4554ff8d0c0091ec424f4a5f82154ef12234e33581e0af30ed6752b40a799067771956d5471b2943d26cbbd6dedb612752547486648a0ee304ccd5c18a7ec7c096792f656988d07d8e0efdb9bda9ae6b0e7c2aba617261f464c95565bb9bd8f89d608371872b2dfe96fb5cadea12ef7aa5703342ade7f307efbbaed094357e4915e3e48bc03f99ae77c6b3f660aedf584915aa37f428751bda0c2e7faf4f1f4da43907ca716917c57502f72988dc2ac443e78aa0045c211e616a5c625632535ba6126ab0131e5a795edc35ef76730b08d3faecd3ddd4f588919dbcd984a8c8e97615a8d4d63e999536e8075acb90c2b36b2e57aa7eb12ec97a8d967468639a5ee651ab7d7328ba334915580c8960fcca335a6633e196beefb8e4cef53ac77814590a3d47486611e50adca3758d881b8cdef37d69b70c503d856389b7c1071db97a5b6bb66271f8229534b47a50e3aa2bf58bdef29e5ddc526ffbc400c09fbd5c3bf4bbd29027b6fe9bede49ccdb3588f52daeed648524dd1e9ae237b74eda79ef42a807ffa33a99a2a2ba237385f1fa997b09073945b87a6babef9e544d384fee7ed2068687ef4325358fc0e4dc47923f3f4f391650a3cf008d75a823a6e754bf8c8764462c6e7f9880d49997f8c6bd67792131604c5a8ca83ead333006";
        let t: Transaction = transaction.into();
        println!("{t:?}");
        assert_eq!(transaction, serialize(&t).to_hex::<String>());
    }

    #[test]
    // https://kmdexplorer.io/tx/5c480ed607993e0a28173ad030077a25a460d14cdd9dda494299c60b832233df
    fn kmd_5c480ed607993e0a28173ad030077a25a460d14cdd9dda494299c60b832233df() {
        let transaction = "02000000000202ecf451020000001976a9140eaccdb0d80773734ebd6deab0b2d8ac1eed1e9188ac83349800000000001976a9145177f8b427e5f47342a4b8ab5dac770815d4389e88ac00000000010000000000000000c5629c5202000000e69af4fa69bbf4766afa924efa74234f731e4bb886a8c97047f6e8679a42f4df7c27e739653d9c49a233f6e65483eedfdfe21e0c566953ad7a71def70dc8f5de3e46735b3217ae32082871d9741370ce7fb26392a261d613ef340d6c97220b5244364d12ed50a7a3c3d23202bee9cb98f4f98b5267c750207d492fdf5d1fc9d4326491262a67b455d9df5177ed87c20ad9d1a2514f8ec54ea7a0d442e1205e76425e773ec00b81a386abff593f341de8fe9733ed956e94ede7de199ca751d520d852cc812e09e19afd9fed4aa314b137c8a810cce86771b66badaf5a6e922fc6402c98b7d478e733fe018121c74e6e615110e70365df0452f701010d697490d9344f6fca14482bf8daecd8f3051fa40a6af3f72c9ee659f37821c1742312f82d021684fc6374c24fbe8802a250bfc60fdcff3363b19f5c6bbbf91d1b6ca192035e0204bb0a20ebd61536c0fbd6cf534615f537cd28c6f809aa5e5027d69f2e353d2d0a05d560d6d2e425bdaa61190a9bdf82f6594e857b2766e7fdbada2bdf165e125cb4931091eee8bf88d57d5b0404df69cce44731287da03dd0db0999b6cdc7e904021094d668fe5454d3c719dbc0012fd69c723edcfbd5e97a4c8bdb2bec13b54f04032bf94d7cac536cd5c64b05f5c1900e2e7be6c059b072b543d92e221c3ef4562f022f104d6594bf81836d0cd65cef476f65f78ee8d913819089ac7dfbe24230b05a022d8e60e011d93bcfc4f7fbf7747a7b0fa487cb1587c8e361b9d084bd4f6822ef03176ecd89df1b2290b628e17c1f5203a1bdd87954c7b0c12b7e061671be22cdb49cf50c2efce60ae3e52fbe8d740db41c5181d408bf1941a16a85d328a17c6f09ab92a5e06884659a02b96225de4de2edaa4595272fdf379ac0c5a8c9074324135127245d19621505d469d3c16e516e12832450c1c7c907325847ed6e80796a82ea3b6cbda51af6ca79fabc06aba17f73491ec4b5579e3f02fc0ca5589f7606b3c719fedb12439b0a4d7e19c9a5798d47e37483ef1ad492cc761d1bc2aee853e068bb23b418904db3b084af62594710d538b8fb82a4dbfb20ef25728c13df7dc548454295c3bd96de43f85fcf558a66261c376c43c740b9644bdbb34da613bd8ecd855ea66b77fcf97c1b7be238faa1a9dfe7caeb904e132cbf29496455d43d1960bc70f407069f4d2fe33fc40f1f22aa92291ea605952763b5bdd138ca85a24e97003915323dd89b15d63c0dbd76222b317a752584a3d0b8810c1135949fd3098f18a7afaaf8d479fe70110976d182e72b4a8f1ffbee84400bd3577b20ec70321654d8a9eb61562d67eab6d4527bc63a5f4f25b9631ca0095beb4abc493d5356dd1d133d6855e368794b3fe6d7d0995d3224c04b13b97310526a58195a321a1e2da8d2cd0af3f8fe3fc1504a994ffbce29cfa508c96ee2126ca7ab92f9503a476cd481a9414a1cc6ee24d18d06138160f931ea609441da60507581f24beb8605e8e2144f6ad620adc6c190e4e6a64364bbae3c99adf94e57fa13f2d2853e732f34131851276a368c23577ca21a4bb743a0cfc5ce8cb76c3e983211e9152e4e3c379edac6d0257a9bc70d706d004c33cc83f24acefcd730e46497e9dcceb84aaf6b46891098b8a274ebf83cba7ce86e3ac4776dd19d42cf0c903535d39c73f633afbaa5c9fd611a57320462f9e164d9fcd30b7f1a37cfd596bf744a785be880c92668bcbec40c3f64cb523ad1ce454b20783b1a0cabc4b1c45e2986a8fa9e646144c87f48f41f39409600a6914c25ac5706b6567307831a86abdee4b11e7efe480f0be8124f96b696dfaa11e48f23dc9deb0f38ecf1423d36af91aee697d25560e6d8a2d1f32b0bbc7392d4206bbfc8a32124c15bbc4be2cb92706fc3e7e7274688fb8f05f08d978e0bdd4ffbcf7d4254ab114c15175f033043059e6f19ac5c4728ae95bf00b5fb55221e313c1e2b5b9c09fb585aa300fb467c110625ed8f286f666f17453cfea74fb8dc1cc7eadd8fd1e7b073fffa0107817183d6c492b6136a2333323fdbde38de4df830b1dcae1ec6115401a3c834527d6924ff48e1a52e18bd8e5a060495050f57e83fcb5456dc2b868fe17e033d09d663a61e26490fc256251a199f554fd56d466ca7c65c9ee1cb6b25516f64120c4323868323c55943d3b224226150ab590b59f3896b30957e6beb5aaf8f7601feeab8e8d621af08059f59438ab1d2d23d603963d2eca6a65c62f9c1deb65f26cbe7d57b51a3a16d1bb13a7453a50c9a480f601fd576824518723543945d9a721ff02e2f4c667ee332ed1b2b3bc92a6f4348d24044e913ef0d080b9da700356fbf2c552efdd8cf37d294bdf1746237c08a33fbae3c6b79f742f5a7b0e8b9a2ae982c975ac93c851fc1ef4ff678ef5f9e7debb550dabb87bb0518ef69d715c0faf4b4c5c7896be17b04dbad7ba039b7e418e1cc68304c7563ebba0487ac4a335e95d021ea7d66903e11af70ec15f8c9bdc096c5312cfd3ae1cfcde3054016e0350c15483ef72fb87fc97ee0664c3d58e8f55db95b2c2f8a78e50d32309de0b5d41813064509aa57ea82f2200eb76bb02795f034165d18ff8e76b7c69d085750480e83f41bd27ac469c647a822f74ad604d12dd74c42a049a634686c0e";
        let t: Transaction = transaction.into();
        println!("{t:?}");
        assert_eq!(transaction, serialize(&t).to_hex::<String>());
    }

    #[test]
    fn sapling_negative_value_balance() {
        let transaction = "0400008085202f890162878ceccb1d4ea904681f3fffad22ec79aeea7e83f117a579ec60859ab9ee3e0000000000feffffff00e5c29360c701000010460afaffffffff00015280edc62c0e174cc112dfcc5c47c34d74d119bca3850c54bbe6383ed4d0d9f242c8cb5819674460802bd39257bf0bb7c7b5e9f9efb051e6f48d681f088c916e713187a7fcbce4eb6ec7f35d5bbdab1225ded98e2f845cddae36ebf45c9e7e00e28614c05416da18888e3883e9667e2df9a513612fe41df4c462b28473c1f10aa03cec99d1e4e23cd84d1bfee827ec9077ddb0d0537660017778e880c7835c9d81e4d8cfb1def0506805cac8fd3ebf2f231d15fbd9807e0ec7c50d26b24f8f07b21552dc0abc0feb4e7ee7ee40c620a07160a87a4683801539e4b22f0778960dd0a6e2b8d6b09cd5e0fbf40484340f9a88c0bea726d6221244ea5ba859f04af0dc8798c4e4379620bad0b7ac9093d877f23431f30a39173f8a2db2a6665a0cc0180f9eab676cac85aeeee14ec53abbb94f977b01437310c4543a390e4e4838c6a3b9de82d63ed7c4032d9c32d631adb477391a30807fffd33f98b8ed72b0684eb9f6afd02a4d9c935353448103d490f7095a725f75d75c3539d4e9ea7b76a725d4898403e9047288ad2ba46bec0e130f5ed160e4b2cf7a0db768a7a1f158baf067f6996f171269fa41df02c5b588fd12d430b40f8763b1d2f4b234f6a490ae90266c262fa8ec8da3172e87064d97d3a2a2700b5215739196553fe2d69db970f8b8d0252439f7829dcc7d8ec17dfa85e53320e26650876888a7505327b03abecb91b6367e4c3feada297ae0eff32ce0ff3780d924cb2f19ae97bb648fa6a49e6a8677ef4aa08b55bc9ae77ac6a2a7ad26c3c7ac718ef263ea53fbad012c641aed9997221a283743e337e97aa62ef6db3ae13835f4b572000f8c5a32b73aecfc7ae68b2b9924b6dde7c6fcca207e25feae0a024c4fcfd0207e9ad346b36af5fd581769cac99daeab680c593a617d9571ed5c247846d14cecda2acfe9cfbbb22408c1bc3bb7da3f7ac4a8ca0726ba01ee4531b5036fcc376970998b617d6cef7bbe4bdbf77d6adaebbdc0542ea60ff25b30cde6764777d1f821fd60f91499ccb3aac20d5ccd01ed313a53ac33bcbdaca460209aaa0e94feb16a82cbaa902210644777a21862c7dbc30df403c75e04dc47696230cf40dfa1045f480abdbebf4ebcbac6f700f287cef3d4b147ddf0e8e9d02ee9baa51604682d5b983dc6f8abee274be0e02a8a7de3fbb22563726c2e7d34b15c437e560e7ce1569ea2ed16e37d908e60af15fd44e46b8969fa74a0f24147553947da7db10fa3394c54413f1b4d6dfcab790a3a91027a1947a229644da8663f05bd05ff4a8621db679467ab74bf8eab1be5861f80b004af53cb679be479e82aa3de8ae5002521474e197975860a9b506e698ffaa385803c656dd984b60b0569ad617c6347722fddbde10f0f485362fbedfde0a600bf551c3f8b02778c4d6960a533fcb302";
        let t: Transaction = transaction.into();
        println!("{t:?}");
        assert_eq!(transaction, serialize(&t).to_hex::<String>());
    }

    #[test]
    fn n_time_transaction() {
        let actual: Transaction = "0100000003b353610304d549c67cb7ae66bd70e6e24e5346e3b904bc1928a0c8ebbcf07c4bceb1f6b9020000006a473044022013d6bb97af945c955e33d49517d75d681a308f5f6c1f48048984bd6673a2657a022050da86bd9f9bc68e4ccec184ad0c17f54387bbad04a1932a52c511c2c6715be30121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffffce1aa62b413dd2870bc9490193add689d7f6a452650b9dfc18d5ace1e1e1b573000000006b483045022100ebc72b69f44f4ea83318e64d379f8c678017bb6e1e9c31578588f7962ee310d8022001839c3c839f5d32c2c491ab9b67ed91cb4b0e94d967d66920bb4d6d8f082c200121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffffb358dde3245b17d219e9b2c40b3ce7ac9477e05719e96504706d39bc0fcd198b000000006a473044022059d50b7ba9a7db6c97e57c77e230a69dedd54f64c2c6f76c72696869968dfade022050eca1c2c5f4ca8db4b602496a9d350d747a3144cb9fb0e57968cedd9e478f310121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff030d1d6a100000000017a91439352bc650acd5e1dfbf109841c1a97824e8b858870000000000000000166a14e1fdcb31df41211da0325bb6dce577b856588b787cd84e02000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac03b35361".into();
        let expected = Transaction {
			version: 1,
			n_time: Some(1632875267),
			overwintered: false,
			expiry_height: 0,
			binding_sig: H512::default(),
			join_split_pubkey: H256::default(),
			join_split_sig: H512::default(),
			join_splits: vec![],
			shielded_spends: vec![],
			shielded_outputs: vec![],
			value_balance: 0,
			version_group_id: 0,
			inputs: vec![TransactionInput {
				previous_output: OutPoint {
					hash: "04d549c67cb7ae66bd70e6e24e5346e3b904bc1928a0c8ebbcf07c4bceb1f6b9".into(),
					index: 2,
				},
				script_sig: "473044022013d6bb97af945c955e33d49517d75d681a308f5f6c1f48048984bd6673a2657a022050da86bd9f9bc68e4ccec184ad0c17f54387bbad04a1932a52c511c2c6715be30121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8".into(),
				sequence: 4294967295,
				script_witness: vec![],
			}, TransactionInput {
				previous_output: OutPoint {
					hash: "ce1aa62b413dd2870bc9490193add689d7f6a452650b9dfc18d5ace1e1e1b573".into(),
					index: 0,
				},
				script_sig: "483045022100ebc72b69f44f4ea83318e64d379f8c678017bb6e1e9c31578588f7962ee310d8022001839c3c839f5d32c2c491ab9b67ed91cb4b0e94d967d66920bb4d6d8f082c200121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8".into(),
				sequence: 4294967295,
				script_witness: vec![],
			}, TransactionInput {
				previous_output: OutPoint {
					hash: "b358dde3245b17d219e9b2c40b3ce7ac9477e05719e96504706d39bc0fcd198b".into(),
					index: 0,
				},
				script_sig: "473044022059d50b7ba9a7db6c97e57c77e230a69dedd54f64c2c6f76c72696869968dfade022050eca1c2c5f4ca8db4b602496a9d350d747a3144cb9fb0e57968cedd9e478f310121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8".into(),
				sequence: 4294967295,
				script_witness: vec![],
			}],
			outputs: vec![TransactionOutput {
				value: 275389709,
				script_pubkey: "a91439352bc650acd5e1dfbf109841c1a97824e8b85887".into(),
			}, TransactionOutput {
				value: 0,
				script_pubkey: "6a14e1fdcb31df41211da0325bb6dce577b856588b78".into(),
			}, TransactionOutput {
				value: 38721660,
				script_pubkey: "76a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac".into(),
			}],
			lock_time: 1632875267,
			zcash: false,
            posv: false,
            str_d_zeel: None,
            tx_hash_algo: TxHashAlgo::DSHA256,
            v_extra_payload: None,
		};
        assert_eq!(actual, expected);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_from_tx_to_ext_tx() {
        // https://live.blockcypher.com/btc-testnet/tx/2be90e03abb4d5328bf7e9467ca9c571aef575837b55f1253119b87e85ccb94f/
        let tx: Transaction = "010000000001016546e6d844ad0142c8049a839e8deae16c17f0a6587e36e75ff2181ed7020a800100000000ffffffff0247070800000000002200200bbfbd271853ec0a775e5455d4bb19d32818e9b5bda50655ac183fb15c9aa01625910300000000001600149a85cc05e9a722575feb770a217c73fd6145cf0102473044022002eac5d11f3800131985c14a3d1bc03dfe5e694f5731bde39b0d2b183eb7d3d702201d62e7ff2dd433260bf7a8223db400d539a2c4eccd27a5aa24d83f5ad9e9e1750121031ac6d25833a5961e2a8822b2e8b0ac1fd55d90cbbbb18a780552cbd66fc02bb35c099c61".into();
        let ext_tx = ExtTransaction::from(tx.clone());
        assert_eq!(tx.hash().reversed().to_string(), ext_tx.txid().to_string());
    }

    #[test]
    fn n_time_posv_transaction() {
        let raw = "0200000001fa402b05b9108ec4762247d74c48a2ff303dd832d24c341c486e32cef0434177010000004847304402207a5283cc0fe6fc384744545cb600206ec730d0cdfa6a5e1479cb509fda536ee402202bec1e79b90638f1c608d805b2877fefc8fa6d0df279f58f0a70883e0e0609ce01ffffffff030000000000000000006a734110a10a0000232102fa0ecb032c7cb7be378efd03a84532b5cf1795996bfad854f042dc521616bfdcacd57f643201000000232103c8fc5c87f00bcc32b5ce5c036957f8befeff05bf4d88d2dcde720249f78d9313ac00000000dfcb3c64";
        let t: Transaction = raw.into();

        assert_eq!(t.version, 2);
        assert_eq!(t.lock_time, 0);
        assert_eq!(t.inputs.len(), 1);
        assert_eq!(t.outputs.len(), 3);
        assert_eq!(t.n_time, Some(1681705951));
        assert!(t.posv);

        let serialized = serialize(&t);
        assert_eq!(Bytes::from(raw), serialized);
    }

    #[test]
    fn n_time_posv_transaction_locktime() {
        let raw = "0200000001a471828d6290f5ca7935bc26a9d07cda37227ca3fb0e8d1282296ea839c134810100000048473044022067bbc1176fe4fa8681db854d9fce8d47d5613d01820ef78a6ab76c4f067500990220560d00098fcb69eb8587873f8072c11bcf832308fdcf5712d0e4aea4b761060e01fdffffff02940237c31d0600001976a9147fb8384a3f328148137447cdf6b1c3c1f0a8559588ac00e40b54020000001976a91485ee21a7f8cdd9034fb55004e0d8ed27db1c03c288acbd8d05002b584e63";
        let t: Transaction = raw.into();

        assert_eq!(t.version, 2);
        assert_eq!(t.lock_time, 363965);
        assert_eq!(t.inputs.len(), 1);
        assert_eq!(t.outputs.len(), 2);
        assert_eq!(t.n_time, Some(1666078763));
        assert!(t.posv);

        let serialized = serialize(&t);
        assert_eq!(Bytes::from(raw), serialized);
    }
}
