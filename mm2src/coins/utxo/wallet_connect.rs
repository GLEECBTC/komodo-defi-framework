//! This module provides functionality to interact with WalletConnect for UTXO-based coins.
use std::{collections::HashMap, convert::TryFrom};

use crate::utxo::utxo_common::DEFAULT_SWAP_VIN;
use crate::utxo::{utxo_common, UtxoCoinFields};
use crate::UtxoTx;
use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use bitcoin::{consensus::Decodable, consensus::Encodable, psbt::Psbt, EcdsaSighashType};
use bitcrypto::sign_message_hash;
use chain::bytes::Bytes;
use chain::hash::H256;
use crypto::StandardHDPath;
use kdf_walletconnect::{
    chain::{WcChainId, WcRequestMethods},
    error::WalletConnectError,
    WalletConnectCtx, WcTopic,
};
use keys::{Address, CompactSignature, Public};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::{MapMmError, MapToMmResult, MmError, MmResult};
use script::{Builder, TransactionInputSigner};
use serialization::{deserialize, Error as SerError};

/// Represents a UTXO address returned by GetAccountAddresses request in WalletConnect.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetAccountAddressesItem {
    address: String,
    public_key: Option<String>,
    path: Option<StandardHDPath>,
    #[allow(dead_code)]
    intention: Option<String>,
}

/// Get the enabled address (chosen by the user)
pub async fn get_walletconnect_address(
    wc: &WalletConnectCtx,
    session_topic: &WcTopic,
    chain_id: &WcChainId,
    derivation_path: &StandardHDPath,
) -> MmResult<(String, Option<String>), WalletConnectError> {
    wc.validate_update_active_chain_id(session_topic, chain_id).await?;
    let (account_str, _) = wc.get_account_and_properties_for_chain_id(session_topic, chain_id)?;
    let params = json!({
        "account": account_str,
    });
    let accounts: Vec<GetAccountAddressesItem> = wc
        .send_session_request_and_wait(
            session_topic,
            chain_id,
            WcRequestMethods::UtxoGetAccountAddresses,
            params,
        )
        .await?;

    // Find the address that the user is interested in (the enabled address).
    let account = accounts.iter().find(|a| a.path.as_ref() == Some(derivation_path));

    match account {
        // If we found an account with the specific derivation path, we pick it.
        Some(account) => Ok((account.address.clone(), account.public_key.clone())),
        // If we didn't find the account with the specific derivation path, we perform some sane fallback.
        None => {
            let first_account = accounts.into_iter().next().ok_or_else(|| {
                WalletConnectError::NoAccountFound(
                    "WalletConnect returned no addresses for getAccountAddresses".to_string(),
                )
            })?;
            // If the response doesn't include derivation path information, just return the first address.
            if first_account.path.is_none() {
                common::log::warn!("WalletConnect didn't specify derivation paths for getAccountAddresses, picking the first address: {}", first_account.address);
                Ok((first_account.address, first_account.public_key))
            } else {
                // Otherwise, the response includes a derivation path, which means we didn't find the one that the user was interested in.
                MmError::err(WalletConnectError::NoAccountFound(format!(
                    "No address found for derivation path: {derivation_path}"
                )))
            }
        },
    }
}

/// The response from WalletConnect for `signMessage` request.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignMessageResponse {
    address: String,
    signature: String,
    #[allow(dead_code)]
    message_hash: Option<String>,
}

