//! Inspired by https://github.com/openethereum/parity-ethereum/blob/v2.7.2-stable/util/EIP-712/src/encode.rs

use crate::eip712::{CustomTypes, Eip712, PropertyType, EIP712_DOMAIN};
use ethabi::{encode, Token};
use indexmap::IndexSet;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use web3::signing::keccak256;
use web3::types::{Address, H256, U256};
use web3::{Error, Result};

type H256Bytes = Vec<u8>;

pub fn hash_typed_data<Domain, SignData>(data: Eip712<Domain, SignData>) -> Result<H256>
where
    Domain: Serialize,
    SignData: Serialize,
{
    let data_raw = Eip712Raw::try_from(data)?;
    hash_typed_data_raw(data_raw)
}

fn hash_typed_data_raw(data: Eip712Raw) -> Result<H256> {
    /// EIP-191 compliant.
    const PREFIX: &[u8; 2] = b"\x19\x01";

    let domain_hash = encode_data(
        &data.types,
        PropertyType::Custom(EIP712_DOMAIN.to_string()),
        &data.domain,
        None,
    )?;
    let data_hash = encode_data(
        &data.types,
        PropertyType::Custom(data.primary_type.clone()),
        &data.message,
        None,
    )?;

    let concat = [PREFIX.as_slice(), domain_hash.as_slice(), data_hash.as_slice()].concat();
    Ok(H256::from(keccak256(&concat)))
}

#[derive(Debug, Deserialize, Serialize)]
struct Eip712Raw {
    types: CustomTypes,
    domain: Json,
    #[serde(rename = "primaryType")]
    primary_type: String,
    message: Json,
}

impl<Domain, SignData> TryFrom<Eip712<Domain, SignData>> for Eip712Raw
where
    Domain: Serialize,
    SignData: Serialize,
{
    type Error = Error;

    fn try_from(value: Eip712<Domain, SignData>) -> std::result::Result<Self, Self::Error> {
        Ok(Eip712Raw {
            types: value.types,
            domain: serde_json::to_value(value.domain)?,
            primary_type: value.primary_type,
            message: serde_json::to_value(value.message)?,
        })
    }
}

fn encode_data(
    custom_types: &CustomTypes,
    data_type: PropertyType,
    data: &Json,
    field_name: Option<&str>,
) -> Result<Vec<u8>> {
    match data_type {
        PropertyType::Bool => encode_bool(data, field_name),
        PropertyType::String => encode_string(data, field_name),
        PropertyType::Uint256 => encode_u256(data, field_name),
        PropertyType::Address => encode_address(data, field_name),
        PropertyType::Bytes => encode_bytes(data, field_name),
        PropertyType::Bytes32 => encode_bytes32(data, field_name),
        PropertyType::Custom(custom) => encode_custom(custom_types, &custom, data, field_name),
    }
}

fn encode_custom(
    custom_types: &CustomTypes,
    data_ident: &str,
    data: &Json,
    field_name: Option<&str>,
) -> Result<H256Bytes> {
    let data_properties = custom_types
        .get(data_ident)
        .ok_or_else(|| decode_error(format!("Found an unknown '{data_ident}' type"), field_name))?;

    let type_hash = type_hash(data_ident, custom_types)?;
    let mut encoded_tokens = encode(&[Token::FixedBytes(type_hash)]);

    for field in data_properties.iter() {
        let field_value = &data[&field.name];
        let field_type = PropertyType::from_str(&field.property_type)?;
        let mut encoded = encode_data(custom_types, field_type, field_value, Some(&*field.name))?;
        encoded_tokens.append(&mut encoded);
    }

    Ok(keccak256(&encoded_tokens).as_ref().to_vec())
}

/// Encode dynamic `bytes` — keccak256 hash of the raw bytes (same treatment as `string`).
fn encode_bytes(value: &Json, field_name: Option<&str>) -> Result<Vec<u8>> {
    let bytes = decode_hex_bytes(value, field_name)?;
    let hash = keccak256(&bytes).to_vec();
    Ok(encode(&[Token::FixedBytes(hash)]))
}

