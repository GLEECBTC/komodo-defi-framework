//! Human-readable logging and statuses.

use super::{now_ms, writeln};
use crate::executor::{spawn_abortable, AbortOnDropHandle, Timer};
use crate::filename;
use chrono::format::strftime::StrftimeItems;
use chrono::format::DelayedFormat;
use chrono::{Local, TimeZone, Utc};
use crossbeam::queue::SegQueue;
use itertools::Itertools;
#[cfg(not(target_arch = "wasm32"))]
use lightning::util::logger::{Level as LightningLevel, Logger as LightningLogger, Record as LightningRecord};
use log::{Level, Record};
use parking_lot::Mutex as PaMutex;
use serde_json::Value as Json;
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::default::Default;
use std::fmt;
use std::fmt::Write as WriteFmt;
use std::hash::{Hash, Hasher};
use std::mem::swap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::thread;

pub use log::{self as log_crate, debug, error, info, trace, warn, LevelFilter};

#[cfg(target_arch = "wasm32")]
#[path = "log/wasm_log.rs"]
pub mod wasm_log;

#[cfg(target_arch = "wasm32")]
pub use wasm_log::{LogLevel, WasmCallback, WasmLoggerBuilder};

#[cfg(not(target_arch = "wasm32"))]
#[path = "log/native_log.rs"]
mod native_log;

#[cfg(not(target_arch = "wasm32"))]
pub use native_log::{FfiCallback, LogLevel, UnifiedLoggerBuilder};

lazy_static! {
    /// If this C callback is present then all the logging output should happen through it
    /// (and leaving stdout untouched).
    /// The *gravity* logging still gets a copy in order for the log-based tests to work.
    pub static ref LOG_CALLBACK: PaMutex<Option<LogCallbackBoxed>> = PaMutex::new(None);
}

pub type LogCallbackBoxed = Box<dyn LogCallback>;

pub trait LogCallback: Send + Sync + 'static {
    fn callback(&mut self, level: LogLevel, line: String);

    fn into_boxed(self) -> LogCallbackBoxed
    where
        Self: Sized,
    {
        Box::new(self)
    }
}

pub fn register_callback(callback: impl LogCallback) {
    let mut log_callback = LOG_CALLBACK.lock();
    *log_callback = Some(callback.into_boxed());
}

/// Initialized and used when there's a need to chute the logging into a given thread.
struct Gravity {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    /// The center of gravity, the thread where the logging should reach the `println!` output.
    target_thread_id: thread::ThreadId,
    /// Log chunks received from satellite threads.
    landing: SegQueue<String>,
    /// Keeps a portion of a recently flushed gravity log in RAM for inspection by the unit tests.
    tail: PaMutex<VecDeque<String>>,
}

impl Gravity {
    /// Files a log chunk to be logged from the center of gravity thread.
    #[cfg(not(target_arch = "wasm32"))]
    fn chunk2log(&self, chunk: String) {
        self.landing.push(chunk);
        if thread::current().id() == self.target_thread_id {
            self.flush()
        }
    }
    #[cfg(target_arch = "wasm32")]
    fn chunk2log(&self, chunk: String) {
        self.landing.push(chunk);
    }

    /// Prints the collected log chunks.  
    /// `println!` is used for compatibility with unit test stdout capturing.
    #[cfg(not(target_arch = "wasm32"))]
    fn flush(&self) {
        let logged_with_log_output = LOG_CALLBACK.lock().is_some();
        let mut tail = self.tail.lock();
        while let Some(chunk) = self.landing.pop() {
            if !logged_with_log_output {
                writeln(&chunk)
            }
            if tail.len() == tail.capacity() {
                let _ = tail.pop_front();
            }
            tail.push_back(chunk)
        }
    }
    #[cfg(target_arch = "wasm32")]
    fn flush(&self) {}
}

thread_local! {
    /// If set, pulls the `chunk2log` (aka `log!`) invocations into the gravity of another thread.
    static GRAVITY: RefCell<Option<Weak<Gravity>>> = const { RefCell::new (None) }
}

#[doc(hidden)]
pub fn chunk2log(mut chunk: String, level: LogLevel) {
    let used_log_callback = if let Some(ref mut log_cb) = *LOG_CALLBACK.lock() {
        log_cb.callback(level, chunk.clone());
        true
    } else {
        false
    };

    // NB: Using gravity even in the non-capturing tests in order to give the tests access to the gravity tail.
    let rc = GRAVITY.try_with(|gravity| {
        if let Some(ref gravity) = *gravity.borrow() {
            if let Some(gravity) = gravity.upgrade() {
                let mut chunkʹ = String::new();
                swap(&mut chunk, &mut chunkʹ);
                gravity.chunk2log(chunkʹ);
                true
            } else {
                false
            }
        } else {
            false
        }
    });
    if let Ok(true) = rc {
        return;
    }

    if !used_log_callback {
        writeln(&chunk)
    }
}

