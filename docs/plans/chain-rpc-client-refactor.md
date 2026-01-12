# ChainRpcClient Refactor: Unified RPC Abstraction for EVM and TRON

> **Document scope**: Architecture and implementation plan for moving `web3_instances` into `EvmRpcClient` and creating a unified RPC abstraction.
>
> **Related document**: `.claude/plans/tron-hd-activation-v2.md` (TRON HD activation plan)

---

## MVP‑First Prioritization (Read This First)

### Sequencing
**TRX activation MVP is the priority**, as defined in `.claude/plans/tron-hd-activation-v2.md` (wallet-only activation via existing ETH routes, correct `T...` addresses, real TRON current block + balance, HD "is used" checks).

This `ChainRpcClient` refactor document remains the plan for **post‑MVP** work:
- moving `web3_instances` into `EvmRpcClient`,
- unifying dispatch (EVM + TRON),
- adding broadcast sessions and receipt/finality handling.

### What MVP MUST NOT do (to avoid refactor pain)
During TRX MVP, avoid pulling in the refactor-heavy parts of this plan:
- Do **not** introduce `ChainTxOps` / `BroadcastSessionOps` / prepare→sign→broadcast abstractions.
- Do **not** move `web3_instances` ownership out of `EthCoinImpl`.
- Do **not** restructure directories or introduce broad "backend layers" that touch both EVM and TRON.

### MVP guardrails that SHOULD be done (small + high leverage)
Even though the refactor is post‑MVP, there is one class of changes that *must* be done during MVP to keep the refactor safe:

1) **Eliminate the `rpc_client.is_some()` trap**
- Current/legacy code patterns that treat `rpc_client.is_some()` as "TRON mode" will **break EVM** the moment
  `ChainRpcClient::Evm` is added.
- MVP should replace these checks with explicit matching:
  - `matches!(rpc_client, Some(ChainRpcClient::Tron(_)))`, or
  - `match chain_spec { ChainSpec::Tron{..} => ..., ChainSpec::Evm{..} => ... }`.

**Expected MVP file touches for guardrails:**
- `mm2src/coins/eth.rs`
- `mm2src/coins/eth/eth_rpc.rs`
- `mm2src/coins/eth/eth_hd_wallet.rs`
- (sometimes) `mm2src/coins/eth/chain_rpc.rs` (to expose/refine enum helpers)

These edits are intentionally **mechanical** and do not require implementing the full refactor.

### Dependencies to keep in mind (MVP vs refactor)
- MVP TRON RPC work relies on existing HTTP/proxy signing infrastructure:
  - `mm2_net` (native + wasm HTTP)
  - `proxy_signature`
- Post‑MVP refactor work will additionally touch:
  - websocket transport + spawner extraction
  - EVM broadcast reliability layers

---

## 0) Goals

1. **Move `web3_instances` ownership** from `EthCoinImpl` into `EvmRpcClient`
2. **Unified RPC abstraction** that works identically for EVM and TRON chains
3. **Decouple websocket transport** from `EthCoin` dependency
4. **Abstract transaction pipeline** (prepare → sign → broadcast) for both chains
5. **Stop cloning `web3_instances`** into token/NFT coins

---

## 1) Architecture: Two-Layer Design

### Layer 1: Chain-Agnostic Traits

#### 1.1 `ChainRpcOps` (Read-Only Operations)

```rust
#[async_trait]
pub trait ChainRpcOps: Send + Sync + Debug {
    type Error;
    type Address;
    type Balance;

    async fn current_block(&self) -> Result<u64, Self::Error>;
    async fn balance_native(&self, address: Self::Address) -> Result<Self::Balance, Self::Error>;
    async fn is_address_used_basic(&self, address: Self::Address) -> Result<bool, Self::Error>;
}
```

#### 1.2 `ChainTxOps` (Transaction Pipeline)

```rust
#[async_trait]
pub trait ChainTxOps: Send + Sync + Debug {
    type Error;
    type Address;
    type SignedTx;
    type TxId;  // = H256 for both chains
    type BroadcastSession: BroadcastSessionOps<
        Error = Self::Error,
        SignedTx = Self::SignedTx,
        TxId = Self::TxId,
    >;

    /// Select best nodes and gather chain-specific signing context.
    async fn prepare_broadcast(&self, from: Self::Address) -> Result<Self::BroadcastSession, Self::Error>;
}
```

#### 1.3 `BroadcastSessionOps` (Consume-On-Use Broadcast)

```rust
#[async_trait]
pub trait BroadcastSessionOps: Send + Sync + Debug {
    type Error;
    type SignedTx;
    type TxId;
    type TxContext: Send + Sync + Debug;

    /// Chain-specific context needed for signing (EVM: nonce, TRON: ref_block).
    fn tx_context(&self) -> &Self::TxContext;

    /// Broadcast signed tx to captured nodes. Consumes self to prevent reuse.
    async fn broadcast(self, signed_tx: Self::SignedTx) -> Result<Self::TxId, Self::Error>;
}
```

### Layer 2: Dispatch Enums (ChainRpcClient)

```rust
pub enum ChainAddress { Evm(Address), Tron(TronAddress) }
pub enum ChainSignedTx { Evm(Bytes), Tron(SignedTronTx) }
pub enum ChainBroadcastSession { Evm(EvmBroadcastSession), Tron(TronBroadcastSession) }
pub enum ChainTxContext { Evm(EvmTxContext), Tron(TronTxContext) }

pub enum ChainRpcClient {
    Evm(EvmRpcClient),
    Tron(TronRpcClient),
}

impl ChainRpcOps for ChainRpcClient { type Address = ChainAddress; ... }
impl ChainTxOps for ChainRpcClient { type Address = ChainAddress; ... }
```

---

## 2) Chain-Specific Implementations

### 2.1 EVM

```rust
pub struct EvmTxContext {
    pub nonce: U256,
}

pub struct EvmBroadcastSession {
    context: EvmTxContext,
    targets: Vec<Arc<Web3<Web3Transport>>>,  // captured at creation
    spawner: RpcLoopSpawner,
}

pub struct EvmRpcClient {
    instances: AsyncMutex<Vec<Web3Instance>>,
    spawner: RpcLoopSpawner,
}
```

