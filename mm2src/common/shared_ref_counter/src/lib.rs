//! `SharedRc` is a thread-safe reference-counting pointer based on `Arc` that stands for 'Atomically
//! Reference Counted'.
//! The main difference from `Arc` is that `SharedRc` allows developers to debug hanging pointers
//! by collecting the location where all `SharedRc` pointers to the same allocation still exist.
//!
//! # Optimization
//!
//! `shared-ref-counter` works exactly the same as `Arc` if the `enable` feature is not activated.
//! It means that `SharedRc` doesn't collect any extra data without `enable` feature.
//!
//! # Enable
//!
//! Add the `enable` feature to the `Cargo.toml` to enable collecting the location of `SharedRc` pointers:
//! ```toml
//! shared_ref_counter = { version = "0.1", features = "enable" }
//! ```
//!
//! Please note `enable` feature should be used for **debug** purposes only.
//!
//! # Panic
//!
//! Some operations over `SharedRc` may lead to a panic if the `enable` features is activated.
//! This behavior is considered acceptable since the `enable` feature is expected to be used for **debug** purposes only.

#[cfg(not(feature = "enable"))]
mod disable;
#[cfg(feature = "enable")]
mod enable;

#[cfg(not(feature = "enable"))]
pub use disable::{SharedRc, WeakRc};
#[cfg(feature = "enable")]
pub use enable::{SharedRc, WeakRc};
