//! Helpers used in the unit and integration tests.

#![allow(missing_docs)]

use crate::electrums::tqtum_electrums;
use crate::structs::*;
use common::custom_futures::repeatable::{Ready, Retry};
use common::executor::Timer;
use common::log::{debug, info};
use common::{cfg_native, now_float, now_ms, now_sec, repeatable, wait_until_ms, wait_until_sec, PagingOptionsEnum};
use common::{get_utc_timestamp, log};
use crypto::CryptoCtx;
use gstuff::{try_s, ERR, ERRL};
use http::{HeaderMap, StatusCode};
use lazy_static::lazy_static;
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_metrics::{MetricType, MetricsJson};
use mm2_number::BigDecimal;
use mm2_rpc::data::legacy::{BalanceResponse, ElectrumProtocol};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{self as json, json, Value as Json};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::env;
use std::net::IpAddr;
use std::num::NonZeroUsize;
use std::process::Child;
use std::sync::Mutex;
use uuid::Uuid;

cfg_native! {
    use common::block_on;
    use common::log::dashboard_path;
    use mm2_io::fs::slurp;
    use mm2_net::transport::slurp_req;
    use common::wio::POOL;
    use chrono::{Local, TimeZone};
    use bytes::Bytes;
    use futures::channel::oneshot;
    use futures::task::SpawnExt;
    use http::Request;
    use regex::Regex;
    use std::fs;
    use std::io::Write;
    use std::net::Ipv4Addr;
    use std::path::{Path, PathBuf};
    use std::process::Command;
}

pub const MAKER_SUCCESS_EVENTS: [&str; 12] = [
    "Started",
    "Negotiated",
    "MakerPaymentInstructionsReceived",
    "TakerFeeValidated",
    "MakerPaymentSent",
    "TakerPaymentReceived",
    "TakerPaymentWaitConfirmStarted",
    "TakerPaymentValidatedAndConfirmed",
    "TakerPaymentSpent",
    "TakerPaymentSpendConfirmStarted",
    "TakerPaymentSpendConfirmed",
    "Finished",
];

pub const MAKER_ERROR_EVENTS: [&str; 15] = [
    "StartFailed",
    "NegotiateFailed",
    "TakerFeeValidateFailed",
    "MakerPaymentTransactionFailed",
    "MakerPaymentDataSendFailed",
    "MakerPaymentWaitConfirmFailed",
    "TakerPaymentValidateFailed",
    "TakerPaymentWaitConfirmFailed",
    "TakerPaymentSpendFailed",
    "TakerPaymentSpendConfirmFailed",
    "MakerPaymentWaitRefundStarted",
    "MakerPaymentRefundStarted",
    "MakerPaymentRefunded",
    "MakerPaymentRefundFailed",
    "MakerPaymentRefundFinished",
];

pub const TAKER_SUCCESS_EVENTS: [&str; 12] = [
    "Started",
    "Negotiated",
    "TakerFeeSent",
    "TakerPaymentInstructionsReceived",
    "MakerPaymentReceived",
    "MakerPaymentWaitConfirmStarted",
    "MakerPaymentValidatedAndConfirmed",
    "TakerPaymentSent",
    "TakerPaymentSpent",
    "MakerPaymentSpent",
    "MakerPaymentSpendConfirmed",
    "Finished",
];

pub const TAKER_USING_WATCHERS_SUCCESS_EVENTS: [&str; 14] = [
    "Started",
    "Negotiated",
    "TakerFeeSent",
    "TakerPaymentInstructionsReceived",
    "MakerPaymentReceived",
    "MakerPaymentWaitConfirmStarted",
    "MakerPaymentValidatedAndConfirmed",
    "TakerPaymentSent",
    "WatcherMessageSent",
    "TakerPaymentSpent",
    "MakerPaymentSpent",
    "MakerPaymentSpentByWatcher",
    "MakerPaymentSpendConfirmed",
    "Finished",
];

// Taker using watchers and watcher spends maker payment
pub const TAKER_ACTUAL_EVENTS_WATCHER_SPENDS_MAKER_PAYMENT: [&str; 13] = [
    "Started",
    "Negotiated",
    "TakerFeeSent",
    "TakerPaymentInstructionsReceived",
    "MakerPaymentReceived",
    "MakerPaymentWaitConfirmStarted",
    "MakerPaymentValidatedAndConfirmed",
    "TakerPaymentSent",
    "WatcherMessageSent",
    "TakerPaymentSpent",
    "MakerPaymentSpentByWatcher",
    "MakerPaymentSpendConfirmed",
    "Finished",
];

// Taker using watchers and spends maker payment instead of watcher
pub const TAKER_ACTUAL_EVENTS_TAKER_SPENDS_MAKER_PAYMENT: [&str; 13] = [
    "Started",
    "Negotiated",
    "TakerFeeSent",
    "TakerPaymentInstructionsReceived",
    "MakerPaymentReceived",
    "MakerPaymentWaitConfirmStarted",
    "MakerPaymentValidatedAndConfirmed",
    "TakerPaymentSent",
    "WatcherMessageSent",
    "TakerPaymentSpent",
    "MakerPaymentSpent",
    "MakerPaymentSpendConfirmed",
    "Finished",
];

pub const TAKER_ERROR_EVENTS: [&str; 17] = [
    "StartFailed",
    "NegotiateFailed",
    "TakerFeeSendFailed",
    "MakerPaymentValidateFailed",
    "MakerPaymentWaitConfirmFailed",
    "TakerPaymentTransactionFailed",
    "TakerPaymentWaitConfirmFailed",
    "TakerPaymentDataSendFailed",
    "TakerPaymentWaitForSpendFailed",
    "MakerPaymentSpendFailed",
    "MakerPaymentSpendConfirmFailed",
    "TakerPaymentWaitRefundStarted",
    "TakerPaymentRefundStarted",
    "TakerPaymentRefunded",
    "TakerPaymentRefundedByWatcher",
    "TakerPaymentRefundFailed",
    "TakerPaymentRefundFinished",
];

pub const RICK: &str = "RICK";
pub const RICK_ELECTRUM_ADDRS: &[&str] = &[
    "electrum1.cipig.net:10017",
    "electrum2.cipig.net:10017",
    "electrum3.cipig.net:10017",
];
pub const MORTY: &str = "MORTY";
pub const MORTY_ELECTRUM_ADDRS: &[&str] = &[
    "electrum1.cipig.net:10018",
    "electrum2.cipig.net:10018",
    "electrum3.cipig.net:10018",
];
pub const DOC: &str = "DOC";
#[cfg(not(target_arch = "wasm32"))]
pub const DOC_ELECTRUM_ADDRS: &[&str] = &[
    "electrum1.cipig.net:10020",
    "electrum2.cipig.net:10020",
    "electrum3.cipig.net:10020",
];

/// NOTE: These are websocket servers.
#[cfg(target_arch = "wasm32")]
pub const DOC_ELECTRUM_ADDRS: &[&str] = &[
    "electrum1.cipig.net:30020",
    "electrum2.cipig.net:30020",
    "electrum3.cipig.net:30020",
];
pub const MARTY: &str = "MARTY";
pub const MARTY_ELECTRUM_ADDRS: &[&str] = &[
    "electrum1.cipig.net:10021",
    "electrum2.cipig.net:10021",
    "electrum3.cipig.net:10021",
];
pub const ZOMBIE_TICKER: &str = "ZOMBIE";
#[cfg(not(target_arch = "wasm32"))]
pub const ZOMBIE_ELECTRUMS: &[&str] = &["zombie.dragonhound.info:10033", "zombie.dragonhound.info:10133"];
#[cfg(target_arch = "wasm32")]
pub const ZOMBIE_ELECTRUMS: &[&str] = &["zombie.dragonhound.info:30058", "zombie.dragonhound.info:30059"];
pub const ZOMBIE_LIGHTWALLETD_URLS: &[&str] = &[
    "https://zombie.dragonhound.info:443",
    "https://zombie.dragonhound.info:1443",
];
pub const ARRR: &str = "ARRR";
#[cfg(not(target_arch = "wasm32"))]
pub const PIRATE_ELECTRUMS: &[&str] = &[
    "electrum1.cipig.net:10008",
    "electrum2.cipig.net:10008",
    "electrum3.cipig.net:10008",
];
#[cfg(target_arch = "wasm32")]
pub const PIRATE_ELECTRUMS: &[&str] = &[
    "electrum3.cipig.net:30008",
    "electrum1.cipig.net:30008",
    "electrum2.cipig.net:30008",
];
#[cfg(not(target_arch = "wasm32"))]
pub const PIRATE_LIGHTWALLETD_URLS: &[&str] = &[
    "https://lightd1.pirate.black:443",
    "https://piratelightd1.cryptoforge.cc:443",
    "https://piratelightd2.cryptoforge.cc:443",
    "https://piratelightd3.cryptoforge.cc:443",
    "https://piratelightd4.cryptoforge.cc:443",
];
#[cfg(target_arch = "wasm32")]
pub const PIRATE_LIGHTWALLETD_URLS: &[&str] = &["https://pirate.battlefield.earth:8581"];
pub const DEFAULT_RPC_PASSWORD: &str = "pass";
pub const QRC20_ELECTRUMS: &[&str] = &[
    "electrum1.cipig.net:10071",
    "electrum2.cipig.net:10071",
    "electrum3.cipig.net:10071",
];
pub const T_BCH_ELECTRUMS: &[&str] = &["tbch.loping.net:60001", "bch0.kister.net:51001"];
pub const TBTC_ELECTRUMS: &[&str] = &["electrum3.cipig.net:10068", "testnet.aranguren.org:51001"];

pub const ETH_MAINNET_NODES: &[&str] = &[
    "https://mainnet.infura.io/v3/c01c1b4cf66642528547624e1d6d9d6b",
    "https://ethereum-rpc.publicnode.com",
    "https://eth.drpc.org",
];
pub const ETH_MAINNET_CHAIN_ID: u64 = 1;
pub const ETH_MAINNET_SWAP_CONTRACT: &str = "0x24abe4c71fc658c91313b6552cd40cd808b3ea80";

pub const ETH_SEPOLIA_NODES: &[&str] = &[
    "https://sepolia.drpc.org",
    "https://ethereum-sepolia-rpc.publicnode.com",
    "https://rpc2.sepolia.org",
    "https://1rpc.io/sepolia",
    "https://sepolia.drpc.org",
];
pub const ETH_SEPOLIA_CHAIN_ID: u64 = 11155111;
pub const ETH_SEPOLIA_SWAP_CONTRACT: &str = "0xeA6D65434A15377081495a9E7C5893543E7c32cB";
pub const ETH_SEPOLIA_TOKEN_CONTRACT: &str = "0x09d0d71FBC00D7CCF9CFf132f5E6825C88293F19";

pub const BCHD_TESTNET_URLS: &[&str] = &["https://bchd-testnet.greyh.at:18335"];

pub struct Mm2TestConf {
    pub conf: Json,
    pub rpc_password: String,
}

impl Mm2TestConf {
    pub fn seednode(passphrase: &str, coins: &Json) -> Self {
        Mm2TestConf {
            conf: json!({
                "gui": "nogui",
                "netid": 9998,
                "passphrase": passphrase,
                "coins": coins,
                "rpc_password": DEFAULT_RPC_PASSWORD,
                "i_am_seed": true,
                "is_bootstrap_node": true
            }),
            rpc_password: DEFAULT_RPC_PASSWORD.into(),
        }
    }

    /// Generates a seed node conf enabling use_trading_proto_v2
    pub fn seednode_trade_v2(passphrase: &str, coins: &Json) -> Self {
        Mm2TestConf {
            conf: json!({
                "gui": "nogui",
                "netid": 9998,
                "passphrase": passphrase,
                "coins": coins,
                "rpc_password": DEFAULT_RPC_PASSWORD,
                "i_am_seed": true,
                "use_trading_proto_v2": true,
                "is_bootstrap_node": true
            }),
            rpc_password: DEFAULT_RPC_PASSWORD.into(),
        }
    }

    pub fn seednode_with_hd_account(passphrase: &str, coins: &Json) -> Self {
        Mm2TestConf {
            conf: json!({
                "gui": "nogui",
                "netid": 9998,
                "passphrase": passphrase,
                "coins": coins,
                "rpc_password": DEFAULT_RPC_PASSWORD,
                "i_am_seed": true,
                "enable_hd": true,
                "is_bootstrap_node": true
            }),
            rpc_password: DEFAULT_RPC_PASSWORD.into(),
        }
    }

    pub fn seednode_with_hd_account_trade_v2(passphrase: &str, coins: &Json) -> Self {
        Mm2TestConf {
            conf: json!({
                "gui": "nogui",
                "netid": 9998,
                "passphrase": passphrase,
                "coins": coins,
                "rpc_password": DEFAULT_RPC_PASSWORD,
                "i_am_seed": true,
                "enable_hd": true,
                "use_trading_proto_v2": true,
                "is_bootstrap_node": true
            }),
            rpc_password: DEFAULT_RPC_PASSWORD.into(),
        }
    }

    pub fn seednode_with_wallet_name(coins: &Json, wallet_name: &str, wallet_password: &str) -> Self {
        Mm2TestConf {
            conf: json!({
                "gui": "nogui",
                "netid": 9998,
                "coins": coins,
                "rpc_password": DEFAULT_RPC_PASSWORD,
                "i_am_seed": true,
                "wallet_name": wallet_name,
                "wallet_password": wallet_password,
                "is_bootstrap_node": true
            }),
            rpc_password: DEFAULT_RPC_PASSWORD.into(),
        }
    }

    pub fn light_node(passphrase: &str, coins: &Json, seednodes: &[&str]) -> Self {
        Mm2TestConf {
            conf: json!({
                "gui": "nogui",
                "netid": 9998,
                "passphrase": passphrase,
                "coins": coins,
                "rpc_password": DEFAULT_RPC_PASSWORD,
                "seednodes": seednodes
            }),
            rpc_password: DEFAULT_RPC_PASSWORD.into(),
        }
    }

    /// Generates a light node conf enabling use_trading_proto_v2
    pub fn light_node_trade_v2(passphrase: &str, coins: &Json, seednodes: &[&str]) -> Self {
        Mm2TestConf {
            conf: json!({
                "gui": "nogui",
                "netid": 9998,
                "passphrase": passphrase,
                "coins": coins,
                "rpc_password": DEFAULT_RPC_PASSWORD,
                "seednodes": seednodes,
                "use_trading_proto_v2": true,
            }),
            rpc_password: DEFAULT_RPC_PASSWORD.into(),
        }
    }

    pub fn watcher_light_node(passphrase: &str, coins: &Json, seednodes: &[&str], conf: WatcherConf) -> Self {
        Mm2TestConf {
            conf: json!({
                "gui": "nogui",
                "netid": 9998,
                "passphrase": passphrase,
                "coins": coins,
                "rpc_password": DEFAULT_RPC_PASSWORD,
                "seednodes": seednodes,
                "is_watcher": true,
                "watcher_conf": conf
            }),
            rpc_password: DEFAULT_RPC_PASSWORD.into(),
        }
    }