**Key behavior:**
- `prepare_broadcast`: Query all nodes for pending nonce → select max → capture nodes that returned max
- `broadcast`: Send raw tx to captured nodes (like current `sign_and_send_transaction_with_keypair`)

### 2.2 TRON

```rust
pub struct TronTxContext {
    pub ref_block_bytes: Vec<u8>,
    pub ref_block_hash: Vec<u8>,
    pub expiration: u64,
}

pub struct TronBroadcastSession {
    context: TronTxContext,
    targets: Vec<TronHttpClient>,  // captured at creation
}

pub struct TronRpcClient {
    inner: Arc<TronApiClient>,
}
```

**Key behavior:**
- `prepare_broadcast`: Select best endpoints (by latest block/health) → capture → get ref_block for signing
- `broadcast`: Send signed tx to captured endpoints

---

## 3) Transaction Flow (Unified)

```rust
// 1. Prepare broadcast (selects best nodes, gets signing context)
let session = coin.rpc_client.prepare_broadcast(from).await?;

// 2. Sign using chain-specific context
let signed = match session.tx_context() {
    ChainTxContext::Evm(ctx) => sign_evm(ctx.nonce, ...),
    ChainTxContext::Tron(ctx) => sign_tron(ctx.ref_block_bytes, ...),
};

// 3. Broadcast to captured nodes (consume-on-use)
let tx_id = session.broadcast(signed).await?;
```

---

## 4) Error Types (mm2 pattern)

```rust
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
```

---

## 5) RpcLoopSpawner (Decouple Websocket from EthCoin)

### Problem

Current websocket transport requires `EthCoin` to spawn connection loops:
```rust
// Current (wrong)
socket.maybe_spawn_connection_loop(self.clone());  // takes EthCoin
```

But it only uses `coin.spawner()` which returns `WeakSpawner`.

### Solution

```rust
// mm2src/coins/eth/web3_transport/spawner.rs
#[derive(Clone)]
pub struct RpcLoopSpawner {
    abortable: AbortableQueue,
}

impl RpcLoopSpawner {
    pub fn new(abortable: AbortableQueue) -> Self { Self { abortable } }
    pub fn spawn(&self, fut: BoxFuture<'static, ()>, settings: AbortSettings) { ... }
}
```

Update websocket transport:
```rust
// New (correct)
pub fn maybe_spawn_connection_loop(&self, spawner: &RpcLoopSpawner);
```

---

## 6) Migration Steps

### Step 1: Add RpcLoopSpawner

**Files:**
- ADD: `mm2src/coins/eth/web3_transport/spawner.rs`
- MODIFY: `mm2src/coins/eth/web3_transport/websocket_transport.rs`

**Changes:**
- Create `RpcLoopSpawner` struct
- Add new websocket methods taking `&RpcLoopSpawner`
- Keep old `EthCoin`-based methods as temporary wrappers

### Step 2: Add EvmRpcClient module

**Files:**
- ADD: `mm2src/coins/eth/chain_rpc/evm_rpc.rs`
- MODIFY: `mm2src/coins/eth/chain_rpc.rs`

**Changes:**
- Implement `EvmRpcClient` with `instances` and `spawner`
- Implement `ChainRpcOps` for `EvmRpcClient`
- Implement `execute()` method (mirrors `try_rpc_send`)

### Step 3: Add BroadcastSession traits and implementations

**Files:**
- ADD: `mm2src/coins/eth/chain_rpc/broadcast.rs`
- MODIFY: `mm2src/coins/eth/chain_rpc/evm_rpc.rs`
- MODIFY: `mm2src/coins/eth/chain_rpc/tron_rpc.rs`

**Changes:**
- Define `BroadcastSessionOps` and `ChainTxOps` traits
- Implement `EvmBroadcastSession` with captured nodes
- Implement `TronBroadcastSession`

### Step 4: Delegate eth_rpc.rs to EvmRpcClient

**Files:**
- MODIFY: `mm2src/coins/eth/eth_rpc.rs`
- MODIFY: `mm2src/coins/eth.rs`

**Changes:**
- Add `evm_rpc: Option<EvmRpcClient>` to `EthCoinImpl` (temporary)
- `try_rpc_send`: delegate to `evm_rpc.execute()` if present

### Step 5: Migrate signing/broadcast to session-based API

**Files:**
- MODIFY: `mm2src/coins/eth.rs`

**Changes:**
- Change `get_addr_nonce` callers to use `prepare_broadcast`
- Update `sign_transaction_with_keypair` to use `EvmTxContext`
- Update `sign_and_send_transaction_with_keypair` to use `session.broadcast()`

### Step 6: Switch v2 activation to build EvmRpcClient

**Files:**
- MODIFY: `mm2src/coins/eth/v2_activation.rs`

**Changes:**
- Replace `build_web3_instances` with `build_evm_rpc_client`
- Set `EthCoinImpl.rpc_client = Some(ChainRpcClient::Evm(...))`

### Step 7: Stop cloning instances in token/NFT initialization

**Files:**
- MODIFY: `mm2src/coins/eth/v2_activation.rs`

**Changes:**
- Remove `web3_instances: AsyncMutex::new(self.web3_instances.lock().await.clone())`
- Clone `rpc_client` instead (Arc-backed, shared)

### Step 8: Update ChainRpcClient dispatch layer

**Files:**
- MODIFY: `mm2src/coins/eth/chain_rpc.rs`

**Changes:**
- Add dispatch enums: `ChainAddress`, `ChainSignedTx`, `ChainBroadcastSession`, `ChainTxContext`
- Remove TRON-specific methods (`balance_native_tron`, `is_address_used_tron`)
- Add chain-agnostic methods that accept `ChainAddress`
- Implement `ChainRpcOps` and `ChainTxOps` for `ChainRpcClient`

### Step 9: Remove web3_instances from EthCoinImpl

**Files:**
- MODIFY: `mm2src/coins/eth.rs`
- MODIFY: `mm2src/coins/eth/eth_rpc.rs`
- MODIFY: `mm2src/coins/eth/for_tests.rs`

