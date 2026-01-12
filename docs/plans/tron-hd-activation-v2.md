# TRX (TRON) HD + Activation via `EthCoin` — Architecture / Refactor Plan

> **Document scope**: design + implementation plan only (no code in this doc).
>
> **Hard constraint**: TRX must reuse the existing RPC routes:
> - request/response: `enable_eth_with_tokens`
> - task-based: `task::enable_eth::{init,status,user_action,cancel}`
>
> **No new dispatcher methods** (do not add `enable_trx...` routes). TRX flows through the ETH platform activation pipeline.

---

## Implementation Status (Updated 2025-12-31)

### Completed ✅
| Item | Location | Notes |
|------|----------|-------|
| TRON RPC client (`TronApiClient`) | `mm2src/coins/eth/tron/api.rs` | HTTP client with native + WASM support, node rotation, error parsing |
| TRON RPC wrapper (`TronRpcClient`) | `mm2src/coins/eth/chain_rpc/tron_rpc.rs` | Implements `ChainRpcOps` for TRON |
| `ChainRpcOps` trait | `mm2src/coins/eth/chain_rpc.rs` | `current_block()`, `balance_native()`, `is_address_used_basic()` |
| TRX stubs removed | `mm2src/coins/eth.rs`, `eth_rpc.rs` | `current_block()` and `balance()` use TRON RPC via `rpc_client` |
| Wallet-only activation | `mm2src/coins/eth/v2_activation.rs` | TRX skips swap contract validation |
| Token/NFT rejection | `mm2src/coins_activation/src/eth_with_token_activation.rs` | TRON rejects ERC20/NFT requests early |
| HD scanning | `mm2src/coins/eth/eth_hd_wallet.rs` | Uses `ChainRpcOps::is_address_used_basic()` for TRON |
| Platform balance fix | `mm2src/coins/eth.rs` | `platform_coin_balance()` is chain-aware |
| TRON address type | `mm2src/coins/eth/tron/address.rs` | `TronAddress` with Base58Check encode/decode |
| Integration tests | `mm2src/coins/eth/tron/api_integration_tests.rs` | Tests for Nile testnet (gated behind feature) |
| TRON address formatting | `mm2src/coins/eth/chain_address.rs` | `ChainTaggedAddress` wraps address with `ChainFamily`; HD and `my_address()` return `T...` for TRX |
| `address_by_coin_conf_and_pubkey_str` for TRX | `mm2src/coins/lp_coins.rs:5975-5982` | Returns Base58 format via `TronAddress.to_base58()` |
| TRX activation test helpers | `mm2src/mm2_test_helpers/src/for_tests.rs` | `enable_trx()`, `task_enable_trx()`, `task_enable_trx_result()`, `trx_conf()` helpers |
| TRX HD activation integration tests | `mm2src/mm2_main/tests/mm2_tests/tron_tests.rs` | 10 tests covering: immediate/task activation, node failover, HD activation, get_new_address, balance structure, gap limit scanning, used-but-zero-balance detection |
| `TaskEnableError` type | `mm2src/mm2_test_helpers/src/structs.rs` | Non-panicking error type for task enable helpers |

### In Progress 🔄
| Item | Notes |
|------|-------|
| Chain-aware address parsing (Section 7.4) | `ChainTaggedAddress::from_str_with_family()` exists but `valid_addr_from_str()` is still EVM-only; not blocking MVP |

### Not Started 📋
| Item | Notes |
|------|-------|
| *None - MVP complete* | |

### Post-MVP Architecture (See `chain-rpc-client-refactor.md`)

After MVP ships, the following architectural improvements are planned:

| PR | Purpose | Description |
|----|---------|-------------|
| **PR-4.5** | ChainBackend Composition | Unify `chain_spec` + `rpc_client` into single `ChainBackend` enum; eliminates redundancy |
| **PR-5-9** | EVM RPC Refactor | Move `web3_instances` into `EvmRpcClient`, broadcast sessions, receipt finality |
| **PR-X** | ChainCoin Typed Model | Make invalid chain×asset combinations unrepresentable at the type level |

**Key concepts:**
- **Two orthogonal dimensions**: Chain (Evm/Tron) × Asset (Native/Token/Nft)
- **ChainBackend**: Single source of truth for chain identity + RPC
- **ChainCoin (future)**: Nested enum where chain variants contain only valid assets for that chain
- **AssetKind rename**: `EthCoinType::Eth` → `AssetKind::Native` (deferred, mechanical)

See `.claude/plans/chain-rpc-client-refactor.md` sections 19-20 for full architectural details.

---

## 0) Goals / Non‑Goals

### Goals (this plan)
1. **Correct address format** for TRX everywhere activation/HD wallet touches:
   - HD activation must return `T...` (Base58Check) addresses, not EVM `0x...`.
   - Single-address `my_address()` for TRX must return `T...`.
2. **Remove TRX stubs** used by activation/HD flows:
   - Replace `EthCoin::current_block()` TRX stub in `mm2src/coins/eth.rs`.
   - Replace `EthCoin::balance()` TRX stub in `mm2src/coins/eth/eth_rpc.rs`.
3. **Wallet-only TRX activation**:
   - TRX activation must *not* require swap contracts.
   - TRX must *not* run EVM swap-contract validation paths.
4. **Architecture direction**:
   - Stop using `EthCoin::is_tron()` as the "chain boundary".
   - Move toward a **backend / trait-based** architecture so call sites don't `match chain_spec` long-term.
   - Keep changes incremental: allow some `match ChainSpec` early, but concentrate it into a single backend layer ASAP.
5. **Testing parity**:
   - Mirror existing ETH activation test patterns for TRX (same RPC routes, similar helpers, same response enums).

### Non-goals (explicitly out of scope for this plan)
- TRC20 activation (tokens on TRON)
- TRON transaction signing, withdraw, swaps, watchers
- TRON fee estimation (bandwidth/energy), swap-contract deployment, any trading support

---

## 0.1 TRX Activation MVP (Priority) — Scope, Guardrails, and File Touch List

### Why "MVP first"
We want **TRX wallet-only activation with correct HD + address formatting** shipped *before* investing in the broader
`ChainRpcClient` / `EvmRpcClient` refactor. The MVP must:
- Deliver working TRX activation through the **existing ETH activation routes** (`enable_eth_with_tokens` + `task::enable_eth::*`)
- Fix correctness gaps (addresses, balances, current block, HD "is used")
- Avoid "half-refactors" that increase blast radius or make the later refactor harder

> **Post‑MVP** work (e.g. moving `web3_instances` into `EvmRpcClient`, broadcast session abstraction, receipt finality policies)
> is still planned and documented (see `chain-rpc-client-refactor.md`), but is explicitly *not required* to ship TRX activation.

---

### MVP Definition (what "done" means)

