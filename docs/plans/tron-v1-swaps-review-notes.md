# TRON V1 Swaps Plan — Review Notes & GPT-PRO Follow-up Questions

> This file captures feedback and research from the step-by-step plan review.
> Each commit has: the original proposal, research findings, user decision, and what to ask GPT-PRO.

---

## EVM Validator Gap (Pre-existing Bug)

GPT-PRO identified a real bug: in `eth.rs:~5683`, the non-watcher ERC20 `validate_payment` path never validates `decoded[1]` (token amount) against `trade_amount`. The amount check only runs inside `if let Some(watcher_reward)`. All other fields (swap_id, token_addr, receiver, secret_hash, timelock) are always checked.

**Decision**: GPT-PRO suggested keeping this out of the TRON PR unless split into a tiny dedicated micro-fix. Needs user confirmation when we reach commit 14 (TRON validation).

---

## Commit 1: TRON V1 Swap Contract

**Original proposal**: Copy EtomicSwap.sol, add `_tronRipemd160` helper using `staticcall(0x20003)`, replace all `ripemd160()` calls.

**Research findings**:
- **Every known TRON HTLC** (Jelly Swap, MovNetwork, reference impls) uses `sha256` — none use ripemd160
- KDF's own `secret_hash_algo_v2()` already returns `SecretHashAlgo::SHA256` for `EthCoinVariant` (which includes TRON) at `lp_coins.rs:3970`
- etomic-swap PRs #24 and #38 already proved sha256 secret hashing works; V2 contracts use sha256 for secret hash
- The `staticcall(0x20003)` hack is non-standard, fragile, used by zero production TRON contracts
- Jelly Swap created a **separate** TRON HTLC contract (not reused EVM) — [jelly-tron-htlc](https://github.com/jelly-swap/jelly-tron-htlc)
- The `sha256_secret_hash_backup` branch in etomic-swap has the V1 contract with dual `SecretHashAlgo` support (`Dhash160` and `Sha256`)
- sha256 means `bytes32 secretHash` instead of `bytes20` — different ABI from EVM V1

**User decision**: Let GPT-PRO decide between sha256 vs ripemd160 via 0x20003.

**What to ask GPT-PRO**:
> Given that (1) every known TRON HTLC uses sha256, (2) KDF already maps TRON to `SecretHashAlgo::SHA256`, (3) etomic-swap V2 contracts already use sha256 for secret hashing, (4) the `staticcall(0x20003)` approach is non-standard and used by zero production TRON contracts, and (5) the `sha256_secret_hash_backup` branch in etomic-swap already has V1 contract code supporting both hash algorithms — should the TRON contract use sha256 (simpler, native, industry standard, but different ABI with bytes32 secretHash) or ripemd160 via 0x20003 (maintains V1 ABI compat but fragile and unprecedented)? Please do your own research on this and make a final recommendation with justification.

---

## Commit 2: Compile & Deploy Tooling

**Original proposal**: Add TronBox for compilation and deployment.

**Research findings**:
- etomic-swap currently uses **Hardhat v2.22.18** with Solidity 0.8.33
- `@layerzerolabs/hardhat-tron` plugin can compile for TVM but **cannot deploy** to TRON (TRON /jsonrpc is read-only for tx sending)
- TronBox is official (v4.5.0, active), but is a Truffle fork — separate toolchain
- Best hybrid: Hardhat compile (consistency) + TronWeb.js deploy scripts
- Alternative: TronBox for everything TRON-specific

| | TronBox | Hardhat + plugin |
|---|---|---|
| Compile for TVM | Native | Via `@layerzerolabs/hardhat-tron` |
| Deploy to TRON | Native | **Cannot** — needs TronWeb scripts |
| Testing | Native (TronWeb) | Read-only via /jsonrpc |
| Status | Official, v4.5.0, active | Third-party, niche |

**User decision**: Let GPT-PRO decide.

**What to ask GPT-PRO**:
> etomic-swap uses Hardhat v2.22.18. The `@layerzerolabs/hardhat-tron` plugin can compile for TVM but cannot deploy. TronBox is official and can do everything but adds a second toolchain. Should we (a) use Hardhat for compile + TronWeb.js scripts for deploy (one toolchain, two deploy targets), or (b) add TronBox alongside Hardhat for TRON-specific work? Please research current best practices and make a recommendation.

---

## Commit 3: Nile Deployment Metadata

**Original proposal**: Record deployment metadata in `deployments/nile/EtomicSwapTron.json` in etomic-swap repo.

**Research findings**:
- etomic-swap has **NO `deployments/` directory** for any chain
- No structured deployment metadata storage exists in etomic-swap
- Contract addresses are managed in KDF codebase (test helpers, coin configs) and the coins repo
- `artifacts/` has Hardhat compilation output only
- `ignition/modules/` is scaffolded but empty

**User decision**: Let GPT-PRO decide.

**What to ask GPT-PRO**:
> etomic-swap has no `deployments/` directory and no deployment metadata for any chain — addresses live in KDF test constants and coins repo configs. Should we (a) follow existing pattern and put the Nile contract address only in KDF test helpers + coins configs, or (b) start a new `deployments/` pattern in etomic-swap for all chains going forward? Please consider what's most practical and consistent.

---

## Commit 4: TronSignedTransaction & TransactionEnum

**Original proposal**: Add `TronSignedTransaction` in `tron/transaction.rs`, add `TransactionEnum::TronTransaction` variant.

**Research findings — code surface comparison**:

| Metric | Approach A (separate variant) | Approach B (wrapper enum) |
|--------|------|------|
| Files to modify | 2 (lp_coins.rs, eth.rs) | 5+ |
| Edits needed | ~6-8 | ~20-25 |
| Breaking changes | None (additive) | Yes |
| Construction sites affected | 0 (uses From macro) | 11+ in eth.rs |
| Test changes | Minimal | Significant |

**Approach A** adds `TronTransaction(TronSignedTransaction)` alongside `SignedEthTx` — matches how Sia/Cosmos were added.

**Approach B** creates `EthFamilyTransaction { Eth(SignedEthTx), Tron(TronSignedTransaction) }` wrapper, replaces `SignedEthTx` variant — more structured but breaks all existing match sites.

Key match sites: `Deref` impl (lp_coins.rs:647), `validate_fee` (eth.rs:1629), `ifrom!` macros (lp_coins.rs:622-627).

**User decision**: Let GPT-PRO decide.

**What to ask GPT-PRO**:
> For adding TRON transaction support to `TransactionEnum`: Approach A adds a separate `TronTransaction` variant (additive, ~6-8 edits, 2 files, matches Sia/Cosmos pattern). Approach B creates an `EthFamilyTransaction` wrapper enum grouping EVM+TRON under one variant (~20-25 edits, 5+ files, breaking change to 11+ construction sites). Both are documented with full code surface analysis. Please evaluate and decide which is better for maintainability and the planned chain-rpc refactor.

---

## Commit 5: Refactor Swap Helpers to Chain-Generic

**Original proposal**: Rename `sign_transaction_with_keypair` → `sign_evm_transaction_with_keypair`, make V1 swap helpers return `TransactionEnum`, add TRON stubs.

**User decision**: Agreed — follows whichever approach is chosen for Commit 4.

---

## Commit 6: Generic TriggerSmartContract Builder

**User decision**: Agreed as proposed.

---

## Commit 7: Extend API Client

**User decision**: Agreed as proposed.

---

## Commit 8: Contract-Call Signing & Fee-Limit Estimation

**User decision**: Agreed as proposed.

---

## Commit 9: Normalize Events & Receipt Logs

**User decision**: Agreed as proposed.

---

## Key Architectural Finding: Zero Upstream Changes Needed

**Research confirmed**: All swap operations flow through trait generics (MakerCoinSwapOpsV2, TakerCoinSwapOpsV2, etc.). TRON is already `MmCoinEnum::EthCoinVariant` → same kickstart handler, same state machine. `ValidateFeeArgs.fee_tx` uses `&TransactionEnum` (coin-agnostic). **ALL TRON swap changes stay inside EthCoin methods via ChainFamily dispatch. NO swap framework code needs modification.**

This principle applies to ALL commits 10–17.

---

## Commit 10: Taker-Fee Send & Validation

**User decision**: Agreed. All changes inside EthCoin via ChainFamily dispatch. No upstream swap framework changes.

---

## Commit 11: Orderbook Address Derivation

**User decision**: Agreed as proposed.

---

## Commit 12: Payment Status Reads & Recovery Helpers

**User decision**: Agreed. Same ChainFamily dispatch principle as commit 10.

---

## Commit 13: Maker/Taker Payment Send

**User decision**: Agreed. Same ChainFamily dispatch principle as commit 10.

---

## Commit 14: Payment Validation + EVM Bug Fix

**Original proposal**: Implement TRON payment validation (protobuf decode, validate all fields, cross-check on-chain).

**EVM bug**: Non-watcher ERC20 path in `validate_payment` (eth.rs:~5683) skips `decoded[1]` (amount) validation.

**User decision**: Fix EVM bug **alongside** TRON validation in the same commit (not separately).

---

## Commit 15: Spend & Refund Transaction Sends

**User decision**: Agreed. Same ChainFamily dispatch principle — all inside EthCoin methods.

---

## Commit 16: Spend/Refund Discovery & Secret Extraction

**User decision**: Agreed. Same ChainFamily dispatch principle.

---

## Commit 17: Trade-Fee Estimation

**User decision**: Agreed, but with important note:

**What to ask GPT-PRO**:
> Trade fee estimation might use V2 methods (not just legacy V1). The GUI may use V2 preimage methods. GPT-PRO should check which trade fee estimation methods are actually used by the GUI and ensure both V1 and V2 paths are covered for TRON if needed.

---

## Commit 18: Swap-Enabled Test Helpers

**User decision**: Agreed as proposed.

---

## Commit 19 & 20: Testing Strategy (REVISED)

**Original proposal**: Batch all unit tests in commit 19, all Nile tests in commit 20.

**User feedback**: Tests should be added **per commit** — each functional commit should include its own unit tests and integration tests to validate correctness before moving to the next commit. Keep commits 19/20 as catch-all for remaining coverage gaps and full end-to-end Nile swap flows.

**What to ask GPT-PRO**:
> Restructure the plan so each functional commit (4–17) includes its own unit tests and relevant integration tests. Commits 19/20 should remain as catch-all for full end-to-end Nile swap flow tests and any remaining coverage gaps not covered by per-commit tests.

---

## Commit 21: Coins Repo Config

**Original proposal**: Add swap_contract_address in H160 hex form.

**Research findings**:
- KDF's `EthActivationV2Request` expects `swap_contract_address: Option<Address>` where `Address = H160`
- All existing swap contracts use `0x` + 40 hex chars format
- TRON addresses have two forms: base58 (`T...` on TronScan) and hex (`41...` raw)
- To convert: strip the `41` prefix from TRON hex → get 20-byte H160 → format as `0x...`
- No TRON entries exist yet in the coins repo (only `TRX-BEP20` which is a BSC token)

**User concern**: How do we find the contract on TronScan if we store H160?

**Answer**: Both representations map to the same 20-byte address. TronScan shows base58 (`T...`), KDF config stores `0x` H160. The docs should document the mapping.

**What to ask GPT-PRO**:
> The coins config must use `0x` H160 hex format (that's what `EthActivationV2Request` deserializes). But TronScan displays base58 (`T...`). Should we store both forms in config comments or documentation? How should we handle the dual representation so operators can easily verify contracts on TronScan?

---

## Commit 22: Documentation

**User decision**: Agreed as proposed.

---

## Summary of All GPT-PRO Decision Points

1. **Hash algorithm**: sha256 vs ripemd160 via 0x20003 for TRON contract (Commit 1)
2. **Tooling**: Hardhat+TronWeb vs TronBox for TRON compile/deploy (Commit 2)
3. **Metadata storage**: KDF/coins only vs new deployments/ dir in etomic-swap (Commit 3)
4. **TransactionEnum approach**: Separate variant (Approach A) vs wrapper enum (Approach B) (Commit 4)
5. **Trade fee V1/V2**: Check which estimation methods GUI uses, cover both if needed (Commit 17)
6. **Test restructuring**: Per-commit tests + catch-all commits 19/20 (Commits 4–20)
7. **Address dual representation**: How to document H160 vs base58 for TronScan (Commit 21)