**Changes:**
- Remove `web3_instances` field
- Move `Web3Instance` to `evm_rpc.rs` (private)
- Remove fallback code paths

### Step 10: Finalize RpcCommonOps

**Files:**
- MODIFY: `mm2src/coins/eth.rs`
- MODIFY: `mm2src/coins/eth/chain_rpc/evm_rpc.rs`

**Changes:**
- Implement `RpcCommonOps` for `EvmRpcClient` returning `Web3<Web3Transport>`
- Update `EthCoin::web3()` to delegate to `evm_rpc`

---

## 7) Files Summary

### Add
- `mm2src/coins/eth/web3_transport/spawner.rs` - RpcLoopSpawner
- `mm2src/coins/eth/chain_rpc/evm_rpc.rs` - EvmRpcClient, EvmBroadcastSession
- `mm2src/coins/eth/chain_rpc/broadcast.rs` - BroadcastSessionOps, ChainTxOps traits

### Modify
- `mm2src/coins/eth/chain_rpc.rs` - dispatch enums, unified API
- `mm2src/coins/eth/chain_rpc/tron_rpc.rs` - TronBroadcastSession, ChainTxOps impl
- `mm2src/coins/eth/web3_transport/websocket_transport.rs` - spawner-based API
- `mm2src/coins/eth/eth_rpc.rs` - delegate to EvmRpcClient
- `mm2src/coins/eth/v2_activation.rs` - build EvmRpcClient, stop cloning instances
- `mm2src/coins/eth.rs` - remove web3_instances, update signing flow
- `mm2src/coins/eth/for_tests.rs` - update test setup

---

## 8) Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| **BroadcastSession captures nodes** | No stale indices, consume-on-use prevents reuse bugs |
| **Associated types preserved** | Each chain uses native Address type; dispatch uses ChainAddress enum |
| **TxId = H256** | Both chains use 32-byte hashes |
| **RpcLoopSpawner** | Concrete struct, decouples websocket from EthCoin |
| **Errors follow mm2 pattern** | Display + SerializeErrorType, structured variants |
| **Transaction abstraction** | prepare_broadcast → sign → broadcast works for both chains |
| **Shared EvmRpcClient** | Platform coin and tokens share one Arc-backed client |
| **Use existing ChainSpec** | Don't introduce ChainKind; use `ChainSpec` from `eth.rs` |
| **ChainFamily only if needed** | Optional lightweight tag for HD display wrapper (parameter-free) |

---

## 9) Clarifications from Plan Review

### ChainSpec vs ChainKind

**Use existing `ChainSpec`** (already in `mm2src/coins/eth.rs`):
```rust
pub enum ChainSpec {
    Evm { chain_id: u64 },
    Tron { network: tron::Network },
}
```

**Only introduce `ChainFamily` if needed** for HD display wrapper where `chain_id` shouldn't matter:
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChainFamily { Evm, Tron }

impl From<&ChainSpec> for ChainFamily { ... }
```

### ChainBackend vs ChainRpcClient

These are **separate concepts**:

| Layer | Responsibility |
|-------|---------------|
| `ChainRpcClient` | "How to talk to nodes" (RPC dispatch) |
| `ChainBackend` | "How this chain behaves" (facade: spec + rpc + codec + rules) |

**Composition**:
```rust
pub struct ChainBackend {
    pub spec: ChainSpec,        // Use existing ChainSpec
    pub rpc: ChainRpcClient,
    // pub address_codec: ...,
    // pub activation_rules: ...,
}
```

### Current Code Gaps

| Area | Plans Expect | Current Code |
|------|-------------|--------------|
| Traits | `ChainRpcOps` + `ChainTxOps` + `BroadcastSessionOps` | Only `ChainRpcOps` |
| Dispatch | Chain-agnostic with `ChainAddress` | TRON-specific (`balance_native_tron`) |
| Errors | mm2 pattern with `EvmRpcError`/`TronRpcError` | Uses `Web3RpcError`, wrong `From` impl |
| Files | `chain_rpc/evm_rpc.rs` module | Placeholder inline in `chain_rpc.rs` |

---

## 10) Current Code Reality (Updated 2025-12-31)

**TRX MVP is complete.** All critical issues have been resolved:

✅ `TronApiClient` exists with native + WASM HTTP implementations
✅ `TronRpcClient` wraps it and implements `ChainRpcOps`
✅ `v2_activation` builds TRON `rpc_client` and no `web3_instances` for TRON
✅ `ChainRpcClient` uses explicit chain matching (no `is_some()` trap)
✅ Address formatting for TRX returns `T...` (Base58Check) everywhere
✅ HD scanning works with gap limit detection and used-but-zero-balance addresses
✅ 10 integration tests validate all functionality

**Remaining for post-MVP refactor:**

- `ChainRpcClient::Evm` variant (PR-6) — EVM still uses legacy `web3_instances` directly
- Broadcast session abstraction (PR-7) — unified tx pipeline
- Broadcast reliability policies (PR-8-9) — concurrent fanout, receipt finality

---

## 11) The `rpc_client.is_some()` Trap (RESOLVED ✅)

**This trap has been eliminated.** All code now uses explicit chain matching:

| Location | Old Pattern | New Pattern |
|----------|-------------|-------------|
| `eth_rpc.rs::balance()` | `if let Some(ref rpc_client)` | `match self.0.chain_spec { ChainSpec::Tron => ... }` |
| `eth_hd_wallet.rs::is_address_used()` | `if let Some(ref rpc_client)` | Explicit `ChainSpec::Tron` match |
| `eth_hd_wallet.rs::known_address_balance()` | `if rpc_client.is_some()` | Chain-aware balance logic |
| `eth.rs::current_block()` | `if rpc_client present` | `match chain_spec` with explicit variants |

**Safe for future EVM refactor:** When `ChainRpcClient::Evm` is added (PR-6), these call sites will correctly dispatch to EVM logic.

---

## 12) Revised PR Stack

### PR stack scope note (MVP vs Post‑MVP)
This PR stack is primarily **post‑TRX‑MVP** work.

However, **PR‑1 is an MVP guardrail** (eliminating the `rpc_client.is_some()` trap) and should be done *during* the TRX activation MVP because it prevents future EVM breakage and reduces refactor risk.

Similarly, **PR‑2 (TRON activation gating)** and **PR‑3 (TRON address correctness)** are MVP‑aligned deliverables, even though they live in the same technical area.

PR‑5 through PR‑9 remain **strictly post‑MVP** and should not be started until the TRX MVP milestone is complete.

### PR-1: Chain-Agnostic Read RPC + Eliminate `rpc_client.is_some()` Traps

**Why first:** Highest leverage, prevents future EVM breakage. **BLOCKER** for any EVM `ChainRpcClient::Evm` work.

**Files:** `chain_rpc.rs`, `eth_rpc.rs`, `eth_hd_wallet.rs`, `eth.rs`

**Key changes:**
- Add `ChainAddress` dispatch enum
- Implement `ChainRpcOps` for `ChainRpcClient` with `Address = ChainAddress`
- Replace `rpc_client.is_some()` with `match self.0.chain_spec { ChainSpec::Tron => ..., ChainSpec::Evm => ... }`
- Delete incorrect `From<MmError<Web3RpcError>> for ChainRpcError`

**Dependencies:** None | **Blocks EVM:** YES

### PR-2: TRON Activation Gating

Reject token/NFT requests early for TRON activation.

**Files:** `eth_with_token_activation.rs`, `v2_activation.rs`

**Dependencies:** PR-1 (recommended) | **Blocks EVM:** No

### PR-3: TRON Address Correctness

Format addresses as `T...` for TRON, implement `ChainFamily` + `ChainDisplayAddress`.

**Files:** `tron/address.rs`, `eth_hd_wallet.rs`, `eth.rs`, `lp_coins.rs`

**Dependencies:** PR-1 | **Blocks EVM:** No (but blocks TRON correctness)

### PR-4: TRX Activation Tests + Doc Updates

Lock in behavior with tests; update plan docs.

**Dependencies:** PR-2, PR-3 | **Blocks EVM:** No

---

### PR-4.5: ChainBackend Composition (Eliminate State Redundancy)

**Purpose:** Unify `chain_spec` + `rpc_client` into a single `ChainBackend` type, eliminating redundant state and the `rpc_client.is_some()` trap permanently.

**Architectural context:** Currently `EthCoinImpl` stores three overlapping concepts:
- `chain_spec: ChainSpec` — chain identity (Evm vs Tron)
- `rpc_client: Option<ChainRpcClient>` — chain runtime
- `coin_type: EthCoinType` — asset type (Eth, Erc20, Nft)

These are actually **two orthogonal dimensions**:
1. **Chain** (which blockchain) — should be ONE source of truth
2. **Asset** (native vs token vs NFT) — orthogonal to chain

PR-4.5 consolidates the chain dimension into a single `ChainBackend`.

**New types** (add `mm2src/coins/eth/chain_backend.rs`):

```rust
pub enum ChainBackend {
    Evm(EvmBackend),
    Tron(TronBackend),
}

