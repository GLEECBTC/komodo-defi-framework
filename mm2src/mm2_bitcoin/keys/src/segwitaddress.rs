use std::fmt;
use std::str::FromStr;

use bech32;
use LockingDestination;

/// Address error.
#[derive(Debug, PartialEq)]
pub enum Error {
    /// Invalid address format
    InvalidSegwitAddressFormat,
    /// Bech32 encoding error
    Bech32(bech32::Error),
    /// The bech32 payload was empty
    EmptyBech32Payload,
    /// Script version must be 0 to 16 inclusive
    InvalidWitnessVersion(u8),
    /// The witness program must be between 2 and 40 bytes in length.
    InvalidWitnessProgramLength(usize),
    /// A v0 witness program must be either of length 20 or 32.
    InvalidSegwitV0ProgramLength(usize),
    /// An uncompressed pubkey was used where it is not allowed.
    UncompressedPubkey,
    /// An address variant that is not supported yet was used.
    UnsupportedAddressVariant(String),
    /// A script version that is not supported yet was used.
    UnsupportedWitnessVersion(u8),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::InvalidSegwitAddressFormat => write!(f, "Invalid segwit address format"),
            Error::Bech32(ref e) => write!(f, "bech32: {e}"),
            Error::EmptyBech32Payload => write!(f, "the bech32 payload was empty"),
            Error::InvalidWitnessVersion(v) => write!(f, "invalid witness script version: {v}"),
            Error::InvalidWitnessProgramLength(l) => write!(
                f,
                "the witness program must be between 2 and 40 bytes in length: length={l}",
            ),
            Error::InvalidSegwitV0ProgramLength(l) => write!(
                f,
                "a v0 witness program must be either of length 20 or 32 bytes: length={l}",
            ),
            Error::UncompressedPubkey => write!(f, "an uncompressed pubkey was used where it is not allowed",),
            Error::UnsupportedAddressVariant(ref v) => write!(f, "address variant/format {v} is not supported yet!"),
            Error::UnsupportedWitnessVersion(v) => write!(f, "witness script version: {v} is not supported yet!"),
        }
    }
}

#[doc(hidden)]
impl From<bech32::Error> for Error {
    fn from(e: bech32::Error) -> Error {
        Error::Bech32(e)
    }
}

/// The different types of segwit addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SegwitAddrType {
    /// pay-to-witness-public-key-hash
    P2wpkh,
    /// pay-to-witness-script-hash
    P2wsh,
    /// pay-to-taproot
    P2tr,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// A Bitcoin segwit address
pub struct SegwitAddress {
    /// The human-readable part
    pub hrp: String,
    /// The witness program version
    version: bech32::u5,
    /// The witness program
    pub program: Vec<u8>,
}

impl SegwitAddress {
    pub fn new(program: &LockingDestination, hrp: String, version: u8) -> Result<SegwitAddress, Error> {
        if version > 16 {
            return Err(Error::InvalidWitnessVersion(version));
        }
        Ok(SegwitAddress {
            hrp,
            version: bech32::u5::try_from_u8(version).expect("version must be < 16, thus also < 32"),
            program: program.to_vec(),
        })
    }

    pub fn version_as_u8(&self) -> u8 {
        self.version.to_u8()
    }

    /// Get the address type of the address.
    /// None if unknown or non-standard.
    pub fn address_type(&self) -> Option<SegwitAddrType> {
        // BIP-141 p2wpkh or p2wsh addresses.
        match self.version_as_u8() {
            0 if self.program.len() == 20 => Some(SegwitAddrType::P2wpkh),
            0 if self.program.len() == 32 => Some(SegwitAddrType::P2wsh),
            1 if self.program.len() == 32 => Some(SegwitAddrType::P2tr),
            // Future versions or non-standard program sizes.
            _ => None,
        }
    }

    /// Check whether or not the address is following Bitcoin
    /// standardness rules.
    ///
    /// Segwit addresses with unassigned witness versions or non-standard
    /// program sizes are considered non-standard.
    pub fn is_standard(&self) -> bool {
        self.address_type().is_some()
    }
}

