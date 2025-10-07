use hex::{FromHex, ToHex};
use primitives::hash::H160 as GlobalH160;
use primitives::hash::H256 as GlobalH256;
use primitives::hash::H264 as GlobalH264;
use serde;
use serde::de::Unexpected;
use serde::ser::SerializeSeq;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

macro_rules! impl_hash {
    ($name: ident, $other: ident, $size: expr) => {
        /// Hash serialization
        #[derive(Clone, Copy)]
        pub struct $name(pub [u8; $size]);

        impl $name {
            pub const fn const_default() -> $name {
                $name([0; $size])
            }

            pub fn serialize_to_byte_seq<S>(value: &Self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                let mut seq = serializer.serialize_seq(Some(value.0.len()))?;
                for byte in &value.0 {
                    seq.serialize_element(byte)?;
                }
                seq.end()
            }

            pub fn deserialize_from_bytes<'de, D>(deserializer: D) -> Result<$name, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct BytesVisitor;
                impl<'de> serde::de::Visitor<'de> for BytesVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                        write!(formatter, "a byte array or sequence of length {}", $size)
                    }

                    fn visit_bytes<E>(self, v: &[u8]) -> Result<$name, E>
                    where
                        E: serde::de::Error,
                    {
                        if v.len() != $size {
                            return Err(E::invalid_length(v.len(), &self));
                        }
                        let mut arr = [0u8; $size];
                        arr.copy_from_slice(v);
                        Ok($name(arr))
                    }

                    fn visit_seq<A>(self, mut seq: A) -> Result<$name, A::Error>
                    where
                        A: serde::de::SeqAccess<'de>,
                    {
                        let mut vec = Vec::with_capacity($size);
                        while let Some(elem) = seq.next_element()? {
                            vec.push(elem);
                        }
                        if vec.len() != $size {
                            return Err(serde::de::Error::invalid_length(vec.len(), &self));
                        }
                        let mut arr = [0u8; $size];
                        arr.copy_from_slice(&vec);
                        Ok($name(arr))
                    }
                }
                deserializer.deserialize_any(BytesVisitor)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                $name::const_default()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
                write!(f, "{:02x}", self)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
                write!(f, "{:02x}", self)
            }
        }

        impl<T> From<T> for $name
        where
            $other: From<T>,
        {
            fn from(o: T) -> Self {
                $name($other::from(o).take())
            }
        }

        impl FromStr for $name {
            type Err = <$other as FromStr>::Err;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let other = $other::from_str(s)?;
                Ok($name(other.take()))
            }
        }

        #[allow(clippy::from_over_into)]
        impl Into<$other> for $name {
            fn into(self) -> $other {
                $other::from(self.0)
            }
        }

        #[allow(clippy::from_over_into)]
        impl Into<Vec<u8>> for $name {
            fn into(self) -> Vec<u8> {
                self.0.to_vec()
            }
        }

        impl Eq for $name {}

        impl Ord for $name {
            fn cmp(&self, other: &Self) -> Ordering {
                let self_ref: &[u8] = &self.0;
                let other_ref: &[u8] = &other.0;
                self_ref.cmp(other_ref)
            }
        }

        impl PartialEq for $name {
            fn eq(&self, other: &Self) -> bool {
                let self_ref: &[u8] = &self.0;
                let other_ref: &[u8] = &other.0;
                self_ref == other_ref
            }
        }

        impl PartialOrd for $name {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        impl Hash for $name {
            fn hash<H>(&self, state: &mut H)
            where
                H: Hasher,
            {
                $other::from(self.0.clone()).hash(state)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                let mut hex = String::new();
                hex.push_str(&$other::from(self.0.clone()).to_hex::<String>());
                serializer.serialize_str(&hex)
            }
        }

        impl<'a> serde::Deserialize<'a> for $name {
            fn deserialize<D>(deserializer: D) -> Result<$name, D::Error>
            where
                D: serde::Deserializer<'a>,
            {
                struct HashVisitor;

                impl<'b> serde::de::Visitor<'b> for HashVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                        formatter.write_str("a hash string")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        if value.len() != $size * 2 {
                            return Err(E::invalid_value(Unexpected::Str(value), &self));
                        }

                        match value[..].from_hex::<Vec<u8>>() {
                            Ok(ref v) => {
                                let mut result = [0u8; $size];
                                result.copy_from_slice(v);
                                Ok($name($other::from(result).take()))
                            },
                            _ => Err(E::invalid_value(Unexpected::Str(value), &self)),
                        }
                    }

                    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        self.visit_str(value.as_ref())
                    }
                }

                deserializer.deserialize_identifier(HashVisitor)
            }
        }

        impl ::std::fmt::LowerHex for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
                for i in &self.0[..] {
                    write!(f, "{:02x}", i)?;
                }
                Ok(())
            }
        }
    };
}

impl_hash!(H264, GlobalH264, 33);
impl_hash!(H256, GlobalH256, 32);
impl_hash!(H160, GlobalH160, 20);

impl H256 {
    #[inline]
    pub fn reversed(&self) -> Self {
        let mut result = *self;
        result.0.reverse();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::H256;
    use primitives::hash::H256 as GlobalH256;
    use std::str::FromStr;

    #[test]
    fn hash_debug() {
        let str_reversed = "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048";
        let reversed_hash = H256::from(str_reversed);
        let debug_result = format!("{reversed_hash:?}");
        assert_eq!(debug_result, str_reversed);
    }

    #[test]
    fn hash_from_str() {
        let str_reversed = "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048";
        match H256::from_str(str_reversed) {
            Ok(reversed_hash) => assert_eq!(format!("{reversed_hash:?}"), str_reversed),
            _ => panic!("unexpected"),
        }

        let str_reversed = "XXXYYY";
        if H256::from_str(str_reversed).is_ok() {
            panic!("unexpected");
        }
    }

    #[test]
    fn hash_to_global_hash() {
        let str_reversed = "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048";
        let reversed_hash = H256::from(str_reversed);
        let global_hash = GlobalH256::from(str_reversed);
        let global_converted: GlobalH256 = reversed_hash.into();
        assert_eq!(global_converted, global_hash);
    }
}
