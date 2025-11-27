//! This module is inspired by the [xpub-converter](https://jlopp.github.io/xpub-converter/).
//! The source code https://github.com/jlopp/xpub-converter/blob/master/js/xpubConvert.js

use bs58::decode::Error as Base58Error;
use derive_more::Display;
use hw_common::primitives::XPub;
use mm2_err_handle::prelude::*;

/// `xpub` prefix is the `[4, 136, 178, 30]` bytes encoded into `base58`.
/// The result of `Buffer.from('0488b21e','hex')`.
/// https://github.com/jlopp/xpub-converter/blob/master/js/xpubConvert.js#L11
const XPUB_PREFIX_RAW: [u8; 4] = [4, 136, 178, 30];

#[derive(Debug, Display)]
pub enum XpubError {
    #[display(fmt = "Unknown prefix")]
    UnknownPrefix,
    #[display(fmt = "base58 error: {_0}")]
    Base58Error(Base58Error),
}

impl From<Base58Error> for XpubError {
    fn from(e: Base58Error) -> Self {
        XpubError::Base58Error(e)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct XPubConverter;

impl XPubConverter {
    /// Checks if the given string is a standard Xpub public key.
    /// The input string must start with the standard `xpub` prefix and contain a valid checksum.
    pub fn is_standard_xpub(xpub: &str) -> MmResult<(), XpubError> {
        let bytes = bs58::decode(&xpub).with_check(None).into_vec()?;
        if has_xpub_prefix(&bytes).map_mm_err()? {
            Ok(())
        } else {
            MmError::err(XpubError::UnknownPrefix)
        }
    }

    /// Replaces a magic prefix (like `dgub`, `ypub` etc) with the standard `xpub` prefix.
    /// List of magic prefixes: https://github.com/satoshilabs/slips/blob/master/slip-0132.md
    pub fn replace_magic_prefix(xpub: XPub) -> MmResult<XPub, XpubError> {
        let mut bytes = bs58::decode(&xpub).with_check(None).into_vec()?;

        if has_xpub_prefix(&bytes).map_mm_err()? {
            return Ok(xpub);
        }

        // Replace the magic prefix (first 4 bytes) with the `xpub` prefix.
        // `has_xpub_prefix` checks if the buffer is longer than 4 bytes.
        bytes.splice(0..4, XPUB_PREFIX_RAW);

        // Encode the bytes into a `base58` string.
        Ok(bs58::encode(bytes).with_check().into_string())
    }
}

fn has_xpub_prefix(bytes: &[u8]) -> MmResult<bool, Base58Error> {
    if bytes.len() < 4 {
        return MmError::err(Base58Error::BufferTooSmall);
    }
    Ok(bytes[0..4] == XPUB_PREFIX_RAW)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem;

    #[test]
    fn test_xpub_check() {
        XPubConverter::is_standard_xpub("xpub6FZD3nMbn98RuBforZLU7N4zArVkmM2m9UwjLX4vtdmmgq8YXPyHeAhCUxFS8wuqoQ9GwSuoSyGdHNv58ZyT5a3wXwYAq83PRyWHgoaA85M").unwrap();

        // Must fail with an invalid checksum error.
        let error = XPubConverter::is_standard_xpub("xpub6DUpU8UQuf4KL15Mc3tYPTTCb44K16q4u7E76iB7FyCvMLmuypkZ9a2UpDGSCN1e2LswKnyov9bbjiXn1oh6FkekAwaEzp7wJAoBBawBwMB").unwrap_err();
        match error.into_inner() {
            XpubError::Base58Error(Base58Error::InvalidChecksum { .. }) => (),
            other => panic!("Expected 'InvalidChecksum' error, found {:?}", other),
        }

        let error = XPubConverter::is_standard_xpub("dgub8sze3tX1SkRjWEwiuLhVpYk7qMCp4fyawZRdg2BLaBzeuYBNvVGupd7BBHmRaLR725Ppmgg7X9oYkSqoaYLqFaWJCdykX5u3em5nu7kmxtZ").unwrap_err();
        // The error variant should equal to `UnknownPrefix`
        assert_eq!(
            mem::discriminant(&error.into_inner()),
            mem::discriminant(&XpubError::UnknownPrefix)
        );
    }

    #[test]
    fn test_xpub_replace_magic_prefix() {
        let xpub = XPubConverter::replace_magic_prefix("dgub8sze3tX1SkRjWEwiuLhVpYk7qMCp4fyawZRdg2BLaBzeuYBNvVGupd7BBHmRaLR725Ppmgg7X9oYkSqoaYLqFaWJCdykX5u3em5nu7kmxtZ".to_string()).unwrap();
        assert_eq!(xpub, "xpub6DUpU8UQuf4KL15Mc3tYPTTCb44K16q4u7E76iB7FyCvMLmuypkZ9a2UpDGSCN1e2LswKnyov9bbjiXn1oh6FkekAwaEzp7wJAoBBY6GsKm");
    }
}
