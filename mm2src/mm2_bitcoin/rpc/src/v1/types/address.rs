use keys::LegacyAddress;
use serde::de::{Unexpected, Visitor};
use serde::{Deserializer, Serialize, Serializer};
use std::fmt;

/// Standard serde serialize for LegacyAddress.
pub fn serialize<S>(address: &LegacyAddress, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    address.to_string().serialize(serializer)
}

/// Standard serde deserialize for LegacyAddress
/// Note: we cannot have the same feature for Address as it must have coin prefixes when deserialized
pub fn deserialize<'a, D>(deserializer: D) -> Result<LegacyAddress, D::Error>
where
    D: Deserializer<'a>,
{
    deserializer.deserialize_any(AddressVisitor)
}

#[derive(Default)]
pub struct AddressVisitor;

impl Visitor<'_> for AddressVisitor {
    type Value = LegacyAddress;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an address")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: ::serde::de::Error,
    {
        value
            .parse()
            .map_err(|_| E::invalid_value(Unexpected::Str(value), &self))
    }
}

pub mod vec {
    use super::AddressVisitor;
    use keys::LegacyAddress;
    use serde::de::Visitor;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(addresses: &[LegacyAddress], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        addresses
            .iter()
            .map(|address| address.to_string())
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub fn deserialize<'a, D>(deserializer: D) -> Result<Vec<LegacyAddress>, D::Error>
    where
        D: Deserializer<'a>,
    {
        <Vec<String> as Deserialize>::deserialize(deserializer)?
            .into_iter()
            .map(|value| AddressVisitor.visit_str(&value))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use keys::LegacyAddress;
    use serde_json;
    use v1::types;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct TestStruct {
        #[serde(with = "types::address")]
        address: LegacyAddress,
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct VecAddressTest {
        #[serde(with = "types::address::vec")]
        pub addresses: Vec<LegacyAddress>,
    }

    impl TestStruct {
        fn new(address: LegacyAddress) -> Self {
            TestStruct { address }
        }
    }

    #[test]
    fn address_serialize() {
        let test = TestStruct::new("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".into());
        assert_eq!(
            serde_json::to_string(&test).unwrap(),
            r#"{"address":"1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"}"#
        );
    }

    #[test]
    fn address_deserialize() {
        let test = TestStruct::new("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".into());
        assert_eq!(
            serde_json::from_str::<TestStruct>(r#"{"address":"1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"}"#).unwrap(),
            test
        );
    }

    #[test]
    fn address_serialize_komodo() {
        let test = TestStruct::new("R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW".into());
        assert_eq!(
            serde_json::to_string(&test).unwrap(),
            r#"{"address":"R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW"}"#
        );
    }

    #[test]
    fn address_deserialize_komodo() {
        let test = TestStruct::new("R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW".into());
        assert_eq!(
            serde_json::from_str::<TestStruct>(r#"{"address":"R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW"}"#).unwrap(),
            test
        );
    }

    #[test]
    fn address_vec_deserialize_from_value() {
        let value: serde_json::Value =
            serde_json::from_str(r#"{"addresses":["1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"]}"#).unwrap();
        let test = serde_json::from_value::<VecAddressTest>(value).unwrap();
        assert_eq!(
            serde_json::from_str::<VecAddressTest>(r#"{"addresses":["1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"]}"#).unwrap(),
            test
        );
    }
}