pub struct EvmBackend {
    pub chain_id: u64,
    pub rpc: EvmRpc,
}

/// Transitional: avoids fake placeholder until PR-6
pub enum EvmRpc {
    LegacyWeb3 { web3_instances: Arc<AsyncMutex<Vec<Web3Instance>>> },
    Client(EvmRpcClient),  // PR-6 introduces this
}

pub struct TronBackend {
    pub network: TronNetwork,
    pub rpc: TronRpcClient,
}

impl ChainBackend {
    pub fn spec(&self) -> ChainSpec { /* derived */ }
    pub fn chain_id(&self) -> Option<u64> { /* EVM only */ }
    pub fn format_address(&self, raw: Address) -> String { /* chain-aware */ }
    pub fn tron_rpc(&self) -> Option<&TronRpcClient> { /* TRON only */ }
}
```

**EthCoinImpl changes:**

```rust
// Before (redundant)
pub struct EthCoinImpl {
    pub chain_spec: ChainSpec,
    pub(crate) rpc_client: Option<ChainRpcClient>,
    // ...
}

// After (unified)
pub struct EthCoinImpl {
    pub chain: Arc<ChainBackend>,  // ONE source of truth
    pub coin_type: EthCoinType,    // asset dimension (rename deferred)
    // ...
}
```

**Key benefits:**
- Tokens share `Arc<ChainBackend>` instead of cloning both fields
- No more "TRON spec but missing rpc_client" mismatch possible
- `ChainSpec` becomes derived, not stored redundantly
- EVM uses `EvmRpc::LegacyWeb3` transitionally (no fake placeholder)

**Files:**
| File | Changes |
|------|---------|
| ADD `mm2src/coins/eth/chain_backend.rs` | New module with `ChainBackend`, `EvmBackend`, `TronBackend`, `EvmRpc` |
| `mm2src/coins/eth.rs` | Replace `chain_spec` + `rpc_client` with `chain: Arc<ChainBackend>` |
| `mm2src/coins/eth/v2_activation.rs` | Build `ChainBackend` at activation; tokens clone `Arc` |
| `mm2src/coins/eth/wallet_connect.rs` | Use `chain.chain_id()` instead of matching `chain_spec` |
| `mm2src/coins/eth/eth_hd_wallet.rs` | Use `chain.tron_rpc()` instead of matching |

**Dependencies:** PR-1 (guardrails), PR-2/3/4 (TRX MVP) | **Blocks EVM:** Reduces PR-5/6 blast radius

---

### PR-5: EVM Refactor Scaffolding

Extract `RpcLoopSpawner`, move `EvmRpcClient` to `chain_rpc/evm_rpc.rs`.

**Files:** ADD `web3_transport/spawner.rs`, ADD `chain_rpc/evm_rpc.rs`

**Dependencies:** PR-1 | **Blocks EVM:** Recommended

### PR-6: EVM Backend Bootstrap

Move `web3_instances` ownership into `EvmRpcClient`, wire EVM activation to `ChainRpcClient::Evm`.

**Files:**
| File | Changes |
|------|---------|
| `chain_rpc/evm_rpc.rs` | Own `Web3Instances`, implement `current_block`, `balance_native`, `is_address_used_basic` |
| `chain_rpc.rs` | `ChainRpcClient::Evm` participates in `ChainRpcOps` dispatch |
| `eth.rs` | Remove `web3_instances` field, add accessor via `rpc_client` |
| `eth_rpc.rs`, `eth_swap.rs`, `erc20.rs`, `gas.rs` | Use `self.web3_instances()?` pattern |
| `eth_with_token_activation.rs` | Build `RpcLoopSpawner` → `Web3Instances` → `EvmRpcClient` |

**Key pattern:**
```rust
impl EthCoin {
    pub fn evm_rpc(&self) -> MmResult<&EvmRpcClient, EthCoinError> {
        match self.rpc_client.as_ref() {
            Some(ChainRpcClient::Evm(evm)) => Ok(evm),
            _ => MmError::err(EthCoinError::Internal("EVM rpc_client not initialized".into())),
        }
    }
    pub fn web3_instances(&self) -> MmResult<&Arc<Web3Instances>, EthCoinError> {
        Ok(self.evm_rpc()?.web3_instances())
    }
}
```

**Dependencies:** PR-5 | **Blocks future work:** YES

### PR-7: EVM Broadcast Session Abstraction

Introduce `BroadcastSessionOps` + `ChainTxOps` traits, implement `EvmBroadcastSession`.

**Files:**
| File | Changes |
|------|---------|
| `chain_rpc.rs` | Add `ChainTxOps`, `BroadcastSessionOps` traits, `ChainBroadcastSession` enum |
| `chain_rpc/evm_rpc.rs` | Add `EvmBroadcastSession`, `start_broadcast_session()` |
| `eth_rpc.rs` | Route `eth_sendRawTransaction` through session |

**Key types:**
```rust
pub struct ChainTxHash { pub chain: ChainFamily, pub hash: H256 }
pub enum ChainTxReceipt { Evm(TransactionReceipt), Tron(/* later */) }
pub enum ChainBroadcastSession { Evm(EvmBroadcastSession), Tron(/* later */) }

