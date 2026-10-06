use crate::mintlayer::{
    select_spendable_coin_utxos, MintlayerFeeError, MintlayerFeeRate, MintlayerUtxo, MintlayerUtxoSelection,
    MintlayerUtxoSelectionError,
};
use keys::KeyPair;
use mintlayer_sdk::crypto::types::*;
use mintlayer_sdk::crypto::{self, Amount, Network, SigHashType, SourceId, TxAdditionalInfo};
use rpc::v1::types::Bytes as BytesJson;
use std::convert::TryInto;
use thiserror::Error;

const MAX_FEE_ITERATIONS: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MintlayerTransaction {
    pub signed_bytes: Vec<u8>,
    pub transaction_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MintlayerHtlcResolutionPlan {
    pub transaction_id: String,
    pub signed_bytes: Vec<u8>,
    pub input_atoms: u128,
    pub output_atoms: u128,
    pub fee_atoms: u128,
    pub serialized_bytes: usize,
    pub iterations: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MintlayerSignedTransactionPlan {
    pub transaction_id: String,
    pub signed_bytes: Vec<u8>,
    pub selected_inputs: usize,
    pub selected_atoms: u128,
    pub send_atoms: u128,
    pub change_atoms: u128,
    pub fee_atoms: u128,
    pub minimum_fee_atoms: u128,
    pub serialized_bytes: usize,
    pub iterations: usize,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum MintlayerTransactionPlanError {
    #[error("Mintlayer send amount must be greater than zero")]
    ZeroSendAmount,
    #[error("Mintlayer sender address does not match the signing key")]
    SenderKeyMismatch,
    #[error("Mintlayer amount calculation overflow")]
    AmountOverflow,
    #[error("Unsupported Mintlayer outpoint source type '{0}'")]
    UnsupportedSourceType(String),
    #[error("Invalid Mintlayer source id '{0}'")]
    InvalidSourceId(String),
    #[error("Mintlayer UTXO selection failed: {0}")]
    UtxoSelection(#[from] MintlayerUtxoSelectionError),
    #[error("Mintlayer fee calculation failed: {0}")]
    Fee(#[from] MintlayerFeeError),
    #[error("Mintlayer SDK transaction operation failed: {0}")]
    Sdk(String),
    #[error("Mintlayer canonical fee convergence entered a cycle")]
    FeeConvergenceCycle,
    #[error("Mintlayer canonical fee did not converge within {0} iterations")]
    FeeDidNotConverge(usize),
    #[error("Mintlayer transaction amount conservation failed")]
    AmountConservation,
    #[error("Failed to bridge the KDF signing key to the Mintlayer SDK: {0}")]
    KdfKeyBridge(String),
    #[error("Mintlayer HTLC resolution requires a coin HTLC output")]
    InvalidHtlcOutput,
    #[error("Mintlayer HTLC amount cannot cover the canonical transaction fee")]
    HtlcAmountBelowFee,
    #[error("Mintlayer HTLC secret must contain exactly 32 bytes, found {0}")]
    InvalidHtlcSecretLength(usize),
}

const SECP256K1_SCHNORR_SCALE_TAG: u8 = 0;

/// Converts the active KDF secp256k1 key into the tagged SCALE representation
/// expected by the official Mintlayer SDK.
///
/// Both temporary byte buffers are cleared before this function returns. The
/// resulting SDK key remains in memory only for the lifetime chosen by the
/// caller and is never logged or exported.
/// Recovers the canonical Mintlayer transaction id from encoded signed-transaction bytes.
///
/// Signed transaction encoding starts with the underlying Transaction; the pinned SDK
/// lenient decoder reads that prefix and the canonical id is computed from it.
pub fn canonical_transaction_id_from_signed_bytes(
    signed_bytes: &[u8],
) -> Result<String, MintlayerTransactionPlanError> {
    let transaction = crypto::decode_transaction_lenient(signed_bytes).map_err(sdk_error)?;
    Ok(crypto::transaction_id(&transaction))
}

pub fn sdk_private_key_from_kdf_key_pair(key_pair: &KeyPair) -> Result<PrivateKey, MintlayerTransactionPlanError> {
    let mut kdf_secret = key_pair.private_bytes();
    let mut tagged_secret = [0_u8; 33];
    tagged_secret[0] = SECP256K1_SCHNORR_SCALE_TAG;
    tagged_secret[1..].copy_from_slice(&kdf_secret);

    let decoded = {
        let mut encoded = tagged_secret.as_slice();
        PrivateKey::decode_all(&mut encoded)
            .map_err(|error| MintlayerTransactionPlanError::KdfKeyBridge(error.to_string()))
    };

    kdf_secret.fill(0);
    tagged_secret.fill(0);
    decoded
}

#[derive(Clone, Copy)]
enum MintlayerHtlcResolution<'a> {
    Spend(&'a [u8]),
    Refund,
}

#[allow(clippy::too_many_arguments)]
pub fn plan_signed_htlc_spend_offline(
    payment_transaction_id: &str,
    htlc_output_index: u32,
    htlc_output: &TxOutput,
    destination_address: &str,
    spend_key: &PrivateKey,
    secret: &[u8],
    fee_rate: MintlayerFeeRate,
    inclusion_height: u64,
    network: Network,
) -> Result<MintlayerHtlcResolutionPlan, MintlayerTransactionPlanError> {
    if secret.len() != 32 {
        return Err(MintlayerTransactionPlanError::InvalidHtlcSecretLength(secret.len()));
    }

    plan_signed_htlc_resolution_offline(
        payment_transaction_id,
        htlc_output_index,
        htlc_output,
        destination_address,
        spend_key,
        MintlayerHtlcResolution::Spend(secret),
        fee_rate,
        inclusion_height,
        network,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn plan_signed_htlc_refund_offline(
    payment_transaction_id: &str,
    htlc_output_index: u32,
    htlc_output: &TxOutput,
    destination_address: &str,
    refund_key: &PrivateKey,
    fee_rate: MintlayerFeeRate,
    inclusion_height: u64,
    network: Network,
) -> Result<MintlayerHtlcResolutionPlan, MintlayerTransactionPlanError> {
    plan_signed_htlc_resolution_offline(
        payment_transaction_id,
        htlc_output_index,
        htlc_output,
        destination_address,
        refund_key,
        MintlayerHtlcResolution::Refund,
        fee_rate,
        inclusion_height,
        network,
    )
}

#[allow(clippy::too_many_arguments)]
fn plan_signed_htlc_resolution_offline(
    payment_transaction_id: &str,
    htlc_output_index: u32,
    htlc_output: &TxOutput,
    destination_address: &str,
    signing_key: &PrivateKey,
    resolution: MintlayerHtlcResolution<'_>,
    fee_rate: MintlayerFeeRate,
    inclusion_height: u64,
    network: Network,
) -> Result<MintlayerHtlcResolutionPlan, MintlayerTransactionPlanError> {
    let input_atoms = match htlc_output {
        TxOutput::Htlc(OutputValue::Coin(amount), _) => amount.into_atoms(),
        _ => return Err(MintlayerTransactionPlanError::InvalidHtlcOutput),
    };

    crypto::encode_destination(destination_address, network).map_err(sdk_error)?;

    let signer_address =
        crypto::pubkey_to_pubkeyhash_address(&crypto::public_key_from_private_key(signing_key), network);

    if signer_address != destination_address {
        return Err(MintlayerTransactionPlanError::SenderKeyMismatch);
    }

    let source_id_bytes = decode_source_id(payment_transaction_id)?;
    let source_id = crypto::encode_outpoint_source_id(H256::from_slice(&source_id_bytes), SourceId::Transaction);

    let mut fee_guess_atoms = 0_u128;
    let mut previous_non_final_state = None;

    for iteration in 1..=MAX_FEE_ITERATIONS {
        let output_atoms = input_atoms
            .checked_sub(fee_guess_atoms)
            .ok_or(MintlayerTransactionPlanError::HtlcAmountBelowFee)?;

        if output_atoms == 0 {
            return Err(MintlayerTransactionPlanError::HtlcAmountBelowFee);
        }

        let input = crypto::encode_input_for_utxo(source_id.clone(), htlc_output_index);

        let output = crypto::encode_output_transfer(Amount::from_atoms(output_atoms), destination_address, network)
            .map_err(sdk_error)?;

        let transaction = crypto::encode_transaction(vec![input], vec![output], 0).map_err(sdk_error)?;
        let transaction_id = crypto::transaction_id(&transaction);

        let input_utxos = [Some(htlc_output.clone())];

        let witness = match resolution {
            MintlayerHtlcResolution::Spend(secret) => {
                let secret: [u8; 32] = secret
                    .try_into()
                    .map_err(|_| MintlayerTransactionPlanError::InvalidHtlcSecretLength(secret.len()))?;

                crypto::encode_witness_htlc_spend(
                    SigHashType::all(),
                    signing_key,
                    destination_address,
                    &transaction,
                    &input_utxos,
                    0,
                    HtlcSecret::new(secret),
                    &TxAdditionalInfo::new(),
                    inclusion_height,
                    network,
                )
                .map_err(sdk_error)?
            },
            MintlayerHtlcResolution::Refund => crypto::encode_witness_htlc_refund_single_sig(
                SigHashType::all(),
                signing_key,
                destination_address,
                &transaction,
                &input_utxos,
                0,
                &TxAdditionalInfo::new(),
                inclusion_height,
                network,
            )
            .map_err(sdk_error)?,
        };

        let signed = crypto::encode_signed_transaction(transaction, vec![witness]).map_err(sdk_error)?;

        let signed_bytes = signed.encode();
        let canonical_fee_atoms = fee_rate.compute_fee_atoms(signed_bytes.len())?;

        if canonical_fee_atoms == fee_guess_atoms {
            return Ok(MintlayerHtlcResolutionPlan {
                transaction_id,
                serialized_bytes: signed_bytes.len(),
                signed_bytes,
                input_atoms,
                output_atoms,
                fee_atoms: canonical_fee_atoms,
                iterations: iteration,
            });
        }

        let state = (signed_bytes.len(), canonical_fee_atoms);

        if previous_non_final_state == Some(state) {
            return Err(MintlayerTransactionPlanError::FeeConvergenceCycle);
        }

        previous_non_final_state = Some(state);
        fee_guess_atoms = canonical_fee_atoms;
    }

    Err(MintlayerTransactionPlanError::FeeDidNotConverge(MAX_FEE_ITERATIONS))
}

/// Builds and signs a native Mintlayer transfer entirely in memory.
///
/// This function has no API client, transport or submission capability. The
/// caller supplies already-fetched UTXOs and the fee rate; the result is a
/// signed byte vector that remains offline until another layer explicitly
/// decides how to handle it.
// Keep the offline planning inputs explicit at the call site.
#[allow(clippy::too_many_arguments)]
pub fn plan_signed_transaction_offline(
    utxos: &[MintlayerUtxo],
    sender_address: &str,
    recipient_address: &str,
    send_atoms: u128,
    fee_rate: MintlayerFeeRate,
    spend_key: &PrivateKey,
    inclusion_height: u64,
    network: Network,
) -> Result<MintlayerSignedTransactionPlan, MintlayerTransactionPlanError> {
    let payment_output = crypto::encode_output_transfer(Amount::from_atoms(send_atoms), recipient_address, network)
        .map_err(sdk_error)?;

    plan_signed_output_offline(
        utxos,
        sender_address,
        payment_output,
        send_atoms,
        fee_rate,
        spend_key,
        inclusion_height,
        network,
    )
}

/// Builds and signs a Mintlayer transaction around an already constructed
/// primary output, while preserving the canonical UTXO selection, fee
/// convergence, change and input-signing path used by ordinary transfers.
// Keep the offline planning inputs explicit at the call site.
#[allow(clippy::too_many_arguments)]
pub fn plan_signed_output_offline(
    utxos: &[MintlayerUtxo],
    sender_address: &str,
    payment_output: TxOutput,
    send_atoms: u128,
    fee_rate: MintlayerFeeRate,
    spend_key: &PrivateKey,
    inclusion_height: u64,
    network: Network,
) -> Result<MintlayerSignedTransactionPlan, MintlayerTransactionPlanError> {
    if send_atoms == 0 {
        return Err(MintlayerTransactionPlanError::ZeroSendAmount);
    }

    let signer_public_key = crypto::public_key_from_private_key(spend_key);
    let signer_address = crypto::pubkey_to_pubkeyhash_address(&signer_public_key, network);
    if signer_address != sender_address {
        return Err(MintlayerTransactionPlanError::SenderKeyMismatch);
    }

    crypto::encode_destination(sender_address, network).map_err(sdk_error)?;

    let mut fee_guess_atoms = 0_u128;
    let mut previous_non_final_state = None;

    for iteration in 1..=MAX_FEE_ITERATIONS {
        let required_atoms = send_atoms
            .checked_add(fee_guess_atoms)
            .ok_or(MintlayerTransactionPlanError::AmountOverflow)?;
        let selection = select_spendable_coin_utxos(utxos, sender_address, required_atoms)?;
        let available_fee_atoms = selection
            .total_atoms
            .checked_sub(send_atoms)
            .ok_or(MintlayerTransactionPlanError::AmountConservation)?;
        let change_atoms = available_fee_atoms
            .checked_sub(fee_guess_atoms)
            .ok_or(MintlayerTransactionPlanError::AmountConservation)?;
        let candidate = build_signed_candidate(
            &selection,
            sender_address,
            payment_output.clone(),
            change_atoms,
            spend_key,
            inclusion_height,
            network,
        )?;
        let canonical_fee_atoms = fee_rate.compute_fee_atoms(candidate.signed_bytes.len())?;

        if canonical_fee_atoms == fee_guess_atoms {
            return finalize_plan(
                candidate,
                selection,
                send_atoms,
                change_atoms,
                canonical_fee_atoms,
                canonical_fee_atoms,
                iteration,
            );
        }

        if change_atoms > 0 && available_fee_atoms <= canonical_fee_atoms {
            let no_change = build_signed_candidate(
                &selection,
                sender_address,
                payment_output.clone(),
                0,
                spend_key,
                inclusion_height,
                network,
            )?;
            let no_change_minimum_fee = fee_rate.compute_fee_atoms(no_change.signed_bytes.len())?;

            if available_fee_atoms >= no_change_minimum_fee {
                return finalize_plan(
                    no_change,
                    selection,
                    send_atoms,
                    0,
                    available_fee_atoms,
                    no_change_minimum_fee,
                    iteration,
                );
            }
        }

        let state = (
            selection.selected.len(),
            change_atoms > 0,
            candidate.signed_bytes.len(),
            canonical_fee_atoms,
        );
        if previous_non_final_state == Some(state) {
            return Err(MintlayerTransactionPlanError::FeeConvergenceCycle);
        }
        previous_non_final_state = Some(state);
        fee_guess_atoms = canonical_fee_atoms;
    }

    Err(MintlayerTransactionPlanError::FeeDidNotConverge(MAX_FEE_ITERATIONS))
}

struct SignedCandidate {
    transaction_id: String,
    signed_bytes: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
fn build_signed_candidate(
    selection: &MintlayerUtxoSelection,
    sender_address: &str,
    payment_output: TxOutput,
    change_atoms: u128,
    spend_key: &PrivateKey,
    inclusion_height: u64,
    network: Network,
) -> Result<SignedCandidate, MintlayerTransactionPlanError> {
    let inputs = selection
        .selected
        .iter()
        .map(|selected| {
            let source_id_bytes = decode_source_id(&selected.outpoint.source_id)?;
            let hash = H256::from_slice(&source_id_bytes);
            let source_type = match selected.outpoint.source_type.as_str() {
                "Transaction" => SourceId::Transaction,
                "BlockReward" => SourceId::BlockReward,
                other => return Err(MintlayerTransactionPlanError::UnsupportedSourceType(other.to_owned())),
            };
            let source_id = crypto::encode_outpoint_source_id(hash, source_type);
            Ok(crypto::encode_input_for_utxo(source_id, selected.outpoint.index))
        })
        .collect::<Result<Vec<_>, MintlayerTransactionPlanError>>()?;

    let previous_destination = crypto::encode_destination(sender_address, network).map_err(sdk_error)?;
    let input_utxos = selection
        .selected
        .iter()
        .map(|selected| {
            Some(TxOutput::Transfer(
                OutputValue::Coin(Amount::from_atoms(selected.atoms)),
                previous_destination.clone(),
            ))
        })
        .collect::<Vec<_>>();

    let mut outputs = vec![payment_output];
    if change_atoms > 0 {
        outputs.push(
            crypto::encode_output_transfer(Amount::from_atoms(change_atoms), sender_address, network)
                .map_err(sdk_error)?,
        );
    }

    let transaction = crypto::encode_transaction(inputs, outputs, 0).map_err(sdk_error)?;
    let transaction_id = crypto::transaction_id(&transaction);
    let mut witnesses = Vec::with_capacity(input_utxos.len());
    for input_index in 0..input_utxos.len() {
        witnesses.push(
            crypto::encode_witness(
                SigHashType::all(),
                spend_key,
                sender_address,
                &transaction,
                &input_utxos,
                input_index,
                &TxAdditionalInfo::new(),
                inclusion_height,
                network,
            )
            .map_err(sdk_error)?,
        );
    }

    let signed = crypto::encode_signed_transaction(transaction, witnesses).map_err(sdk_error)?;
    Ok(SignedCandidate {
        transaction_id,
        signed_bytes: signed.encode(),
    })
}

fn finalize_plan(
    candidate: SignedCandidate,
    selection: MintlayerUtxoSelection,
    send_atoms: u128,
    change_atoms: u128,
    fee_atoms: u128,
    minimum_fee_atoms: u128,
    iterations: usize,
) -> Result<MintlayerSignedTransactionPlan, MintlayerTransactionPlanError> {
    let conserved = send_atoms
        .checked_add(change_atoms)
        .and_then(|value| value.checked_add(fee_atoms))
        .ok_or(MintlayerTransactionPlanError::AmountOverflow)?;
    if conserved != selection.total_atoms {
        return Err(MintlayerTransactionPlanError::AmountConservation);
    }

    let serialized_bytes = candidate.signed_bytes.len();
    Ok(MintlayerSignedTransactionPlan {
        transaction_id: candidate.transaction_id,
        signed_bytes: candidate.signed_bytes,
        selected_inputs: selection.selected.len(),
        selected_atoms: selection.total_atoms,
        send_atoms,
        change_atoms,
        fee_atoms,
        minimum_fee_atoms,
        serialized_bytes,
        iterations,
    })
}

fn decode_source_id(value: &str) -> Result<[u8; 32], MintlayerTransactionPlanError> {
    let decoded = hex::decode(value).map_err(|_| MintlayerTransactionPlanError::InvalidSourceId(value.to_owned()))?;
    decoded
        .try_into()
        .map_err(|_| MintlayerTransactionPlanError::InvalidSourceId(value.to_owned()))
}

fn sdk_error(error: impl ToString) -> MintlayerTransactionPlanError {
    MintlayerTransactionPlanError::Sdk(error.to_string())
}

impl crate::Transaction for MintlayerTransaction {
    fn tx_hex(&self) -> Vec<u8> {
        self.signed_bytes.clone()
    }

    fn tx_hash_as_bytes(&self) -> BytesJson {
        BytesJson::from(
            hex::decode(&self.transaction_id)
                .expect("Mintlayer transaction_id must contain canonical hexadecimal bytes"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PUBLIC_TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const FEE_RATE_ATOMS_PER_KB: u128 = 100_000_000_000;
    const INCLUSION_HEIGHT: u64 = 700_000;

    #[test]
    fn recovers_canonical_transaction_id_from_signed_bytes() {
        let network = Network::Mainnet;
        let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();
        let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
        let sender_address = address_for_key(&spend_key, network);
        let recipient_key = crypto::make_receiving_address(&account_key, 1).unwrap();
        let recipient_address = address_for_key(&recipient_key, network);

        let utxos = vec![
            coin_utxo("11".repeat(32), 0, 60_000_000_000, "0.6", &sender_address),
            coin_utxo("22".repeat(32), 1, 40_000_000_000, "0.4", &sender_address),
        ];

        let plan = plan_signed_transaction_offline(
            &utxos,
            &sender_address,
            &recipient_address,
            50_000_000_000,
            MintlayerFeeRate::from_atoms_per_kb(FEE_RATE_ATOMS_PER_KB),
            &spend_key,
            INCLUSION_HEIGHT,
            network,
        )
        .unwrap();

        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&plan.signed_bytes).unwrap(),
            plan.transaction_id
        );
    }

    #[test]
    fn mintlayer_transaction_roundtrips_through_kdf_transaction_enum() {
        use crate::{Transaction, TransactionEnum};

        let network = Network::Mainnet;
        let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();
        let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
        let sender_address = address_for_key(&spend_key, network);
        let recipient_key = crypto::make_receiving_address(&account_key, 1).unwrap();
        let recipient_address = address_for_key(&recipient_key, network);

        let utxos = vec![
            coin_utxo("11".repeat(32), 0, 60_000_000_000, "0.6", &sender_address),
            coin_utxo("22".repeat(32), 1, 40_000_000_000, "0.4", &sender_address),
        ];

        let plan = plan_signed_transaction_offline(
            &utxos,
            &sender_address,
            &recipient_address,
            50_000_000_000,
            MintlayerFeeRate::from_atoms_per_kb(FEE_RATE_ATOMS_PER_KB),
            &spend_key,
            INCLUSION_HEIGHT,
            network,
        )
        .unwrap();

        let canonical_txid = canonical_transaction_id_from_signed_bytes(&plan.signed_bytes).unwrap();

        assert_eq!(canonical_txid, plan.transaction_id);

        let transaction = MintlayerTransaction {
            signed_bytes: plan.signed_bytes.clone(),
            transaction_id: plan.transaction_id.clone(),
        };

        assert_eq!(transaction.tx_hex(), plan.signed_bytes);
        assert_eq!(
            transaction.tx_hash_as_bytes(),
            BytesJson::from(hex::decode(&canonical_txid).unwrap())
        );

        let transaction_enum: TransactionEnum = transaction.into();

        assert!(matches!(transaction_enum, TransactionEnum::MintlayerTransaction(_)));

        assert_eq!(transaction_enum.tx_hex(), plan.signed_bytes);
        assert_eq!(
            transaction_enum.tx_hash_as_bytes(),
            BytesJson::from(hex::decode(&canonical_txid).unwrap())
        );
    }

    #[test]
    fn plans_signed_htlc_spend_with_canonical_fee_and_extractable_secret() {
        use crate::mintlayer::build_mintlayer_htlc_output;

        const PAYMENT_TXID: &str = "1111111111111111111111111111111111111111111111111111111111111111";
        const HTLC_ATOMS: u128 = 100_000_000_000;
        const TIME_LOCK: u64 = 1_800_000_000;
        const SECRET_BYTES: [u8; 32] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11,
            0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        ];

        let network = Network::Mainnet;
        let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();

        let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
        let spend_address = address_for_key(&spend_key, network);

        let refund_key = crypto::make_receiving_address(&account_key, 1).unwrap();
        let refund_address = address_for_key(&refund_key, network);

        let secret = HtlcSecret::new(SECRET_BYTES);
        let secret_hash = secret.hash();

        let htlc_output = build_mintlayer_htlc_output(
            HTLC_ATOMS,
            secret_hash.as_bytes(),
            &spend_address,
            &refund_address,
            TIME_LOCK,
            network,
        )
        .unwrap();

        let fee_rate = MintlayerFeeRate::from_atoms_per_kb(FEE_RATE_ATOMS_PER_KB);

        let plan = plan_signed_htlc_spend_offline(
            PAYMENT_TXID,
            0,
            &htlc_output,
            &spend_address,
            &spend_key,
            &SECRET_BYTES,
            fee_rate,
            INCLUSION_HEIGHT,
            network,
        )
        .unwrap();

        assert_eq!(plan.input_atoms, HTLC_ATOMS);
        assert_eq!(plan.output_atoms + plan.fee_atoms, plan.input_atoms);
        assert_eq!(
            fee_rate.compute_fee_atoms(plan.serialized_bytes).unwrap(),
            plan.fee_atoms
        );
        assert!(plan.iterations >= 2);

        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&plan.signed_bytes).unwrap(),
            plan.transaction_id
        );

        let decoded = crypto::decode_transaction_lenient(&plan.signed_bytes).unwrap();

        assert_eq!(decoded.outputs().len(), 1);

        match &decoded.outputs()[0] {
            TxOutput::Transfer(OutputValue::Coin(amount), destination) => {
                assert_eq!(amount.into_atoms(), plan.output_atoms);
                assert_eq!(
                    destination,
                    &crypto::encode_destination(&spend_address, network).unwrap()
                );
            },
            other => panic!(
                "expected HTLC spend to produce one coin transfer output, got {:?}",
                other
            ),
        }

        let signed = SignedTransaction::decode_all(&mut &plan.signed_bytes[..]).unwrap();

        let source_id = crypto::encode_outpoint_source_id(
            H256::from_slice(&hex::decode(PAYMENT_TXID).unwrap()),
            SourceId::Transaction,
        );

        let extracted = crypto::extract_htlc_secret(&signed, source_id, 0).unwrap();

        assert_eq!(extracted, HtlcSecret::new(SECRET_BYTES));
        assert_eq!(extracted.encode(), SECRET_BYTES.to_vec());
    }

    #[test]
    fn plans_signed_htlc_refund_with_canonical_fee_without_revealing_secret() {
        use crate::mintlayer::build_mintlayer_htlc_output;

        const PAYMENT_TXID: &str = "2222222222222222222222222222222222222222222222222222222222222222";
        const HTLC_ATOMS: u128 = 100_000_000_000;
        const TIME_LOCK: u64 = 1_800_000_000;
        const SECRET_BYTES: [u8; 32] = [
            0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f, 0x30, 0x31,
            0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c, 0x3d, 0x3e, 0x3f,
        ];

        let network = Network::Mainnet;
        let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();

        let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
        let spend_address = address_for_key(&spend_key, network);

        let refund_key = crypto::make_receiving_address(&account_key, 1).unwrap();
        let refund_address = address_for_key(&refund_key, network);

        let secret = HtlcSecret::new(SECRET_BYTES);
        let secret_hash = secret.hash();

        let htlc_output = build_mintlayer_htlc_output(
            HTLC_ATOMS,
            secret_hash.as_bytes(),
            &spend_address,
            &refund_address,
            TIME_LOCK,
            network,
        )
        .unwrap();

        let fee_rate = MintlayerFeeRate::from_atoms_per_kb(FEE_RATE_ATOMS_PER_KB);

        let plan = plan_signed_htlc_refund_offline(
            PAYMENT_TXID,
            0,
            &htlc_output,
            &refund_address,
            &refund_key,
            fee_rate,
            INCLUSION_HEIGHT,
            network,
        )
        .unwrap();

        assert_eq!(plan.input_atoms, HTLC_ATOMS);
        assert_eq!(plan.output_atoms + plan.fee_atoms, plan.input_atoms);
        assert_eq!(
            fee_rate.compute_fee_atoms(plan.serialized_bytes).unwrap(),
            plan.fee_atoms
        );
        assert!(plan.iterations >= 2);

        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&plan.signed_bytes).unwrap(),
            plan.transaction_id
        );

        let decoded = crypto::decode_transaction_lenient(&plan.signed_bytes).unwrap();

        match &decoded.outputs()[0] {
            TxOutput::Transfer(OutputValue::Coin(amount), destination) => {
                assert_eq!(amount.into_atoms(), plan.output_atoms);
                assert_eq!(
                    destination,
                    &crypto::encode_destination(&refund_address, network).unwrap()
                );
            },
            other => panic!(
                "expected HTLC refund to produce one coin transfer output, got {:?}",
                other
            ),
        }

        let signed = SignedTransaction::decode_all(&mut &plan.signed_bytes[..]).unwrap();

        let source_id = crypto::encode_outpoint_source_id(
            H256::from_slice(&hex::decode(PAYMENT_TXID).unwrap()),
            SourceId::Transaction,
        );

        let result = crypto::extract_htlc_secret(&signed, source_id, 0);

        assert!(
            matches!(result, Err(crypto::Error::UnexpectedHtlcSpendType)),
            "refund must not expose an HTLC secret, got {:?}",
            result
        );
    }

    #[test]
    fn rejects_invalid_htlc_resolution_inputs() {
        use crate::mintlayer::build_mintlayer_htlc_output;

        const PAYMENT_TXID: &str = "3333333333333333333333333333333333333333333333333333333333333333";
        const TIME_LOCK: u64 = 1_800_000_000;
        const SECRET_BYTES: [u8; 32] = [0x55; 32];

        let network = Network::Mainnet;
        let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();

        let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
        let spend_address = address_for_key(&spend_key, network);

        let refund_key = crypto::make_receiving_address(&account_key, 1).unwrap();
        let refund_address = address_for_key(&refund_key, network);

        let secret = HtlcSecret::new(SECRET_BYTES);
        let secret_hash = secret.hash();

        let fee_rate = MintlayerFeeRate::from_atoms_per_kb(FEE_RATE_ATOMS_PER_KB);

        let normal_output =
            crypto::encode_output_transfer(Amount::from_atoms(100_000_000_000), &spend_address, network).unwrap();

        assert_eq!(
            plan_signed_htlc_spend_offline(
                PAYMENT_TXID,
                0,
                &normal_output,
                &spend_address,
                &spend_key,
                &SECRET_BYTES,
                fee_rate,
                INCLUSION_HEIGHT,
                network,
            ),
            Err(MintlayerTransactionPlanError::InvalidHtlcOutput)
        );

        let htlc_output = build_mintlayer_htlc_output(
            100_000_000_000,
            secret_hash.as_bytes(),
            &spend_address,
            &refund_address,
            TIME_LOCK,
            network,
        )
        .unwrap();

        assert_eq!(
            plan_signed_htlc_spend_offline(
                PAYMENT_TXID,
                0,
                &htlc_output,
                &spend_address,
                &spend_key,
                &[0_u8; 31],
                fee_rate,
                INCLUSION_HEIGHT,
                network,
            ),
            Err(MintlayerTransactionPlanError::InvalidHtlcSecretLength(31))
        );

        let tiny_htlc = build_mintlayer_htlc_output(
            1,
            secret_hash.as_bytes(),
            &spend_address,
            &refund_address,
            TIME_LOCK,
            network,
        )
        .unwrap();

        assert_eq!(
            plan_signed_htlc_spend_offline(
                PAYMENT_TXID,
                0,
                &tiny_htlc,
                &spend_address,
                &spend_key,
                &SECRET_BYTES,
                fee_rate,
                INCLUSION_HEIGHT,
                network,
            ),
            Err(MintlayerTransactionPlanError::HtlcAmountBelowFee)
        );
    }

    #[test]
    fn plans_and_signs_htlc_output_offline() {
        use crate::mintlayer::build_mintlayer_htlc_output;

        const SECRET_HASH: [u8; 20] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x10, 0x20,
            0x30, 0x40,
        ];
        const TIME_LOCK: u64 = 1_800_000_000;

        let network = Network::Mainnet;
        let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();
        let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
        let sender_address = address_for_key(&spend_key, network);
        let recipient_key = crypto::make_receiving_address(&account_key, 1).unwrap();
        let recipient_address = address_for_key(&recipient_key, network);

        let utxos = vec![
            coin_utxo("11".repeat(32), 0, 60_000_000_000, "0.6", &sender_address),
            coin_utxo("22".repeat(32), 1, 40_000_000_000, "0.4", &sender_address),
        ];

        let htlc_output = build_mintlayer_htlc_output(
            50_000_000_000,
            &SECRET_HASH,
            &recipient_address,
            &sender_address,
            TIME_LOCK,
            network,
        )
        .unwrap();

        let plan = plan_signed_output_offline(
            &utxos,
            &sender_address,
            htlc_output,
            50_000_000_000,
            MintlayerFeeRate::from_atoms_per_kb(FEE_RATE_ATOMS_PER_KB),
            &spend_key,
            INCLUSION_HEIGHT,
            network,
        )
        .unwrap();

        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&plan.signed_bytes).unwrap(),
            plan.transaction_id
        );
        assert_eq!(
            plan.send_atoms + plan.change_atoms + plan.fee_atoms,
            plan.selected_atoms
        );

        let transaction = crypto::decode_transaction_lenient(&plan.signed_bytes).unwrap();
        assert!(matches!(transaction.outputs().first(), Some(TxOutput::Htlc(_, _))));
    }

    #[test]
    fn plans_and_signs_canonical_transaction_without_submission() {
        let network = Network::Mainnet;
        let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();
        let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
        let sender_address = address_for_key(&spend_key, network);
        let recipient_key = crypto::make_receiving_address(&account_key, 1).unwrap();
        let recipient_address = address_for_key(&recipient_key, network);
        let utxos = vec![
            coin_utxo("11".repeat(32), 0, 60_000_000_000, "0.6", &sender_address),
            coin_utxo("22".repeat(32), 1, 40_000_000_000, "0.4", &sender_address),
        ];

        let plan = plan_signed_transaction_offline(
            &utxos,
            &sender_address,
            &recipient_address,
            50_000_000_000,
            MintlayerFeeRate::from_atoms_per_kb(FEE_RATE_ATOMS_PER_KB),
            &spend_key,
            INCLUSION_HEIGHT,
            network,
        )
        .unwrap();

        assert_eq!(plan.selected_inputs, 2);
        assert_eq!(plan.selected_atoms, 100_000_000_000);
        assert_eq!(plan.change_atoms, 15_500_000_000);
        assert_eq!(plan.fee_atoms, 34_500_000_000);
        assert_eq!(plan.minimum_fee_atoms, plan.fee_atoms);
        assert_eq!(plan.serialized_bytes, 345);
        assert_eq!(
            plan.transaction_id,
            "df383086aebaf0f809b8e1ce55fb9e2faafb4aef3716cd1af32954944d96720c"
        );
        assert_eq!(
            plan.send_atoms + plan.change_atoms + plan.fee_atoms,
            plan.selected_atoms
        );
        assert!(plan.iterations >= 2);
        assert!(!plan.signed_bytes.is_empty());
    }

    #[test]
    fn absorbs_small_residue_without_creating_change() {
        let network = Network::Mainnet;
        let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();
        let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
        let sender_address = address_for_key(&spend_key, network);
        let recipient_key = crypto::make_receiving_address(&account_key, 1).unwrap();
        let recipient_address = address_for_key(&recipient_key, network);
        let input_atoms = 68_700_000_000;
        let utxos = vec![coin_utxo("33".repeat(32), 0, input_atoms, "0.687", &sender_address)];

        let plan = plan_signed_transaction_offline(
            &utxos,
            &sender_address,
            &recipient_address,
            50_000_000_000,
            MintlayerFeeRate::from_atoms_per_kb(FEE_RATE_ATOMS_PER_KB),
            &spend_key,
            INCLUSION_HEIGHT,
            network,
        )
        .unwrap();

        assert_eq!(plan.change_atoms, 0);
        assert_eq!(plan.serialized_bytes, 175);
        assert_eq!(plan.minimum_fee_atoms, 17_500_000_000);
        assert_eq!(plan.fee_atoms, 18_700_000_000);
        assert_eq!(plan.send_atoms + plan.fee_atoms, input_atoms);
    }

    #[test]
    fn rejects_zero_amount_and_mismatched_key() {
        let network = Network::Mainnet;
        let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();
        let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
        let other_key = crypto::make_receiving_address(&account_key, 1).unwrap();
        let sender_address = address_for_key(&spend_key, network);
        let recipient_address = address_for_key(&other_key, network);
        let fee_rate = MintlayerFeeRate::from_atoms_per_kb(FEE_RATE_ATOMS_PER_KB);

        assert_eq!(
            plan_signed_transaction_offline(
                &[],
                &sender_address,
                &recipient_address,
                0,
                fee_rate,
                &spend_key,
                INCLUSION_HEIGHT,
                network,
            ),
            Err(MintlayerTransactionPlanError::ZeroSendAmount)
        );
        assert_eq!(
            plan_signed_transaction_offline(
                &[],
                &sender_address,
                &recipient_address,
                1,
                fee_rate,
                &other_key,
                INCLUSION_HEIGHT,
                network,
            ),
            Err(MintlayerTransactionPlanError::SenderKeyMismatch)
        );
    }

    fn address_for_key(key: &PrivateKey, network: Network) -> String {
        let public_key = crypto::public_key_from_private_key(key);
        crypto::pubkey_to_pubkeyhash_address(&public_key, network)
    }

    fn coin_utxo(source_id: String, index: u32, atoms: u128, decimal: &str, destination: &str) -> MintlayerUtxo {
        serde_json::from_value(json!({
            "outpoint": {
                "index": index,
                "source_id": source_id,
                "source_type": "Transaction"
            },
            "utxo": {
                "destination": destination,
                "type": "Transfer",
                "value": {
                    "amount": { "atoms": atoms.to_string(), "decimal": decimal },
                    "type": "Coin"
                }
            }
        }))
        .unwrap()
    }
}