    pub fn light_node_with_hd_account(passphrase: &str, coins: &Json, seednodes: &[&str]) -> Self {
        Mm2TestConf {
            conf: json!({
                "gui": "nogui",
                "netid": 9998,
                "passphrase": passphrase,
                "coins": coins,
                "rpc_password": DEFAULT_RPC_PASSWORD,
                "seednodes": seednodes,
                "enable_hd": true,
            }),
            rpc_password: DEFAULT_RPC_PASSWORD.into(),
        }
    }

    pub fn light_node_with_hd_account_trade_v2(passphrase: &str, coins: &Json, seednodes: &[&str]) -> Self {
        Mm2TestConf {
            conf: json!({
                "gui": "nogui",
                "netid": 9998,
                "passphrase": passphrase,
                "coins": coins,
                "rpc_password": DEFAULT_RPC_PASSWORD,
                "seednodes": seednodes,
                "enable_hd": true,
                "use_trading_proto_v2": true,
            }),
            rpc_password: DEFAULT_RPC_PASSWORD.into(),
        }
    }

    pub fn no_login_node(coins: &Json, seednodes: &[&str]) -> Self {
        assert!(
            !seednodes.is_empty(),
            "Invalid Test Setup: A no-login node requires at least one seednode."
        );

        Mm2TestConf {
            conf: json!({
                "gui": "nogui",
                "netid": 9998,
                "coins": coins,
                "rpc_password": DEFAULT_RPC_PASSWORD,
                "seednodes": seednodes,
            }),
            rpc_password: DEFAULT_RPC_PASSWORD.into(),
        }
    }
}

pub struct Mm2TestConfForSwap;

impl Mm2TestConfForSwap {
    /// TODO consider moving it to read it from a env file.
    pub const BOB_HD_PASSPHRASE: &'static str =
        "involve work eager scene give acoustic tooth mimic dance smoke hold foster";
    /// TODO consider moving it to read it from a env file.
    pub const ALICE_HD_PASSPHRASE: &'static str =
        "tank abandon bind salon remove wisdom net size aspect direct source fossil";

    pub fn bob_conf_with_policy(priv_key_policy: &Mm2InitPrivKeyPolicy, coins: &Json) -> Mm2TestConf {
        match priv_key_policy {
            Mm2InitPrivKeyPolicy::Iguana => {
                let bob_passphrase = crate::get_passphrase!(".env.seed", "BOB_PASSPHRASE").unwrap();
                Mm2TestConf::seednode(&bob_passphrase, coins)
            },
            Mm2InitPrivKeyPolicy::GlobalHDAccount => {
                Mm2TestConf::seednode_with_hd_account(Self::BOB_HD_PASSPHRASE, coins)
            },
        }
    }

    pub fn alice_conf_with_policy(priv_key_policy: &Mm2InitPrivKeyPolicy, coins: &Json, bob_ip: &str) -> Mm2TestConf {
        match priv_key_policy {
            Mm2InitPrivKeyPolicy::Iguana => {
                let alice_passphrase = crate::get_passphrase!(".env.client", "ALICE_PASSPHRASE").unwrap();
                Mm2TestConf::light_node(&alice_passphrase, coins, &[bob_ip])
            },
            Mm2InitPrivKeyPolicy::GlobalHDAccount => {
                Mm2TestConf::light_node_with_hd_account(Self::ALICE_HD_PASSPHRASE, coins, &[bob_ip])
            },
        }
    }
}

pub enum Mm2InitPrivKeyPolicy {
    Iguana,
    GlobalHDAccount,
}

pub fn zombie_conf() -> Json {
    zombie_conf_inner(None, 0)
}

pub fn zombie_conf_for_docker() -> Json {
    zombie_conf_inner(Some(10), 1)
}

pub fn zombie_conf_inner(custom_blocktime: Option<u8>, required_confirmations: u8) -> Json {
    json!({
        "coin":"ZOMBIE",
        "asset":"ZOMBIE",
        "txversion":4,
        "overwintered": 1,
        "mm2":1,
        "avg_blocktime": custom_blocktime.unwrap_or(60),
        "protocol":{
            "type":"ZHTLC",
            "protocol_data": {
                "consensus_params": {
                    "overwinter_activation_height": 0,
                    "sapling_activation_height": 1,
                    "blossom_activation_height": null,
                    "heartwood_activation_height": null,
                    "canopy_activation_height": null,
                    "coin_type": 133,
                    "hrp_sapling_extended_spending_key": "secret-extended-key-main",
                    "hrp_sapling_extended_full_viewing_key": "zxviews",
                    "hrp_sapling_payment_address": "zs",
                    "b58_pubkey_address_prefix": [ 28, 184 ],
                    "b58_script_address_prefix": [ 28, 189 ]
                },
                "z_derivation_path": "m/32'/133'",
            }
        },
        "required_confirmations": required_confirmations,
        "derivation_path": "m/44'/133'",
    })
}

pub fn pirate_conf() -> Json {
    json!({
        "coin":"ARRR",
        "asset":"PIRATE",
        "txversion":4,
        "overwintered":1,
        "mm2":1,
        "avg_blocktime": 60,
        "protocol":{
            "type":"ZHTLC",
            "protocol_data": {
                "consensus_params": {
                    "overwinter_activation_height": 152855,
                    "sapling_activation_height": 152855,
                    "blossom_activation_height": null,
                    "heartwood_activation_height": null,
                    "canopy_activation_height": null,
                    "coin_type": 133,
                    "hrp_sapling_extended_spending_key": "secret-extended-key-main",
                    "hrp_sapling_extended_full_viewing_key": "zxviews",
                    "hrp_sapling_payment_address": "zs",
                    "b58_pubkey_address_prefix": [ 28, 184 ],
                    "b58_script_address_prefix": [ 28, 189 ]
                },
                "z_derivation_path": "m/32'/133'",
            }
        },
        "required_confirmations":0,
        "derivation_path": "m/44'/133'",
    })
}

pub fn rick_conf() -> Json {
    json!({
        "coin":"RICK",
        "asset":"RICK",
        "required_confirmations":0,
        "txversion":4,
        "overwintered":1,
        "derivation_path": "m/44'/141'",
        "sign_message_prefix": "Komodo Signed Message:\n",
        "protocol":{
            "type":"UTXO"
        }
    })
}

pub fn doc_conf() -> Json {
    json!({
        "coin":"DOC",
        "asset":"DOC",
        "required_confirmations":0,
        "txversion":4,
        "overwintered":1,
        "derivation_path": "m/44'/141'",
        "protocol":{
            "type":"UTXO"
        }
    })
}

pub fn morty_conf() -> Json {
    json!({
        "coin":"MORTY",
        "asset":"MORTY",
        "required_confirmations":0,
        "txversion":4,
        "overwintered":1,
        "derivation_path": "m/44'/141'",
        "protocol":{
            "type":"UTXO"
        }
    })
}

pub fn kmd_conf(tx_fee: u64) -> Json {
    json!({
        "coin":"KMD",
        "txversion":4,
        "overwintered":1,
        "txfee":tx_fee,
        "protocol":{
            "type":"UTXO"
        }
    })
}

pub fn mycoin_conf(tx_fee: u64) -> Json {
    json!({
        "coin":"MYCOIN",
        "asset":"MYCOIN",
        "txversion":4,
        "overwintered":1,
        "txfee":tx_fee,
        "protocol":{
            "type":"UTXO"
        }
    })
}

pub fn mycoin1_conf(tx_fee: u64) -> Json {
    json!({
        "coin":"MYCOIN1",
        "asset":"MYCOIN1",
        "txversion":4,
        "overwintered":1,
        "txfee":tx_fee,
        "protocol":{
            "type":"UTXO"
        }
    })
}

pub fn atom_testnet_conf() -> Json {
    json!({
        "coin":"ATOM",
        "avg_blocktime": 5,
        "protocol":{
            "type":"TENDERMINT",
            "protocol_data": {
                "decimals": 6,
                "denom": "uatom",
                "account_prefix": "cosmos",
                "chain_id": "cosmoshub-testnet",
            },
        },
        "derivation_path": "m/44'/118'",
    })
}

pub fn btc_segwit_conf() -> Json {
    json!({
        "coin": "BTC-segwit",
        "name": "bitcoin",
        "fname": "Bitcoin",
        "rpcport": 8332,
        "pubtype": 0,
        "p2shtype": 5,
        "wiftype": 128,
        "segwit": true,
        "bech32_hrp": "bc",
        "address_format": {
            "format": "segwit"
        },
        "orderbook_ticker": "BTC",
        "txfee": 0,
        "estimate_fee_mode": "ECONOMICAL",
        "mm2": 1,
        "required_confirmations": 1,
        "avg_blocktime": 10,
        "derivation_path": "m/84'/0'",
        "protocol": {
            "type": "UTXO"
        }
    })
}

pub fn btc_with_spv_conf() -> Json {
    json!({
        "coin": "BTC",
        "asset":"BTC",
        "pubtype": 0,
        "p2shtype": 5,
        "wiftype": 128,
        "segwit": true,
        "bech32_hrp": "bc",
        "txfee": 0,
        "estimate_fee_mode": "ECONOMICAL",
        "required_confirmations": 0,
        "protocol": {
            "type": "UTXO"
        },
        "spv_conf": {
            "starting_block_header": {
                "height": 0,
                "hash": "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f",
                "bits": 486604799,
                "time": 1231006505,
            },
            "validation_params": {
                "difficulty_check": true,
                "constant_difficulty": false,
                "difficulty_algorithm": "Bitcoin Mainnet"
            }
        }
    })
}

pub fn btc_with_sync_starting_header() -> Json {
    json!({
        "coin": "BTC",
        "asset":"BTC",
        "pubtype": 0,
        "p2shtype": 5,
        "wiftype": 128,
        "segwit": true,
        "bech32_hrp": "bc",
        "txfee": 0,
        "estimate_fee_mode": "ECONOMICAL",
        "required_confirmations": 0,
        "protocol": {
            "type": "UTXO"
        },
        "spv_conf": {
            "starting_block_header": {
                "height": 764064,
                "hash": "00000000000000000006da48b920343944908861fa05b28824922d9e60aaa94d",
                "bits": 386375189,
                "time": 1668986059,
            },
            "max_stored_block_headers": 3000,
            "validation_params": {
                "difficulty_check": true,
                "constant_difficulty": false,
                "difficulty_algorithm": "Bitcoin Mainnet"
            }
        }
    })
}

pub fn tbtc_conf() -> Json {
    json!({
        "coin": "tBTC",
        "asset":"tBTC",
        "pubtype": 111,
        "p2shtype": 196,
        "wiftype": 239,
        "segwit": true,
        "bech32_hrp": "tb",
        "txfee": 1000,
        "required_confirmations": 0,
        "protocol": {
            "type": "UTXO"
        }
    })
}

pub fn tbtc_segwit_conf() -> Json {
    json!({
        "coin": "tBTC-Segwit",
        "asset":"tBTC-Segwit",
        "pubtype": 111,
        "p2shtype": 196,
        "wiftype": 239,
        "segwit": true,
        "bech32_hrp": "tb",
        "txfee": 1000,
        "required_confirmations": 0,
        "derivation_path": "m/84'/1'",
        "address_format": {
            "format": "segwit"
        },
        "protocol": {
            "type": "UTXO"
        },
        "orderbook_ticker": "tBTC",
    })
}

pub fn tbtc_with_spv_conf() -> Json {
    json!({
        "coin": "tBTC-TEST",
        "asset":"tBTC-TEST",
        "pubtype": 111,
        "p2shtype": 196,
        "wiftype": 239,
        "segwit": true,
        "bech32_hrp": "tb",
        "txfee": 0,
        "estimate_fee_mode": "ECONOMICAL",
        "required_confirmations": 0,
        "enable_spv_proof": true,
        "protocol": {
            "type": "UTXO"
        },
        "spv_conf": {
            "starting_block_header": {
                "height": 0,
                "hash": "000000000933ea01ad0ee984209779baaec3ced90fa3f408719526f8d77f4943",
                "bits": 486604799,
                "time": 1296688602,
            },
            "validation_params": {
                "difficulty_check": true,
                "constant_difficulty": false,
                "difficulty_algorithm": "Bitcoin Testnet"
            }
        }
    })
}

pub fn tbtc_legacy_conf() -> Json {
    json!({
        "coin": "tBTC",
        "asset":"tBTC",
        "pubtype": 111,
        "p2shtype": 196,
        "wiftype": 239,
        "segwit": false,
        "bech32_hrp": "tb",
        "txfee": 0,
        "estimate_fee_mode": "ECONOMICAL",
        "required_confirmations": 0,
        "protocol": {
            "type": "UTXO"
        }
    })
}

pub fn eth_testnet_conf_trezor() -> Json {
    json!({
        "coin": "ETH",
        "name": "ethereum",
        "mm2": 1,
        "max_eth_tx_type": 2,
        "derivation_path": "m/44'/1'", // Trezor uses coin type 1 for testnet
        "protocol": {
            "type": "ETH",
            "protocol_data": {
                "chain_id": 1337,
            }
        },
        "trezor_coin": "Ethereum"
    })
}

/// ETH configuration used for dockerized Geth dev node
pub fn eth_dev_conf() -> Json {
    eth_conf("ETH")
}

pub fn eth1_dev_conf() -> Json {
    eth_conf("ETH1")
}

fn eth_conf(coin: &str) -> Json {
    json!({
        "coin": coin,
        "name": "ethereum",
        "mm2": 1,
        "derivation_path": "m/44'/60'",
        "protocol": {
            "type": "ETH",
            "protocol_data": {
                "chain_id": 1337,
            }
        },
        "max_eth_tx_type": 2
    })
}

/// ERC20 token configuration used for dockerized Geth dev node
pub fn erc20_dev_conf(contract_address: &str) -> Json {
    json!({
        "coin": "ERC20DEV",
        "name": "erc20dev",
        "mm2": 1,
        "derivation_path": "m/44'/60'",
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH",
                "contract_address": contract_address,
            }
        },
        "max_eth_tx_type": 2
    })
}

/// ERC20 token configuration used for dockerized tests on Sepolia
pub fn sepolia_erc20_dev_conf(contract_address: &str) -> Json {
    let mut conf = erc20_dev_conf(contract_address);
    set_chain_id(&mut conf, ETH_SEPOLIA_CHAIN_ID);
    conf
}

/// global NFT configuration used for dockerized Geth dev node
pub fn nft_dev_conf() -> Json {
    json!({
        "coin": "NFT_ETH",
        "name": "nftdev",
        "mm2": 1,
        "derivation_path": "m/44'/60'",
        "protocol": {
            "type": "NFT",
            "protocol_data": {
                "platform": "ETH"
            }
        },
        "max_eth_tx_type": 2
    })
}

fn set_chain_id(conf: &mut Json, chain_id: u64) {
    conf["chain_id"] = json!(chain_id);
}

pub fn eth_sepolia_conf() -> Json {
    json!({
        "coin": "ETH",
        "name": "ethereum",
        "derivation_path": "m/44'/60'",
        "protocol": {
            "type": "ETH",
            "protocol_data": {
                "chain_id": ETH_SEPOLIA_CHAIN_ID,
            }
        },
        "max_eth_tx_type": 2,
        "trezor_coin": "Ethereum"
    })
}

