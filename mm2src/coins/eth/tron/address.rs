//! TRON address handling (base58, hex, validation, serde).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::convert::{TryFrom, TryInto};
use std::fmt;
use std::str::FromStr;

pub const ADDRESS_PREFIX: u8 = 0x41;
pub const ADDRESS_BASE58_PREFIX: char = 'T';
pub const ADDRESS_HEX_LEN: usize = 42;
pub const ADDRESS_BYTES_LEN: usize = 21;
pub const ADDRESS_BASE58_LEN: usize = 34;

/// TRON mainnet or testnet address (21 bytes, 0x41 prefix + 20-bytes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address {
    pub inner: [u8; ADDRESS_BYTES_LEN],
}

impl Address {
    /// Construct from raw 21 bytes (must be 0x41-prefixed).
    pub fn from_bytes(bytes: [u8; ADDRESS_BYTES_LEN]) -> Result<Self, String> {
        if bytes[0] != ADDRESS_PREFIX {
            return Err("TRON address must start with 0x41".into());
        }
        Ok(Self { inner: bytes })
    }

    /// Construct from base58 string (with checksum).
    pub fn from_base58(s: &str) -> Result<Self, String> {
        let data = bs58::decode(s)
            .with_check(None)
            .into_vec()
            .map_err(|e| format!("Invalid base58check address: {e}"))?;

        // SAFETY: Accessing `data[0]` is safe here because we first check that
        // `data.len() == ADDRESS_BYTES_LEN`, guaranteeing the slice is not empty
        // and has at least one element.
        if data.len() != ADDRESS_BYTES_LEN || data[0] != ADDRESS_PREFIX {
            return Err(format!(
                "Invalid address: expected {ADDRESS_BYTES_LEN} bytes with prefix 0x{ADDRESS_PREFIX:x}"
            ));
        }

        let inner = data
            .try_into()
            .map_err(|_| "Failed to convert address bytes to array".to_string())?;

        Ok(Self { inner })
    }

    /// Construct from hex string, with or without `0x` prefix.
    pub fn from_hex(s: &str) -> Result<Self, String> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        let data = hex::decode(s).map_err(|e| format!("Invalid hex address: {e}"))?;

        // SAFETY: Accessing `data[0]` is safe here because we first check that
        // `data.len() == ADDRESS_BYTES_LEN`, guaranteeing the slice is not empty
        // and has at least one element.
        if data.len() != ADDRESS_BYTES_LEN || data[0] != ADDRESS_PREFIX {
            return Err(format!(
                "Invalid address: expected {ADDRESS_BYTES_LEN} bytes with prefix 0x{ADDRESS_PREFIX:x}"
            ));
        }

        let inner = data
            .try_into()
            .map_err(|_| "Failed to convert address bytes to array".to_string())?;

        Ok(Self { inner })
    }

    /// Show as base58 string (canonical user format).
    pub fn to_base58(&self) -> String {
        bs58::encode(self.inner).with_check().into_string()
    }

    /// Show as hex string, lowercase (canonical hex format).
    pub fn to_hex(&self) -> String {
        hex::encode(self.inner)
    }

    /// Return the 21 bytes (0x41 + 20).
    pub fn as_bytes(&self) -> &[u8] {
        &self.inner
    }
}

impl TryFrom<[u8; ADDRESS_BYTES_LEN]> for Address {
    type Error = String;

    fn try_from(bytes: [u8; ADDRESS_BYTES_LEN]) -> Result<Self, Self::Error> {
        Self::from_bytes(bytes)
    }
}

impl AsRef<[u8]> for Address {
    fn as_ref(&self) -> &[u8] {
        &self.inner
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_base58())
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({} / 0x{})", self.to_base58(), self.to_hex())
    }
}

impl Serialize for Address {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_base58())
    }
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D>(deserializer: D) -> Result<Address, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = <&str>::deserialize(deserializer)?;
        Address::from_str(s).map_err(serde::de::Error::custom)
    }
}

impl FromStr for Address {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Check for Base58 format
        if s.len() == ADDRESS_BASE58_LEN && s.starts_with(ADDRESS_BASE58_PREFIX) {
            return Self::from_base58(s);
        }

        // Check for hex format (with or without 0x prefix)
        if (s.len() == ADDRESS_HEX_LEN && s.starts_with("41"))
            || (s.len() == ADDRESS_HEX_LEN + 2 && s.starts_with("0x41"))
        {
            return Self::from_hex(s);
        }

        Err(format!(
            "Invalid TRON address '{s}': must be Base58 (34 chars starting with 'T') or hex (42 chars without 0x, 44 chars with 0x prefix)"
        ))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_tron_address_base58_and_hex() {
        let base58 = "TNPeeaaFB7K9cmo4uQpcU32zGK8G1NYqeL";
        let hex = "418840e6c55b9ada326d211d818c34a994aeced808";
        let addr1 = Address::from_str(base58).unwrap();
        let addr2 = Address::from_str(hex).unwrap();
        assert_eq!(addr1, addr2);
        assert_eq!(addr1.to_hex(), hex);
        assert_eq!(addr2.to_base58(), base58);
    }

    #[test]
    fn test_invalid_tron_address() {
        assert!(Address::from_str("foo").is_err());
        assert!(Address::from_str("0xdeadbeef").is_err());
    }
}
