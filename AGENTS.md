# AGENTS.md — Komodo DeFi Framework

Guide for AI-assisted development. Keep changes small, follow patterns.

## Project Overview

Komodo DeFi Framework (KDF) is an open-source atomic-swap DEX enabling trustless P2P trading across blockchains. Core capabilities:
- **Atomic swaps** via HTLCs (Hash Time Locked Contracts)
- **Multi-chain support**: UTXO, EVM, Tendermint, Zcash, Lightning, Sia, Solana
- **Wallets**:
  - HD wallets: BIP39/BIP32/SLIP-10 derivation
  - Hardware: Trezor (native)
  - External: WalletConnect v2 (native + WASM), MetaMask (WASM only)

Targets: Linux (x86-64), macOS (x86-64, ARM64, Universal), Windows (x86-64), WASM, iOS (aarch64), Android (aarch64, armv7).

## Workspace Layout

All crates reside in `mm2src/`. Crates with AGENTS.md files are marked with `→`.

```
mm2src/
├── mm2_bin_lib/         # Platform entry points (native/WASM/mobile) → see AGENTS.md
├── mm2_main/            # App entry, RPC, swaps, ordermatch → see AGENTS.md
├── coins/               # Multi-protocol coin support → see AGENTS.md
│   └── utxo_signer/     # UTXO transaction signing (keypair/Trezor)
├── crypto/              # Key management, HD derivation → see AGENTS.md
├── mm2_core/            # MmArc/MmCtx central context, event dispatch
├── mm2_p2p/             # libp2p networking, gossipsub → see AGENTS.md
├── coins_activation/    # Coin activation flows → see AGENTS.md
├── trezor/              # Trezor hardware wallet → see AGENTS.md
├── mm2_bitcoin/         # UTXO primitives → see AGENTS.md
│   ├── chain/           # Block/transaction structures
│   ├── crypto/          # Hash functions (bitcrypto)
│   ├── keys/            # Address and key management
│   ├── primitives/      # H160, H256, U256 types
│   ├── script/          # Bitcoin scripting
│   ├── serialization/   # Binary encoding
│   ├── serialization_derive/
│   ├── rpc/             # RPC response types
│   ├── spv_validation/  # SPV proof verification
│   └── test_helpers/    # Testing utilities
├── common/              # Shared utilities → see AGENTS.md
│   └── shared_ref_counter/ # Debug-instrumented Arc
├── kdf_walletconnect/   # WalletConnect v2 protocol
├── mm2_event_stream/    # SSE streaming infrastructure
├── mm2_err_handle/      # MmError framework
├── mm2_net/             # HTTP/WebSocket/gRPC-web networking
├── mm2_rpc/             # RPC data types and protocol
├── mm2_db/              # IndexedDB wrapper (WASM only)
├── mm2_eth/             # Ethereum utilities, EIP-712
├── mm2_metamask/        # MetaMask integration (WASM only)
├── mm2_number/          # High-precision numerics (MmNumber)
├── mm2_state_machine/   # Generic state machine framework
├── mm2_metrics/         # Prometheus metrics
├── mm2_io/              # File I/O (native only)
├── mm2_git/             # GitHub API client
├── mm2_gui_storage/     # GUI persistence layer
├── rpc_task/            # Long-running RPC task framework
├── trading_api/         # External DEX integration (1inch)
├── proxy_signature/     # libp2p message signing for proxy auth
├── db_common/           # SQLite abstractions (native)
├── hw_common/           # Hardware wallet abstractions
├── ledger/              # Ledger device protocol (scaffolding only, not integrated)
├── derives/
│   ├── enum_derives/    # Enum conversion macros
│   ├── ser_error/       # Error serialization trait
│   └── ser_error_derive/ # Error serialization macro
└── mm2_test_helpers/    # Testing utilities (excluded from workspace)
```

## Global Conventions

### Performance
- Prefer optimal solutions over quick fixes—check other crates for existing efficient implementations before writing new code. Consider algorithmic complexity.

### Rust Style
- `cargo fmt` before commit
- `cargo clippy --all-targets --all-features -- -D warnings` (zero warnings)
- Prefer absolute imports from crate root over deep `super::` chains
- `async`/`await` only; avoid blocking in async context

### Error Handling (MmError)
```rust
use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use mm2_err_handle::prelude::*;
use ser_error_derive::SerializeErrorType;

#[derive(Display, Serialize, SerializeErrorType)]  // Debug optional
#[serde(tag = "error_type", content = "error_data")]
pub enum MyError {
    #[display(fmt = "Not found: {}", _0)]
    NotFound(String),
    #[display(fmt = "Internal error: {}", _0)]
    InternalError(String),
}

impl HttpStatusCode for MyError {
    fn status_code(&self) -> StatusCode {
        match self {
            MyError::NotFound(_) => StatusCode::NOT_FOUND,
            MyError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
// Usage: MmResult<T, MyError>, convert with .map_to_mm()?
```

### Platform Guards
```rust
// Attribute-based: for modules, functions, enum variants, impls, match arms
#[cfg(not(target_arch = "wasm32"))]  // Native only
pub mod lightning;

#[cfg(target_arch = "wasm32")]        // WASM only
fn wasm_only_fn() { }

// Macro-based: for grouping multiple imports
cfg_native! {
    use crate::lightning::LightningCoin;
    use std::path::PathBuf;
}
cfg_wasm32! {
    use mm2_db::indexed_db::SharedDb;
}
```