#[async_trait]
pub trait BroadcastSessionOps: Send + Sync {
    async fn broadcast_raw_tx(&self, raw_tx: Bytes) -> MmResult<ChainTxHash, ChainRpcError>;
    async fn tx_receipt(&self, tx_hash: &ChainTxHash) -> MmResult<Option<ChainTxReceipt>, ChainRpcError>;
}
```

**Dependencies:** PR-6 | **Blocks future work:** YES (foundation for `BroadcastTxPolicy`, nonce management, swap v2 migration)

### PR-8: EVM Broadcast Reliability (BroadcastTxPolicy + Concurrent Fanout + Receipt Wait)

Configurable broadcast behavior with concurrent fanout and receipt waiting.

**Files:**
| File | Changes |
|------|---------|
| `chain_rpc.rs` | Add `BroadcastTxPolicy`, `BroadcastMode`, `RetryPolicy`, `ExponentialBackoff`, `ReceiptWaitPolicy`; extend `BroadcastSessionOps` with `wait_for_receipt()` |
| `chain_rpc/evm_rpc.rs` | Extend `EvmBroadcastSession` with policy + concurrent send + retry loop |
| `web3_transport/web3_instances.rs` | Add `web3_snapshot()` for stable node list |
| `eth_with_token_activation.rs` | Add optional `broadcast_tx` and `receipt_wait` JSON config |

**Key types:**
```rust
pub struct BroadcastTxPolicy {
    pub mode: BroadcastMode,        // FirstSuccess | BroadcastToAll
    pub concurrency: usize,         // how many nodes in parallel
    pub retry: RetryPolicy,         // max_rounds + backoff
}

pub struct ReceiptWaitPolicy {
    pub timeout: Duration,
    pub backoff: ExponentialBackoff,
}

// Default preserves PR-7 behavior: sequential (concurrency=1), no retries
impl Default for BroadcastTxPolicy { ... }
```

**Key features:**
- Concurrent broadcasting via `futures::stream::buffer_unordered`
- Configurable retry with exponential backoff
- `wait_for_receipt()` with timeout and backoff polling
- Backwards compatible: `Default` preserves PR-7 sequential fallback

**Dependencies:** PR-7 | **Blocks future work:** YES (reliability layer for nonce/replacement tx, swap pipeline, TRON parity, observability)

### PR-9: Receipt Finality (min_confirmations + Separate Polling vs Broadcast Retry)

Finality-aware receipt waiting with chain-specific confirmation semantics.

**Files:**
| File | Changes |
|------|---------|
| `chain_rpc.rs` | Reshape `ReceiptWaitPolicy` → `ReceiptPollingPolicy` + `TxFinalityPolicy` + `ConfirmationSource`; add `TxInclusionMeta`, new error variants |
| `chain_rpc/evm_rpc.rs` | Enrich `tx_receipt()` with inclusion + tip metadata |
| `chain_rpc/tests/finality_wait_tests.rs` | min_confirmations, timeout progress, polling error classification |

**Key types:**
```rust
pub struct TxFinalityPolicy {
    pub min_confirmations: NonZeroU64,  // 1 = receipt exists, >1 = wait for more blocks
    pub source: ConfirmationSource,
}

pub enum ConfirmationSource {
    BestTip,                        // EVM: latest, TRON: full node
    FinalizedTip,                   // EVM: finalized/safe, TRON: solidity
    FinalizedIfAvailableElseBest,   // Prefer finalized, fallback to best
}

