use chrono::Utc;
use http::Uri;
use libp2p::identity::{Keypair, PublicKey, SigningError};
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;

/// Represents a message and its corresponding signature.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ProxySign {
    /// Signature of the raw message.
    pub signature_bytes: Vec<u8>,
    /// Unique address of the sign's owner.
    pub address: String,
    /// The raw message that has been signed.
    pub raw_message: RawMessage,
}

/// Essential type that contains information required for generating signed messages (see `ProxySign`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct RawMessage {
    /// This field is used to verify the proxy sign on the Komodo DeFi proxy side.
    pub uri: String,
    /// This field is used to check the payload size on the komodo-defi-proxy side.
    /// Along with the `uri` field it helps confirm that the proxy sign matches the request.
    pub body_size: usize,
    pub public_key_encoded: Vec<u8>,
    pub expires_at: i64,
}

impl RawMessage {
    fn new(uri: &Uri, body_size: usize, public_key_encoded: Vec<u8>, expires_in_seconds: i64) -> Self {
        RawMessage {
            uri: uri.to_string(),
            body_size,
            public_key_encoded,
            expires_at: Utc::now().timestamp() + expires_in_seconds,
        }
    }

    /// Generates a byte vector representation of `self`.
    fn encode(&self) -> Vec<u8> {
        const PREFIX: &str = "Encoded Message for KDP\n";
        let mut bytes = PREFIX.as_bytes().to_owned();
        bytes.extend(self.public_key_encoded.clone());
        bytes.extend(self.uri.to_string().as_bytes().to_owned());
        bytes.extend(self.body_size.to_ne_bytes().to_owned());
        bytes.extend(self.expires_at.to_ne_bytes().to_owned());
        bytes
    }

    /// Generates `ProxySign` using the provided keypair and coin ticker.
    pub fn sign(
        keypair: &Keypair,
        uri: &Uri,
        body_size: usize,
        expires_in_seconds: i64,
    ) -> Result<ProxySign, SigningError> {
        let public_key_encoded = keypair.public().encode_protobuf();
        let address = keypair.public().to_peer_id().to_string();
        let raw_message = RawMessage::new(uri, body_size, public_key_encoded, expires_in_seconds);
        let signature_bytes = keypair.sign(&raw_message.encode())?;

        Ok(ProxySign {
            raw_message,
            address,
            signature_bytes,
        })
    }
}

impl ProxySign {
    /// Validates if the message is still valid based on its expiration time and signature verification.
    pub fn is_valid_message(&self, max_message_exp_secs: u64) -> bool {
        let now = Utc::now().timestamp();
        let remaining_expiration_seconds = u64::try_from(self.raw_message.expires_at - now).unwrap_or(0);

        if remaining_expiration_seconds == 0 || remaining_expiration_seconds > max_message_exp_secs {
            return false;
        }

        let Ok(public_key) = PublicKey::try_decode_protobuf(&self.raw_message.public_key_encoded) else {
            return false;
        };

        if self.address != public_key.to_peer_id().to_string() {
            return false;
        }

        public_key.verify(&self.raw_message.encode(), &self.signature_bytes)
    }
}

#[cfg(test)]
pub mod proxy_signature_tests {
    use libp2p::identity;
    use rand::RngCore;

    use super::*;

    fn generate_ed25519_keypair(mut p2p_key: [u8; 32]) -> identity::Keypair {
        let secret = identity::ed25519::SecretKey::try_from_bytes(&mut p2p_key).expect("Secret length is 32 bytes");
        let keypair = identity::ed25519::Keypair::from(secret);
        identity::Keypair::from(keypair)
    }

    fn os_rng(dest: &mut [u8]) -> Result<(), rand::Error> {
        rand::rngs::OsRng.try_fill_bytes(dest)
    }

    fn random_keypair() -> Keypair {
        let mut p2p_key = [0u8; 32];
        os_rng(&mut p2p_key).unwrap();
        generate_ed25519_keypair(p2p_key)
    }

    #[test]
    fn sign_and_verify() {
        let keypair = random_keypair();
        let signed_proxy_message = RawMessage::sign(&keypair, &Uri::from_static("http://example.com"), 0, 5).unwrap();
        assert!(signed_proxy_message.is_valid_message(10));
    }

    #[test]
    fn expired_signature() {
        let keypair = random_keypair();
        let signed_proxy_message = RawMessage::sign(&keypair, &Uri::from_static("http://example.com"), 0, -1).unwrap();
        assert!(!signed_proxy_message.is_valid_message(10));
    }

    #[test]
    fn dirty_raw_message() {
        let keypair = random_keypair();
        let mut signed_proxy_message =
            RawMessage::sign(&keypair, &Uri::from_static("http://example.com"), 0, 5).unwrap();
        signed_proxy_message.raw_message.uri = "http://demo.com".to_string();
        assert!(!signed_proxy_message.is_valid_message(10));

        let mut signed_proxy_message =
            RawMessage::sign(&keypair, &Uri::from_static("http://example.com"), 0, 5).unwrap();
        signed_proxy_message.raw_message.body_size += 1;
        assert!(!signed_proxy_message.is_valid_message(10));

        let mut signed_proxy_message =
            RawMessage::sign(&keypair, &Uri::from_static("http://example.com"), 0, 5).unwrap();
        signed_proxy_message.raw_message.expires_at += 1;
        assert!(!signed_proxy_message.is_valid_message(10));
    }

    #[test]
    fn message_lifetime_overflow() {
        let keypair = random_keypair();
        let signed_proxy_message = RawMessage::sign(&keypair, &Uri::from_static("http://example.com"), 0, 5).unwrap();
        assert!(!signed_proxy_message.is_valid_message(4));
    }

    #[test]
    fn verify_peer_id() {
        let expected_address = "12D3KooWJPtxrHVDPoETNfPJY4WWVzX7Ti4WPemtXDgb5qmFrDiv";

        let p2p_key = [123u8; 32];
        let keypair = generate_ed25519_keypair(p2p_key);
        assert_eq!(keypair.public().to_peer_id().to_string(), expected_address);

        let signed_proxy_message = RawMessage::sign(&keypair, &Uri::from_static("http://example.com"), 0, 5).unwrap();
        assert_eq!(signed_proxy_message.address, expected_address);
    }
}