#### MVP deliverables (must-have)
1. **Activation succeeds via existing routes**:
   - request/response: `enable_eth_with_tokens`
   - task-based: `task::enable_eth::{init,status,user_action,cancel}`
   - **No new dispatcher methods**.

2. **TRX addresses are correct everywhere activation/HD touches**:
   - HD activation returns `T...` (Base58Check) addresses
   - `my_address()` for TRX returns `T...`
   - No `0x...` user-facing formatting for TRX

3. **Remove TRX "stubs" for activation/HD correctness** (no "fake 0" data):
   - TRX `current_block()` uses TRON RPC (`/wallet/getnowblock`)
   - TRX `balance()` uses TRON RPC (`/wallet/getaccount`)
   - HD scanning uses TRON "address used" detection (via account existence / metadata)

4. **Wallet-only semantics are enforced for TRX activation**:
   - TRX activation must not require swap contracts
   - TRX activation must not run EVM swap-contract validation paths
   - TRON tokens/NFT activation requests are rejected (TRC20, NFTs out of scope)

5. **Refactor-friendly implementation**:
   - No new `coin.is_tron()` usage introduced
   - No `rpc_client.is_some()` used as a chain detector
   - Keep changes incremental, but keep chain branching localized and explicit

#### Explicit non-goals for MVP (deferred)
- TRC20 activation and balances
- TRON tx signing / withdraw / swaps / watchers
- TRON fee estimation (bandwidth/energy)
- Implementing `ChainTxOps` / broadcast sessions / receipt finality logic
- Moving `web3_instances` out of `EthCoinImpl` (that is **post‑MVP refactor** work)
- Directory restructures (new folders/moves)

---

### MVP Guardrails (do these to avoid making the later refactor harder)

1. **Never detect TRON by `Option` presence**
   - Do **not** write: `if self.rpc_client.is_some() { /* TRON */ }`
   - Do **not** write: `if let Some(client) = rpc_client { /* TRON */ }`
   - Instead, explicitly match the variant:
     - `match self.rpc_client { Some(ChainRpcClient::Tron(_)) => ..., _ => ... }`
   - Or match `ChainSpec` where appropriate:
     - `match self.chain_spec { ChainSpec::Tron{..} => ..., ChainSpec::Evm{..} => ... }`

   This prevents the "`rpc_client.is_some()` trap" documented in `chain-rpc-client-refactor.md`
   when `ChainRpcClient::Evm` is introduced.

2. **Avoid spreading chain conditionals**
   - It is OK for MVP to use `match ChainSpec` in a small number of high‑level call sites,
     but do not scatter "TRON special cases" across unrelated modules.
   - Prefer to consolidate: address formatting + HD scanning + balance/current_block.

3. **Keep internal address bytes stable**
   - Continue using `ethereum_types::Address` internally as the raw 20-byte address.
   - TRON formatting/parsing is only a codec layer.

4. **Do not introduce transaction pipeline abstractions in MVP**
   - MVP should not add `ChainTxOps`, `BroadcastSessionOps`, "prepare_broadcast", etc.
   - Those belong to the `ChainRpcClient` refactor plan.

---

### MVP: Expected Code Changes (files + dependencies)

> The list below is the **expected blast radius for MVP**. Post‑MVP refactor files remain documented elsewhere and should not
> be pulled into MVP unless they are required for the guardrails above.

#### Core files expected to change (MVP)
| Area | Files | Why | Key dependencies / notes |
|---|---|---|---|
| Activation builder | `mm2src/coins/eth/v2_activation.rs` | Build TRON RPC backend for TRX; enforce wallet-only TRON rules; avoid building EVM web3 for TRON | depends on `mm2_net` HTTP patterns, `url`, `serde_json`, existing `EthActivationV2Request` |
| Activation pipeline (platform+tokens) | `mm2src/coins_activation/src/eth_with_token_activation.rs` | Reject token/NFT requests for TRON early; ensure `enable_eth_with_tokens` stays the only route | keep request schema stable; depends on `CoinProtocol` → `ChainSpec` mapping |
| TRON RPC client (HTTP) | `mm2src/coins/eth/tron/api.rs` | Implement/solidify `get_now_block_number`, `get_account`, and error parsing; ensure WASM + native paths | uses `mm2_net::transport::slurp_req` (native) and `mm2_net::wasm::http::FetchRequest` (wasm); proxy signing via `proxy_signature`, `mm2_p2p::Keypair` |
| TRON RPC wrapper | `mm2src/coins/eth/chain_rpc/tron_rpc.rs` | Provide `ChainRpcOps` for TRON (current_block, balance_native, is_address_used_basic) | depends on `TronApiClient`, `TronAddress`, `MmError<Web3RpcError>` conversion conventions |
| Chain RPC enum boundary (minimal) | `mm2src/coins/eth/chain_rpc.rs` | Ensure variant matching is explicit; avoid `is_some()` chain detection patterns | MVP should keep API minimal; no EVM client added in MVP |
| Replace stubs: block + balance | `mm2src/coins/eth.rs`, `mm2src/coins/eth/eth_rpc.rs` | TRX current_block and balance must use TRON RPC (no stub values) | keep EVM behavior unchanged |
| HD wallet scanning + balances | `mm2src/coins/eth/eth_hd_wallet.rs` | TRX "is address used" + known balance must route to TRON backend and not query EVM token logic | must remain compatible with `coin_balance::HDAddressBalanceScanner` |
| TRON address conversions | `mm2src/coins/eth/tron/address.rs` | Add missing conversions (e.g. TRON → raw 20-byte), Base58 formatting, parse helpers | depends on `bs58`, existing constants/prefix rules |
| User-facing address formatting | `mm2src/coins/eth.rs`, `mm2src/coins/eth/eth_hd_wallet.rs` | Ensure `my_address()` and HD activation outputs `T...` for TRX | implement via a small wrapper type (recommended) or localized codec |

#### Additional "edge" file that may need MVP edits
| File | Why |
|---|---|
| `mm2src/coins/lp_coins.rs` | `address_by_coin_conf_and_pubkey_str()` currently errors for `CoinProtocol::TRX`. Fixing this removes a consistency gap and prevents future "TRX address cannot be derived" issues. |

#### Dependencies (expected to be used, not necessarily modified)
- HTTP + WASM compat: `mm2src/mm2_net/src/transport.rs`, `mm2src/mm2_net/src/native_http.rs`, `mm2src/mm2_net/src/wasm/http.rs`
- Proxy signing: `mm2src/proxy_signature/src/lib.rs`
- EVM transport patterns for reference parity: `mm2src/coins/eth/web3_transport/http_transport.rs`

---

### MVP ↔ Post‑MVP Boundary (what stays clean for later refactor)
- MVP may add/adjust **TRON-only RPC client code**, but must not restructure EVM code.
- MVP should **not** attempt to unify EVM + TRON call paths beyond:
  - explicit variant matching (no `is_some()` traps),
  - chain-aware address formatting (TRX `T...`).
