use crate::Signature;
use chain::{Transaction as UtxoTx, TransactionInput};
use keys::bytes::Bytes;
use keys::Public as PublicKey;
use primitives::hash::{H256, H512};
use script::{Builder, Script, TransactionInputSigner, UnsignedTransactionInput};

pub fn complete_tx(unsigned: TransactionInputSigner, signed_inputs: Vec<TransactionInput>) -> UtxoTx {
    UtxoTx {
        inputs: signed_inputs,
        n_time: unsigned.n_time,
        outputs: unsigned.outputs.clone(),
        version: unsigned.version,
        overwintered: unsigned.overwintered,
        lock_time: unsigned.lock_time,
        expiry_height: unsigned.expiry_height,
        join_splits: vec![],
        shielded_spends: vec![],
        shielded_outputs: vec![],
        value_balance: 0,
        version_group_id: unsigned.version_group_id,
        binding_sig: H512::default(),
        join_split_sig: H512::default(),
        join_split_pubkey: H256::default(),
        zcash: unsigned.zcash,
        posv: unsigned.posv,
        str_d_zeel: unsigned.str_d_zeel,
        tx_hash_algo: unsigned.hash_algo.into(),
        v_extra_payload: None,
    }
}

/// Create TransactionInput spending P2PK output, adding script sig with signature
pub(crate) fn p2pk_spend_with_signature(
    unsigned_input: &UnsignedTransactionInput,
    fork_id: u32,
    signature: Signature,
) -> TransactionInput {
    let script_sig = script_sig(signature, fork_id);

    TransactionInput {
        previous_output: unsigned_input.previous_output,
        script_sig: Builder::default().push_data(&script_sig).into_bytes(),
        sequence: unsigned_input.sequence,
        script_witness: vec![],
    }
}

/// Create TransactionInput spending P2PKH output, adding script sig with signature
pub(crate) fn p2pkh_spend_with_signature(
    unsigned_input: &UnsignedTransactionInput,
    public_key: &PublicKey,
    fork_id: u32,
    signature: Signature,
) -> TransactionInput {
    let script_sig = script_sig_with_pub(public_key, fork_id, signature);

    TransactionInput {
        previous_output: unsigned_input.previous_output,
        script_sig,
        sequence: unsigned_input.sequence,
        script_witness: vec![],
    }
}

/// Create TransactionInput spending P2SH output, adding script sig with signature and redeem script
pub(crate) fn p2sh_spend_with_signature(
    unsigned_input: &UnsignedTransactionInput,
    redeem_script: Script,
    script_data: Script,
    fork_id: u32,
    signature: Signature,
) -> TransactionInput {
    let script_sig = script_sig(signature, fork_id);

    let mut resulting_script = Builder::default().push_data(&script_sig).into_bytes();
    if !script_data.is_empty() {
        resulting_script.extend_from_slice(&script_data);
    }

    let redeem_part = Builder::default().push_data(&redeem_script).into_bytes();
    resulting_script.extend_from_slice(&redeem_part);

    TransactionInput {
        previous_output: unsigned_input.previous_output,
        script_sig: resulting_script,
        sequence: unsigned_input.sequence,
        script_witness: vec![],
    }
}

/// Create TransactionInput spending P2WPKH output, adding script witness with signature
pub(crate) fn p2wpkh_spend_with_signature(
    unsigned_input: &UnsignedTransactionInput,
    public_key: &PublicKey,
    fork_id: u32,
    signature: Signature,
) -> TransactionInput {
    let script_sig = script_sig(signature, fork_id);

    TransactionInput {
        previous_output: unsigned_input.previous_output,
        script_sig: Bytes::from(Vec::new()),
        sequence: unsigned_input.sequence,
        script_witness: vec![script_sig, Bytes::from(public_key.to_vec())],
    }
}

pub(crate) fn script_sig_with_pub(public_key: &PublicKey, fork_id: u32, signature: Signature) -> Bytes {
    let script_sig = script_sig(signature, fork_id);
    let builder = Builder::default();
    builder
        .push_data(&script_sig)
        .push_data(public_key.to_vec().as_slice())
        .into_bytes()
}

pub(crate) fn script_sig(mut signature: Signature, fork_id: u32) -> Bytes {
    let mut sig_script = Bytes::default();

    sig_script.append(&mut signature);
    // Using SIGHASH_ALL only for now
    sig_script.append(&mut Bytes::from(vec![1 | fork_id as u8]));

    sig_script
}
