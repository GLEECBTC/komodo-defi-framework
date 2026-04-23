use super::api_types::{deserialize_exact_version_one, deserialize_u256, serialize_u256_as_string};
use super::config::ResolvedTronGaslessProvider;
use super::error::TronGasfreeError;
use super::typed_data::{hash_permit_transfer_typed_data, PermitTransferData, GASFREE_PERMIT_VERSION};
use crate::eth::tron::{TronAddress, TronSignature};
use crate::PrivKeyPolicy;
use bip32::DerivationPath;
use common::now_sec;
use ethereum_types::U256;
use ethkey::{public_to_address, sign, KeyPair};
use mm2_err_handle::prelude::*;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GasfreeSignedAuthorization {
    pub token: TronAddress,
    pub service_provider: TronAddress,
    pub user: TronAddress,
    pub receiver: TronAddress,
    #[serde(serialize_with = "serialize_u256_as_string", deserialize_with = "deserialize_u256")]
    pub value: U256,
    #[serde(serialize_with = "serialize_u256_as_string", deserialize_with = "deserialize_u256")]
    pub max_fee: U256,
    #[serde(serialize_with = "serialize_u256_as_string", deserialize_with = "deserialize_u256")]
    pub deadline: U256,
    #[serde(
        serialize_with = "serialize_u256_as_string",
        deserialize_with = "deserialize_exact_version_one"
    )]
    pub version: U256,
    #[serde(serialize_with = "serialize_u256_as_string", deserialize_with = "deserialize_u256")]
    pub nonce: U256,
    pub sig: TronSignature,
}

impl fmt::Debug for GasfreeSignedAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct RedactedSignature;

        impl fmt::Debug for RedactedSignature {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "<redacted>")
            }
        }

        f.debug_struct("GasfreeSignedAuthorization")
            .field("token", &self.token)
            .field("service_provider", &self.service_provider)
            .field("user", &self.user)
            .field("receiver", &self.receiver)
            .field("value", &self.value)
            .field("max_fee", &self.max_fee)
            .field("deadline", &self.deadline)
            .field("version", &self.version)
            .field("nonce", &self.nonce)
            .field("sig", &RedactedSignature)
            .finish()
    }
}

pub fn sign_permit_transfer(
    provider: &ResolvedTronGaslessProvider,
    transfer: &PermitTransferData,
    priv_key_policy: &PrivKeyPolicy<KeyPair>,
    derivation_path: Option<&DerivationPath>,
) -> MmResult<GasfreeSignedAuthorization, TronGasfreeError> {
    let key_pair = crate::eth::eth_utils::key_pair_from_priv_key_policy(priv_key_policy, derivation_path)
        .map_to_mm(TronGasfreeError::InvalidRequest)?;
    sign_permit_transfer_with_key_pair(provider, transfer, &key_pair)
}

fn sign_permit_transfer_with_key_pair(
    provider: &ResolvedTronGaslessProvider,
    transfer: &PermitTransferData,
    key_pair: &KeyPair,
) -> MmResult<GasfreeSignedAuthorization, TronGasfreeError> {
    reject_expired_deadline(transfer.deadline)?;
    verify_signing_key_matches_user(key_pair, &transfer.user)?;

    let digest = hash_permit_transfer_typed_data(provider, transfer)?;
    let signature = sign(key_pair.secret(), &digest).map_to_mm(|e| TronGasfreeError::Internal(e.to_string()))?;
    let mut signature_bytes = signature.to_vec();
    if signature_bytes.len() != 65 {
        return MmError::err(TronGasfreeError::Internal(format!(
            "Invalid GasFree signature length: {}",
            signature_bytes.len()
        )));
    }
    signature_bytes[64] = normalize_eip712_v(signature_bytes[64])?;
    let sig = TronSignature::from_hex_str(&hex::encode(signature_bytes)).map_to_mm(TronGasfreeError::Internal)?;

    Ok(GasfreeSignedAuthorization {
        token: transfer.token,
        service_provider: *provider.service_provider(),
        user: transfer.user,
        receiver: transfer.receiver,
        value: transfer.value,
        max_fee: transfer.max_fee,
        deadline: transfer.deadline,
        version: U256::from(GASFREE_PERMIT_VERSION),
        nonce: transfer.nonce,
        sig,
    })
}

fn reject_expired_deadline(deadline: U256) -> MmResult<(), TronGasfreeError> {
    if deadline <= U256::from(now_sec()) {
        return MmError::err(TronGasfreeError::InvalidRequest(
            "GasFree PermitTransfer deadline is expired".to_string(),
        ));
    }
    Ok(())
}

fn verify_signing_key_matches_user(key_pair: &KeyPair, user: &TronAddress) -> MmResult<(), TronGasfreeError> {
    let signer = TronAddress::from(public_to_address(key_pair.public()));
    if &signer != user {
        return MmError::err(TronGasfreeError::InvalidRequest(
            "GasFree PermitTransfer signing key does not match user address".to_string(),
        ));
    }
    Ok(())
}

fn normalize_eip712_v(v: u8) -> MmResult<u8, TronGasfreeError> {
    match v {
        0 | 1 => Ok(v + 27),
        27 | 28 => Ok(v),
        invalid => MmError::err(TronGasfreeError::Internal(format!(
            "Invalid GasFree signature recovery id: {invalid}"
        ))),
    }
}