/// Get the public key associated with some address via WalletConnect signature.
pub async fn get_pubkey_via_walletconnect_signature(
    wc: &WalletConnectCtx,
    session_topic: &WcTopic,
    chain_id: &WcChainId,
    address: &str,
    sign_message_prefix: &str,
) -> MmResult<String, WalletConnectError> {
    const AUTH_MSG: &str = "Authenticate with KDF";

    wc.validate_update_active_chain_id(session_topic, chain_id).await?;
    let (account_str, _) = wc.get_account_and_properties_for_chain_id(session_topic, chain_id)?;
    let params = json!({
        "account": account_str,
        "address": address,
        "message": AUTH_MSG,
        "protocol": "ecdsa",
    });
    let signature_response: SignMessageResponse = wc
        .send_session_request_and_wait(session_topic, chain_id, WcRequestMethods::UtxoPersonalSign, params)
        .await?;

    // The wallet is required to send back the same address in the response.
    // We validate it here even though there shouldn't be a mismatch (otherwise the wallet is broken).
    if signature_response.address != address {
        return MmError::err(WalletConnectError::InternalError(format!(
            "Address mismatch: requested signature from {}, got it from {}",
            address, signature_response.address
        )));
    }

    let decoded_signature = match hex::decode(&signature_response.signature) {
        Ok(decoded) => decoded,
        Err(hex_decode_err) => BASE64_ENGINE
            .decode(&signature_response.signature)
            .map_err(|base64_decode_err| {
                WalletConnectError::InternalError(format!(
                    "Failed to decode signature={} from hex (error={:?}) and from base64 (error={:?})",
                    signature_response.signature, hex_decode_err, base64_decode_err
                ))
            })?,
    };
    let signature = CompactSignature::try_from(decoded_signature).map_err(|e| {
        WalletConnectError::InternalError(format!(
            "Failed to parse signature={} into compact signature: {:?}",
            signature_response.signature, e
        ))
    })?;
    let message_hash = sign_message_hash(sign_message_prefix, AUTH_MSG);
    let pubkey = Public::recover_compact(&H256::from(message_hash), &signature).map_err(|e| {
        WalletConnectError::InternalError(format!(
            "Failed to recover public key from walletconnect signature={signature:?}: {e:?}"
        ))
    })?;

    Ok(pubkey.to_string())
}

/// The response from WalletConnect for `signPsbt` request.
#[derive(Deserialize)]
struct SignedPsbt {
    #[serde(deserialize_with = "common::seri::deserialize_base64")]
    psbt: Vec<u8>,
    #[expect(dead_code)]
    txid: Option<String>,
}

/// The parameters used to instruct WalletConnect how to sign a specific input in a PSBT.
///
/// An **array** of this struct is sent to WalletConnect in `SignPsbt` request.
#[derive(Serialize)]
struct InputSigningParams {
    /// The index of the input to sign.
    index: u32,
    /// The address to sign the input with.
    address: String,
    /// The sighash types to use for signing.
    sighash_types: Vec<u8>,
}

/// A utility function to sign a PSBT with WalletConnect.
async fn sign_psbt(
    wc: &WalletConnectCtx,
    session_topic: &WcTopic,
    chain_id: &WcChainId,
    mut psbt: Psbt,
    sign_inputs: Vec<InputSigningParams>,
    broadcast: bool,
) -> MmResult<Psbt, WalletConnectError> {
    // Serialize the PSBT and encode it in base64 format.
    let mut serialized_psbt = Vec::new();
    psbt.consensus_encode(&mut serialized_psbt).map_to_mm(|e| {
        WalletConnectError::InternalError(format!("Failed to serialize our PSBT for WalletConnect: {e}"))
    })?;
    let serialized_psbt = BASE64_ENGINE.encode(serialized_psbt);

    wc.validate_update_active_chain_id(session_topic, chain_id).await?;
    let (account_str, _) = wc.get_account_and_properties_for_chain_id(session_topic, chain_id)?;
    let params = json!({
        "account": account_str,
        "psbt": serialized_psbt,
        "signInputs": sign_inputs,
        "broadcast": broadcast,
    });
    let signed_psbt: SignedPsbt = wc
        .send_session_request_and_wait(session_topic, chain_id, WcRequestMethods::UtxoSignPsbt, params)
        .await?;

    let signed_psbt = Psbt::consensus_decode(&mut &signed_psbt.psbt[..]).map_to_mm(|e| {
        WalletConnectError::InternalError(format!("Failed to parse signed PSBT from WalletConnect: {e}"))
    })?;

    // The signed PSBT has strictly more information than our own PSBT, thus it's enough to proceed with it.
    // But we still combine it into our own PSBT to run compatibility validation and make sure WalletConnect didn't send us some nonsense.
    psbt.combine(signed_psbt).map_to_mm(|e| {
        WalletConnectError::InternalError(format!("Failed to merge the signed PSBT into the unsigned one: {e}"))
    })?;

    Ok(psbt)
}

