// TODO: Remove this allow when Rust 1.92 regression is fixed.
// See: https://github.com/rust-lang/rust/issues/147648
#![allow(unused_assignments)]

#[cfg(target_arch = "wasm32")]
#[path = "indexed_db/indexed_db.rs"]
pub mod indexed_db;
