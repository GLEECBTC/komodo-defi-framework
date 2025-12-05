# mm2_bin_lib — Platform Entry Points

> **Note:** Always follow the root `/CLAUDE.md` for global conventions (fmt, clippy, error handling, etc.).

Thin wrapper providing platform-specific entry points for KDF. Bridges native, WASM, and mobile platforms to `mm2_main`.

## Responsibilities

- Platform-specific initialization (native/WASM/mobile)
- Singleton instance management (`LP_MAIN_RUNNING`)
- FFI/WASM bindings for external callers
- Lifecycle control (start, status, stop)
- Version information exposure

## Module Structure

```
src/
├── lib.rs            # Shared types, status checking, stop logic
├── mm2_bin.rs        # Native binary entry point (main())
├── mm2_native_lib.rs # C FFI for mobile/embedded (native only)
└── mm2_wasm_lib.rs   # WASM bindings (wasm32 only)
```

## Entry Points by Platform

| Platform | Entry Point | File |
|----------|-------------|------|
| Native CLI | `main()` | `mm2_bin.rs` |
| iOS/Android | `mm2_main(conf, log_cb)` | `mm2_native_lib.rs` |
| WASM | `mm2_main(params, log_cb)` | `mm2_wasm_lib.rs` |

## Core Types

### MainStatus

Status of the MM2 singleton:

```rust
pub enum MainStatus {
    NotRunning = 0,  // MM2 not started
    NoContext = 1,   // Running, no MmCtx yet
    NoRpc = 2,       // Context exists, RPC not ready
    RpcIsUp = 3,     // Fully operational
}
```

### StartupResultCode

Initialization result:

```rust
pub enum StartupResultCode {
    Ok = 0,
    InvalidParams = 1,
    ConfigError = 2,
    AlreadyRunning = 3,
    InitError = 4,
    SpawnError = 5,
}
```

### StopStatus

Shutdown result:

```rust
pub enum StopStatus {
    Ok = 0,
    NotRunning = 1,
    ErrorStopping = 2,
    StoppingAlready = 3,
}
```

## Native Library API (C FFI)

For mobile/embedded integration:

```c
// Start MM2 with JSON config and log callback
int8_t mm2_main(const char* conf, void (*log_cb)(const char*));

// Check running status (returns MainStatus)
int8_t mm2_main_status();

// Stop MM2 instance
int8_t mm2_stop();

// Run embedded tests
int32_t mm2_test(int32_t torch, void (*log_cb)(const char*));
```

## WASM API

For browser integration:

```typescript
// Start MM2
async function mm2_main(params: MainParams, log_cb: Function): Promise<number>;

// Check status
function mm2_main_status(): MainStatus;

// Make RPC call
async function mm2_rpc(payload: object): Promise<object>;

// Get version (works before mm2 starts)
function mm2_version(): { result: string, datetime: string };

// Stop MM2
async function mm2_stop(): Promise<void>;
```

### WASM Usage Example

```javascript
import init, { mm2_main, mm2_rpc, LogLevel } from "./kdflib.js";

const params = {
    conf: {
        gui: "WASMTEST",
        mm2: 1,
        passphrase: "...",
        rpc_password: "test123",
        coins: [{ coin: "ETH", protocol: { type: "ETH" }}]
    },
    log_level: LogLevel.Info,
};

await mm2_main(params, (level, line) => console.log(line));

const version = await mm2_rpc({ userpass: "test123", method: "version" });
```

## Initialization Flow

### Native
1. `main()` calls `mm2_main::mm2_main(version, datetime)`
2. Config loaded from CLI args or `MM2.json`
3. `lp_main()` initializes `MmCtx`
4. `lp_run()` spawned in "lp_run" thread

### Native Library (Mobile)
1. `mm2_main(conf, log_cb)` called via FFI
2. `init_crash_reports()` sets up panic handler
3. `register_callback()` configures logging
4. `block_on(lp_main())` initializes context
5. Thread spawned for `block_on(lp_run())`
6. Returns immediately, MM2 runs in background

### WASM
1. `mm2_main(params, log_cb)` called from JS
2. `set_panic_hook()` for error logging
3. `await lp_main()` initializes context
4. `spawn_local(lp_run())` runs event loop
5. Returns after initialization

## Singleton Management

Global state ensures single instance:

```rust
static LP_MAIN_RUNNING: AtomicBool = AtomicBool::new(false);
static CTX: AtomicU32 = AtomicU32::new(0);  // FFI handle to MmArc
```

- `LP_MAIN_RUNNING`: Guards against multiple starts
- `CTX`: Stores context handle for status/stop operations

## Interactions

| Crate | Usage |
|-------|-------|
| **mm2_main** | Core logic (`lp_main`, `lp_run`) |
| **mm2_core** | `MmArc` context management |
| **common** | Logging, crash reports, executor |
| **mm2_rpc** | WASM RPC bridge (wasm_rpc) |

## Platform-Specific Notes

### Native
- Uses jemalloc on Linux x86_64 GNU
- Spawns separate "lp_run" thread
- Supports `MM2.json` config file

### WASM
- Single-threaded (`spawn_local`)
- No `block_on` (panics)
- RPC via `mm2_rpc()` function, not HTTP

### Mobile
- Compiled as static library (`libkdf.a`)
- Log callback required for output
- Same API as native library

## Common Pitfalls

| Issue | Solution |
|-------|----------|
| "Already running" | Call `mm2_stop()` first |
| Status stuck at NoRpc | Wait for initialization to complete |
| WASM panic | Check browser console, `set_panic_hook` logs |
| Mobile logs missing | Ensure log callback is provided |

## Build Outputs

| Target | Output |
|--------|--------|
| Native | `kdf` binary |
| WASM | `kdflib_bg.wasm` + `kdflib.js` |
| iOS | `libkdf.a` (static library) |
| Android | `libkdf.a` (static library) |

## Tests

- Native: Run via `mm2_test()` FFI function
- WASM: Browser-based via `wasm-pack test`