/// Encode fixed-size `bytes32` — the raw 32-byte value is encoded directly, NOT hashed.
/// Per EIP-712: fixed-size bytesN values are zero-padded to 32 bytes and encoded as-is.
fn encode_bytes32(value: &Json, field_name: Option<&str>) -> Result<Vec<u8>> {
    let bytes = decode_hex_bytes_exact::<32>(value, "bytes32", field_name)?;
    Ok(encode(&[Token::FixedBytes(bytes.to_vec())]))
}

fn encode_string(value: &Json, field_name: Option<&str>) -> Result<Vec<u8>> {
    let string = value
        .as_str()
        .ok_or_else(|| expected_type_error("string", value, field_name))?;
    let hash = keccak256(string.as_ref()).to_vec();

    Ok(encode(&[Token::FixedBytes(hash)]))
}

fn encode_bool(value: &Json, field_name: Option<&str>) -> Result<Vec<u8>> {
    let bin = value
        .as_bool()
        .ok_or_else(|| expected_type_error("bool", value, field_name))?;
    Ok(encode(&[Token::Bool(bin)]))
}

fn encode_address(value: &Json, field_name: Option<&str>) -> Result<Vec<u8>> {
    let bytes = decode_hex_bytes_exact::<20>(value, "address", field_name)?;
    Ok(encode(&[Token::Address(Address::from(bytes))]))
}

fn encode_u256(value: &Json, field_name: Option<&str>) -> Result<Vec<u8>> {
    let string = value
        .as_str()
        .ok_or_else(|| expected_type_error("uint256", value, field_name))?;
    let hex_str = string
        .strip_prefix("0x")
        .or_else(|| string.strip_prefix("0X"))
        .ok_or_else(|| decode_error("Expected 0x-prefixed hex string", field_name))?;
    if hex_str.len() > 64 {
        return Err(decode_error("uint256 value exceeds 32 bytes", field_name));
    }
    let uint = U256::from_str(hex_str).map_err(|e| decode_error(e, field_name))?;
    Ok(encode(&[Token::Uint(uint)]))
}

fn encode_type(custom_types: &CustomTypes, data_type: &str) -> Result<String> {
    let deps = {
        let mut temp = build_dependencies(data_type, custom_types).ok_or_else(|| {
            let error = format!("'SignTypedDataV4Raw::types' doesn't contain '{data_type}'");
            decode_error(error, None)
        })?;
        temp.remove(data_type);
        let mut temp = temp.into_iter().collect::<Vec<_>>();
        temp.sort_unstable();
        temp.insert(0, data_type);
        temp
    };

    let encoded = deps
        .into_iter()
        .filter_map(|dep| {
            custom_types.get(dep).map(|field_types| {
                let types = field_types
                    .iter()
                    .map(|value| format!("{} {}", value.property_type, value.name))
                    .join(",");
                format!("{dep}({types})")
            })
        })
        .collect::<Vec<_>>()
        .concat();
    Ok(encoded)
}

fn type_hash(data_type: &str, custom_types: &CustomTypes) -> Result<H256Bytes> {
    Ok(keccak256(encode_type(custom_types, data_type)?.as_ref()).to_vec())
}

/// Given a type and the set of custom types.
/// Returns a `HashSet` of dependent types of the given type.
fn build_dependencies<'a>(data_type: &'a str, custom_types: &'a CustomTypes) -> Option<HashSet<&'a str>> {
    custom_types.get(data_type)?;

    let mut types_stack = IndexSet::new();
    types_stack.insert(data_type);
    let mut deps = HashSet::new();

    while let Some(item) = types_stack.pop() {
        if let Some(fields) = custom_types.get(item) {
            deps.insert(item);

            for field in fields.iter() {
                // check if this field is an array type
                let field_type = if let Some(index) = field.property_type.find('[') {
                    &field.property_type[..index]
                } else {
                    &field.property_type
                };
                // seen this type before? or not a custom type skip
                if !deps.contains(field_type) || custom_types.contains_key(field_type) {
                    types_stack.insert(field_type);
                }
            }
        }
    }

    Some(deps)
}

