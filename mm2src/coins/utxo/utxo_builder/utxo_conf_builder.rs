use crate::utxo::rpc_clients::EstimateFeeMode;
use crate::utxo::{
    parse_hex_encoded_u32, UtxoCoinConf, DEFAULT_DYNAMIC_FEE_VOLATILITY_PERCENT, KMD_MTP_BLOCK_COUNT,
    MATURE_CONFIRMATIONS_DEFAULT,
};
use crate::UtxoActivationParams;
use bitcrypto::ChecksumType;
use crypto::{Bip32Error, HDPathToCoin};
use derive_more::Display;
pub use keys::AddressFormat as UtxoAddressFormat;
use keys::NetworkAddressPrefixes;
use mm2_err_handle::prelude::*;
use script::SignatureVersion;
use serde_json::{self as json, Value as Json};
use spv_validation::conf::SPVConf;
use std::convert::TryInto;
use std::num::NonZeroU64;
use std::sync::atomic::AtomicBool;

pub type UtxoConfResult<T> = Result<T, MmError<UtxoConfError>>;

#[derive(Debug, Display)]
pub enum UtxoConfError {
    #[display(fmt = "'name' field is not found in config")]
    CurrencyNameIsNotSet,
    #[display(fmt = "'derivation_path' field is not found in config")]
    DerivationPathIsNotSet,
    #[display(fmt = "'trezor_coin' field is not found in config")]
    TrezorCoinIsNotSet,
    #[display(fmt = "Error deserializing 'derivation_path': {_0}")]
    ErrorDeserializingDerivationPath(String),
    #[display(fmt = "Error deserializing 'spv_conf': {_0}")]
    ErrorDeserializingSPVConf(String),
    InvalidConsensusBranchId(String),
    InvalidVersionGroupId(String),
    InvalidAddressFormat(String),
    InvalidDecimals(String),
    InvalidProtocolData(String),
}

impl From<Bip32Error> for UtxoConfError {
    fn from(e: Bip32Error) -> Self {
        UtxoConfError::ErrorDeserializingDerivationPath(e.to_string())
    }
}

pub struct UtxoConfBuilder<'a> {
    conf: &'a Json,
    ticker: &'a str,
    params: &'a UtxoActivationParams,
}

impl<'a> UtxoConfBuilder<'a> {
    pub fn new(conf: &'a Json, params: &'a UtxoActivationParams, ticker: &'a str) -> Self {
        UtxoConfBuilder { conf, ticker, params }
    }

    pub fn build(&self) -> UtxoConfResult<UtxoCoinConf> {
        let checksum_type = self.checksum_type();

        let pub_addr_prefix = self.pub_addr_prefix();
        let pub_t_addr_prefix = self.pub_t_address_prefix();
        let mut p2pkh_prefixes = vec![];
        if pub_t_addr_prefix != 0 {
            p2pkh_prefixes.push(pub_t_addr_prefix);
        }
        p2pkh_prefixes.push(pub_addr_prefix);
        drop_mutability!(p2pkh_prefixes);

        let p2sh_addr_prefix = self.p2sh_address_prefix();
        let p2sh_t_addr_prefix = self.p2sh_t_address_prefix();
        let mut p2sh_prefixes = vec![];
        if p2sh_t_addr_prefix != 0 {
            p2sh_prefixes.push(p2sh_t_addr_prefix);
        }
        p2sh_prefixes.push(p2sh_addr_prefix);
        drop_mutability!(p2sh_prefixes);

        let address_prefixes = NetworkAddressPrefixes {
            p2pkh: p2pkh_prefixes.as_slice().try_into().expect("prefixes valid"),
            p2sh: p2sh_prefixes.as_slice().try_into().expect("prefixes valid"),
        };
        let sign_message_prefix = self.sign_message_prefix();

        let wif_prefix = self.wif_prefix();

        let bech32_hrp = self.bech32_hrp();

        let default_address_format = self.default_address_format();

        let asset_chain = self.asset_chain();
        let tx_version = self.tx_version();
        let overwintered = self.overwintered();

        let tx_fee_volatility_percent = self.tx_fee_volatility_percent();
        let version_group_id = self.version_group_id(tx_version, overwintered)?;
        let consensus_branch_id = self.consensus_branch_id(tx_version)?;
        let signature_version = self.signature_version();
        let fork_id = self.fork_id();

        // should be sufficient to detect zcash by overwintered flag
        let zcash = overwintered;

        let required_confirmations = self.required_confirmations();
        let requires_notarization = self.requires_notarization();

        let mature_confirmations = self.mature_confirmations();

        let is_pos = self.is_pos();
        let is_posv = self.is_posv();
        let segwit = self.segwit();
        let force_min_relay_fee = self.conf["force_min_relay_fee"].as_bool().unwrap_or(false);
        let mtp_block_count = self.mtp_block_count();
        let estimate_fee_mode = self.estimate_fee_mode();
        let estimate_fee_blocks = self.estimate_fee_blocks();
        let trezor_coin = self.trezor_coin();
        let derivation_path = self.derivation_path()?;
        let avg_blocktime = self.avg_blocktime();
        let spv_conf = self.spv_conf()?;

        Ok(UtxoCoinConf {
            ticker: self.ticker.to_owned(),
            is_pos,
            is_posv,
            requires_notarization,
            overwintered,
            address_prefixes,
            sign_message_prefix,
            bech32_hrp,
            segwit,
            wif_prefix,
            tx_version,
            default_address_format,
            asset_chain,
            tx_fee_volatility_percent,
            version_group_id,
            consensus_branch_id,
            zcash,
            checksum_type,
            signature_version,
            fork_id,
            required_confirmations: required_confirmations.into(),
            force_min_relay_fee,
            mtp_block_count,
            estimate_fee_mode,
            mature_confirmations,
            estimate_fee_blocks,
            trezor_coin,
            spv_conf,
            derivation_path,
            avg_blocktime,
        })
    }