pub fn eth_sepolia_trezor_firmware_compat_conf() -> Json {
    json!({
        "coin": "tETH",
        "name": "ethereum",
        "derivation_path": "m/44'/1'", // Note: trezor uses coin type 1' for eth for testnet (SLIP44_TESTNET)
        "protocol": {
            "type": "ETH",
            "protocol_data": {
                "chain_id": ETH_SEPOLIA_CHAIN_ID,
            }
        },
        "max_eth_tx_type": 2,
        "trezor_coin": "tETH"
    })
}

pub fn eth_jst_testnet_conf() -> Json {
    json!({
        "coin": "JST",
        "name": "jst",
        "derivation_path": "m/44'/60'",
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH",
                "contract_address": ETH_SEPOLIA_TOKEN_CONTRACT
            }
        },
        "max_eth_tx_type": 2
    })
}

pub fn jst_sepolia_conf() -> Json {
    json!({
        "coin": "JST",
        "name": "jst",
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH",
                "contract_address": ETH_SEPOLIA_TOKEN_CONTRACT
            }
        },
        "max_eth_tx_type": 2
    })
}

pub fn jst_sepolia_trezor_conf() -> Json {
    json!({
        "coin": "tJST",
        "name": "tjst",
        "derivation_path": "m/44'/1'", // Note: Trezor uses 1' coin type for all testnets
        "trezor_coin": "tETH",
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH",
                "contract_address": ETH_SEPOLIA_TOKEN_CONTRACT
            }
        }
    })
}

pub fn iris_testnet_conf() -> Json {
    json!({
        "coin": "IRIS-TEST",
        "avg_blocktime": 5,
        "derivation_path": "m/44'/566'",
        "protocol":{
            "type":"TENDERMINT",
            "protocol_data": {
                "decimals": 6,
                "denom": "unyan",
                "account_prefix": "iaa",
                "chain_id": "nyancat-9",
            },
        }
    })
}

pub fn nucleus_testnet_conf() -> Json {
    json!({
        "coin": "NUCLEUS-TEST",
        "avg_blocktime": 5,
        "derivation_path": "m/44'/566'",
        "protocol":{
            "type":"TENDERMINT",
            "protocol_data": {
                "decimals": 6,
                "denom": "unucl",
                "account_prefix": "nuc",
                "chain_id": "nucleus-testnet",
            },
        }
    })
}

pub fn iris_nimda_testnet_conf() -> Json {
    json!({
        "coin": "IRIS-NIMDA",
        "derivation_path": "m/44'/566'",
        "protocol": {
            "type": "TENDERMINTTOKEN",
            "protocol_data": {
                "platform": "IRIS-TEST",
                "decimals": 6,
                "denom": "nim",
            },
        }
    })
}

pub fn iris_ibc_nucleus_testnet_conf() -> Json {
    json!({
        "coin":"IRIS-IBC-NUCLEUS-TEST",
        "protocol":{
            "type":"TENDERMINTTOKEN",
            "protocol_data": {
                "platform": "NUCLEUS-TEST",
                "decimals": 6,
                "denom": "ibc/F7F28FF3C09024A0225EDBBDB207E5872D2B4EF2FB874FE47B05EF9C9A7D211C",
            },
        }
    })
}

pub fn usdc_ibc_iris_testnet_conf() -> Json {
    json!({
        "coin":"USDC-IBC-IRIS",
        "protocol":{
            "type":"TENDERMINTTOKEN",
            "protocol_data": {
                "platform": "IRIS-TEST",
                "decimals": 6,
                "denom": "ibc/5C465997B4F582F602CD64E12031C6A6E18CAF1E6EDC9B5D808822DC0B5F850C",
            },
        }
    })
}

/// `245` is SLP coin type within the derivation path.
pub fn tbch_for_slp_conf() -> Json {
    json!({
        "coin": "tBCH",
        "pubtype": 0,
        "p2shtype": 5,
        "mm2": 1,
        "derivation_path": "m/44'/245'",
        "protocol": {
            "type": "BCH",
            "protocol_data": {
                "slp_prefix": "slptest"
            }
        },
        "address_format": {
            "format": "cashaddress",
            "network": "bchtest"
        }
    })
}

pub fn tbch_usdf_conf() -> Json {
    json!({
        "coin": "USDF",
        "protocol": {
            "type": "SLPTOKEN",
            "protocol_data": {
                "decimals": 4,
                "token_id": "bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb7",
                "platform": "tBCH",
                "required_confirmations": 1
            }
        }
    })
}

pub fn tbnb_conf() -> Json {
    json!({
        "coin": "tBNB",
        "name": "binancesmartchaintest",
        "avg_blocktime": 0.25,
        "mm2": 1,
        "required_confirmations": 0,
        "protocol": {
            "type": "ETH",
            "protocol_data": {
                "chain_id": 97
            }
        }
    })
}

pub fn tqrc20_conf() -> Json {
    json!({
        "coin": "QRC20",
        "required_confirmations": 0,
        "pubtype": 120,
        "p2shtype": 50,
        "wiftype": 128,
        "txfee": 0,
        "mm2": 1,
        "mature_confirmations": 2000,
        "derivation_path": "m/44'/2301'",
        "protocol": {
            "type": "QRC20",
            "protocol_data": {
                "platform": "QTUM",
                "contract_address": "0xd362e096e873eb7907e205fadc6175c6fec7bc44"
            }
        }
    })
}

pub fn mm_ctx_with_iguana(passphrase: Option<&str>) -> MmArc {
    const DEFAULT_IGUANA_PASSPHRASE: &str = "123";

    let ctx = MmCtxBuilder::default().into_mm_arc();
    CryptoCtx::init_with_iguana_passphrase(ctx.clone(), passphrase.unwrap_or(DEFAULT_IGUANA_PASSPHRASE)).unwrap();
    ctx
}