- After MVP ships, proceed with the `chain-rpc-client-refactor.md` PR stack (EVM client + broadcast sessions + finality).

---

## 1) Current Architecture Snapshot (what we're working with)

### Protocol & activation pipeline (already correct)
- Protocol enum: `CoinProtocol::TRX { network }` in `mm2src/coins/lp_coins.rs`.
- Protocol → chain mapping:
  - `TryFromCoinProtocol for ChainSpec` maps TRX to `ChainSpec::Tron { network }`
  - This mapping lives in `mm2src/coins_activation/src/eth_with_token_activation.rs`.
- Activation entrypoints:
  - `enable_eth_with_tokens` → `coins_activation::enable_platform_coin_with_tokens::<EthCoin>()`
  - `task::enable_eth::*` → same activation path, task-wrapped
  - Dispatcher already routes these in `mm2src/mm2_main/src/rpc/dispatcher/dispatcher.rs`.

### Known TRX gaps/stubs
- **Fake current block** for TRX:
  - `EthCoin::current_block()` returns a stub when `coin.is_tron()` (`mm2src/coins/eth.rs`).
- **Fake balance** for TRX:
  - `EthCoin::balance()` in `eth_rpc.rs` returns `0 SUN` when `self.is_tron()` (`mm2src/coins/eth/eth_rpc.rs`).
- **EVM-centric address formatting**
  - HD wallet display uses checksum `0x...` via `DisplayAddress for ethereum_types::Address` (`mm2src/coins/eth/eth_hd_wallet.rs`).
  - `MarketCoinOps::my_address()` returns checksum `0x...` (`mm2src/coins/eth.rs`).
- **Swap contract validation is unconditional** in v2 activation:
  - `eth_coin_from_conf_and_request_v2()` rejects `swap_contract_address == 0x0` today (`mm2src/coins/eth/v2_activation.rs`).
  - That makes wallet-only TRX activation impossible through the existing route.
- **TRX pubkey→address helper is missing**
  - `address_by_coin_conf_and_pubkey_str()` returns error for `CoinProtocol::TRX` (`mm2src/coins/lp_coins.rs`).

---

## 2) Design Principles (Rust + codebase idioms)

1. **Single abstraction boundary**: concentrate chain differences behind a small backend layer.
   - Call sites should call `coin.chain().balance(...)` (or equivalent), not `if is_tron { ... }`.
2. **Prefer enum-backed polymorphism first**, then trait objects later if needed.
   - An `enum ChainBackend { Evm(...), Tron(...) }` implementing a trait is:
     - idiomatic Rust,
     - avoids object-safety constraints early,
     - removes repetitive `match` at call sites by centralizing it.
3. **Keep internal address type stable**: continue using `ethereum_types::Address` as the *raw* 20-byte address.
   - TRON's user-facing format is different; the raw bytes are still 20 bytes.
4. **Avoid global impl changes** that would break EVM:
   - Do **not** change `impl DisplayAddress for ethereum_types::Address` to return TRON Base58.
   - Instead introduce a wrapper type for HD flows where display is chain-dependent.

---

## 3) Proposed Module / File Structure (incremental)

### Target layout inside `mm2src/coins/eth/`
This layout supports adding TRON without exploding `eth.rs` further:

```
mm2src/coins/eth/
├── mod.rs / eth.rs (existing public EthCoin surface; gradually shrinks)
├── v2_activation.rs (existing; updated to create backend)
├── hd_wallet.rs (existing eth_hd_wallet.rs; updated to use chain-aware address wrapper)
├── chain_backend.rs    # NEW: ChainBackend enum, ChainKind, traits
├── tron/
│   ├── mod.rs          # existing: re-export address + add rpc
│   ├── address.rs      # existing: TronAddress type
│   └── api.rs          # NEW: TRON HTTP RPC client
└── eth_rpc.rs (existing; EVM-only)
```

**Key decision (validated by RepoPrompt review)**: **Delay directory restructure**.
- Do NOT create `chain/evm.rs` or move files in the first iteration.
- Add new modules (`chain_backend.rs`, `tron/api.rs`) alongside existing files.
- Keep `eth_rpc.rs` where it is - `EvmBackend` will wrap it, not replace it.
- This minimizes blast radius and keeps EVM paths unchanged.

**Incremental migration path**:
1. Add `chain_backend.rs` with `ChainBackend` enum
2. Add `tron/api.rs` for TRON HTTP client
3. Wire `EthCoinImpl` to use `ChainBackend`
4. Later (out of scope): consider moving to `chain/` directory structure

---

## 4) Core Abstractions (the "backend layer")

### 4.1 Use Existing `ChainSpec`
Location: `mm2src/coins/eth.rs` (already exists)

Use the existing `ChainSpec` enum for chain identity/config:

```rust
pub enum ChainSpec {
    Evm { chain_id: u64 },
    Tron { network: tron::Network },
}
```

**Only introduce `ChainFamily` if needed** for HD display wrapper where `chain_id` shouldn't affect equality:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChainFamily { Evm, Tron }

impl From<&ChainSpec> for ChainFamily {
    fn from(spec: &ChainSpec) -> Self {
        match spec {
            ChainSpec::Evm { .. } => ChainFamily::Evm,
            ChainSpec::Tron { .. } => ChainFamily::Tron,
        }
    }
}
```

**When to use which:**
- `ChainSpec` - activation validation, rpc construction, feature gating (most cases)
- `ChainFamily` - address formatting, HD display wrapper (where chain_id shouldn't matter)

### 4.2 `ChainAddressCodec`

Purpose: parse/format addresses for user-facing APIs.

```rust
pub trait ChainAddressCodec {
    fn kind(&self) -> ChainKind;

    /// User-facing string (EVM checksum `0x...` vs TRON base58 `T...`).
    fn format_address(&self, raw: ethereum_types::Address) -> String;

    /// Parse a user-provided address string into raw 20 bytes.
    fn parse_address(&self, s: &str) -> Result<ethereum_types::Address, String>;
}
```

- EVM: checksum format + existing `valid_addr_from_str`.
- TRON: accept:
  - Base58 (`T...`)
  - hex `41...` (with/without `0x`), using `mm2src/coins/eth/tron/address.rs`
  - convert to raw `ethereum_types::Address` by dropping the `0x41` prefix.

### 4.3 `ChainRpcOps` (only what activation/HD needs)

> **Related**: See `docs/plans/chain-rpc-client-refactor.md` for the complete ChainRpcClient architecture including transaction abstraction.

Purpose: remove TRX stubs and support HD gap-limit scanning.

**Design decision**: Use associated types for maximum flexibility. No `&EthCoin` parameter - implementors own their state.

```rust
#[async_trait]
pub trait ChainRpcOps: Send + Sync + std::fmt::Debug {
    type Error;
    type Address;
    type Balance;

