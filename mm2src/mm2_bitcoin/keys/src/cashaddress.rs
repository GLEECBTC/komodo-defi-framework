use std::fmt;
use std::str::FromStr;

const DEFAULT_PREFIX: NetworkPrefix = NetworkPrefix::BitcoinCash;

#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum CashAddrType {
    /// Pay to PubKey Hash
    /// https://bitcoin.org/en/glossary/p2pkh-address
    P2PKH,
    /// Pay to Script Hash
    /// https://bitcoin.org/en/glossary/p2sh-address
    P2SH,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum NetworkPrefix {
    BitcoinCash,
    BchTest,
    BchReg,
    // SLP on BCH mainnet
    SimpleLedger,
    // SLP on BCH testnet
    SlpTest,
    Other(String),
}

impl fmt::Display for NetworkPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let as_str = match self {
            NetworkPrefix::BitcoinCash => "bitcoincash",
            NetworkPrefix::BchTest => "bchtest",
            NetworkPrefix::BchReg => "bchreg",
            NetworkPrefix::SimpleLedger => "simpleledger",
            NetworkPrefix::SlpTest => "slptest",
            NetworkPrefix::Other(network) => network,
        };
        write!(f, "{as_str}")
    }
}

impl FromStr for NetworkPrefix {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_lowercase();
        let prefix = match s.as_str() {
            "bitcoincash" => NetworkPrefix::BitcoinCash,
            "bchtest" => NetworkPrefix::BchTest,
            "bchreg" => NetworkPrefix::BchReg,
            "simpleledger" => NetworkPrefix::SimpleLedger,
            "slptest" => NetworkPrefix::SlpTest,
            _ => NetworkPrefix::Other(s),
        };
        Ok(prefix)
    }
}

impl From<&'static str> for NetworkPrefix {
    fn from(s: &str) -> Self {
        s.parse().unwrap()
    }
}

