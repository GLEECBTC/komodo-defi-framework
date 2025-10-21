/******************************************************************************
 * Copyright © 2023 Pampex LTD and TillyHK LTD                                *
 *                                                                            *
 * See the CONTRIBUTOR-LICENSE-AGREEMENT, COPYING, LICENSE-COPYRIGHT-NOTICE   *
 * and DEVELOPER-CERTIFICATE-OF-ORIGIN files in the LEGAL directory in        *
 * the top-level directory of this distribution for the individual copyright  *
 * holder information and the developer policies on copyright and licensing.  *
 *                                                                            *
 * Unless otherwise agreed in a custom licensing agreement, no part of the    *
 * Komodo DeFi Framework software, including this file may be copied, modified, propagated*
 * or distributed except according to the terms contained in the              *
 * LICENSE-COPYRIGHT-NOTICE file.                                             *
 *                                                                            *
 * Removal or modification of this copyright notice is prohibited.            *
 *                                                                            *
 ******************************************************************************/
//
//  mm2.rs
//  marketmaker
//
//  Copyright © 2023 Pampex LTD and TillyHK LTD. All rights reserved.
//

// `mockable` implementation uses these
#![allow(
    forgetting_references,
    forgetting_copy_types,
    clippy::swap_ptr_to_ref,
    clippy::forget_non_drop,
    clippy::let_unit_value
)]
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]
#![cfg_attr(target_arch = "wasm32", allow(unused_imports))]

#[macro_use]
extern crate common;
#[macro_use]
extern crate gstuff;
#[macro_use]
extern crate serde_json;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate ser_error_derive;
#[cfg(test)]
extern crate mm2_test_helpers;

#[cfg(not(target_arch = "wasm32"))]
use common::block_on;
use common::crash_reports::init_crash_reports;
use common::executor::Timer;
use common::log;
use common::log::LogLevel;
use common::password_policy::password_policy;
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};

#[cfg(any(feature = "custom-swap-locktime", test, feature = "run-docker-tests"))]
use common::log::warn;
#[cfg(any(feature = "custom-swap-locktime", test, feature = "run-docker-tests"))]
use lp_swap::PAYMENT_LOCKTIME;
#[cfg(any(feature = "custom-swap-locktime", test, feature = "run-docker-tests"))]
use std::sync::atomic::Ordering;

use gstuff::slurp;
use serde_json::{self as json, Value as Json};

use std::process::exit;
use std::str;

pub use self::lp_native_dex::init_hw;
pub use self::lp_native_dex::lp_init;
use mm2_err_handle::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod database;

pub mod heartbeat_event;
pub mod lp_dispatcher;
pub mod lp_healthcheck;
pub mod lp_message_service;
mod lp_native_dex;
pub mod lp_network;
pub mod lp_ordermatch;
pub mod lp_stats;
pub mod lp_swap;
pub mod lp_wallet;
pub mod rpc;
mod swap_versioning;
#[cfg(all(target_arch = "wasm32", test))]
mod wasm_tests;

use clap::Parser;

pub const PASSWORD_MAXIMUM_CONSECUTIVE_CHARACTERS: usize = 3;

#[cfg(any(feature = "custom-swap-locktime", test, feature = "run-docker-tests"))]
const CUSTOM_PAYMENT_LOCKTIME_DEFAULT: u64 = 900;

const EXTRA_HELP_MESSAGE: &str = r#"
Environment variables:

  MM_CONF_PATH   ..  File path. MM2 will try to load the JSON configuration from this file.
                     File must contain valid json with structure mentioned above.
                     Defaults to `MM2.json`
  MM_COINS_PATH  ..  File path. MM2 will try to load coins data from this file.
                     File must contain valid json.
                     Recommended: https://github.com/komodoplatform/coins/blob/master/coins.
                     Defaults to `coins`.
  MM_LOG         ..  File path. Must end with '.log'. MM will log to this file.

See also the online documentation at
https://komodoplatform.com/en/docs
"#;

#[derive(Parser, Debug)]
#[command(about="Komodo DeFi Framework Daemon", long_about=None, after_help=EXTRA_HELP_MESSAGE)]
pub struct Cli {
    /// JSON configuration string - will be used instead of the json config file
    pub config: Option<String>,

    /// Print version
    #[clap(short, long)]
    pub version: bool,
}

pub struct LpMainParams {
    conf: Json,
    filter: Option<LogLevel>,
}

impl LpMainParams {
    pub fn with_conf(conf: Json) -> LpMainParams {
        LpMainParams { conf, filter: None }
    }

    pub fn log_filter(mut self, filter: Option<LogLevel>) -> LpMainParams {
        self.filter = filter;
        self
    }
}

#[cfg(any(feature = "custom-swap-locktime", test, feature = "run-docker-tests"))]
/// Reads `payment_locktime` from conf arg and assigns it into `PAYMENT_LOCKTIME` in lp_swap.
/// Assigns 900 if `payment_locktime` is invalid or not provided.
fn initialize_payment_locktime(conf: &Json) {
    match conf["payment_locktime"].as_u64() {
        Some(lt) => PAYMENT_LOCKTIME.store(lt, Ordering::Relaxed),
        None => {
            warn!(
                "payment_locktime is either invalid type or not provided in the configuration or
                MM2.json file. payment_locktime will be proceeded as {} seconds.",
                CUSTOM_PAYMENT_LOCKTIME_DEFAULT
            );
        },
    };
}

