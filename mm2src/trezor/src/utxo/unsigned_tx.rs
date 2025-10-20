use crate::proto::messages_bitcoin as proto_bitcoin;
use crate::utxo::prev_tx::PrevTx;
use crate::{serialize_derivation_path, TrezorError, TrezorResult};
use hw_common::primitives::DerivationPath;
use mm2_err_handle::prelude::*;

/// https://github.com/trezor/trezor-common/blob/master/protob/messages-bitcoin.proto#L16
#[derive(Clone, Copy)]
pub enum TrezorInputScriptType {
    /// Standard P2PKH address.
    SpendAddress,
    /// P2SH multisig address.
    SpendMultiSig,
    /// Reserved for external inputs (coinjoin).
    External,
    /// Native SegWit.
    SpendWitness,
    /// SegWit over P2SH (backward compatible).
    SpendP2SHWitness,
}

impl From<TrezorInputScriptType> for proto_bitcoin::InputScriptType {
    fn from(script: TrezorInputScriptType) -> Self {
        match script {
            TrezorInputScriptType::SpendAddress => proto_bitcoin::InputScriptType::Spendaddress,
            TrezorInputScriptType::SpendMultiSig => proto_bitcoin::InputScriptType::Spendmultisig,
            TrezorInputScriptType::External => proto_bitcoin::InputScriptType::External,
            TrezorInputScriptType::SpendWitness => proto_bitcoin::InputScriptType::Spendwitness,
            TrezorInputScriptType::SpendP2SHWitness => proto_bitcoin::InputScriptType::Spendp2shwitness,
        }
    }
}

#[derive(Clone, Copy)]
pub enum TrezorOutputScriptType {
    /// Used for all addresses: bitcoin, p2sh, witness (except for the change output).
    PayToAddress,
    /// OP_RETURN.
    PayToOpReturn,
    /// pay to witness v0, used for the change output
    PayToWitness,
}

impl From<TrezorOutputScriptType> for proto_bitcoin::OutputScriptType {
    fn from(script: TrezorOutputScriptType) -> Self {
        match script {
            TrezorOutputScriptType::PayToAddress => proto_bitcoin::OutputScriptType::Paytoaddress,
            TrezorOutputScriptType::PayToOpReturn => proto_bitcoin::OutputScriptType::Paytoopreturn,
            TrezorOutputScriptType::PayToWitness => proto_bitcoin::OutputScriptType::Paytowitness,
        }
    }
}

/// Missing fields:
/// * script_sig - https://docs.trezor.io/trezor-firmware/common/communication/bitcoin-signing.html#external-inputs
/// * multisig - filled if input is going to spend multisig tx
/// * decred_tree - only for Decred, 0 is a normal transaction while 1 is a stake transaction
/// * witness - witness data, only set for EXTERNAL inputs
/// * ownership_proof - SLIP-0019 proof of ownership, only set for EXTERNAL inputs
/// * commitment_data - optional commitment data for the SLIP-0019 proof of ownership
/// * orig_hash - tx_hash of the original transaction where this input was spent (used when creating a replacement transaction)
/// * orig_index - index of the input in the original transaction (used when creating a replacement transaction)
/// * decred_staking_spend - if not None this holds the type of stake spend: revocation or stake generation
pub struct UnsignedTxInput {
    /// BIP-32 path to derive the key from master node.
    /// TODO I guess this field shouldn't be set if the input script type is Multisig, for example.
    pub address_derivation_path: Option<DerivationPath>,
    /// Info of previous transaction.
    pub prev_tx: PrevTx,
    /// Hash of previous transaction output to spend by this input.
    pub prev_hash: Vec<u8>,
    /// Index of previous output to spend.
    pub prev_index: u32,
    /// Sequence.
    pub sequence: u32,
    /// Defines template of input script.
    pub input_script_type: TrezorInputScriptType,
    /// Amount of previous transaction output.
    pub amount: u64,
}

impl UnsignedTxInput {
    fn to_proto(&self) -> proto_bitcoin::TxAckInput {
        let address_n = match self.address_derivation_path {
            Some(ref address_n) => serialize_derivation_path(address_n),
            None => Vec::new(),
        };
        let input = proto_bitcoin::TxInput {
            address_n,
            prev_hash: self.prev_hash.clone(),
            prev_index: self.prev_index,
            script_sig: None,
            sequence: Some(self.sequence),
            script_type: Some(proto_bitcoin::InputScriptType::from(self.input_script_type) as i32),
            multisig: None,
            amount: self.amount,
            decred_tree: None,
            witness: None,
            ownership_proof: None,
            commitment_data: None,
            orig_hash: None,
            orig_index: None,
            decred_staking_spend: None,
        };

        let tx = proto_bitcoin::tx_ack_input::TxAckInputWrapper { input };
        proto_bitcoin::TxAckInput { tx }
    }
}

