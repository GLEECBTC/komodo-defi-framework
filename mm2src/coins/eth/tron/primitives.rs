use ethereum_types::{H256, H520};
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TronSignature(H520);

impl TronSignature {
    pub fn from_hex_str(value: &str) -> Result<Self, String> {
        // H520::from_str silently strips a lowercase `0x` prefix. We reject it upfront to
        // enforce the GasFree spec's "hex, no 0x prefix" wire-format requirement.
        if value.starts_with("0x") || value.starts_with("0X") {
            return Err("signature must not include a 0x prefix".to_string());
        }

        H520::from_str(value)
            .map(TronSignature)
            .map_err(|e| format!("Invalid signature: {e}"))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0.as_bytes())
    }
}

impl Serialize for TronSignature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for TronSignature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        TronSignature::from_hex_str(&value).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TronTxHash(H256);

impl TronTxHash {
    pub fn from_hex_str(value: &str) -> Result<Self, String> {
        // H256::from_str silently strips a lowercase `0x` prefix. We reject it upfront to
        // enforce the GasFree spec's "hex, no 0x prefix" wire-format requirement.
        if value.starts_with("0x") || value.starts_with("0X") {
            return Err("txnHash must not include a 0x prefix".to_string());
        }

        H256::from_str(value)
            .map(TronTxHash)
            .map_err(|e| format!("Invalid txnHash: {e}"))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0.as_bytes())
    }
}

impl Serialize for TronTxHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for TronTxHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        TronTxHash::from_hex_str(&value).map_err(D::Error::custom)
    }
}