/// Signs a P2SH transaction that has a single input using WalletConnect.
///
/// This is to be used for payment spend transactions and refund transactions, where the payment output is being spent.
/// `prev_tx` is the previous transaction that contains the P2SH output being spent.
/// `redeem_script` is the redeem script that is used to spend the P2SH output.
/// `unlocking_script` is the unlocking script that picks the appropriate spending path (normal spend (with secret hash) vs refund)
#[expect(clippy::too_many_arguments)]
pub async fn sign_p2sh_with_walletconnect(
    wc: &WalletConnectCtx,
    session_topic: &WcTopic,
    chain_id: &WcChainId,
    signing_address: &Address,
    tx_input_signer: &TransactionInputSigner,
    prev_tx: UtxoTx,
    redeem_script: Bytes,
    unlocking_script: Bytes,
) -> MmResult<UtxoTx, WalletConnectError> {
    let signing_address = signing_address.display_address().map_to_mm(|e| {
        WalletConnectError::InternalError(format!("Failed to convert the signing address to a string: {e}"))
    })?;

    let mut tx_to_sign: UtxoTx = tx_input_signer.clone().into();
    // Make sure we have exactly one input. We can later safely index inputs (by `[DEFAULT_SWAP_VIN]`) in the transaction and PSBT.
    if tx_to_sign.inputs.len() != 1 {
        return MmError::err(WalletConnectError::InternalError(
            "Expected exactly one input in the PSBT for P2SH signing".to_string(),
        ));
    }

    let mut psbt = Psbt::from_unsigned_tx(tx_to_sign.clone().into()).map_to_mm(|e| {
        WalletConnectError::InternalError(format!("Failed to create PSBT from unsigned transaction: {e}"))
    })?;
    // Since we are spending a P2SH input, we know for sure it's non-segwit.
    psbt.inputs[DEFAULT_SWAP_VIN].non_witness_utxo = Some(prev_tx.into());
    // We need to provide the redeem script as it's used in the signing process.
    psbt.inputs[DEFAULT_SWAP_VIN].redeem_script = Some(redeem_script.take().into());
    // TODO: Check whether we should put `fork_id` here or not. When we support a `fork_id`-based chain in WalletConnect.
    psbt.inputs[DEFAULT_SWAP_VIN].sighash_type = Some(EcdsaSighashType::All.into());

    // Ask WalletConnect to sign the PSBT for us.
    let inputs = vec![InputSigningParams {
        index: DEFAULT_SWAP_VIN as u32,
        address: signing_address.clone(),
        sighash_types: vec![EcdsaSighashType::All as u8],
    }];
    let signed_psbt = sign_psbt(wc, session_topic, chain_id, psbt, inputs, false).await?;

    // WalletConnect can't finalize the scriptSig for us since it doesn't have the unlocking script.
    // Thus, the signature for this input must be in the `partial_sigs` field.
    let walletconnect_sig = signed_psbt.inputs[DEFAULT_SWAP_VIN]
        .partial_sigs
        .values()
        .next()
        .ok_or_else(|| WalletConnectError::InternalError("No signature found in the signed PSBT".to_string()))?;
    let redeem_script = signed_psbt.inputs[DEFAULT_SWAP_VIN]
        .redeem_script
        .as_ref()
        .ok_or_else(|| WalletConnectError::InternalError("No redeem script found in the signed PSBT".to_string()))?;

    // The signature and the redeem script are inserted as data.
    let p2sh_signature = Builder::default().push_data(&walletconnect_sig.to_vec()).into_bytes();
    let redeem_script = Builder::default().push_data(redeem_script.as_bytes()).into_bytes();

    let mut final_script_sig = Bytes::new();
    final_script_sig.extend_from_slice(&p2sh_signature);
    final_script_sig.extend_from_slice(&unlocking_script);
    final_script_sig.extend_from_slice(&redeem_script);

    // Sign the transaction input with the final scriptSig.
    tx_to_sign.inputs[DEFAULT_SWAP_VIN].script_sig = final_script_sig;
    tx_to_sign.inputs[DEFAULT_SWAP_VIN].script_witness = vec![];

    Ok(tx_to_sign)
}

