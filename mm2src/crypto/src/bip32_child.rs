use crate::RpcDerivationPath;
use hw_common::primitives::{Bip32Error, ChildNumber, DerivationPath};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::convert::TryFrom;
use std::fmt;
use std::str::FromStr;

pub type HardenedValue = AnyValue<true>;
pub type NonHardenedValue = AnyValue<false>;

#[derive(Debug, Eq, PartialEq)]
pub enum Bip32DerPathError {
    InvalidDerivationPathLength {
        expected: usize,
        found: usize,
    },
    ChildIsNotHardened {
        child_at: usize,
    },
    ChildIsHardened {
        child_at: usize,
    },
    UnexpectedChildValue {
        child_at: usize,
        actual: u32,
        expected: String,
    },
    Bip32Error(Bip32Error),
}

impl From<Bip32Error> for Bip32DerPathError {
    fn from(e: Bip32Error) -> Self {
        Bip32DerPathError::Bip32Error(e)
    }
}

pub trait Bip32DerPathOps: Sized {
    fn to_derivation_path(&self) -> DerivationPath {
        DerivationPath::default()
    }

    fn derive<T>(&self, bip32_number: ChildNumber) -> Result<T, Bip32DerPathError>
    where
        T: TryFrom<DerivationPath, Error = Bip32DerPathError>,
    {
        let mut derivation_path = self.to_derivation_path();
        derivation_path.push(bip32_number);
        T::try_from(derivation_path)
    }
}

pub trait Bip32InternalOps: Sized {
    /// This method is used to get the number of expected children.
    fn depth() -> usize {
        0
    }

    fn from_iter<I>(iter: I, current_depth: usize) -> Result<Self, Bip32DerPathError>
    where
        I: Iterator<Item = ChildNumber>;

    fn fill_derivation_path(&self, _derivation_path: &mut DerivationPath) {}

    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

pub trait Bip32ChildValue: Sized {
    type Value;

    fn hardened() -> bool;

    fn number(&self) -> u32;

    fn value(&self) -> Self::Value;

    fn bip32_number(&self) -> ChildNumber {
        ChildNumber::new(self.number(), Self::hardened()).expect("'ChildValue::number' is expected to be a valid index")
    }

    fn from_bip32_number(child_number: ChildNumber, child_index: usize) -> Result<Self, Bip32DerPathError>;
}

#[derive(Clone, PartialEq)]
pub struct AnyValue<const HARDENED: bool> {
    number: u32,
}

impl<const HARDENED: bool> Bip32ChildValue for AnyValue<HARDENED> {
    type Value = u32;

    fn hardened() -> bool {
        HARDENED
    }

    fn number(&self) -> u32 {
        self.number
    }

    fn value(&self) -> Self::Value {
        self.number
    }

    fn from_bip32_number(child_number: ChildNumber, child_at: usize) -> Result<Self, Bip32DerPathError> {
        if child_number.is_hardened() == HARDENED {
            return Ok(AnyValue {
                number: child_number.index(),
            });
        }
        if HARDENED {
            Err(Bip32DerPathError::ChildIsNotHardened { child_at })
        } else {
            Err(Bip32DerPathError::ChildIsHardened { child_at })
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Bip32Child<Value, Child> {
    pub(crate) value: Value,
    pub(crate) child: Child,
}

impl<Value: Bip32ChildValue, Child: Bip32InternalOps> fmt::Debug for Bip32Child<Value, Child> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl<Value: Bip32ChildValue, Child: Bip32InternalOps> fmt::Display for Bip32Child<Value, Child> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "m")?;
        Bip32InternalOps::fmt(self, f)
    }
}

impl<Value, Child> TryFrom<DerivationPath> for Bip32Child<Value, Child>
where
    Child: Bip32InternalOps,
    Value: Bip32ChildValue,
{
    type Error = Bip32DerPathError;

    fn try_from(value: DerivationPath) -> Result<Self, Self::Error> {
        const INITIAL_DEPTH: usize = 0;
        Self::from_iter(value.iter(), INITIAL_DEPTH)
    }
}

impl<'de, Value, Child> Deserialize<'de> for Bip32Child<Value, Child>
where
    Child: Bip32InternalOps,
    Value: Bip32ChildValue,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let derivation_path: RpcDerivationPath = Deserialize::deserialize(deserializer)?;
        Self::try_from(derivation_path.0).map_err(|e| serde::de::Error::custom(format!("{e:?}")))
    }
}

impl<Value, Child> Serialize for Bip32Child<Value, Child>
where
    Child: Bip32DerPathOps + Bip32InternalOps,
    Value: Bip32ChildValue,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let derivation_path = RpcDerivationPath(self.to_derivation_path());
        derivation_path.serialize(serializer)
    }
}

impl<Value, Child> FromStr for Bip32Child<Value, Child>
where
    Child: Bip32InternalOps,
    Value: Bip32ChildValue,
{
    type Err = Bip32DerPathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let derivation_path = DerivationPath::from_str(s)?;
        Bip32Child::try_from(derivation_path)
    }
}

impl<Value, Child> Bip32DerPathOps for Bip32Child<Value, Child>
where
    Child: Bip32DerPathOps + Bip32InternalOps,
    Value: Bip32ChildValue,
{
    fn to_derivation_path(&self) -> DerivationPath {
        let mut derivation_path = DerivationPath::default();
        self.fill_derivation_path(&mut derivation_path);
        derivation_path
    }
}

impl<Value, Child> Bip32InternalOps for Bip32Child<Value, Child>
where
    Child: Bip32InternalOps,
    Value: Bip32ChildValue,
{
    fn depth() -> usize {
        Child::depth() + 1
    }

    fn from_iter<I>(mut iter: I, current_depth: usize) -> Result<Self, Bip32DerPathError>
    where
        I: Iterator<Item = ChildNumber>,
    {
        let child_number = match iter.next() {
            Some(ch) => ch,
            None => {
                let found = current_depth;
                let expected = Self::depth() + current_depth;
                return Err(Bip32DerPathError::InvalidDerivationPathLength { expected, found });
            },
        };
        let value = Value::from_bip32_number(child_number, current_depth)?;
        let child = Child::from_iter(iter, current_depth + 1)?;
        Ok(Bip32Child { value, child })
    }

    fn fill_derivation_path(&self, derivation_path: &mut DerivationPath) {
        derivation_path.push(self.value.bip32_number());
        self.child.fill_derivation_path(derivation_path)
    }

    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.value.bip32_number())?;
        Bip32InternalOps::fmt(&self.child, f)
    }
}

impl<Value: Bip32ChildValue, Child> Bip32Child<Value, Child> {
    pub fn value(&self) -> Value::Value {
        self.value.value()
    }

    pub fn child(&self) -> &Child {
        &self.child
    }
}

#[derive(Clone, PartialEq)]
pub struct Bip44Tail;

impl Bip32DerPathOps for Bip44Tail {}

impl Bip32InternalOps for Bip44Tail {
    fn from_iter<I>(iter: I, current_depth: usize) -> Result<Self, Bip32DerPathError>
    where
        I: Iterator<Item = ChildNumber>,
    {
        let left = iter.count();
        if left == 0 {
            return Ok(Bip44Tail);
        }
        Err(Bip32DerPathError::InvalidDerivationPathLength {
            expected: current_depth,
            found: current_depth + left,
        })
    }
}
