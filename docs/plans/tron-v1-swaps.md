# TRON V1 Atomic Swap Implementation Plan

> **Status**: Active
> **Scope**: V1 swaps only (NOT V2)
> **Repos**: KDF, etomic-swap, coins, docs
> **Last updated**: 2026-03-14

## Core Recommendations

* **Make the contract change first.**
* **Keep `eth.rs` as the dispatch façade only.**
* **Put TRON V1 swap logic in focused `coins/eth/tron/` modules.**
* **Add a first-class `TronSignedTransaction` and `TransactionEnum::TronTransaction`.**
* **Deploy a separate TRON V1 contract artifact instead of trying to reuse the Ethereum one.**

TRON smart-contract execution, receipts, and indexed events are reached through TRON's HTTP APIs and indexed event services rather than Ethereum-style `eth_getLogs`, so the KDF swap path needs real chain-family dispatch rather than more EVM shims.

## Resolved Design Decisions

### Decision 1: SHA-256 for TRON V1 Contract (Override)

**Chosen:** SHA-256 (native TVM opcode) with `bytes32 secretHash` in V1 ABI.

**Why (override rationale)**

* Every known TRON HTLC (Jelly Swap, MovNetwork, reference impls) uses SHA-256.
* KDF's `secret_hash_algo_v2()` already returns `SHA256` for `EthCoinVariant` (which includes TRON).
* The `staticcall(0x20003)` approach for RIPEMD-160 is non-standard, used by zero production TRON contracts, and cannot be audited.
* The V1 `detect_secret_hash_algo()` change is only **2 lines** — a pure variant-to-algorithm lookup, not swap logic:
  ```rust
  (MmCoinEnum::EthCoinVariant(c), _) if c.is_tron() => SecretHashAlgo::SHA256,
  (_, MmCoinEnum::EthCoinVariant(c)) if c.is_tron() => SecretHashAlgo::SHA256,
  ```
* The ABI change (`bytes20` → `bytes32` for secretHash) is contained to the TRON contract; EVM V1 ABI is untouched.

**Rejected alternative:** RIPEMD-160 via `0x20003` — fragile, unprecedented, closed to audit.

---

### Decision 2: TronBox for TRON Contract Tooling (Override)

**Chosen:** TronBox for compile/deploy/verify.

**Why (override rationale)**

* `@layerzerolabs/hardhat-tron` and `@layerzerolabs/hardhat-deploy` have **no public source code** — repos are private/deleted on GitHub.
* Cannot audit, fork, or fix bugs in closed-source build dependencies.
* TronBox is official (tronprotocol/tronbox), open-source, actively maintained (~1,597 npm downloads/week), v4.3.0.

**Rejected alternative:** Hardhat + LayerZero plugins — closed-source dependency risk.

---

### Decision 3: Deployment Artifacts in etomic-swap + KDF/coins as Runtime Source

**Chosen:** TronBox-generated deployment artifacts checked into `etomic-swap/deployments/nile/`, but KDF test helpers and coins configs remain the runtime source of truth for addresses.

---

### Decision 4: Separate `TransactionEnum::TronTransaction` Variant (Approach A)

**Chosen:** Add `TransactionEnum::TronTransaction(TronSignedTransaction)` as a new variant.

**Why**

* **0 existing function signature changes** — all 34+ functions using `SignedEthTx` keep their signatures.
* ~5 match site additions (`Deref`, `validate_fee`, `ifrom!`, etc.).
* Matches how Sia/Cosmos were added — additive, minimal blast radius.
* Wrapper enum (Approach B) would require 34+ signature changes, 14+ call site changes, ~8 test changes.

---

### Decision 5: Shared EthCoin Fee Methods Cover Both Legacy and V2

**Chosen:** Add TRON arms to `get_sender_trade_fee`, `get_receiver_trade_fee`, `get_fee_to_send_taker_fee`. Both legacy `trade_preimage` RPC and V2 state machine callers are covered automatically since they call the same EthCoin methods.

---

### Decision 6: Per-Commit Tests + Catch-All

**Chosen:** Every functional commit (4–17) includes its own unit + integration tests. Commits 19/20 remain as catch-all for coverage gaps and full end-to-end Nile swap flows.