#[doc(hidden)]
pub fn short_log_time(ms: u64) -> DelayedFormat<StrftimeItems<'static>> {
    // NB: Given that the debugging logs are targeted at the developers and not the users
    // I think it's better to output the time in GMT here
    // in order for the developers to more easily match the events between the various parts of the peer-to-peer system.
    #[allow(deprecated)]
    let time = Utc.timestamp_millis(ms as i64);
    time.format("%d %H:%M:%S")
}

#[cfg(not(test))]
#[macro_export]
macro_rules! covered_warn {
    ($($arg:tt)+) => {
        common::log::warn!($($arg)+)
    };
}

#[cfg(test)]
#[macro_export]
macro_rules! covered_warn {
    ($($arg:tt)+) => {
        panic!($($arg)+)
    };
}

/// A macro to log errors in production environment and panic in tests.
#[macro_export]
macro_rules! covered_error {
    ($($arg:tt)+) => {
        if cfg!(not(test)) {
            common::log::error!($($arg)+)
        } else {
            panic!($($arg)+)
        }
    };
}

/// Debug logging.
///
/// This logging SHOULD be human-readable but it is not intended for the end users specifically.
/// Rather, it's being used as debugging and testing tool.
///
/// (As such it doesn't have to be a text paragraph, the capital letters and end marks are not necessary).
///
/// For the user-targeted logging use the `LogState::log` instead.
///
/// On Windows the Rust standard output and the standard output of the MM1 C library are not compatible,
/// they will overlap and overwrite each other if used togather.
/// In order to avoid it, all logging MUST be done through this macros and NOT through `println!` or `eprintln!`.
#[macro_export]
macro_rules! log {
    ($($args: tt)+) => {{
        let time = $crate::log::short_log_time($crate::now_ms());
        let file = $crate::filename(file!());
        let msg = format!($($args)+);
        let chunk = format!("{}, {}:{}] {}", time, file, line!(), msg);
        $crate::log::chunk2log(chunk, $crate::log::LogLevel::Info)
    }}
}

/// Log to the `ctx` dashboard with single tags, or key-value tags, or without any tags.
///
/// # Examples
///
/// ## With single and key-value tags
///
/// ```rust
/// log_tag!(
///   ctx,
///   "😅",
///   "tx_history",
///   "coin" => coin.as_ref().conf.ticker;
///   fmt = "Some message: {}",
///   any_message
/// );
/// ```
///
/// ## Without any tags
///
/// ```rust
/// log_tag!(ctx, "😅"; fmt = "Some message: {}", any_message);
/// ```
///
/// # Important
///
/// Don't forget to separate tags and message formatting using `;` symbol.
#[macro_export]
macro_rules! log_tag {
    ($ctx:expr, $emotion:literal $(, $tag_key:expr $(=> $tag_val:expr)? )* ; fmt = $($arg:tt)*) => {{
        let tags: &[&dyn $crate::log::TagParam] = &[
            $(
                &(
                    $tag_key.to_string()
                    $(, $tag_val.to_string())?
                )
            ),*
        ];
        let line = ERRL!($($arg)*);
        $ctx.log.log($emotion, tags, &line);
    }};
}

pub trait LogOnError {
    // Log the error and caller location to WARN level here.
    fn warn_log(self);

    // Log the error, caller location and the given message to WARN level here.
    fn warn_log_with_msg(self, msg: &str);

    // Log the error and caller location to ERROR level here.
    fn error_log(self);

    // Log the error, caller location and the given message to ERROR level here.
    fn error_log_with_msg(self, msg: &str);

    fn error_log_passthrough(self) -> Self;

    // Log the error, caller location and the given message to DEBUG level here.
    fn debug_log_with_msg(self, msg: &str);
}

impl<T, E: fmt::Display> LogOnError for Result<T, E> {
    #[track_caller]
    fn warn_log(self) {
        if let Err(e) = self {
            let location = std::panic::Location::caller();
            let file = filename(location.file());
            let line = location.line();
            warn!("{}:{}] {}", file, line, e);
        }
    }

    #[track_caller]
    fn warn_log_with_msg(self, msg: &str) {
        if let Err(e) = self {
            let location = std::panic::Location::caller();
            let file = filename(location.file());
            let line = location.line();
            warn!("{}:{}] {}: {}", file, line, msg, e);
        }
    }

    #[track_caller]
    fn error_log(self) {
        if let Err(e) = self {
            let location = std::panic::Location::caller();
            let file = filename(location.file());
            let line = location.line();
            error!("{}:{}] {}", file, line, e);
        }
    }

    #[track_caller]
    fn error_log_with_msg(self, msg: &str) {
        if let Err(e) = self {
            let location = std::panic::Location::caller();
            let file = filename(location.file());
            let line = location.line();
            error!("{}:{}] {}: {}", file, line, msg, e);
        }
    }