#[cfg(target_arch = "wasm32")]
pub fn mm_ctx_with_custom_db() -> MmArc {
    MmCtxBuilder::new().with_test_db_namespace().into_mm_arc()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn mm_ctx_with_custom_db() -> MmArc {
    mm_ctx_with_custom_db_with_conf(None)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn mm_ctx_with_custom_db_with_conf(conf: Option<Json>) -> MmArc {
    use db_common::sqlite::rusqlite::Connection;
    use std::sync::Arc;

    let mut ctx_builder = MmCtxBuilder::new();
    if let Some(conf) = conf {
        ctx_builder = ctx_builder.with_conf(conf);
    }
    let ctx = ctx_builder.into_mm_arc();

    let connection = Connection::open_in_memory().unwrap();
    let _ = ctx
        .sqlite_connection
        .set(Arc::new(Mutex::new(connection)))
        .map_err(|_| "Already Initialized".to_string());

    let connection = Connection::open_in_memory().unwrap();
    let _ = ctx
        .shared_sqlite_conn
        .set(Arc::new(Mutex::new(connection)))
        .map_err(|_| "Already Initialized".to_string());

    ctx
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn mm_ctx_with_custom_async_db() -> MmArc {
    use db_common::async_sql_conn::AsyncConnection;
    use futures::lock::Mutex as AsyncMutex;
    use std::sync::Arc;

    let ctx = MmCtxBuilder::new().into_mm_arc();

    let connection = AsyncConnection::open_in_memory().await.unwrap();
    let _ = ctx
        .async_sqlite_connection
        .set(Arc::new(AsyncMutex::new(connection)))
        .map_err(|_| "Already Initialized".to_string());

    ctx
}

#[cfg(target_arch = "wasm32")]
pub async fn mm_ctx_with_custom_async_db() -> MmArc {
    MmCtxBuilder::new().with_test_db_namespace().into_mm_arc()
}

/// Automatically kill a wrapped process.
pub struct RaiiKill {
    pub handle: Child,
    running: bool,
}
impl RaiiKill {
    pub fn from_handle(handle: Child) -> RaiiKill {
        RaiiKill { handle, running: true }
    }
    pub fn running(&mut self) -> bool {
        if !self.running {
            return false;
        }
        match self.handle.try_wait() {
            Ok(None) => true,
            _ => {
                self.running = false;
                false
            },
        }
    }
}
impl Drop for RaiiKill {
    fn drop(&mut self) {
        // The cached `running` check might provide some protection against killing a wrong process under the same PID,
        // especially if the cached `running` check is also used to monitor the status of the process.
        if self.running() {
            let _ = self.handle.kill();
        }
    }
}

/// When `drop`ped, dumps the given file to the stdout.
///
/// Used in the tests, copying the MM log to the test output.
///
/// Note that because of https://github.com/rust-lang/rust/issues/42474 it's currently impossible to share the MM log interactively,
/// hence we're doing it in the `drop`.
pub struct RaiiDump {
    #[cfg(not(target_arch = "wasm32"))]
    pub log_path: PathBuf,
}
#[cfg(not(target_arch = "wasm32"))]
impl Drop for RaiiDump {
    fn drop(&mut self) {
        const DARK_YELLOW_ANSI_CODE: &str = "\x1b[33m";
        const YELLOW_ANSI_CODE: &str = "\x1b[93m";
        const RESET_COLOR_ANSI_CODE: &str = "\x1b[0m";

        // `term` bypasses the stdout capturing, we should only use it if the capturing was disabled.
        let nocapture = env::args().any(|a| a == "--nocapture");

        let log = slurp(&self.log_path).unwrap();

        // Make sure the log is Unicode.
        // We'll get the "io error when listing tests: Custom { kind: InvalidData, error: StringError("text was not valid unicode") }" otherwise.
        let log = String::from_utf8_lossy(&log);
        let log = log.trim();

        // If we want to determine is a tty or not here and write logs to stdout only if it's tty,
        // we can use something like https://docs.rs/atty/latest/atty/ here, look like it's more cross-platform than gstuff::ISATTY .

        if nocapture {
            std::io::stdout()
                .write_all(format!("{}vvv {:?} vvv\n", DARK_YELLOW_ANSI_CODE, self.log_path).as_bytes())
                .expect("Printing to stdout failed");
            std::io::stdout()
                .write_all(format!("{}{}{}\n", YELLOW_ANSI_CODE, log, RESET_COLOR_ANSI_CODE).as_bytes())
                .expect("Printing to stdout failed");
        } else {
            log!("vvv {:?} vvv\n{}", self.log_path, log);
        }
    }
}

lazy_static! {
    /// A singleton with the IPs used by the MarketMakerIt instances created in this session.
    /// The value is set to `false` when the instance is retired.
    static ref MM_IPS: Mutex<HashMap<IpAddr, bool>> = Mutex::new (HashMap::new());
}

#[cfg(not(target_arch = "wasm32"))]
pub type LocalStart = fn(PathBuf, PathBuf, Json);

#[cfg(target_arch = "wasm32")]
pub type LocalStart = fn(MmArc);

/// An instance of a MarketMaker process started by and for an integration test.
/// Given that [in CI] the tests are executed before the build, the binary of that process is the tests binary.
#[cfg(not(target_arch = "wasm32"))]
pub struct MarketMakerIt {
    /// The MarketMaker's current folder where it will try to open the database files.
    pub folder: PathBuf,
    /// Unique (to run multiple instances) IP, like "127.0.0.$x".
    pub ip: IpAddr,
    /// The file we redirected the standard output and error streams to.
    pub log_path: PathBuf,
    /// The PID of the MarketMaker process.
    pub pc: Option<RaiiKill>,
    /// RPC API key.
    pub userpass: String,
}

/// A MarketMaker instance started by and for an integration test.
#[cfg(target_arch = "wasm32")]
pub struct MarketMakerIt {
    pub ctx: mm2_core::mm_ctx::MmArc,
    /// RPC API key.
    pub userpass: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for MarketMakerIt {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "MarketMakerIt {{ folder: {:?}, ip: {}, log_path: {:?}, userpass: {} }}",
            self.folder, self.ip, self.log_path, self.userpass
        )
    }
}

impl MarketMakerIt {
    /// Start a new MarketMaker node without any specific environment variables.
    /// For more information see [`MarketMakerIt::start_with_envs`].
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start(conf: Json, userpass: String, local: Option<LocalStart>) -> Result<MarketMakerIt, String> {
        block_on(MarketMakerIt::start_with_envs(conf, userpass, local, &[]))
    }

    /// Start a new MarketMaker node asynchronously without any specific environment variables.
    /// For more information see [`MarketMakerIt::start_with_envs`].
    pub async fn start_async(conf: Json, userpass: String, local: Option<LocalStart>) -> Result<MarketMakerIt, String> {
        MarketMakerIt::start_with_envs(conf, userpass, local, &[]).await
    }

    /// Create a new temporary directory and start a new MarketMaker process there.
    ///
    /// * `conf` - The command-line configuration passed to the MarketMaker.
    ///            Unique local IP address is injected as "myipaddr" unless this field is already present.
    /// * `userpass` - RPC API key. We should probably extract it automatically from the MM log.
    /// * `local` - Function to start the MarketMaker in a local thread, instead of spawning a process.
    /// * `envs` - The enviroment variables passed to the process
    /// It's required to manually add 127.0.0.* IPs aliases on Mac to make it properly work.
    /// cf. https://superuser.com/a/458877, https://superuser.com/a/635327
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn start_with_envs(
        mut conf: Json,
        userpass: String,
        local: Option<LocalStart>,
        envs: &[(&str, &str)],
    ) -> Result<MarketMakerIt, String> {
        conf["allow_weak_password"] = true.into();
        let ip = try_s!(Self::myipaddr_from_conf(&mut conf));
        let folder = new_mm2_temp_folder_path(Some(ip));
        let db_dir = match conf["dbdir"].as_str() {
            Some(path) => path.into(),
            None => {
                let dir = folder.join("DB");
                conf["dbdir"] = dir.to_str().unwrap().into();
                dir
            },
        };

        try_s!(fs::create_dir(&folder));
        match fs::create_dir(db_dir) {
            Ok(_) => (),
            Err(ref ie) if ie.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return ERR!("{}", e),
        };
        let log_path = match conf["log"].as_str() {
            Some(path) => path.into(),
            None => {
                let path = folder.join("mm2.log");
                conf["log"] = path.to_str().unwrap().into();
                path
            },
        };

        // If `local` is provided
        // then instead of spawning a process we start the MarketMaker in a local thread,
        // allowing us to easily *debug* the tested MarketMaker code.
        // Note that this should only be used while running a single test,
        // using this option while running multiple tests (or multiple MarketMaker instances) is currently UB.
        let pc = if let Some(local) = local {
            local(folder.clone(), log_path.clone(), conf.clone());
            None
        } else {
            let executable = try_s!(env::args().next().ok_or("No program name"));
            let executable = try_s!(Path::new(&executable).canonicalize());
            let log = try_s!(fs::File::create(&log_path));
            let child = try_s!(Command::new(executable)
                .arg("test_mm_start")
                .arg("--nocapture")
                .current_dir(&folder)
                .env("_MM2_TEST_CONF", try_s!(json::to_string(&conf)))
                .env("MM2_UNBUFFERED_OUTPUT", "1")
                .env("RUST_LOG", "debug")
                .envs(envs.to_vec())
                .stdout(try_s!(log.try_clone()))
                .stderr(log)
                .spawn());
            Some(RaiiKill::from_handle(child))
        };

        let mut mm = MarketMakerIt {
            folder,
            ip,
            log_path,
            pc,
            userpass,
        };

        try_s!(mm.startup_checks(&conf).await);
        Ok(mm)
    }

    /// Start a new MarketMaker locally.
    ///
    /// * `conf` - The command-line configuration passed to the MarketMaker.
    /// * `userpass` - RPC API key.
    /// * `local` - Function to start the MarketMaker locally.
    /// * `envs` - The environment variables passed to the process.
    ///            The argument is ignored for nodes running in a browser.
    #[cfg(target_arch = "wasm32")]
    pub async fn start_with_envs(
        conf: Json,
        userpass: String,
        local: Option<LocalStart>,
        _envs: &[(&str, &str)],
    ) -> Result<MarketMakerIt, String> {
        MarketMakerIt::start_market_maker(conf, userpass, local, None).await
    }

    /// Start a new MarketMaker locally with a specific database namespace.
    ///
    /// * `conf` - The command-line configuration passed to the MarketMaker.
    /// * `userpass` - RPC API key.
    /// * `local` - Function to start the MarketMaker locally.
    /// * `db_namespace_id` - The test database namespace identifier.
    #[cfg(target_arch = "wasm32")]
    pub async fn start_with_db(
        conf: Json,
        userpass: String,
        local: Option<LocalStart>,
        db_namespace_id: u64,
    ) -> Result<MarketMakerIt, String> {
        MarketMakerIt::start_market_maker(conf, userpass, local, Some(db_namespace_id)).await
    }

    /// Common helper function to start the MarketMaker.
    ///
    /// * `conf` - The command-line configuration passed to the MarketMaker.
    ///            Unique P2P in-memory port is injected as `p2p_in_memory_port` unless this field is already present.
    /// * `userpass` - RPC API key. We should probably extract it automatically from the MM log.
    /// * `local` - Function to start the MarketMaker locally. Required for nodes running in a browser.
    /// * `db_namespace_id` - Optional test database namespace identifier.
    #[cfg(target_arch = "wasm32")]
    async fn start_market_maker(
        mut conf: Json,
        userpass: String,
        local: Option<LocalStart>,
        db_namespace_id: Option<u64>,
    ) -> Result<MarketMakerIt, String> {
        conf["allow_weak_password"] = true.into();
        if conf["p2p_in_memory"].is_null() {
            conf["p2p_in_memory"] = Json::Bool(true);
        }

        let i_am_seed = conf["i_am_seed"].as_bool().unwrap_or_default();
        let p2p_in_memory_port_missed = conf["p2p_in_memory_port"].is_null();
        if i_am_seed && p2p_in_memory_port_missed {
            let mut rng = common::small_rng();
            let new_p2p_port: u64 = rng.gen();

            log!("Set 'p2p_in_memory_port' to {:?}", new_p2p_port);
            conf["p2p_in_memory_port"] = Json::Number(new_p2p_port.into());
        }

        let ctx = {
            let builder = MmCtxBuilder::new().with_conf(conf.clone());

            let builder = if let Some(ns) = db_namespace_id {
                builder.with_test_db_namespace_with_id(ns)
            } else {
                builder.with_test_db_namespace()
            };

            builder.into_mm_arc()
        };

        let local = try_s!(local.ok_or("!local"));
        local(ctx.clone());

        let mut mm = MarketMakerIt { ctx, userpass };
        try_s!(mm.startup_checks(&conf).await);
        Ok(mm)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn log_as_utf8(&self) -> Result<String, String> {
        let mm_log = try_s!(slurp(&self.log_path));
        let mm_log = unsafe { String::from_utf8_unchecked(mm_log) };
        Ok(mm_log)
    }

    /// Busy-wait on the log until the `pred` returns `true` or `timeout_sec` expires.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn wait_for_log<F>(&mut self, timeout_sec: f64, pred: F) -> Result<(), String>
    where
        F: Fn(&str) -> bool,
    {
        let start = now_float();
        let ms = 50.min((timeout_sec * 1000.) as u64 / 20 + 10);
        loop {
            let mm_log = try_s!(self.log_as_utf8());
            if pred(&mm_log) {
                return Ok(());
            }
            if now_float() - start > timeout_sec {
                return ERR!("Timeout expired waiting for a log condition");
            }
            if let Some(ref mut pc) = self.pc {
                if !pc.running() {
                    return ERR!("MM process terminated prematurely at: {:?}.", self.folder);
                }
            }
            Timer::sleep(ms as f64 / 1000.).await
        }
    }

    /// Busy-wait on the log until the `pred` returns `true` or `timeout_sec` expires.
    /// The difference from standard wait_for_log is this function keeps working
    /// after process is stopped
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn wait_for_log_after_stop<F>(&self, timeout_sec: f64, pred: F) -> Result<(), String>
    where
        F: Fn(&str) -> bool,
    {
        use common::try_or_ready_err;

        let ms = 50.min((timeout_sec * 1000.) as u64 / 20 + 10);

        repeatable!(async {
            let mm_log = try_or_ready_err!(self.log_as_utf8());
            if pred(&mm_log) {
                return Ready(Ok(()));
            }
            Retry(())
        })
        .repeat_every_ms(ms)
        .with_timeout_secs(timeout_sec)
        .await
        .map_err(|e| ERRL!("{:?}", e))
        .and_then(|inner_result| inner_result)
    }

    /// Busy-wait on the instance in-memory log until the `pred` returns `true` or `timeout_sec` expires.
    #[cfg(target_arch = "wasm32")]
    pub async fn wait_for_log<F>(&mut self, timeout_sec: f64, pred: F) -> Result<(), String>
    where
        F: Fn(&str) -> bool,
    {
        wait_for_log(&self.ctx, timeout_sec, pred).await
    }

    /// Invokes the locally running MM and returns its reply.
    #[cfg(target_arch = "wasm32")]
    pub async fn rpc(&self, payload: &Json) -> Result<(StatusCode, String, HeaderMap), String> {
        let wasm_rpc = self
            .ctx
            .wasm_rpc
            .get()
            .expect("'MmCtx::rpc' must be initialized already");
        match wasm_rpc.request(payload.clone()).await {
            // Please note a new type of error will be introduced soon.
            Ok(body) => {
                let status_code = if body["error"].is_null() {
                    StatusCode::OK
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                };
                let body_str =
                    json::to_string(&body).unwrap_or_else(|_| panic!("Response {:?} is not a valid JSON", body));
                Ok((status_code, body_str, HeaderMap::new()))
            },
            Err(e) => Ok((StatusCode::INTERNAL_SERVER_ERROR, e, HeaderMap::new())),
        }
    }

    /// Invokes the locally running MM and returns its reply.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn rpc(&self, payload: &Json) -> Result<(StatusCode, String, HeaderMap), String> {
        let uri = format!("http://{}:7783", self.ip);
        log!("sending rpc request {} to {}", json::to_string(payload).unwrap(), uri);

        let payload = try_s!(json::to_vec(payload));
        let request = try_s!(Request::builder().method("POST").uri(uri).body(payload));

        let (status, headers, body) = try_s!(slurp_req(request).await);
        Ok((status, try_s!(std::str::from_utf8(&body)).trim().into(), headers))
    }

    /// Sends the &str payload to the locally running MM and returns it's reply.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn rpc_str(&self, payload: &'static str) -> Result<(StatusCode, String, HeaderMap), String> {
        let uri = format!("http://{}:7783", self.ip);
        let request = try_s!(Request::builder().method("POST").uri(uri).body(payload.into()));
        let (status, headers, body) = try_s!(block_on(slurp_req(request)));
        Ok((status, try_s!(std::str::from_utf8(&body)).trim().into(), headers))
    }

    #[cfg(target_arch = "wasm32")]
    pub fn rpc_str(&self, _payload: &'static str) -> Result<(StatusCode, String, HeaderMap), String> {
        unimplemented!()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn mm_dump(&self) -> (RaiiDump, RaiiDump) {
        mm_dump(&self.log_path)
    }

    #[cfg(target_arch = "wasm32")]
    pub fn mm_dump(&self) -> (RaiiDump, RaiiDump) {
        (RaiiDump {}, RaiiDump {})
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn my_seed_addr(&self) -> String {
        format!("{}", self.ip)
    }

    /// # Panic
    ///
    /// Panic if this instance is not a seed.
    #[cfg(target_arch = "wasm32")]
    pub fn my_seed_addr(&self) -> String {
        let p2p_port = self
            .ctx
            .p2p_in_memory_port()
            .expect("This instance is not a seed, so 'p2p_in_memory_port' is None");
        format!("/memory/{}", p2p_port)
    }

    /// Send the "stop" request to the locally running MM.
    pub async fn stop(&self) -> Result<(), String> {
        let (status, body, _headers) = match self.rpc(&json!({"userpass": self.userpass, "method": "stop"})).await {
            Ok(t) => t,
            Err(err) => {
                // Downgrade the known errors into log warnings,
                // in order not to spam the unit test logs with confusing panics, obscuring the real issue.
                if err.contains("An existing connection was forcibly closed by the remote host") {
                    log!("stop] MM already down? {}", err);
                    return Ok(());
                } else {
                    return ERR!("{}", err);
                }
            },
        };
        if status != StatusCode::OK {
            return ERR!("MM didn't accept a stop. body: {}", body);
        }
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn stop_and_wait_for_ctx_is_dropped(self, timeout_ms: u64) -> Result<(), String> {
        try_s!(self.stop().await);
        let ctx_weak = self.ctx.weak();
        drop(self);

        let started_at = now_ms();
        repeatable!(async {
            if MmArc::from_weak(&ctx_weak).is_none() {
                let took_ms = now_ms() - started_at;
                log!("stop] MmCtx was dropped in {took_ms}ms");
                return Ready(());
            }
            Retry(())
        })
        .repeat_every_secs(0.05)
        .with_timeout_ms(timeout_ms)
        .await
        .map_err(|e| ERRL!("{:?}", e))
    }

    /// Currently, we cannot wait for the `Completed IAmrelay handling for peer` log entry on WASM node,
    /// because the P2P module logs to a global logger and doesn't log to the dashboard.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn check_seednodes(&mut self) -> Result<(), String> {
        // wait for at least 1 node to be added to relay mesh
        self.wait_for_log(22., |log| log.contains("Completed IAmrelay handling for peer"))
            .await
            .map_err(|e| ERRL!("{}", e))
    }

    /// Wait for the node to start listening to new P2P connections.
    /// Please note the node is expected to be a seed.
    ///
    /// Currently, we cannot wait for the `INFO Listening on` log entry on WASM node,
    /// because the P2P module logs to a global logger and doesn't log to the dashboard.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn wait_for_p2p_listen(&mut self) -> Result<(), String> {
        self.wait_for_log(22., |log| log.contains("INFO Listening on"))
            .await
            .map_err(|e| ERRL!("{}", e))
    }

    /// Wait for the RPC to be up.
    pub async fn wait_for_rpc_is_up(&mut self) -> Result<(), String> {
        self.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))
            .await
            .map_err(|e| ERRL!("{}", e))
    }

    async fn startup_checks(&mut self, conf: &Json) -> Result<(), String> {
        let skip_startup_checks = conf["skip_startup_checks"].as_bool().unwrap_or_default();
        if skip_startup_checks {
            return Ok(());
        }

        try_s!(self.wait_for_rpc_is_up().await);

        #[cfg(not(target_arch = "wasm32"))]
        {
            let is_seed = conf["i_am_seed"].as_bool().unwrap_or_default();
            if is_seed {
                try_s!(self.wait_for_p2p_listen().await);
            }

            let skip_seednodes_check = conf["skip_seednodes_check"].as_bool().unwrap_or_default();
            if conf["seednodes"].as_array().is_some() && !skip_seednodes_check {
                try_s!(self.check_seednodes().await);
            }
        }

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn myipaddr_from_conf(conf: &mut Json) -> Result<IpAddr, String> {
        if conf["myipaddr"].is_null() {
            // Generate an unique IP.
            let mut attempts = 0;
            let mut rng = common::small_rng();
            loop {
                if attempts > 128 {
                    return ERR!("Out of local IPs?");
                }
                let ip4 = Ipv4Addr::new(127, 0, 0, rng.gen_range(1, 255));
                let ip = IpAddr::from(ip4);
                let mut mm_ips = try_s!(MM_IPS.lock());
                if mm_ips.contains_key(&ip) {
                    attempts += 1;
                    continue;
                }
                mm_ips.insert(ip, true);
                conf["myipaddr"] = format!("{}", ip).into();
                conf["rpcip"] = format!("{}", ip).into();
                return Ok(ip);
            }
        }

        // Just use the IP given in the `conf`.

        let ip: IpAddr = try_s!(try_s!(conf["myipaddr"].as_str().ok_or("myipaddr is not a string")).parse());
        let mut mm_ips = try_s!(MM_IPS.lock());
        if mm_ips.contains_key(&ip) {
            log!("MarketMakerIt] Warning, IP {} was already used.", ip)
        }
        mm_ips.insert(ip, true);
        Ok(ip)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for MarketMakerIt {
    fn drop(&mut self) {
        if let Ok(mut mm_ips) = MM_IPS.lock() {
            mm_ips.remove(&self.ip);
        } else {
            log!("MarketMakerIt] Can't lock MM_IPS.")
        }
    }
}

/// Busy-wait on the log until the `pred` returns `true` or `timeout_sec` expires.
pub async fn wait_for_log<F>(ctx: &MmArc, timeout_sec: f64, pred: F) -> Result<(), String>
where
    F: Fn(&str) -> bool,
{
    let start = now_float();
    let ms = 50.min((timeout_sec * 1000.) as u64 / 20 + 10);
    let mut buf = String::with_capacity(128);
    let mut found = false;
    loop {
        ctx.log.with_tail(&mut |tail| {
            for en in tail {
                if en.format(&mut buf).is_ok() && pred(&buf) {
                    found = true;
                    break;
                }
            }
        });
        if found {
            return Ok(());
        }

        ctx.log.with_gravity_tail(&mut |tail| {
            for chunk in tail {
                if pred(chunk) {
                    found = true;
                    break;
                }
            }
        });
        if found {
            return Ok(());
        }

        if now_float() - start > timeout_sec {
            return ERR!("Timeout expired waiting for a log condition");
        }
        Timer::sleep_ms(ms).await;
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct ToWaitForLogRe {
    ctx: u32,
    timeout_sec: f64,
    re_pred: String,
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn common_wait_for_log_re(req: Bytes) -> Result<Vec<u8>, String> {
    let args: ToWaitForLogRe = try_s!(json::from_slice(&req));
    let ctx = try_s!(MmArc::from_ffi_handle(args.ctx));
    let re = try_s!(Regex::new(&args.re_pred));

    // Run the blocking `wait_for_log` in the `POOL`.
    let (tx, rx) = oneshot::channel();
    try_s!(try_s!(POOL.lock()).spawn(async move {
        let res = wait_for_log(&ctx, args.timeout_sec, |line| re.is_match(line)).await;
        let _ = tx.send(res);
    }));
    try_s!(try_s!(rx.await));

    Ok(Vec::new())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn wait_for_log_re(ctx: &MmArc, timeout_sec: f64, re_pred: &str) -> Result<(), String> {
    let re = try_s!(Regex::new(re_pred));
    wait_for_log(ctx, timeout_sec, |line| re.is_match(line)).await
}

/// Create RAII variables to the effect of dumping the log and the status dashboard at the end of the scope.
#[cfg(not(target_arch = "wasm32"))]
pub fn mm_dump(log_path: &Path) -> (RaiiDump, RaiiDump) {
    (
        RaiiDump {
            log_path: log_path.to_path_buf(),
        },
        RaiiDump {
            log_path: dashboard_path(log_path).unwrap(),
        },
    )
}

/// A typical MM instance.
#[cfg(not(target_arch = "wasm32"))]
pub fn mm_spat() -> (&'static str, MarketMakerIt, RaiiDump, RaiiDump) {
    let passphrase = "SPATsRps3dhEtXwtnpRCKF";
    let mm = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "passphrase": passphrase,
            "rpccors": "http://localhost:4000",
            "coins": [
                {"coin":"RICK","asset":"RICK","rpcport":8923},
                {"coin":"MORTY","asset":"MORTY","rpcport":11608},
            ],
            "i_am_seed": true,
            "rpc_password": "pass",
            "is_bootstrap_node": true,
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (dump_log, dump_dashboard) = mm_dump(&mm.log_path);
    (passphrase, mm, dump_log, dump_dashboard)
}

/// Asks MM to enable the given currency in electrum mode
/// fresh list of servers at https://github.com/jl777/coins/blob/master/electrums/.
pub async fn enable_electrum(mm: &MarketMakerIt, coin: &str, tx_history: bool, urls: &[&str]) -> Json {
    let servers = urls.iter().map(|url| json!({ "url": url })).collect();
    enable_electrum_json(mm, coin, tx_history, servers).await
}

/// Asks MM to enable the given currency in electrum mode
/// fresh list of servers at https://github.com/jl777/coins/blob/master/electrums/.
pub async fn enable_electrum_json(mm: &MarketMakerIt, coin: &str, tx_history: bool, servers: Vec<Json>) -> Json {
    let electrum = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "electrum",
            "coin": coin,
            "servers": servers,
            "mm2": 1,
            "tx_history": tx_history,
        }))
        .await
        .unwrap();
    assert_eq!(
        electrum.0,
        StatusCode::OK,
        "RPC «electrum» failed with {} {}",
        electrum.0,
        electrum.1
    );
    json::from_str(&electrum.1).unwrap()
}

pub async fn enable_qrc20(
    mm: &MarketMakerIt,
    coin: &str,
    urls: &[&str],
    swap_contract_address: &str,
    path_to_address: Option<HDAccountAddressId>,
) -> Json {
    let servers: Vec<_> = urls.iter().map(|url| json!({ "url": url })).collect();
    let electrum = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "electrum",
            "coin": coin,
            "servers": servers,
            "mm2": 1,
            "swap_contract_address": swap_contract_address,
            "path_to_address": path_to_address.unwrap_or_default(),
        }))
        .await
        .unwrap();
    assert_eq!(
        electrum.0,
        StatusCode::OK,
        "RPC «electrum» failed with {} {}",
        electrum.0,
        electrum.1
    );
    json::from_str(&electrum.1).unwrap()
}

pub async fn peer_connection_healthcheck(mm: &MarketMakerIt, peer_address: &str) -> Json {
    let response = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "peer_connection_healthcheck",
            "mmrpc": "2.0",
            "params": {
                "peer_address": peer_address
            }
        }))
        .await
        .unwrap();

    assert_eq!(
        response.0,
        StatusCode::OK,
        "RPC «peer_connection_healthcheck» failed with {} {}",
        response.0,
        response.1
    );

    json::from_str(&response.1).unwrap()
}