---

### Decision 7: Base58 in Config, Auto-Convert to H160 at Activation (Override)

**Chosen:** Store TRON addresses in Base58 (`T...`) format in coins config. Auto-convert to H160 during activation.

**Why (override rationale)**

* Base58 is what TronScan and the TRON ecosystem use natively — human-readable and operator-friendly.
* Auto-conversion at activation time gives the correct runtime type (`Address = H160`).
* Dual constants (`*_H160` / `*_BASE58`) in test helpers for developer convenience.
* No second config field needed.

---

## Architectural Rules

### Off-Limits Files (No Functional Swap Logic Changes)

These files must NOT have functional swap logic changes. Only data-routing additions (like `detect_secret_hash_algo` variant mapping) are allowed:

* `mm2_main/src/lp_swap.rs` — only the 2-line SHA-256 mapping addition
* `mm2_main/src/lp_swap/maker_swap.rs`
* `mm2_main/src/lp_swap/taker_swap.rs`
* `mm2_main/src/lp_swap/maker_swap_v2.rs`
* `mm2_main/src/lp_swap/taker_swap_v2.rs`

### Nile Proof Gate

Commit 3 must include an explicit Nile proof that SHA-256 works correctly on the deployed contract for a known test vector. If that proof fails, stop and re-evaluate before touching KDF.

### Release Gate

After commit 20 (tests), perform mainnet deployment using commit-2 tooling, verify on TronScan, and record both address forms. This is the prerequisite for the runtime config PR (commit 21).

---

## Discrepancies Found

1. **The research doc is mixed V1/V2.**
   `kdf/docs/research/tron-swap-research.md` has V2 recommendations in the executive summary, but the code and the requested scope are clearly **V1-only**.

2. **TRON activation is already mostly there.**
   `EthActivationV2Request` already has `swap_contract_address: Option<Address>`, and `resolve_swap_contracts()` in `v2_activation.rs` already handles it generically. The real missing pieces are:
   * TRON orderbook address derivation
   * swap-enabled test/config helpers
   * swap runtime behavior

3. **`sign_transaction_with_keypair` is the wrong seam for literal TRON support.**
   The helper currently returns `(SignedEthTx, Vec<Web3Instance>)`, so it is structurally EVM-only. The right fix is:
   * keep the EVM signer EVM-only
   * add a TRON contract-call signer
   * wire swap paths and raw-tx signing through chain dispatch

4. **Pre-existing EVM validator gap.**
   In `eth.rs:~5683`, the ERC20 non-watcher validation path skips `decoded[1]` (amount) validation against `trade_amount`. Fix alongside TRON validation in commit 14.

5. **The coins repo does not contain native TRON coin entries yet.**
   Only `TRX-BEP20` (BSC token) exists. Coins-repo work is "add TRON coin definitions + swap_contract_address."

---

# Part 1: Architecture Overview

## 1) Module Structure

### KDF

```text
kdf/mm2src/coins/
├── eth.rs                                  # keep as SwapOps façade + ChainFamily dispatch
├── lp_coins.rs                             # add TransactionEnum::TronTransaction
├── tx_fee_details.rs                       # already supports Tron; reuse, no schema change
└── eth/
    ├── chain_rpc.rs                        # small explicit-match guardrail edits only
    ├── swap_contract_abi.json              # unchanged for EVM; TRON uses SHA-256 ABI variant
    └── tron/
        ├── api.rs                          # extend RPC structs + estimateenergy + events + richer receipts
        ├── contract.rs                     # NEW: generic TRON contract-call build/sign/send/read helpers
        ├── events.rs                       # NEW: normalize TronGrid and receipt logs for ABI decoding
        ├── fee.rs                          # extend contract-call fee estimation
        ├── proto.rs                        # existing protobuf messages, reused
        ├── sign.rs                         # existing signer, reused
        ├── swap.rs                         # NEW: TRON V1 swap-specific payment/spend/refund/validate/search logic
        ├── transaction.rs                  # NEW: TronSignedTransaction + decode helpers
        └── tx_builder.rs                   # extend with generic TriggerSmartContract builder

kdf/mm2src/mm2_main/src/
├── lp_swap.rs                              # 2-line detect_secret_hash_algo() addition ONLY
└── lp_ordermatch.rs                        # add TRX/TRC20 orderbook address derivation

kdf/mm2src/mm2_test_helpers/src/
└── for_tests.rs                            # add TRON swap-enabled configs/constants

kdf/mm2src/mm2_main/tests/mm2_tests/
└── tron_tests.rs or tron_swap_tests.rs     # Nile swap integration tests
```