    /// Get the current block number.
    async fn current_block(&self) -> Result<u64, Self::Error>;

    /// Get native token balance in smallest unit.
    async fn balance_native(&self, address: Self::Address) -> Result<Self::Balance, Self::Error>;

    /// Basic address usage check for HD wallet gap-limit scanning.
    /// Does NOT check token balances - that's done at HD scanner level.
    async fn is_address_used_basic(&self, address: Self::Address) -> Result<bool, Self::Error>;
}
```

**Additional traits** (defined in `chain-rpc-client-refactor.md`):
- `ChainTxOps` - Transaction pipeline abstraction (`prepare_broadcast` returns a session)
- `BroadcastSessionOps` - Consume-on-use broadcast with captured nodes and chain-specific `TxContext`

**TRON implementation** (`TronRpcClient` in `chain_rpc/tron_rpc.rs`):
- `type Error = MmError<TronRpcError>`
- `type Address = TronAddress`
- `type Balance = U256`
- Delegates to `TronApiClient`

**EVM implementation** (`EvmRpcClient` in `chain_rpc/evm_rpc.rs`):
- `type Error = MmError<EvmRpcError>`
- `type Address = ethereum_types::Address`
- `type Balance = U256`
- Owns `web3_instances` and `RpcLoopSpawner`

**File structure**:
- Trait definitions: `mm2src/coins/eth/chain_rpc.rs`
- TRON impl: `mm2src/coins/eth/chain_rpc/tron_rpc.rs`
- EVM impl: `mm2src/coins/eth/chain_rpc/evm_rpc.rs`
- Broadcast traits: `mm2src/coins/eth/chain_rpc/broadcast.rs`

**Enum dispatch** (`ChainRpcClient`):
- Uses explicit `match` dispatch (not `Deref<Target = dyn ChainRpcOps>`)
- Converts backend-specific errors to unified `ChainRpcError` at enum boundary
- Located in `chain_rpc.rs`

### 4.4 `ChainActivationRules`

Purpose: keep swap-contract validation and feature gates chain- and wallet-only aware.

```rust
pub trait ChainActivationRules {
    fn validate_platform_request(
        &self,
        ctx: &MmArc,
        conf: &serde_json::Value,
        req: &EthActivationV2Request,
        is_wallet_only: bool,
    ) -> Result<(), EthActivationV2Error>;
}
```

This avoids scattering "TRON rejects swap contracts" checks across activation.

### 4.5 `ChainRpcClient` enum (centralizes RPC dispatch)

> **Related**: See `docs/plans/chain-rpc-client-refactor.md` for the complete design including transaction abstraction and broadcast sessions.

This is the key step to eliminate `EthCoin::is_tron()` as a boundary for RPC operations.

**Key decisions**:
- Use explicit `match` dispatch (not `Deref<Target = dyn ChainRpcOps>`)
- Each backend implements `ChainRpcOps` and `ChainTxOps` traits with its own associated types
- Dispatch enums for unified API: `ChainAddress`, `ChainSignedTx`, `ChainBroadcastSession`, `ChainTxContext`
- Enum converts backend errors to unified `ChainRpcError` at boundary
- Modern `#[async_trait]` (not legacy `futures01::Future`)

```rust
/// Location: mm2src/coins/eth/chain_rpc.rs

/// Dispatch enums for unified API
pub enum ChainAddress { Evm(Address), Tron(TronAddress) }
pub enum ChainSignedTx { Evm(Bytes), Tron(SignedTronTx) }
pub enum ChainBroadcastSession { Evm(EvmBroadcastSession), Tron(TronBroadcastSession) }
pub enum ChainTxContext { Evm(EvmTxContext), Tron(TronTxContext) }

/// Unified error for enum dispatch boundary.
#[derive(Clone, Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum ChainRpcError {
    #[display(fmt = "EVM RPC error: {}", _0)]
    Evm(EvmRpcError),
    #[display(fmt = "TRON RPC error: {}", _0)]
    Tron(TronRpcError),
    #[display(fmt = "Wrong chain: expected {}, got {}", expected, got)]
    WrongChain { expected: &'static str, got: &'static str },
    #[display(fmt = "Not implemented: {}", _0)]
    NotImplemented(String),
}

pub type ChainRpcResult<T> = Result<T, MmError<ChainRpcError>>;

/// Runtime dispatch enum for chain RPC operations.
#[derive(Clone, Debug)]
pub enum ChainRpcClient {
    Evm(EvmRpcClient),
    Tron(TronRpcClient),
}

impl ChainRpcClient {
    pub fn is_tron(&self) -> bool { matches!(self, ChainRpcClient::Tron(_)) }
    pub fn is_evm(&self) -> bool { matches!(self, ChainRpcClient::Evm(_)) }
}

/// ChainRpcClient implements both ChainRpcOps and ChainTxOps with:
/// - type Address = ChainAddress
/// - type BroadcastSession = ChainBroadcastSession
/// - type TxId = H256 (both chains use 32-byte hashes)
```

**Why explicit match over Deref**:
- Async through `Deref<Target = dyn Trait>` is awkward
- Explicit dispatch is clearer and allows converting errors at boundary
- Better async ergonomics with `#[async_trait]`
- Each backend keeps its native error type until enum boundary

**Note**: Call sites use `coin.rpc_client.is_tron()` for type checks, or access `as_tron()` for TRON-specific operations.

---

## 5) Activation Rules: Wallet‑Only + TRON

This section is the "contract" for how `enable_eth_with_tokens` works for TRX.

### 5.1 Wallet-only semantics (activation-level)

**Definition** (for activation validation):
- If `wallet_only` is `true` in coin config, the activation must not require swap contracts and must not require swap-v2 contracts.

This should apply generally, not just to TRX, but TRX depends on it.

### 5.2 TRX semantics (until swaps exist)

For `ChainSpec::Tron`:
- **Swap contracts are not supported**:
  - `swap_contract_address` must be zero address (`0x000…000`), OR (if request schema evolves later) absent.
  - `swap_v2_contracts` must be `None`.
  - `fallback_swap_contract` must be `None`.
  - `contract_supports_watchers` must be `false` (or ignored).
- **Tokens are not supported**:
  - `erc20_tokens_requests` must be empty.
  - `nft_req` must be `None`.
- **RPC mode restrictions**:
  - disallow MetaMask mode for TRON (already implied by `chain_id()` being `None`)
  - WalletConnect: either disallow entirely for TRX in the wallet-only phase, or require a future TRON WC path (out of scope). Prefer explicit "unsupported" error.

### 5.3 How this works with the existing request schema

`EthActivationV2Request` contains `swap_contract_address: Address` (non-optional). Since we must reuse the route, the TRX client must send:
- `swap_contract_address` = `0x0000000000000000000000000000000000000000`
- no `swap_v2_contracts`
- no `fallback_swap_contract`