/// Reads passphrase and userpass from .env file
pub fn from_env_file(env: Vec<u8>) -> (Option<String>, Option<String>) {
    use regex::bytes::Regex;
    let (mut passphrase, mut userpass) = (None, None);
    for cap in Regex::new(r"^\w+_(PASSPHRASE|USERPASS)=(\w+( \w+)+)\s*")
        .unwrap()
        .captures_iter(&env)
    {
        match cap.get(1) {
            Some(name) if name.as_bytes() == b"PASSPHRASE" => {
                passphrase = cap.get(2).map(|v| String::from_utf8(v.as_bytes().into()).unwrap())
            },
            Some(name) if name.as_bytes() == b"USERPASS" => {
                userpass = cap.get(2).map(|v| String::from_utf8(v.as_bytes().into()).unwrap())
            },
            _ => (),
        }
    }
    (passphrase, userpass)
}

#[macro_export]
#[cfg(target_arch = "wasm32")]
macro_rules! get_passphrase {
    ($_env_file:literal, $env:literal) => {
        option_env!($env)
            .map(|pass| pass.to_string())
            .ok_or_else(|| ERRL!("No such '{}' environment variable", $env))
    };
}

#[macro_export]
#[cfg(not(target_arch = "wasm32"))]
macro_rules! get_passphrase {
    ($env_file:literal, $env:literal) => {
        $crate::for_tests::get_passphrase(&$env_file, $env)
    };
}

/// Reads passphrase from file or environment.
/// Note that if you try to read the passphrase file from the current directory
/// the current directory could be different depending on how you run tests
/// (it could be either the workspace directory or the module source directory)
#[cfg(not(target_arch = "wasm32"))]
pub fn get_passphrase(path: &dyn AsRef<Path>, env: &str) -> Result<String, String> {
    if let (Some(file_passphrase), _file_userpass) = from_env_file(try_s!(slurp(path))) {
        return Ok(file_passphrase);
    }

    if let Ok(v) = common::var(env) {
        Ok(v)
    } else {
        ERR!("No {} or {}", env, path.as_ref().display())
    }
}

/// Asks MM to enable the given currency in native mode.
/// Returns the RPC reply containing the corresponding wallet address.
pub async fn enable_native(
    mm: &MarketMakerIt,
    coin: &str,
    urls: &[&str],
    path_to_address: Option<HDAccountAddressId>,
) -> Json {
    let native = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "enable",
            "coin": coin,
            "urls": urls,
            // Dev chain swap contract address
            "swap_contract_address": ETH_SEPOLIA_SWAP_CONTRACT,
            "path_to_address": path_to_address.unwrap_or_default(),
            "mm2": 1,
        }))
        .await
        .unwrap();
    assert_eq!(native.0, StatusCode::OK, "'enable' failed: {}", native.1);
    json::from_str(&native.1).unwrap()
}

pub async fn enable_eth_coin(
    mm: &MarketMakerIt,
    coin: &str,
    urls: &[&str],
    swap_contract_address: &str,
    fallback_swap_contract: Option<&str>,
    contract_supports_watcher: bool,
) -> Json {
    let enable = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "enable",
            "coin": coin,
            "urls": urls,
            "swap_contract_address": swap_contract_address,
            "fallback_swap_contract": fallback_swap_contract,
            "mm2": 1,
            "contract_supports_watchers": contract_supports_watcher
        }))
        .await
        .unwrap();
    assert_eq!(enable.0, StatusCode::OK, "'enable' failed: {}", enable.1);
    json::from_str(&enable.1).unwrap()
}

#[derive(Clone)]
pub struct SwapV2TestContracts {
    pub maker_swap_v2_contract: String,
    pub taker_swap_v2_contract: String,
    pub nft_maker_swap_v2_contract: String,
}

#[derive(Clone)]
pub struct TestNode {
    pub url: String,
}

pub async fn enable_eth_coin_v2(
    mm: &MarketMakerIt,
    ticker: &str,
    swap_contract_address: &str,
    swap_v2_contracts: SwapV2TestContracts,
    fallback_swap_contract: Option<&str>,
    nodes: &[TestNode],
) -> Json {
    let enable = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "enable_eth_with_tokens",
            "mmrpc": "2.0",
            "params": {
                "ticker": ticker,
                "mm2": 1,
                "swap_contract_address": swap_contract_address,
                "swap_v2_contracts": {
                    "maker_swap_v2_contract": swap_v2_contracts.maker_swap_v2_contract,
                    "taker_swap_v2_contract": swap_v2_contracts.taker_swap_v2_contract,
                    "nft_maker_swap_v2_contract": swap_v2_contracts.nft_maker_swap_v2_contract
                },
                "fallback_swap_contract": fallback_swap_contract,
                "nodes": nodes.iter().map(|node| json!({ "url": node.url })).collect::<Vec<_>>(),
                "erc20_tokens_requests": []
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        enable.0,
        StatusCode::OK,
        "'enable_eth_with_tokens' failed: {}",
        enable.1
    );
    json::from_str(&enable.1).unwrap()
}

pub async fn enable_slp(mm: &MarketMakerIt, coin: &str) -> Json {
    let enable = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "enable_slp",
            "mmrpc": "2.0",
            "params": {
                "ticker": coin,
                "activation_params": {}
            }
        }))
        .await
        .unwrap();
    assert_eq!(enable.0, StatusCode::OK, "'enable_slp' failed: {}", enable.1);
    json::from_str(&enable.1).unwrap()
}

#[derive(Serialize)]
pub struct ElectrumRpcRequest {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ElectrumProtocol>,
}

#[derive(Serialize)]
#[serde(tag = "rpc", content = "rpc_data")]
pub enum UtxoRpcMode {
    Native,
    Electrum { servers: Vec<ElectrumRpcRequest> },
}

#[cfg(not(target_arch = "wasm32"))]
pub fn electrum_servers_rpc(servers: &[&str]) -> Vec<ElectrumRpcRequest> {
    servers
        .iter()
        .map(|url| ElectrumRpcRequest {
            url: url.to_string(),
            protocol: None,
        })
        .collect()
}

#[cfg(target_arch = "wasm32")]
pub fn electrum_servers_rpc(servers: &[&str]) -> Vec<ElectrumRpcRequest> {
    servers
        .iter()
        .map(|url| ElectrumRpcRequest {
            url: url.to_string(),
            protocol: Some(ElectrumProtocol::WSS),
        })
        .collect()
}

impl UtxoRpcMode {
    pub fn electrum(servers: &[&str]) -> Self {
        UtxoRpcMode::Electrum {
            servers: electrum_servers_rpc(servers),
        }
    }
}

pub async fn enable_bch_with_tokens(
    mm: &MarketMakerIt,
    platform_coin: &str,
    tokens: &[&str],
    mode: UtxoRpcMode,
    tx_history: bool,
    path_to_address: Option<HDAccountAddressId>,
) -> Json {
    let slp_requests: Vec<_> = tokens.iter().map(|ticker| json!({ "ticker": ticker })).collect();

    let enable = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "enable_bch_with_tokens",
            "mmrpc": "2.0",
            "params": {
                "ticker": platform_coin,
                "allow_slp_unsafe_conf": true,
                "bchd_urls": [],
                "mode": mode,
                "tx_history": tx_history,
                "slp_tokens_requests": slp_requests,
                "path_to_address": path_to_address.unwrap_or_default(),
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        enable.0,
        StatusCode::OK,
        "'enable_bch_with_tokens' failed: {}",
        enable.1
    );
    json::from_str(&enable.1).unwrap()
}