pub struct TxInclusionMeta {
    pub included_block: u64,
    pub tips: TxTipHeights,  // { best: Option<u64>, finalized: Option<u64> }
}
```

**Key features:**
- Separate `BroadcastRetryPolicy` (bounded attempts) vs `ReceiptPollingPolicy` (time-bounded)
- Chain receipts carry inclusion metadata for confirmation calculation
- EVM: best tip + finalized/safe tag (best-effort), TRON: full node + solidity tips
- New errors: `FinalityTimeout`, `FinalityNotSupported`, `ReceiptPollingStalled`

**Dependencies:** PR-8 | **Blocks future work:** YES (swap hardening, reorg tolerance, TRON parity, EVM finalized/safe)

---

## 13) PR Summary Table

> **See Section 20 for the complete, updated PR summary table** including PR-4.5 (ChainBackend composition) and PR-X (ChainCoin ideal end-state).

Quick reference (MVP PRs):

| PR | Purpose | Status |
|---:|---------|--------|
| **1** | Chain-agnostic read RPC + fix `is_some()` traps | ✅ Complete |
| **2** | TRON activation gating (reject token/NFT) | ✅ Complete |
| **3** | Address formatting (`T...` everywhere) | ✅ Complete |
| **4** | TRX tests + docs updates | ✅ Complete |

**TRX MVP is complete.** All 10 integration tests pass (immediate/task activation, node failover, HD activation, get_new_address, balance structure, gap limit scanning, used-but-zero-balance detection).

---

## 14) Key Principle: Keep PR-1 "Read-Only" and "Mechanical"

PR-1 should **NOT** introduce:
- `ChainTxOps` / `BroadcastSessionOps`
- Move `web3_instances`
- Transaction pipeline changes

It should **ONLY** focus on:
- API shape cleanup (`ChainAddress` + `ChainRpcOps` on `ChainRpcClient`)
- Trap removal in call sites
- Error conversion correctness

This keeps PR-1 small enough to merge quickly, unblocking both safe EVM backend work and TRON correctness PRs.

---

## 15) PR-9 Migration Notes

### Call Site Impact (PR-8 → PR-9)

**Estimated edits:** ~6–18 total
- `ReceiptWaitPolicy { ... }` literals: ~3–10 call sites
- Config/activation parsing: ~1–2 call sites
- Tests: ~2–6 call sites

**Typical diff:**
```rust
// Before (PR-8)
let policy = ReceiptWaitPolicy { timeout, backoff };

// After (PR-9)
let policy = ReceiptWaitPolicy {
    timeout,
    polling: ReceiptPollingPolicy { backoff, max_consecutive_retryable_errors: 10 },
    finality: TxFinalityPolicy { min_confirmations: NonZeroU64::new(1).unwrap(), source: ConfirmationSource::BestTip },
};
```

**Reduce churn with builders:**
```rust
impl ReceiptWaitPolicy {
    pub fn for_inclusion(timeout: Duration) -> Self { Self { timeout, ..Self::default() } }
    pub fn with_min_confirmations(mut self, n: NonZeroU64) -> Self { self.finality.min_confirmations = n; self }
}
// Usage: ReceiptWaitPolicy::for_inclusion(Duration::from_secs(120))
```

---

## 16) Recommended Policy Presets

### Normal Sends / Withdrawals (fast UX)
```rust
BroadcastTxPolicy { mode: FirstSuccess, concurrency: 2, retry: { max_rounds: 2 } }
ReceiptWaitPolicy { timeout: 120s, finality: { min_confirmations: 1, source: BestTip } }
```

### Swap Execution (critical finality)

**EVM swaps:**
```rust
BroadcastTxPolicy { mode: BroadcastToAll, concurrency: 4, retry: { max_rounds: 3 } }
ReceiptWaitPolicy { timeout: 45min, finality: { min_confirmations: 6-12, source: BestTip } }
// Per-chain: ETH L1 = 12, L2s = 3
```

**TRON swaps:**
```rust
ReceiptWaitPolicy { timeout: 45min, finality: { min_confirmations: 1, source: FinalizedTip } }
// FinalizedTip = solidity block
```

### Background Watcher (low RPC pressure)
```rust
ReceiptWaitPolicy { timeout: 6h, polling: { backoff: { initial: 2s, max: 30s } }, finality: { source: FinalizedIfAvailableElseBest } }
```

---

## 17) RPC Pool Trait Refactoring (Future)

### Problem

Node rotation logic is currently duplicated:
- **EVM:** `try_rpc_send` in `eth_rpc.rs`
- **TRON:** `try_clients` in `tron/api.rs`

Both implement the same pattern: try each node, rotate on success, retry on transient errors. This duplication makes the codebase harder to maintain and understand.

### Proposed Solution: `RpcPool` Trait

```rust
#[async_trait]
pub trait RpcPool: Send + Sync + Clone {
    /// Single-node client type (e.g., Web3<Transport> for EVM, TronHttpClient for TRON)
    type Client: Send + Sync + Clone;

    /// Error type for RPC operations
    type Error;

    /// Execute an operation with node rotation and retry logic.
    /// Tries each node until success, handles retryable vs permanent errors.
    async fn try_nodes<F, Fut, T>(&self, op: F) -> Result<T, Self::Error>
    where
        F: Fn(Self::Client) -> Fut + Send + Sync,
        Fut: Future<Output = Result<T, Self::Error>> + Send;

    /// Check if an error is retryable (chain-specific logic).
    /// EVM: transport/timeout errors
    /// TRON: transport/timeout + SERVER_BUSY, NO_CONNECTION, etc.
    fn is_retryable(error: &Self::Error) -> bool;
}
```

### Target Module Structure

```text
mm2src/coins/eth/rpc/
├── mod.rs               # Re-exports, ChainRpcClient enum
├── traits.rs            # RpcPool trait, ChainRpcOps trait
├── evm/
│   ├── mod.rs
│   ├── client.rs        # EvmHttpClient (single node, wraps Web3<Transport>)
│   ├── pool.rs          # EvmRpcPool implements RpcPool
│   └── methods.rs       # EVM-specific RPC methods (balance, nonce, etc.)
└── tron/
    ├── mod.rs
    ├── client.rs        # TronHttpClient (single node)
    ├── pool.rs          # TronRpcPool implements RpcPool
    └── methods.rs       # TRON-specific RPC methods (getaccount, getnowblock)
```

### Implementation Sketch

```rust
// EvmRpcPool
pub struct EvmRpcPool {
    clients: Arc<AsyncMutex<Vec<Web3Instance>>>,
    spawner: RpcLoopSpawner,
}

impl RpcPool for EvmRpcPool {
    type Client = Arc<Web3<Web3Transport>>;
    type Error = Web3RpcError;