impl NetworkPrefix {
    /// The method converts self to string and returns a byte array with each element
    /// being the corresponding character's right-most 5 bits.
    /// Result additionally includes a null termination byte.
    fn encode_to_checksum(&self) -> Vec<u8> {
        // Grab the right most 5 bits of each char.
        let mut prefix: Vec<u8> = self.to_string().as_bytes().iter().map(|x| x & 0b11111).collect();
        prefix.push(0);
        prefix
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CashAddress {
    pub prefix: NetworkPrefix,
    pub hash: Vec<u8>,
    pub address_type: CashAddrType,
}

impl CashAddress {
    pub fn decode(addr: &str) -> Result<CashAddress, String> {
        let (prefix, payload) = split_address(addr)?;

        if is_mixedcase(payload) {
            return Err("cashaddress contains mixed upper and lowercase characters".into());
        }

        let payload = base32::decode(payload)?;

        // Ensure the checksum is zero when decoding.
        let checksum = calculate_checksum(&prefix, &payload);
        if checksum != 0 {
            return Err("Checksum verification failed".into());
        }

        // The checksum sits in the last eight bytes.
        // We should have at least one more byte than the checksum.
        if payload.len() < 9 {
            return Err("Insufficient packed data to decode".into());
        }

        let payload_len = payload.len();
        // Get payload subslice without the checksum.
        let payload = &payload[..payload_len - 8];

        let (mut payload, _) = convert_bits(5, 8, payload, false);

        // Ensure there isn't extra non-zero padding.
        // Note: the condition fails on each address whose hash size is not 20 and not 28.
        // let extrabits = payload.len() * 5 % 8;
        // if extrabits >= 5 {
        //     return Err("Non-zero padding".into());
        // }

        // The version byte is the first byte. Pop it.
        let version = payload.remove(0);

        let address_type = addr_type_from_version(version)?;
        let hash_len = hash_size_from_version(version);

        if payload.len() != hash_len {
            return Err(format!(
                "Incorrect address hash len: expected={}, actual={}",
                hash_len,
                payload.len()
            ));
        }

        Ok(CashAddress {
            prefix,
            hash: payload,
            address_type,
        })
    }

    pub fn encode(&self) -> Result<String, String> {
        let mut payload = vec![self.version_byte()?];
        payload.extend(self.hash.iter());

        let (mut payload, _) = convert_bits(8, 5, &payload, true);
        let mut payload_with_phantom_checksum = payload.clone();

        // The checksum sits in the last eight bytes.
        // Append the phantom checksum to calculate an actual value.
        payload_with_phantom_checksum.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        let checksum = calculate_checksum(&self.prefix, &payload_with_phantom_checksum);

        // Append the actual checksum.
        payload.reserve(8);
        for i in 0..8 {
            let byte = ((checksum >> (5 * (7 - i))) & 0x1F) as u8;
            payload.push(byte);
        }

        let address = base32::encode(&payload)?;
        Ok(format!("{}:{}", self.prefix, address))
    }

    pub fn new(network_prefix: &str, hash: Vec<u8>, address_type: CashAddrType) -> Result<CashAddress, String> {
        match hash.len() {
            20 | 24 | 28 | 32 | 40 | 48 | 56 | 64 => (),
            _ => return Err(format!("Unexpected hash size {}", hash.len())),
        }

        let prefix = network_prefix.parse()?;
        Ok(CashAddress {
            prefix,
            hash,
            address_type,
        })
    }

    /// Get version byte from
    fn version_byte(&self) -> Result<u8, String> {
        let en_address_type: u8 = match self.address_type {
            CashAddrType::P2PKH => 0,
            CashAddrType::P2SH => 1,
        };

        let en_hash_size: u8 = match self.hash.len() {
            20 => 0,
            24 => 1,
            28 => 2,
            32 => 3,
            40 => 4,
            48 => 5,
            56 => 6,
            64 => 7,
            _ => return Err(format!("Unexpected hash size {}", self.hash.len())),
        };

        let mut version_byte = en_address_type;
        version_byte <<= 3;
        version_byte |= en_hash_size;

        Ok(version_byte)
    }
}

impl FromStr for CashAddress {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        CashAddress::decode(s)
    }
}

impl From<&'static str> for CashAddress {
    fn from(s: &'static str) -> Self {
        s.parse().unwrap()
    }
}

fn split_address(addr: &str) -> Result<(NetworkPrefix, &str), String> {
    let tokens: Vec<&str> = addr.split(':').collect();
    if tokens.len() == 1 {
        Ok((DEFAULT_PREFIX, tokens[0]))
    } else if tokens.len() == 2 {
        Ok((tokens[0].parse()?, tokens[1]))
    } else {
        Err("Invalid address: expect 'network:payload'".into())
    }
}

/// Get actual hash size from version byte.
/// The 3 least significant bits of version byte indicate the size of the hash.
/// See https://github.com/bitcoincashorg/bitcoincash.org/blob/master/spec/cashaddr.md#version-byte
fn hash_size_from_version(version: u8) -> usize {
    match version & 0b00000111 {
        0 => 20,
        1 => 24,
        2 => 28,
        3 => 32,
        4 => 40,
        5 => 48,
        6 => 56,
        7 => 64,
        _ => unreachable!(),
    }
}

/// Get address type from version byte.
/// The version byte's most significant bit is reserved and must be 0.
/// The 4 next bits indicate the type of address.
/// See https://github.com/bitcoincashorg/bitcoincash.org/blob/master/spec/cashaddr.md#version-byte
fn addr_type_from_version(version: u8) -> Result<CashAddrType, String> {
    if (version & 0b10000000) != 0 {
        return Err("The version byte's most significant bit is reserved and must be 0".into());
    }

    // shift
    match version >> 3 {
        0 => Ok(CashAddrType::P2PKH),
        1 => Ok(CashAddrType::P2SH),
        _ => Err("Unexpected address type".into()),
    }
}

