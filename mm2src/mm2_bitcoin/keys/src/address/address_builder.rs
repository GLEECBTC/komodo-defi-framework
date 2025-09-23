use crate::Public;
use crypto::ChecksumType;

use {Address, AddressFormat, AddressHashEnum, AddressPrefix, AddressScriptType, NetworkAddressPrefixes};

/// Params for AddressBuilder to select output script type
#[derive(PartialEq)]
pub enum AddressBuilderOption {
    /// build for pay to pubkey hash output (witness or legacy)
    PubkeyHash(AddressHashEnum),
    /// build for pay to script hash output (witness or legacy)
    ScriptHash(AddressHashEnum),
    /// build for pay to pubkey hash but using a public key as an input (not pubkey hash)
    FromPubKey(Public),
}

/// Builds Address struct depending on addr_format, validates params to build Address
pub struct AddressBuilder {
    /// Coin base58 address prefixes from coin config
    prefixes: NetworkAddressPrefixes,
    /// Segwit addr human readable part
    hrp: Option<String>,
    /// Checksum type
    checksum_type: ChecksumType,
    /// Address Format
    addr_format: AddressFormat,
    /// Indicate whether tx output for this address is pubkey hash or script hash
    build_option: Option<AddressBuilderOption>,
}

impl AddressBuilder {
    pub fn new(
        addr_format: AddressFormat,
        checksum_type: ChecksumType,
        prefixes: NetworkAddressPrefixes,
        hrp: Option<String>,
    ) -> Self {
        Self {
            addr_format,
            checksum_type,
            prefixes,
            hrp,
            build_option: None,
        }
    }

    /// Sets build option for Address tx output script type
    pub fn with_build_option(mut self, build_option: AddressBuilderOption) -> Self {
        self.build_option = Some(build_option);
        self
    }

    /// Sets Address tx output script type as p2pkh or p2wpkh, but also keep the public key stored.
    pub fn as_pkh_from_pk(mut self, pubkey: Public) -> Self {
        self.build_option = Some(AddressBuilderOption::FromPubKey(pubkey));
        self
    }

    /// Sets Address tx output script type as p2pkh or p2wpkh
    pub fn as_pkh(mut self, hash: AddressHashEnum) -> Self {
        self.build_option = Some(AddressBuilderOption::PubkeyHash(hash));
        self
    }

    /// Sets Address tx output script type as p2sh or p2wsh
    pub fn as_sh(mut self, hash: AddressHashEnum) -> Self {
        self.build_option = Some(AddressBuilderOption::ScriptHash(hash));
        self
    }

    pub fn build(&self) -> Result<Address, String> {
        let build_option = self.build_option.as_ref().ok_or("no address builder option set")?;
        match &self.addr_format {
            AddressFormat::Standard => Ok(Address {
                prefix: self.get_address_prefix(build_option)?,
                hrp: None,
                hash: self.get_hash(build_option),
                pubkey: self.get_pubkey(build_option),
                checksum_type: self.checksum_type,
                addr_format: self.addr_format.clone(),
                script_type: self.get_legacy_script_type(build_option),
            }),
            AddressFormat::Segwit { .. } => {
                self.check_segwit_hrp()?;
                self.check_segwit_hash(build_option)?;
                Ok(Address {
                    prefix: AddressPrefix::default(),
                    hrp: self.hrp.clone(),
                    hash: self.get_hash(build_option),
                    pubkey: self.get_pubkey(build_option),
                    checksum_type: self.checksum_type,
                    addr_format: self.addr_format.clone(),
                    script_type: self.get_segwit_script_type(build_option),
                })
            },
            AddressFormat::CashAddress { .. } => Ok(Address {
                prefix: self.get_address_prefix(build_option)?,
                hrp: None,
                hash: self.get_hash(build_option),
                pubkey: self.get_pubkey(build_option),
                checksum_type: self.checksum_type,
                addr_format: self.addr_format.clone(),
                script_type: self.get_legacy_script_type(build_option),
            }),
        }
    }

    fn get_address_prefix(&self, build_option: &AddressBuilderOption) -> Result<AddressPrefix, String> {
        let prefix = match build_option {
            AddressBuilderOption::PubkeyHash(_) | AddressBuilderOption::FromPubKey(_) => &self.prefixes.p2pkh,
            AddressBuilderOption::ScriptHash(_) => &self.prefixes.p2sh,
        };
        if prefix.is_empty() {
            return Err("no prefix for address set".to_owned());
        }
        Ok(prefix.clone())
    }

    fn get_legacy_script_type(&self, build_option: &AddressBuilderOption) -> AddressScriptType {
        match build_option {
            AddressBuilderOption::PubkeyHash(_) | AddressBuilderOption::FromPubKey(_) => AddressScriptType::P2PKH,
            AddressBuilderOption::ScriptHash(_) => AddressScriptType::P2SH,
        }
    }

    fn get_segwit_script_type(&self, build_option: &AddressBuilderOption) -> AddressScriptType {
        match build_option {
            AddressBuilderOption::PubkeyHash(_) | AddressBuilderOption::FromPubKey(_) => AddressScriptType::P2WPKH,
            AddressBuilderOption::ScriptHash(_) => AddressScriptType::P2WSH,
        }
    }

    fn get_hash(&self, build_option: &AddressBuilderOption) -> AddressHashEnum {
        match build_option {
            AddressBuilderOption::PubkeyHash(hash) => hash.clone(),
            AddressBuilderOption::ScriptHash(hash) => hash.clone(),
            AddressBuilderOption::FromPubKey(pubkey) => AddressHashEnum::AddressHash(pubkey.address_hash()),
        }
    }

    fn get_pubkey(&self, build_option: &AddressBuilderOption) -> Option<Public> {
        match build_option {
            AddressBuilderOption::FromPubKey(pubkey) => Some(*pubkey),
            _ => None,
        }
    }

    fn check_segwit_hrp(&self) -> Result<(), String> {
        if self.hrp.is_none() {
            return Err("no hrp for address".to_owned());
        }
        Ok(())
    }

    fn check_segwit_hash(&self, build_option: &AddressBuilderOption) -> Result<(), String> {
        let is_hash_valid = match build_option {
            AddressBuilderOption::PubkeyHash(hash) => hash.is_address_hash(),
            AddressBuilderOption::ScriptHash(hash) => hash.is_witness_script_hash(),
            AddressBuilderOption::FromPubKey(_) => true,
        };
        if !is_hash_valid {
            return Err("invalid hash for segwit address".to_owned());
        }
        Ok(())
    }
}
