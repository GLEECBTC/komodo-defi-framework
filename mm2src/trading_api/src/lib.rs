//! This module is for indirect connection to third-party trading APIs, processing their results and errors

// TODO: Remove this allow when Rust 1.92 regression is fixed.
// See: https://github.com/rust-lang/rust/issues/147648
#![allow(unused_assignments)]

pub mod one_inch_api;
