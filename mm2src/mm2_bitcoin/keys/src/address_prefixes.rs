use std::{convert::TryFrom, fmt};

/// Prefix for a legacy address (p2pkh or p2sh)
#[derive(Debug, Clone, Eq, Hash, PartialEq, Default)]
pub struct AddressPrefix {
    data: Vec<u8>,
}

impl TryFrom<&[u8]> for AddressPrefix {
    type Error = ();

    fn try_from(prefix: &[u8]) -> Result<Self, Self::Error> {
        if !prefix.is_empty() && prefix.len() <= 2 {
            Ok(Self { data: prefix.to_vec() })
        } else {
            Err(())
        }
    }
}

impl From<[u8; 1]> for AddressPrefix {
    fn from(prefix: [u8; 1]) -> Self {
        Self { data: prefix.to_vec() }
    }
}

impl From<[u8; 2]> for AddressPrefix {
    fn from(prefix: [u8; 2]) -> Self {
        Self { data: prefix.to_vec() }
    }
}

impl fmt::Display for AddressPrefix {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "[")?;
        for i in 0..self.data.len() {
            write!(f, "{}", self.data[i])?;
            if i < self.data.len() - 1 {
                write!(f, ", ")?;
            }
        }
        write!(f, "]")?;
        Ok(())
    }
}

impl AddressPrefix {
    /// Get as vec of u8
    pub fn to_vec(&self) -> Vec<u8> {
        self.data.to_vec()
    }

    /// Get if prefix size is 1, for use in cash_address
    pub fn get_size_1_prefix(&self) -> u8 {
        if self.data.len() == 1 {
            self.data[0]
        } else {
            0 // maybe assert should be here as it is not supposed to have other prefix size for cash_address
        }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// All prefixes for legacy address types supported for a coin, from coin config
#[derive(Debug, Clone, Default)]
pub struct NetworkAddressPrefixes {
    pub p2pkh: AddressPrefix,
    pub p2sh: AddressPrefix,
}

impl fmt::Display for NetworkAddressPrefixes {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{{")?;
        write!(f, "{}", self.p2pkh)?;
        write!(f, "{}", self.p2sh)?;

        write!(f, "}}")?;
        Ok(())
    }
}

/// Some prefixes used in tests
pub mod prefixes {
    use super::NetworkAddressPrefixes;
    use lazy_static::lazy_static;

    lazy_static! {
        pub static ref KMD_PREFIXES: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [60].into(),
            p2sh: [85].into(),
        };
        pub static ref BTC_PREFIXES: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [0].into(),
            p2sh: [5].into(),
        };
        pub static ref T_BTC_PREFIXES: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [111].into(),
            p2sh: [196].into(),
        };
        pub static ref BCH_PREFIXES: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [0].into(),
            p2sh: [5].into(),
        };
        pub static ref QRC20_PREFIXES: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [120].into(),
            p2sh: [50].into(),
        };
        pub static ref QTUM_PREFIXES: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [58].into(),
            p2sh: [50].into(),
        };
        pub static ref T_QTUM_PREFIXES: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [120].into(),
            p2sh: [110].into(),
        };
        pub static ref GRS_PREFIXES: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [36].into(),
            p2sh: [5].into(),
        };
        pub static ref SYS_PREFIXES: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [63].into(),
            p2sh: [5].into(),
        };
        pub static ref ZCASH_PREFIXES: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [28, 184].into(),
            p2sh: [28, 189].into(),
        };
        pub static ref T_ZCASH_PREFIXES: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [29, 37].into(),
            p2sh: [28, 186].into(),
        };
    }
}