/// Missing fields:
/// * multisig - defines multisig address; script_type must be PAYTOMULTISIG
/// * op_return_data - defines op_return data; script_type must be PAYTOOPRETURN, amount must be 0
/// * orig_hash - tx_hash of the original transaction where this output was present (used when creating a replacement transaction)
/// * orig_index - index of the output in the original transaction (used when creating a replacement transaction)
pub struct TxOutput {
    /// Destination address in Base58 encoding; script_type must be PAYTOADDRESS.
    /// This field is `None` if the output is a change.
    pub address: Option<String>,
    /// BIP-32 path to derive the key from master node.
    /// This field is set if the output is a change, otherwise it's `None`.
    pub address_derivation_path: Option<DerivationPath>,
    /// Amount to spend in satoshis.
    pub amount: u64,
    /// Output script type.
    pub script_type: TrezorOutputScriptType,
}

impl TxOutput {
    fn to_proto(&self) -> proto_bitcoin::TxAckOutput {
        let address_n = match self.address_derivation_path {
            Some(ref address_n) => serialize_derivation_path(address_n),
            None => Vec::new(),
        };
        let output = proto_bitcoin::TxOutput {
            address: self.address.clone(),
            address_n,
            amount: self.amount,
            script_type: Some(proto_bitcoin::OutputScriptType::from(self.script_type) as i32),
            multisig: None,
            op_return_data: None,
            orig_hash: None,
            orig_index: None,
        };

        let tx = proto_bitcoin::tx_ack_output::TxAckOutputWrapper { output };
        proto_bitcoin::TxAckOutput { tx }
    }
}

/// Missing fields:
/// * overwintered - deprecated in 2.3.2, the field is not needed as it can be derived from `version`.
///   The main reason why it's ignored is that this can be requested as an extra data:
///   https://docs.trezor.io/trezor-firmware/common/communication/bitcoin-signing.html#extra-data
pub struct UnsignedUtxoTx {
    pub coin: String,
    /// Transaction inputs.
    pub inputs: Vec<UnsignedTxInput>,
    /// Transaction outputs.
    pub outputs: Vec<TxOutput>,
    /// Transaction version.
    pub version: u32,
    /// Transaction lock_time.
    pub lock_time: u32,
    /// Expiry height. Only for Decred and Zcash.
    pub expiry: Option<u32>,
    /// only for Zcash, nVersionGroupId.
    pub version_group_id: Option<u32>,
    /// only for Zcash, BRANCH_ID.
    pub branch_id: Option<u32>,
}

impl UnsignedUtxoTx {
    pub(crate) fn sign_tx_message(&self) -> proto_bitcoin::SignTx {
        #[allow(deprecated)]
        proto_bitcoin::SignTx {
            outputs_count: self.outputs.len() as u32,
            inputs_count: self.inputs.len() as u32,
            coin_name: Some(self.coin.clone()),
            version: Some(self.version),
            lock_time: Some(self.lock_time),
            expiry: self.expiry,
            overwintered: None,
            version_group_id: self.version_group_id,
            timestamp: None,
            branch_id: self.branch_id,
            amount_unit: None,
            decred_staking_ticket: None,
        }
    }

    pub(crate) fn prev_tx(&self, hash: &[u8]) -> TrezorResult<&PrevTx> {
        self.inputs
            .iter()
            .find(|input| input.prev_hash == hash)
            .map(|input| &input.prev_tx)
            .or_mm_err(|| {
                let error = format!("Previous tx not found by the hash '{hash:?}'");
                TrezorError::ProtocolError(error)
            })
    }

    pub(crate) fn input_message(&self, input_index: usize) -> TrezorResult<proto_bitcoin::TxAckInput> {
        match self.inputs.get(input_index) {
            Some(prev_input) => Ok(prev_input.to_proto()),
            None => {
                let error = format!(
                    "Unexpected index '{}' of the tx input. Actual count of inputs: {}",
                    input_index,
                    self.inputs.len()
                );
                MmError::err(TrezorError::ProtocolError(error))
            },
        }
    }

    pub(crate) fn output_message(&self, output_index: usize) -> TrezorResult<proto_bitcoin::TxAckOutput> {
        match self.outputs.get(output_index) {
            Some(prev_output) => Ok(prev_output.to_proto()),
            None => {
                let error = format!(
                    "Unexpected index '{}' of the tx output. Actual count of outputs: {}",
                    output_index,
                    self.outputs.len()
                );
                MmError::err(TrezorError::ProtocolError(error))
            },
        }
    }
}