/// Signs a P2SH transaction that has a single input using WalletConnect.
///
/// This is just another wrapper around `sign_p2sh_with_walletconnect` to avoid some boilerplate given
/// that there is an accessible `coin`.
pub async fn sign_p2sh(
    coin: &impl AsRef<UtxoCoinFields>,
    session_topic: &WcTopic,
    tx_input_signer: &TransactionInputSigner,
    prev_tx: UtxoTx,
    redeem_script: Bytes,
    unlocking_script: Bytes,
) -> MmResult<UtxoTx, WalletConnectError> {
    let ctx = MmArc::from_weak(&coin.as_ref().ctx)
        .ok_or_else(|| WalletConnectError::InternalError("Couldn't get access to MmArc".to_string()))?;
    let wc_ctx = WalletConnectCtx::from_ctx(&ctx)?;
    // Get the address that's supposed to sign the P2SH transaction (its signature is required as per the redeem sript).
    let address = coin
        .as_ref()
        .derivation_method
        .single_addr()
        .await
        .ok_or_else(|| WalletConnectError::InternalError("Couldn't get address for P2SH signing".to_string()))?;
    let chain_id = coin
        .as_ref()
        .conf
        .chain_id
        .as_ref()
        .ok_or_else(|| WalletConnectError::InternalError("Chain ID is not set".to_string()))?;

    sign_p2sh_with_walletconnect(
        &wc_ctx,
        session_topic,
        chain_id,
        &address,
        tx_input_signer,
        prev_tx,
        redeem_script,
        unlocking_script,
    )
    .await
}

/// Signs a P2PKH/P2WPKH spending transaction using WalletConnect.
///
/// Contrary to what the function name might suggest, this function can sign both P2PKH and **P2WPKH** inputs.
/// `prev_txs` is a map of previous transactions that contain the P2PKH inputs being spent. P2WPKH inputs don't need their previous transactions.
pub async fn sign_p2pkh_with_walletconnect(
    wc: &WalletConnectCtx,
    session_topic: &WcTopic,
    chain_id: &WcChainId,
    signing_address: &Address,
    tx_input_signer: &TransactionInputSigner,
    prev_txs: HashMap<H256, UtxoTx>,
) -> MmResult<UtxoTx, WalletConnectError> {
    let signing_address = signing_address.display_address().map_to_mm(|e| {
        WalletConnectError::InternalError(format!("Failed to convert the signing address to a string: {e}"))
    })?;

    let mut tx_to_sign: UtxoTx = tx_input_signer.clone().into();
    let mut psbt = Psbt::from_unsigned_tx(tx_to_sign.clone().into()).map_to_mm(|e| {
        WalletConnectError::InternalError(format!("Failed to create PSBT from unsigned transaction: {e}"))
    })?;

    for (psbt_input, input) in psbt.inputs.iter_mut().zip(tx_input_signer.inputs.iter()) {
        if input.prev_script.is_pay_to_witness_key_hash() {
            // Set the witness output for P2WPKH inputs.
            psbt_input.witness_utxo = Some(bitcoin::TxOut {
                value: input.amount,
                script_pubkey: input.prev_script.to_vec().into(),
            });
        } else if input.prev_script.is_pay_to_public_key_hash() {
            // Set the previous Transaction for P2PKH inputs.
            let prev_tx = prev_txs.get(&input.previous_output.hash).ok_or_else(|| {
                WalletConnectError::InternalError(format!(
                    "Previous transaction not found for P2PKH input: {:?}",
                    input.previous_output
                ))
            })?;
            psbt_input.non_witness_utxo = Some(prev_tx.clone().into());
        } else {
            return MmError::err(WalletConnectError::InternalError(format!(
                "Expected a P2WPKH or P2PKH input for WalletConnect signing, got: {}",
                input.prev_script
            )));
        }
        // TODO: Check whether we should put `fork_id` here or not. When we support a `fork_id`-based chain in WalletConnect.
        psbt_input.sighash_type = Some(EcdsaSighashType::All.into());
    }

    // Ask WalletConnect to sign the PSBT for us.
    let inputs = psbt
        .inputs
        .iter()
        .enumerate()
        .map(|(idx, _)| InputSigningParams {
            index: idx as u32,
            address: signing_address.clone(),
            sighash_types: vec![EcdsaSighashType::All as u8],
        })
        .collect();
    let signed_psbt = sign_psbt(wc, session_topic, chain_id, psbt, inputs, false).await?;

    for ((psbt_input, input_to_sign), unsigned_input) in signed_psbt
        .inputs
        .into_iter()
        .zip(tx_to_sign.inputs.iter_mut())
        .zip(tx_input_signer.inputs.iter())
    {
        input_to_sign.script_sig = Default::default();
        input_to_sign.script_witness = Default::default();
        // If WalletConnect already finalized the script, use it at face value.
        // P2(W)PKH inputs are simple enough that some wallets will finalize the script for us.
        if let Some(final_script_witness) = psbt_input.final_script_witness {
            input_to_sign.script_witness = final_script_witness.to_vec().into_iter().map(Bytes::from).collect();
        } else if let Some(final_script_sig) = psbt_input.final_script_sig {
            input_to_sign.script_sig = Bytes::from(final_script_sig.to_bytes());
        } else {
            // If WalletConnect didn't finalize the script, we need to figure out whether it's a P2PKH or P2WPKH input and finalize it accordingly.
            let (pubkey, walletconnect_sig) = psbt_input.partial_sigs.iter().next().ok_or_else(|| {
                WalletConnectError::InternalError("No signature found in the signed PSBT".to_string())
            })?;
            if unsigned_input.prev_script.is_pay_to_witness_key_hash() {
                input_to_sign.script_witness =
                    vec![Bytes::from(walletconnect_sig.to_vec()), Bytes::from(pubkey.to_bytes())];
            } else {
                input_to_sign.script_sig = Builder::default()
                    .push_data(&walletconnect_sig.to_vec())
                    .push_data(&pubkey.to_bytes())
                    .into_bytes();
            }
        }
    }

    Ok(tx_to_sign)
}