    async fn try_nodes<F, Fut, T>(&self, op: F) -> Result<T, Self::Error> { ... }
    fn is_retryable(error: &Self::Error) -> bool { error.is_retryable() }
}

// TronRpcPool
pub struct TronRpcPool {
    clients: Arc<AsyncMutex<Vec<TronHttpClient>>>,
}

impl RpcPool for TronRpcPool {
    type Client = TronHttpClient;
    type Error = Web3RpcError;

    async fn try_nodes<F, Fut, T>(&self, op: F) -> Result<T, Self::Error> { ... }
    fn is_retryable(error: &Self::Error) -> bool {
        error.is_retryable() || is_retryable_tron_error(&error.to_string())
    }
}
```

### Benefits

1. **Single source of truth** for node rotation logic
2. **Consistent retry behavior** across chains
3. **Easier testing** — can test rotation logic once
4. **Clear separation** between single-node client and pool
5. **Extensible** — adding new chains just implements the trait

### When to Do This

This is a **post-MVP refactoring task**. Current priorities:
1. Complete TRON HD activation MVP
2. Fix `rpc_client.is_some()` traps (PR-1)
3. Address correctness for TRON (PR-3)

Then this structural refactoring can be done as a cleanup PR.

---

## 17.5) EVM Error Classification Alignment (Future)

### Problem

TRON now has proper error classification at source:
- **Transient errors** (`Transport`, `Timeout`, `BadResponse`) → retryable, rotate nodes
- **Deterministic rejections** (`RemoteError`) → non-retryable, fail fast with HTTP 400

EVM still uses legacy `web3::Error` conversion that lumps different error types together:

```rust
impl From<web3::Error> for Web3RpcError {
    fn from(e: web3::Error) -> Self {
        match e {
            // All of these become InvalidResponse - mixing deterministic rejections with schema issues
            web3::Error::InvalidResponse(_) | web3::Error::Decoder(_) | web3::Error::Rpc(_) => {
                Web3RpcError::InvalidResponse(error_str)
            },
            // ...
        }
    }
}
```

This creates an inconsistency:
- `web3::Error::Rpc(_)` includes deterministic rejections like "nonce too low", "insufficient funds" → should be `RemoteError` (non-retryable)
- `web3::Error::Decoder(_)` and `web3::Error::InvalidResponse(_)` are often node-specific schema issues → should be `BadResponse` (retryable)

### Proposed Fix

Refactor `impl From<web3::Error> for Web3RpcError`:

```rust
impl From<web3::Error> for Web3RpcError {
    fn from(e: web3::Error) -> Self {
        match e {
            // Deterministic RPC rejection (user error)
            web3::Error::Rpc(rpc_err) => {
                let code = rpc_err.code.to_string();
                let message = rpc_err.message.clone();
                Web3RpcError::RemoteError { code: Some(code), message }
            },
            // Node returned unexpected/malformed payload (faulty node, retry)
            web3::Error::Decoder(e) | web3::Error::InvalidResponse(e) => {
                Web3RpcError::BadResponse(e.to_string())
            },
            // Network/connectivity issues (retry)
            web3::Error::Unreachable | web3::Error::Transport(_) | web3::Error::Io(_) => {
                Web3RpcError::Transport(e.to_string())
            },
            _ => Web3RpcError::Internal(e.to_string()),
        }
    }
}
```

### Benefits

1. **Consistent retry semantics** across TRON and EVM
2. **Correct HTTP status codes** (400 for user errors, 502 for transport)
3. **Principled `is_retryable()`** that works for both chains
4. **Clear separation** of error categories

### When to Do This

This is a **post-MVP refactoring task**. It should be done after:
1. TRON HD activation is stable
2. The TRON classification pattern is validated in production
3. Before the RPC Pool Trait unification (Section 17)

### Files to Change

| File | Changes |
|------|---------|
| `mm2src/coins/eth.rs` | Update `From<web3::Error>` impl |
| Integration tests | Verify EVM error handling still works |

### Risk Assessment

- **Low risk**: Only changes internal error classification, not public APIs
- **Testing required**: Ensure EVM operations (swaps, withdrawals) handle errors correctly
- **Backwards compatible**: Same error variants, just classified differently

---

## 18) Refactor Arc Completion

**PR-9 is a good stopping point.** The ChainRpcClient refactor is complete when:
- ✅ Broadcast behavior is policy-driven and concurrent
- ✅ Receipt waiting is policy-driven (timeouts/backoff)
- ✅ Finality is explicit (min_confirmations + source)
- ✅ Retry concerns are separated (broadcast vs receipt polling)
- ✅ Trait surface stays small and chain-agnostic

### Natural Follow-On PRs (post-refactor)

| PR | Purpose |
|----|---------|
| **RPC caching** | Cache tip-height per polling iteration; skip finalized queries unless requested |
| **TRON finality** | Implement solidity tip retrieval + integration tests for TRON confirmation semantics |
| **Observability** | Structured tracing + metrics for broadcast/polling rounds, timeout reasons, error rates |
| **Config migration** | Backward-compatible parsing (old fields → new nested policy), deprecate old knobs |
| **Endpoint health** | Track per-endpoint success/latency, bias broadcast and polling selection |

---

## 19) Ideal End-State: ChainCoin Typed Model (PR-X)

> **Status:** Future work, post PR-9. Documented here as the architectural north star.

### The Problem: Chain×Asset Combinations

After PR-4.5 (`ChainBackend`), we have two orthogonal dimensions:
- `chain: Arc<ChainBackend>` — Evm | Tron
- `coin_type: EthCoinType` — Eth | Erc20 | Nft

But **not all combinations are valid**:
- ERC20 only exists on EVM chains
- TRC20 only exists on TRON (future)
- NFT only exists on EVM (for now)

With composition, invalid states are *representable* (just prevented at construction). The ideal end-state makes invalid states **unrepresentable at the type level**.

### The Solution: Nested Enum Model

A `ChainCoin` enum where each chain variant contains **only valid asset types for that chain**:

```rust
/// ONE type that IS "an asset on a chain"
pub enum ChainCoin {
    Evm(EvmCoin),
    Tron(TronCoin),
}

