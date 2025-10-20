use std::str::FromStr;
use std::{convert::TryInto, fmt};

use crypto::{checksum, ChecksumType};
use std::ops::Deref;
use {AddressHashEnum, AddressPrefix, DisplayLayout};

use crate::{address::detect_checksum, Error};

/// Struct for legacy address representation.
/// Note: LegacyAddress::from_str deserialization is added, which is used at least in the convertaddress rpc.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Default)]
pub struct LegacyAddress {
    /// The prefix of the address.
    pub prefix: AddressPrefix,
    /// Checksum type
    pub checksum_type: ChecksumType,
    /// Public key hash.
    pub hash: Vec<u8>,
}

pub struct LegacyAddressDisplayLayout(Vec<u8>);

impl Deref for LegacyAddressDisplayLayout {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DisplayLayout for LegacyAddress {
    type Target = LegacyAddressDisplayLayout;

    fn layout(&self) -> Self::Target {
        let mut result = self.prefix.to_vec();
        result.extend_from_slice(&self.hash.to_vec());
        let cs = checksum(&result, &self.checksum_type);
        result.extend_from_slice(&*cs);

        LegacyAddressDisplayLayout(result)
    }

    fn from_layout(data: &[u8]) -> Result<Self, Error>
    where
        Self: Sized,
    {
        match data.len() {
            25 => {
                let checksum_type = detect_checksum(&data[0..21], &data[21..])?;
                let hash = data[1..21].to_vec();

                let address = LegacyAddress {
                    prefix: data[0..1].try_into().expect("prefix conversion should not fail"),
                    checksum_type,
                    hash,
                };

                Ok(address)
            },
            26 => {
                let checksum_type = detect_checksum(&data[0..22], &data[22..])?;
                let hash = data[2..22].to_vec();

                let address = LegacyAddress {
                    prefix: data[0..2].try_into().expect("prefix conversion should not fail"),
                    checksum_type,
                    hash,
                };

                Ok(address)
            },
            _ => Err(Error::InvalidAddress),
        }
    }
}

/// Converts legacy addresses from string
impl FromStr for LegacyAddress {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error>
    where
        Self: Sized,
    {
        let hex = bs58::decode(s).into_vec().map_err(|_| Error::InvalidAddress)?;
        LegacyAddress::from_layout(&hex)
    }
}

impl From<&'static str> for LegacyAddress {
    fn from(s: &'static str) -> Self {
        s.parse().unwrap_or_default()
    }
}

impl fmt::Display for LegacyAddress {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        bs58::encode(self.layout().as_ref()).into_string().fmt(fmt)
    }
}

impl LegacyAddress {
    pub fn new(hash: &AddressHashEnum, prefix: AddressPrefix, checksum_type: ChecksumType) -> LegacyAddress {
        LegacyAddress {
            prefix,
            checksum_type,
            hash: hash.to_vec(),
        }
    }
}
