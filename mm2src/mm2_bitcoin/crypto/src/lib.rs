extern crate groestl;
extern crate primitives;
extern crate ripemd160;
extern crate serialization;
extern crate sha1;
extern crate sha2;
extern crate sha3;
extern crate siphasher;

use groestl::Groestl512;
use primitives::hash::{H160, H256, H32, H512};
use ripemd160::{Digest, Ripemd160};
use sha1::Sha1;
use sha2::{Digest as Sha2Digest, Sha256};
use sha3::Keccak256;
use siphasher::sip::SipHasher24;
use std::hash::Hasher;

/// Enum representing different variants of checksum calculation
/// Most coins use double sha256
/// GRS uses double groestl512
/// SMART uses keccak
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ChecksumType {
    #[default]
    DSHA256,
    DGROESTL512,
    KECCAK256,
}

/// RIPEMD160
#[inline]
pub fn ripemd160(input: &[u8]) -> H160 {
    let mut hasher = Ripemd160::new();
    hasher.update(input);
    let array: [u8; 20] = hasher.finalize().into();
    array.into()
}

/// SHA-1
#[inline]
pub fn sha1(input: &[u8]) -> H160 {
    let mut hasher = Sha1::new();
    hasher.update(input);
    let array: [u8; 20] = hasher.finalize().into();
    array.into()
}

/// SHA-256
#[inline]
pub fn sha256(input: &[u8]) -> H256 {
    let mut hasher = Sha256::new();
    hasher.update(input);
    let array: [u8; 32] = hasher.finalize().into();
    array.into()
}

/// Groestl-512
#[inline]
pub fn groestl512(input: &[u8]) -> H512 {
    let mut hasher = Groestl512::new();
    hasher.update(input);
    let array: [u8; 64] = hasher.finalize().into();
    array.into()
}

/// Keccak-256
#[inline]
pub fn keccak256(input: &[u8]) -> H256 {
    let mut hasher = Keccak256::new();
    hasher.update(input);
    let array: [u8; 32] = hasher.finalize().into();
    array.into()
}

/// Double Keccak-256
#[inline]
pub fn dkeccak256(input: &[u8]) -> H256 {
    keccak256(&*keccak256(input))
}

/// SHA-256 and RIPEMD160
#[inline]
pub fn dhash160(input: &[u8]) -> H160 {
    ripemd160(&*sha256(input))
}

/// Double SHA-256
#[inline]
pub fn dhash256(input: &[u8]) -> H256 {
    sha256(&*sha256(input))
}

/// SipHash-2-4
#[inline]
pub fn siphash24(key0: u64, key1: u64, input: &[u8]) -> u64 {
    let mut hasher = SipHasher24::new_with_keys(key0, key1);
    hasher.write(input);
    hasher.finish()
}

/// Double Groestl-512
#[inline]
pub fn dgroestl512(input: &[u8]) -> H512 {
    groestl512(&*groestl512(input))
}

/// Data checksum
#[inline]
pub fn checksum(data: &[u8], sum_type: &ChecksumType) -> H32 {
    let mut result = H32::default();
    match sum_type {
        ChecksumType::DSHA256 => result.copy_from_slice(&dhash256(data)[0..4]),
        ChecksumType::DGROESTL512 => result.copy_from_slice(&dgroestl512(data)[0..4]),
        ChecksumType::KECCAK256 => result.copy_from_slice(&keccak256(data)[0..4]),
    }
    result
}

/// Hash message for signature using Bitcoin's message signing format.
/// sha256(sha256(PREFIX_LENGTH + PREFIX + MESSAGE_LENGTH + MESSAGE))
pub fn sign_message_hash(sign_message_prefix: &str, message: &str) -> [u8; 32] {
    use serialization::{CompactInteger, Serializable, Stream};
    let mut stream = Stream::new();
    let prefix_len = CompactInteger::from(sign_message_prefix.len());
    prefix_len.serialize(&mut stream);
    stream.append_slice(sign_message_prefix.as_bytes());
    let msg_len = CompactInteger::from(message.len());
    msg_len.serialize(&mut stream);
    stream.append_slice(message.as_bytes());
    dhash256(&stream.out()).take()
}

#[cfg(test)]
mod tests {
    use super::{checksum, dhash160, dhash256, ripemd160, sha1, sha256, siphash24};
    use primitives::bytes::Bytes;
    use primitives::hash::{H160, H256, H32};
    use ChecksumType;

    #[test]
    fn test_ripemd160() {
        let expected: H160 = "108f07b8382412612c048d07d13f814118445acd".into();
        let result = ripemd160(b"hello");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_sha1() {
        let expected: H160 = "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d".into();
        let result = sha1(b"hello");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_sha256() {
        let expected: H256 = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into();
        let result = sha256(b"hello");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_dhash160() {
        let expected: H160 = "b6a9c8c230722b7c748331a8b450f05566dc7d0f".into();
        let result = dhash160(b"hello");
        assert_eq!(result, expected);

        let expected: H160 = "865c71bfc7e314709207ab9e7e205c6f8e453d08".into();
        let bytes: Bytes =
            "210292be03ed9475445cc24a34a115c641a67e4ff234ccb08cb4c5cea45caa526cb26ead6ead6ead6ead6eadac".into();
        let result = dhash160(&bytes);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_dhash256() {
        let expected: H256 = "9595c9df90075148eb06860365df33584b75bff782a510c6cd4883a419833d50".into();
        let result = dhash256(b"hello");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_siphash24() {
        let expected = 0x74f839c593dc67fd_u64;
        let result = siphash24(0x0706050403020100_u64, 0x0F0E0D0C0B0A0908_u64, &[0; 1]);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_checksum() {
        assert_eq!(checksum(b"hello", &ChecksumType::DSHA256), H32::from("9595c9df"));
    }
}
