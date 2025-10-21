#[cfg(not(target_arch = "wasm32"))]
pub mod file_lock;
#[cfg(not(target_arch = "wasm32"))]
pub mod fs;
