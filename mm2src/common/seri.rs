use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Deserializes an empty string into `None`.  
/// Does not try to trim the string, passing `" "` will produce `Some (String::from (" "))`.  
/// Use with `#[serde(default, deserialize_with = "de_none_if_empty")]`.
pub fn de_none_if_empty<'de, D: Deserializer<'de>>(des: D) -> Result<Option<String>, D::Error> {
    struct Visitor;
    impl de::Visitor<'_> for Visitor {
        type Value = Option<String>;
        fn expecting(&self, fm: &mut fmt::Formatter) -> fmt::Result {
            fm.write_str("Optional string")
        }

        fn visit_none<E>(self) -> Result<Option<String>, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Option<String>, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_str<E>(self, sv: &str) -> Result<Option<String>, E>
        where
            E: de::Error,
        {
            if sv.is_empty() {
                Ok(None)
            } else {
                Ok(Some(sv.into()))
            }
        }
    }
    des.deserialize_any(Visitor)
}

pub fn deserialize_base64<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let base64 = String::deserialize(d)?;
    BASE64_ENGINE.decode(base64).map_err(serde::de::Error::custom)
}

pub fn serialize_base64<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
    let base64 = BASE64_ENGINE.encode(v);
    String::serialize(&base64, s)
}