The activation code must treat that as valid for TRX + wallet-only.

---

## 6) TRON RPC Backend Design (remove stubs)

### 6.1 Why HTTP JSON (not JSON-RPC, not gRPC)

- TRON's public APIs (TronGrid, TronWeb patterns) are HTTP POST JSON endpoints.
- gRPC is awkward in WASM; this codebase already has strong HTTP infrastructure (`mm2_net::transport`, proxy signing, timeouts).
- **Validated via DeepWiki**: TronWeb (official JS SDK) uses HTTP exclusively, not gRPC.

### 6.2 Minimal endpoints needed (HD + balances)

**Validated from `tronprotocol/java-tron` and `tronprotocol/tronweb` via DeepWiki:**

| Operation | FullNode Endpoint | SolidityNode Endpoint | Request | Response Field |
|-----------|-------------------|----------------------|---------|----------------|
| Current block | `POST /wallet/getnowblock` | `POST /walletsolidity/getnowblock` | `{}` | `block_header.raw_data.number` |
| Account info | `POST /wallet/getaccount` | `POST /walletsolidity/getaccount` | `{"address": "T...", "visible": true}` | `balance`, `create_time`, `owner_permission` |

**Endpoint choice**:
- `/wallet/*` (FullNode): Latest blocks, potentially unconfirmed. Use for HD scanning and "latest" balance.
- `/walletsolidity/*` (SolidityNode): Confirmed blocks only (70% witness consensus). Use if confirmed data is required.

**Recommendation**: Start with `/wallet/*` for HD activation. Add policy hook for `/walletsolidity/*` later if needed.

### 6.3 "Address used" / activation detection

For HD gap-limit scanning, TRON needs a cheap "is this address used?" check.

**Validated predicate** based on `tronprotocol/java-tron` behavior:

```rust
/// Treat account as "used" if ANY of these is true.
/// From java-tron: if getaccount returns no "balance" key, account doesn't exist.
pub fn exists_meaningfully(&self) -> bool {
    self.address.is_some()              // Some nodes only echo for existing accounts
        || self.create_time.is_some()   // Account was created on-chain
        || self.owner_permission.is_some() // Has permission structure
        || self.balance.unwrap_or(0) > 0   // Has positive balance
}
```

**Note**: TronWeb considers an empty object from `getAccount()` as "account doesn't exist".

### 6.4 Node pool behavior (parity with EVM `try_rpc_send`)

- Keep a vector of nodes (endpoints) and rotate on success, same as EVM web3 rotation.
- Per-node timeout: align with existing `TRY_RPC_NODE_TIMEOUT_S` (10s) patterns.
- Error model:
  - transport timeout / non-200 / parse errors
  - "all nodes unreachable" as terminal error

### 6.5 Proxy / API-key handling

TRON endpoints may require API keys (TronGrid). The existing request schema provides `komodo_proxy: bool` per node.

TRON backend must support the same "komodo proxy signing" approach as EVM transports:
- Use `mm2_net::transport` infrastructure (not raw `reqwest`) for WASM compatibility
- Reuse `proxy_signature` crate's `RawMessage::sign` for signed headers
- Reference: `mm2src/coins/eth/web3_transport/http_transport.rs` for EVM pattern

### 6.7 WASM vs Native implementations (CRITICAL)

**The TRON RPC client MUST have separate implementations for native and WASM**, following the existing `HttpTransport` pattern:

```rust
// Native implementation
#[cfg(not(target_arch = "wasm32"))]
async fn send_request(...) -> Result<...> {
    use mm2_net::transport::slurp_req;
    // ... native HTTP request
}

// WASM implementation
#[cfg(target_arch = "wasm32")]
async fn send_request(...) -> Result<...> {
    use mm2_net::wasm::http::FetchRequest;
    // ... WASM fetch API
}
```

Reference: `mm2src/coins/eth/web3_transport/http_transport.rs:100-245`

### 6.6 TronGrid error handling

**Important**: TronGrid may return HTTP 200 with error payload. Handle this case:

```rust
// TronGrid can return 200 with {"Error": "..."} or {"code": ..., "message": ...}
// Check for error fields before treating response as success
if response.get("Error").is_some() || response.get("code").is_some() {
    return Err(TronRpcError::ApiError(...));
}
```

---

## 7) Address Formatting Plan (HD + non-HD)

### 7.1 Extend `TronAddress` conversions

Location: `mm2src/coins/eth/tron/address.rs`

Add one missing inverse conversion:
- `TronAddress::to_eth_address() -> ethereum_types::Address` (drop `0x41` prefix)

This enables `ChainAddressCodec::parse_address()` for TRX.

### 7.2 Chain-aware HD address wrapper (no global trait changes)

Location: `mm2src/coins/eth/eth_hd_wallet.rs`

**Problem**: `DisplayAddress` is implemented for `ethereum_types::Address` as EVM checksum. We cannot change it globally.

