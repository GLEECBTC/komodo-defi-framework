//! The MarketMaker2 unified error representation tracing an error path.
//!
//! # Tracing while chaining
//!
//! `MmError` supports several ways to trace the error path while different error types replace each other along the path.
//!
//! ## Operator `?`
//!
//! `MmError` main goal is to use the operator `?` to convert one `E1` type into another `E2` type
//! and track the place where this conversion took place.
//! Every time [`MmError<E2>`] converts from [`MmError<E1>`] by using the operator `?`, an `MmError` instance tracks where that operator was called.
//! The main limitation is that the `E2` has to be directly convertible from `E1`, that is `E2: From<E1>`.
//!
//! ```rust
//! fn filename(path: &str) -> Result<String, E1> { Err(E1::new()) }
//! fn get_file_extension(filename: &str) -> Result<&str, MmError<E1>> { MmError::err(E1::new()) }
//!
//! fn is_static_library(path: &str) -> Result<(), MmError<E2>> {
//!     let filename = filename(path)?;
//!     let extension = get_file_extension(filename).map_mm_err()?;
//!     if extension == "a" || extension == "lib" {
//!         Ok(())
//!     } else {
//!         MmError::err(E2::new())
//!     }
//! }
//! ```
//!
//! ## Map inner error type
//!
//! If the `E2` is not directly convertible from `E1`, then it can be mapped into [`Result<T, MmError<E2>>`].
//! * [`Result<T, E1>`] can be mapped to [`Result<T, MmError<E2>>`] by applying [`MapToMmResult::map_to_mm`];
//! * [`Result<T, MmError<E1>>`] can be mapped to [`Result<T, MmError<E2>>`] by applying [`MapMmError::mm_err`];
//! * [`Option<T>`] can be mapped to [`Result<T, MmError<E>>`] by applying [`OrMmError::or_mm_err`];
//! * [`Future<Item=T, Error=E1>`] can be mapped to [`Future<Item=T, Error=MmError<E2>>`] by applying [`MapToMmFutureExt::map_to_mm_fut`].
//!
//! Every time one of the methods is called, an `MmError` instance tracks where that method was called.
//! Let's modify the example:
//!
//! ```rust
//! fn filename(path: &str) -> Result<String, E1> { Err(E1::new()) }
//! fn get_file_extension(filename: &str) -> Result<&str, MmError<E1>> { MmError::err(E1::new()) }
//!
//! fn is_static_library(path: &str) -> Result<(), MmError<E2>> {
//!     let filename = filename(path).map_to_mm(|e1| E2::from_e1(e1)).map_mm_err()??;
//!     let extension = get_file_extension(filename).mm_err(|e1| E2::from_e1(e1))?;
//!     if extension == "a" || extension == "lib" {
//!         Ok(())
//!     } else {
//!         MmError::err(E2::new())
//!     }
//! }
//! ```
//!
//! # Serialization
//!
//! The serialized representation of an [`MmError<E>`] error consists of the following fields:
//! * `error` - the common error description;
//! * `error_path` - the error path consisting of file names separated by a dot similar to JSON path notation;
//!   Example: `rpc.lp_coins.utxo`
//! * `error_trace` - it is a more detailed error path consisting of file and line number pairs separated by ']';
//!   Example: `rpc:392] lp_coins:1104] lp_coins:245] utxo:778]`
//! * `error_type` - the string error identifier of the `E` type;
//! * `error_data` - an object containing the error data of the `E` type.
//!
//! ## Important
//!
//! The error type must be [`tagged`](https://serde.rs/enum-representations.html#adjacently-tagged) with the `tag = "error_type"` and `content = "error_data".
//! Otherwise, the serialized error would conflict with the unified `mmrpc` protocol.
//! To check at compile time that the error type flattens into `error_type` and `error_data` fields only, it has to implement `SerializeErrorType` trait.
//!
//! Please note the `SerializeErrorType` trait can be derived only by adding the `#[derive(SerializeErrorType)]` attribute to the target error type.
//!
//! # Example
//!
//! ```rust
//! #[derive(Display, Serialize, SerializeErrorType)]
//!  #[serde(tag = "error_type", content = "error_data")]
//!  enum RpcError {
//!      TransportError {
//!          reason: String,
//!      },
//!      InternalError,
//!  }
//! ```