struct UpperWriter<W: fmt::Write>(W);

impl<W: fmt::Write> fmt::Write for UpperWriter<W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            self.0.write_char(c.to_ascii_uppercase())?;
        }
        Ok(())
    }
}

// Alternate formatting `{:#}` is used to return uppercase version of bech32 addresses which should
// be used in QR codes, see [Address::to_qr_uri]
impl fmt::Display for SegwitAddress {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let mut upper_writer;
        let writer = if fmt.alternate() {
            upper_writer = UpperWriter(fmt);
            &mut upper_writer as &mut dyn fmt::Write
        } else {
            fmt as &mut dyn fmt::Write
        };
        let bech32_version = match self.version.to_u8() {
            0 => bech32::Variant::Bech32,
            1 => bech32::Variant::Bech32m,
            // Ideally, all v1+ segwit addresses should be formatted using Bech32m.
            // But let's error on such attempts unless we explicitly support higher versions.
            _ => return Err(fmt::Error),
        };
        let mut bech32_writer = bech32::Bech32Writer::new(self.hrp.as_str(), bech32_version, writer)?;
        bech32::WriteBase32::write_u5(&mut bech32_writer, self.version)?;
        bech32::ToBase32::write_base32(&self.program, &mut bech32_writer)
    }
}

impl FromStr for SegwitAddress {
    type Err = Error;

    fn from_str(s: &str) -> Result<SegwitAddress, Error> {
        // decode as bech32, should use Variant if Bech32m is used alongside Bech32
        // The improved Bech32m variant described in [BIP-0350](https://github.com/bitcoin/bips/blob/master/bip-0350.mediawiki)
        // hrp checks (mixed case not allowed, allowed length and characters) are part of the decode function
        let (hrp, payload, variant) = bech32::decode(s)?;
        if payload.is_empty() {
            return Err(Error::EmptyBech32Payload);
        }

        // We perform this match to trigger a compilation error if a new variant gets added that we didn't handle yet.
        match variant {
            bech32::Variant::Bech32 => (),
            bech32::Variant::Bech32m => (),
        }

        // Get the script version and program (converted from 5-bit to 8-bit)
        let (version, program): (bech32::u5, Vec<u8>) = {
            let (v, p5) = payload.split_at(1);
            (v[0], bech32::FromBase32::from_base32(p5)?)
        };

        // Generic segwit checks.
        if version.to_u8() > 16 {
            return Err(Error::InvalidWitnessVersion(version.to_u8()));
        }

        // Only support segwit v0 and v1. Note that we are relaxing this check in tests
        // so to test the detection of other errors (invalid length, wrong encoding used, etc...)
        #[cfg(not(test))]
        if ![0, 1].contains(&version.to_u8()) {
            return Err(Error::UnsupportedWitnessVersion(version.to_u8()));
        }

        if program.len() < 2 || program.len() > 40 {
            return Err(Error::InvalidWitnessProgramLength(program.len()));
        }

        if version.to_u8() == 0 {
            // Bech32 length check for segwit v0 (later versions use bech32m which isn't vulnerable to this problem).
            // Important: we should be careful when using new program lengths since a valid Bech32 string can be modified according to
            // the below 2 links while still having a valid checksum.
            // https://github.com/bitcoin/bips/blob/master/bip-0350.mediawiki#motivation
            // https://github.com/sipa/bech32/issues/51
            if program.len() != 20 && program.len() != 32 {
                return Err(Error::InvalidSegwitV0ProgramLength(program.len()));
            }
            if variant == bech32::Variant::Bech32m {
                return Err(Error::UnsupportedAddressVariant(
                    "Bech32m is not supported for witness version 0. Bech32 should be used instead.".into(),
                ));
            }
        }

        if version.to_u8() != 0 && variant == bech32::Variant::Bech32 {
            return Err(Error::UnsupportedAddressVariant(format!(
                "Bech32 is not supported for witness version {}. Bech32m should be used instead.",
                version.to_u8()
            )));
        }

        Ok(SegwitAddress { hrp, version, program })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::sha256;
    use hex::ToHex;
    use primitives::hash::H256;
    use Public;

    fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
        if s.len().is_multiple_of(2) {
            (0..s.len())
                .step_by(2)
                .map(|i| s.get(i..i + 2).and_then(|sub| u8::from_str_radix(sub, 16).ok()))
                .collect()
        } else {
            None
        }
    }