    #[track_caller]
    fn error_log_passthrough(self) -> Self {
        if let Err(e) = &self {
            let location = std::panic::Location::caller();
            let file = filename(location.file());
            let line = location.line();
            error!("{}:{}] {}", file, line, e);
        }
        self
    }

    #[track_caller]
    fn debug_log_with_msg(self, msg: &str) {
        if let Err(e) = self {
            let location = std::panic::Location::caller();
            let file = filename(location.file());
            let line = location.line();
            debug!("{}:{}] {}: {}", file, line, msg, e);
        }
    }
}

pub trait TagParam<'a> {
    fn key(&self) -> String;
    fn val(&self) -> Option<String>;
}

impl<'a> TagParam<'a> for &'a str {
    fn key(&self) -> String {
        String::from(&self[..])
    }
    fn val(&self) -> Option<String> {
        None
    }
}

impl TagParam<'_> for String {
    fn key(&self) -> String {
        self.clone()
    }
    fn val(&self) -> Option<String> {
        None
    }
}

impl<'a> TagParam<'a> for (&'a str, &'a str) {
    fn key(&self) -> String {
        String::from(self.0)
    }
    fn val(&self) -> Option<String> {
        Some(String::from(self.1))
    }
}

impl<'a> TagParam<'a> for (String, &'a str) {
    fn key(&self) -> String {
        self.0.clone()
    }
    fn val(&self) -> Option<String> {
        Some(String::from(self.1))
    }
}

impl<'a> TagParam<'a> for (&'a str, i32) {
    fn key(&self) -> String {
        String::from(self.0)
    }
    fn val(&self) -> Option<String> {
        Some(self.1.to_string())
    }
}