// CalculateChecksum calculates a BCH checksum for a nibble-packed cashaddress
// that properly includes the network prefix.
fn calculate_checksum(prefix: &NetworkPrefix, payload: &[u8]) -> u64 {
    let mut raw_data = prefix.encode_to_checksum();
    raw_data.extend(payload);
    poly_mod(&raw_data)
}

/// The poly_mod is a BCH-encoding checksum function per the CashAddr specification.
/// See https://github.com/bitcoincashorg/bitcoincash.org/blob/master/spec/cashaddr.md#checksum
fn poly_mod(raw_data: &[u8]) -> u64 {
    let mut c = 1u64;
    for d in raw_data {
        let c0 = c >> 35;
        c = ((c & 0x07ffffffff) << 5) ^ (*d as u64);

        if c0 & 0x01 != 0 {
            c ^= 0x98f2bc8e61;
        }
        if c0 & 0x02 != 0 {
            c ^= 0x79b76d99e2;
        }
        if c0 & 0x04 != 0 {
            c ^= 0xf33e5fb3c4;
        }
        if c0 & 0x08 != 0 {
            c ^= 0xae2eabe2a8;
        }
        if c0 & 0x10 != 0 {
            c ^= 0x1e4f43e470;
        }
    }

    c ^ 1
}

/// ConvertBits takes a byte array as `input`, and converts it from `frombits`
/// bit representation to a `tobits` bit representation, while optionally
/// padding it.  ConvertBits returns the new representation and a bool
/// indicating that the output was not truncated.
fn convert_bits(frombits: u8, tobits: u8, input: &[u8], pad: bool) -> (Vec<u8>, bool) {
    assert!(0 < frombits && frombits <= 8 && 0 < tobits && tobits <= 8);

    let mut acc = 0u64;
    let mut bits = 0u64;
    let maxv: u64 = (1 << tobits) - 1;
    let max_acc: u64 = (1 << (frombits + tobits - 1)) - 1;

    let output_len = input.len() * frombits as usize / tobits as usize;
    let mut output = Vec::with_capacity(output_len);

    let frombits = frombits as u64;
    let tobits = tobits as u64;

    for d in input {
        acc = ((acc << frombits) | (*d as u64)) & max_acc;
        bits += frombits;
        while bits >= tobits {
            bits -= tobits;
            let v = (acc >> bits) & maxv;
            output.push(v as u8);
        }
    }

    // We have remaining bits to encode but do not pad.
    if !pad && bits > 0 {
        return (output, false);
    }

    // We have remaining bits to encode so we do pad.
    if pad && bits > 0 {
        let v = (acc << (tobits - bits)) & maxv;
        output.push(v as u8);
    }

    (output, true)
}

/// Check if the input string contains mixed upper and lowercase characters.
fn is_mixedcase(s: &str) -> bool {
    let first_char = match s.chars().next() {
        Some(c) => c,
        _ => return true,
    };

    let is_lowercase = first_char.is_lowercase();
    !s.chars().all(|c| c.is_numeric() || c.is_lowercase() == is_lowercase)
}

/// Bitcoin Cash base32 specific format.
mod base32 {
    /// Charset for converting from base32.
    const CHARSET_REV: [i8; 128] = [
        -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 15, -1, 10, 17, 21, 20, 26, 30,
        7, 5, -1, -1, -1, -1, -1, -1, -1, 29, -1, 24, 13, 25, 9, 8, 23, -1, 18, 22, 31, 27, 19, -1, 1, 0, 3, 16, 11,
        28, 12, 14, 6, 4, 2, -1, -1, -1, -1, -1, -1, 29, -1, 24, 13, 25, 9, 8, 23, -1, 18, 22, 31, 27, 19, -1, 1, 0, 3,
        16, 11, 28, 12, 14, 6, 4, 2, -1, -1, -1, -1, -1,
    ];