    #[test]
    fn test_p2wpkh_address() {
        // Bitcoin transaction: b3c8c2b6cfc335abbcb2c7823a8453f55d64b2b5125a9a61e8737230cdb8ce20
        let pk = "033bc8c83c52df5712229a2f72206d90192366c36428cb0c12b6af98324d97bfbc";
        let bytes = hex_to_bytes(pk).unwrap();
        let public_key = Public::from_slice(&bytes).unwrap();
        let hash = public_key.address_hash();
        let hrp = "bc";
        let addr = SegwitAddress::new(&LockingDestination::AddressHash(hash), hrp.to_string(), 0).unwrap();
        assert_eq!(&addr.to_string(), "bc1qvzvkjn4q3nszqxrv3nraga2r822xjty3ykvkuw");
        assert_eq!(addr.address_type(), Some(SegwitAddrType::P2wpkh));
    }

    #[test]
    fn test_p2wsh_address() {
        let script = "210279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798ac";
        let bytes = hex_to_bytes(script).unwrap();
        let hash = sha256(&bytes);
        let hrp = "bc";
        let addr = SegwitAddress::new(&LockingDestination::WitnessScriptHash(hash), hrp.to_string(), 0).unwrap();
        assert_eq!(
            &addr.to_string(),
            "bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3"
        );
        assert_eq!(addr.address_type(), Some(SegwitAddrType::P2wsh));
    }

    #[test]
    fn test_p2tr_address() {
        let x_only_pub = "d5e89e0b73605abba690ba5e00484e279d006283bed0055a0530fb6a8c9adac7";
        let bytes = hex_to_bytes(x_only_pub).unwrap();
        let x_only_pub = H256::from_slice(&bytes).unwrap();
        let hrp = "tb";
        let addr = SegwitAddress::new(&LockingDestination::TweakedXOnlyPubkey(x_only_pub), hrp.to_string(), 1).unwrap();
        assert_eq!(
            &addr.to_string(),
            "tb1p6h5fuzmnvpdthf5shf0qqjzwy7wsqc5rhmgq2ks9xrak4ry6mtrscsqvzp"
        );
        assert_eq!(addr.address_type(), Some(SegwitAddrType::P2tr));
    }

    #[test]
    // https://github.com/bitcoin/bips/blob/master/bip-0173.mediawiki#test-vectors
    fn test_valid_segwit() {
        // p2wpkh
        let addr = "BC1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4";
        let segwit_addr = SegwitAddress::from_str(addr).unwrap();
        assert_eq!(0, segwit_addr.version.to_u8());
        assert_eq!(
            "751e76e8199196d454941c45d1b3a323f1433bd6",
            segwit_addr.program.to_hex::<String>()
        );
        // p2wsh
        let addr = "tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7";
        let segwit_addr = SegwitAddress::from_str(addr).unwrap();
        assert_eq!(0, segwit_addr.version.to_u8());
        assert_eq!(
            "1863143c14c5166804bd19203356da136c985678cd4d27a1b8c6329604903262",
            segwit_addr.program.to_hex::<String>()
        );
        // p2wsh
        let addr = "tb1qqqqqp399et2xygdj5xreqhjjvcmzhxw4aywxecjdzew6hylgvsesrxh6hy";
        let segwit_addr = SegwitAddress::from_str(addr).unwrap();
        assert_eq!(0, segwit_addr.version.to_u8());
        assert_eq!(
            "000000c4a5cad46221b2a187905e5266362b99d5e91c6ce24d165dab93e86433",
            segwit_addr.program.to_hex::<String>()
        );
        // p2tr
        let addr = "tb1p6h5fuzmnvpdthf5shf0qqjzwy7wsqc5rhmgq2ks9xrak4ry6mtrscsqvzp";
        let segwit_addr = SegwitAddress::from_str(addr).unwrap();
        assert_eq!(1, segwit_addr.version.to_u8());
        assert_eq!(
            "d5e89e0b73605abba690ba5e00484e279d006283bed0055a0530fb6a8c9adac7",
            segwit_addr.program.to_hex::<String>()
        );
    }