### etomic-swap

```text
etomic-swap/
├── contracts/
│   ├── EtomicSwap.sol                      # leave Ethereum contract unchanged
│   └── EtomicSwapTron.sol                  # NEW: TRON V1 contract with SHA-256 secretHash
├── tronbox-config.js                       # NEW
├── migrations/
│   └── 2_deploy_etomic_swap_tron.js        # NEW
├── test/
│   ├── EtomicSwap.js                       # existing Ethereum tests
│   └── EtomicSwapTron.js                   # NEW
└── deployments/
    └── nile/EtomicSwapTron.json            # NEW deployed artifact metadata
```

### coins repo

```text
coins/
├── coins                                   # canonical coin definitions (Base58 swap_contract_address)
└── utils/
    ├── coins_config.json
    ├── coins_config_ssl.json
    ├── coins_config_tcp.json
    ├── coins_config_wss.json
    └── coins_config_unfiltered.json
```

## 2) Key Abstractions

No new cross-chain swap trait. `ChainFamily` is the intended abstraction.

```rust
/// Signed TRON transaction stored in KDF swap flows.
#[derive(Clone, Debug, PartialEq)]
pub struct TronSignedTransaction {
    pub tx_hash: H256,
    pub raw_tx_hex: BytesJson,
    pub tx: crate::eth::tron::proto::Transaction,
}

impl Transaction for TronSignedTransaction {
    fn tx_hex(&self) -> Vec<u8> { self.raw_tx_hex.0.clone() }
    fn tx_hash_as_bytes(&self) -> BytesJson {
        BytesJson::from(self.tx_hash.as_bytes().to_vec())
    }
}
```

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum TransactionEnum {
    UtxoTx(UtxoTx),
    SignedEthTx(SignedEthTx),
    TronTransaction(crate::eth::tron::TronSignedTransaction),
    ZTransaction(z_coin::ZTransaction),
    CosmosTransaction(crate::tendermint::TendermintTx),
    LightningPayment(crate::lightning::ln_platform::PaymentInfo),
    SiaTransaction(crate::siacoin::SiaTransaction),
}
```

```rust
/// Generic TRON smart-contract call input.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TronContractCallInput {
    pub owner: crate::eth::tron::TronAddress,
    pub contract: crate::eth::tron::TronAddress,
    pub data: Vec<u8>,      // 4-byte selector + ABI args
    pub call_value: u64,    // SUN
    pub fee_limit: u64,     // SUN cap
}
```

```rust
/// Normalized event record for TRON swap discovery.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SwapContractLog {
    pub tx_hash: H256,
    pub block_number: u64,
    pub block_timestamp_ms: u64,
    pub topics: Vec<H256>,
    pub data: Vec<u8>,
}
```

```rust
/// Internal TRON swap error. Mapped to existing public error types at boundaries.
#[derive(Debug, derive_more::Display)]
pub(crate) enum TronSwapError {
    #[display(fmt = "TRON RPC error: {_0}")]
    Rpc(String),
    #[display(fmt = "ABI encoding or decoding error: {_0}")]
    Abi(String),
    #[display(fmt = "Invalid TRON transaction: {_0}")]
    InvalidTransaction(String),
    #[display(fmt = "Unexpected contract method selector: expected {expected}, found {found}")]
    UnexpectedMethod { expected: &'static str, found: String },
    #[display(fmt = "Swap contract event not found")]
    EventNotFound,
    #[display(fmt = "Missing or incomplete transaction receipt for {tx_hash}")]
    MissingReceipt { tx_hash: H256 },
    #[display(fmt = "TRON watcher reward paths are not supported for V1 swaps")]
    WatchersUnsupported,
}
```

### Error Mapping

* `TronSwapError -> TransactionErr` for send/spend/refund paths
* `TronSwapError -> ValidatePaymentError` for fee/payment validation
* `TronSwapError -> TradePreimageError` for fee estimation
* plain `String` for `extract_secret` / `search_for_swap_tx_spend`

---

## 3) Dependency Graph

```text
lp_coins.rs
  └── depends on tron/transaction.rs (type only)

eth.rs
  ├── depends on tron/swap.rs
  ├── depends on tron/contract.rs
  ├── depends on tron/transaction.rs
  └── depends on existing EVM helpers

tron/swap.rs
  ├── depends on EthCoin / ChainFamily / swap ABI
  ├── depends on tron/contract.rs
  ├── depends on tron/events.rs
  ├── depends on tron/api.rs
  ├── depends on tron/transaction.rs
  └── depends on tron/fee.rs

tron/contract.rs
  ├── depends on tron/api.rs
  ├── depends on tron/tx_builder.rs
  ├── depends on tron/sign.rs
  ├── depends on tron/fee.rs
  └── depends on tron/transaction.rs

tron/events.rs
  ├── depends on tron/api.rs
  └── depends on swap ABI
```

---

## 4) Data-Flow Diagrams

### A. Send Maker/Taker Payment

**TRON target**

```text
SwapOps::send_maker_payment / send_taker_payment
→ EthCoin::send_hash_time_locked_payment()
→ match ChainFamily::from(&self.chain_spec)
→ tron::swap::send_hash_time_locked_payment()
→ ABI encode ethPayment / erc20Payment (with bytes32 secretHash)
→ optional approve(0) + approve(max) for USDT-like zero-first tokens
→ tron::contract::estimate_fee_limit()
→ tron::tx_builder::build_trigger_smart_contract()
→ tron::sign::sign_tron_transaction()
→ tron::api::broadcast_hex()
→ TransactionEnum::TronTransaction
```

### B. Validate Maker/Taker Payment

```text
SwapOps::validate_*_payment
→ EthCoin::validate_payment()
→ match ChainFamily
→ tron::swap::validate_payment()
→ protobuf decode payment_tx bytes
→ extract TriggerSmartContract.data
→ decode ethPayment / erc20Payment ABI
→ tron::contract::constant_call(payments(id))
→ tron::api::get_transaction_by_id() and/or receipt
→ compare owner_address / call_value / token / receiver / secret hash / timelock
```

### C. Search for Spend/Refund Tx

```text
SwapOps::search_for_swap_tx_spend_*
→ match ChainFamily
→ tron::swap::search_for_swap_tx_spend()
→ protobuf decode original payment tx
→ derive swap id
→ tron::events::find_event()
  ├─ primary: indexed contract events API
  └─ confirmation fallback: tx receipt logs
→ fetch candidate tx by hash
→ FoundSwapTxSpend::{Spent, Refunded}(TronSignedTransaction)
```

### D. Extract Secret

```text
SwapOps::extract_secret()
→ protobuf decode signed tx bytes
→ read TriggerSmartContract.data
→ inspect selector
→ ABI decode receiverSpend
→ return decoded secret
```

---

## 5) Contract Modification Strategy

Use a **separate** TRON contract file with SHA-256 secretHash.

### Key Differences from EVM V1

| Field | EVM V1 | TRON V1 |
|-------|--------|---------|
| `secretHash` type | `bytes20` (DHASH160) | `bytes32` (SHA-256) |
| Hash function | `ripemd160(abi.encodePacked(sha256(...)))` | `sha256(...)` |
| `Payment.paymentHash` | `bytes20` | `bytes32` |

### Contract Structure

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.33;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

contract EtomicSwapTron {
    using SafeERC20 for IERC20;

    enum PaymentState { Uninitialized, PaymentSent, ReceiverSpent, SenderRefunded }

    struct Payment {
        bytes32 paymentHash;  // SHA-256 based
        uint64 lockTime;
        PaymentState state;
    }

    mapping(bytes32 => Payment) public payments;

    event PaymentSent(bytes32 id);
    event ReceiverSpent(bytes32 id, bytes32 secret);
    event SenderRefunded(bytes32 id);

    // Uses sha256() natively — no 0x20003 hack needed
    // Payment hash: sha256(abi.encodePacked(secret))
    // compared directly as bytes32
}
```

### Security Notes

* Checks-effects-interactions pattern preserved from EVM V1.
* No `SELFDESTRUCT` path.
* TVM's smaller call-depth limit doesn't affect this HTLC path.

---

## 6) Testing Strategy

### Per-Commit Tests (Commits 4–17)

Each functional commit includes:
* Focused unit tests for the specific functionality added
* Targeted Nile smoke test when behavior depends on real TRON execution

### Offline Unit Tests (Commit 19 — Catch-All)

Remaining coverage gaps:
* address conversions
* ABI selector checks
* protobuf recovery corner cases
* receipt-log normalization edge cases
* zero-first approval regression fixtures
* explicit "no event endpoint configured" errors

### End-to-End Nile Tests (Commit 20 — Catch-All)

Full feature-gated Nile swap flows:
1. Maker payment → spend (TRX, TRC20)
2. Taker payment → spend (TRX, TRC20)
3. Payment → refund after timelock (TRX, TRC20)
4. Payment validation
5. Secret extraction
6. Spend/refund discovery
7. Fee estimation sanity

### Timing and Polling

* Poll in ~3–5 second intervals
* Use bounded retries, not brittle sleeps
* Use receipt confirmation for known tx, event scans for discovery

### CI Strategy

Keep tests feature-gated. Add a separate manual/nightly workflow only after Nile stability is proven.

---

## 7) Deployment Pipeline

1. Add `EtomicSwapTron.sol` with SHA-256
2. Compile with TronBox / tron-solc
3. Deploy once to Nile + verify SHA-256 works for known test vector
4. Verify on TronScan
5. Run KDF Nile swap tests
6. **Release gate:** After soak, deploy once to mainnet
7. Add concrete swap addresses to coins configs (Base58 format)
8. Auto-convert to H160 at activation time

### USDT Note

TronScan metadata identifies mainnet TRON USDT as `TetherToken` with legacy approval-related methods. Implement **approve-to-zero-first fallback** for TRC20 swap approvals.

---

# Part 2: Commit-by-Commit Plan

## Contract Preparation

### Commit 1: `feat(tron-contract): add TVM-compatible V1 swap contract with SHA-256`

- [ ] **Done**

**Repo:** `etomic-swap`

**Files**
* `contracts/EtomicSwapTron.sol` **(new)**
* `test/EtomicSwapTron.js` **(new)**

**Description**
* Copy V1 contract into `EtomicSwapTron.sol`.
* Change hash algorithm from RIPEMD-160 to SHA-256:
  * `bytes20 secretHash` → `bytes32 secretHash`
  * `bytes20 paymentHash` → `bytes32 paymentHash` in `Payment` struct
  * Replace `ripemd160(abi.encodePacked(sha256(...)))` with `sha256(...)`
* Keep same events, `payments(bytes32)` mapping, function names.
* Add JS tests with fixed vectors.

**Tests in this commit:** `npx tronbox compile`; ABI test ensuring function/event signatures match expected TRON V1 ABI; SHA-256 hash vector tests.

**Dependencies:** none

---

### Commit 2: `build(tron-contract): add TronBox compile and deployment tooling`

- [ ] **Done**

**Repo:** `etomic-swap`

**Files**
* `tronbox-config.js` **(new)**
* `migrations/2_deploy_etomic_swap_tron.js` **(new)**
* `package.json` / scripts **(modify)**

**Description**
* Add TronBox network definitions for Nile and Mainnet.
* Use env vars for private key and fullHost.
* Add compile/deploy scripts.
* Record required compiler pin and optimizer settings.

**Tests in this commit:** `tronbox compile`; dry-run migration on Nile config.

**Dependencies:** 1

---

### Commit 3: `chore(tron-contract): deploy to Nile and record deployment metadata`

- [ ] **Done**

**Repo:** `etomic-swap`

**Files**
* `deployments/nile/EtomicSwapTron.json` **(new)**
* helper script printing both address forms **(new)**

**Description**
* Deploy once to Nile.
* Check in generated deployment artifact (base58 + H160 addresses, deployment tx hash, compiler version, optimizer config, bytecode hash).
* **Nile proof gate:** Include explicit test that SHA-256 works correctly on the deployed contract for a known test vector. If this fails, stop and re-evaluate.

**Tests in this commit:** `payments(id)` constant call on Nile; SHA-256 hash verification on-chain vs off-chain for known vector.

**Dependencies:** 2

---

## KDF Infrastructure

### Commit 4: `feat(tron): add TronSignedTransaction and TransactionEnum support`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth/tron/transaction.rs` **(new)**
* `mm2src/coins/eth/tron.rs` **(modify)**
* `mm2src/coins/lp_coins.rs` **(modify)**

**Description**
* Add `TronSignedTransaction`.
* Add `TransactionEnum::TronTransaction` variant.
* Update `lp_coins.rs` `ifrom!` macros and `Deref` match.

**Tests in this commit:** unit tests for protobuf decode/encode, tx hash extraction, `TransactionEnum` deref behavior.

**Dependencies:** none

---

### Commit 5: `refactor(eth): make V1 swap internals chain-generic while keeping public swap framework unchanged`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth.rs`
* `mm2src/mm2_main/src/lp_swap.rs` — **2-line addition only** to `detect_secret_hash_algo()`

**Description**
* Rename `sign_transaction_with_keypair` → `sign_evm_transaction_with_keypair`.
* Refactor internal V1 swap helpers to return `TransactionEnum` instead of hard-wiring `SignedEthTx`.
* Add TRON SHA-256 mapping to `detect_secret_hash_algo()` (both native and WASM variants).

**Tests in this commit:** full compile; existing EVM swap tests pass unchanged.

**Dependencies:** 4

---

### Commit 6: `feat(tron): add generic TriggerSmartContract builder`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth/tron/tx_builder.rs`

**Description**
* Add `build_trigger_smart_contract(...)` for swap payments, spends, refunds, approvals, and raw contract calls.
* Refactor existing `build_trc20_transfer()` to delegate where practical.

**Tests in this commit:** golden protobuf builder tests; existing transfer builders still pass.

**Dependencies:** 4

---

### Commit 7: `feat(tron): extend API client for constant calls, energy estimation, receipts, and events`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth/tron/api.rs`

**Description**
* Add typed support for: `triggerconstantcontract`, `estimateenergy`, richer `gettransactionbyid`, richer `gettransactioninfobyid`, indexed contract events API.
* Keep explicit chain matches; no `is_some()` traps.

**Tests in this commit:** JSON fixture deserialization tests for all new response structs; client rotation tests.

**Dependencies:** 6

---

### Commit 8: `feat(tron): add generic contract-call signing, fee-limit estimation, and ETH-path raw signing support`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth/tron/contract.rs` **(new)**
* `mm2src/coins/eth/tron/fee.rs` **(modify)**
* `mm2src/coins/eth.rs` **(modify)**

**Description**
* Add generic TRON contract-call build/sign/broadcast helpers.
* Wire TRON contract-call path into raw signing.
* Energy estimation: try `estimateenergy` → fallback `triggerconstantcontract` → apply 1.25x safety multiplier.

**Tests in this commit:** unit tests for fee-limit fallback logic; raw-signing tests returning signed protobuf hex.

**Dependencies:** 5, 6, 7

---

### Commit 9: `feat(tron): normalize indexed events and receipt logs for swap ABI decoding`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth/tron/events.rs` **(new)**

**Description**
* Add `SwapContractLog` type.
* Normalize TronGrid-style indexed events and receipt `log[]` entries into one type.
* Reuse existing ABI decoder path.

**Tests in this commit:** fixture tests for `PaymentSent`, `ReceiverSpent`, `SenderRefunded` from both event sources.

**Dependencies:** 7

---

## EthCoin TRON Swap Functionality

### Commit 10: `feat(tron): implement taker-fee send and validation for TRX and TRC20`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth/tron/swap.rs` **(new)**
* `mm2src/coins/eth.rs` **(modify)**

**Description**
* Add TRON branches in `EthCoin` for `send_taker_fee` and `validate_fee`.
* TRX: transfer flow. TRC20: contract-call transfer flow.
* All changes stay in `eth.rs` + `eth/tron/*`; no swap framework edits.

**Tests in this commit:** targeted Nile tests for TRX and TRC20 taker fee send/validate.

**Dependencies:** 8

---

### Commit 11: `feat(tron): add TRX and TRC20 orderbook address derivation`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/mm2_main/src/lp_ordermatch.rs`

**Description**
* Replace TRON `CoinIsNotSupported` stub with real orderbook address generation.
* Use existing ETH-family pubkey-to-H160 core plus TRON Base58 formatting.

**Tests in this commit:** deterministic pubkey → Base58 TRON address vectors.

**Dependencies:** none

---

### Commit 12: `feat(tron): add payment status reads, wait_for_confirmations, and payment recovery helpers`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth/tron/swap.rs`
* `mm2src/coins/eth.rs`

**Description**
* Add `payments(id)` constant reads via `triggerconstantcontract`.
* Decode original payment txs from protobuf.
* Add `check_if_my_payment_sent` TRON path.
* `wait_for_confirmations` must use txid from protobuf (not RLP), poll TRON receipt/solidified endpoints.

**Tests in this commit:** unit tests for `payments(id)` decode; Nile smoke test waiting for confirmations.

**Dependencies:** 7, 10

---

### Commit 13: `feat(tron): implement maker and taker payment send for TRX and TRC20`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth/tron/swap.rs`
* `mm2src/coins/eth.rs`

**Description**
* TRX path: `TriggerSmartContract` with `call_value`.
* TRC20 path: allowance check + approve, including approve-to-zero-first fallback for USDT-like behavior.
* Watcher-reward V1 variants: explicitly unsupported on TRON.

**Tests in this commit:** Nile payment-send smoke tests for TRX and TRC20; unit test for zero-first approval fallback.

**Dependencies:** 12

---

### Commit 14: `fix(eth): implement TRON payment validation and fix legacy ERC20 amount validation bug`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth/tron/swap.rs`
* `mm2src/coins/eth.rs`

**Description**
* Implement TRON `validate_maker_payment` / `validate_taker_payment` using protobuf decode + on-chain `payments(id)` cross-check.
* **EVM bug fix:** In non-watcher ERC20 path (`eth.rs:~5683`), `decoded[1] == expected_amount` check must run unconditionally, not only inside `if let Some(watcher_reward)`.

**Tests in this commit:**
* TRON positive and negative validation tests
* EVM regression test proving non-watcher ERC20 amount mismatch is now rejected

**Dependencies:** 13

---

### Commit 15: `feat(tron): implement spend and refund transaction sends`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth/tron/swap.rs`
* `mm2src/coins/eth.rs`

**Description**
* Implement TRON branches for all four spend/refund methods.
* Decode original payment tx from protobuf, reconstruct args, check on-chain state before sending.

**Tests in this commit:** Nile spend tests for TRX/TRC20; Nile refund tests after timelock.

**Dependencies:** 14

---

### Commit 16: `feat(tron): implement spend/refund discovery and secret extraction`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth/tron/swap.rs`
* `mm2src/coins/eth.rs`

**Description**
* `extract_secret`: protobuf decode → ABI decode `receiverSpend` → return secret.
* `search_for_swap_tx_spend_*`: indexed events for discovery, receipt logs for confirmation.
* If no event-capable endpoint configured, fail clearly.

**Tests in this commit:** unit tests for secret extraction; Nile tests for spend/refund discovery.

**Dependencies:** 9, 15

---

### Commit 17: `feat(tron): implement shared TRON trade-fee estimation for legacy and V2 callers`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/coins/eth.rs`
* `mm2src/coins/eth/tron/fee.rs`
* `mm2src/coins/eth/tron/swap.rs`

**Description**
* Add TRON arms to `get_sender_trade_fee`, `get_receiver_trade_fee`, `get_fee_to_send_taker_fee`.
* Covers both legacy `trade_preimage` RPC and V2 state-machine callers.

**Tests in this commit:**
* Direct EthCoin fee-method tests for multiple `FeeApproxStage` values
* `trade_preimage` RPC tests for TRX and TRC20
* Nile sanity tests comparing estimated vs actual fees

**Dependencies:** 10, 13, 15

---

## Test Helpers and Coverage

### Commit 18: `test(tron): add swap-enabled TRON test helpers and dual-format contract constants`

- [ ] **Done**

**Repo:** `KDF`

**Files**
* `mm2src/mm2_test_helpers/src/for_tests.rs`

**Description**
* Add TRON Nile swap contract constants in both forms: `*_H160` and `*_BASE58`.
* Add swap-enabled TRX/TRC20 configs.
* Add pre-funded maker/taker fixtures.

**Tests in this commit:** helper/config deserialization tests; activation smoke tests.

**Dependencies:** 3, 17

---

### Commit 19: `test(tron): add remaining offline regression and fixture coverage`

- [ ] **Done**

**Repo:** `KDF`

**Description**
Catch everything not already covered by per-commit tests:
* address conversions
* ABI selector checks
* protobuf recovery corner cases
* receipt-log normalization edge cases
* zero-first approval regression fixtures
* explicit "no event endpoint configured" errors

**Tests in this commit:** offline/unit only.

**Dependencies:** 18

---

### Commit 20: `test(tron): add full Nile end-to-end TRON V1 swap flows`

- [ ] **Done**

**Repo:** `KDF`

**Description**
Full feature-gated Nile swap flows:
* maker payment → spend (TRX, TRC20)
* taker payment → spend (TRX, TRC20)
* payment → refund after timelock (TRX, TRC20)
* payment validation, secret extraction, spend/refund discovery, fee estimation sanity

**Tests:** `cargo test --test mm2_tests_main --features tron-network-tests tron_...`

**Dependencies:** 18, 19

---

## Release Gate

After commit 20: **mainnet deployment** using commit-2 tooling, verify on TronScan, record both address forms. This is the prerequisite for the runtime config PR.

---

## Runtime Config and Docs

### Commit 21: `chore(tron): add TRON swap contract addresses to runtime configs`

- [ ] **Done**

**Repo:** `coins`

**Files**
* `coins/coins`
* generated configs under `coins/utils/`

**Description**
* Add/update TRX/TRC20 entries with `swap_contract_address` in **Base58 format**.
* KDF activation auto-converts Base58 to H160 at runtime.
* Run config generation pipeline and validate.

**Dependencies:** mainnet deployment gate

---

### Commit 22: `docs(tron): document TRON V1 swaps, address mapping, activation, deployment, and testing`

- [ ] **Done**

**Repo:** `docs-repo`

**Description**
Document:
* V1-only scope
* Activation shape (ETH-family endpoint)
* `swap_contract_address` is Base58 in config, auto-converted to H160 at activation
* How to map Base58 ↔ H160 for TronScan
* Runtime uses native TRON HTTP APIs
* Nile test commands
* Deployment and verification checklist
* Watcher-reward V1 paths unsupported on TRON
* SHA-256 secretHash (bytes32) difference from EVM V1

**Dependencies:** 21

---

# Extra Implementation Notes

1. **Do not use Ethereum JSON-RPC compatibility mode for swap logic.** Use TRON HTTP APIs.
2. **Do not let `eth.rs` grow another TRON subsystem.** Dispatch there, implement in `tron/*.rs`.
3. **Use explicit chain matches everywhere.** No `_ =>` wildcard on `ChainFamily`.
4. **Do not change the activation request schema** unless absolutely necessary.
5. **Reject watcher reward paths explicitly on TRON V1.**
6. **Use indexed event service for discovery; receipt logs for confirmation.**
7. **Keep TRON USDT conservative.** Implement zero-first approval fallback.
8. **Never hardcode energy price.** Query at runtime.
9. **Keep CREATE2 out of MVP.** Deploy and record concrete addresses.
10. **Use `TriggerSmartContract` with `call_value` for TRX contract payments**, not plain transfers.

---

# Post-MVP Follow-Up

* `TransactionEnum::TronTransaction` + focused TRON modules + exhaustive `ChainFamily` dispatch.
* Follow-up plan for the chain-rpc refactor.
* The EVM ERC20 amount-check fix (commit 14) is the only cross-cutting fix.
