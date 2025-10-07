//! Bitcoin key pair.

use crate::SECP_SIGN;
use crypto::ChecksumType;
use hash::{H264, H520};
use secp256k1::{PublicKey, SecretKey};
use std::fmt;
use {Error, Private, Public, Secret};

#[derive(Clone, Copy, Default, PartialEq)]
pub struct KeyPair {
    private: Private,
    public: Public,
}

impl fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.private.fmt(f)?;
        writeln!(f, "public: {:?}", self.public)
    }
}

impl fmt::Display for KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "private: {}", self.private)?;
        writeln!(f, "public: {}", self.public)
    }
}

impl KeyPair {
    pub fn private(&self) -> &Private {
        &self.private
    }

    pub fn private_bytes(&self) -> [u8; 32] {
        self.private.secret.take()
    }

    pub fn private_ref(&self) -> &[u8; 32] {
        &self.private.secret
    }

    pub fn public(&self) -> &Public {
        &self.public
    }

    pub fn public_slice(&self) -> &[u8] {
        &self.public
    }

    pub fn from_private(private: Private) -> Result<KeyPair, Error> {
        let s: SecretKey = SecretKey::from_slice(&*private.secret)?;
        let pub_key = PublicKey::from_secret_key(&SECP_SIGN, &s);

        let public = if private.compressed {
            let mut public = H264::default();
            let serialized = pub_key.serialize();
            public.copy_from_slice(&serialized[0..33]);
            Public::Compressed(public)
        } else {
            let mut public = H520::default();
            let serialized = pub_key.serialize_uncompressed();
            public.copy_from_slice(&serialized[0..65]);
            Public::Normal(public)
        };

        let keypair = KeyPair { private, public };

        Ok(keypair)
    }

    pub fn from_keypair(sec: SecretKey, public: PublicKey, prefix: u8) -> Self {
        let serialized = public.serialize_uncompressed();
        let mut secret = Secret::default();
        secret.copy_from_slice(&sec[..]);
        let mut public = H520::default();
        public.copy_from_slice(&serialized[0..65]);

        KeyPair {
            private: Private {
                prefix,
                secret,
                compressed: false,
                checksum_type: ChecksumType::DSHA256,
            },
            public: Public::Normal(public),
        }
    }

    pub fn random_compressed() -> Self {
        let secp_secret = SecretKey::new(&mut rand::thread_rng());
        let pub_key = PublicKey::from_secret_key(&SECP_SIGN, &secp_secret);

        KeyPair {
            private: Private {
                prefix: 0,
                secret: (*secp_secret.as_ref()).into(),
                compressed: true,
                checksum_type: Default::default(),
            },
            public: Public::Compressed(pub_key.serialize().into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::KeyPair;
    use crypto::dhash256;

    /// Tests from:
    /// https://github.com/bitcoin/bitcoin/blob/a6a860796a44a2805a58391a009ba22752f64e32/src/test/key_tests.cpp
    const SECRET_0: &str = "5KSCKP8NUyBZPCCQusxRwgmz9sfvJQEgbGukmmHepWw5Bzp95mu";
    const SECRET_1: &str = "5HxWvvfubhXpYYpS3tJkw6fq9jE9j18THftkZjHHfmFiWtmAbrj";
    const SECRET_2: &str = "5KC4ejrDjv152FGwP386VD1i2NYc5KkfSMyv1nGy1VGDxGHqVY3";
    const SECRET_1C: &str = "Kwr371tjA9u2rFSMZjTNun2PXXP3WPZu2afRHTcta6KxEUdm1vEw";
    const SECRET_2C: &str = "L3Hq7a8FEQwJkW1M2GNKDW28546Vp5miewcCzSqUD9kCAXrJdS3g";
    const SIGN_1: &str = "304402205dbbddda71772d95ce91cd2d14b592cfbc1dd0aabd6a394b6c2d377bbe59d31d022014ddda21494a4e221f0824f0b8b924c43fa43c0ad57dccdaa11f81a6bd4582f6";
    const SIGN_2: &str = "3044022052d8a32079c11e79db95af63bb9600c5b04f21a9ca33dc129c2bfa8ac9dc1cd5022061d8ae5e0f6c1a16bde3719c64c2fd70e404b6428ab9a69566962e8771b5944d";
    #[allow(dead_code)]
    const SIGN_COMPACT_1: &str = "1c5dbbddda71772d95ce91cd2d14b592cfbc1dd0aabd6a394b6c2d377bbe59d31d14ddda21494a4e221f0824f0b8b924c43fa43c0ad57dccdaa11f81a6bd4582f6";
    #[allow(dead_code)]
    const SIGN_COMPACT_1C: &str = "205dbbddda71772d95ce91cd2d14b592cfbc1dd0aabd6a394b6c2d377bbe59d31d14ddda21494a4e221f0824f0b8b924c43fa43c0ad57dccdaa11f81a6bd4582f6";
    #[allow(dead_code)]
    const SIGN_COMPACT_2: &str = "1c52d8a32079c11e79db95af63bb9600c5b04f21a9ca33dc129c2bfa8ac9dc1cd561d8ae5e0f6c1a16bde3719c64c2fd70e404b6428ab9a69566962e8771b5944d";
    #[allow(dead_code)]
    const SIGN_COMPACT_2C: &str = "2052d8a32079c11e79db95af63bb9600c5b04f21a9ca33dc129c2bfa8ac9dc1cd561d8ae5e0f6c1a16bde3719c64c2fd70e404b6428ab9a69566962e8771b5944d";

    fn check_compressed(secret: &'static str, compressed: bool) -> bool {
        let kp = KeyPair::from_private(secret.into()).unwrap();
        kp.private().compressed == compressed
    }

    fn check_sign(secret: &'static str, raw_message: &[u8], signature: &'static str) -> bool {
        let message = dhash256(raw_message);
        let kp = KeyPair::from_private(secret.into()).unwrap();
        kp.private().sign_low_r(&message).unwrap() == signature.into()
    }

    fn check_verify(secret: &'static str, raw_message: &[u8], signature: &'static str) -> bool {
        let message = dhash256(raw_message);
        let kp = KeyPair::from_private(secret.into()).unwrap();
        kp.public().verify(&message, &signature.into()).unwrap()
    }

    #[test]
    fn test_keypair_is_compressed() {
        assert!(check_compressed(SECRET_0, false));
        assert!(check_compressed(SECRET_1, false));
        assert!(check_compressed(SECRET_2, false));
        assert!(check_compressed(SECRET_1C, true));
        assert!(check_compressed(SECRET_2C, true));
    }

    #[test]
    fn test_sign() {
        let message = b"Very deterministic message";
        assert!(check_sign(SECRET_1, message, SIGN_1));
        assert!(check_sign(SECRET_1C, message, SIGN_1));
        assert!(check_sign(SECRET_2, message, SIGN_2));
        assert!(check_sign(SECRET_2C, message, SIGN_2));
        assert!(!check_sign(SECRET_2C, b"", SIGN_2));
    }

    #[test]
    fn test_verify() {
        let message = b"Very deterministic message";
        assert!(check_verify(SECRET_1, message, SIGN_1));
        assert!(check_verify(SECRET_1C, message, SIGN_1));
        assert!(check_verify(SECRET_2, message, SIGN_2));
        assert!(check_verify(SECRET_2C, message, SIGN_2));
        assert!(!check_verify(SECRET_2C, b"", SIGN_2));
    }
}
