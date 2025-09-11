//! A common dependency for subcrates.
//!
//!                   common
//!                     ^
//!                     |
//!     subcrate A   ---+---   subcrate B
//!         ^                      ^
//!         |                      |
//!         +-----------+----------+
//!                     |
//!                   binary

#![allow(uncommon_codepoints)]

#[macro_use]
extern crate arrayref;
#[macro_use]
extern crate gstuff;
#[macro_use]
extern crate lazy_static;
#[macro_use]
pub extern crate serde_derive;
#[macro_use]
pub extern crate serde_json;
#[macro_use]
extern crate ser_error_derive;

/// Implements a `From` for `enum` with a variant name matching the name of the type stored.
///
/// This is helpful as a workaround for the lack of datasort refinements.
/// And also as a simpler alternative to `enum_dispatch` and `enum_derive`.
///
///     enum Color {Red (Red)}
///     ifrom! (Color, Red);
#[macro_export]
macro_rules! ifrom {
    ($enum: ident, $id: ident) => {
        impl From<$id> for $enum {
            fn from(t: $id) -> $enum {
                $enum::$id(t)
            }
        }
    };
}

/// This macro is used to implement `From<$t>` for `$name`, where `$name($inner)`.
#[macro_export]
macro_rules! ifrom_inner {
    ($name:ident, $inner:ident, $($t:ty)*) => ($(
        impl From<$t> for $name {
            fn from(num: $t) -> $name { $name($inner::from(num)) }
        }
    )*);
}

#[macro_export]
macro_rules! cfg_wasm32 {
    ($($tokens:tt)*) => {
        cfg_if::cfg_if! {
            if #[cfg(target_arch = "wasm32")] {
                $($tokens)*
            }
        }
    };
}

#[macro_export]
macro_rules! cfg_native {
    ($($tokens:tt)*) => {
        cfg_if::cfg_if! {
            if #[cfg(not(target_arch = "wasm32"))] {
                $($tokens)*
            }
        }
    };
}

/// Returns a JSON error HyRes on a failure.
#[macro_export]
macro_rules! try_h {
    ($e: expr) => {
        match $e {
            Ok(ok) => ok,
            Err(err) => return $crate::rpc_err_response(500, &ERRL!("{}", err)),
        }
    };
}

/// Drops mutability of given variable
#[macro_export]
macro_rules! drop_mutability {
    ($t: ident) => {
        let $t = $t;
    };
}

/// Reads inner value of `Option<T>`, returns `Ok(None)` otherwise.
#[macro_export]
macro_rules! some_or_return_ok_none {
    ($val:expr) => {
        match $val {
            Some(t) => t,
            None => {
                return Ok(None);
            },
        }
    };
}

#[macro_export]
macro_rules! cross_test {
    ($test_name:ident, $test_code:block) => {
        cross_test!($test_name, $test_code, not(target_arch = "wasm32"));
    };

    ($test_name:ident, $test_code:block, $($cfgs:meta),+) => {
        $(
            #[cfg($cfgs)]
            #[tokio::test(flavor = "multi_thread")]
            async fn $test_name() { $test_code }
        )+

        #[cfg(target_arch = "wasm32")]
        #[wasm_bindgen_test]
        async fn $test_name() { $test_code }
    };
}

#[macro_use]
pub mod jsonrpc_client;
#[macro_use]
pub mod write_safe;
#[macro_use]
pub mod log;

pub mod bool_as_int;
pub mod crash_reports;
pub mod custom_futures;
pub mod custom_iter;
#[path = "executor/mod.rs"]
pub mod executor;
pub mod notifier;
pub mod number_type_casting;
pub mod on_drop_callback;
pub mod password_policy;
pub mod seri;

#[cfg(not(target_arch = "wasm32"))]
#[path = "wio.rs"]
pub mod wio;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

#[cfg(target_arch = "wasm32")]
pub use wasm::*;

use backtrace::SymbolName;
use chrono::format::ParseError;
use chrono::{DateTime, Utc};
use derive_more::Display;
pub use futures::compat::Future01CompatExt;
use futures01::{future, Future};
use http::header::CONTENT_TYPE;
use http::Response;
use parking_lot::{Mutex as PaMutex, MutexGuard as PaMutexGuard};
pub use paste::paste;
use primitive_types::U256;
use rand::RngCore;
use rand::{rngs::SmallRng, SeedableRng};
use serde::{de, ser};
use serde_json::{self as json, Value as Json};
use sha2::{Digest, Sha256};
use std::convert::TryInto;
use std::fmt::Write as FmtWrite;
use std::fs::File;
use std::future::Future as Future03;
use std::io::{self, BufReader, Read, Write};
use std::iter::Peekable;
use std::mem::{forget, zeroed};
use std::num::{NonZeroUsize, TryFromIntError};
use std::ops::{Add, Deref, Div, RangeInclusive};
use std::os::raw::c_void;
use std::panic::{set_hook, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::ptr::read_volatile;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, SystemTimeError};
use uuid::Uuid;

pub use http::StatusCode;
pub use serde;

