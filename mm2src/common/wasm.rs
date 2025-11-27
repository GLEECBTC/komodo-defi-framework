use crate::filename;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_wasm_bindgen::Serializer;
use std::fmt;
use wasm_bindgen::prelude::*;

/// Get only the first line of the error.
/// Generally, the `JsValue` error contains the stack trace of an error.
/// This function cuts off the stack trace.
pub fn stringify_js_error(error: &JsValue) -> String {
    format!("{error:?}")
        .lines()
        .next()
        .map(|e| e.to_owned())
        .unwrap_or_default()
}

/// The function helper for the `WasmUnwrapExt`, `WasmUnwrapErrExt` traits.
#[track_caller]
fn caller_file_line() -> (&'static str, u32) {
    let location = std::panic::Location::caller();
    let file = filename(location.file());
    let line = location.line();
    (file, line)
}

pub trait WasmUnwrapExt<T> {
    fn unwrap_w(self) -> T;
    fn expect_w(self, description: &str) -> T;
}

pub trait WasmUnwrapErrExt<E> {
    fn unwrap_err_w(self) -> E;
    fn expect_err_w(self, description: &str) -> E;
}

impl<T, E: fmt::Debug> WasmUnwrapExt<T> for Result<T, E> {
    #[track_caller]
    fn unwrap_w(self) -> T {
        match self {
            Ok(t) => t,
            Err(e) => {
                let (file, line) = caller_file_line();
                let error = format!("{file}:{line}] 'Result::unwrap_w' called on an 'Err' value: {e:?}");
                wasm_bindgen::throw_str(&error)
            },
        }
    }

    #[track_caller]
    fn expect_w(self, description: &str) -> T {
        match self {
            Ok(t) => t,
            Err(e) => {
                let (file, line) = caller_file_line();
                let error = format!("{file}:{line}] {description}: {e:?}");
                wasm_bindgen::throw_str(&error)
            },
        }
    }
}

impl<T> WasmUnwrapExt<T> for Option<T> {
    #[track_caller]
    fn unwrap_w(self) -> T {
        match self {
            Some(t) => t,
            None => {
                let (file, line) = caller_file_line();
                let error = format!("{file}:{line}] 'Option::unwrap_w' called on a 'None' value");
                wasm_bindgen::throw_str(&error)
            },
        }
    }

    #[track_caller]
    fn expect_w(self, description: &str) -> T {
        match self {
            Some(t) => t,
            None => {
                let (file, line) = caller_file_line();
                let error = format!("{file}:{line}] {description}");
                wasm_bindgen::throw_str(&error)
            },
        }
    }
}

impl<T: fmt::Debug, E> WasmUnwrapErrExt<E> for Result<T, E> {
    #[track_caller]
    fn unwrap_err_w(self) -> E {
        match self {
            Ok(t) => {
                let (file, line) = caller_file_line();
                let error = format!("{file}:{line}] 'Result::unwrap_err_w' called on an 'Ok' value: {t:?}");
                wasm_bindgen::throw_str(&error)
            },
            Err(e) => e,
        }
    }

    #[track_caller]
    fn expect_err_w(self, description: &str) -> E {
        match self {
            Ok(t) => {
                let (file, line) = caller_file_line();
                let error = format!("{file}:{line}] {description}: {t:?}");
                wasm_bindgen::throw_str(&error)
            },
            Err(e) => e,
        }
    }
}

#[track_caller]
pub fn panic_w(description: &str) {
    let (file, line) = caller_file_line();
    let error = format!("{file}:{line}] 'panic_w' called: {description:?}");
    wasm_bindgen::throw_str(&error)
}

lazy_static! {
    static ref TO_JS_SERIALIZER: Serializer = Serializer::json_compatible();
}

#[inline]
pub fn serialize_to_js<T: Serialize>(value: &T) -> Result<JsValue, serde_wasm_bindgen::Error> {
    value.serialize(&*TO_JS_SERIALIZER)
}

#[inline]
pub fn deserialize_from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, serde_wasm_bindgen::Error> {
    serde_wasm_bindgen::from_value(value)
}
