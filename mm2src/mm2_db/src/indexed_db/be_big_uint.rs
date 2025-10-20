//! This module contains the `BeBigUint` number representation
//! that is intended to be used as an index in `IndexedDb`.
//! `BeBigUint` solves the problem related to `BigInt` built in JavaScript
//! that can't be used as an index in [`IdbKeyRange::range`] due to `IndexedDb` implementation:
//!
//! ```js
//! // Chromium: `Failed to execute 'bound' on 'IDBKeyRange': The parameter is not a valid key.`
//! // Firefox: `DataError: Data provided to an operation does not meet requirements.`
//! range = IDBKeyRange.bound(['RICK', BigInt(1)], ['RICK', BigInt(2)]);
//! ```
//!
//! Please note `wasm-bindgen` converts `u64`, `i64`, `u128`, `i128` types to `BigInt`,
//! so we can't use them as indexes in [`IdbKeyRange::range`].
//!
//! As a solution we could use `BigIntStr`, but the comparison of stringified numbers
//! doesn't work properly in some cases like in the following:
//!
//! ```
//! assert(3000 < 200000);
//! assert("3000" < "200000"); // fail due to the first characters '3' < '2'
//! ```
//!
//! Another way is to serialize numbers as byte arrays or digits (`u32`) arrays
//! using [`num_bigint::BigUint::to_bytes_be`] or [`num_bigint::BigUint::to_u32_digits`].
//! But the problem is the same - comparison of such serialized numbers doesn't work properly.
//!
//! ```
//! // BigUint::from(3000).to_bytes_be() => [11, 184]
//! // BigUint::from(200000).to_bytes_be() => [3, 13, 64]
//! assert(3000 < 200000);
//! assert([11, 184] < [3, 13, 64]); // fail due to the first bytes `11 > 3`
//! ```
//!
//! `BeBigUint` solves the previous problem by adding the number of digits required to represent a big integer.
//! `BeBigUint` serializes into array of `u32` digits because `IndexedDb` is optimized to store and compare it.
//!
//! ```
//! // BeBigUint::from(u128::MAX / 2).to_be_digits() => [4, 4294967295, 4294967295, 4294967295, 2147483647],
//! // where `4` is the number of digits representing `u128::MAX / 2`.
//! // BeBigUint::from(u64::MAX / 2).to_be_digits() => [2, 4294967295, 2147483647],
//! // where `2` is the number of digits representing `u64::MAX / 2`.
//! assert(u64::MAX / 2 < u128::MAX / 2);
//! // The following check passes due to the first bytes `2 < 4`.
//! assert([2, 4294967295, 2147483647] < [4, 4294967295, 4294967295, 4294967295, 2147483647]);
//! ```

use common::ifrom_inner;
use derive_more::Display;
use mm2_number::BigUint;
use num_traits::ToPrimitive;
use serde::de::Error as DeError;
use serde::ser::Error as SerError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::ops::Add;

#[derive(Debug, Display)]
pub enum BigUintError {
    #[display(fmt = "Expected at least one digit")]
    NoDigits,
    #[display(fmt = "Unexpected number of digits: expected '{expected}', found '{found}'")]
    InvalidNumberOfDigits { expected: u32, found: usize },
    #[display(fmt = "The number is too large: {_0}")]
    NumberIsTooLarge(BigUint),
}

impl std::error::Error for BigUintError {}

/// The big number representation that is intended to be used as an index in `IndexedDb`
#[derive(Clone, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct BeBigUint(BigUint);

impl BeBigUint {
    /// Serializes the inner big number into array of `u32` digits.
    ///
    /// # Representation
    ///
    /// The big integer serializes into array of `u32` digits in big-endian order.
    /// The first digits is **always** the number of digits used to represent the big integer.
    pub fn to_be_digits(&self) -> Result<Vec<u32>, BigUintError> {
        // Get u32 digits in little-endian byte order.
        let mut digits = self.0.to_u32_digits();
        let bytes_len = digits.len();
        if bytes_len > (u32::MAX as usize) {
            return Err(BigUintError::NumberIsTooLarge(self.0.clone()));
        }

        // Reorder the digits from little-endian to big-endian byte order.
        digits.reverse();

        digits.insert(0, bytes_len as u32);
        Ok(digits)
    }

