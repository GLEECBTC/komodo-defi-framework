use crate::error::SessionError;

use serde::{Deserialize, Serialize};
use wc_common::SymKey;
use x25519_dalek::{PublicKey, SharedSecret, StaticSecret};
use {
    hkdf::Hkdf,
    rand::{rngs::OsRng, CryptoRng, RngCore},
    sha2::{Digest, Sha256},
};

pub(crate) struct SymKeyPair {
    pub(crate) secret: StaticSecret,
    pub(crate) public_key: PublicKey,
}

impl SymKeyPair {
    pub(crate) fn new() -> Self {
        let static_secret = StaticSecret::random_from_rng(OsRng);
        let public_key = PublicKey::from(&static_secret);
        Self {
            secret: static_secret,
            public_key,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionKey {
    pub(crate) sym_key: SymKey,
    pub(crate) public_key: SymKey,
}

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionKey")
            .field("sym_key", &"*******")
            .field("public_key", &self.public_key)
            .finish()
    }
}

impl SessionKey {
    /// Creates a new `SessionKey` with the given public key and an empty symmetric key.
    pub fn new(public_key: PublicKey) -> Self {
        Self {
            sym_key: [0u8; 32],
            public_key: public_key.to_bytes(),
        }
    }

    /// Creates a new `SessionKey` using a random number generator and a peer's public key.
    pub fn from_osrng(other_public_key: &SymKey) -> Result<Self, SessionError> {
        SessionKey::diffie_hellman(OsRng, other_public_key)
    }

    /// Performs Diffie-Hellman key exchange to derive a symmetric key.
    pub fn diffie_hellman<T>(csprng: T, other_public_key: &SymKey) -> Result<Self, SessionError>
    where
        T: RngCore + CryptoRng,
    {
        let static_private_key = StaticSecret::random_from_rng(csprng);
        let public_key = PublicKey::from(&static_private_key);
        let shared_secret = static_private_key.diffie_hellman(&PublicKey::from(*other_public_key));

        let mut session_key = Self {
            sym_key: [0u8; 32],
            public_key: public_key.to_bytes(),
        };
        session_key.derive_symmetric_key(&shared_secret)?;

        Ok(session_key)
    }

    /// Generates the symmetric key using the static secret and the peer's public key.
    pub fn generate_symmetric_key(
        &mut self,
        static_secret: &StaticSecret,
        peer_public_key: &SymKey,
    ) -> Result<(), SessionError> {
        let shared_secret = static_secret.diffie_hellman(&PublicKey::from(*peer_public_key));
        self.derive_symmetric_key(&shared_secret)
    }

    /// Derives the symmetric key from a shared secret.
    fn derive_symmetric_key(&mut self, shared_secret: &SharedSecret) -> Result<(), SessionError> {
        let hk = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
        hk.expand(&[], &mut self.sym_key)
            .map_err(|e| SessionError::SymKeyGeneration(e.to_string()))
    }

    /// Gets symmetic key reference.
    pub fn symmetric_key(&self) -> SymKey {
        self.sym_key
    }

    /// Gets "our" public key used in symmetric key derivation.
    pub fn diffie_public_key(&self) -> SymKey {
        self.public_key
    }

    /// Generates new session topic.
    pub fn generate_topic(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.sym_key);
        hex::encode(hasher.finalize())
    }
}

#[cfg(test)]
mod session_key_tests {
    use super::*;
    use anyhow::Result;
    use rand::rngs::OsRng;
    use x25519_dalek::{PublicKey, StaticSecret};

    #[test]
    fn test_diffie_hellman_key_exchange() -> Result<()> {
        // Alice's key pair
        let alice_static_secret = StaticSecret::random_from_rng(OsRng);
        let alice_public_key = PublicKey::from(&alice_static_secret);

        // Bob's key pair
        let bob_static_secret = StaticSecret::random_from_rng(OsRng);
        let bob_public_key = PublicKey::from(&bob_static_secret);

        // Alice computes shared secret and session key
        let alice_shared_secret = alice_static_secret.diffie_hellman(&bob_public_key);
        let mut alice_session_key = SessionKey::new(alice_public_key);
        alice_session_key.derive_symmetric_key(&alice_shared_secret)?;

        // Bob computes shared secret and session key
        let bob_shared_secret = bob_static_secret.diffie_hellman(&alice_public_key);
        let mut bob_session_key = SessionKey::new(bob_public_key);
        bob_session_key.derive_symmetric_key(&bob_shared_secret)?;

        // Both symmetric keys should be the same
        assert_eq!(alice_session_key.symmetric_key(), bob_session_key.symmetric_key());

        // Ensure public keys are different
        assert_ne!(alice_session_key.public_key, bob_session_key.public_key);

        Ok(())
    }

    #[test]
    fn test_generate_symmetric_key() -> Result<()> {
        // Alice's key pair
        let alice_static_secret = StaticSecret::random_from_rng(OsRng);
        let alice_public_key = PublicKey::from(&alice_static_secret);

        // Bob's public key
        let bob_static_secret = StaticSecret::random_from_rng(OsRng);
        let bob_public_key = PublicKey::from(&bob_static_secret);

        // Alice initializes session key
        let mut alice_session_key = SessionKey::new(alice_public_key);

        // Alice generates symmetric key using Bob's public key
        alice_session_key.generate_symmetric_key(&alice_static_secret, &bob_public_key.to_bytes())?;

        // Bob computes shared secret and session key
        let bob_shared_secret = bob_static_secret.diffie_hellman(&alice_public_key);
        let mut bob_session_key = SessionKey::new(bob_public_key);
        bob_session_key.derive_symmetric_key(&bob_shared_secret)?;

        // Both symmetric keys should be the same
        assert_eq!(alice_session_key.symmetric_key(), bob_session_key.symmetric_key());

        Ok(())
    }

    #[test]
    fn test_from_osrng() -> Result<()> {
        // Bob's public key
        let bob_static_secret = StaticSecret::random_from_rng(OsRng);
        let bob_public_key = PublicKey::from(&bob_static_secret);

        // Alice creates session key using from_osrng
        let alice_session_key = SessionKey::from_osrng(&bob_public_key.to_bytes())?;

        // Bob computes shared secret and session key
        let bob_shared_secret = bob_static_secret.diffie_hellman(&PublicKey::from(alice_session_key.public_key));
        let mut bob_session_key = SessionKey::new(bob_public_key);
        bob_session_key.derive_symmetric_key(&bob_shared_secret)?;

        // Both symmetric keys should be the same
        assert_eq!(alice_session_key.symmetric_key(), bob_session_key.symmetric_key());

        Ok(())
    }

    #[test]
    fn test_debug_trait() {
        let static_secret = StaticSecret::random_from_rng(OsRng);
        let public_key = PublicKey::from(&static_secret);
        let session_key = SessionKey::new(public_key);

        let debug_str = format!("{session_key:?}");
        assert!(debug_str.contains("SessionKey"));
        assert!(debug_str.contains("sym_key: \"*******\""));
    }
}
