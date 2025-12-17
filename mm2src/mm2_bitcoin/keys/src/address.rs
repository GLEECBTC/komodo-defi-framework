//! `AddressHash` with network identifier and format type
//!
//! A Bitcoin address, or simply address, is an identifier of 26-35 alphanumeric characters, beginning with the number 1
//! or 3, that represents a possible destination for a bitcoin payment.
//!
//! https://en.bitcoin.it/wiki/Address

use crate::Public;
use crypto::{dgroestl512, dhash256, keccak256, ChecksumType};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::{fmt, hash::Hash};

use {
    AddressHashEnum, AddressPrefix, CashAddrType, CashAddress, Error, LegacyAddress, NetworkAddressPrefixes,
    SegwitAddress,
};

mod address_builder;
pub use self::address_builder::{AddressBuilder, AddressBuilderOption};

/// There are two address formats currently in use.
/// https://bitcoin.org/en/developer-reference#address-conversion
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum AddressScriptType {
    /// Pay to PubKey Hash
    /// Common P2PKH which begin with the number 1, eg: 1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2.
    /// https://bitcoin.org/en/glossary/p2pkh-address
    P2PKH,
    /// Pay to Script Hash
    /// Newer P2SH type starting with the number 3, eg: 3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy.
    /// https://bitcoin.org/en/glossary/p2sh-address
    P2SH,
    /// Pay to Witness PubKey Hash
    /// Segwit P2WPKH which begins with the human readable part followed by 1 followed by 39 base32 characters
    /// as the address hash, eg: bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4.
    /// https://github.com/bitcoin/bips/blob/master/bip-0173.mediawiki
    P2WPKH,
    /// Pay to Witness Script Hash
    /// Segwit P2WSH which begins with the human readable part followed by 1 followed by 59 base32 characters
    /// as the scripthash, eg: bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3.
    /// https://github.com/bitcoin/bips/blob/master/bip-0173.mediawiki
    P2WSH,
}

#[derive(Clone, Debug, Default, Display, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "format")]
pub enum AddressFormat {
    /// Standard UTXO address format.
    /// In Bitcoin Cash context the standard format also known as 'legacy'.
    #[serde(rename = "standard")]
    #[display(fmt = "Legacy")]
    #[default]
    Standard,
    /// Segwit Address
    /// https://github.com/bitcoin/bips/blob/master/bip-0173.mediawiki
    #[serde(rename = "segwit")]
    Segwit,
    /// Bitcoin Cash specific address format.
    /// https://github.com/bitcoincashorg/bitcoincash.org/blob/master/spec/cashaddr.md
    #[serde(rename = "cashaddress")]
    #[display(fmt = "CashAddress")]
    CashAddress {
        network: String,
        #[serde(default)]
        pub_addr_prefix: u8,
        #[serde(default)]
        p2sh_addr_prefix: u8,
    },
}

impl AddressFormat {
    pub fn is_segwit(&self) -> bool {
        matches!(*self, AddressFormat::Segwit)
    }

    pub fn is_cashaddress(&self) -> bool {
        matches!(*self, AddressFormat::CashAddress { .. })
    }

    pub fn is_legacy(&self) -> bool {
        matches!(*self, AddressFormat::Standard)
    }
}

// Todo: add segwit checksum detection
pub fn detect_checksum(data: &[u8], checksum: &[u8]) -> Result<ChecksumType, Error> {
    if checksum == &dhash256(data)[0..4] {
        return Ok(ChecksumType::DSHA256);
    }

    if checksum == &dgroestl512(data)[0..4] {
        return Ok(ChecksumType::DGROESTL512);
    }

    if checksum == &keccak256(data)[0..4] {
        return Ok(ChecksumType::KECCAK256);
    }
    Err(Error::InvalidChecksum)
}

/// Struct for utxo address types representation
/// Contains address hash, format, prefix to get as a string.
/// Also has output ScriptType field to create output script.
#[derive(Clone, Debug, Eq)]
pub struct Address {
    /// The base58 prefix of the address.
    prefix: AddressPrefix,
    /// Segwit addr human readable part
    hrp: Option<String>,
    /// The public key of the address.
    pubkey: Option<Public>,
    /// Public key/Script hash.
    hash: AddressHashEnum,
    /// Checksum type
    checksum_type: ChecksumType,
    /// Address Format
    addr_format: AddressFormat,
    // which output script corresponds to this address format and prefix
    script_type: AddressScriptType,
}