/// Decode a `0x`-prefixed hex string from a JSON value into raw bytes.
/// Accepts any even-length hex payload (including empty `"0x"`).
fn decode_hex_bytes(value: &Json, field_name: Option<&str>) -> Result<Vec<u8>> {
    let string = value
        .as_str()
        .ok_or_else(|| expected_type_error("hex bytes", value, field_name))?;
    let hex_payload = string
        .strip_prefix("0x")
        .or_else(|| string.strip_prefix("0X"))
        .ok_or_else(|| decode_error("Expected 0x-prefixed hex string", field_name))?;
    if hex_payload.len() % 2 != 0 {
        return Err(decode_error(
            format!(
                "Hex payload must have even length, found {} characters",
                hex_payload.len()
            ),
            field_name,
        ));
    }
    hex::decode(hex_payload).map_err(|e| decode_error(e, field_name))
}

/// Decode a `0x`-prefixed hex string and enforce exactly `N` decoded bytes.
fn decode_hex_bytes_exact<const N: usize>(value: &Json, type_name: &str, field_name: Option<&str>) -> Result<[u8; N]> {
    let bytes = decode_hex_bytes(value, field_name)?;
    bytes
        .try_into()
        .map_err(|_| decode_error(format!("{} must be exactly {} bytes", type_name, N), field_name))
}

fn expected_type_error(expected: &str, found: &Json, field_name: Option<&str>) -> Error {
    decode_error(format!("Expected '{expected}' type, found '{found}'"), field_name)
}

