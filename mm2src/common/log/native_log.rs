use super::{format_record, LogCallback};
use std::env;
use std::io::Write;
use std::os::raw::c_char;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub enum LogLevel {
    /// A level lower than all log levels.
    Off = 0,
    /// Corresponds to the `ERROR` log level.
    Error = 1,
    /// Corresponds to the `WARN` log level.
    Warn = 2,
    /// Corresponds to the `INFO` log level.
    #[default]
    Info = 3,
    /// Corresponds to the `DEBUG` log level.
    Debug = 4,
    /// Corresponds to the `TRACE` log level.
    Trace = 5,
}

impl LogLevel {
    pub fn from_env() -> Option<LogLevel> {
        let env_val = std::env::var("RUST_LOG").ok()?;
        LogLevel::from_str(&env_val).ok()
    }
}

pub struct FfiCallback {
    cb_f: extern "C" fn(line: *const c_char),
}

impl FfiCallback {
    pub fn with_ffi_function(callback: extern "C" fn(line: *const c_char)) -> FfiCallback {
        FfiCallback { cb_f: callback }
    }
}

impl LogCallback for FfiCallback {
    fn callback(&mut self, _level: LogLevel, mut line: String) {
        line.push('\0');
        (self.cb_f)(line.as_ptr() as *const c_char)
    }
}

#[derive(Default)]
pub struct UnifiedLoggerBuilder {
    /// Prevents writing to stdout/err
    silent_console: bool,
}

impl UnifiedLoggerBuilder {
    pub fn new() -> UnifiedLoggerBuilder {
        UnifiedLoggerBuilder::default()
    }

    pub fn silent_console(mut self, silent_console: bool) -> UnifiedLoggerBuilder {
        self.silent_console = silent_console;
        self
    }

    pub fn init(self) {
        const MM2_LOG_ENV_KEY: &str = "RUST_LOG";

        if env::var_os(MM2_LOG_ENV_KEY).is_none() {
            env::set_var(MM2_LOG_ENV_KEY, "info");
        };

        let mut logger = env_logger::builder();

        logger.format(move |buf, record| {
            let log = format_record(record);

            if let Ok(mut log_file) = crate::LOG_FILE.lock() {
                if let Some(ref mut log_file) = *log_file {
                    writeln!(log_file, "{log}")?;
                }
            }

            if !self.silent_console {
                writeln!(buf, "{log}")?;
            }

            Ok(())
        });

        if let Err(e) = logger.try_init() {
            log::error!("env_logger is already initialized. {}", e);
        };
    }
}