impl PartialEq for Address {
    /// A `PartialEq` implementation that doesn't take `pubkey` into consideration. That's because
    /// we want to rely on address `hash` only for knowing whether two addresses are equal since `pubkey`
    /// might not always be available.
    fn eq(&self, other: &Self) -> bool {
        self.prefix == other.prefix
            && self.hrp == other.hrp
            && self.hash == other.hash
            && self.checksum_type == other.checksum_type
            && self.addr_format == other.addr_format
            && self.script_type == other.script_type
    }
}

impl Hash for Address {
    /// Like the `PartialEq` implementation, this `Hash` implementation doesn't take `pubkey` into consideration.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.prefix.hash(state);
        self.hrp.hash(state);
        self.hash.hash(state);
        self.checksum_type.hash(state);
        self.addr_format.hash(state);
        self.script_type.hash(state);
    }
}

impl Address {
    pub fn prefix(&self) -> &AddressPrefix {
        &self.prefix
    }
    pub fn hrp(&self) -> &Option<String> {
        &self.hrp
    }
    pub fn pubkey(&self) -> &Option<Public> {
        &self.pubkey
    }
    pub fn hash(&self) -> &AddressHashEnum {
        &self.hash
    }
    pub fn checksum_type(&self) -> &ChecksumType {
        &self.checksum_type
    }
    pub fn addr_format(&self) -> &AddressFormat {
        &self.addr_format
    }
    pub fn script_type(&self) -> &AddressScriptType {
        &self.script_type
    }

    /// Returns true if output script type is pubkey hash (p2pkh or p2wpkh)
    pub fn is_pubkey_hash(&self) -> bool {
        if matches!(self.addr_format, AddressFormat::Segwit) {
            self.script_type == AddressScriptType::P2WPKH
        } else {
            self.script_type == AddressScriptType::P2PKH
        }
    }

    pub fn display_address(&self) -> Result<String, String> {
        match &self.addr_format {
            AddressFormat::Standard => {
                Ok(LegacyAddress::new(&self.hash, self.prefix.clone(), self.checksum_type).to_string())
            },
            AddressFormat::Segwit => match &self.hrp {
                Some(hrp) => Ok(SegwitAddress::new(&self.hash, hrp.clone()).to_string()),
                None => Err("Cannot display segwit address for a coin with no bech32_hrp in config".into()),
            },
            AddressFormat::CashAddress {
                network,
                pub_addr_prefix,
                p2sh_addr_prefix,
            } => self
                .to_cashaddress(
                    network,
                    &NetworkAddressPrefixes {
                        p2pkh: [*pub_addr_prefix].into(),
                        p2sh: [*p2sh_addr_prefix].into(),
                    },
                )
                .and_then(|cashaddress| cashaddress.encode()),
        }
    }

    pub fn from_legacyaddress(s: &str, prefixes: &NetworkAddressPrefixes) -> Result<Address, String> {
        let address = LegacyAddress::from_str(s).map_err(|_| String::from("invalid address"))?;
        if address.hash.len() != 20 {
            return Err("Expect 20 bytes long hash".into());
        }
        let mut hash = AddressHashEnum::default_address_hash();
        hash.copy_from_slice(address.hash.as_slice());

        let script_type = if address.prefix == prefixes.p2pkh {
            AddressScriptType::P2PKH
        } else if address.prefix == prefixes.p2sh {
            AddressScriptType::P2SH
        } else {
            return Err(String::from("invalid address prefix"));
        };

        Ok(Address {
            prefix: address.prefix,
            hash,
            checksum_type: address.checksum_type,
            hrp: None,
            pubkey: None,
            addr_format: AddressFormat::Standard,
            script_type,
        })
    }

    pub fn from_cashaddress(
        cashaddr: &str,
        checksum_type: ChecksumType,
        net_addr_prefixes: &NetworkAddressPrefixes,
    ) -> Result<Address, String> {
        let address = CashAddress::decode(cashaddr)?;

        if address.hash.len() != 20 {
            return Err("Expect 20 bytes long hash".into());
        }

        let mut hash = AddressHashEnum::default_address_hash();
        hash.copy_from_slice(address.hash.as_slice());

        let (script_type, addr_prefix) = match address.address_type {
            CashAddrType::P2PKH => (AddressScriptType::P2PKH, net_addr_prefixes.p2pkh.clone()),
            CashAddrType::P2SH => (AddressScriptType::P2SH, net_addr_prefixes.p2sh.clone()),
        };

        Ok(Address {
            prefix: addr_prefix,
            hash,
            checksum_type,
            hrp: None,
            pubkey: None,
            addr_format: AddressFormat::CashAddress {
                network: address.prefix.to_string(),
                pub_addr_prefix: net_addr_prefixes.p2pkh.get_size_1_prefix(),
                p2sh_addr_prefix: net_addr_prefixes.p2sh.get_size_1_prefix(),
            },
            script_type,
        })
    }