    fn checksum_type(&self) -> ChecksumType {
        match self.ticker {
            "GRS" => ChecksumType::DGROESTL512,
            "SMART" => ChecksumType::KECCAK256,
            _ => ChecksumType::DSHA256,
        }
    }

    fn pub_addr_prefix(&self) -> u8 {
        let pubtype = self.conf["pubtype"]
            .as_u64()
            .unwrap_or(if self.ticker == "BTC" { 0 } else { 60 });
        pubtype as u8
    }

    fn p2sh_address_prefix(&self) -> u8 {
        self.conf["p2shtype"]
            .as_u64()
            .unwrap_or(if self.ticker == "BTC" { 5 } else { 85 }) as u8
    }

    fn pub_t_address_prefix(&self) -> u8 {
        self.conf["taddr"].as_u64().unwrap_or(0) as u8
    }

    fn p2sh_t_address_prefix(&self) -> u8 {
        self.conf["taddr"].as_u64().unwrap_or(0) as u8
    }

    fn sign_message_prefix(&self) -> Option<String> {
        json::from_value(self.conf["sign_message_prefix"].clone()).unwrap_or(None)
    }

    fn wif_prefix(&self) -> u8 {
        let wiftype = self.conf["wiftype"]
            .as_u64()
            .unwrap_or(if self.ticker == "BTC" { 128 } else { 188 });
        wiftype as u8
    }

    fn bech32_hrp(&self) -> Option<String> {
        json::from_value(self.conf["bech32_hrp"].clone()).unwrap_or(None)
    }

    fn default_address_format(&self) -> UtxoAddressFormat {
        let mut address_format: UtxoAddressFormat =
            json::from_value(self.conf["address_format"].clone()).unwrap_or(UtxoAddressFormat::Standard);

        if let UtxoAddressFormat::CashAddress {
            network: _,
            ref mut pub_addr_prefix,
            ref mut p2sh_addr_prefix,
        } = address_format
        {
            *pub_addr_prefix = self.pub_addr_prefix();
            *p2sh_addr_prefix = self.p2sh_address_prefix();
        }

        address_format
    }

    fn asset_chain(&self) -> bool {
        self.conf["asset"].as_str().is_some()
    }

    fn tx_version(&self) -> i32 {
        self.conf["txversion"].as_i64().unwrap_or(1) as i32
    }

    fn overwintered(&self) -> bool {
        self.conf["overwintered"].as_u64().unwrap_or(0) == 1
    }

    fn tx_fee_volatility_percent(&self) -> f64 {
        self.conf["txfee_volatility_percent"]
            .as_f64()
            .unwrap_or(DEFAULT_DYNAMIC_FEE_VOLATILITY_PERCENT)
    }

    fn version_group_id(&self, tx_version: i32, overwintered: bool) -> UtxoConfResult<u32> {
        let version_group_id = match self.conf["version_group_id"].as_str() {
            Some(s) => parse_hex_encoded_u32(s).mm_err(UtxoConfError::InvalidVersionGroupId)?,
            None => {
                if tx_version == 3 && overwintered {
                    0x03c4_8270
                } else if tx_version == 4 && overwintered {
                    0x892f_2085
                } else {
                    0
                }
            },
        };
        Ok(version_group_id)
    }

