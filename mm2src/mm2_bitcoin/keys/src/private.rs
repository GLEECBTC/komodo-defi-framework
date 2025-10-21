//! Secret with additional network identifier and format type

use crate::SECP_SIGN;
use address::detect_checksum;
use crypto::{checksum, ChecksumType};
use hex::ToHex;
use secp256k1::{Message as SecpMessage, SecretKey};
use std::fmt;
use std::str::FromStr;
use {DisplayLayout, Error, Message, Secret, Signature};

/// Secret with additional network prefix and format type
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Private {
    /// The network prefix on which this key should be used.
    pub prefix: u8,
    /// ECDSA key.
    pub secret: Secret,
    /// True if this private key represents a compressed address.
    pub compressed: bool,
    /// checksum type
    pub checksum_type: ChecksumType,
}

impl Private {
    pub fn sign(&self, message: &Message) -> Result<Signature, Error> {
        let secret = SecretKey::from_slice(&*self.secret)?;
        let message = SecpMessage::from_slice(&**message)?;
        let signature = SECP_SIGN.sign(&message, &secret);
        let data = signature.serialize_der();
        Ok(data.as_ref().to_vec().into())
    }

    /// Sign a message with a low R value, this reduces signature malleability for Bitcoin transactions
    /// and makes fee estimation more reliable.
    pub fn sign_low_r(&self, message: &Message) -> Result<Signature, Error> {
        let secret = SecretKey::from_slice(&*self.secret)?;
        let message = SecpMessage::from_slice(&**message)?;
        let signature = SECP_SIGN.sign_low_r(&message, &secret);
        let data = signature.serialize_der();
        Ok(data.as_ref().to_vec().into())
    }

    // https://github.com/qtumproject/qtum/blob/master/src/key.cpp#L302
    pub fn sign_compact(&self, message: &Message) -> Result<Signature, Error> {
        let secret = SecretKey::from_slice(&*self.secret)?;
        let message = SecpMessage::from_slice(&**message)?;
        let signature = SECP_SIGN.sign_recoverable(&message, &secret);
        let (recover_id, bytes) = signature.serialize_compact();
        let mut out = bytes.to_vec();
        let header = 27 + recover_id.to_i32() as u8;
        let byte: u8 = if self.compressed { header + 4 } else { header };
        out.insert(0, byte);
        Ok(out.into())
    }
}

impl DisplayLayout for Private {
    type Target = Vec<u8>;

    fn layout(&self) -> Self::Target {
        let mut result = vec![self.prefix];
        result.extend(*self.secret);
        if self.compressed {
            result.push(1);
        }
        let cs = checksum(&result, &self.checksum_type);
        result.extend_from_slice(&*cs);
        result
    }

    fn from_layout(data: &[u8]) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let compressed = match data.len() {
            37 => false,
            38 => true,
            _ => return Err(Error::InvalidPrivate),
        };

        if compressed && data[data.len() - 5] != 1 {
            return Err(Error::InvalidPrivate);
        }

        let sum_type = detect_checksum(&data[0..data.len() - 4], &data[data.len() - 4..])?;
        let prefix = data[0];

        let mut secret = Secret::default();
        secret.copy_from_slice(&data[1..33]);

        let private = Private {
            prefix,
            secret,
            compressed,
            checksum_type: sum_type,
        };

        Ok(private)
    }
}

impl fmt::Debug for Private {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "prefix: {:?}", self.prefix)?;
        writeln!(f, "secret: {}", self.secret.to_hex::<String>())?;
        writeln!(f, "compressed: {}", self.compressed)
    }
}

impl fmt::Display for Private {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        bs58::encode(self.layout()).into_string().fmt(f)
    }
}

impl FromStr for Private {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let hex = bs58::decode(s).into_vec().map_err(|_| Error::InvalidPrivate)?;
        Private::from_layout(&hex)
    }
}