use common::{filename, HttpStatusCode};
use derive_more::Display;
use http::StatusCode;
use itertools::Itertools;
use ser_error::SerializeErrorType;
use serde::{Serialize, Serializer};
use std::alloc::Allocator;
use std::cell::UnsafeCell;
use std::error::Error as StdError;
use std::fmt;
use std::panic::Location;
use std::sync::Arc;

pub type MmResult<T, E> = Result<T, MmError<E>>;

pub auto trait NotMmError {}

impl<E> !NotMmError for MmError<E> {}

/// This is required because an auto trait is not automatically implemented for a non-sized types,
/// for Box<dyn Trait>.
impl<T: ?Sized, A: Allocator> NotMmError for Box<T, A> {}
/// for Arc<dyn Trait>.
impl<T: ?Sized> NotMmError for Arc<T> {}
/// for UnsafeCell<dyn Trait>
impl<T: ?Sized> NotMmError for UnsafeCell<T> {}

pub trait SerMmErrorType: SerializeErrorType + fmt::Display + NotMmError {}

impl<E> SerMmErrorType for E where E: SerializeErrorType + fmt::Display + NotMmError {}

/// The unified error representation tracing an error path.
#[derive(Clone, Eq, PartialEq)]
pub struct MmError<E: NotMmError> {
    pub(crate) etype: E,
    pub(crate) trace: Vec<TraceLocation>,
}

impl<E> fmt::Display for MmError<E>
where
    E: NotMmError + fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.trace.formatted(), self.etype)
    }
}

impl<E> fmt::Debug for MmError<E>
where
    E: NotMmError + fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?}", self.trace.formatted(), self.etype)
    }
}

impl<E: fmt::Display + StdError + NotMmError> StdError for MmError<E> {}

/// Track the location whenever `MmError<E2>::from(E1)` is called.
impl<E1, E2> From<E1> for MmError<E2>
where
    E1: NotMmError,
    E2: From<E1> + NotMmError,
{
    #[track_caller]
    fn from(e1: E1) -> Self {
        MmError::new(E2::from(e1))
    }
}

impl<E> Serialize for MmError<E>
where
    E: SerMmErrorType,
{
    fn serialize<S>(&self, serializer: S) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct ErrorHelper<'a, E> {
            error: String,
            error_path: String,
            error_trace: String,
            /// `etype` will be flatten into `error_type` and `error_data` (it's guaranteed by [`ser_error::SerializeErrorType`] trait)
            #[serde(flatten)]
            etype: &'a E,
        }

        let helper = ErrorHelper {
            error: self.etype.to_string(),
            error_path: self.path(),
            error_trace: self.stack_trace(),
            etype: &self.etype,
        };
        helper.serialize(serializer)
    }
}

impl<E> HttpStatusCode for MmError<E>
where
    E: HttpStatusCode + NotMmError,
{
    fn status_code(&self) -> StatusCode {
        self.etype.status_code()
    }
}

pub struct MmErrorTrace {
    trace: Vec<TraceLocation>,
}

impl MmErrorTrace {
    pub fn new(trace: Vec<TraceLocation>) -> MmErrorTrace {
        MmErrorTrace { trace }
    }
}

impl<E: NotMmError> MmError<E> {
    #[track_caller]
    pub fn new(etype: E) -> MmError<E> {
        let location = TraceLocation::from(Location::caller());
        MmError {
            etype,
            trace: vec![location],
        }
    }

    #[track_caller]
    pub fn new_with_trace(etype: E, mut trace: MmErrorTrace) -> MmError<E> {
        trace.trace.push(TraceLocation::from(Location::caller()));
        MmError {
            etype,
            trace: trace.trace,
        }
    }

    pub fn split(self) -> (E, MmErrorTrace) {
        (self.etype, MmErrorTrace::new(self.trace))
    }

    #[track_caller]
    pub fn map<MapE, F>(mut self, f: F) -> MmError<MapE>
    where
        MapE: NotMmError,
        F: FnOnce(E) -> MapE,
    {
        self.trace.push(TraceLocation::from(Location::caller()));
        MmError {
            etype: f(self.etype),
            trace: self.trace,
        }
    }

