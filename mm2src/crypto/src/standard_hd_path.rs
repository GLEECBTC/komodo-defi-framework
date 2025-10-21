use crate::bip32_child::{Bip32Child, Bip32ChildValue, Bip32DerPathError, Bip44Tail, HardenedValue, NonHardenedValue};
use bip32::ChildNumber;
use derive_more::Display;
use enum_primitive_derive::Primitive;
use hw_common::primitives::Bip32Error;
use num_traits::FromPrimitive;
use std::convert::TryFrom;

/// Standard HD Path for [BIP-44](https://github.com/bitcoin/bips/blob/master/bip-0044.mediawiki),
/// [BIP-49](https://github.com/bitcoin/bips/blob/master/bip-0049.mediawiki),
/// [BIP-84](https://github.com/bitcoin/bips/blob/master/bip-0084.mediawiki)
/// and similar.
/// For path as `m/purpose'/coin_type'/account'/change/address_index`.
#[rustfmt::skip]
pub type StandardHDPath =
    Bip32Child<Bip32PurposeValue, // `purpose`
    Bip32Child<HardenedValue, // `coin_type`
    Bip32Child<HardenedValue, // `account_id`
    Bip32Child<Bip44ChainValue, // `chain`
    Bip32Child<NonHardenedValue, // `address_id`
    Bip44Tail>>>>>;
#[rustfmt::skip]
pub type HDPathToCoin =
    Bip32Child<Bip32PurposeValue, // `purpose`
    Bip32Child<HardenedValue, // `coin_type`
    Bip44Tail>>;
#[rustfmt::skip]
pub type HDPathToAccount =
    Bip32Child<Bip32PurposeValue, // `purpose`
    Bip32Child<HardenedValue, // `coin_type`
    Bip32Child<HardenedValue, // `account_id`
    Bip44Tail>>>;

impl StandardHDPath {
    pub fn purpose(&self) -> Bip43Purpose {
        self.value()
    }

    pub fn coin_type(&self) -> u32 {
        self.child().value()
    }

    pub fn account_id(&self) -> u32 {
        self.child().child().value()
    }

    pub fn chain(&self) -> Bip44Chain {
        self.child().child().child().value()
    }

    pub fn address_id(&self) -> u32 {
        self.child().child().child().child().value()
    }

    /// Derive `HDPathToCoin` from `StandardHDPath`
    pub fn path_to_coin(&self) -> HDPathToCoin {
        let Bip32Child {
            value: purpose,
            child: rest,
        } = self;
        let Bip32Child { value: coin_type, .. } = rest;

        Bip32Child {
            value: purpose.clone(),
            child: Bip32Child {
                value: coin_type.clone(),
                child: Bip44Tail,
            },
        }
    }
}

impl HDPathToCoin {
    pub fn purpose(&self) -> Bip43Purpose {
        self.value()
    }

    pub fn coin_type(&self) -> u32 {
        self.child().value()
    }
}

impl HDPathToAccount {
    pub fn purpose(&self) -> Bip43Purpose {
        self.value()
    }

    pub fn coin_type(&self) -> u32 {
        self.child().value()
    }

    pub fn account_id(&self) -> u32 {
        self.child().child().value()
    }
}

#[derive(Debug)]
pub struct UnknownChainError {
    pub chain: u32,
}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum StandardHDPathError {
    #[display(fmt = "Invalid derivation path length '{found}', expected '{expected}'")]
    InvalidDerivationPathLength { expected: usize, found: usize },
    #[display(fmt = "Child '{child}' is expected to be hardened")]
    ChildIsNotHardened { child: String },
    #[display(fmt = "Child '{child}' is expected not to be hardened")]
    ChildIsHardened { child: String },
    #[display(fmt = "Unexpected '{child}' child value '{value}', expected: {expected}")]
    UnexpectedChildValue {
        child: String,
        value: u32,
        expected: String,
    },
    #[display(fmt = "Unknown BIP32 error: {_0}")]
    Bip32Error(Bip32Error),
    #[display(fmt = "Invalid coin type '{found}', expected '{expected}'")]
    InvalidCoinType { expected: u32, found: u32 },
    #[display(fmt = "Invalid path to coin '{found}', expected '{expected}'")]
    InvalidPathToCoin { expected: String, found: String },
}