impl From<&'static str> for Private {
    fn from(s: &'static str) -> Self {
        s.parse().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::{ChecksumType, Private};
    use hash::H256;

    #[test]
    fn test_private_to_string() {
        let private = Private {
            prefix: 128,
            secret: H256::from_reversed_str("063377054c25f98bc538ac8dd2cf9064dd5d253a725ece0628a34e2f84803bd5"),
            compressed: false,
            checksum_type: ChecksumType::DSHA256,
        };

        assert_eq!(
            "5KSCKP8NUyBZPCCQusxRwgmz9sfvJQEgbGukmmHepWw5Bzp95mu".to_owned(),
            private.to_string()
        );
    }

    #[test]
    fn test_private_to_string_kmd() {
        let private = Private {
            prefix: 188,
            secret: H256::from_reversed_str("063377054c25f98bc538ac8dd2cf9064dd5d253a725ece0628a34e2f84803bd5"),
            compressed: true,
            checksum_type: ChecksumType::DSHA256,
        };

        assert_eq!(
            "UwA3FpHWKfwrQ1DTiwbErpEnCEhvLuq1WnbfmqGBPSLNNvXtzYd5".to_owned(),
            private.to_string()
        );
    }

    #[test]
    fn test_private_to_string_zec_testnet() {
        let private = Private {
            prefix: 239,
            secret: H256::from_reversed_str("063377054c25f98bc538ac8dd2cf9064dd5d253a725ece0628a34e2f84803bd5"),
            compressed: true,
            checksum_type: ChecksumType::DSHA256,
        };

        assert_eq!(
            "cUjCR3fPFWfs6PtdvoinTh4ctPxBvFf5pKNKJzw1RqmfjogL7GuU".to_owned(),
            private.to_string()
        );
    }

    #[test]
    fn test_private_from_str() {
        let private = Private {
            prefix: 128,
            secret: H256::from_reversed_str("063377054c25f98bc538ac8dd2cf9064dd5d253a725ece0628a34e2f84803bd5"),
            compressed: false,
            checksum_type: ChecksumType::DSHA256,
        };

        assert_eq!(private, "5KSCKP8NUyBZPCCQusxRwgmz9sfvJQEgbGukmmHepWw5Bzp95mu".into());
    }

    #[test]
    fn test_private_from_str_kmd() {
        let private = Private {
            prefix: 188,
            secret: H256::from_reversed_str("063377054c25f98bc538ac8dd2cf9064dd5d253a725ece0628a34e2f84803bd5"),
            compressed: true,
            checksum_type: ChecksumType::DSHA256,
        };

        assert_eq!(private, "UwA3FpHWKfwrQ1DTiwbErpEnCEhvLuq1WnbfmqGBPSLNNvXtzYd5".into());
    }

    #[test]
    fn test_private_from_str_zec_testnet() {
        let private = Private {
            prefix: 239,
            secret: H256::from_reversed_str("063377054c25f98bc538ac8dd2cf9064dd5d253a725ece0628a34e2f84803bd5"),
            compressed: true,
            checksum_type: ChecksumType::DSHA256,
        };

        assert_eq!(private, "cUjCR3fPFWfs6PtdvoinTh4ctPxBvFf5pKNKJzw1RqmfjogL7GuU".into());
    }

    #[test]
    fn test_private_from_str_grs() {
        let private = Private {
            prefix: 128,
            secret: H256::from_reversed_str("cbc8853bd3617a5fcecfcc97f4a68853481657fc575cf85e04a64a2d1a78f974"),
            compressed: true,
            checksum_type: ChecksumType::DGROESTL512,
        };

        assert_eq!(private, "L196QUb5fAcBVvZizvx66ABsU7iVTS4iAz15YEgB8QWY35KfD6ox".into());
        assert_eq!(
            private.to_string(),
            "L196QUb5fAcBVvZizvx66ABsU7iVTS4iAz15YEgB8QWY35KfD6ox".to_owned()
        );
    }

    #[test]
    fn test_private_from_str_smart_cash() {
        let private = Private {
            prefix: 191,
            secret: H256::from_reversed_str("48688b0cd9440864b95916f53d6e06cdab5f50dc3abfa74b5c6a176620daa302"),
            compressed: true,
            checksum_type: ChecksumType::KECCAK256,
        };

        assert_eq!(private, "VFqZrZNzkJEk29Kzp87J7eXDuQFMh1UsqYcMmi9bfdAZ522nz1mv".into());
        assert_eq!(
            private.to_string(),
            "VFqZrZNzkJEk29Kzp87J7eXDuQFMh1UsqYcMmi9bfdAZ522nz1mv".to_owned()
        );
    }
}