cfg_native! {
    pub use gstuff::{now_float, now_ms};
    #[cfg(not(windows))]
    use findshlibs::{IterationControl, Segment, SharedLibrary, TargetSharedLibrary};
    use std::env;
    use std::str::FromStr;
}

cfg_wasm32! {
    use std::sync::atomic::AtomicUsize;
}

// Directory used to store configuration and database files within the user's home directory.
#[cfg(not(target_arch = "wasm32"))]
const KOMODO_DEFI_FRAMEWORK_DIR_NAME: &str = ".kdf";

pub const X_GRPC_WEB: &str = "x-grpc-web";
pub const X_AUTH_PAYLOAD: &str = "X-Auth-Payload";
pub const APPLICATION_JSON: &str = "application/json";
pub const APPLICATION_GRPC_WEB: &str = "application/grpc-web";
pub const APPLICATION_GRPC_WEB_PROTO: &str = "application/grpc-web+proto";
pub const APPLICATION_GRPC_WEB_TEXT: &str = "application/grpc-web-text";
pub const APPLICATION_GRPC_WEB_TEXT_PROTO: &str = "application/grpc-web-text+proto";

pub const SATOSHIS: u64 = 100_000_000;

/// Dex fee public key for chains where SECP256K1 is supported
pub const DEX_FEE_ADDR_PUBKEY: &str = "03bc2c7ba671bae4a6fc835244c9762b41647b9827d4780a89a949b984a8ddcc06";
/// Public key to collect the burn part of dex fee, for chains where SECP256K1 is supported
pub const DEX_BURN_ADDR_PUBKEY: &str = "0369aa10c061cd9e085f4adb7399375ba001b54136145cb748eb4c48657be13153";

pub const PROXY_REQUEST_EXPIRATION_SEC: i64 = 15;

lazy_static! {
    pub static ref DEX_FEE_ADDR_RAW_PUBKEY: Vec<u8> =
        hex::decode(DEX_FEE_ADDR_PUBKEY).expect("DEX_FEE_ADDR_PUBKEY is expected to be a hexadecimal string");
    pub static ref DEX_BURN_ADDR_RAW_PUBKEY: Vec<u8> =
        hex::decode(DEX_BURN_ADDR_PUBKEY).expect("DEX_BURN_ADDR_PUBKEY is expected to be a hexadecimal string");
}

#[cfg(not(target_arch = "wasm32"))]
lazy_static! {
    pub(crate) static ref LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(open_log_file());
}

/// Converts u64 satoshis to f64
pub fn sat_to_f(sat: u64) -> f64 {
    sat as f64 / SATOSHIS as f64
}

/// Marker type to indicate that a type is `!Send` in a stable way.
///
/// Raw pointers are not `Send` by default, so this makes `NotSend`
/// not `Send` either.
pub struct NotSend(std::marker::PhantomData<*const ()>);

impl Default for NotSend {
    fn default() -> NotSend {
        NotSend(std::marker::PhantomData)
    }
}

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct bits256 {
    pub bytes: [u8; 32],
}

impl Default for bits256 {
    fn default() -> bits256 {
        bits256 {
            bytes: unsafe { zeroed() },
        }
    }
}

impl std::fmt::Display for bits256 {
    fn fmt(&self, fm: &mut std::fmt::Formatter) -> std::fmt::Result {
        for &ch in self.bytes.iter() {
            fn hex_from_digit(num: u8) -> char {
                if num < 10 {
                    (b'0' + num) as char
                } else {
                    (b'a' + num - 10) as char
                }
            }
            fm.write_char(hex_from_digit(ch / 16))?;
            fm.write_char(hex_from_digit(ch % 16))?;
        }
        Ok(())
    }
}

impl ser::Serialize for bits256 {
    fn serialize<S>(&self, se: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        se.serialize_bytes(&self.bytes[..])
    }
}

impl<'de> de::Deserialize<'de> for bits256 {
    fn deserialize<D>(deserializer: D) -> Result<bits256, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        struct Bits256Visitor;
        impl<'de> de::Visitor<'de> for Bits256Visitor {
            type Value = bits256;
            fn expecting(&self, fm: &mut std::fmt::Formatter) -> std::fmt::Result {
                fm.write_str("a byte array")
            }
            fn visit_seq<S>(self, mut seq: S) -> Result<bits256, S::Error>
            where
                S: de::SeqAccess<'de>,
            {
                let mut bytes: [u8; 32] = [0; 32];
                let mut pos = 0;
                while let Some(byte) = seq.next_element()? {
                    if pos >= bytes.len() {
                        return Err(de::Error::custom("bytes length > 32"));
                    }
                    bytes[pos] = byte;
                    pos += 1;
                }
                Ok(bits256 { bytes })
            }
            fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v.len() != 32 {
                    return Err(de::Error::custom("bytes length <> 32"));
                }
                Ok(bits256 {
                    bytes: *array_ref![v, 0, 32],
                })
            }
        }
        deserializer.deserialize_bytes(Bits256Visitor)
    }
}

impl std::fmt::Debug for bits256 {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        (self as &dyn std::fmt::Display).fmt(f)
    }
}

impl From<[u8; 32]> for bits256 {
    fn from(bytes: [u8; 32]) -> Self {
        bits256 { bytes }
    }
}

