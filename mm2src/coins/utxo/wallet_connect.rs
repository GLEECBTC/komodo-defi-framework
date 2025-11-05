//! This module provides functionality to interact with WalletConnect for UTXO-based coins.
use std::convert::TryFrom;

use bitcrypto::sign_message_hash;
use chain::hash::H256;
use crypto::StandardHDPath;
use kdf_walletconnect::{
    chain::{WcChainId, WcRequestMethods},
    error::WalletConnectError,
    WalletConnectCtx, WcTopic,
};
use keys::{CompactSignature, Public};
use mm2_err_handle::prelude::{MmError, MmResult};

use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;

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
pub async fn get_pubkey_via_wallatconnect_signature(
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
