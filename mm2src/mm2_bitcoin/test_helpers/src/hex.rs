/// Strips the '0x' prefix off of hex string so it can be deserialized.
///
/// # Arguments
///
/// * `s` - The hex str
fn strip_0x_prefix(s: &str) -> &str {
    if &s[..2] == "0x" {
        &s[2..]
    } else {
        s
    }
}

/// Deserializes a hex string into a u8 array.
///
/// # Arguments
///
/// * `s` - The hex string
fn deserialize_hex(s: &str) -> Result<Vec<u8>, hex::FromHexError> {
    hex::decode(strip_0x_prefix(s))
}

/// Deserialize a hex string into bytes.
/// Panics if the string is malformatted.
///
/// # Arguments
///
/// * `s` - The hex string
///
/// # Panics
///
/// When the string is not validly formatted hex.
#[inline]
pub fn force_deserialize_hex(s: &str) -> Vec<u8> {
    deserialize_hex(s).unwrap()
}