    /// Charset for converting to base32.
    const CHARSET: [char; 32] = [
        'q', 'p', 'z', 'r', 'y', '9', 'x', '8', 'g', 'f', '2', 't', 'v', 'd', 'w', '0', 's', '3', 'j', 'n', '5', '4',
        'k', 'h', 'c', 'e', '6', 'm', 'u', 'a', '7', 'l',
    ];

    /// `encode` converts an input byte array into a base32 string.
    /// It expects the byte array to be 5-bit packed.
    pub fn encode(input: &[u8]) -> Result<String, String> {
        let mut output = String::with_capacity(input.len());

        for i in input {
            let i = *i as usize;
            if i >= CHARSET.len() {
                return Err("Invalid byte in input array".into());
            }
            output.push(CHARSET[i]);
        }

        Ok(output)
    }

    /// `decode` takes a string in base32 format and returns a byte array that is
    /// 5-bit packed.
    pub fn decode(input: &str) -> Result<Vec<u8>, String> {
        let mut output = Vec::with_capacity(input.len());
        for c in input.chars() {
            let cpos = c as usize;
            if cpos >= CHARSET_REV.len() {
                return Err("Invalid base32 input string".into());
            }

            let val = CHARSET_REV[cpos];
            if val == -1 {
                return Err("Invalid base32 input string".into());
            }
            output.push(val as u8);
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_bits() {
        let (five, padded) = convert_bits(8, 5, &[0xFF], true);
        assert!(padded, "Should have been padded");
        assert_eq!(vec![0x1F, 0x1C], five);
        let (eight, padded) = convert_bits(5, 8, &five, false);
        assert!(!padded, "Should not have been padded");
        assert_eq!(eight, vec![0xFF]);
    }

    #[test]
    fn test_base32() {
        // the raw arrays are 5-bit packed - the condition is required by base32 encode and decode functions
        let raw = [
            vec![
                24, 14, 9, 25, 19, 30, 22, 1, 28, 0, 30, 28, 22, 7, 1, 11, 18, 7, 1, 7, 19, 23, 21, 30, 24, 25, 20, 27,
                3, 27, 29, 10,
            ],
            vec![
                8, 8, 15, 8, 28, 29, 16, 2, 1, 17, 25, 5, 17, 4, 2, 8, 17, 21, 15, 20, 11, 24, 29, 16, 6, 20, 11, 2,
                22, 18, 22, 5,
            ],
            vec![
                12, 2, 21, 24, 0, 0, 5, 14, 7, 6, 22, 25, 22, 31, 20, 9, 18, 12, 10, 6, 11, 28, 7, 14, 19, 9, 15, 29,
                15, 22, 11, 27,
            ],
        ];
        let encoded = [
            "cwfen7kpuq7uk8ptj8p8nh47ce5mrma2",
            "gg0guaszp3e93yzg3405tcasx5tzkjk9",
            "vz4cqq9w8xkekl5fjv2xtu8wnf0a0ktm",
        ];

        for i in 0..3 {
            let actual_encoded = base32::encode(&raw[i]).unwrap();
            let expected = encoded[i];
            assert_eq!(&actual_encoded, expected);
            let actual_raw = base32::decode(&actual_encoded).unwrap();
            let expected = &raw[i];
            assert_eq!(&actual_raw, expected);
        }
    }

    #[test]
    fn test_encode_decode() {
        let encoded = [
            "bitcoincash:pq4ql3ph6738xuv2cycduvkpu4rdwqge5q2uxdfg6f",
            "qrplwyx7kueqkrh6dmd3fclta6u32hafp5tnpkchx2",
            "BitCoinCash:QRPLWYX7KUEQKRH6DMD3FCLTA6U32HAFP5TNPKCHX2",
            "bchtest:qqjr7yu573z4faxw8ltgvjwpntwys08fysk07zmvce",
            "bchtest:pnq8zwpj8cq05n7pytfmskuk9r4gzzel8qtsvwz79zdskftrzxtar994cgutavfklvmgm6ynej",
        ];
        let expected_addresses = [
            CashAddress {
                prefix: "bitcoincash".into(),
                hash: vec![
                    42, 15, 196, 55, 215, 162, 115, 113, 138, 193, 48, 222, 50, 193, 229, 70, 215, 1, 25, 160,
                ],
                address_type: CashAddrType::P2SH,
            },
            CashAddress {
                prefix: "bitcoincash".into(),
                hash: vec![
                    195, 247, 16, 222, 183, 50, 11, 14, 250, 110, 219, 20, 227, 235, 238, 185, 21, 95, 169, 13,
                ],
                address_type: CashAddrType::P2PKH,
            },
            CashAddress {
                prefix: "bitcoincash".into(),
                hash: vec![
                    195, 247, 16, 222, 183, 50, 11, 14, 250, 110, 219, 20, 227, 235, 238, 185, 21, 95, 169, 13,
                ],
                address_type: CashAddrType::P2PKH,
            },
            CashAddress {
                prefix: "bchtest".into(),
                hash: vec![
                    36, 63, 19, 148, 244, 69, 84, 244, 206, 63, 214, 134, 73, 193, 154, 220, 72, 60, 233, 36,
                ],
                address_type: CashAddrType::P2PKH,
            },
            CashAddress {
                prefix: "bchtest".into(),
                hash: vec![
                    192, 113, 56, 50, 62, 0, 250, 79, 193, 34, 211, 184, 91, 150, 40, 234, 129, 11, 63, 56, 23, 6, 56,
                    94, 40, 155, 11, 37, 99, 17, 151, 209, 148, 181, 194, 56, 190, 177, 54, 251,
                ],
                address_type: CashAddrType::P2SH,
            },
        ];

        for i in 0..4 {
            let actual_address = CashAddress::decode(encoded[i]).unwrap();
            let expected_address = expected_addresses[i].clone();
            assert_eq!(actual_address, expected_address);
            let actual_encoded = actual_address.encode().unwrap();
            let expected_encoded = encoded[i].to_lowercase();

            // Do not check the equal, because the source encoded may have default network prefix.
            assert!(actual_encoded.contains(&expected_encoded));
        }
    }

    #[test]
    fn test_checksum() {
        let addresses = vec![
            "bchtest:p9adhakpwzztepkpwp5z0dq62m6u5v5xtyj7j3h2u94tsynr",
            "bitcoincash:qmvl5lzvdm6km38lgga64ek5jhdl7e3aqd9895wu04fvhlnare5937w4ywkq57juxsrhvw8ym5d8qx7sz7zz0zvcypqscw8jd03f",
            "bitcoincash:qpzry9x8gf2tvdw0s3jn54khce6mua7lcw20ayyn",
            "bchtest:testnetaddress4d6njnut",
            "bchreg:555555555555555555555555555555555555555555555udxmlmrz",
            "ergon:qq0fj6kgmpxet2za56lxv4zlkjhu6a6psq0jqd4tvs",
        ];

        for addr in addresses {
            let (prefix, payload) = split_address(addr).unwrap();
            let payload = base32::decode(payload).unwrap();
            assert_eq!(calculate_checksum(&prefix, &payload), 0);
        }

        let incorrect_addresses = vec![
            "bchtest:9adhakpwzztepkpwp5z0dq62m6u5v5xtyj7j3h2u94tsynr",
            "bitcoincash:qpzry9x8gf2tvdw0s3jn54khce6mua7lcw20ayyc",
            "bchtest:testnetaddress4d6njnu",
            "ergon:p9adhakpwzztepkpwp5z0dq62m6u5v5xtyj7j3h2u94tsynr",
        ];

        for addr in incorrect_addresses {
            let (prefix, payload) = split_address(addr).unwrap();
            let payload = base32::decode(payload).unwrap();
            assert_ne!(calculate_checksum(&prefix, &payload), 0);
        }
    }
}