/// Use the value, preventing the compiler and linker from optimizing it away.
pub fn black_box<T>(v: T) -> T {
    // https://github.com/rust-lang/rfcs/issues/1484#issuecomment-240853111
    //std::hint::black_box (v)

    let ret = unsafe { read_volatile(&v) };
    forget(v);
    ret
}

/// Using a static buffer in order to minimize the chance of heap and stack allocations in the signal handler.
fn trace_buf() -> PaMutexGuard<'static, [u8; 256]> {
    static TRACE_BUF: OnceLock<PaMutex<[u8; 256]>> = OnceLock::new();
    TRACE_BUF.get_or_init(|| PaMutex::new([0; 256])).lock()
}

fn trace_name_buf() -> PaMutexGuard<'static, [u8; 128]> {
    static TRACE_NAME_BUF: OnceLock<PaMutex<[u8; 128]>> = OnceLock::new();
    TRACE_NAME_BUF.get_or_init(|| PaMutex::new([0; 128])).lock()
}

/// Shortcut to path->filename conversion.
///
/// # Notes
///
/// Returns the file name without extension if only the file name ends on `.rs`.
/// Returns the unchanged `path` if there is a character encoding error or something.
///
/// Inspired by https://docs.rs/gstuff/latest/gstuff/fn.filename.html
pub fn filename(path: &str) -> &str {
    // NB: `Path::new (path) .file_name()` only works for file separators of the current operating system,
    // whereas the error trace might be coming from another operating system.
    // In particular, I see `file_name` failing with WASM.

    let name = match path.rfind(['/', '\\']) {
        Some(ofs) => &path[ofs + 1..],
        None => path,
    };

    if name.ends_with(".rs") {
        &name[0..name.len() - 3]
    } else {
        name
    }
}

/// Formats a stack frame.
/// Some common and less than useful frames are skipped.
pub fn stack_trace_frame(instr_ptr: *mut c_void, buf: &mut dyn Write, symbol: &backtrace::Symbol) {
    let filename = match symbol.filename() {
        Some(path) => match path.components().next_back() {
            Some(c) => c.as_os_str().to_string_lossy(),
            None => "??".into(),
        },
        None => "??".into(),
    };
    let lineno = symbol.lineno().unwrap_or(0);
    let name = match symbol.name() {
        Some(name) => name,
        None => SymbolName::new(&[]),
    };
    let mut name_buf = trace_name_buf();
    let name = gstring!(name_buf, {
        let _ = write!(name_buf, "{name}"); // NB: `fmt` is different from `SymbolName::as_str`.
    });

    // Skip common and less than informative frames.

    match name {
        "common::crash_reports::rust_seh_handler"
        | "veh_exception_filter"
        | "common::stack_trace"
        // Super-main on Windows.
        | "__scrt_common_main_seh" => return,
        _ => (),
    }

    match filename.as_ref() {
        "boxed.rs" | "panic.rs" => return,
        _ => (),
    }

    if name.starts_with("alloc::")
        || name.starts_with("backtrace::")
        || name.starts_with("common::set_panic_hook")
        || name.starts_with("common::stack_trace")
        || name.starts_with("core::ops::")
        || name.starts_with("futures::")
        || name.starts_with("hyper::")
        || name.starts_with("common::crash_reports::signal_handler")
        || name.starts_with("panic_unwind::")
        || name.starts_with("std::")
        || name.starts_with("scoped_tls::")
        || name.starts_with("test::run_test::")
        || name.starts_with("tokio::")
        || name.starts_with("tokio_core::")
        || name.starts_with("tokio_reactor::")
        || name.starts_with("tokio_executor::")
        || name.starts_with("tokio_timer::")
    {
        return;
    }

    let _ = writeln!(buf, "  {filename}:{lineno}] {name} {instr_ptr:?}");
}

/// Generates a string with the current stack trace.
///
/// To get a simple stack trace:
///
///     let mut trace = String::with_capacity (4096);
///     stack_trace (&mut stack_trace_frame, &mut |l| trace.push_str (l));
///
/// * `format` - Generates the string representation of a frame.
/// * `output` - Function used to print the stack trace.
///   Printing immediately, without buffering, should make the tracing somewhat more reliable.
pub fn stack_trace(
    format: &mut dyn FnMut(*mut c_void, &mut dyn Write, &backtrace::Symbol),
    output: &mut dyn FnMut(&str),
) {
    // cf. https://github.com/rust-lang/rust/pull/64154 (standard library backtrace)

    backtrace::trace(|frame| {
        backtrace::resolve(frame.ip(), |symbol| {
            let mut trace_buf = trace_buf();
            let trace = gstring!(trace_buf, {
                // frame.ip() is next instruction pointer typically so offset(-1) points to current instruction
                format(frame.ip().wrapping_offset(-1), trace_buf, symbol);
            });
            output(trace);
        });
        true
    });

    // not(wasm) and not(windows)
    #[cfg(not(any(target_arch = "wasm32", windows)))]
    output_pc_mem_addr(output)
}

