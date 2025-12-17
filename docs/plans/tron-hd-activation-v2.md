# TRX (TRON) HD + Activation via `EthCoin` — Architecture / Refactor Plan

> **Document scope**: design + implementation plan only (no code in this doc).
>
> **Hard constraint**: TRX must reuse the existing RPC routes:
> - request/response: `enable_eth_with_tokens`
> - task-based: `task::enable_eth::{init,status,user_action,cancel}`
>
> **No new dispatcher methods** (do not add `enable_trx...` routes). TRX flows through the ETH platform activation pipeline.

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
│   └── rpc.rs          # NEW: TRON HTTP RPC client
└── eth_rpc.rs (existing; EVM-only)
```

**Key decision (validated by RepoPrompt review)**: **Delay directory restructure**.
- Do NOT create `chain/evm.rs` or move files in the first iteration.
- Add new modules (`chain_backend.rs`, `tron/rpc.rs`) alongside existing files.
- Keep `eth_rpc.rs` where it is - `EvmBackend` will wrap it, not replace it.
- This minimizes blast radius and keeps EVM paths unchanged.

**Incremental migration path**:
1. Add `chain_backend.rs` with `ChainBackend` enum
2. Add `tron/rpc.rs` for TRON HTTP client
3. Wire `EthCoinImpl` to use `ChainBackend`
4. Later (out of scope): consider moving to `chain/` directory structure

---

## 4) Core Abstractions (the "backend layer")

### 4.1 `ChainKind` (small, copyable)
Location: `mm2src/coins/eth/chain/mod.rs` (new)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainKind { Evm, Tron }
```

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

Purpose: remove TRX stubs and support HD gap-limit scanning.

```rust
#[async_trait]
pub trait ChainRpcOps: Send + Sync {
    async fn current_block(&self) -> Result<u64, String>;

    /// Balance in smallest unit: wei for EVM, SUN for TRON.
    async fn balance(&self, addr: ethereum_types::Address) -> Result<ethereum_types::U256, String>;

    /// "Used address" predicate for HD gap limit scanning.
    async fn is_address_used(&self, addr: ethereum_types::Address) -> Result<bool, String>;
}
```

- EVM implementation (existing behavior):
  - `is_address_used` = `tx_count > 0 OR balance != 0 OR any-token-balance != 0`
- TRON implementation:
  - `is_address_used` = `getaccount.exists_meaningfully()` (see §6).

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

### 4.5 `ChainBackend` enum (centralizes branching)

This is the key step to eliminate `EthCoin::is_tron()` as a boundary.

**Key decision (validated by RepoPrompt review)**: **Use enum first, not trait objects**.

```rust
/// Location: mm2src/coins/eth/chain_backend.rs
pub enum ChainBackend {
    Evm(EvmBackend),
    Tron(TronBackend),
}

impl ChainBackend {
    pub fn kind(&self) -> ChainKind {
        match self {
            Self::Evm(_) => ChainKind::Evm,
            Self::Tron(_) => ChainKind::Tron,
        }
    }

    pub fn format_address(&self, raw: &ethereum_types::Address) -> String {
        match self {
            Self::Evm(b) => b.format_address(raw),
            Self::Tron(b) => b.format_address(raw),
        }
    }

    pub async fn current_block(&self) -> Result<u64, String> {
        match self {
            Self::Evm(b) => b.current_block().await,
            Self::Tron(b) => b.current_block().await,
        }
    }

    pub async fn balance(&self, addr: &ethereum_types::Address) -> Result<U256, String> {
        match self {
            Self::Evm(b) => b.balance(addr).await,
            Self::Tron(b) => b.balance(addr).await,
        }
    }

    pub async fn is_address_used(&self, addr: &ethereum_types::Address) -> Result<bool, String> {
        match self {
            Self::Evm(b) => b.is_address_used(addr).await,
            Self::Tron(b) => b.is_address_used(addr).await,
        }
    }
}
```

**Why enum over trait objects**:
- Avoids object-safety constraints
- Single choke point for all chain branching
- Simpler lifetime management
- Match exhaustiveness catches missing implementations at compile time
- Can add trait objects later if extensibility is needed

**Note**: This still uses `match` internally, but only inside `chain_backend.rs`. Call sites just call `coin.chain_backend.balance(...)` without any branching.

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
- `chain/tron/rpc.rs`: mock HTTP responses (rotation, timeouts, parsing)

(Exact test harness choice can mirror existing network-mocking conventions in the repo.)

---

## 13) Phased Rollout Summary (implementation order)

### Phase 1 — Make TRX activation possible (wallet-only)

1. Add chain-aware activation validation:
   - TRX: require zero swap contract; forbid `swap_v2` contracts
   - `wallet_only`: relax swap contract requirements
2. Build TRON backend in `eth_coin_from_conf_and_request_v2()` (don't build web3 for TRX)

### Phase 2 — Remove stubs (HD + balances)

1. Implement TRON HTTP RPC client:
   - `current_block`, `get_account`, `balance`, `is_address_used`
2. Wire `current_block()` and `balance()` through backend (no `is_tron` checks)

### Phase 3 — Fix address formatting everywhere

1. Add `TronAddress::to_eth_address()`
2. Add `ChainDisplayAddress` for HD wallet output
3. Make `my_address()` + `address_from_str()` chain-aware via backend codec
4. Implement TRX branch in `address_by_coin_conf_and_pubkey_str()`

### Phase 4 — Consolidate chain branching into backend

1. Introduce `ChainBackend` enum and traits (`ChainRpcOps`, `ChainAddressCodec`, `ChainActivationRules`)
2. Update remaining call sites to backend
3. Remove `EthCoin::is_tron()`

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
