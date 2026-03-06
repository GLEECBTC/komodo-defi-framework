use anyhow::{anyhow, Result};
use bip39::{Language, Mnemonic, MnemonicType};
use common::log::{error, info};
use common::password_policy;
use inquire::{validator::Validation, Confirm, CustomType, CustomUserError, Text};
use passwords::PasswordGenerator;
use serde::Serialize;
use std::net::Ipv4Addr;
use std::ops::Not;
use std::path::Path;

use super::inquire_extentions::{InquireOption, DEFAULT_DEFAULT_OPTION_BOOL_FORMATTER, DEFAULT_OPTION_BOOL_FORMATTER,
                                OPTION_BOOL_PARSER};
use crate::helpers;
use crate::logging::error_anyhow;

const DEFAULT_NET_ID: u16 = 6133;
const DEFAULT_GID: &str = "adex-cli";
const DEFAULT_OPTION_PLACEHOLDER: &str = "Tap enter to skip";
const RPC_PORT_MIN: u16 = 1024;
const RPC_PORT_MAX: u16 = 49151;

pub(crate) fn init_mm2_cfg(cfg_file: &str) -> Result<()> {
    let mut mm2_cfg = Mm2Cfg::new();
    info!("Start collecting mm2_cfg into: {cfg_file}");
    mm2_cfg.inquire()?;
    helpers::rewrite_json_file(&mm2_cfg, cfg_file)?;
    info!("mm2_cfg has been writen into: {cfg_file}");

    Ok(())
}

#[derive(Serialize)]
struct Mm2Cfg {
    gui: Option<String>,
    netid: Option<u16>,
    rpc_password: Option<String>,
    #[serde(rename = "passphrase", skip_serializing_if = "Option::is_none")]
    seed_phrase: Option<String>,
    allow_weak_password: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dbdir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rpcip: Option<Ipv4Addr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rpcport: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rpc_local_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    i_am_seed: Option<bool>,
    #[serde(skip_serializing_if = "Vec::<Ipv4Addr>::is_empty")]
    seednodes: Vec<Ipv4Addr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_hd: Option<bool>,
}

impl Mm2Cfg {
    fn new() -> Mm2Cfg {
        Mm2Cfg {
            gui: None,
            netid: None,
            rpc_password: None,
            seed_phrase: None,
            allow_weak_password: None,
            dbdir: None,
            rpcip: None,
            rpcport: None,
            rpc_local_only: None,
            i_am_seed: None,
            seednodes: Vec::<Ipv4Addr>::new(),
            enable_hd: None,
        }
    }

    fn inquire(&mut self) -> Result<()> {
        self.inquire_gui()?;
        self.inquire_net_id()?;
        self.inquire_seed_phrase()?;
        self.inquire_allow_weak_password()?;
        self.inquire_rpc_password()?;
        self.inquire_dbdir()?;
        self.inquire_rpcip()?;
        self.inquire_rpcport()?;
        self.inquire_rpc_local_only()?;
        self.inquire_i_am_a_seed()?;
        self.inquire_seednodes()?;
        self.inquire_enable_hd()?;
        Ok(())
    }

    #[inline]
    fn inquire_dbdir(&mut self) -> Result<()> {
        let is_reachable_dir = |dbdir: &InquireOption<String>| -> Result<Validation, CustomUserError> {
            match dbdir {
                InquireOption::None => Ok(Validation::Valid),
                InquireOption::Some(dbdir) => {
                    let path = Path::new(dbdir);
                    if path.is_dir().not() {
                        return Ok(Validation::Invalid(
                            format!("\"{dbdir}\" - is not a directory or does not exist").into(),
                        ));
                    }
                    Ok(Validation::Valid)
                },
            }
        };

        self.dbdir = CustomType::<InquireOption<String>>::new("What is dbdir")
                .with_placeholder(DEFAULT_OPTION_PLACEHOLDER)
                .with_help_message("Komodo DeFi Framework database path. Optional, defaults to a subfolder named DB in the path of your mm2 binary")
                .with_validator(is_reachable_dir)
                .prompt()
                .map_err(|error|
                    error_anyhow!("Failed to get dbdir: {error}")
                )?.into();

        Ok(())
    }

    #[inline]
    fn inquire_gui(&mut self) -> Result<()> {
        self.gui = Some(DEFAULT_GID.into());
        info!("> gui is set by default: {DEFAULT_GID}");
        Ok(())
    }