// not(wasm) and not(windows)
#[cfg(not(any(target_arch = "wasm32", windows)))]
fn output_pc_mem_addr(output: &mut dyn FnMut(&str)) {
    TargetSharedLibrary::each(|shlib| {
        let mut trace_buf = trace_buf();
        let name = gstring!(trace_buf, {
            let _ = write!(
                trace_buf,
                "Virtual memory addresses of {}",
                shlib.name().to_string_lossy()
            );
        });
        output(name);
        for seg in shlib.segments() {
            let segment = gstring!(trace_buf, {
                let _ = write!(
                    trace_buf,
                    "  {}:{}",
                    seg.name(),
                    seg.actual_virtual_memory_address(shlib)
                );
            });
            output(segment);
        }
        // First TargetSharedLibrary is initial executable, we are not interested in other libs
        IterationControl::Break
    });
}

/// Set up a panic hook that prints the panic location, the message and the backtrace.
/// (The default Rust handler doesn't have the means to print the message).
#[cfg(target_arch = "wasm32")]
pub fn set_panic_hook() {
    set_hook(Box::new(|info: &PanicHookInfo| {
        let mut trace = String::new();
        stack_trace(&mut stack_trace_frame, &mut |l| trace.push_str(l));
        console_err!("{}", info);
        console_err!("backtrace\n{}", trace);
    }))
}

/// Sets our own panic handler using patched backtrace crate. It was discovered that standard Rust panic
/// handlers print only "unknown" in Android backtraces which is not helpful.
/// Using custom hook with patched backtrace version solves this issue.
/// NB: https://github.com/rust-lang/backtrace-rs/issues/227
#[cfg(not(target_arch = "wasm32"))]
pub fn set_panic_hook() {
    use std::sync::atomic::AtomicBool;

    thread_local! {static ENTERED: AtomicBool = const { AtomicBool::new(false) };}

    set_hook(Box::new(|info: &PanicHookInfo| {
        // Stack tracing and logging might panic (in `println!` for example).
        // Let us detect this and do nothing on second panic.
        // We'll likely still get a crash after the hook is finished
        // (experimenting with this I'm getting the "thread panicked while panicking. aborting." on Windows)
        // but that crash will have a better stack trace compared to the one with deep hook recursion.
        if let Ok(Err(_)) = ENTERED.try_with(|e| e.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed))
        {
            return;
        }

        let mut trace = String::new();
        stack_trace(&mut stack_trace_frame, &mut |l| trace.push_str(l));
        log!("{}", info);
        log!("backtrace\n{}", trace);

        let _ = ENTERED.try_with(|e| e.compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed));
    }))
}

/// RPC response, returned by the RPC handlers.
/// NB: By default the future is executed on the shared asynchronous reactor (`CORE`),
/// the handler is responsible for spawning the future on another reactor if it doesn't fit the `CORE` well.
pub type HyRes = Box<dyn Future<Item = Response<Vec<u8>>, Error = String> + Send>;

pub type BoxFut<T, E> = Box<dyn Future<Item = T, Error = E> + Send>;

pub trait HttpStatusCode {
    fn status_code(&self) -> StatusCode;
}

/// Wraps a JSON string into the `HyRes` RPC response future.
pub fn rpc_response<T>(status: u16, body: T) -> HyRes
where
    Vec<u8>: From<T>,
{
    let rf = match Response::builder()
        .status(status)
        .header(CONTENT_TYPE, APPLICATION_JSON)
        .body(Vec::from(body))
    {
        Ok(r) => future::ok::<Response<Vec<u8>>, String>(r),
        Err(err) => {
            let err = ERRL!("{}", err);
            future::err::<Response<Vec<u8>>, String>(json!({ "error": err }).to_string())
        },
    };
    Box::new(rf)
}

/// An alternative to the `std::convert::Infallible` that implements `Serialize`.
/// Replace it with `!` when it's stable.
#[derive(Clone, Deserialize, Serialize)]
pub enum SerdeInfallible {}

/// An mmrpc 2.0 compatible error variant that is used when the serialization of an RPC response is failed.
#[derive(Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum SerializationError {
    InternalError(String),
}

impl std::fmt::Display for SerializationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SerializationError::InternalError(internal) => {
                write!(f, "Internal error: Couldn't serialize an RPC response: {internal}")
            },
        }
    }
}

impl SerializationError {
    pub fn from_error<E: ser::Error>(e: E) -> SerializationError {
        SerializationError::InternalError(e.to_string())
    }
}

