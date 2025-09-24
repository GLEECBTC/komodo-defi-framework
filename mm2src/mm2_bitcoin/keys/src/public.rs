use crate::SECP_VERIFY;
use crypto::dhash160;
use hash::{H160, H264, H520};
use hex::ToHex;
use secp256k1::{
    recovery::{RecoverableSignature, RecoveryId},
    Error as SecpError, Message as SecpMessage, PublicKey, Signature as SecpSignature,
};
use std::{fmt, ops::Deref};
use {CompactSignature, Error, Message, Signature};

/// Secret public key
#[derive(Copy, Clone, Eq)]
pub enum Public {
    /// Normal version of public key
    Normal(H520),
    /// Compressed version of public key
    Compressed(H264),
}

impl Default for Public {
    fn default() -> Public {
        Public::Compressed(H264::default())
    }
}

impl Public {
    pub fn from_slice(data: &[u8]) -> Result<Self, Error> {
        match data.len() {
            33 => {
                let mut public = H264::default();
                public.copy_from_slice(data);
                Ok(Public::Compressed(public))
            },
            65 => {
                let mut public = H520::default();
                public.copy_from_slice(data);
                Ok(Public::Normal(public))
            },
            _ => Err(Error::InvalidPublic),
        }
    }

    pub fn address_hash(&self) -> H160 {
        dhash160(self)
    }

    pub fn verify(&self, message: &Message, signature: &Signature) -> Result<bool, Error> {
        let public = match self {
            Public::Compressed(public) => PublicKey::from_slice(&**public)?,
            Public::Normal(public) => PublicKey::from_slice(&**public)?,
        };
        let mut signature = SecpSignature::from_der_lax(signature)?;
        signature.normalize_s();
        let message = SecpMessage::from_slice(&**message)?;
        Ok(SECP_VERIFY.verify(&message, &signature, &public).is_ok())
    }

    pub fn recover_compact(message: &Message, signature: &CompactSignature) -> Result<Self, Error> {
        if signature[0] < 27 {
            return Err(Error::InvalidSignature);
        };
        let recovery_id = (signature[0] - 27) & 3;
        let compressed = (signature[0] - 27) & 4 != 0;
        let recovery_id = RecoveryId::from_i32(recovery_id as i32)?;
        let signature = RecoverableSignature::from_compact(&signature[1..65], recovery_id)?;
        let message = SecpMessage::from_slice(&**message)?;
        let pubkey = SECP_VERIFY.recover(&message, &signature)?;
        let public = if compressed {
            let serialized = pubkey.serialize();
            Public::Compressed(serialized.into())
        } else {
            let serialized = pubkey.serialize_uncompressed();
            Public::Normal(serialized.into())
        };
        Ok(public)
    }

    pub fn compressed_unprefixed(&self) -> Option<[u8; 32]> {
        let mut res = [0; 32];
        match self {
            Public::Compressed(pubkey) => {
                res.copy_from_slice(&pubkey.as_slice()[1..33]);
                Some(res)
            },
            Public::Normal(_) => None,
        }
    }

    #[inline(always)]
    pub fn to_secp256k1_pubkey(&self) -> Result<PublicKey, SecpError> {
        PublicKey::from_slice(self.deref())
    }
}

impl Deref for Public {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        match *self {
            Public::Normal(ref hash) => &**hash,
            Public::Compressed(ref hash) => &**hash,
        }
    }
}

impl PartialEq for Public {
    fn eq(&self, other: &Self) -> bool {
        let s_slice: &[u8] = self;
        let o_slice: &[u8] = other;
        s_slice == o_slice
    }
}

impl fmt::Debug for Public {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Public::Normal(ref hash) => writeln!(f, "normal: {}", hash.to_hex::<String>()),
            Public::Compressed(ref hash) => writeln!(f, "compressed: {}", hash.to_hex::<String>()),
        }
    }
}

impl fmt::Display for Public {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.to_hex::<String>().fmt(f)
    }
}