**Solution**: introduce a wrapper type used only by ETH/TRX HD wallet flows:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChainDisplayAddress {
    raw: ethereum_types::Address,
    kind: ChainKind,
}
```

- Implements `DisplayAddress` by delegating to the chain backend's `ChainAddressCodec`.
- Provides `raw()` accessor so RPC/balance code still uses raw 20-byte address.

**Update type aliases**:
- `pub type EthHDAddress = HDAddress<ChainDisplayAddress, Public>;`
- `pub type EthHDAccount = HDAccount<EthHDAddress, Secp256k1ExtendedPublicKey>;`

Update `HDWalletCoinOps::address_from_extended_pubkey()` to wrap the derived raw address with the correct `ChainKind` derived from `coin.chain_backend.kind()`.

### 7.3 Non-HD `my_address()` formatting

Location: `mm2src/coins/eth.rs` (`MarketCoinOps` impl)

Replace "always checksum" behavior with backend formatting:
- `DerivationMethod::SingleAddress(raw)` → `coin.chain_backend.address_codec().format_address(raw)`

Do not add more `if coin.is_tron()` checks; this should be one of the first call sites moved to backend.

### 7.4 `address_from_str()` parsing

Location: `mm2src/coins/eth.rs` (currently `valid_addr_from_str`)

Change to chain-aware parsing:
- For EVM: accept hex with/without `0x` and checksum variations.
- For TRX: accept Base58 (`T...`) and `41...` hex.

This is required for withdraw/transfer APIs later, but also prevents "TRX coin accepts only 0x…" inconsistencies.

### 7.5 Pubkey → address formatting helper

Location: `mm2src/coins/lp_coins.rs`, function `address_by_coin_conf_and_pubkey_str`

Currently errors for TRX. For wallet-only TRX we might not hit this often, but it's a consistency gap.

**Plan**:
- Derive raw 20-byte address using the same algorithm already used for ETH (`eth::addr_from_pubkey_str`).
- If protocol is TRX:
  - format using TRON codec (`T...`)
- Otherwise keep existing behavior.

---

## 8) HD Wallet Scanning & Balance Computation

### 8.1 Replace EVM-centric `is_address_used` logic for TRX

Location: `mm2src/coins/eth/eth_hd_wallet.rs` (`impl HDAddressBalanceScanner for EthCoin`)

Current EVM logic calls:
- `transaction_count`
- `address_balance`
- `get_tokens_balance_list_for_address`

This must not run on TRX.

**Plan**:
- Replace scanner implementation to call backend:
  - `coin.chain_backend.rpc().is_address_used(raw_addr)`

This is the cleanest early win that removes chain branching from HD scanning.

### 8.2 Known-address balance for TRX

Location: `mm2src/coins/eth/eth_hd_wallet.rs` (`known_address_balance`, `known_addresses_balances`)

For TRX:
- Return `CoinBalanceMap` with only the platform coin ticker ("TRX") populated.
- Do not call token balance routines.

---

## 9) Activation Construction Changes (where backends are instantiated)

### 9.1 `eth_coin_from_conf_and_request_v2()` becomes backend-builder

Location: `mm2src/coins/eth/v2_activation.rs`

Today this function:
- validates swap contracts unconditionally,
- builds `web3_instances`,
- sets decimals based on `ChainSpec`,
- constructs `EthCoinImpl` with EVM assumptions.

**Plan changes**:

1. **Compute `is_wallet_only` from config**:
   - use `is_wallet_only_conf(conf)` semantics already in `mm2src/coins/lp_coins.rs`.

2. **Instantiate a `ChainBackend` based on `chain_spec`**:
   - `EvmBackend` uses existing web3 transport builder.
   - `TronBackend` builds a TRON RPC pool from `req.nodes`.

3. **Run validation via backend rules**:
   - `chain_backend.rules().validate_platform_request(...)`

4. **Store backend in `EthCoinImpl`**:
   - add field: `chain_backend: ChainBackend`

5. **Stop exposing/using `EthCoin::is_tron()`**:
   - keep `ChainSpec` stored for informational/serialization if needed,
   - but prefer `chain_backend.kind()` for runtime decisions.

**Incremental compromise**: in the first iteration, it's acceptable to keep `web3_instances` in `EthCoinImpl` and add `tron_rpc` as an optional field, as long as we immediately funnel calls through `chain_backend` and do not add new `is_tron()` call sites. The backend can be a thin adapter over existing fields until the follow-up refactor moves them.

---

## 10) Deprecation Plan for `EthCoin::is_tron()`

### 10.1 Immediate rule

- **Do not introduce any new `coin.is_tron()` usage.**
- Any new TRX behavior must be wired through the backend.

### 10.2 Migration targets (high ROI)

Update these first:
- `mm2src/coins/eth.rs`: `current_block()` and `my_address()`
- `mm2src/coins/eth/eth_rpc.rs`: `balance()`
- `mm2src/coins/eth/eth_hd_wallet.rs`: `HDAddressBalanceScanner::is_address_used`

Once those are backend-driven, `is_tron()` becomes unused and can be removed.

---

## 11) TRX Coin Config Shape (proposed)

`release/coins` excerpt didn't show a native TRX entry, but the runtime protocol enum supports it. Proposed config shape:

```json
{
  "coin": "TRX",
  "name": "tron",
  "fname": "TRON",
  "mm2": 1,
  "wallet_only": true,
  "decimals": 6,
  "avg_blocktime": 3,
  "required_confirmations": 1,
  "protocol": {
    "type": "TRX",
    "protocol_data": {
      "network": "Mainnet"
    }
  }
}
```

**Notes**:
- `wallet_only: true` is required for the swap-contract relaxation rule.
- Nodes are supplied by the enable request (`params.platform_request.nodes`) because we reuse `enable_eth_with_tokens`.

---

## 12) Testing Parity Plan (mirror ETH patterns)

Even though this is primarily architectural, test parity shapes the public API contract, so it belongs here.

### 12.1 Mirror existing ETH activation helpers

Existing ETH patterns:
- request/response helper: `enable_eth_coin_with_tokens_v2(...)` in `mm2src/mm2_test_helpers/src/for_tests.rs`
- task helper: `task_enable_eth_with_tokens(...)` in the same file
- response parsing uses `EthWithTokensActivationResult` and `InitEthWithTokensStatus` in `mm2src/mm2_test_helpers/src/structs.rs`

**Plan for TRX parity**:
- Add TRX-specific wrappers that call the same RPC methods but with:
  - `ticker: "TRX"`
  - `swap_contract_address: 0x000...000`
  - `erc20_tokens_requests: []`
  - `nodes: [{ "url": "<tron endpoint>" }]`

**Assertions to mirror ETH checks** (adapted to TRX):
- Activation succeeds through `enable_eth_with_tokens` and `task::enable_eth::*`
- Returned addresses for TRX start with `T` (Base58)
- `current_block` is non-stub (monotonic, > 0)
- Balances use 6-decimal interpretation (SUN → TRX in UI-facing values)

### 12.2 Unit tests for TRON codec + RPC client

- `tron/address.rs`: roundtrip conversions (eth raw ↔ tron base58/hex)
- `chain/tron/api.rs`: mock HTTP responses (rotation, timeouts, parsing)

(Exact test harness choice can mirror existing network-mocking conventions in the repo.)

---

## 13) MVP‑First Rollout Summary (implementation order)

### MVP Milestone (ship first): **TRX wallet‑only activation + HD correctness**
This milestone is intentionally scoped to:
- TRX activation works through **existing ETH activation routes**
- TRX addresses are **correct (`T...`)**
- TRX `current_block` + `balance` + HD "is used" checks are **real** (no stubs)
- Implementation stays **refactor-friendly** (no `rpc_client.is_some()` traps; no new `is_tron()` usage)

> **Not in MVP:** moving `web3_instances`, adding `EvmRpcClient`, adding broadcast sessions, finality policies, or any tx pipeline work.
> Those remain in `chain-rpc-client-refactor.md` as post‑MVP steps.

#### MVP Step 0 — Guardrail cleanup (must be done early)
- Replace `rpc_client.is_some()` chain detection patterns with explicit enum/chain matching in:
  - `mm2src/coins/eth.rs`
  - `mm2src/coins/eth/eth_rpc.rs`
  - `mm2src/coins/eth/eth_hd_wallet.rs`
- This is a **small, mechanical** change but critical to prevent future EVM breakage when `ChainRpcClient::Evm` is added.

#### MVP Step 1 — TRON activation gating (wallet-only semantics)
- Enforce TRON wallet-only activation rules:
  - swap contracts not required
  - reject token/NFT requests for TRON
  - disallow MetaMask/WC modes for TRON (until explicitly supported)
- Expected touch points:
  - `mm2src/coins/eth/v2_activation.rs`
  - `mm2src/coins_activation/src/eth_with_token_activation.rs`

#### MVP Step 2 — Remove TRX stubs with TRON HTTP RPC
- Ensure TRON HTTP RPC client exists and is used:
  - `current_block` uses `/wallet/getnowblock`
  - `balance_native` uses `/wallet/getaccount`
  - HD "is used" uses account existence/metadata predicate
- Expected touch points:
  - `mm2src/coins/eth/tron/api.rs`
  - `mm2src/coins/eth/chain_rpc/tron_rpc.rs`
  - `mm2src/coins/eth.rs` (`current_block`)
  - `mm2src/coins/eth/eth_rpc.rs` (`balance`)

#### MVP Step 3 — Address correctness (TRX = `T...`) in HD + non-HD paths
- Implement chain-aware formatting without global trait changes:
  - Use a wrapper (e.g. `ChainDisplayAddress`) for HD flows
  - Use chain-aware formatting for `my_address()`
- Expected touch points:
  - `mm2src/coins/eth/eth_hd_wallet.rs`
  - `mm2src/coins/eth.rs`
  - `mm2src/coins/eth/tron/address.rs`
  - (optional but recommended) `mm2src/coins/lp_coins.rs` pubkey→address helper for TRX

---

### Post‑MVP (do not block MVP): ChainRpcClient Refactor + EVM backend extraction
After MVP ships, proceed with the `ChainRpcClient` refactor plan (see `docs/plans/chain-rpc-client-refactor.md`), including:
- Move `web3_instances` into `EvmRpcClient`
- Add chain-agnostic dispatch enums (`ChainAddress`, etc.)
- Add transaction pipeline abstractions (`BroadcastSessionOps`, `ChainTxOps`)
- Add broadcast reliability policies + receipt/finality waiting

This work is explicitly separated so MVP can land quickly without introducing cross‑cutting refactors.

---

## 14) Codebase-Specific Implementation Details

This section documents specific types, files, and patterns to use (validated via codebase exploration).

### 14.1 HTTP Transport Infrastructure

**Use `mm2_net::transport` (not raw `reqwest`)** for WASM compatibility.

| Component | Location | Usage |
|-----------|----------|-------|
| `SlurpResult` | `mm2src/mm2_net/src/transport.rs` | Return type for HTTP requests |
| `SlurpError` | `mm2src/mm2_net/src/transport.rs` | Error enum (Transport, Timeout, etc.) |
| `slurp_req` | `mm2src/mm2_net/src/transport.rs` | Main HTTP request function (native) |
| `SlurpHttpClient` | `mm2src/mm2_net/src/native_http.rs` | Trait for HTTP methods |

**Pattern from EVM's HttpTransport** (`mm2src/coins/eth/web3_transport/http_transport.rs`):
```rust
use mm2_net::transport::slurp_req;

