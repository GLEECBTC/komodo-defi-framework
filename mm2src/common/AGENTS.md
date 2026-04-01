# common — Shared Utilities

> **Note:** Always follow the root `/CLAUDE.md` for global conventions (fmt, clippy, error handling, etc.).

Foundation crate providing utilities used across all KDF crates. Platform-aware (native/WASM).

## Responsibilities

- Async task execution and abortion (`executor`)
- Logging infrastructure (native and WASM)
- Time utilities (`now_ms`, `now_sec`, `get_utc_timestamp`)
- RPC response helpers (`HyRes`, `rpc_response`)
- Platform-specific abstractions
- Shared constants and types

## Module Structure

```
├── common.rs              # Main module, re-exports, helpers
├── log.rs                 # Logging (includes log/ submodule)
├── executor/              # Async task management
│   ├── mod.rs            # spawn, Timer, AbortSettings
│   ├── native_executor.rs # Tokio-based (native)
│   ├── wasm_executor.rs   # Browser-based (WASM)
│   ├── abortable_system/  # AbortableQueue, graceful shutdown
│   └── spawner.rs         # SpawnFuture trait
├── log/                   # Logging implementations
│   ├── native_log.rs     # File/stdout logging
│   └── wasm_log.rs       # console.log
├── custom_futures/        # Future utilities (timeout, repeatable)
├── jsonrpc_client.rs      # JSON-RPC macros
├── password_policy.rs     # Password validation
├── crash_reports.rs       # Panic/signal handlers
├── wio.rs / wasm.rs       # Platform I/O
└── write_safe/            # Safe write abstractions
```

## Executor Module

### Spawning Tasks

```rust
use common::executor::{spawn, Timer, SpawnFuture};

// Spawn fire-and-forget task
spawn(async { /* ... */ });

// Spawn with abort handle
let handle = spawn_abortable(async { /* ... */ });
drop(handle); // Aborts the task

// Sleep
Timer::sleep(1.5).await; // 1.5 seconds
```

### AbortableQueue

Manages groups of tasks that can be aborted together:

```rust
use common::executor::abortable_queue::AbortableQueue;

let queue = AbortableQueue::default();
let spawner = queue.weak_spawner();

// Spawn task that will be aborted when queue is dropped
spawner.spawn(async { /* ... */ });

// Graceful shutdown
queue.abort_all().await;
```

### SpawnFuture Trait

Abstraction for spawning futures:

```rust
pub trait SpawnFuture: Send + Sync + 'static {
    fn spawn<F>(&self, f: F) where F: Future<Output = ()> + Send + 'static;
}
```

## Logging

### Macros

```rust
use common::log;

log::info!("Message");
log::error!("Error: {}", err);
log::debug!("Debug info");

// Human-readable log (adds location)
log!("Custom message");
```

### Custom Log Callback

```rust
use common::log::{register_callback, LogCallback};

// Set custom handler (e.g., for GUI)
register_callback(my_callback);
```

## Time Utilities

```rust
use common::{now_ms, now_sec, wait_until_ms, get_utc_timestamp};

let timestamp_ms = now_ms();           // Current time in milliseconds
let timestamp_sec = now_sec();         // Current time in seconds
let deadline = wait_until_ms(5000);    // 5 seconds from now
let utc = get_utc_timestamp();         // UTC timestamp (i64)
```

## RPC Helpers

```rust
use common::{rpc_response, rpc_err_response, HyRes, HttpStatusCode};

// Success response
fn handler() -> HyRes {
    rpc_response(200, json!({"result": "ok"}).to_string())
}

// Error response (logs automatically)
fn error_handler() -> HyRes {
    rpc_err_response(400, "Invalid request")
}

// HttpStatusCode trait for errors
impl HttpStatusCode for MyError {
    fn status_code(&self) -> StatusCode { ... }
}
```

## Constants

```rust
// DEX fee addresses
pub const DEX_FEE_ADDR_PUBKEY: &str = "0348685437...";
pub const DEX_BURN_ADDR_PUBKEY: &str = "0348685437..."; // Same as fee (burn disabled)
// Satoshis conversion
pub const SATOSHIS: u64 = 100_000_000;
pub fn sat_to_f(sat: u64) -> f64;
```

## Platform Macros

```rust
// Conditional compilation helpers
cfg_native! {
    // Native-only code
}

cfg_wasm32! {
    // WASM-only code
}
```

## Useful Types

| Type | Purpose |
|------|---------|
| `bits256` | 32-byte hash type with hex display |
| `HyRes` | Legacy RPC response future |
| `SuccessResponse` | Standard `{"result": "success"}` |
| `PagingOptions` | Pagination parameters |
| `OrdRange<T>` | Ordered inclusive range |

## Key Functions

| Function | Purpose |
|----------|---------|
| `block_on(future)` | Block on async (native only) |
| `small_rng()` | Seeded random number generator |
| `os_rng(&mut buf)` | Cryptographic random bytes |
| `median(&mut slice)` | Calculate median value |
| `sha256_digest(path)` | File hash |
| `kdf_app_dir()` | Application directory path |

## Interactions

This crate is imported by virtually all other crates in the workspace:

| Crate | Usage |
|-------|-------|
| **All crates** | Logging, time, executor utilities |
| **mm2_core** | MmCtx uses AbortableQueue for task management |
| **mm2_main** | RPC handlers use response helpers |
| **mm2_p2p** | SpawnFuture trait for async tasks |
| **coins** | Time utilities, DEX fee constants |

## Sub-Crate

- **shared_ref_counter** (`common/shared_ref_counter/`) — Debug-instrumented Arc alternative with optional allocation site tracking (enable with `enable` feature)

## Platform Differences

| Feature | Native | WASM |
|---------|--------|------|
| `spawn()` | Tokio runtime | `wasm_bindgen_futures` |
| `Timer::sleep()` | `tokio::time` | `setTimeout` |
| `block_on()` | Works | Panics |
| `LOG_FILE` | File output | N/A |
| `writeln()` | stdout/file | `console.log` |

## Common Pitfalls

| Issue | Solution |
|-------|----------|
| `block_on` in WASM | Use `.await` instead |
| Missing logs | Check `MM_LOG` env var |
| Task not aborting | Use `spawn_abortable` or `AbortableQueue` |

## Tests

- Unit: `cargo test -p common --lib`