/// Signs a P2PKH/P2WPKH spending transaction using WalletConnect.
///
/// This is just another wrapper around `sign_p2pkh_with_walletconnect` to avoid some boilerplate given
/// that there is an accessible `coin`.
pub async fn sign_p2pkh(
    coin: &impl AsRef<UtxoCoinFields>,
    session_topic: &WcTopic,
    tx_input_signer: &TransactionInputSigner,
) -> MmResult<UtxoTx, WalletConnectError> {
    let ctx = MmArc::from_weak(&coin.as_ref().ctx)
        .ok_or_else(|| WalletConnectError::InternalError("Couldn't get access to MmArc".to_string()))?;
    let wc_ctx = WalletConnectCtx::from_ctx(&ctx)?;
    let address =
        coin.as_ref().derivation_method.single_addr().await.ok_or_else(|| {
            WalletConnectError::InternalError("Couldn't get address for P2(W)PKH signing".to_string())
        })?;
    let chain_id = coin
        .as_ref()
        .conf
        .chain_id
        .as_ref()
        .ok_or_else(|| WalletConnectError::InternalError("Chain ID is not set".to_string()))?;

    // Collect the outpoints of each P2PKH input (non-witness ones).
    let prev_p2pkh_tx_hashes = tx_input_signer
        .inputs
        .iter()
        .filter(|input| input.prev_script.is_pay_to_public_key_hash())
        .map(|input| input.previous_output.hash.reversed().into())
        .collect();
    // Get the previous transactions that created these P2PKH inputs.
    let prev_p2pkh_txs_rpc_format =
        utxo_common::get_verbose_transactions_from_cache_or_rpc(coin.as_ref(), prev_p2pkh_tx_hashes)
            .await
            .mm_err(|e| WalletConnectError::InternalError(format!("Failed to get previous P2PKH transactions: {e}")))?;
    let prev_p2pkh_txs = prev_p2pkh_txs_rpc_format
        .into_iter()
        .map(|(hash, tx)| Ok((hash.reversed().into(), deserialize(tx.into_inner().hex.as_slice())?)))
        .collect::<Result<_, SerError>>()
        .map_err(|e| {
            WalletConnectError::InternalError(format!("Failed to deserialize previous P2PKH transactions: {e}"))
        })?;

    sign_p2pkh_with_walletconnect(
        &wc_ctx,
        session_topic,
        chain_id,
        &address,
        tx_input_signer,
        prev_p2pkh_txs,
    )
    .await
}