    /// Deserializes a big number from the array of `u32` digits.
    /// The array is expected to be in big-endian order.
    /// For more information see [`BeBigUint::to_be_digits`].
    pub fn from_be_digits(mut be_digits: Vec<u32>) -> Result<BeBigUint, BigUintError> {
        if be_digits.is_empty() {
            return Err(BigUintError::NoDigits);
        }
        // First digit is the expected number of digits that represent the big integer.
        let expected = be_digits[0];
        // An actual number of digits that are considered as a representation of the big integer.
        let found = be_digits.len() - 1;
        if found != expected as usize {
            return Err(BigUintError::InvalidNumberOfDigits { expected, found });
        }

        // Reorder the digits from big-endian to little-endian byte order.
        be_digits.reverse();
        // Remove the number of digits.
        be_digits.pop();
        Ok(BeBigUint(BigUint::new(be_digits)))
    }
}

impl fmt::Debug for BeBigUint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for BeBigUint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<BigUint> for BeBigUint {
    fn from(int: BigUint) -> Self {
        BeBigUint(int)
    }
}

impl From<BeBigUint> for BigUint {
    fn from(int: BeBigUint) -> Self {
        int.0
    }
}

ifrom_inner!(BeBigUint, BigUint, u8 u16 u32 u64 usize u128);

impl<T> Add<T> for BeBigUint
where
    BigUint: From<T>,
{
    type Output = BeBigUint;

    fn add(self, rhs: T) -> Self::Output {
        BeBigUint::from(self.0 + BigUint::from(rhs))
    }
}

impl ToPrimitive for BeBigUint {
    #[inline]
    fn to_i64(&self) -> Option<i64> {
        self.0.to_i64()
    }

    #[inline]
    fn to_i128(&self) -> Option<i128> {
        self.0.to_i128()
    }

    #[inline]
    fn to_u64(&self) -> Option<u64> {
        self.0.to_u64()
    }

    #[inline]
    fn to_u128(&self) -> Option<u128> {
        self.0.to_u128()
    }
}

impl Serialize for BeBigUint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_be_digits().map_err(S::Error::custom)?.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BeBigUint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let be_digits: Vec<u32> = Vec::deserialize(deserializer)?;
        BeBigUint::from_be_digits(be_digits).map_err(D::Error::custom)
    }
}

mod tests {
    use super::*;
    use serde_json::{self as json, json, Value as Json};
    use wasm_bindgen_test::*;
    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn test_be_big_uint_ser_de() {
        fn test_ser_de_impl<U>(initial_int: U, expected_ser: Json)
        where
            BeBigUint: From<U>,
        {
            let int = BeBigUint::from(initial_int);
            let int_ser = json::to_value(&int).unwrap();
            assert_eq!(int_ser, expected_ser);
            let int_de: BeBigUint = json::from_value(int_ser).unwrap();
            assert_eq!(int, int_de);
        }

        test_ser_de_impl(0u32, json!([0]));
        test_ser_de_impl(1u32, json!([1, 1]));
        test_ser_de_impl(2u32, json!([1, 2]));
        test_ser_de_impl(u8::MAX, json!([1, (u8::MAX as u32)]));
        test_ser_de_impl(u16::MAX, json!([1, (u16::MAX as u32)]));
        test_ser_de_impl((u32::MAX as u64) + 1, json!([2, 1, 0]));
        test_ser_de_impl(u64::MAX / 2, json!([2, u32::MAX / 2, u32::MAX]));
        test_ser_de_impl(u64::MAX / 2 + 1, json!([2, u32::MAX / 2 + 1, 0]));
        test_ser_de_impl(u128::MAX / 2, json!([4, u32::MAX / 2, u32::MAX, u32::MAX, u32::MAX]));
        test_ser_de_impl(u128::MAX / 2 + 1, json!([4, u32::MAX / 2 + 1, 0, 0, 0]));
        test_ser_de_impl(u128::MAX, json!([4, u32::MAX, u32::MAX, u32::MAX, u32::MAX]));
    }

    #[wasm_bindgen_test]
    fn test_be_big_uint_debug_display() {
        let num = BeBigUint::from(1023u32);
        assert_eq!("1023", format!("{num:?}"));
        assert_eq!("1023", num.to_string());
    }
}
