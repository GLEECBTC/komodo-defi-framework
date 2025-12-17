use super::{LogCallback, LOG_CALLBACK};
use crate::executor::spawn_local;
use crate::log::format_record;
use futures::channel::mpsc;
use futures::stream::StreamExt;
use log::{set_boxed_logger, set_max_level, LevelFilter, Log, Metadata, Record};
use serde_repr::*;
use wasm_bindgen::prelude::*;

pub use js_sys::Function as JsFunction;
pub use wasm_bindgen::JsValue;
pub use web_sys::console;

const DEFAULT_LEVEL_FILTER: LogLevel = LogLevel::Info;

#[macro_export]
macro_rules! console_err {
    ($($args: tt)+) => {{
        let here = format!("{}:{}]", $crate::filename(file!()), line!());
        let msg = format!($($args)+);
        let msg_formatted = format!("{} {}", here, msg);
        let msg_js = $crate::log::wasm_log::JsValue::from(msg_formatted);
        $crate::log::wasm_log::console::error_1(&msg_js);
    }};
}

#[macro_export]
macro_rules! console_info {
    ($($args: tt)+) => {{
        let here = format!("{}:{}]", $crate::filename(file!()), line!());
        let msg = format!($($args)+);
        let msg_formatted = format!("{} {}", here, msg);
        let msg_js = $crate::log::wasm_log::JsValue::from(msg_formatted);
        $crate::log::wasm_log::console::info_1(&msg_js);
    }};
}

#[macro_export]
macro_rules! console_log {
    ($($args: tt)+) => {{
        let here = format!("{}:{}]", $crate::filename(file!()), line!());
        let msg = format!($($args)+);
        let msg_formatted = format!("{} {}", here, msg);
        let msg_js = $crate::log::wasm_log::JsValue::from(msg_formatted);
        $crate::log::wasm_log::console::log_1(&msg_js);
    }};
}

const CHANNEL_BUF_SIZE: usize = 1024;

#[wasm_bindgen]
#[derive(Clone, Copy, Debug, Deserialize_repr, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum LogLevel {
    /// A level lower than all log levels.
    Off = 0,
    /// Corresponds to the `ERROR` log level.
    Error = 1,
    /// Corresponds to the `WARN` log level.
    Warn = 2,
    /// Corresponds to the `INFO` log level.
    Info = 3,
    /// Corresponds to the `DEBUG` log level.
    Debug = 4,
    /// Corresponds to the `TRACE` log level.
    Trace = 5,
}

impl Default for LogLevel {
    fn default() -> Self {
        DEFAULT_LEVEL_FILTER
    }
}

pub struct WasmCallback {
    tx: mpsc::Sender<CallbackMsg>,
}

impl LogCallback for WasmCallback {
    fn callback(&mut self, level: LogLevel, line: String) {
        let msg = CallbackMsg { level, line };
        if let Err(e) = self.tx.try_send(msg) {
            console_err!("!WasmCallback::handle: {}", e)
        }
    }
}

impl WasmCallback {
    pub fn with_js_function(cb: JsFunction) -> WasmCallback {
        let (tx, mut rx) = mpsc::channel(CHANNEL_BUF_SIZE);
        let fut = async move {
            let this = JsValue::null();
            // read until the channel is closed
            while let Some(CallbackMsg { level, line }) = rx.next().await {
                let level = JsValue::from(level);
                let line = JsValue::from(line);
                if let Err(e) = cb.call2(&this, &level, &line) {
                    console_err!("Couldn't invoke a JS callback: {:?}", e);
                }
            }
        };

        // There is no async operations within the loop except reading from the `rx` receiver.
        // So we're sure that once the `tx` sender is dropped, the loop will be stopped immediately.
        // Please note that `WasmCallBack` is set on MarketMaker start/restart.
        spawn_local(fut);
        WasmCallback { tx }
    }

    pub fn console_log() -> WasmCallback {
        let (tx, mut rx) = mpsc::channel(CHANNEL_BUF_SIZE);
        let fut = async move {
            // read until the channel is closed
            // pass the line to `console_log` always, because the `wasm_bindgen_tests` prints logs from `console_log` only
            while let Some(CallbackMsg { line, .. }) = rx.next().await {
                let msg_js = JsValue::from(line);
                console::log_1(&msg_js);
            }
        };

        // The future can be spawned safely since it doesn't hold any shared pointer,
        // and will be stopped immediately once `WasmCallback` is dropped.
        // Please note that `WasmCallBack` is set on MarketMaker start/restart.
        spawn_local(fut);
        WasmCallback { tx }
    }
}

pub struct WasmLoggerBuilder {
    filter: LogLevel,
}

impl Default for WasmLoggerBuilder {
    fn default() -> Self {
        WasmLoggerBuilder {
            filter: DEFAULT_LEVEL_FILTER,
        }
    }
}

impl WasmLoggerBuilder {
    pub fn level_filter(mut self, filter: LogLevel) -> WasmLoggerBuilder {
        self.filter = filter;
        self
    }

    pub fn try_init(self) -> Result<(), String> {
        let logger = WasmLogger { filter: self.filter };
        let max_level = LevelFilter::from(self.filter);
        set_max_level(max_level);
        set_boxed_logger(Box::new(logger)).map_err(|e| ERRL!("{}", e))
    }
}

/// Replace `WasmCallback` into the `WasmLogger` when the `log!` macro is gone.
struct WasmLogger {
    filter: LogLevel,
}

impl Log for WasmLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        LogLevel::from(metadata.level()) <= self.filter
    }

    fn log(&self, record: &Record) {
        if let Some(ref mut log_cb) = *LOG_CALLBACK.lock() {
            let line = format_record(record);
            let level = LogLevel::from(record.level());
            log_cb.callback(level, line);
        }
    }

    fn flush(&self) {}
}

struct CallbackMsg {
    level: LogLevel,
    line: String,
}

pub fn register_wasm_log() {
    use crate::log::register_callback;
    use std::str::FromStr;
    use std::sync::atomic::{AtomicBool, Ordering};

    static IS_INITIALIZED: AtomicBool = AtomicBool::new(false);

    // Check if the logger is initialized already
    if let Err(true) = IS_INITIALIZED.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed) {
        return;
    }

    let log_level = match option_env!("RUST_WASM_TEST_LOG") {
        Some(level_str) => LogLevel::from_str(level_str).unwrap_or(LogLevel::Info),
        None => LogLevel::Info,
    };

    register_callback(WasmCallback::console_log());
    WasmLoggerBuilder::default()
        .level_filter(log_level)
        .try_init()
        .expect("Must be initialized only once");
}
