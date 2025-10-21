//! Serializable wrapper around vector of bytes

use hex::{FromHex, ToHex};
use primitives::bytes::Bytes as GlobalBytes;
use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, ops};

/// Wrapper structure around vector of bytes.
#[derive(Debug, PartialEq, Eq, Default, Hash, Clone)]
pub struct Bytes(pub Vec<u8>);

impl Bytes {
    /// Simple constructor.
    pub fn new(bytes: Vec<u8>) -> Bytes {
        Bytes(bytes)
    }

    /// Convert back to vector
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

impl<T> From<T> for Bytes
where
    GlobalBytes: From<T>,
{
    fn from(other: T) -> Self {
        Bytes(GlobalBytes::from(other).take())
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(bytes: Bytes) -> Self {
        bytes.0
    }
}

impl Serialize for Bytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut serialized = String::new();
        serialized.push_str(self.0.to_hex::<String>().as_ref());
        serializer.serialize_str(serialized.as_ref())
    }
}

impl<'a> Deserialize<'a> for Bytes {
    fn deserialize<D>(deserializer: D) -> Result<Bytes, D::Error>
    where
        D: Deserializer<'a>,
    {
        deserializer.deserialize_identifier(BytesVisitor)
    }
}

struct BytesVisitor;

impl Visitor<'_> for BytesVisitor {
    type Value = Bytes;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a bytes")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: Error,
    {
        if value.is_empty() {
            Ok(Bytes::new(vec![]))
        } else if value.len() & 1 == 0 {
            Ok(Bytes::new(
                FromHex::from_hex(&value).map_err(|_| Error::custom("invalid hex"))?,
            ))
        } else {
            Err(Error::custom("invalid format"))
        }
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: Error,
    {
        self.visit_str(value.as_ref())
    }
}

impl ops::Deref for Bytes {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ::std::fmt::LowerHex for Bytes {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        for i in &self.0[..] {
            write!(f, "{i:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex::FromHex;
    use serde_json;

    #[test]
    fn test_bytes_serialize() {
        let bytes = Bytes("0123456789abcdef".from_hex().unwrap());
        let serialized = serde_json::to_string(&bytes).unwrap();
        assert_eq!(serialized, r#""0123456789abcdef""#);
    }

    #[test]
    fn test_bytes_deserialize() {
        let bytes1: Bytes = serde_json::from_str(r#""""#).unwrap();
        let bytes2: Result<Bytes, serde_json::Error> = serde_json::from_str(r#""123""#);
        let bytes3: Result<Bytes, serde_json::Error> = serde_json::from_str(r#""gg""#);

        let bytes4: Bytes = serde_json::from_str(r#""12""#).unwrap();
        let bytes5: Bytes = serde_json::from_str(r#""0123""#).unwrap();
        // tx https://live.blockcypher.com/btc/tx/4ab5828480046524afa3fac5eb7f93f768c3eeeaeb5d4d6b6ff22801d3dc521e/
        // has input with empty hex in scriptSig so deserialization from empty string should be allowed
        assert_eq!(bytes1, Bytes(vec![]));
        assert!(bytes2.is_err());
        assert!(bytes3.is_err());
        assert_eq!(bytes4, Bytes(vec![0x12]));
        assert_eq!(bytes5, Bytes(vec![0x1, 0x23]));
    }
}