// TODO: Input validation errors currently reuse `web3::Error::Decoder`, which is
// intended for RPC response parsing failures. Introduce a dedicated `Eip712EncodeError`
// so callers can distinguish typed-data input errors from transport/deserialization errors.
fn decode_error<E: fmt::Display>(error: E, field_name: Option<&str>) -> Error {
    let error = match field_name {
        Some(field_name) => format!("EIP712 '{field_name}' deserialization error: {error}"),
        None => format!("EIP712 deserialization error: {error}"),
    };
    Error::Decoder(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_dependencies() {
        let custom_types = r#"{
			"EIP712Domain": [
				{ "name": "name", "type": "string" },
				{ "name": "version", "type": "string" },
				{ "name": "chainId", "type": "uint256" },
				{ "name": "verifyingContract", "type": "address" }
			],
			"Person": [
				{ "name": "name", "type": "string" },
				{ "name": "wallet", "type": "address" }
			],
			"Mail": [
				{ "name": "from", "type": "Person" },
				{ "name": "to", "type": "Person" },
				{ "name": "contents", "type": "string" }
			]
		}"#;

        let custom_types: CustomTypes = serde_json::from_str(custom_types).unwrap();

        let mail = "Mail";
        let person = "Person";

        let expected = {
            let mut temp = HashSet::new();
            temp.insert(mail);
            temp.insert(person);
            temp
        };
        assert_eq!(build_dependencies(mail, &custom_types), Some(expected));
    }

    #[test]
    fn test_encode_type() {
        let custom_types = r#"{
			"EIP712Domain": [
				{ "name": "name", "type": "string" },
				{ "name": "version", "type": "string" },
				{ "name": "chainId", "type": "uint256" },
				{ "name": "verifyingContract", "type": "address" }
			],
			"Person": [
				{ "name": "name", "type": "string" },
				{ "name": "wallet", "type": "address" }
			],
			"Mail": [
				{ "name": "from", "type": "Person" },
				{ "name": "to", "type": "Person" },
				{ "name": "contents", "type": "string" }
			]
		}"#;

        let custom_types: CustomTypes = serde_json::from_str(custom_types).expect("alas error!");
        assert_eq!(
            "Mail(Person from,Person to,string contents)Person(string name,address wallet)",
            encode_type(&custom_types, "Mail").expect("alas error!")
        )
    }

    #[test]
    fn test_encode_type_hash() {
        let custom_types = r#"{
			"EIP712Domain": [
				{ "name": "name", "type": "string" },
				{ "name": "version", "type": "string" },
				{ "name": "chainId", "type": "uint256" },
				{ "name": "verifyingContract", "type": "address" }
			],
			"Person": [
				{ "name": "name", "type": "string" },
				{ "name": "wallet", "type": "address" }
			],
			"Mail": [
				{ "name": "from", "type": "Person" },
				{ "name": "to", "type": "Person" },
				{ "name": "contents", "type": "string" }
			]
		}"#;

        let custom_types = serde_json::from_str::<CustomTypes>(custom_types).expect("alas error!");
        let hash = type_hash("Mail", &custom_types).expect("alas error!");
        let actual = hex::encode(hash);
        assert_eq!(
            actual,
            "a0cedeb2dc280ba39b857546d74f5549c3a1d7bdc2dd96bf881f76108e23dac2"
        );
    }

    #[test]
    fn test_hash_data() {
        const JSON: &str = r#"{
            "primaryType": "Mail",
            "domain": {
                "name": "Ether Mail",
                "version": "1",
                "chainId": "0x1",
                "verifyingContract": "0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC"
            },
            "message": {
                "from": {
                    "name": "Cow",
                    "wallet": "0xCD2a3d9F938E13CD947Ec05AbC7FE734Df8DD826"
                },
                "to": {
                    "name": "Bob",
                    "wallet": "0xbBbBBBBbbBBBbbbBbbBbbbbBBbBbbbbBbBbbBBbB"
                },
                "contents": "Hello, Bob!"
            },
            "types": {
                "EIP712Domain": [
                    { "name": "name", "type": "string" },
                    { "name": "version", "type": "string" },
                    { "name": "chainId", "type": "uint256" },
                    { "name": "verifyingContract", "type": "address" }
                ],
                "Person": [
                    { "name": "name", "type": "string" },
                    { "name": "wallet", "type": "address" }
                ],
                "Mail": [
                    { "name": "from", "type": "Person" },
                    { "name": "to", "type": "Person" },
                    { "name": "contents", "type": "string" }
                ]
            }
        }"#;

        let typed_data = serde_json::from_str::<Eip712Raw>(JSON).expect("alas error!");
        let hash = hash_typed_data_raw(typed_data).expect("alas error!");
        assert_eq!(
            format!("{hash:02x}"),
            "be609aee343fb3c4b28e1df9e632fca64fcfaede20f02e86244efddf30957bd2",
        );
    }

    #[test]
    fn test_encode_bytes32_direct_no_hash() {
        // Per EIP-712: bytes32 is encoded directly as its raw 32-byte value, NOT keccak256-hashed.
        let value = Json::String("0x0000000000000000000000000000000000000000000000000000000000000001".to_string());
        let encoded = encode_bytes32(&value, Some("testField")).unwrap();
        // The ABI encoding of a 32-byte value that is 0x...01 should be the value itself,
        // left-padded to 32 bytes (which it already is).
        let expected = encode(&[Token::FixedBytes(
            hex::decode("0000000000000000000000000000000000000000000000000000000000000001").unwrap(),
        )]);
        assert_eq!(encoded, expected);

        // Verify this is NOT the old buggy behavior (keccak256 of the bytes).
        let old_buggy = encode(&[Token::FixedBytes(
            keccak256(&hex::decode("0000000000000000000000000000000000000000000000000000000000000001").unwrap())
                .to_vec(),
        )]);
        assert_ne!(encoded, old_buggy, "bytes32 must NOT be keccak256-hashed");
    }

    #[test]
    fn test_encode_bytes32_rejects_wrong_length() {
        // Too short (16 bytes).
        let short = Json::String("0x00000000000000000000000000000001".to_string());
        assert!(encode_bytes32(&short, Some("salt")).is_err());

        // Too long (33 bytes).
        let long = Json::String(format!("0x{}", "ab".repeat(33)));
        assert!(encode_bytes32(&long, Some("salt")).is_err());
    }

    #[test]
    fn test_encode_bytes_dynamic_hashed() {
        // Per EIP-712: dynamic `bytes` is encoded as keccak256 of its contents.
        let value = Json::String("0x1234".to_string());
        let encoded = encode_bytes(&value, Some("data")).unwrap();
        let expected = encode(&[Token::FixedBytes(keccak256(&[0x12, 0x34]).to_vec())]);
        assert_eq!(encoded, expected);
    }

    #[test]
    fn test_encode_bytes_empty() {
        // Empty bytes "0x" is valid — keccak256 of empty input.
        let value = Json::String("0x".to_string());
        let encoded = encode_bytes(&value, Some("data")).unwrap();
        let expected = encode(&[Token::FixedBytes(keccak256(&[]).to_vec())]);
        assert_eq!(encoded, expected);
    }

    #[test]
    fn test_decode_hex_bytes_validation() {
        // Missing 0x prefix.
        let no_prefix = Json::String("1234".to_string());
        assert!(decode_hex_bytes(&no_prefix, Some("f")).is_err());

        // Odd-length hex.
        let odd = Json::String("0x123".to_string());
        assert!(decode_hex_bytes(&odd, Some("f")).is_err());

        // Invalid hex characters.
        let bad_chars = Json::String("0xZZZZ".to_string());
        assert!(decode_hex_bytes(&bad_chars, Some("f")).is_err());

        // Valid input.
        let valid = Json::String("0xabcd".to_string());
        assert_eq!(decode_hex_bytes(&valid, Some("f")).unwrap(), vec![0xab, 0xcd]);
    }

    #[test]
    fn test_encode_u256_hex_parsing() {
        // uint256 accepts odd-length hex like "0x1" (used by Ether Mail chainId).
        let one = Json::String("0x1".to_string());
        assert_eq!(encode_u256(&one, None).unwrap(), encode(&[Token::Uint(U256::from(1))]));

        // Verify hex parsing — "0x10" must parse as 16, not 10.
        let sixteen = Json::String("0x10".to_string());
        assert_eq!(
            encode_u256(&sixteen, None).unwrap(),
            encode(&[Token::Uint(U256::from(16))])
        );

        // Alpha hex digits work.
        let aa = Json::String("0xaa".to_string());
        assert_eq!(
            encode_u256(&aa, None).unwrap(),
            encode(&[Token::Uint(U256::from(0xaa))])
        );
    }

    #[test]
    fn test_property_type_bytes_roundtrip() {
        use crate::eip712::PropertyType;
        assert_eq!(PropertyType::Bytes.to_string(), "bytes");
        assert!(matches!(PropertyType::from_str("bytes").unwrap(), PropertyType::Bytes));
    }

    /// End-to-end hash_typed_data_raw test with bytes and bytes32 fields.
    /// Validates the full dispatch + struct-hash composition path.
    #[test]
    fn test_hash_typed_data_with_bytes_and_bytes32() {
        const JSON: &str = r#"{
            "primaryType": "Transfer",
            "domain": {
                "name": "TestDomain",
                "version": "1",
                "chainId": "0x1",
                "verifyingContract": "0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC"
            },
            "message": {
                "salt": "0x0000000000000000000000000000000000000000000000000000000000000001",
                "data": "0xdeadbeef"
            },
            "types": {
                "EIP712Domain": [
                    { "name": "name", "type": "string" },
                    { "name": "version", "type": "string" },
                    { "name": "chainId", "type": "uint256" },
                    { "name": "verifyingContract", "type": "address" }
                ],
                "Transfer": [
                    { "name": "salt", "type": "bytes32" },
                    { "name": "data", "type": "bytes" }
                ]
            }
        }"#;

        let typed_data = serde_json::from_str::<Eip712Raw>(JSON).unwrap();
        let hash = hash_typed_data_raw(typed_data).unwrap();
        // Expected value independently computed via Python + pycryptodome keccak256.
        assert_eq!(
            format!("{hash:02x}"),
            "0f421a4ad456c22cd0dd059375a17daa4d2f6e12ae0fae0b441313cf58207ae9",
        );
    }
}