    #[inline]
    fn inquire_net_id(&mut self) -> Result<()> {
        self.netid = CustomType::<u16>::new("What is the network `mm2` is going to be a part, netid:")
                .with_default(DEFAULT_NET_ID)
                .with_help_message(r#"Network ID number, telling the Komodo DeFi Framework which network to join. 6133 is the current main network, though alternative netids can be used for testing or "private" trades"#)
                .with_placeholder(format!("{DEFAULT_NET_ID}").as_str())
                .prompt()
                .map_err(|error|
                    error_anyhow!("Failed to get netid: {error}")
                )?.into();
        Ok(())
    }

    #[inline]
    fn inquire_seed_phrase(&mut self) -> Result<()> {
        let mnemonic = Mnemonic::new(MnemonicType::Words12, Language::English);
        let default_password: &str = mnemonic.phrase();
        self.seed_phrase = Text::new("What is the seed phrase:")
            .with_default(default_password)
            .with_validator(|phrase: &str| {
                if phrase == "none" {
                    return Ok(Validation::Valid);
                }
                match Mnemonic::validate(phrase, Language::English) {
                    Ok(_) => Ok(Validation::Valid),
                    Err(error) => Ok(Validation::Invalid(error.into())),
                }
            })
            .with_placeholder(default_password)
            .with_help_message(
                "Type \"none\" to leave it blank and use limited service\n\
                 Your passphrase; this is the source of each of your coins' private keys. KEEP IT SAFE!",
            )
            .prompt()
            .map_err(|error| error_anyhow!("Failed to get passphrase: {error}"))
            .map(|value| if "none" == value { None } else { Some(value) })?;

        Ok(())
    }

    #[inline]
    fn inquire_rpc_password(&mut self) -> Result<()> {
        let allow_weak_password = self.allow_weak_password;
        let validator = move |password: &str| {
            if let Some(false) = allow_weak_password {
                match password_policy::password_policy(password) {
                    Err(error) => Ok(Validation::Invalid(error.into())),
                    Ok(_) => Ok(Validation::Valid),
                }
            } else {
                Ok(Validation::Valid)
            }
        };
        let default_password = Self::generate_password()?;

        self.rpc_password = Text::new("What is the rpc_password:")
            .with_help_message("Your password for protected RPC methods (userpass)")
            .with_validator(validator)
            .with_default(default_password.as_str())
            .with_placeholder(default_password.as_str())
            .prompt()
            .map_err(|error| error_anyhow!("Failed to get rpc_password: {error}"))?
            .into();
        Ok(())
    }

    fn generate_password() -> Result<String> {
        let pg = PasswordGenerator {
            length: 8,
            numbers: true,
            lowercase_letters: true,
            uppercase_letters: true,
            symbols: true,
            spaces: false,
            exclude_similar_characters: true,
            strict: true,
        };
        let mut password = String::new();
        while password_policy::password_policy(&password).is_err() {
            password = pg
                .generate_one()
                .map_err(|error| error_anyhow!("Failed to generate password: {error}"))?;
        }
        Ok(password)
    }

    #[inline]
    fn inquire_allow_weak_password(&mut self) -> Result<()> {
        self.allow_weak_password = Confirm::new("Allow weak password:")
                .with_default(false)
                .with_placeholder("No")
                .with_help_message(r#"If true, will allow low entropy rpc_password. If false rpc_password must not have 3 of the same characters in a row, must be at least 8 characters long, must contain at least one of each of the following: numeric, uppercase, lowercase, special character (e.g. !#$*). It also can not contain the word "password", or the chars <, >, and &. Defaults to false."#)
                .prompt()
                .map_err(|error|
                    error_anyhow!("Failed to get allow_weak_password: {error}")
                )?
                .into();
        Ok(())
    }

    #[inline]
    fn inquire_rpcip(&mut self) -> Result<()> {
        self.rpcip = CustomType::<InquireOption<Ipv4Addr>>::new("What is rpcip:")
            .with_placeholder(DEFAULT_OPTION_PLACEHOLDER)
            .with_help_message("IP address to bind to for RPC server. Optional, defaults to 127.0.0.1")
            .prompt()
            .map_err(|error| error_anyhow!("Failed to get rpcip: {error}"))?
            .into();
        Ok(())
    }

    #[inline]
    fn inquire_rpcport(&mut self) -> Result<()> {
        let validator = |value: &InquireOption<u16>| -> Result<Validation, CustomUserError> {
            match value {
                InquireOption::None => Ok(Validation::Valid),
                InquireOption::Some(value) => {
                    if (RPC_PORT_MIN..RPC_PORT_MAX + 1).contains(value) {
                        Ok(Validation::Valid)
                    } else {
                        Ok(Validation::Invalid(
                            format!("rpc_port is out of range: [{RPC_PORT_MIN}, {RPC_PORT_MAX}]").into(),
                        ))
                    }
                },
            }
        };
        self.rpcport = CustomType::<InquireOption<u16>>::new("What is the rpcport:")
            .with_help_message(r#"Port to use for RPC communication. Optional, defaults to 7783"#)
            .with_validator(validator)
            .with_placeholder(DEFAULT_OPTION_PLACEHOLDER)
            .prompt()
            .map_err(|error| error_anyhow!("Failed to get rpcport: {error}"))?
            .into();
        Ok(())
    }

    #[inline]
    fn inquire_rpc_local_only(&mut self) -> Result<()> {
        self.rpc_local_only = CustomType::<InquireOption<bool>>::new("What is rpc_local_only:")
                .with_parser(OPTION_BOOL_PARSER)
                .with_formatter(DEFAULT_OPTION_BOOL_FORMATTER)
                .with_default_value_formatter(DEFAULT_DEFAULT_OPTION_BOOL_FORMATTER)
                .with_default(InquireOption::None)
                .with_help_message("If false the Komodo DeFi Framework will allow rpc methods sent from external IP addresses. Optional, defaults to true. Warning: Only use this if you know what you are doing, and have put the appropriate security measures in place.")
                .prompt()
                .map_err(|error|
                    error_anyhow!("Failed to get rpc_local_only: {error}")
                )?.into();
        Ok(())
    }

    #[inline]
    fn inquire_i_am_a_seed(&mut self) -> Result<()> {
        self.i_am_seed = CustomType::<InquireOption<bool>>::new("What is i_am_a_seed:")
                .with_parser(OPTION_BOOL_PARSER)
                .with_formatter(DEFAULT_OPTION_BOOL_FORMATTER)
                .with_default_value_formatter(DEFAULT_DEFAULT_OPTION_BOOL_FORMATTER)
                .with_default(InquireOption::None)
                .with_help_message("Runs Komodo DeFi Framework as a seed node mode (acting as a relay for Komodo DeFi Framework clients). Optional, defaults to false. Use of this mode is not reccomended on the main network (6133) as it could result in a pubkey ban if non-compliant. on alternative testing or private networks, at least one seed node is required to relay information to other Komodo DeFi Framework clients using the same netID.")
                .prompt()
                .map_err(|error|
                    error_anyhow!("Failed to get i_am_a_seed: {error}")
                )?.into();
        Ok(())
    }

    #[inline]
    fn inquire_seednodes(&mut self) -> Result<()> {
        info!("Reading seed nodes until tap enter is met");
        loop {
            let seednode: Option<Ipv4Addr> = CustomType::<InquireOption<Ipv4Addr>>::new("What is the next seednode:")
                  .with_help_message("Optional. If operating on a test or private netID, the IP address of at least one seed node is required (on the main network, these are already hardcoded)")
                  .with_placeholder(DEFAULT_OPTION_PLACEHOLDER)
                  .prompt()
                  .map_err(|error|
                      error_anyhow!("Failed to get seed node: {error}")
                  )?.into();
            let Some(seednode) = seednode else {
                break;
            };
            self.seednodes.push(seednode);
        }
        Ok(())
    }

    #[inline]
    fn inquire_enable_hd(&mut self) -> Result<()> {
        self.enable_hd = CustomType::<InquireOption<bool>>::new("What is enable_hd:")
                .with_parser(OPTION_BOOL_PARSER)
                .with_formatter(DEFAULT_OPTION_BOOL_FORMATTER)
                .with_default_value_formatter(DEFAULT_DEFAULT_OPTION_BOOL_FORMATTER)
                .with_default(InquireOption::None)
                .with_help_message(r#"Optional. If this value is set, the Komodo DeFi API will work in HD wallet mode only, coins will need to have a coin derivation path entry in the coins file for activation. path_to_address `/account'/change/address_index` will have to be set in coins activation to change the default HD wallet address that is used in swaps for a coin in the full derivation path as follows: m/purpose'/coin_type/account'/change/address_index"#)
                .prompt()
                .map_err(|error|
                    error_anyhow!("Failed to get enable_hd: {}", error)
                )?
                .into();
        Ok(())
    }
}