let mut req = http::Request::new(body_bytes);
*req.method_mut() = http::Method::POST;
*req.uri_mut() = node_uri.clone();
req.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static(APPLICATION_JSON));

let (status, _headers, body) = slurp_req(req).await?;
```

### 14.2 Proxy Signing

**File**: `mm2src/proxy_signature/src/lib.rs`

```rust
use proxy_signature::RawMessage;
use common::{X_AUTH_PAYLOAD, PROXY_REQUEST_EXPIRATION_SEC};
use mm2_p2p::Keypair;

// Sign request for komodo proxy
let proxy_sign = RawMessage::sign(
    &keypair,
    &node_uri,
    body_bytes.len(),
    PROXY_REQUEST_EXPIRATION_SEC,
)?;
let proxy_sign_json = serde_json::to_string(&proxy_sign)?;
req.headers_mut().insert(X_AUTH_PAYLOAD, proxy_sign_json.parse().unwrap());
```

### 14.3 Error Type Pattern

**Mirror `Web3RpcError`** (`mm2src/coins/eth.rs:618-705`):

```rust
#[derive(Debug, Display, EnumFromStringify, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum TronRpcError {
    #[display(fmt = "Transport: {_0}")]
    Transport(String),
    #[display(fmt = "Invalid response: {_0}")]
    InvalidResponse(String),
    #[display(fmt = "Timeout: {_0}")]
    Timeout(String),
    #[display(fmt = "Internal: {_0}")]
    Internal(String),
    #[display(fmt = "All nodes unreachable")]
    AllNodesUnreachable,
    #[display(fmt = "API error: {_0}")]
    ApiError(String),  // For TronGrid 200-with-error responses
}
```

### 14.4 HD Scanner Signature

**File**: `mm2src/coins/coin_balance.rs:408-417`

```rust
#[async_trait]
pub trait HDAddressBalanceScanner {
    type Address: Send + Sync;