impl From<Bip32DerPathError> for StandardHDPathError {
    fn from(e: Bip32DerPathError) -> Self {
        fn display_child_at(child_at: usize) -> String {
            StandardHDIndex::from_usize(child_at)
                .map(|index| format!("{index:?}"))
                .unwrap_or_else(|| "UNKNOWN".to_owned())
        }

        match e {
            Bip32DerPathError::InvalidDerivationPathLength { expected, found } => {
                StandardHDPathError::InvalidDerivationPathLength { expected, found }
            },
            Bip32DerPathError::ChildIsNotHardened { child_at } => StandardHDPathError::ChildIsNotHardened {
                child: display_child_at(child_at),
            },
            Bip32DerPathError::ChildIsHardened { child_at } => StandardHDPathError::ChildIsHardened {
                child: display_child_at(child_at),
            },
            Bip32DerPathError::UnexpectedChildValue {
                child_at,
                actual,
                expected,
            } => StandardHDPathError::UnexpectedChildValue {
                child: display_child_at(child_at),
                value: actual,
                expected,
            },
            Bip32DerPathError::Bip32Error(bip32) => StandardHDPathError::Bip32Error(bip32),
        }
    }
}

impl From<UnknownChainError> for Bip32DerPathError {
    fn from(e: UnknownChainError) -> Self {
        Bip32DerPathError::UnexpectedChildValue {
            child_at: StandardHDIndex::Chain as usize,
            actual: e.chain,
            expected: "0 or 1 chain".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Primitive)]
pub enum StandardHDIndex {
    Purpose = 0,
    CoinType = 1,
    AccountId = 2,
    Chain = 3,
    AddressId = 4,
}

#[derive(Debug, Copy, Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[repr(u32)]
pub enum Bip44Chain {
    External = 0,
    Internal = 1,
}

impl TryFrom<u32> for Bip44Chain {
    type Error = UnknownChainError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Bip44Chain::External),
            1 => Ok(Bip44Chain::Internal),
            chain => Err(UnknownChainError { chain }),
        }
    }
}

impl Bip44Chain {
    pub fn to_child_number(&self) -> ChildNumber {
        ChildNumber::from(*self as u32)
    }
}

#[derive(Clone, PartialEq)]
pub struct Bip44ChainValue {
    chain: Bip44Chain,
}

impl Bip32ChildValue for Bip44ChainValue {
    type Value = Bip44Chain;

    /// `chain` is a non-hardened child as it's described in the BIP44 standard.
    fn hardened() -> bool {
        false
    }

    fn number(&self) -> u32 {
        self.chain as u32
    }

    fn value(&self) -> Self::Value {
        self.chain
    }

