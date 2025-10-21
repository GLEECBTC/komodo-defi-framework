use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// A wrapper struct representing a boolean value as an integer (0 or 1).
#[derive(Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct BoolAsInt(bool);

impl BoolAsInt {
    /// Creates a new `BoolAsInt` instance from a boolean value.
    pub fn new(value: bool) -> Self {
        BoolAsInt(value)
    }

    /// Retrieves the inner boolean value.
    pub fn as_bool(&self) -> bool {
        self.0
    }
}

impl From<bool> for BoolAsInt {
    fn from(value: bool) -> Self {
        BoolAsInt(value)
    }
}

impl Serialize for BoolAsInt {
    /// Serializes the `BoolAsInt` into a single-byte integer (0 or 1).
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(self.0 as u8)
    }
}

impl<'de> Deserialize<'de> for BoolAsInt {
    /// Deserializes a single-byte integer (0 or 1) into a `BoolAsInt`.
    ///
    /// # Errors
    ///
    /// Returns an error if the value is not 0 or 1.
    fn deserialize<D>(deserializer: D) -> Result<BoolAsInt, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u8::deserialize(deserializer)?;

        match value {
            0 => Ok(BoolAsInt(false)),
            1 => Ok(BoolAsInt(true)),
            _ => Err(de::Error::custom("Value must be 0 or 1")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests the serialization of `BoolAsInt`.
    #[test]
    fn test_serialization() {
        let bool_value = BoolAsInt::new(true);
        let serialized = serde_json::to_string(&bool_value).unwrap();
        assert_eq!(serialized, "1");
    }

    /// Tests the deserialization of `BoolAsInt` for a value of 0.
    #[test]
    fn test_deserialization() {
        let deserialized: BoolAsInt = serde_json::from_str("0").unwrap();
        assert_eq!(deserialized, BoolAsInt::new(false));
    }

    /// Tests the round trip of serialization and deserialization for `BoolAsInt`.
    #[test]
    fn test_round_trip() {
        let bool_value = BoolAsInt::new(true);
        let serialized = serde_json::to_string(&bool_value).unwrap();
        let deserialized: BoolAsInt = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, bool_value);
    }

    /// Tests the deserialization error case when the value is not 0 or 1.
    #[test]
    fn test_deserialization_error() {
        let result: Result<BoolAsInt, _> = serde_json::from_str("2");
        assert!(result.is_err());
    }

    /// Tests the `as_bool` method to retrieve the inner boolean value.
    #[test]
    fn test_as_bool() {
        let bool_value = BoolAsInt::new(true);
        assert!(bool_value.as_bool());
    }
}