    fn consensus_branch_id(&self, tx_version: i32) -> UtxoConfResult<u32> {
        let consensus_branch_id = match self.conf["consensus_branch_id"].as_str() {
            Some(s) => parse_hex_encoded_u32(s).mm_err(UtxoConfError::InvalidConsensusBranchId)?,
            None => match tx_version {
                3 => 0x5ba8_1b19,
                4 => 0x76b8_09bb,
                _ => 0,
            },
        };
        Ok(consensus_branch_id)
    }

    fn signature_version(&self) -> SignatureVersion {
        let default_signature_version = if self.ticker == "BCH" || self.fork_id() != 0 {
            SignatureVersion::ForkId
        } else {
            SignatureVersion::Base
        };
        json::from_value(self.conf["signature_version"].clone()).unwrap_or(default_signature_version)
    }

    fn fork_id(&self) -> u32 {
        let default_fork_id = match self.ticker {
            "BCH" => "0x40",
            _ => "0x0",
        };
        let hex_string = self.conf["fork_id"].as_str().unwrap_or(default_fork_id);
        let fork_id = u32::from_str_radix(hex_string.trim_start_matches("0x"), 16).unwrap();
        fork_id
    }

    fn required_confirmations(&self) -> u64 {
        // param from request should override the config
        self.params
            .required_confirmations
            .unwrap_or_else(|| self.conf["required_confirmations"].as_u64().unwrap_or(1))
    }

    fn requires_notarization(&self) -> AtomicBool {
        self.params
            .requires_notarization
            .unwrap_or_else(|| self.conf["requires_notarization"].as_bool().unwrap_or(false))
            .into()
    }

    fn mature_confirmations(&self) -> u32 {
        self.conf["mature_confirmations"]
            .as_u64()
            .map(|x| x as u32)
            .unwrap_or(MATURE_CONFIRMATIONS_DEFAULT)
    }

    fn is_pos(&self) -> bool {
        self.conf["isPoS"].as_u64() == Some(1)
    }

    fn is_posv(&self) -> bool {
        self.conf["isPoSV"].as_u64() == Some(1)
    }

    fn segwit(&self) -> bool {
        self.conf["segwit"].as_bool().unwrap_or(false)
    }

    fn mtp_block_count(&self) -> NonZeroU64 {
        json::from_value(self.conf["mtp_block_count"].clone()).unwrap_or(KMD_MTP_BLOCK_COUNT)
    }

    fn estimate_fee_mode(&self) -> Option<EstimateFeeMode> {
        json::from_value(self.conf["estimate_fee_mode"].clone()).unwrap_or(None)
    }

    fn estimate_fee_blocks(&self) -> u32 {
        json::from_value(self.conf["estimate_fee_blocks"].clone()).unwrap_or(1)
    }

    fn trezor_coin(&self) -> Option<String> {
        self.conf["trezor_coin"].as_str().map(|coin| coin.to_string())
    }

    fn spv_conf(&self) -> UtxoConfResult<Option<SPVConf>> {
        json::from_value(self.conf["spv_conf"].clone())
            .map_to_mm(|e| UtxoConfError::ErrorDeserializingSPVConf(e.to_string()))
    }

    fn derivation_path(&self) -> UtxoConfResult<Option<HDPathToCoin>> {
        json::from_value(self.conf["derivation_path"].clone())
            .map_to_mm(|e| UtxoConfError::ErrorDeserializingDerivationPath(e.to_string()))
    }

    fn avg_blocktime(&self) -> Option<u64> {
        self.conf["avg_blocktime"].as_u64()
    }
}

/// 'txfee' coins param config values
pub(crate) enum UtxoFeeConfig {
    NotSet,
    Dynamic,
    FixedPerKb(u64),
    FixedPerKbDingo(u64),
}

impl UtxoFeeConfig {
    /// Parse the txfee-related coins param, like:
    /// "txfee"=0 and/or "dingo_fee"=true
    pub(crate) fn parse_val(conf: &Json) -> Self {
        match (conf["txfee"].as_u64(), conf["dingo_fee"].as_bool()) {
            (Some(0), _) => Self::Dynamic,
            (Some(val), Some(true)) => Self::FixedPerKbDingo(val),
            (Some(val), _) => Self::FixedPerKb(val),
            (_, _) => Self::NotSet,
        }
    }
}