/// * `ctx_cb` - callback used to share the `MmCtx` ID with the call site.
pub async fn lp_main(
    params: LpMainParams,
    ctx_cb: &dyn Fn(u32),
    version: String,
    datetime: String,
) -> Result<MmArc, String> {
    let log_filter = params.filter.unwrap_or_default();
    // Logger can be initialized once.
    // If `kdf` is linked as a library, and `kdf` is restarted, `init_logger` returns an error.
    init_logger(log_filter, params.conf["silent_console"].as_bool().unwrap_or_default()).ok();

    let conf = params.conf;
    if !conf["rpc_password"].is_null() {
        if !conf["rpc_password"].is_string() {
            return ERR!("rpc_password must be string");
        }

        let is_weak_password_accepted = conf["allow_weak_password"].as_bool() == Some(true);

        if conf["rpc_password"].as_str() == Some("") {
            return ERR!("rpc_password must not be empty");
        }

        if !is_weak_password_accepted && cfg!(not(test)) {
            match password_policy(conf["rpc_password"].as_str().unwrap()) {
                Ok(_) => {},
                Err(err) => return Err(format!("{err}")),
            }
        }
    }

    #[cfg(any(feature = "custom-swap-locktime", test, feature = "run-docker-tests"))]
    initialize_payment_locktime(&conf);

    let ctx = MmCtxBuilder::new()
        .with_conf(conf)
        .with_log_level(log_filter)
        .with_version(version.clone())
        .with_datetime(datetime.clone())
        .into_mm_arc();
    ctx_cb(try_s!(ctx.ffi_handle()));

    #[cfg(not(target_arch = "wasm32"))]
    spawn_ctrl_c_handler(ctx.clone());

    try_s!(lp_init(ctx.clone(), version, datetime).await);
    Ok(ctx)
}

pub async fn lp_run(ctx: MmArc) {
    // In the mobile version we might depend on `lp_init` staying around until the context stops.
    loop {
        if ctx.is_stopping() {
            break;
        };
        Timer::sleep(0.2).await
    }

    // Clearing up the running swaps removes any circular references that might prevent the context from being dropped.
    lp_swap::clear_running_swaps(&ctx);
}

/// Handles CTRL-C signals and shutdowns the KDF runtime gracefully.
///
/// It's important to spawn this task as soon as `Ctx` is in the correct state.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_ctrl_c_handler(ctx: MmArc) {
    use crate::lp_dispatcher::{dispatch_lp_event, StopCtxEvent};

    common::executor::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Couldn't listen for the CTRL-C signal.");

        log::info!("Wrapping things up and shutting down...");

        dispatch_lp_event(ctx.clone(), StopCtxEvent.into()).await;
        ctx.stop().await.expect("Couldn't stop the KDF runtime.");
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)] // Not used by mm2_lib.
pub fn mm2_main(version: String, datetime: String) {
    init_crash_reports();

    let cli = Cli::parse();
    if cli.version {
        println!("Komodo DeFi Framework: {version}");
        return;
    }

    let json_config = cli.config.as_deref();

    log!("Komodo DeFi Framework {} DT {}", version, datetime);

    if let Err(err) = run_lp_main(json_config, &|_| (), version, datetime) {
        log!("{}", err);
        exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Parses and returns the `json_config` as JSON.
/// Attempts to load the config from `MM2.json` file if `json_config` is None
pub fn get_mm2config(json_config: Option<&str>) -> Result<Json, String> {
    let conf = match json_config {
        Some(s) => s.to_owned(),
        None => {
            let conf_path = common::kdf_config_file().map_err(|e| e.to_string())?;
            let conf_from_file = slurp(&conf_path);

            if conf_from_file.is_empty() {
                return ERR!(
                    "Config is not set from command line arg and {} file doesn't exist.",
                    conf_path.display()
                );
            }
            try_s!(String::from_utf8(conf_from_file))
        },
    };

    let mut conf: Json = match json::from_str(&conf) {
        Ok(json) => json,
        // Syntax or io errors may include the conf string in the error message so we don't want to take risks and show these errors internals in the log.
        // If new variants are added to the Error enum, there can be a risk of exposing the conf string in the error message when updating serde_json so
        // I think it's better to not include the serde_json::error::Error at all in the returned error message rather than selectively excluding certain variants.
        Err(_) => return ERR!("Couldn't parse mm2 config to JSON format!"),
    };

    if conf["coins"].is_null() {
        let coins_path = common::kdf_coins_file().map_err(|e| e.to_string())?;

        let coins_from_file = slurp(&coins_path);
        if coins_from_file.is_empty() {
            return ERR!(
                "No coins are set in JSON config and '{}' file doesn't exist",
                coins_path.display()
            );
        }
        conf["coins"] = match json::from_slice(&coins_from_file) {
            Ok(j) => j,
            Err(e) => {
                return ERR!(
                    "Error {} parsing the coins file, please ensure it contains valid json",
                    e
                )
            },
        }
    }

    Ok(conf)
}

/// Runs LP_main with result of `get_mm2config()`.
///
/// * `ctx_cb` - Invoked with the MM context handle,
///   allowing the `run_lp_main` caller to communicate with MM.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_lp_main(
    first_arg: Option<&str>,
    ctx_cb: &dyn Fn(u32),
    version: String,
    datetime: String,
) -> Result<(), String> {
    let conf = get_mm2config(first_arg)?;

    let log_filter = LogLevel::from_env();

    let params = LpMainParams::with_conf(conf).log_filter(log_filter);
    let ctx = try_s!(block_on(lp_main(params, ctx_cb, version, datetime)));
    block_on(lp_run(ctx));
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn init_logger(_level: LogLevel, silent_console: bool) -> Result<(), String> {
    common::log::UnifiedLoggerBuilder::default()
        .silent_console(silent_console)
        .init();

    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn init_logger(level: LogLevel, _silent_console: bool) -> Result<(), String> {
    common::log::WasmLoggerBuilder::default().level_filter(level).try_init()
}
