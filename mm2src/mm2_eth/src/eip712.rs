//! Inspired by https://github.com/openethereum/parity-ethereum/blob/v2.7.2-stable/util/EIP-712/src/eip712.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

pub const EIP712_DOMAIN: &str = "EIP712Domain";

pub type CustomTypes = HashMap<String, Vec<ObjectProperty>>;

/// `ObjectType` is used to describes an object type accordingly to:
/// https://github.com/ethereum/EIPs/blob/master/EIPS/eip-712.md#definition-of-typed-structured-data-%F0%9D%95%8A
///
/// # Example
///
/// Let's you need to describe the following types:
///
/// ```rust
/// struct Mail {
///   message: String,
///   from: Person,
///   to: Vec<Person>,
/// }
///
/// struct Person {
///   address: String,
/// }
/// ```
///
/// They can be described as follows:
///
/// ```rust
/// # use mm2_eth::eip712::{ObjectType, PropertyType};
///
/// let mut mail_type = ObjectType::new("Mail");
/// mail_type.property("message", PropertyType::String);
/// mail_type.property("from", PropertyType::Custom("Person".into()));
/// mail_type.property_array("to", PropertyType::Custom("Person".into()));
///
/// let mut person_type = ObjectType::new("Person");
/// person_type.property("address", PropertyType::Address);
///
/// let types = vec![mail_type, person_type];
/// ```
pub struct ObjectType {
    pub name: String,
    pub properties: Vec<ObjectProperty>,
}

impl ObjectType {
    /// Creates an `ObjectType` with the `EIP712Domain` name
    /// (required to be set for a domain typed structure).
    pub fn domain() -> ObjectType {
        ObjectType {
            name: EIP712_DOMAIN.to_string(),
            properties: Vec::new(),
        }
    }

    /// Creates an `ObjectType` with a custom `name`.
    pub fn new(name: &str) -> ObjectType {
        ObjectType {
            name: name.to_string(),
            properties: Vec::new(),
        }
    }

    /// Describes a property.
    pub fn property(&mut self, property_name: &str, property_type: PropertyType) -> &mut ObjectType {
        let property = ObjectProperty {
            name: property_name.to_string(),
            property_type: property_type.to_string(),
        };
        self.properties.push(property);
        self
    }
}

/// Add `Int64`, `Uint64`, `Int256`, `Array` types if required.
/// https://github.com/ethereum/EIPs/blob/master/EIPS/eip-712.md#definition-of-typed-structured-data-%F0%9D%95%8A
#[derive(Clone, Debug)]
pub enum PropertyType {
    Bool,
    String,
    Uint256,
    Address,
    Bytes,
    Bytes32,
    Custom(String),
}

impl fmt::Display for PropertyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropertyType::Bool => write!(f, "bool"),
            PropertyType::String => write!(f, "string"),
            PropertyType::Uint256 => write!(f, "uint256"),
            PropertyType::Address => write!(f, "address"),
            PropertyType::Bytes => write!(f, "bytes"),
            PropertyType::Bytes32 => write!(f, "bytes32"),
            PropertyType::Custom(custom) => write!(f, "{custom}"),
        }
    }
}

impl FromStr for PropertyType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let property_type = match s {
            "bool" => PropertyType::Bool,
            "string" => PropertyType::String,
            "uint256" => PropertyType::Uint256,
            "address" => PropertyType::Address,
            "bytes" => PropertyType::Bytes,
            "bytes32" => PropertyType::Bytes32,
            custom => PropertyType::Custom(custom.to_string()),
        };
        Ok(property_type)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ObjectProperty {
    pub(crate) name: String,
    #[serde(rename = "type")]
    pub(crate) property_type: String,
}

#[derive(Debug, Serialize)]
pub struct Eip712<Domain, SignData> {
    /// Defines the types of the domain and data you will be signing.
    pub types: CustomTypes,
    /// Ensures that the signature will be unique across multiple DApps and across Blockchains.
    pub domain: Domain,
    /// Name of the `sign_data` structured type.
    #[serde(rename = "primaryType")]
    pub primary_type: String,
    /// The message signing data content.
    pub message: SignData,
}