    pub fn to_cashaddress(
        &self,
        network_prefix: &str,
        network_addr_prefixes: &NetworkAddressPrefixes,
    ) -> Result<CashAddress, String> {
        let address_type = if self.prefix == network_addr_prefixes.p2pkh {
            CashAddrType::P2PKH
        } else if self.prefix == network_addr_prefixes.p2sh {
            CashAddrType::P2SH
        } else {
            return Err(format!(
                "Unknown address prefix {}. Expect: {}, {}",
                self.prefix, network_addr_prefixes.p2pkh, network_addr_prefixes.p2sh
            ));
        };
        CashAddress::new(network_prefix, self.hash.to_vec(), address_type)
    }

    pub fn from_segwitaddress(segaddr: &str, checksum_type: ChecksumType) -> Result<Address, String> {
        let address = SegwitAddress::from_str(segaddr).map_err(|e| e.to_string())?;

        let (script_type, mut hash) = if address.program.len() == 20 {
            (AddressScriptType::P2WPKH, AddressHashEnum::default_address_hash())
        } else if address.program.len() == 32 {
            (AddressScriptType::P2WSH, AddressHashEnum::default_witness_script_hash())
        } else {
            return Err("Expect either 20 or 32 bytes long hash".into());
        };
        hash.copy_from_slice(address.program.as_slice());

        let hrp = Some(address.hrp);

        Ok(Address {
            prefix: AddressPrefix::default(),
            hash,
            checksum_type,
            hrp,
            pubkey: None,
            addr_format: AddressFormat::Segwit,
            script_type,
        })
    }