    /// Checks if the given `address` has been used before.
    async fn is_address_used(&self, address: &Self::Address) -> BalanceResult<bool>;
}
```

**EVM implementation** (`mm2src/coins/eth/eth_hd_wallet.rs:68-94`) checks:
1. Transaction count > 0
2. Platform balance != 0
3. Any token balance != 0

**TRON implementation** should route through `ChainBackend::is_address_used()`.

### 14.5 Address Display Integration

**Current EVM** (`mm2src/coins/eth/eth_hd_wallet.rs:15-21`):
```rust
impl DisplayAddress for Address {
    fn display_address(&self) -> String {
        checksum_address(&self.addr_to_string())  // Returns 0x...
    }
}
```

**TRON already has** (`mm2src/coins/eth/tron/address.rs:74-77`):
```rust
impl Address {
    pub fn to_base58(&self) -> String {
        bs58::encode(self.inner).with_check().into_string()  // Returns T...
    }
}
```

**Activation response** will automatically use correct format because `CoinAddressInfo` calls `display_address()`.

---

## 15) Validation Summary

This plan was validated through:

### DeepWiki Queries (TRON Official Repositories)

| Repository | Questions Asked | Key Findings |
|------------|-----------------|--------------|
| `tronprotocol/java-tron` | HTTP endpoints, account activation detection | `/wallet/*` vs `/walletsolidity/*` difference documented; account existence = has "balance" field |
| `tronprotocol/tronweb` | Balance queries, address formats | Uses HTTP exclusively (validates HTTP choice); converts Base58↔hex internally |

### RepoPrompt Context Building

- Built context for: `ChainSpec`, `v2_activation.rs`, `eth_hd_wallet.rs`, TRON stubs, RPC dispatcher
- Compared original plan vs v2 plan
- Chat session created: `tron-hd-activation-plan--2CD73B`

### Key Validated Decisions

| Decision | Validation Source |
|----------|-------------------|
| Use HTTP JSON (not gRPC) | TronWeb uses HTTP exclusively (DeepWiki) |
| `exists_meaningfully()` predicate | java-tron behavior documented (DeepWiki) |
| Enum-backed `ChainBackend` | RepoPrompt review: better than scattered trait objects |
| Delay directory restructure | RepoPrompt review: minimize blast radius |
| Use `mm2_net::transport` | Codebase exploration: WASM compatibility |
| Mirror `Web3RpcError` pattern | Codebase exploration: consistent error handling |

### Additional Validation (Codebase Investigation)

**Q1: WASM Compatibility for TRON RPC Client**
- `HttpTransport` has SEPARATE implementations: native uses `mm2_net::transport::slurp_req`, WASM uses `mm2_net::wasm::http::FetchRequest`
- **TRON RPC client MUST have both native and WASM implementations**, following the same `#[cfg(target_arch = "wasm32")]` / `#[cfg(not(target_arch = "wasm32"))]` pattern
- Reference: `mm2src/coins/eth/web3_transport/http_transport.rs:100-245`

**Q2: Backwards Compatibility (EthCoinImpl Serialization)**
- `EthCoinImpl` is **NOT serialized** - it contains `Arc<>`, `Mutex<>`, `AsyncMutex<>` which aren't `Serialize/Deserialize`
- Coins are created fresh on each activation, not persisted to storage
- **Adding `chain_backend: ChainBackend` has NO backwards compatibility concerns**

**Q3: Phasing Order**
- Current order (validation → RPC → addresses → backend) is **correct** for incremental delivery
- Phase 1 unblocks wallet-only activation immediately
- Each phase is independently testable and shippable
- Creating `ChainBackend` first would delay the first shippable milestone

**Q4: Test Strategy**
- Unit tests with HTTP mocking for RPC client (no network dependency)
- Integration tests against Nile/Shasta can be added in Phase 2
- Mirror existing `docker_tests` patterns for network tests - not precisely but look at unit and integration tests for eth first then look at this

---

## 16) Follow-ups (explicitly deferred)

- TRC20 support (`TriggerSmartContract`, TRC20 balance, token activation)
- TRON signing / withdraw
- Swap support and contract validation for TRON

---

## 17) Tech Debt: HTTP Transport Extraction

### 17.1 Problem

`TronHttpClient::send_request()` in `mm2src/coins/eth/tron/api.rs` duplicates patterns from EVM's `HttpTransport` in `mm2src/coins/eth/web3_transport/http_transport.rs`:

| Component | TRON Lines | EVM Lines | Duplication |
|-----------|------------|-----------|-------------|
| Native proxy signing | 96-113 | 120-134 | Identical pattern |
| Native timeout wrapper | 115-132 | 136-157 | Same `Timer::sleep` + `select` |
| WASM proxy signing | 150-163 | 198-213 | Identical pattern |
| FetchRequest builder | 143-148 | 248-254 | Same CORS + JSON headers |

### 17.2 What Should Be Extracted

**Generic utilities** (not TRON-specific):
- URI join helper (`base.trim_end_matches('/') + "/" + path.trim_start_matches('/')`)
- Proxy signing helper (shared pattern for native + WASM)
- Timeout wrapper pattern (native only)

**Keep in `rpc.rs`** (TRON-specific):
- TronGrid "200 OK but error payload" guard (lines 63-77)
- TRON endpoint methods (`get_now_block_number`, `get_account`, etc.)
- `TronRpcPool` node rotation logic

### 17.3 Recommended Extraction (Phase 4 or later)

**Option A: Minimal (coins-local)**

Create `mm2src/coins/eth/tron/http_transport.rs` with:
- `fn join_uri(base: &str, path: &str) -> String`
- `fn sign_proxy_request(keypair: &Keypair, uri: &Uri, body_len: usize) -> Result<String, String>`

**Option B: Broader (cross-crate, future)**

Create `mm2src/coins/http/signed_json_transport.rs` with a generic `SignedJsonHttpTransport` that:
- Handles POST JSON requests (native + WASM)
- Integrates proxy signing
- Provides timeout wrapper
- Returns raw bytes (callers do their own response parsing)

Could be reused by: TRON RPC, Tendermint HTTP clients, potentially EVM (as inner layer).

### 17.4 Phase Mapping

This is **Phase 4 prep work** or a separate tech debt task. Current duplication is acceptable because:
1. TRON's transport is simpler (no JSON-RPC framing, no event handlers)
2. EVM has extra responsibilities (request ID, event handlers)
3. Both work correctly today

### 17.5 Testing: Use `cross_test!` for Pure Logic Tests

Pure logic tests (URI join, error parsing) should use `cross_test!` macro to run on both native and WASM:

```rust
// In mm2src/coins/eth/tron/api.rs tests module
cross_test!(test_uri_join_avoids_double_slash, {
    let base = "https://api.trongrid.io/";
    let path = "/wallet/getaccount";
    // ... assertions
});
```

**Network integration tests** should remain native-only with `#[ignore]`:
- Rate limiting on public nodes
- CORS restrictions in browser
- No env var gating needed - `#[ignore]` is sufficient

---

## 18) Architecture Decision: Why EthCoin Cannot Be Generic

### The Ideal Design

Ideally, `EthCoin<B: ChainRpcOps>` would allow compile-time static dispatch:
```rust
struct EthCoin<B: ChainRpcOps> {
    rpc_client: B,
    // ...
}
```

This would give: compile-time enforcement, no runtime dispatch overhead, type-safe backend access.

### Why This Is Currently Not Feasible

**Problem: `MmCoinEnum` stores concrete types**

`MmCoinEnum` in `mm2src/coins/lp_coins.rs` is a non-generic enum:
```rust
pub enum MmCoinEnum {
    EthCoinVariant(EthCoin),  // Concrete, not generic
    // ...
}
```

If `EthCoin` became `EthCoin<B>`, we would need to:
1. Split the enum variant into `EthEvmVariant(EthCoin<EvmRpcClient>)` and `EthTronVariant(EthCoin<TronRpcClient>)`
2. Update 50+ match sites that pattern match on `EthCoinVariant`
3. Update all `From<EthCoin>` implementations
4. Propagate generics through token activation

### Current Approach: Runtime Enum Dispatch

For Phase 2, we use `ChainRpcClient` enum with explicit match:
- Trait with associated types defines the interface
- Each backend implements trait with its own types
- Enum provides runtime dispatch with error conversion at boundary
- Can be refactored later when `MmCoinEnum` is restructured

### Future: MmCoinEnum Refactor

When `MmCoinEnum` is refactored (has TODO about `enum_dispatch`), consider:
1. Split `EthCoinVariant` into `EvmCoinVariant` and `TronCoinVariant`
2. Make both implement common traits
3. Generic `EthCoin<B>` becomes viable

This is documented here so the constraint is known during future refactoring.