pub async fn my_tx_history_v2(
    mm: &MarketMakerIt,
    coin: &str,
    limit: usize,
    paging: Option<PagingOptionsEnum<String>>,
) -> Json {
    let paging = paging.unwrap_or(PagingOptionsEnum::PageNumber(NonZeroUsize::new(1).unwrap()));
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "my_tx_history",
            "mmrpc": "2.0",
            "params": {
                "coin": coin,
                "limit": limit,
                "paging_options": paging,
            }
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'my_tx_history' failed: {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn z_coin_tx_history(
    mm: &MarketMakerIt,
    coin: &str,
    limit: usize,
    paging: Option<PagingOptionsEnum<i64>>,
) -> Json {
    let paging = paging.unwrap_or_default();
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "z_coin_tx_history",
            "mmrpc": "2.0",
            "params": {
                "coin": coin,
                "limit": limit,
                "paging_options": paging,
            }
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'z_coin_tx_history' failed: {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn enable_native_bch(mm: &MarketMakerIt, coin: &str, bchd_urls: &[&str]) -> Json {
    let native = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "enable",
            "coin": coin,
            "bchd_urls": bchd_urls,
            "allow_slp_unsafe_conf": true,
            "mm2": 1,
        }))
        .await
        .unwrap();
    assert_eq!(native.0, StatusCode::OK, "'enable' failed: {}", native.1);
    json::from_str(&native.1).unwrap()
}

pub async fn init_lightning(mm: &MarketMakerIt, coin: &str) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::enable_lightning::init",
            "mmrpc": "2.0",
            "params": {
                "ticker": coin,
                "activation_params": {
                    "name": "test-node"
                }
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::enable_lightning::init' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn init_lightning_status(mm: &MarketMakerIt, task_id: u64) -> Json {
    let request = mm
        .rpc(&json! ({
            "userpass": mm.userpass,
            "method": "task::enable_lightning::status",
            "mmrpc": "2.0",
            "params": {
                "task_id": task_id,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::enable_lightning::status' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

/// Use a separate (unique) temporary folder for each MM.
/// We could also remove the old folders after some time in order not to spam the temporary folder.
/// Though we don't always want to remove them right away, allowing developers to check the files).
/// Appends IpAddr if it is pre-known
#[cfg(not(target_arch = "wasm32"))]
pub fn new_mm2_temp_folder_path(ip: Option<IpAddr>) -> PathBuf {
    let now = common::now_ms();
    #[allow(deprecated)]
    let now = Local.timestamp((now / 1000) as i64, (now % 1000) as u32 * 1_000_000);
    let folder = match ip {
        Some(ip) => format!("mm2_{}_{}", now.format("%Y-%m-%d_%H-%M-%S-%3f"), ip),
        None => format!("mm2_{}", now.format("%Y-%m-%d_%H-%M-%S-%3f")),
    };
    common::temp_dir().join(folder)
}

pub fn find_metrics_in_json(
    metrics: MetricsJson,
    search_key: &str,
    search_labels: &[(&str, &str)],
) -> Option<MetricType> {
    metrics.metrics.into_iter().find(|metric| {
        let (key, labels) = match metric {
            MetricType::Counter { key, labels, .. } => (key, labels),
            _ => return false,
        };

        if key != search_key {
            return false;
        }

        for (s_label_key, s_label_value) in search_labels.iter() {
            let label_value = match labels.get(&(*s_label_key).to_string()) {
                Some(x) => x,
                _ => return false,
            };

            if s_label_value != label_value {
                return false;
            }
        }

        true
    })
}

pub async fn my_swap_status(mm: &MarketMakerIt, uuid: &str) -> Result<Json, String> {
    let response = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "my_swap_status",
            "params": {
                "uuid": uuid,
            }
        }))
        .await
        .unwrap();

    if !response.0.is_success() {
        return Err(format!("!status of {}: {}", uuid, response.1));
    }

    Ok(json::from_str(&response.1).unwrap())
}

pub async fn wait_for_swap_status(mm: &MarketMakerIt, uuid: &str, wait_sec: i64) {
    let wait_until = get_utc_timestamp() + wait_sec;
    loop {
        if my_swap_status(mm, uuid).await.is_ok() {
            break;
        }

        if get_utc_timestamp() > wait_until {
            panic!("Timed out waiting for swap {} status", uuid);
        }

        Timer::sleep(1.).await;
    }
}

pub async fn wait_for_swap_finished(mm: &MarketMakerIt, uuid: &str, wait_sec: i64) {
    let wait_until = get_utc_timestamp() + wait_sec;
    loop {
        let status = my_swap_status(mm, uuid).await.unwrap();
        if status["result"]["is_finished"].as_bool().unwrap() {
            break;
        }

        if get_utc_timestamp() > wait_until {
            panic!("Timed out waiting for swap {} to finish", uuid);
        }

        Timer::sleep(0.5).await;
    }
}

pub async fn wait_for_swap_contract_negotiation(mm: &MarketMakerIt, swap: &str, expected_contract: Json, until: i64) {
    let events = loop {
        if get_utc_timestamp() > until {
            panic!("Timed out");
        }

        let swap_status = my_swap_status(mm, swap).await.unwrap();
        let events = swap_status["result"]["events"].as_array().unwrap();
        if events.len() < 2 {
            Timer::sleep(1.).await;
            continue;
        }

        break events.clone();
    };
    assert_eq!(events[1]["event"]["type"], Json::from("Negotiated"));
    assert_eq!(
        events[1]["event"]["data"]["maker_coin_swap_contract_addr"],
        expected_contract
    );
    assert_eq!(
        events[1]["event"]["data"]["taker_coin_swap_contract_addr"],
        expected_contract
    );
}

pub async fn wait_for_swap_negotiation_failure(mm: &MarketMakerIt, swap: &str, until: i64) {
    let events = loop {
        if get_utc_timestamp() > until {
            panic!("Timed out");
        }

        let swap_status = my_swap_status(mm, swap).await.unwrap();
        let events = swap_status["result"]["events"].as_array().unwrap();
        if events.len() < 2 {
            Timer::sleep(1.).await;
            continue;
        }

        break events.clone();
    };
    assert_eq!(events[1]["event"]["type"], Json::from("NegotiateFailed"));
}

/// Helper function requesting my swap status and checking it's events
pub async fn check_my_swap_status(mm: &MarketMakerIt, uuid: &str, maker_amount: BigDecimal, taker_amount: BigDecimal) {
    let status_response = my_swap_status(mm, uuid).await.unwrap();
    let swap_type = match status_response["result"]["type"].as_str() {
        Some(t) => t,
        None => return,
    };

    let success_events: Vec<String> = json::from_value(status_response["result"]["success_events"].clone()).unwrap();
    if swap_type == "Taker" {
        assert!(success_events == TAKER_SUCCESS_EVENTS || success_events == TAKER_USING_WATCHERS_SUCCESS_EVENTS);
    } else {
        assert_eq!(success_events, MAKER_SUCCESS_EVENTS)
    }

    let expected_error_events = if swap_type == "Taker" {
        TAKER_ERROR_EVENTS.to_vec()
    } else {
        MAKER_ERROR_EVENTS.to_vec()
    };
    let error_events: Vec<String> = json::from_value(status_response["result"]["error_events"].clone()).unwrap();
    assert_eq!(expected_error_events, error_events.as_slice());

    let events_array = status_response["result"]["events"].as_array().unwrap();
    let actual_maker_amount = json::from_value(events_array[0]["event"]["data"]["maker_amount"].clone()).unwrap();
    assert_eq!(maker_amount, actual_maker_amount);
    let actual_taker_amount = json::from_value(events_array[0]["event"]["data"]["taker_amount"].clone()).unwrap();
    assert_eq!(taker_amount, actual_taker_amount);
    let actual_events = events_array
        .iter()
        .map(|item| item["event"]["type"].as_str().unwrap().to_string())
        .collect::<Vec<String>>();
    assert!(actual_events.iter().all(|item| success_events.contains(item)));
}

pub async fn check_my_swap_status_amounts(
    mm: &MarketMakerIt,
    uuid: Uuid,
    maker_amount: BigDecimal,
    taker_amount: BigDecimal,
) {
    let status_response = my_swap_status(mm, &uuid.to_string()).await.unwrap();

    let events_array = status_response["result"]["events"].as_array().unwrap();
    let actual_maker_amount = json::from_value(events_array[0]["event"]["data"]["maker_amount"].clone()).unwrap();
    assert_eq!(maker_amount, actual_maker_amount);
    let actual_taker_amount = json::from_value(events_array[0]["event"]["data"]["taker_amount"].clone()).unwrap();
    assert_eq!(taker_amount, actual_taker_amount);
}

pub async fn wait_check_stats_swap_status(mm: &MarketMakerIt, uuid: &str, timeout: i64) {
    let wait_until = get_utc_timestamp() + timeout;
    loop {
        let response = mm
            .rpc(&json!({
                "method": "stats_swap_status",
                "params": {
                    "uuid": uuid,
                }
            }))
            .await
            .unwrap();
        assert!(response.0.is_success(), "!status of {}: {}", uuid, response.1);
        let status_response: Json = json::from_str(&response.1).unwrap();

        // Perform the checks only if the maker and taker stats are available.
        // Sometimes they are slow to propagate so we need to wait a bit.
        if status_response["result"]["maker"].is_null() || status_response["result"]["taker"].is_null() {
            Timer::sleep(1.).await;
            if get_utc_timestamp() > wait_until {
                panic!("Timed out waiting for swap stats status uuid={}", uuid);
            }
        } else {
            let maker_events_array = status_response["result"]["maker"]["events"].as_array().unwrap();
            let taker_events_array = status_response["result"]["taker"]["events"].as_array().unwrap();
            let maker_actual_events = maker_events_array
                .iter()
                .map(|item| item["event"]["type"].as_str().unwrap());
            let maker_actual_events: Vec<&str> = maker_actual_events.collect();
            let taker_actual_events = taker_events_array
                .iter()
                .map(|item| item["event"]["type"].as_str().unwrap());
            let taker_actual_events: Vec<&str> = taker_actual_events.collect();

            assert_eq!(maker_actual_events.as_slice(), MAKER_SUCCESS_EVENTS);
            assert!(
                taker_actual_events.as_slice() == TAKER_SUCCESS_EVENTS
                    || taker_actual_events.as_slice() == TAKER_ACTUAL_EVENTS_WATCHER_SPENDS_MAKER_PAYMENT
                    || taker_actual_events.as_slice() == TAKER_ACTUAL_EVENTS_TAKER_SPENDS_MAKER_PAYMENT
            );
            return;
        }
    }
}

pub async fn check_recent_swaps(mm: &MarketMakerIt, expected_len: usize) {
    let response = mm
        .rpc(&json!({
            "method": "my_recent_swaps",
            "userpass": mm.userpass,
        }))
        .await
        .unwrap();
    assert!(response.0.is_success(), "!status of my_recent_swaps {}", response.1);
    let swaps_response: Json = json::from_str(&response.1).unwrap();
    let swaps: &Vec<Json> = swaps_response["result"]["swaps"].as_array().unwrap();
    assert_eq!(expected_len, swaps.len());
}

pub async fn wait_till_history_has_records(mm: &MarketMakerIt, coin: &str, expected_len: usize) {
    // give 2 second max to fetch a single transaction
    let to_wait = expected_len as u64 * 2;
    let wait_until = wait_until_ms(to_wait * 1000);
    loop {
        let tx_history = mm
            .rpc(&json!({
                "userpass": mm.userpass,
                "method": "my_tx_history",
                "coin": coin,
                "limit": 100,
            }))
            .await
            .unwrap();
        assert_eq!(
            tx_history.0,
            StatusCode::OK,
            "RPC «my_tx_history» failed with status «{}», response «{}»",
            tx_history.0,
            tx_history.1
        );
        log!("{:?}", tx_history.1);
        let tx_history_json: Json = json::from_str(&tx_history.1).unwrap();
        if tx_history_json["result"]["transactions"].as_array().unwrap().len() >= expected_len {
            break;
        }

        Timer::sleep(1.).await;
        assert!(now_ms() <= wait_until, "wait_till_history_has_records timed out");
    }
}

pub async fn orderbook(mm: &MarketMakerIt, base: &str, rel: &str) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "orderbook",
            "base": base,
            "rel": rel,
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'orderbook' failed: {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn orderbook_v2(mm: &MarketMakerIt, base: &str, rel: &str) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "orderbook",
            "mmrpc": "2.0",
            "params": {
                "base": base,
                "rel": rel,
            }
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'orderbook' failed: {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn best_orders_v2(
    mm: &MarketMakerIt,
    coin: &str,
    action: &str,
    volume: &str,
) -> RpcV2Response<BestOrdersV2Response> {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "best_orders",
            "mmrpc": "2.0",
            "params": {
                "coin": coin,
                "action": action,
                "request_by": {
                    "type": "volume",
                    "value": volume,
                }
            }
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'best_orders' failed: {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn best_orders_v2_by_number(
    mm: &MarketMakerIt,
    coin: &str,
    action: &str,
    number: usize,
    exclude_mine: bool,
) -> RpcV2Response<BestOrdersV2Response> {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "best_orders",
            "mmrpc": "2.0",
            "params": {
                "coin": coin,
                "action": action,
                "request_by": {
                    "type": "number",
                    "value": number,
                },
                "exclude_mine": exclude_mine
            }
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'best_orders' failed: {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn init_withdraw(mm: &MarketMakerIt, coin: &str, to: &str, amount: &str, from: Option<Json>) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::withdraw::init",
            "mmrpc": "2.0",
            "params": {
                "coin": coin,
                "to": to,
                "amount": amount,
                "from": from,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::withdraw::init' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn withdraw_v1(
    mm: &MarketMakerIt,
    coin: &str,
    to: &str,
    amount: &str,
    from: Option<HDAccountAddressId>,
) -> TransactionDetails {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "withdraw",
            "coin": coin,
            "to": to,
            "amount": amount,
            "from": from,
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'withdraw' failed: {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn ibc_withdraw(
    mm: &MarketMakerIt,
    source_channel: u16,
    coin: &str,
    to: &str,
    amount: &str,
    from: Option<HDAccountAddressId>,
) -> TransactionDetails {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "withdraw",
            "mmrpc": "2.0",
            "params": {
                "ibc_source_channel": source_channel,
                "coin": coin,
                "to": to,
                "amount": amount,
                "from": from,
            }
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'withdraw' failed: {}", request.1);

    let json: Json = json::from_str(&request.1).unwrap();
    json::from_value(json["result"].clone()).unwrap()
}

pub async fn withdraw_status(mm: &MarketMakerIt, task_id: u64) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::withdraw::status",
            "mmrpc": "2.0",
            "params": {
                "task_id": task_id,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::withdraw::status' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn init_z_coin_native(mm: &MarketMakerIt, coin: &str) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::enable_z_coin::init",
            "mmrpc": "2.0",
            "params": {
                "ticker": coin,
                "activation_params": {
                    "mode": {
                        "rpc": "Native",
                    }
                },
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::enable_z_coin::init' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn init_z_coin_light(
    mm: &MarketMakerIt,
    coin: &str,
    electrums: &[&str],
    lightwalletd_urls: &[&str],
    starting_date: Option<u64>,
    account: Option<u32>,
) -> Json {
    // Number of seconds in a day
    let one_day_seconds = 24 * 60 * 60;
    let starting_date = starting_date.unwrap_or(now_sec() - one_day_seconds);

    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::enable_z_coin::init",
            "mmrpc": "2.0",
            "params": {
                "ticker": coin,
                "activation_params": {
                    "mode": {
                        "rpc": "Light",
                        "rpc_data": {
                            "electrum_servers": electrum_servers_rpc(electrums),
                            "light_wallet_d_servers": lightwalletd_urls,
                            "sync_params": {
                                "date": starting_date
                            }
                        },
                    },
                    "account": account.unwrap_or_default(),
                },
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::enable_z_coin::init' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn init_z_coin_status(mm: &MarketMakerIt, task_id: u64) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::enable_z_coin::status",
            "mmrpc": "2.0",
            "params": {
                "task_id": task_id,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::enable_z_coin::status' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn sign_message(mm: &MarketMakerIt, coin: &str, derivation_path: Option<HDAddressSelector>) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method":"sign_message",
            "mmrpc":"2.0",
            "id": 0,
            "params":{
              "coin": coin,
              "message": "test",
              "address": derivation_path
            }
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'sign_message' failed: {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn verify_message(mm: &MarketMakerIt, coin: &str, signature: &str, address: &str) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method":"verify_message",
            "mmrpc":"2.0",
            "id": 0,
            "params":{
              "coin": coin,
              "message":"test",
              "signature": signature,
              "address": address

            }
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'verify_message' failed: {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn send_raw_transaction(mm: &MarketMakerIt, coin: &str, tx: &str) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "send_raw_transaction",
            "coin": coin,
            "tx_hex": tx,
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'send_raw_transaction' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn my_balance(mm: &MarketMakerIt, coin: &str) -> BalanceResponse {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "my_balance",
            "coin": coin
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'my_balance' failed: {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn get_shared_db_id(mm: &MarketMakerIt) -> GetSharedDbIdResult {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "get_shared_db_id",
            "mmrpc": "2.0",
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'get_shared_db_id' failed: {}", request.1);
    let res: RpcSuccessResponse<_> = json::from_str(&request.1).unwrap();
    res.result
}

pub async fn get_wallet_names(mm: &MarketMakerIt) -> GetWalletNamesResult {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "get_wallet_names",
            "mmrpc": "2.0",
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'get_wallet_names' failed: {}", request.1);
    let res: RpcSuccessResponse<_> = json::from_str(&request.1).unwrap();
    res.result
}

pub async fn delete_wallet(mm: &MarketMakerIt, wallet_name: &str, password: &str) -> (StatusCode, String, HeaderMap) {
    mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "delete_wallet",
        "mmrpc": "2.0",
        "params": {
            "wallet_name": wallet_name,
            "password": password,
        }
    }))
    .await
    .unwrap()
}

pub async fn max_maker_vol(mm: &MarketMakerIt, coin: &str) -> RpcResponse {
    let rc = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "mmrpc": "2.0",
            "method": "max_maker_vol",
            "params": {
                "coin": coin,
            }
        }))
        .await
        .unwrap();
    RpcResponse::new("max_maker_vol", rc)
}

pub async fn disable_coin(mm: &MarketMakerIt, coin: &str, force_disable: bool) -> DisableResult {
    let req = json! ({
        "userpass": mm.userpass,
        "method": "disable_coin",
        "coin": coin,
        "force_disable": force_disable,
    });
    let disable = mm.rpc(&req).await.unwrap();
    assert_eq!(disable.0, StatusCode::OK, "!disable_coin: {}", disable.1);
    let res: Json = json::from_str(&disable.1).unwrap();
    json::from_value(res["result"].clone()).unwrap()
}

/// Checks whether the `disable_coin` RPC fails.
/// Returns a `DisableCoinError` error.
pub async fn disable_coin_err(mm: &MarketMakerIt, coin: &str, force_disable: bool) -> DisableCoinError {
    let disable = mm
        .rpc(&json! ({
            "userpass": mm.userpass,
            "method": "disable_coin",
            "coin": coin,
            "force_disable": force_disable,
        }))
        .await
        .unwrap();
    assert!(!disable.0.is_success(), "'disable_coin' should have failed");
    json::from_str(&disable.1).unwrap()
}

pub async fn assert_coin_not_found_on_balance(mm: &MarketMakerIt, coin: &str) {
    let balance = mm
        .rpc(&json! ({
            "userpass": mm.userpass,
            "method": "my_balance",
            "coin": coin
        }))
        .await
        .unwrap();
    assert_eq!(balance.0, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(balance.1.contains(&format!("No such coin: {coin}")));
}

pub async fn enable_tendermint(
    mm: &MarketMakerIt,
    coin: &str,
    ibc_assets: &[&str],
    rpc_urls: &[&str],
    tx_history: bool,
) -> Json {
    let ibc_requests: Vec<_> = ibc_assets.iter().map(|ticker| json!({ "ticker": ticker })).collect();
    let nodes: Vec<Json> = rpc_urls
        .iter()
        .map(|u| json!({"url": u, "komodo_proxy": false }))
        .collect();

    let request = json!({
        "userpass": mm.userpass,
        "method": "enable_tendermint_with_assets",
        "mmrpc": "2.0",
        "params": {
            "ticker": coin,
            "tokens_params": ibc_requests,
            "nodes": nodes,
            "tx_history": tx_history
        }
    });
    log!(
        "enable_tendermint_with_assets request {}",
        json::to_string(&request).unwrap()
    );

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'enable_tendermint_with_assets' failed: {}",
        request.1
    );
    log!("enable_tendermint_with_assets response {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn enable_tendermint_without_balance(
    mm: &MarketMakerIt,
    coin: &str,
    ibc_assets: &[&str],
    rpc_urls: &[&str],
    tx_history: bool,
) -> Json {
    let ibc_requests: Vec<_> = ibc_assets.iter().map(|ticker| json!({ "ticker": ticker })).collect();
    let nodes: Vec<Json> = rpc_urls
        .iter()
        .map(|u| json!({"url": u, "komodo_proxy": false }))
        .collect();

    let request = json!({
        "userpass": mm.userpass,
        "method": "enable_tendermint_with_assets",
        "mmrpc": "2.0",
        "params": {
            "ticker": coin,
            "tokens_params": ibc_requests,
            "nodes": nodes,
            "tx_history": tx_history,
            "get_balances": false
        }
    });
    log!(
        "enable_tendermint_with_assets request {}",
        serde_json::to_string(&request).unwrap()
    );

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'enable_tendermint_with_assets' failed: {}",
        request.1
    );
    log!("enable_tendermint_with_assets response {}", request.1);
    serde_json::from_str(&request.1).unwrap()
}

pub async fn get_tendermint_my_tx_history(mm: &MarketMakerIt, coin: &str, limit: usize, page_number: usize) -> Json {
    let request = json!({
        "userpass": mm.userpass,
        "method": "my_tx_history",
        "mmrpc": "2.0",
        "params": {
            "coin": coin,
            "limit": limit,
            "paging_options": {
                "PageNumber": page_number
            },
        }
    });
    log!(
        "tendermint 'my_tx_history' request {}",
        json::to_string(&request).unwrap()
    );

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "tendermint 'my_tx_history' failed: {}",
        request.1
    );

    log!("tendermint 'my_tx_history' response {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn tendermint_delegations(mm: &MarketMakerIt, coin: &str) -> Json {
    let rpc_endpoint = "experimental::staking::query::delegations";
    let request = json!({
        "userpass": mm.userpass,
        "method": rpc_endpoint,
        "mmrpc": "2.0",
        "params": {
            "coin": coin,
            "info_details": {
                "type": "Cosmos",
                "limit": 0,
                "page_number": 1
            }
        }
    });
    log!("{rpc_endpoint} request {}", json::to_string(&request).unwrap());

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(request.0, StatusCode::OK, "'{rpc_endpoint}' failed: {}", request.1);
    log!("{rpc_endpoint} response {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn tendermint_ongoing_undelegations(mm: &MarketMakerIt, coin: &str) -> Json {
    let rpc_endpoint = "experimental::staking::query::ongoing_undelegations";
    let request = json!({
        "userpass": mm.userpass,
        "method": rpc_endpoint,
        "mmrpc": "2.0",
        "params": {
            "coin": coin,
            "info_details": {
                "type": "Cosmos",
                "limit": 0,
                "page_number": 1
            }
        }
    });
    log!("{rpc_endpoint} request {}", json::to_string(&request).unwrap());

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(request.0, StatusCode::OK, "'{rpc_endpoint}' failed: {}", request.1);
    log!("{rpc_endpoint} response {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn enable_tendermint_token(mm: &MarketMakerIt, coin: &str) -> Json {
    let request = json!({
        "userpass": mm.userpass,
        "method": "enable_tendermint_token",
        "mmrpc": "2.0",
        "params": {
            "ticker": coin,
            "activation_params": {}
        }
    });
    log!("enable_tendermint_token request {}", json::to_string(&request).unwrap());

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'enable_tendermint_token' failed: {}",
        request.1
    );
    log!("enable_tendermint_token response {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn tendermint_validators(
    mm: &MarketMakerIt,
    coin: &str,
    filter_by_status: &str,
    limit: usize,
    page_number: usize,
) -> Json {
    let rpc_endpoint = "experimental::staking::query::validators";
    let request = json!({
        "userpass": mm.userpass,
        "method": rpc_endpoint,
        "mmrpc": "2.0",
        "params": {
            "coin": coin,
            "info_details": {
                "type": "Cosmos",
                "filter_by_status": filter_by_status,
                "limit": limit,
                "page_number": page_number
            }
        }
    });
    log!("{rpc_endpoint} request {}", json::to_string(&request).unwrap());

    let response = mm.rpc(&request).await.unwrap();
    assert_eq!(response.0, StatusCode::OK, "{rpc_endpoint} failed: {}", response.1);
    log!("{rpc_endpoint} response {}", response.1);
    json::from_str(&response.1).unwrap()
}

pub async fn tendermint_add_delegation(
    mm: &MarketMakerIt,
    coin: &str,
    validator_address: &str,
    amount: &str,
) -> TransactionDetails {
    let rpc_endpoint = "experimental::staking::delegate";
    let request = json!({
        "userpass": mm.userpass,
        "method": rpc_endpoint,
        "mmrpc": "2.0",
        "params": {
            "coin": coin,
            "staking_details": {
                "type": "Cosmos",
                "validator_address": validator_address,
                "amount": amount,
            }
        }
    });
    log!("{rpc_endpoint} request {}", json::to_string(&request).unwrap());

    let response = mm.rpc(&request).await.unwrap();
    assert_eq!(response.0, StatusCode::OK, "{rpc_endpoint} failed: {}", response.1);
    log!("{rpc_endpoint} response {}", response.1);

    let json: Json = json::from_str(&response.1).unwrap();
    json::from_value(json["result"].clone()).unwrap()
}

pub async fn tendermint_remove_delegation_raw(
    mm: &MarketMakerIt,
    coin: &str,
    validator_address: &str,
    amount: &str,
) -> (StatusCode, String, HeaderMap) {
    let rpc_endpoint = "experimental::staking::undelegate";
    let request = json!({
        "userpass": mm.userpass,
        "method": rpc_endpoint,
        "mmrpc": "2.0",
        "params": {
            "coin": coin,
            "staking_details": {
                "type": "Cosmos",
                "validator_address": validator_address,
                "amount": amount,
            }
        }
    });
    log!("{rpc_endpoint} request {}", json::to_string(&request).unwrap());

    mm.rpc(&request).await.unwrap()
}

pub async fn tendermint_remove_delegation(
    mm: &MarketMakerIt,
    coin: &str,
    validator_address: &str,
    amount: &str,
) -> TransactionDetails {
    let rpc_endpoint = "experimental::staking::undelegate";
    let response = tendermint_remove_delegation_raw(mm, coin, validator_address, amount).await;
    assert_eq!(response.0, StatusCode::OK, "{rpc_endpoint} failed: {}", response.1);
    log!("{rpc_endpoint} response {}", response.1);

    let json: Json = json::from_str(&response.1).unwrap();
    json::from_value(json["result"].clone()).unwrap()
}

pub async fn init_utxo_electrum(
    mm: &MarketMakerIt,
    coin: &str,
    servers: Vec<Json>,
    path_to_address: Option<HDAccountAddressId>,
    priv_key_policy: Option<&str>,
) -> Json {
    let mut activation_params = json!({
        "mode": {
            "rpc": "Electrum",
            "rpc_data": {
                "servers": servers,
            }
        }
    });
    if let Some(priv_key_policy) = priv_key_policy {
        activation_params["priv_key_policy"] = priv_key_policy.into();
    }
    if let Some(path_to_address) = path_to_address {
        activation_params["path_to_address"] = json!(path_to_address);
    }
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::enable_utxo::init",
            "mmrpc": "2.0",
            "params": {
                "ticker": coin,
                "activation_params": activation_params,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::enable_utxo::init' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn init_utxo_status(mm: &MarketMakerIt, task_id: u64) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::enable_utxo::status",
            "mmrpc": "2.0",
            "params": {
                "task_id": task_id,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::enable_utxo::status' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn enable_utxo_v2_electrum(
    mm: &MarketMakerIt,
    coin: &str,
    servers: Vec<Json>,
    path_to_address: Option<HDAccountAddressId>,
    timeout: u64,
    priv_key_policy: Option<&str>,
) -> UtxoStandardActivationResult {
    let init = init_utxo_electrum(mm, coin, servers, path_to_address, priv_key_policy).await;
    let init: RpcV2Response<InitTaskResult> = json::from_value(init).unwrap();
    let timeout = wait_until_ms(timeout * 1000);

    loop {
        if now_ms() > timeout {
            panic!("{} initialization timed out", coin);
        }

        let status = init_utxo_status(mm, init.result.task_id).await;
        let status: RpcV2Response<InitUtxoStatus> = json::from_value(status).unwrap();
        log!("init_utxo_status: {:?}", status);
        match status.result {
            InitUtxoStatus::Ok(result) => break result,
            InitUtxoStatus::Error(e) => panic!("{} initialization error {:?}", coin, e),
            _ => Timer::sleep(1.).await,
        }
    }
}

pub async fn init_eth_with_tokens(
    mm: &MarketMakerIt,
    platform_coin: &str,
    tokens: &[&str],
    swap_contract_address: &str,
    nodes: &[&str],
    path_to_address: Option<HDAccountAddressId>,
) -> Json {
    let erc20_tokens_requests: Vec<_> = tokens.iter().map(|ticker| json!({ "ticker": ticker })).collect();
    let nodes: Vec<_> = nodes.iter().map(|url| json!({ "url": url })).collect();

    let response = mm
        .rpc(&json!({
        "userpass": mm.userpass,
        "method": "task::enable_eth::init",
        "mmrpc": "2.0",
        "params": {
                "ticker": platform_coin,
                "swap_contract_address": swap_contract_address,
                "nodes": nodes,
                "tx_history": true,
                "erc20_tokens_requests": erc20_tokens_requests,
                "path_to_address": path_to_address.unwrap_or_default(),
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        response.0,
        StatusCode::OK,
        "'task::enable_eth::init' failed: {}",
        response.1
    );
    json::from_str(&response.1).unwrap()
}

pub async fn init_eth_with_tokens_status(mm: &MarketMakerIt, task_id: u64) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::enable_eth::status",
            "mmrpc": "2.0",
            "params": {
                "task_id": task_id,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::enable_eth::status' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn enable_eth_with_tokens_v2(
    mm: &MarketMakerIt,
    platform_coin: &str,
    tokens: &[&str],
    swap_contract_address: &str,
    nodes: &[&str],
    timeout: u64,
    path_to_address: Option<HDAccountAddressId>,
) -> EthWithTokensActivationResult {
    let init = init_eth_with_tokens(mm, platform_coin, tokens, swap_contract_address, nodes, path_to_address).await;
    let init: RpcV2Response<InitTaskResult> = json::from_value(init).unwrap();
    let timeout = wait_until_ms(timeout * 1000);

    loop {
        if now_ms() > timeout {
            panic!("{} initialization timed out", platform_coin);
        }

        let status = init_eth_with_tokens_status(mm, init.result.task_id).await;
        let status: RpcV2Response<InitEthWithTokensStatus> = json::from_value(status).unwrap();
        match status.result {
            InitEthWithTokensStatus::Ok(result) => break result,
            InitEthWithTokensStatus::Error(e) => panic!("{} initialization error {:?}", platform_coin, e),
            _ => Timer::sleep(1.).await,
        }
    }
}

async fn init_erc20_token(
    mm: &MarketMakerIt,
    ticker: &str,
    protocol: Option<Json>,
    path_to_address: Option<HDAccountAddressId>,
) -> Result<(StatusCode, Json), Json> {
    let (status, response, _) = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::enable_erc20::init",
            "mmrpc": "2.0",
            "params": {
                "ticker": ticker,
                "protocol": protocol,
                "activation_params": {
                    "path_to_address": path_to_address.unwrap_or_default(),
                }
            }
        }))
        .await
        .unwrap();

    if status.is_success() {
        Ok((status, json::from_str(&response).unwrap()))
    } else {
        Err(json::from_str(&response).unwrap())
    }
}

async fn init_erc20_token_status(mm: &MarketMakerIt, task_id: u64) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::enable_erc20::status",
            "mmrpc": "2.0",
            "params": {
                "task_id": task_id,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::enable_erc20::status' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn enable_erc20_token_v2(
    mm: &MarketMakerIt,
    ticker: &str,
    protocol: Option<Json>,
    timeout: u64,
    path_to_address: Option<HDAccountAddressId>,
) -> Result<InitTokenActivationResult, Json> {
    let init = init_erc20_token(mm, ticker, protocol, path_to_address).await?.1;
    let init: RpcV2Response<InitTaskResult> = json::from_value(init).unwrap();
    let timeout = wait_until_ms(timeout * 1000);

    loop {
        if now_ms() > timeout {
            panic!("{} initialization timed out", ticker);
        }

        let status = init_erc20_token_status(mm, init.result.task_id).await;
        let status: RpcV2Response<InitErc20TokenStatus> = json::from_value(status).unwrap();
        match status.result {
            InitErc20TokenStatus::Ok(result) => break Ok(result),
            InitErc20TokenStatus::Error(e) => break Err(e),
            _ => Timer::sleep(1.).await,
        }
    }
}

pub async fn get_token_info(mm: &MarketMakerIt, protocol: Json) -> TokenInfoResponse {
    let response = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "get_token_info",
            "mmrpc": "2.0",
            "params": {
                "protocol": protocol,
            }
        }))
        .await
        .unwrap();
    assert_eq!(response.0, StatusCode::OK, "'get_token_info' failed: {}", response.1);
    let response_json: Json = json::from_str(&response.1).unwrap();
    json::from_value(response_json["result"].clone()).unwrap()
}

/// Note that mm2 ignores `volume` if `max` is true.
pub async fn set_price(
    mm: &MarketMakerIt,
    base: &str,
    rel: &str,
    price: &str,
    vol: &str,
    max: bool,
    timeout_in_minutes: Option<u16>,
) -> SetPriceResponse {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "setprice",
            "base": base,
            "rel": rel,
            "price": price,
            "volume": vol,
            "max": max,
            "timeout_in_minutes": timeout_in_minutes,
        }))
        .await
        .unwrap();
    assert_eq!(request.0, StatusCode::OK, "'setprice' failed: {}", request.1);
    json::from_str(&request.1).unwrap()
}

pub async fn start_swaps(
    maker: &mut MarketMakerIt,
    taker: &mut MarketMakerIt,
    pairs: &[(&'static str, &'static str)],
    maker_price: f64,
    taker_price: f64,
    volume: f64,
) -> Vec<String> {
    let mut uuids = vec![];

    // issue sell request on Bob side by setting base/rel price
    for (base, rel) in pairs.iter() {
        common::log::info!("Issue maker {}/{} sell request", base, rel);
        let rc = maker
            .rpc(&json!({
                "userpass": maker.userpass,
                "method": "setprice",
                "base": base,
                "rel": rel,
                "price": maker_price,
                "volume": volume
            }))
            .await
            .unwrap();
        assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    }

    for (base, rel) in pairs.iter() {
        common::log::info!(
            "Trigger taker subscription to {}/{} orderbook topic first and sleep for 1 second",
            base,
            rel
        );
        let rc = taker
            .rpc(&json!({
                "userpass": taker.userpass,
                "method": "orderbook",
                "mmrpc": "2.0",
                "params": {
                    "base": base,
                    "rel": rel,
                },
            }))
            .await
            .unwrap();
        assert!(rc.0.is_success(), "!orderbook: {}", rc.1);
        Timer::sleep(1.).await;
        common::log::info!("Issue taker {}/{} buy request", base, rel);
        let rc = taker
            .rpc(&json!({
                "userpass": taker.userpass,
                "method": "buy",
                "base": base,
                "rel": rel,
                "volume": volume,
                "price": taker_price
            }))
            .await
            .unwrap();
        assert!(rc.0.is_success(), "!buy: {}", rc.1);
        let buy_json: Json = serde_json::from_str(&rc.1).unwrap();
        uuids.push(buy_json["result"]["uuid"].as_str().unwrap().to_owned());
    }

    for uuid in uuids.iter() {
        // ensure the swaps are started
        wait_for_swap_status(taker, uuid, 10).await;
        wait_for_swap_status(maker, uuid, 10).await;
    }

    uuids
}

pub async fn wait_for_swaps_finish_and_check_status(
    maker: &mut MarketMakerIt,
    taker: &mut MarketMakerIt,
    uuids: &[impl AsRef<str>],
    volume: f64,
    maker_price: f64,
) {
    for uuid in uuids.iter() {
        wait_for_swap_finished(maker, uuid.as_ref(), 900).await;
        wait_for_swap_finished(taker, uuid.as_ref(), 900).await;

        log!("Checking taker status..");
        check_my_swap_status(
            taker,
            uuid.as_ref(),
            BigDecimal::try_from(volume).unwrap(),
            BigDecimal::try_from(volume * maker_price).unwrap(),
        )
        .await;

        log!("Checking maker status..");
        check_my_swap_status(
            maker,
            uuid.as_ref(),
            BigDecimal::try_from(volume).unwrap(),
            BigDecimal::try_from(volume * maker_price).unwrap(),
        )
        .await;
    }
}

pub async fn test_qrc20_history_impl(local_start: Option<LocalStart>) {
    let passphrase = "daring blind measure rebuild grab boost fix favorite nurse stereo april rookie";
    let coins = json!([
        {"coin":"QRC20","required_confirmations":0,"pubtype": 120,"p2shtype": 50,"wiftype": 128,"txfee": 0,"mm2": 1,"mature_confirmations":2000,
         "protocol":{"type":"QRC20","protocol_data":{"platform":"QTUM","contract_address":"0xd362e096e873eb7907e205fadc6175c6fec7bc44"}}},
    ]);

    let mut mm = MarketMakerIt::start_async(
        json! ({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var ("BOB_TRADE_IP") .ok(),
            "rpcip": env::var ("BOB_TRADE_IP") .ok(),
            "passphrase": passphrase,
            "coins": coins,
            "rpc_password": "pass",
            "metrics_interval": 30.,
            "disable_p2p": true,
            "p2p_in_memory": false
        }),
        "pass".into(),
        local_start,
    )
    .await
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm.mm_dump();

    #[cfg(not(target_arch = "wasm32"))]
    common::log::info!("log path: {}", mm.log_path.display());

    mm.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))
        .await
        .unwrap();

    let electrum = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "electrum",
            "coin": "QRC20",
            "servers": tqtum_electrums(),
            "mm2": 1,
            "tx_history": true,
            "swap_contract_address": "0xd362e096e873eb7907e205fadc6175c6fec7bc44",
        }))
        .await
        .unwrap();
    assert_eq!(
        electrum.0,
        StatusCode::OK,
        "RPC «electrum» failed with status «{}», response «{}»",
        electrum.0,
        electrum.1
    );
    let electrum_json: Json = json::from_str(&electrum.1).unwrap();
    assert_eq!(
        electrum_json["address"].as_str(),
        Some("qfkXE2cNFEwPFQqvBcqs8m9KrkNa9KV4xi")
    );

    // Wait till tx_history will not be loaded
    mm.wait_for_log(22., |log| log.contains("history has been loaded successfully"))
        .await
        .unwrap();

    // let the MarketMaker save the history to the file
    Timer::sleep(1.).await;

    let tx_history = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "my_tx_history",
            "coin": "QRC20",
            "limit": 100,
        }))
        .await
        .unwrap();
    assert_eq!(
        tx_history.0,
        StatusCode::OK,
        "RPC «my_tx_history» failed with status «{}», response «{}»",
        tx_history.0,
        tx_history.1
    );
    debug!("{:?}", tx_history.1);
    let tx_history_json: Json = json::from_str(&tx_history.1).unwrap();
    let tx_history_result = &tx_history_json["result"];

    let mut expected = vec![
        // https://testnet.qtum.info/tx/45d722e615feb853d608033ffc20fd51c9ee86e2321cfa814ba5961190fb57d2
        "45d722e615feb853d608033ffc20fd51c9ee86e2321cfa814ba5961190fb57d200000000000000020000000000000000",
        // https://testnet.qtum.info/tx/45d722e615feb853d608033ffc20fd51c9ee86e2321cfa814ba5961190fb57d2
        "45d722e615feb853d608033ffc20fd51c9ee86e2321cfa814ba5961190fb57d200000000000000020000000000000001",
        // https://testnet.qtum.info/tx/abcb51963e720fdfed7b889cea79947ba3cabd7b8b384f6b5adb41a3f4b5d61b
        "abcb51963e720fdfed7b889cea79947ba3cabd7b8b384f6b5adb41a3f4b5d61b00000000000000020000000000000000",
        // https://testnet.qtum.info/tx/4ea5392d03a9c35126d2d5a8294c3c3102cfc6d65235897c92ca04c5515f6be5
        "4ea5392d03a9c35126d2d5a8294c3c3102cfc6d65235897c92ca04c5515f6be500000000000000020000000000000000",
        // https://testnet.qtum.info/tx/9156f5f1d3652c27dca0216c63177da38de5c9e9f03a5cfa278bf82882d2d3d8
        "9156f5f1d3652c27dca0216c63177da38de5c9e9f03a5cfa278bf82882d2d3d800000000000000020000000000000000",
        // https://testnet.qtum.info/tx/35e03bc529528a853ee75dde28f27eec8ed7b152b6af7ab6dfa5d55ea46f25ac
        "35e03bc529528a853ee75dde28f27eec8ed7b152b6af7ab6dfa5d55ea46f25ac00000000000000010000000000000000",
        // https://testnet.qtum.info/tx/39104d29d77ba83c5c6c63ab7a0f096301c443b4538dc6b30140453a40caa80a
        "39104d29d77ba83c5c6c63ab7a0f096301c443b4538dc6b30140453a40caa80a00000000000000000000000000000000",
        // https://testnet.qtum.info/tx/d9965e3496a8a4af2d462424b989694b3146d78c61654b99bbadba64464f75cb
        "d9965e3496a8a4af2d462424b989694b3146d78c61654b99bbadba64464f75cb00000000000000000000000000000000",
        // https://testnet.qtum.info/tx/c2f346d3d2aadc35f5343d0d493a139b2579175496d685ec30734d161e62f7a1
        "c2f346d3d2aadc35f5343d0d493a139b2579175496d685ec30734d161e62f7a100000000000000000000000000000000",
    ];

    assert_eq!(tx_history_result["total"].as_u64().unwrap(), expected.len() as u64);
    for tx in tx_history_result["transactions"].as_array().unwrap() {
        // pop front item
        let expected_tx = expected.remove(0);
        assert_eq!(tx["internal_id"].as_str().unwrap(), expected_tx);
    }
}

pub async fn get_locked_amount(mm: &MarketMakerIt, coin: &str) -> GetLockedAmountResponse {
    let request = json!({
        "userpass": mm.userpass,
        "method": "get_locked_amount",
        "mmrpc": "2.0",
        "params": {
            "coin": coin
        }
    });
    log!("get_locked_amount request {}", json::to_string(&request).unwrap());

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(request.0, StatusCode::OK, "'get_locked_amount' failed: {}", request.1);
    log!("get_locked_amount response {}", request.1);
    let response: RpcV2Response<GetLockedAmountResponse> = json::from_str(&request.1).unwrap();
    response.result
}

pub async fn coins_needed_for_kickstart(mm: &MarketMakerIt) -> Vec<String> {
    let request = json!({
        "userpass": mm.userpass,
        "method": "coins_needed_for_kick_start",
        "params": []
    });
    let response = mm.rpc(&request).await.unwrap();
    assert_eq!(
        response.0,
        StatusCode::OK,
        "'coins_needed_for_kick_start' failed: {}",
        response.1
    );
    let result: CoinsNeededForKickstartResponse = json::from_str(&response.1).unwrap();
    result.result
}

pub async fn enable_z_coin_light(
    mm: &MarketMakerIt,
    coin: &str,
    electrums: &[&str],
    lightwalletd_urls: &[&str],
    account: Option<u32>,
    starting_height: Option<u64>,
) -> ZCoinActivationResult {
    let init = init_z_coin_light(mm, coin, electrums, lightwalletd_urls, starting_height, account).await;
    let init: RpcV2Response<InitTaskResult> = json::from_value(init).unwrap();
    let timeout = wait_until_sec(300);

    loop {
        if now_sec() > timeout {
            panic!("{} initialization timed out", coin);
        }
        let status = init_z_coin_status(mm, init.result.task_id).await;
        info!("Status {}", json::to_string(&status).unwrap());
        let status: RpcV2Response<InitZcoinStatus> = json::from_value(status).unwrap();
        match status.result {
            InitZcoinStatus::Ok(result) => break result,
            InitZcoinStatus::Error(e) => panic!("{} initialization error {:?}", coin, e),
            _ => Timer::sleep(1.).await,
        }
    }
}

pub async fn get_new_address(
    mm: &MarketMakerIt,
    coin: &str,
    account_id: u32,
    chain: Option<Bip44Chain>,
) -> GetNewAddressResponse {
    let request = json!({
        "userpass": mm.userpass,
        "method": "get_new_address",
        "mmrpc": "2.0",
        "params": {
            "coin": coin,
            "account_id": account_id,
            "chain": chain
        }
    });

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(request.0, StatusCode::OK, "'get_new_address' failed: {}", request.1);
    let response: RpcV2Response<GetNewAddressResponse> = json::from_str(&request.1).unwrap();
    response.result
}

pub async fn account_balance(
    mm: &MarketMakerIt,
    coin: &str,
    account_index: u32,
    chain: Bip44Chain,
) -> HDAccountBalanceResponse {
    let request = json!({
        "userpass": mm.userpass,
        "method": "account_balance",
        "mmrpc": "2.0",
        "params": {
            "coin": coin,
            "account_index": account_index,
            "chain": chain
        }
    });

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(request.0, StatusCode::OK, "'account_balance' failed: {}", request.1);
    let response: RpcV2Response<HDAccountBalanceResponse> = json::from_str(&request.1).unwrap();
    response.result
}

pub async fn init_create_new_account(mm: &MarketMakerIt, coin: &str, account_id: Option<u32>) -> Json {
    let request = json!({
        "userpass": mm.userpass,
        "method": "task::create_new_account::init",
        "mmrpc": "2.0",
        "params": {
            "coin": coin,
            "account_id": account_id
        }
    });

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::create_new_account::init' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn create_new_account_status(mm: &MarketMakerIt, task_id: u64) -> Json {
    let request = json!({
        "userpass": mm.userpass,
        "method": "task::create_new_account::status",
        "mmrpc": "2.0",
        "params": {
            "task_id": task_id,
        }
    });

    let request = mm.rpc(&request).await.unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::create_new_account::status' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_parse_env_file() {
    let env_client =
        b"ALICE_PASSPHRASE=spice describe gravity federal blast come thank unfair canal monkey style afraid";
    let env_client_new_line =
        b"ALICE_PASSPHRASE=spice describe gravity federal blast come thank unfair canal monkey style afraid\n";
    let env_client_space =
        b"ALICE_PASSPHRASE=spice describe gravity federal blast come thank unfair canal monkey style afraid  ";

    let parsed1 = from_env_file(env_client.to_vec());
    let parsed2 = from_env_file(env_client_new_line.to_vec());
    let parsed3 = from_env_file(env_client_space.to_vec());
    assert_eq!(parsed1, parsed2);
    assert_eq!(parsed1, parsed3);
    assert_eq!(
        parsed1,
        (
            Some(String::from(
                "spice describe gravity federal blast come thank unfair canal monkey style afraid"
            )),
            None
        )
    );
}

/// test helper to call sign_raw_transaction rpc with utxo coin param
pub async fn test_sign_raw_transaction_rpc_helper(
    mm: &MarketMakerIt,
    expected_ret: StatusCode,
    json_params: &Json,
) -> Json {
    let response = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method":"sign_raw_transaction",
            "mmrpc":"2.0",
            "id": 0,
            "params": json_params
        }))
        .await
        .expect("sign_raw_transaction rpc result okay");
    assert_eq!(
        response.0, expected_ret,
        "'sign_raw_transaction' unexpected return code: {}",
        response.1
    );
    json::from_str(&response.1).expect("response to json conversion must be okay")
}

/// Helper to call init trezor rpc
pub async fn init_trezor_rpc(mm: &MarketMakerIt, coin: &str) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::init_trezor::init",
            "mmrpc": "2.0",
            "params": {
                "ticker": coin,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::init_trezor::init' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

/// Helper to call init trezor status
pub async fn init_trezor_status_rpc(mm: &MarketMakerIt, task_id: u64) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::init_trezor::status",
            "mmrpc": "2.0",
            "params": {
                "task_id": task_id,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::init_trezor::status' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn init_trezor_user_action_rpc(mm: &MarketMakerIt, task_id: u64, user_action: Json) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "task::init_trezor::user_action",
            "mmrpc": "2.0",
            "params": {
                "task_id": task_id,
                "user_action": user_action
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'task::init_trezor::user_action' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

pub async fn active_swaps(mm: &MarketMakerIt) -> ActiveSwapsResponse {
    let request = json!({
        "userpass": mm.userpass,
        "method": "active_swaps",
        "params": []
    });
    let response = mm.rpc(&request).await.unwrap();
    assert_eq!(response.0, StatusCode::OK, "'active_swaps' failed: {}", response.1);
    json::from_str(&response.1).unwrap()
}