## Security Rules

### Sensitive Data
1. **Never log/serialize**: mnemonics, seeds, private keys, extended keys, session tokens
2. **Zeroize secrets on drop**: use `zeroize` crate for sensitive types (see `Bip39Seed`)
3. **Sanitize error messages**: no internal paths, keys, or sensitive data in errors

### Input Validation
4. **Validate all RPC inputs**: bounds, formats, existence checks
5. **Use strict types over strings**: prefer typed structs over raw `String`/`Value` for API boundaries
6. **Specify bounds**: use bounded integers, fixed-size arrays where lengths are known

### Code Safety
7. **No `unwrap()`/`expect()`** in RPC paths without justification
8. **Avoid panics**: return `MmError` instead of panicking in library code

*Note: Codebase is progressively improving; some legacy code may not follow all rules.*

*When working on any code, if you identify a security-related pattern that could be generalized as a rule, propose adding it here.*

## Build & Test

```bash
# Build
cargo build --release

# Unit tests
cargo test --bins --lib

# Integration tests
cargo test --test 'mm2_tests_main'

# Docker tests
cargo test --test 'docker_tests_main' --features run-docker-tests

# Clippy (must pass)
cargo clippy --all-targets --all-features -- -D warnings
```

See `docs/DEV_ENVIRONMENT.md` for full setup and running specific tests.

## Testing

- **Bug fixes**: Prefer writing a failing test first, then fix the bug
- **New features**: Scaffold first, then prefer writing tests before implementing logic where practical
- Always run new/modified tests in isolation to verify they pass
- After large features or refactors, suggest running the full test suite to check for regressions
- Use `#[serde(deny_unknown_fields)]` in test deserialization structs to catch unexpected fields

## CI/CD

- **Workflows**: `.github/workflows/`
  - `test.yml` — Unit, integration, docker, and WASM tests
  - `fmt-and-lint.yml` — Format and clippy (native + WASM)
  - `dev-build.yml` — Dev builds for all targets
  - `release-build.yml` — Release builds on `main`
- **Toolchain**: `stable` (see `rust-toolchain.toml`)
- **Builds**: Linux, macOS (x86/ARM/Universal), Windows, WASM, iOS, Android
- **Docker**: Images pushed on `dev` and `main` branches

## RPC Overview

| Namespace | Example | Purpose |
|-----------|---------|---------|
| (none) | `"withdraw"` | Stable APIs |
| `task::` | `"task::withdraw::init"` | Long-running ops (init/status/user_action/cancel) |
| `stream::` | `"stream::balance::enable"` | SSE subscriptions |
| `gui_storage::` | `"gui_storage::add_account"` | GUI state persistence |
| `lightning::` | `"lightning::channels::open_channel"` | Lightning Network (native only) |
| `experimental::` | `"experimental::..."` | Unstable APIs (may have sub-namespaces) |

See `mm2_main/src/rpc/dispatcher/dispatcher.rs` for all methods, `mm2_main/AGENTS.md` for adding new handlers.

## Key File Locations

| Component | Location |
|-----------|----------|
| MmCtx (central context) | `mm2_core/src/mm_ctx.rs` |
| RPC dispatcher | `mm2_main/src/rpc/dispatcher/dispatcher.rs` |
| RPC handlers | `mm2_main/src/rpc/lp_commands/` |
| Order matching | `mm2_main/src/lp_ordermatch.rs` |
| Swap V1 | `mm2_main/src/lp_swap/{maker,taker}_swap.rs` |
| Swap V2 | `mm2_main/src/lp_swap/{maker,taker}_swap_v2.rs` |
| Watchers | `mm2_main/src/lp_swap/swap_watcher.rs` |
| Coin traits | `coins/lp_coins.rs` |
| CryptoCtx | `crypto/src/crypto_ctx.rs` |
| HD derivation | `crypto/src/global_hd_ctx.rs` |
| P2P behaviour | `mm2_p2p/src/behaviours/atomicdex.rs` |
| Coin activation | `coins_activation/src/platform_coin_with_tokens.rs` |

## Working on Large Features

For significant features or large refactors, make small, self-contained commits incrementally. Each commit should be a logical unit that leaves the codebase working.

## Keeping Documentation Current

Update relevant AGENTS.md files when changing module structure, key types, patterns, or conventions.

## Common Pitfalls

| Issue | Solution |
|-------|----------|
| Wrote suboptimal code when efficient implementation existed in another crate | Search other crates for reusable functions; make private functions public if needed |
| Large refactor done in one massive commit | Break into small, self-contained commits as you work |
| Changed public API but didn't update AGENTS.md | Update documentation alongside code changes |
| Compared against wrong branch (e.g., deprecated `mm2.1`) | Use `git merge-base HEAD origin/dev origin/staging origin/main` to find the common ancestor, or ask the user which branch the feature is based on. Branch hierarchy: `main` ← `staging` ← `dev` ← feature branches |

## Documentation

- `README.md` — Build overview
- `docs/DEV_ENVIRONMENT.md` — Full test setup
- `docs/WASM_BUILD.md` — WASM build setup
- `docs/PR_REVIEW_CHECKLIST.md` — PR review checklist
- `docs/CONTRIBUTING.md` — Contribution guidelines
- `docs/GIT_FLOW_AND_WORKING_PROCESS.md` — Branch strategy