    fn from_bip32_number(child_number: ChildNumber, child_at: usize) -> Result<Self, Bip32DerPathError> {
        if child_number.is_hardened() {
            return Err(Bip32DerPathError::ChildIsHardened { child_at });
        }
        Ok(Bip44ChainValue {
            chain: Bip44Chain::try_from(child_number.index())?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u32)]
pub enum Bip43Purpose {
    Bip32 = 32,
    Bip44 = 44,
    Bip49 = 49,
    Bip84 = 84,
}

#[derive(Clone, PartialEq)]
pub struct Bip32PurposeValue {
    purpose: Bip43Purpose,
}

impl Bip32ChildValue for Bip32PurposeValue {
    type Value = Bip43Purpose;

    /// `purpose` is always a hardened child as it's described in the BIP44/BIP49/BIP84 standards.
    fn hardened() -> bool {
        true
    }

    fn number(&self) -> u32 {
        self.purpose as u32
    }

    fn value(&self) -> Bip43Purpose {
        self.purpose
    }

    fn from_bip32_number(child_number: ChildNumber, child_at: usize) -> Result<Self, Bip32DerPathError> {
        if !child_number.is_hardened() {
            return Err(Bip32DerPathError::ChildIsNotHardened { child_at });
        }

        let purpose = match child_number.index() {
            32 => Bip43Purpose::Bip32,
            44 => Bip43Purpose::Bip44,
            49 => Bip43Purpose::Bip49,
            84 => Bip43Purpose::Bip84,
            _chain => {
                return Err(Bip32DerPathError::UnexpectedChildValue {
                    child_at,
                    actual: child_number.0,
                    expected: "one of the following: 32, 44, 49, 84".to_string(),
                })
            },
        };

        Ok(Bip32PurposeValue { purpose })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bip32_child::Bip32DerPathOps;
    use bip32::DerivationPath;
    use std::str::FromStr;

    #[test]
    fn test_from_str() {
        let der_path = StandardHDPath::from_str("m/44'/141'/1'/0/10").unwrap();
        assert_eq!(der_path.coin_type(), 141);
        assert_eq!(der_path.account_id(), 1);
        assert_eq!(der_path.chain(), Bip44Chain::External);
        assert_eq!(der_path.address_id(), 10);
    }

    #[test]
    fn test_display() {
        let der_path = HDPathToAccount::from_str("m/44'/141'/1'").unwrap();
        let actual = format!("{der_path}");
        assert_eq!(actual, "m/44'/141'/1'");
    }

    #[test]
    fn test_derive() {
        let der_path_to_coin = HDPathToCoin::from_str("m/44'/141'").unwrap();
        let der_path_to_account: HDPathToAccount =
            der_path_to_coin.derive(ChildNumber::new(10, true).unwrap()).unwrap();
        assert_eq!(
            der_path_to_account.to_derivation_path(),
            DerivationPath::from_str("m/44'/141'/10'").unwrap()
        );
    }

    #[test]
    fn test_from_invalid_length() {
        let error = StandardHDPath::from_str("m/44'/141'/0'").expect_err("derivation path is too short");
        assert_eq!(
            error,
            Bip32DerPathError::InvalidDerivationPathLength { expected: 5, found: 3 }
        );

        let error =
            StandardHDPath::from_str("m/44'/141'/0'/1/2/3").expect_err("max number of children is 5, but 6 passes");
        assert_eq!(
            error,
            Bip32DerPathError::InvalidDerivationPathLength { expected: 5, found: 6 }
        );
    }

    #[test]
    fn test_from_unexpected_child_value() {
        let error = HDPathToAccount::from_str("m/44'/141'/0").expect_err("'account_id' is not hardened");
        assert_eq!(error, Bip32DerPathError::ChildIsNotHardened { child_at: 2 });
        let error = StandardHDPathError::from(error);
        assert_eq!(
            error,
            StandardHDPathError::ChildIsNotHardened {
                child: "AccountId".to_owned()
            }
        );
    }

    #[test]
    fn test_purposes() {
        let path = StandardHDPath::from_str("m/32'/141'/0'/0/0").unwrap();
        assert_eq!(path.purpose(), Bip43Purpose::Bip32);

        let path = StandardHDPath::from_str("m/44'/141'/0'/0/0").unwrap();
        assert_eq!(path.purpose(), Bip43Purpose::Bip44);

        let path = StandardHDPath::from_str("m/49'/141'/0'/0/0").unwrap();
        assert_eq!(path.purpose(), Bip43Purpose::Bip49);

        let path = StandardHDPath::from_str("m/84'/141'/0'/0/0").unwrap();
        assert_eq!(path.purpose(), Bip43Purpose::Bip84);
    }
}