impl TagParam<'_> for (String, String) {
    fn key(&self) -> String {
        self.0.clone()
    }
    fn val(&self) -> Option<String> {
        Some(self.1.clone())
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Tag {
    pub key: String,
    pub val: Option<String>,
}

impl Tag {
    /// Returns the tag's value or the empty string if there is no value.
    pub fn val_s(&self) -> &str {
        match self.val {
            Some(ref s) => &s[..],
            None => "",
        }
    }
}

impl fmt::Debug for Tag {
    fn fmt(&self, ft: &mut fmt::Formatter) -> fmt::Result {
        ft.write_str(&self.key)?;
        if let Some(ref val) = self.val {
            ft.write_str("=")?;
            ft.write_str(val)?;
        }
        Ok(())
    }
}

/// The status entry kept in the dashboard.
pub struct Status {
    pub tags: PaMutex<Vec<Tag>>,
    pub line: PaMutex<String>,
    /// The time, in milliseconds since UNIX epoch, when the tracked operation started.
    pub start: AtomicU64,
    /// Expected time limit of the tracked operation, in milliseconds since UNIX epoch.  
    /// 0 if no deadline is set.
    pub deadline: AtomicU64,
}

impl Clone for Status {
    fn clone(&self) -> Status {
        let tags = self.tags.lock().clone();
        let line = self.line.lock().clone();
        Status {
            tags: PaMutex::new(tags),
            line: PaMutex::new(line),
            start: AtomicU64::new(self.start.load(Ordering::SeqCst)),
            deadline: AtomicU64::new(self.deadline.load(Ordering::SeqCst)),
        }
    }
}

impl Hash for Status {
    fn hash<H: Hasher>(&self, state: &mut H) {
        {
            let tags = self.tags.lock();
            for tag in tags.iter() {
                tag.hash(state)
            }
        }

        self.line.lock().hash(state);
        self.start.load(Ordering::SeqCst).hash(state);
        self.deadline.load(Ordering::SeqCst).hash(state);
    }
}

impl Status {
    /// Invoked when the `StatusHandle` is dropped, marking the status as finished.
    fn finished(
        status: &Arc<Status>,
        dashboard: &Arc<PaMutex<Vec<Arc<Status>>>>,
        tail: &Arc<PaMutex<VecDeque<LogEntry>>>,
    ) {
        {
            let mut dashboard = dashboard.lock();
            if let Some(idx) = dashboard.iter().position(|e| Arc::ptr_eq(e, status)) {
                dashboard.swap_remove(idx);
            } else {
                log!("log] Warning, a finished StatusHandle was missing from the dashboard.");
            }
        }

        let mut log = LogEntry::default();
        swap(&mut log.tags, &mut *status.tags.lock());
        swap(&mut log.line, &mut *status.line.lock());
        let mut chunk = String::with_capacity(256);
        if let Err(err) = log.format(&mut chunk) {
            log!("log] Error formatting log entry: {}", err);
        }

        {
            let mut tail = tail.lock();
            if tail.len() == tail.capacity() {
                tail.pop_front();
            }
            tail.push_back(log);
        }

        chunk2log(chunk, LogLevel::Info)
    }
}

#[derive(Clone)]
pub struct LogEntry {
    pub time: u64,
    pub emotion: String,
    pub tags: Vec<Tag>,
    pub line: String,
}

impl Default for LogEntry {
    fn default() -> Self {
        LogEntry {
            time: now_ms(),
            emotion: Default::default(),
            tags: Default::default(),
            line: Default::default(),
        }
    }
}

impl LogEntry {
    pub fn format(&self, buf: &mut String) -> Result<(), fmt::Error> {
        #[allow(deprecated)]
        let time = Local.timestamp_millis(self.time as i64);
        let time_formatted = time.format("%Y-%m-%d %H:%M:%S %z");
        let emotion = if self.emotion.is_empty() { "·" } else { &self.emotion };
        let tags = format_tags(&self.tags);
        write!(buf, "{} {} [{}] {}", emotion, time_formatted, tags, self.line)
    }
}

/// Tracks the status of an ongoing operation, allowing us to inform the user of the status updates.
///
/// Dropping the handle tells us that the operation was "finished" and that we can dump the final status into the log.
pub struct StatusHandle {
    status: Option<Arc<Status>>,
    dashboard: Arc<PaMutex<Vec<Arc<Status>>>>,
    tail: Arc<PaMutex<VecDeque<LogEntry>>>,
}

impl StatusHandle {
    /// Creates the status or rewrites it.
    ///
    /// The `tags` can be changed as well:
    /// with `StatusHandle` the status line is directly identified by the handle and doesn't use the tags to lookup the status line.
    pub fn status(&mut self, tags: &[&dyn TagParam], line: &str) {
        let tagsʹ: Vec<Tag> = tags
            .iter()
            .map(|t| Tag {
                key: t.key(),
                val: t.val(),
            })
            .collect();
        if let Some(ref status) = self.status {
            // Skip a status update if it is equal to the previous update.
            if status.line.lock().as_str() == line && *status.tags.lock() == tagsʹ {
                return;
            }

            *status.tags.lock() = tagsʹ;
            *status.line.lock() = String::from(line);
        } else {
            let status = Arc::new(Status {
                tags: PaMutex::new(tagsʹ),
                line: PaMutex::new(line.into()),
                start: AtomicU64::new(now_ms()),
                deadline: AtomicU64::new(0),
            });
            self.status = Some(status.clone());
            self.dashboard.lock().push(status);
        }
    }

    /// Adds new text into the status line.  
    /// Does nothing if the status handle is empty (if the status wasn't created yet).
    pub fn append(&self, suffix: &str) {
        if let Some(ref status) = self.status {
            status.line.lock().push_str(suffix)
        }
    }

    /// Detach the handle from the status, allowing the status to remain in the dashboard when the handle is dropped.
    ///
    /// The code should later manually finish the status (finding it with `LogState::find_status`).
    pub fn detach(&mut self) -> &mut Self {
        self.status = None;
        self
    }

    /// Sets the deadline for the operation tracked by the status.
    ///
    /// The deadline is used to inform the user of the time constaints of the operation.  
    /// It is not enforced by the logging/dashboard subsystem.
    ///
    /// * `ms` - The time, in milliseconds since UNIX epoch,
    ///   when the operation is bound to end regardless of its status (aka a timeout).
    pub fn deadline(&self, ms: u64) {
        if let Some(ref status) = self.status {
            status.deadline.store(ms, Ordering::SeqCst)
        }
    }

    /// Sets the deadline for the operation tracked by the status.
    ///
    /// The deadline is used to inform the user of the time constaints of the operation.  
    /// It is not enforced by the logging/dashboard subsystem.
    ///
    /// * `ms` - The time, in milliseconds since the creation of the status,
    ///   when the operation is bound to end (aka a timeout).
    pub fn timeframe(&self, ms: u64) {
        if let Some(ref status) = self.status {
            let start = status.start.load(Ordering::SeqCst);
            status.deadline.store(start + ms, Ordering::SeqCst)
        }
    }

    /// The number of milliseconds remaining till the deadline.  
    /// Negative if the deadline is in the past.
    pub fn ms2deadline(&self) -> Option<i64> {
        if let Some(ref status) = self.status {
            let deadline = status.deadline.load(Ordering::SeqCst);
            if deadline == 0 {
                None
            } else {
                Some(deadline as i64 - now_ms() as i64)
            }
        } else {
            None
        }
    }
}

impl Drop for StatusHandle {
    fn drop(&mut self) {
        if let Some(ref status) = self.status {
            Status::finished(status, &self.dashboard, &self.tail)
        }
    }
}

/// Generates a MM dashboard file path from the MM log file path.
pub fn dashboard_path(log_path: &Path) -> Result<PathBuf, String> {
    let log_path = try_s!(log_path.to_str().ok_or("Non-unicode log_path?"));
    Ok(format!("{log_path}.dashboard").into())
}

/// The shared log state of a MarketMaker instance.  
/// Carried around by the MarketMaker state, `MmCtx`.  
/// Keeps track of the log file and the status dashboard.
pub struct LogState {
    dashboard: Arc<PaMutex<Vec<Arc<Status>>>>,
    /// Keeps recent log entries in memory in case we need them for debugging.  
    /// Should allow us to examine the log from withing the unit tests, core dumps and live debugging sessions.
    tail: Arc<PaMutex<VecDeque<LogEntry>>>,
    /// Initialized when we need the logging to happen through a certain thread
    /// (this thread becomes a center of gravity for the other registered threads).
    /// In the future we might also use `gravity` to log into a file.
    gravity: PaMutex<Option<Arc<Gravity>>>,
    /// Keeps track of the log level that the log state is initiated with
    level: LogLevel,
    /// `log_dashboard_sometimes` abort handle if it has been spawned.
    _dashboard_abort_handle: Option<AbortOnDropHandle>,
}

#[derive(Clone)]
pub struct LogArc(pub Arc<LogState>);

impl Deref for LogArc {
    type Target = LogState;
    fn deref(&self) -> &LogState {
        &self.0
    }
}

impl LogArc {
    /// Create LogArc from real `LogState`.
    pub fn new(state: LogState) -> LogArc {
        LogArc(Arc::new(state))
    }

    /// Try to obtain the `LogState` from the weak pointer.
    pub fn from_weak(weak: &LogWeak) -> Option<LogArc> {
        weak.0.upgrade().map(LogArc)
    }

    /// Create a weak pointer to `LogState`.
    pub fn weak(&self) -> LogWeak {
        LogWeak(Arc::downgrade(&self.0))
    }
}

#[derive(Default)]
pub struct LogWeak(pub Weak<LogState>);

impl LogWeak {
    /// Create a default MmWeak without allocating any memory.
    pub fn new() -> LogWeak {
        Default::default()
    }

    pub fn dropped(&self) -> bool {
        self.0.strong_count() == 0
    }
}

/// The state used to periodically log the dashboard.
struct DashboardLogging {
    /// The time when the dashboard was last printed into the log.
    last_log_ms: AtomicU64,
    /// Checksum of the dashboard that was last printed into the log.  
    /// Allows us to detect whether the dashboard has changed since then.
    last_hash: AtomicU64,
}

impl Default for DashboardLogging {
    fn default() -> DashboardLogging {
        DashboardLogging {
            last_log_ms: AtomicU64::new(0),
            last_hash: AtomicU64::new(0),
        }
    }
}

fn log_dashboard_sometimesʹ(dashboard: Vec<Arc<Status>>, dl: &mut DashboardLogging) {
    // See if it's time to log the dashboard.
    if dashboard.is_empty() {
        return;
    }
    let mut hasher = DefaultHasher::new();
    for status in dashboard.iter() {
        status.hash(&mut hasher)
    }
    let hash = hasher.finish();

    let now = now_ms();
    let delta = now as i64 - dl.last_log_ms.load(Ordering::SeqCst) as i64;
    let last_hash = dl.last_hash.load(Ordering::SeqCst);
    let itʹs_time = if hash != last_hash {
        delta > 7777
    } else {
        delta > 7777 * 3
    };
    if !itʹs_time {
        return;
    }

    dl.last_hash.store(hash, Ordering::SeqCst);
    dl.last_log_ms.store(now, Ordering::SeqCst);
    let mut buf = format!("+--- {} -------", short_log_time(now));
    for status in dashboard.iter() {
        let start = status.start.load(Ordering::SeqCst);

        let tags_str = {
            let tags = status.tags.lock();
            format_tags(&tags)
        };

        let passed = (now as i64 - start as i64) / 1000;
        let passed_str = if passed >= 0 {
            format!("{}:{:0>2}", passed / 60, passed % 60)
        } else {
            "-".to_owned()
        };

        let deadline = status.deadline.load(Ordering::SeqCst);
        let timeframe = (deadline as i64 - start as i64) / 1000;
        let timeframe_str = if deadline > 0 {
            format!("/{}:{:0>2}", timeframe / 60, timeframe % 60)
        } else {
            String::new()
        };

        let line = status.line.lock().clone();
        buf.push_str(&format!("\n| ({passed_str}{timeframe_str}) [{tags_str}] {line}"));
    }
    chunk2log(buf, LogLevel::Info)
}

async fn log_dashboard_sometimes(dashboardʷ: Weak<PaMutex<Vec<Arc<Status>>>>) {
    let mut dashboard_logging = DashboardLogging::default();
    loop {
        Timer::sleep(0.777).await;
        // The loop stops when the `LogState::dashboard` is dropped.
        let dashboard = match dashboardʷ.upgrade() {
            Some(arc) => arc,
            None => break,
        };
        let dashboard = dashboard.lock().clone();
        log_dashboard_sometimesʹ(dashboard, &mut dashboard_logging);
    }
}

impl LogState {
    /// Log into memory, for unit testing.
    pub fn in_memory() -> LogState {
        LogState {
            dashboard: Arc::new(PaMutex::new(Vec::new())),
            tail: Arc::new(PaMutex::new(VecDeque::with_capacity(64))),
            gravity: PaMutex::new(None),
            level: LogLevel::default(),
            _dashboard_abort_handle: None,
        }
    }

    /// Initialize according to the MM command-line configuration.
    pub fn mm(_conf: &Json) -> LogState {
        let dashboard = Arc::new(PaMutex::new(Vec::new()));
        let abort_handle = spawn_abortable(log_dashboard_sometimes(Arc::downgrade(&dashboard)));

        LogState {
            dashboard,
            tail: Arc::new(PaMutex::new(VecDeque::with_capacity(64))),
            gravity: PaMutex::new(None),
            level: LogLevel::default(),
            _dashboard_abort_handle: Some(abort_handle),
        }
    }

    pub fn set_level(&mut self, level: LogLevel) {
        self.level = level;
    }

    /// The operation is considered "in progress" while the `StatusHandle` exists.
    ///
    /// When the `StatusHandle` is dropped the operation is considered "finished" (possibly with a failure)
    /// and the status summary is dumped into the log.
    pub fn status_handle(&self) -> StatusHandle {
        StatusHandle {
            status: None,
            dashboard: self.dashboard.clone(),
            tail: self.tail.clone(),
        }
    }

    /// Read-only access to the status dashboard.
    pub fn with_dashboard(&self, cb: &mut dyn FnMut(&[Arc<Status>])) {
        let dashboard = self.dashboard.lock();
        cb(&dashboard[..])
    }

    pub fn with_tail(&self, cb: &mut dyn FnMut(&VecDeque<LogEntry>)) {
        let tail = self.tail.lock();
        cb(&tail)
    }

    pub fn with_gravity_tail(&self, cb: &mut dyn FnMut(&VecDeque<String>)) {
        let gravity = self.gravity.lock().clone();
        if let Some(ref gravity) = gravity {
            gravity.flush();
            let tail = gravity.tail.lock();
            cb(&tail);
        }
    }

    /// Creates the status or rewrites it if the tags match.
    pub fn status(&self, tags: &[&dyn TagParam], line: &str) -> StatusHandle {
        let mut status = self.claim_status(tags).unwrap_or_else(|| self.status_handle());
        status.status(tags, line);
        status
    }

    /// Search dashboard for status matching the tags.
    ///
    /// Note that returned status handle represent an ownership of the status and on the `drop` will mark the status as finished.
    pub fn claim_status(&self, tags: &[&dyn TagParam]) -> Option<StatusHandle> {
        let mut found = Vec::new();
        let tags: Vec<Tag> = tags
            .iter()
            .map(|t| Tag {
                key: t.key(),
                val: t.val(),
            })
            .collect();
        let dashboard_statuses = self.dashboard.lock().clone();
        for status_arc in dashboard_statuses {
            if *status_arc.tags.lock() == tags {
                found.push(StatusHandle {
                    status: Some(status_arc.clone()),
                    dashboard: self.dashboard.clone(),
                    tail: self.tail.clone(),
                })
            }
        }
        if found.len() > 1 {
            log!("log] Dashboard tags not unique!")
        }
        found.pop()
    }

    /// Returns `true` if there are recent log entries exactly matching the tags.
    pub fn tail_any(&self, tags: &[&dyn TagParam]) -> bool {
        let tags: Vec<Tag> = tags
            .iter()
            .map(|t| Tag {
                key: t.key(),
                val: t.val(),
            })
            .collect();
        let tail = self.tail.lock();
        for en in tail.iter() {
            if en.tags == tags {
                return true;
            }
        }
        false
    }

    /// Creates a new human-readable log entry.
    ///
    /// The method is identical to `log_deref_tags` except the `tags` are `TagParam` trait objects.
    pub fn log(&self, emotion: &str, tags: &[&dyn TagParam], line: &str) {
        let entry = LogEntry {
            time: now_ms(),
            emotion: emotion.into(),
            tags: tags
                .iter()
                .map(|t| Tag {
                    key: t.key(),
                    val: t.val(),
                })
                .collect(),
            line: line.into(),
        };

        self.log_entry(entry)
    }

    /// Creates a new human-readable log entry.
    ///
    /// This is a bit different from the `println!` logging
    /// (https://www.reddit.com/r/rust/comments/9hpk65/which_tools_are_you_using_to_debug_rust_projects/e6dkciz/)
    /// as the information here is intended for the end users
    /// (and to be shared through the GUI),
    /// explaining what's going on with MM.
    ///
    /// Since the focus here is on human-readability, the log entry SHOULD be treated
    /// as a text paragraph, namely starting with a capital letter and ending with an end mark.
    ///
    /// * `emotion` - We might use a unicode smiley here
    ///   (https://unicode.org/emoji/charts/full-emoji-list.html)
    ///   to emotionally color the event (the good, the bad and the ugly)
    ///   or enrich it with infographics.
    /// * `tags` - Parsable part of the log,
    ///   representing subsystems and sharing concrete values.
    ///   GUI might use it to get some useful information from the log.
    /// * `line` - The human-readable description of the event,
    ///   we have no intention to make it parsable.
    pub fn log_deref_tags(&self, emotion: &str, tags: Vec<Tag>, line: &str) {
        let entry = LogEntry {
            time: now_ms(),
            emotion: emotion.into(),
            tags,
            line: line.into(),
        };

        self.log_entry(entry)
    }

    fn log_entry(&self, entry: LogEntry) {
        let mut chunk = String::with_capacity(256);
        if let Err(err) = entry.format(&mut chunk) {
            log!("log] Error formatting log entry: {}", err);
            return;
        }

        {
            let mut tail = self.tail.lock();
            if tail.len() == tail.capacity() {
                let _ = tail.pop_front();
            }
            tail.push_back(entry);
        }

        self.chunk2log(chunk, self.level)
    }

    fn chunk2log(&self, chunk: String, level: LogLevel) {
        if self.level.ge(&level) {
            self::chunk2log(chunk, level);
        }
        /*
        match self.log_file {
            Some (ref f) => match f.lock() {
                Ok (mut f) => {
                    if let Err (err) = f.write (chunk.as_bytes()) {
                        eprintln! ("log] Can't write to the log: {}", err);
                        println! ("{}", chunk);
                    }
                },
                Err (err) => {
                    eprintln! ("log] Can't lock the log: {}", err);
                    println! ("{}", chunk)
                }
            },
            None => println! ("{}", chunk)
        }
        */
    }

    /// Writes into the *raw* portion of the log, the one not shared with the UI.
    pub fn rawln(&self, mut line: String) {
        line.push('\n');
        self.chunk2log(line, self.level);
    }

    /// Binds the logger to the current thread,
    /// creating a gravity anomaly that would pull log entries made on other threads into this thread.
    /// Useful for unit tests, since they can only capture the output made from the initial test thread
    /// (https://github.com/rust-lang/rust/issues/12309,
    ///  https://github.com/rust-lang/rust/issues/50297#issuecomment-388988381).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn thread_gravity_on(&self) -> Result<(), String> {
        let mut gravity = self.gravity.lock();
        if let Some(ref gravity) = *gravity {
            if gravity.target_thread_id == thread::current().id() {
                Ok(())
            } else {
                ERR!("Gravity already enabled and for a different thread")
            }
        } else {
            *gravity = Some(Arc::new(Gravity {
                target_thread_id: thread::current().id(),
                landing: SegQueue::new(),
                tail: PaMutex::new(VecDeque::with_capacity(64)),
            }));
            Ok(())
        }
    }
    #[cfg(target_arch = "wasm32")]
    pub fn thread_gravity_on(&self) -> Result<(), String> {
        Ok(())
    }

    /// Start intercepting the `log!` invocations happening on the current thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn register_my_thread(&self) -> Result<(), String> {
        let gravity = self.gravity.lock();
        if let Some(ref gravity) = *gravity {
            try_s!(GRAVITY
                .try_with(|thread_local_gravity| { thread_local_gravity.replace(Some(Arc::downgrade(gravity))) }));
        } else {
            // If no gravity thread is registered then `register_my_thread` is currently a no-op.
            // In the future we might implement a version of `Gravity` that pulls log entries into a file
            // (but we might want to get rid of C logging first).
        }
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    pub fn register_my_thread(&self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl LightningLogger for LogState {
    fn log(&self, record: &LightningRecord) {
        let level = match record.level {
            LightningLevel::Gossip => Level::Trace,
            LightningLevel::Trace => Level::Debug,
            LightningLevel::Debug => Level::Debug,
            LightningLevel::Info => Level::Info,
            LightningLevel::Warn => Level::Warn,
            LightningLevel::Error => Level::Error,
        };
        let record = Record::builder()
            .args(record.args)
            .level(level)
            .module_path(Some(record.module_path))
            .file(Some(record.file))
            .line(Some(record.line))
            .build();
        let as_string = format_record(&record);
        let level = LogLevel::from(record.metadata().level());
        self.chunk2log(as_string, level);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for LogState {
    /// Make sure to log the chunks received from the satellite threads.
    /// NB: The `drop` might happen in a thread that is not the center of gravity,
    ///     resulting in log chunks escaping the unit test capture.
    ///     One way to fight this might be adding a flushing RAII struct into a unit test.
    /// NB: The `drop` will not be happening if some of the satellite threads still hold to the context.
    fn drop(&mut self) {
        let gravity = self.gravity.lock().clone();
        if let Some(gravity) = gravity {
            gravity.flush()
        }

        let dashboard_copy = self.dashboard.lock().clone();
        if !dashboard_copy.is_empty() {
            log!("--- LogState] Bye! Remaining status entries. ---");
            for status in &*dashboard_copy {
                Status::finished(status, &self.dashboard, &self.tail)
            }
        } else {
            log!("LogState] Bye!");
        }
    }
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" => Ok(LogLevel::Off),
            "error" => Ok(LogLevel::Error),
            "warn" => Ok(LogLevel::Warn),
            "info" => Ok(LogLevel::Info),
            "debug" => Ok(LogLevel::Debug),
            "trace" => Ok(LogLevel::Trace),
            _ => Err(s.to_owned()),
        }
    }
}

impl From<Level> for LogLevel {
    fn from(orig: Level) -> Self {
        match orig {
            Level::Error => LogLevel::Error,
            Level::Warn => LogLevel::Warn,
            Level::Info => LogLevel::Info,
            Level::Debug => LogLevel::Debug,
            Level::Trace => LogLevel::Trace,
        }
    }
}

impl From<LogLevel> for LevelFilter {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Off => LevelFilter::Off,
            LogLevel::Error => LevelFilter::Error,
            LogLevel::Warn => LevelFilter::Warn,
            LogLevel::Info => LevelFilter::Info,
            LogLevel::Debug => LevelFilter::Debug,
            LogLevel::Trace => LevelFilter::Trace,
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self {
            LogLevel::Off => "OFF",
            LogLevel::Error => "ERROR",
            LogLevel::Warn => "WARN",
            LogLevel::Info => "INFO",
            LogLevel::Debug => "DEBUG",
            LogLevel::Trace => "TRACE",
        };
        write!(f, "{level}")
    }
}

/// It's the temporary `log::Record` formatter.
/// Format: `{d(%d %H:%M:%S)(utc)}, {f}:{L}] {l} {m}`
pub fn format_record(record: &Record) -> String {
    const DATE_FORMAT: &str = "%d %H:%M:%S";

    let metadata = record.metadata();
    let level = metadata.level();
    let date = Utc::now().format(DATE_FORMAT);
    let line = record.line().unwrap_or(0);
    let message = record.args();

    format!(
        "{d}, {p}:{L}] {l} {m}",
        d = date,
        p = record.module_path().unwrap_or(""),
        L = line,
        l = level,
        m = message
    )
}

fn format_tags(tags: &[Tag]) -> String {
    tags.iter()
        .map(|tag| {
            if let Some(ref val) = tag.val {
                format!("{}={}", tag.key, val)
            } else {
                tag.key.clone()
            }
        })
        .join(" ")
}

#[doc(hidden)]
pub mod tests {
    use super::LogState;

    pub fn test_status() {
        crate::writeln(""); // Begin from a new line in the --nocapture mode.
        let log = LogState::in_memory();

        log.with_dashboard(&mut |dashboard| assert_eq!(dashboard.len(), 0));

        let mut handle = log.status_handle();
        for n in 1..=3 {
            handle.status(&[&"tag1", &"tag2"], &format!("line {n}"));

            log.with_dashboard(&mut |dashboard| {
                assert_eq!(dashboard.len(), 1);
                let status = &dashboard[0];
                assert!(status.tags.lock().iter().any(|tag| tag.key == "tag1"));
                assert!(status.tags.lock().iter().any(|tag| tag.key == "tag2"));
                assert_eq!(status.tags.lock().len(), 2);
                assert_eq!(*status.line.lock(), format!("line {n}"));
            });
        }
        drop(handle);

        log.with_dashboard(&mut |dashboard| assert_eq!(dashboard.len(), 0)); // The status was dumped into the log.
        log.with_tail(&mut |tail| {
            assert_eq!(tail.len(), 1);
            assert_eq!(tail[0].line, "line 3");

            assert!(tail[0].tags.iter().any(|tag| tag.key == "tag1"));
            assert!(tail[0].tags.iter().any(|tag| tag.key == "tag2"));
            assert_eq!(tail[0].tags.len(), 2);
        })
    }

    pub fn test_printed_dashboard() {
        crate::writeln(""); // Begin from a new line in the --nocapture mode.
        let log = LogState::in_memory();
        log.thread_gravity_on().unwrap();
        log.register_my_thread().unwrap();
        let mut status = log.status_handle();
        status.status(&[&"tag"], "status 1%…");
        status.timeframe((3 * 60 + 33) * 1000);

        {
            let dashboard = log.dashboard.lock().clone();
            let mut dashboard_logging = super::DashboardLogging::default();
            super::log_dashboard_sometimesʹ(dashboard, &mut dashboard_logging);
        }

        log.with_gravity_tail(&mut |tail| {
            assert!(tail[0].ends_with("/3:33) [tag] status 1%…"));
        });
    }

    #[test]
    #[should_panic(expected = "Fail with me...")]
    fn test_covered_warn() {
        covered_warn!("Fail with me...");
    }
}
