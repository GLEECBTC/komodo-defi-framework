#[cfg(not(target_arch = "wasm32"))]
pub mod sse_handler;
#[cfg(target_arch = "wasm32")]
pub mod wasm_event_stream;