/// EVM chain: context + EVM-specific assets
pub struct EvmCoin {
    pub ctx: EvmContext,
    pub asset: EvmAsset,
}

pub struct EvmContext {
    pub chain_id: u64,
    pub rpc: EvmRpcClient,
}

pub enum EvmAsset {
    Native,
    Erc20 { token_addr: Address, platform: String },
    Nft { platform: String },
}

/// TRON chain: context + TRON-specific assets
pub struct TronCoin {
    pub ctx: TronContext,
    pub asset: TronAsset,
}

pub struct TronContext {
    pub network: TronNetwork,
    pub rpc: TronRpcClient,
}

pub enum TronAsset {
    Native,
    // Trc20 { contract_addr: TronAddress, ... } // future
}
```

### What This Achieves

| Goal | How |
|------|-----|
| **Invalid states unrepresentable** | No `EvmAsset::Erc20` inside `TronCoin` — doesn't compile |
| **Real-world structure** | "Coin IS asset on chain" — single unified object |
| **No field duplication** | Context factored out, not repeated per asset variant |
| **Generic over any coin** | `impl ChainCoin { fn format_address()... }` with match dispatch |
| **ChainSpec derived** | `chain_coin.spec()` returns lightweight identity |
| **Asset type derived** | `chain_coin.asset_kind()` returns generic view |

### EthCoinImpl Becomes Minimal

```rust
pub struct EthCoinImpl {
    pub chain_coin: ChainCoin,  // THE single source of truth
    // ... swap contracts, gas policy, etc. (EVM-only fields gated appropriately)
}

impl EthCoinImpl {
    pub fn chain_spec(&self) -> ChainSpec { self.chain_coin.spec() }
    pub fn is_native(&self) -> bool { self.chain_coin.is_native() }
}
```

### PR-X Implementation Steps

1. **Add model types** (`mm2src/coins/eth/chain_coin.rs`):
   - `ChainCoin`, `EvmCoin`, `TronCoin`
   - `EvmContext`, `TronContext`
   - `EvmAsset`, `TronAsset`

2. **Refactor `EthCoinImpl`**:
   - Replace `chain: Arc<ChainBackend>` + `coin_type: EthCoinType`
   - With `chain_coin: ChainCoin`

3. **Update v2 activation**:
   - Build `ChainCoin::Evm(...)` or `ChainCoin::Tron(...)` based on protocol
   - Invalid combos fail at construction (type-enforced)

4. **Migrate call sites**:
   - Replace `match (chain_spec, coin_type)` with `match chain_coin`
   - Each chain variant handles only its valid assets

### Why This Is Post-PR-9

- Requires `EvmRpcClient` to be fully implemented (PR-6)
- Significant structural change — better after RPC layer is stable
- MVP and core refactor can ship without this

### Deferred: `EthCoinType` → `AssetKind` Rename

**TODO:** Before or alongside PR-X, rename `EthCoinType` to `AssetKind` for clarity:

```rust
// Current (confusing)
pub enum EthCoinType {
    Eth,  // Used for TRX too!
    Erc20 { ... },
    Nft { ... },
}

// Future (chain-agnostic naming)
pub enum AssetKind {
    Native,  // ETH on EVM, TRX on TRON
    Token { token_addr: Address, platform: String },
    Nft { platform: String },
}
```

This rename is mechanical but touches many files. Can be done as a separate PR before PR-X.

---

## 20) PR Summary Table (Updated 2025-12-31)

| PR | Purpose | Depends On | MVP? | Blocks EVM? | Status |
|---:|---------|------------|------|-------------|--------|
| **1** | Chain-agnostic read RPC + fix `is_some()` traps | — | ✅ Guardrail | **YES** | ✅ Complete |
| **2** | TRON activation gating (reject token/NFT) | PR-1 | ✅ | No | ✅ Complete |
| **3** | Address formatting (`T...` everywhere) | PR-1 | ✅ | No | ✅ Complete |
| **4** | TRX tests + docs updates | PR-2, PR-3 | ✅ | No | ✅ Complete |
| **4.5** | **ChainBackend composition** (eliminate redundancy) | PR-1, PR-2/3/4 | Post-MVP | Reduces blast radius | Not started |
| **5** | EVM scaffolding (RpcLoopSpawner) | PR-1 | Post-MVP | Recommended | Not started |
| **6** | EVM backend bootstrap (`web3_instances` → `EvmRpcClient`) | PR-5 | Post-MVP | **YES** | Not started |
| **7** | EVM broadcast session abstraction | PR-6 | Post-MVP | **YES** | Not started |
| **8** | EVM broadcast reliability (policy + concurrency) | PR-7 | Post-MVP | **YES** | Not started |
| **9** | Receipt finality (min_confirmations) | PR-8 | Post-MVP | **YES** | Not started |
| **X** | **ChainCoin typed model** (ideal end-state) | PR-6+ | Future | No | Not started |

### Visual PR Flow

```
┌─────────────────────────────────────────────────────────────────┐
│                   TRX MVP ✅ COMPLETE                           │
│  PR-1 ✅ ──→ PR-2 ✅ ──→ PR-3 ✅ ──→ PR-4 ✅                    │
│   │                                                             │
│   └──────────────────────────────────────────────────┐          │
└─────────────────────────────────────────────────────────────────┘
                                                       │
┌─────────────────────────────────────────────────────────────────┐
│                    Post-MVP Refactor                            │
│                                                      ▼          │
│                                                   PR-4.5        │
│                                                      │          │
│  PR-5 ◄──────────────────────────────────────────────┘          │
│    │                                                            │
│    ▼                                                            │
│  PR-6 ──→ PR-7 ──→ PR-8 ──→ PR-9                               │
│                                │                                │
└────────────────────────────────┼────────────────────────────────┘
                                 │
┌────────────────────────────────┼────────────────────────────────┐
│                    Future / Ideal                               │
│                                ▼                                │
│                              PR-X (ChainCoin typed model)       │
│                                                                 │
│  + AssetKind rename (can be before or with PR-X)               │
└─────────────────────────────────────────────────────────────────┘
```