    #[test]
    // https://github.com/bitcoin/bips/blob/master/bip-0173.mediawiki#test-vectors
    // https://github.com/bitcoin/bips/blob/master/bip-0350.mediawiki#test-vectors
    fn test_invalid_segwit_addresses() {
        // Invalid checksum
        let invalid_address = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t5";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(err, Error::Bech32(bech32::Error::InvalidChecksum));

        // Invalid witness version
        let invalid_address = "BC13W508D6QEJXTDG4Y5R3ZARVARY0C5XW7KN40WF2";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(err, Error::InvalidWitnessVersion(17));

        // Invalid program length (bech32)
        let invalid_address = "bc1rw5uspcuh";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(err, Error::InvalidWitnessProgramLength(1));

        // Invalid program length (bech32m)
        let invalid_address = "bc1pw5dgrnzv";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(err, Error::InvalidWitnessProgramLength(1));

        // Invalid program length
        let invalid_address = "bc10w508d6qejxtdg4y5r3zarvary0c5xw7kw508d6qejxtdg4y5r3zarvary0c5xw7kw5rljs90";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(err, Error::InvalidWitnessProgramLength(41));

        // Invalid program length for witness version 0 (per BIP141)
        let invalid_address = "BC1QR508D6QEJXTDG4Y5R3ZARVARYV98GJ9P";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(err, Error::InvalidSegwitV0ProgramLength(16));

        // Mixed case invalid
        let invalid_address = "tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sL5k7";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(err, Error::Bech32(bech32::Error::MixedCase));

        // zero padding of more than 4 bits
        let invalid_address = "bc1zw508d6qejxtdg4y5r3zarvaryvqyzf3du";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(err, Error::Bech32(bech32::Error::InvalidPadding));

        // Non-zero padding in 8-to-5 conversion
        let invalid_address = "tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3pjxtptv";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(err, Error::Bech32(bech32::Error::InvalidPadding));

        // Empty data section
        let invalid_address = "bc1gmk9yu";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(err, Error::EmptyBech32Payload);

        // Version 1 shouldn't be used with bech32 variant although the below address is given as valid in BIP173
        // https://github.com/bitcoin/bips/blob/master/bip-0350.mediawiki#abstract
        let invalid_address = "bc1pw508d6qejxtdg4y5r3zarvary0c5xw7kw508d6qejxtdg4y5r3zarvary0c5xw7k7grplx";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(
            err,
            Error::UnsupportedAddressVariant(
                "Bech32 is not supported for witness version 1. Bech32m should be used instead.".into()
            )
        );

        // Invalid checksum for version 0 (bech32m instead of bech32)
        let invalid_address = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kemeawh";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(
            err,
            Error::UnsupportedAddressVariant(
                "Bech32m is not supported for witness version 0. Bech32 should be used instead.".into()
            )
        );

        // Version 16 shouldn't be used with bech32
        let invalid_address = "BC1SW50QA3JX3S";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(
            err,
            Error::UnsupportedAddressVariant(
                "Bech32 is not supported for witness version 16. Bech32m should be used instead.".into()
            )
        );

        // Version 2 shouldn't be used with bech32
        let invalid_address = "bc1zw508d6qejxtdg4y5r3zarvaryvg6kdaj";
        let err = SegwitAddress::from_str(invalid_address).unwrap_err();
        assert_eq!(
            err,
            Error::UnsupportedAddressVariant(
                "Bech32 is not supported for witness version 2. Bech32m should be used instead.".into()
            )
        );
    }
}