    #[track_caller]
    pub fn err<T>(etype: E) -> Result<T, MmError<E>> {
        Err(MmError::new(etype))
    }

    #[track_caller]
    pub fn err_with_trace<T>(etype: E, trace: MmErrorTrace) -> Result<T, MmError<E>> {
        Err(MmError::new_with_trace(etype, trace))
    }

    pub fn get_inner(&self) -> &E {
        &self.etype
    }

    pub fn into_inner(self) -> E {
        self.etype
    }

    /// Format the [`MmError::trace`] similar to JSON path notation: `mm2.lp_swap.utxo.rpc_client`.
    /// The return path is deduplicated.
    pub fn path(&self) -> String {
        self.trace
            .iter()
            .map(|src| src.file)
            .rev()
            // If we call functions x -> y -> z which are defined in the same module, the module
            // name would be duplicated three times in the path chain. `dedup` solves this issue,
            // and there is no need for a `sort` before this deduplication.
            .dedup()
            .collect::<Vec<_>>()
            .join(".")
    }

    /// Format the [`MmError::trace`] similar to stack trace: `mm2:379] lp_swap:21] utxo:1105] rpc_client:39]`.
    pub fn stack_trace(&self) -> String {
        self.trace
            .iter()
            .map(|src| src.formatted())
            .rev()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub trait FormattedTrace {
    fn formatted(&self) -> String;
}

/// The location where an error was tracked.
/// The location is formatted like this:
/// ```txt
/// location_file:379]
/// ```
#[derive(Clone, Debug, Display, Eq, PartialEq)]
#[display(fmt = "{file}:{line}]")]
pub struct TraceLocation {
    file: &'static str,
    line: u32,
}

impl From<&'static Location<'static>> for TraceLocation {
    fn from(location: &'static Location<'static>) -> Self {
        TraceLocation {
            file: filename(location.file()),
            line: location.line(),
        }
    }
}

impl FormattedTrace for TraceLocation {
    fn formatted(&self) -> String {
        self.to_string()
    }
}

impl TraceLocation {
    pub fn new(file: &'static str, line: u32) -> TraceLocation {
        TraceLocation { file, line }
    }

    pub fn file(&self) -> &'static str {
        self.file
    }

    pub fn line(&self) -> u32 {
        self.line
    }
}

impl<T: FormattedTrace> FormattedTrace for Vec<T> {
    fn formatted(&self) -> String {
        self.iter()
            .map(|src| src.formatted())
            .rev()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use common::block_on_f01;
    use futures01::Future;
    use ser_error_derive::SerializeErrorType;
    use serde_json::{self as json, json};

    enum ErrorKind {
        NotSufficientBalance { actual: u64, required: u64 },
    }

    #[derive(Display, Serialize, SerializeErrorType)]
    #[serde(tag = "error_type", content = "error_data")]
    enum ForwardedError {
        #[display(fmt = "Not sufficient balance. Top up your balance by {missing}")]
        NotSufficientBalance { missing: u64 },
    }

    impl From<ErrorKind> for ForwardedError {
        fn from(kind: ErrorKind) -> Self {
            match kind {
                ErrorKind::NotSufficientBalance { actual, required } => ForwardedError::NotSufficientBalance {
                    missing: required - actual,
                },
            }
        }
    }

    #[test]
    fn test_mm_error() {
        const GENERATED_LINE: u32 = line!() + 2;
        fn generate_error(actual: u64, required: u64) -> Result<(), MmError<ErrorKind>> {
            Err(MmError::new(ErrorKind::NotSufficientBalance { actual, required }))
        }

        const FORWARDED_LINE: u32 = line!() + 2;
        fn forward_error(actual: u64, required: u64) -> Result<(), MmError<ForwardedError>> {
            generate_error(actual, required).map_mm_err()?;
            unreachable!("'generate_error' must return an error")
        }

        let actual = 1000;
        let required = 1500;
        let missing = required - actual;
        let error = forward_error(actual, required).expect_err("'forward_error' must return an error");

        let expected_display = format!(
            "mm_error:{FORWARDED_LINE}] mm_error:{GENERATED_LINE}] Not sufficient balance. Top up your balance by {missing}"
        );
        assert_eq!(error.to_string(), expected_display);

        // the path is deduplicated
        let expected_path = "mm_error";
        assert_eq!(error.path(), expected_path);

        let expected_stack_trace = format!("mm_error:{FORWARDED_LINE}] mm_error:{GENERATED_LINE}]");
        assert_eq!(error.stack_trace(), expected_stack_trace);

        let actual_json = json::to_value(error).expect("!json::to_value");
        let expected_json = json!({
            "error": format!("Not sufficient balance. Top up your balance by {}", missing),
            "error_path": expected_path,
            "error_trace":expected_stack_trace,
            "error_type": "NotSufficientBalance",
            "error_data": {
                "missing": missing,
            }
        });
        assert_eq!(actual_json, expected_json);
    }

    #[test]
    fn test_map_error() {
        let res: Result<(), _> = Err("An error".to_string());

        let into_mm_with_line = line!() + 1;
        let mm_res = res.map_to_mm(|e| e.len()).expect_err("Expected MmError<usize>");
        assert_eq!(mm_res.etype, 8);
        assert_eq!(mm_res.trace, vec![TraceLocation::new("mm_error", into_mm_with_line)]);

        let error_line = line!() + 1;
        let mm_res: Result<(), _> = None.or_mm_err(|| "An error".to_owned());
        let mm_err = mm_res.expect_err("Expected MmError<String>");

        assert_eq!(mm_err.etype, "An error");
        assert_eq!(mm_err.trace, vec![TraceLocation::new("mm_error", error_line)]);
    }

    #[test]
    fn test_map_fut() {
        fn generate_error(desc: &str) -> Box<dyn Future<Item = (), Error = String> + Send> {
            Box::new(futures01::future::err(desc.to_owned()))
        }

        let into_mm_line = line!() + 2;
        let mm_err =
            block_on_f01(generate_error("An error").map_to_mm_fut(|error| error.len())).expect_err("Expected an error");
        assert_eq!(mm_err.etype, 8);
        assert_eq!(mm_err.trace, vec![TraceLocation::new("mm_error", into_mm_line)]);
    }

    #[derive(Display)]
    #[allow(dead_code)]
    enum ForwardedErrorWithBox {
        #[display(fmt = "Not sufficient balance. Top up your balance by {missing}")]
        NotSufficientBalance {
            missing: u64,
        },
        Box(Box<dyn std::error::Error>),
    }

    impl From<ErrorKind> for ForwardedErrorWithBox {
        fn from(kind: ErrorKind) -> Self {
            match kind {
                ErrorKind::NotSufficientBalance { actual, required } => ForwardedErrorWithBox::NotSufficientBalance {
                    missing: required - actual,
                },
            }
        }
    }

    #[test]
    // Testing that error conversion works for error containing Box<dyn ...>
    fn test_mm_error_with_box() {
        const GENERATED_LINE: u32 = line!() + 2;
        fn generate_error_for_box(actual: u64, required: u64) -> Result<(), MmError<ErrorKind>> {
            Err(MmError::new(ErrorKind::NotSufficientBalance { actual, required }))
        }

        const FORWARDED_LINE: u32 = line!() + 2;
        fn forward_error_for_box(actual: u64, required: u64) -> Result<(), MmError<ForwardedErrorWithBox>> {
            generate_error_for_box(actual, required).map_mm_err()?;
            unreachable!("'generate_error' must return an error")
        }

        let actual = 1000;
        let required = 1500;
        let missing = required - actual;
        let error = forward_error_for_box(actual, required).expect_err("'forward_error' must return an error");

        let expected_display = format!(
            "mm_error:{FORWARDED_LINE}] mm_error:{GENERATED_LINE}] Not sufficient balance. Top up your balance by {missing}"
        );
        assert_eq!(error.to_string(), expected_display);

        // the path is deduplicated
        let expected_path = "mm_error";
        assert_eq!(error.path(), expected_path);

        let expected_stack_trace = format!("mm_error:{FORWARDED_LINE}] mm_error:{GENERATED_LINE}]");
        assert_eq!(error.stack_trace(), expected_stack_trace);
    }
}