    pub fn to_segwitaddress(&self) -> Result<SegwitAddress, String> {
        match &self.hrp {
            Some(hrp) => Ok(SegwitAddress::new(&self.hash, hrp.to_string())),
            None => Err("hrp must be provided for segwit address".into()),
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.addr_format {
            AddressFormat::Segwit => {
                SegwitAddress::new(&self.hash, self.hrp.clone().expect("Segwit address should have an hrp")).fmt(f)
            },
            AddressFormat::CashAddress {
                network,
                pub_addr_prefix,
                p2sh_addr_prefix,
            } => {
                let cash_address = self
                    .to_cashaddress(
                        network,
                        &NetworkAddressPrefixes {
                            p2pkh: [*pub_addr_prefix].into(),
                            p2sh: [*p2sh_addr_prefix].into(),
                        },
                    )
                    .expect("A valid address");
                cash_address.encode().expect("A valid address").fmt(f)
            },
            AddressFormat::Standard => LegacyAddress::new(&self.hash, self.prefix.clone(), self.checksum_type).fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Address, AddressBuilder, AddressFormat, AddressHashEnum, CashAddrType, CashAddress, ChecksumType};
    use crate::address_prefixes::prefixes::*;
    use crate::{NetworkAddressPrefixes, NetworkPrefix};

    #[test]
    fn test_address_to_string() {
        let address = AddressBuilder::new(
            AddressFormat::Standard,
            ChecksumType::DSHA256,
            (*BTC_PREFIXES).clone(),
            None,
        )
        .as_pkh(AddressHashEnum::AddressHash(
            "3f4aa1fedf1f54eeb03b759deadb36676b184911".into(),
        ))
        .build()
        .expect("valid address props");

        assert_eq!("16meyfSoQV6twkAAxPe51RtMVz7PGRmWna".to_owned(), address.to_string());
    }

    #[test]
    fn test_komodo_address_to_string() {
        let address = AddressBuilder::new(
            AddressFormat::Standard,
            ChecksumType::DSHA256,
            (*KMD_PREFIXES).clone(),
            None,
        )
        .as_pkh(AddressHashEnum::AddressHash(
            "05aab5342166f8594baf17a7d9bef5d567443327".into(),
        ))
        .build()
        .expect("valid address props");

        assert_eq!("R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW".to_owned(), address.to_string());
    }

    #[test]
    fn test_zec_t_address_to_string() {
        let address = AddressBuilder::new(
            AddressFormat::Standard,
            ChecksumType::DSHA256,
            (*T_ZCASH_PREFIXES).clone(),
            None,
        )
        .as_pkh(AddressHashEnum::AddressHash(
            "05aab5342166f8594baf17a7d9bef5d567443327".into(),
        ))
        .build()
        .expect("valid address props");

        assert_eq!("tmAEKD7psc1ajK76QMGEW8WGQSBBHf9SqCp".to_owned(), address.to_string());
    }

    #[test]
    fn test_komodo_p2sh_address_to_string() {
        let address = AddressBuilder::new(
            AddressFormat::Standard,
            ChecksumType::DSHA256,
            (*KMD_PREFIXES).clone(),
            None,
        )
        .as_sh(AddressHashEnum::AddressHash(
            "ca0c3786c96ff7dacd40fdb0f7c196528df35f85".into(),
        ))
        .build()
        .expect("valid address props"); // TODO: check with P2PKH

        assert_eq!("bX9bppqdGvmCCAujd76Tq76zs1suuPnB9A".to_owned(), address.to_string());
    }

    #[test]
    fn test_address_from_str() {
        let address = AddressBuilder::new(
            AddressFormat::Standard,
            ChecksumType::DSHA256,
            (*BTC_PREFIXES).clone(),
            None,
        )
        .as_pkh(AddressHashEnum::AddressHash(
            "3f4aa1fedf1f54eeb03b759deadb36676b184911".into(),
        ))
        .build()
        .expect("valid address props");

        assert_eq!(
            address,
            Address::from_legacyaddress("16meyfSoQV6twkAAxPe51RtMVz7PGRmWna", &BTC_PREFIXES).unwrap()
        );
        assert_eq!(address.to_string(), "16meyfSoQV6twkAAxPe51RtMVz7PGRmWna".to_owned());
    }

    #[test]
    fn test_komodo_address_from_str() {
        let address = AddressBuilder::new(
            AddressFormat::Standard,
            ChecksumType::DSHA256,
            (*KMD_PREFIXES).clone(),
            None,
        )
        .as_pkh(AddressHashEnum::AddressHash(
            "05aab5342166f8594baf17a7d9bef5d567443327".into(),
        ))
        .build()
        .expect("valid address props");

        assert_eq!(
            address,
            Address::from_legacyaddress("R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW", &KMD_PREFIXES).unwrap()
        );
        assert_eq!(address.to_string(), "R9o9xTocqr6CeEDGDH6mEYpwLoMz6jNjMW".to_owned());
    }

    #[test]
    fn test_zec_address_from_str() {
        let address = AddressBuilder::new(
            AddressFormat::Standard,
            ChecksumType::DSHA256,
            (*T_ZCASH_PREFIXES).clone(),
            None,
        )
        .as_pkh(AddressHashEnum::AddressHash(
            "05aab5342166f8594baf17a7d9bef5d567443327".into(),
        ))
        .build()
        .expect("valid address props");

        assert_eq!(
            address,
            Address::from_legacyaddress("tmAEKD7psc1ajK76QMGEW8WGQSBBHf9SqCp", &T_ZCASH_PREFIXES).unwrap()
        );
        assert_eq!(address.to_string(), "tmAEKD7psc1ajK76QMGEW8WGQSBBHf9SqCp".to_owned());
    }

    #[test]
    fn test_komodo_p2sh_address_from_str() {
        let address = AddressBuilder::new(
            AddressFormat::Standard,
            ChecksumType::DSHA256,
            (*KMD_PREFIXES).clone(),
            None,
        )
        .as_sh(AddressHashEnum::AddressHash(
            "ca0c3786c96ff7dacd40fdb0f7c196528df35f85".into(),
        ))
        .build()
        .expect("valid address props");

        assert_eq!(
            address,
            Address::from_legacyaddress("bX9bppqdGvmCCAujd76Tq76zs1suuPnB9A", &KMD_PREFIXES).unwrap()
        );
        assert_eq!(address.to_string(), "bX9bppqdGvmCCAujd76Tq76zs1suuPnB9A".to_owned());
    }

    #[test]
    fn test_grs_addr_from_str() {
        let address = AddressBuilder::new(
            AddressFormat::Standard,
            ChecksumType::DGROESTL512,
            (*GRS_PREFIXES).clone(),
            None,
        )
        .as_pkh(AddressHashEnum::AddressHash(
            "c3f710deb7320b0efa6edb14e3ebeeb9155fa90d".into(),
        ))
        .build()
        .expect("valid address props");

        assert_eq!(
            address,
            Address::from_legacyaddress("Fo2tBkpzaWQgtjFUkemsYnKyfvd2i8yTki", &GRS_PREFIXES).unwrap()
        );
        assert_eq!(address.to_string(), "Fo2tBkpzaWQgtjFUkemsYnKyfvd2i8yTki".to_owned());
    }

    #[test]
    fn test_smart_addr_from_str() {
        let address = AddressBuilder::new(
            AddressFormat::Standard,
            ChecksumType::KECCAK256,
            (*SYS_PREFIXES).clone(),
            None,
        )
        .as_pkh(AddressHashEnum::AddressHash(
            "56bb05aa20f5a80cf84e90e5dab05be331333e27".into(),
        ))
        .build()
        .expect("valid address props");

        assert_eq!(
            address,
            Address::from_legacyaddress("SVCbBs6FvPYxJrYoJc4TdCe47QNCgmTabv", &SYS_PREFIXES).unwrap()
        );
        assert_eq!(address.to_string(), "SVCbBs6FvPYxJrYoJc4TdCe47QNCgmTabv".to_owned());
    }

    #[test]
    fn test_from_to_cashaddress() {
        let cashaddresses = [
            "bitcoincash:qzxqqt9lh4feptf0mplnk58gnajfepzwcq9f2rxk55",
            "bitcoincash:qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2",
            "bitcoincash:pq4ql3ph6738xuv2cycduvkpu4rdwqge5q2uxdfg6f",
        ];
        let expected = [
            "1DmFp16U73RrVZtYUbo2Ectt8mAnYScpqM",
            "1PQPheJQSauxRPTxzNMUco1XmoCyPoEJCp",
            "35XRC5HRZjih1sML23UXv1Ry1SzTDKSmfQ",
        ];

        for i in 0..3 {
            let actual_address =
                Address::from_cashaddress(cashaddresses[i], ChecksumType::DSHA256, &BCH_PREFIXES).unwrap();
            let expected_address: Address = Address::from_legacyaddress(expected[i], &BCH_PREFIXES).unwrap();
            // comparing only hashes here as Address::from_cashaddress has a different internal format from into()
            assert_eq!(actual_address.hash, expected_address.hash);
            let actual_cashaddress = actual_address
                .to_cashaddress("bitcoincash", &BCH_PREFIXES)
                .unwrap()
                .encode()
                .unwrap();
            let expected_cashaddress = cashaddresses[i];
            assert_eq!(actual_cashaddress, expected_cashaddress);
        }
    }

    #[test]
    fn test_from_cashaddress_err() {
        assert_eq!(
            Address::from_cashaddress(
                "bitcoincash:qgagf7w02x4wnz3mkwnchut2vxphjzccwxgjvvjmlsxqwkcw59jxxuz",
                ChecksumType::DSHA256,
                &BCH_PREFIXES,
            ),
            Err("Expect 20 bytes long hash".into())
        );
    }

    #[test]
    fn test_to_cashaddress_err() {
        let unknown_prefixes: NetworkAddressPrefixes = NetworkAddressPrefixes {
            p2pkh: [2; 1].into(),
            p2sh: [2; 1].into(),
        };
        let address = AddressBuilder::new(
            AddressFormat::CashAddress {
                network: "bitcoincash".into(),
                pub_addr_prefix: 0,
                p2sh_addr_prefix: 5,
            },
            ChecksumType::DSHA256,
            unknown_prefixes,
            None,
        )
        .as_sh(AddressHashEnum::AddressHash(
            [
                140, 0, 44, 191, 189, 83, 144, 173, 47, 216, 127, 59, 80, 232, 159, 100, 156, 132, 78, 192,
            ]
            .into(),
        ))
        .build()
        .expect("valid address props"); // actually prefix == 2 is unknown and is neither P2PKH nor P2SH

        assert_eq!(
            address.to_cashaddress("bitcoincash", &BCH_PREFIXES),
            Err("Unknown address prefix [2]. Expect: [0], [5]".into())
        );
    }

    #[test]
    fn test_to_cashaddress_other_prefix() {
        let expected_address = CashAddress {
            prefix: NetworkPrefix::Other("prefix".into()),
            hash: vec![
                140, 0, 44, 191, 189, 83, 144, 173, 47, 216, 127, 59, 80, 232, 159, 100, 156, 132, 78, 192,
            ],
            address_type: CashAddrType::P2PKH,
        };
        let address: Address =
            Address::from_legacyaddress("1DmFp16U73RrVZtYUbo2Ectt8mAnYScpqM", &BCH_PREFIXES).unwrap();
        assert_eq!(
            address.to_cashaddress("prefix", &BCH_PREFIXES).unwrap(),
            expected_address
        );
    }
}