#[derive(Clone, Serialize)]
pub struct SuccessResponse(&'static str);

impl SuccessResponse {
    pub fn new() -> SuccessResponse {
        SuccessResponse("success")
    }
}

impl Default for SuccessResponse {
    fn default() -> Self {
        SuccessResponse::new()
    }
}

#[derive(Serialize)]
struct ErrResponse {
    error: String,
}

/// Converts the given `err` message into the `{error: $err}` JSON string.
pub fn err_to_rpc_json_string(err: &str) -> String {
    let err = ErrResponse { error: err.to_owned() };
    json::to_string(&err).unwrap()
}

pub fn err_tp_rpc_json(error: String) -> Json {
    json::to_value(ErrResponse { error }).unwrap()
}

/// Returns the `{error: $msg}` JSON response with the given HTTP `status`.
/// Also logs the error (if possible).
pub fn rpc_err_response(status: u16, msg: &str) -> HyRes {
    // TODO: Like in most other places, we should check for a thread-local access to the proper log here.
    // Might be a good idea to use emoji too, like "🤒" or "🤐" or "😕".
    // TODO: Consider turning this into a macros or merging with `try_h` in order to retain the `line!`.
    log::error!("RPC error response: {}", msg);

    rpc_response(status, err_to_rpc_json_string(msg))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn var(name: &str) -> Result<String, String> {
    match env::var(name) {
        Ok(v) => Ok(v),
        Err(_err) => ERR!("No {}", name),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn env_var_as_bool(name: &str) -> bool {
    match env::var(name) {
        Ok(v) => FromStr::from_str(&v).unwrap_or_default(),
        Err(_err) => false,
    }
}

#[cfg(target_arch = "wasm32")]
pub fn env_var_as_bool(_name: &str) -> bool {
    false
}

/// TODO make it wasm32 only
#[cfg(target_arch = "wasm32")]
pub fn var(_name: &str) -> Result<String, String> {
    ERR!("Environment variable not supported in WASM")
}

/// Runs the given future on MM2's executor and waits for the result.
///
/// This is compatible with futures 0.1.
pub fn block_on_f01<F>(f: F) -> Result<F::Item, F::Error>
where
    F: Future,
{
    block_on(f.compat())
}

#[cfg(not(target_arch = "wasm32"))]
/// Runs the given future on MM2's executor and waits for the result.
///
/// This is compatible with futures 0.3.
pub fn block_on<F>(f: F) -> F::Output
where
    F: Future03,
{
    if var("TRACE_BLOCK_ON").map(|v| v == "true") == Ok(true) {
        let mut trace = String::with_capacity(4096);
        stack_trace(&mut stack_trace_frame, &mut |l| trace.push_str(l));
        log::info!("block_on at\n{}", trace);
    }

    wio::CORE.0.block_on(f)
}

#[cfg(target_arch = "wasm32")]
pub fn block_on<F>(_f: F) -> F::Output
where
    F: Future03,
{
    panic!("block_on is not supported in WASM!");
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn async_blocking<F, R>(blocking_fn: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(blocking_fn)
        .await
        .expect("spawn_blocking to succeed")
}

#[cfg(target_arch = "wasm32")]
pub fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(target_arch = "wasm32")]
pub fn now_float() -> f64 {
    use gstuff::duration_to_float;
    duration_to_float(Duration::from_millis(now_ms()))
}

pub fn wait_until_sec(seconds: u64) -> u64 {
    (now_ms() / 1000) + seconds
}

pub fn wait_until_ms(milliseconds: u64) -> u64 {
    now_ms() + milliseconds
}

pub fn now_sec() -> u64 {
    now_ms() / 1000
}

pub fn now_sec_u32() -> u32 {
    (now_ms() / 1000)
        .try_into()
        .expect("current time in seconds should fit into u32 until 2106!")
}

pub fn now_sec_i64() -> i64 {
    (now_ms() / 1000)
        .try_into()
        .expect("current time in seconds should fit into i64 for the foreseeable future!")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn temp_dir() -> PathBuf {
    env::temp_dir()
}

/// If the `MM_LOG` variable is present then tries to open that file.
/// Prints a warning to `stdout` if there's a problem opening the file.
/// Returns `None` if `MM_LOG` variable is not present or if the specified path can't be opened.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn open_log_file() -> Option<std::fs::File> {
    let mm_log = match var("MM_LOG") {
        Ok(v) => v,
        Err(_) => return None,
    };

    // For security reasons we want the log path to always end with ".log".
    if !mm_log.ends_with(".log") {
        println!("open_log_file] MM_LOG doesn't end with '.log'");
        return None;
    }

    match std::fs::OpenOptions::new().append(true).create(true).open(&mm_log) {
        Ok(f) => Some(f),
        Err(err) => {
            println!("open_log_file] Can't open {mm_log}: {err}");
            None
        },
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn writeln(line: &str) {
    use std::panic::catch_unwind;

    // `catch_unwind` protects the tests from error
    //
    //     thread 'CORE' panicked at 'cannot access stdout during shutdown'
    //
    // (which might be related to https://github.com/rust-lang/rust/issues/29488).
    let _ = catch_unwind(|| {
        if let Ok(mut log_file) = LOG_FILE.lock() {
            if let Some(ref mut log_file) = *log_file {
                writeln!(log_file, "{line}").ok();
                return;
            }
        }
        println!("{line}");
    });
}

/// Keep a tail of the log in RAM for the integration tests.
#[cfg(target_arch = "wasm32")]
pub fn append_log_tail(line: &str) {
    static PROCESS_LOG_TAIL: OnceLock<Mutex<[u8; 0x10000]>> = OnceLock::new();
    static TAIL_CUR: AtomicUsize = AtomicUsize::new(0);

    let process_log_tail = PROCESS_LOG_TAIL.get_or_init(|| Mutex::new([0; 0x10000]));
    let mut process_log_tail = process_log_tail.lock().unwrap();

    if line.len() < process_log_tail.len() {
        let posⁱ = TAIL_CUR.load(Ordering::Relaxed);
        let posⱼ = posⁱ + line.len();
        let (posˢ, posⱼ) = if posⱼ > process_log_tail.len() {
            (0, line.len())
        } else {
            (posⁱ, posⱼ)
        };
        if TAIL_CUR
            .compare_exchange(posⁱ, posⱼ, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            for (cur, ix) in (posˢ..posⱼ).zip(0..line.len()) {
                process_log_tail[cur] = line.as_bytes()[ix]
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub fn writeln(line: &str) {
    use web_sys::console;

    console::log_1(&line.into());
    append_log_tail(line);
}

/// Returns the path for application directory of kdf(komodo-defi-framework).
#[allow(deprecated)]
pub fn kdf_app_dir() -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    return Some(env::home_dir()?.join(KOMODO_DEFI_FRAMEWORK_DIR_NAME));

    #[cfg(target_arch = "wasm32")]
    None
}

/// Returns path of the coins file.
pub fn kdf_coins_file() -> Result<PathBuf, io::Error> {
    #[cfg(not(target_arch = "wasm32"))]
    let value_from_env = env::var("MM_COINS_PATH").ok();

    #[cfg(target_arch = "wasm32")]
    let value_from_env = None;

    find_kdf_dependency_file(value_from_env, "coins")
}

/// Returns path of the config file.
pub fn kdf_config_file() -> Result<PathBuf, io::Error> {
    #[cfg(not(target_arch = "wasm32"))]
    let value_from_env = env::var("MM_CONF_PATH").ok();

    #[cfg(target_arch = "wasm32")]
    let value_from_env = None;

    find_kdf_dependency_file(value_from_env, "MM2.json")
}

/// Returns the desired file path for kdf(komodo-defi-framework).
///
/// Path priority:
///  1- From the environment variable.
///  2- From the current directory where app is called.
///  3- From the root application directory.
fn find_kdf_dependency_file(value_from_env: Option<String>, path_leaf: &str) -> Result<PathBuf, io::Error> {
    if let Some(path) = value_from_env {
        let path = PathBuf::from(path);
        require_file(&path)?;
        return Ok(path);
    }

    let from_current_dir = PathBuf::from(path_leaf);

    let path = if from_current_dir.exists() {
        from_current_dir
    } else {
        kdf_app_dir().unwrap_or_default().join(path_leaf)
    };

    require_file(&path)?;
    return Ok(path);

    fn require_file(path: &Path) -> Result<(), io::Error> {
        if path.is_dir() {
            // TODO: use `IsADirectory` variant which is stabilized with 1.83
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Expected file but '{}' is a directory.", path.display()),
            ));
        }

        if !path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("File '{}' is not present.", path.display()),
            ));
        }

        Ok(())
    }
}

pub fn small_rng() -> SmallRng {
    SmallRng::seed_from_u64(now_ms())
}

#[inline(always)]
pub fn os_rng(dest: &mut [u8]) -> Result<(), rand::Error> {
    rand::rngs::OsRng.try_fill_bytes(dest)
}

#[derive(Debug, Clone)]
/// Ordered from low to height inclusive range.
pub struct OrdRange<T>(RangeInclusive<T>);

impl<T> Deref for OrdRange<T> {
    type Target = RangeInclusive<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: PartialOrd> OrdRange<T> {
    /// Construct the OrderedRange from the start-end pair.
    pub fn new(start: T, end: T) -> Result<Self, String> {
        if start > end {
            return Err("".into());
        }

        Ok(Self(start..=end))
    }
}

impl<T: Copy> OrdRange<T> {
    /// Flatten a start-end pair into the vector.
    pub fn flatten(&self) -> Vec<T> {
        vec![*self.start(), *self.end()]
    }
}

pub const fn true_f() -> bool {
    true
}

pub const fn ten() -> usize {
    10
}

pub const fn ten_f64() -> f64 {
    10.
}

pub const fn fifty() -> usize {
    50
}

pub const fn one_hundred() -> usize {
    100
}

pub const fn one_thousand_u32() -> u32 {
    1000
}

pub const fn one_and_half_f64() -> f64 {
    1.5
}

pub const fn three_hundred_f64() -> f64 {
    300.
}

pub const fn one_f64() -> f64 {
    1.
}

pub const fn sixty_f64() -> f64 {
    60.
}

pub fn one() -> NonZeroUsize {
    NonZeroUsize::new(1).unwrap()
}

#[derive(Debug, Deserialize)]
pub struct PagingOptions {
    #[serde(default = "ten")]
    pub limit: usize,
    #[serde(default = "one")]
    pub page_number: NonZeroUsize,
    pub from_uuid: Option<Uuid>,
}

#[inline]
pub fn new_uuid() -> Uuid {
    Uuid::new_v4()
}

pub fn first_char_to_upper(input: &str) -> String {
    let mut v: Vec<char> = input.chars().collect();
    if let Some(c) = v.first_mut() {
        c.make_ascii_uppercase()
    }
    v.into_iter().collect()
}

#[test]
fn test_first_char_to_upper() {
    assert_eq!("", first_char_to_upper(""));
    assert_eq!("K", first_char_to_upper("k"));
    assert_eq!("Komodo", first_char_to_upper("komodo"));
    assert_eq!(".komodo", first_char_to_upper(".komodo"));
}

/// Calculates the median of the set represented as slice
pub fn median<T: Add<Output = T> + Div<Output = T> + Copy + From<u8> + Ord>(input: &mut [T]) -> Option<T> {
    // median is undefined on empty sets
    if input.is_empty() {
        return None;
    }
    input.sort();
    let median_index = input.len() / 2;
    if input.len().is_multiple_of(2) {
        Some((input[median_index - 1] + input[median_index]) / T::from(2u8))
    } else {
        Some(input[median_index])
    }
}

#[test]
fn test_median() {
    let mut input = [3, 2, 1];
    let expected = Some(2u32);
    let actual = median(&mut input);
    assert_eq!(expected, actual);

    let mut input = [3, 1];
    let expected = Some(2u32);
    let actual = median(&mut input);
    assert_eq!(expected, actual);

    let mut input = [1, 3, 2, 8, 10];
    let expected = Some(3u32);
    let actual = median(&mut input);
    assert_eq!(expected, actual);
}

pub fn calc_total_pages(entries_len: usize, limit: usize) -> usize {
    if limit == 0 {
        return 0;
    }
    let pages_num = entries_len / limit;
    if entries_len.is_multiple_of(limit) {
        pages_num
    } else {
        pages_num + 1
    }
}

#[test]
fn test_calc_total_pages() {
    assert_eq!(0, calc_total_pages(0, 0));
    assert_eq!(0, calc_total_pages(0, 1));
    assert_eq!(0, calc_total_pages(0, 100));
    assert_eq!(1, calc_total_pages(1, 1));
    assert_eq!(2, calc_total_pages(16, 8));
    assert_eq!(2, calc_total_pages(15, 8));
}

struct SequentialCount<I>
where
    I: Iterator,
{
    iter: Peekable<I>,
}

impl<I> SequentialCount<I>
where
    I: Iterator,
{
    fn new(iter: I) -> Self {
        SequentialCount { iter: iter.peekable() }
    }
}

/// https://stackoverflow.com/questions/32702386/iterator-adapter-that-counts-repeated-characters
impl<I> Iterator for SequentialCount<I>
where
    I: Iterator,
    I::Item: Eq,
{
    type Item = (I::Item, usize);

    fn next(&mut self) -> Option<Self::Item> {
        // Check the next value in the inner iterator
        match self.iter.next() {
            // There is a value, so keep it
            Some(head) => {
                // We've seen one value so far
                let mut count = 1;
                // Check to see what the next value is without
                // actually advancing the inner iterator
                while self.iter.peek() == Some(&head) {
                    // It's the same value, so go ahead and consume it
                    self.iter.next();
                    count += 1;
                }
                // The next element doesn't match the current value
                // complete this iteration
                Some((head, count))
            },
            // The inner iterator is complete, so we are also complete
            None => None,
        }
    }
}

pub fn is_acceptable_input_on_repeated_characters(entry: &str, limit: usize) -> bool {
    for (_, count) in SequentialCount::new(entry.chars()) {
        if count >= limit {
            return false;
        }
    }
    true
}

#[test]
fn test_is_acceptable_input_on_repeated_characters() {
    assert!(is_acceptable_input_on_repeated_characters("Hello", 3));
    assert!(!is_acceptable_input_on_repeated_characters("Hellooo", 3));
    assert!(is_acceptable_input_on_repeated_characters("SuperStrongPassword123*", 3));
    assert!(!is_acceptable_input_on_repeated_characters(
        "SuperStrongaaaPassword123*",
        3
    ));
}

#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
pub enum PagingOptionsEnum<Id> {
    FromId(Id),
    PageNumber(NonZeroUsize),
}

impl<Id> PagingOptionsEnum<Id> {
    pub fn map<U, F>(self, f: F) -> PagingOptionsEnum<U>
    where
        F: FnOnce(Id) -> U,
    {
        match self {
            PagingOptionsEnum::FromId(id) => PagingOptionsEnum::FromId(f(id)),
            PagingOptionsEnum::PageNumber(s) => PagingOptionsEnum::PageNumber(s),
        }
    }
}

impl<Id> Default for PagingOptionsEnum<Id> {
    fn default() -> Self {
        PagingOptionsEnum::PageNumber(NonZeroUsize::new(1).expect("1 > 0"))
    }
}

#[inline(always)]
pub fn get_utc_timestamp() -> i64 {
    // get_utc_timestamp for tests allowing to add some bias to 'now'
    #[cfg(feature = "for-tests")]
    return Utc::now().timestamp()
        + std::env::var("TEST_TIMESTAMP_OFFSET")
            .map(|s| s.as_str().parse::<i64>().unwrap_or_default())
            .unwrap_or_default();

    #[cfg(not(feature = "for-tests"))]
    return Utc::now().timestamp();
}

#[inline(always)]
pub fn get_utc_timestamp_nanos() -> Result<i64, String> {
    Utc::now()
        .timestamp_nanos_opt()
        .ok_or("Failed to get timestamp in nanoseconds; the system clock may be unreliable.".to_owned())
}

#[inline(always)]
pub fn get_local_duration_since_epoch() -> Result<Duration, SystemTimeError> {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
}

/// open file and calculate its sha256 digest as lowercase hex string
pub fn sha256_digest(path: &PathBuf) -> Result<String, std::io::Error> {
    let input = File::open(path)?;
    let mut reader = BufReader::new(input);

    let digest = {
        let mut hasher = Sha256::new();
        let mut buffer = [0; 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        format!("{:x}", hasher.finalize())
    };
    Ok(digest)
}

#[derive(Clone, Debug, Deserialize, Display, PartialEq, Serialize)]
pub enum ParseRfc3339Err {
    #[display(fmt = "Error parsing datetime to timestamp. Expected format 'YYYY-MM-DDTHH:MM:SS.sssZ', got: {_0}")]
    ParseTimestampError(String),
    #[display(fmt = "Error while converting types: {_0}")]
    TryFromIntError(String),
}

impl From<ParseError> for ParseRfc3339Err {
    fn from(e: ParseError) -> Self {
        ParseRfc3339Err::ParseTimestampError(e.to_string())
    }
}

impl From<TryFromIntError> for ParseRfc3339Err {
    fn from(e: TryFromIntError) -> Self {
        ParseRfc3339Err::TryFromIntError(e.to_string())
    }
}

pub fn parse_rfc3339_to_timestamp(date_str: &str) -> Result<u64, ParseRfc3339Err> {
    let date: DateTime<Utc> = date_str.parse()?;
    Ok(date.timestamp().try_into()?)
}

/// `is_initial_upgrade` function checks if the database is being upgraded from version 0 to 1.
/// This function returns a boolean indicating whether the database is being upgraded from version 0 to 1.
#[cfg(target_arch = "wasm32")]
pub fn is_initial_upgrade(old_version: u32, new_version: u32) -> bool {
    old_version == 0 && new_version == 1
}

/// Takes `http:Uri` and converts it into `String` of websocket address
///
/// Panics if the given URI doesn't contain a host value.
pub fn http_uri_to_ws_address(uri: http::Uri) -> String {
    let address_prefix = match uri.scheme_str() {
        Some("https") => "wss://",
        _ => "ws://",
    };

    let host_address = uri.host().expect("Host can't be empty.");
    let path = if uri.path() == "/" { "" } else { uri.path() };
    let port = uri.port_u16().map(|p| format!(":{p}")).unwrap_or_default();

    format!("{address_prefix}{host_address}{port}{path}")
}

/// Converts a U256 value to a lowercase hexadecimal string with "0x" prefix
#[inline]
pub fn u256_to_hex(value: U256) -> String {
    format!("0x{value:x}")
}

/// If 0x prefix exists in an str strip it or return the str as-is
#[macro_export]
macro_rules! str_strip_0x {
    ($s: expr) => {
        $s.strip_prefix("0x").unwrap_or($s)
    };
}

/// If value is 'some' push key and value (as string) into an array containing (key, value) elements
#[macro_export]
macro_rules! push_if_some {
    ($arr: expr, $k: expr, $v: expr) => {
        if let Some(v) = $v {
            $arr.push(($k, v.to_string()))
        }
    };
}

/// Define 'with_...' method to set a parameter with an optional value in a builder
#[macro_export]
macro_rules! def_with_opt_param {
    ($var: ident, $var_type: ty) => {
        $crate::paste! {
            pub fn [<with_ $var>](&mut self, $var: Option<$var_type>) -> &mut Self {
                self.$var = $var;
                self
            }
        }
    };
}

#[test]
fn test_http_uri_to_ws_address() {
    let uri = "https://cosmos-rpc.polkachu.com".parse::<http::Uri>().unwrap();
    let ws_connection = http_uri_to_ws_address(uri);
    assert_eq!(ws_connection, "wss://cosmos-rpc.polkachu.com");

    let uri = "http://cosmos-rpc.polkachu.com/".parse::<http::Uri>().unwrap();
    let ws_connection = http_uri_to_ws_address(uri);
    assert_eq!(ws_connection, "ws://cosmos-rpc.polkachu.com");

    let uri = "http://34.82.96.8:26657".parse::<http::Uri>().unwrap();
    let ws_connection = http_uri_to_ws_address(uri);
    assert_eq!(ws_connection, "ws://34.82.96.8:26657");

    let uri = "https://cosmos.blockpi.network/rpc/v1/65cc8a9ffe1627352b911dd4b7c751db4a3eaee3"
        .parse::<http::Uri>()
        .unwrap();
    let ws_connection = http_uri_to_ws_address(uri);
    assert_eq!(
        ws_connection,
        "wss://cosmos.blockpi.network/rpc/v1/65cc8a9ffe1627352b911dd4b7c751db4a3eaee3"
    );
}

#[test]
#[should_panic(expected = "Host can't be empty.")]
fn test_http_uri_to_ws_address_panic() {
    let uri = "/demo/value".parse::<http::Uri>().unwrap();
    http_uri_to_ws_address(uri);
}
