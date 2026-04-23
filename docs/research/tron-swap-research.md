# TRON Swap Support Research

> **Date**: 2026-03-13
> **Purpose**: Research for implementing HTLC atomic swap support for TRX and TRC20 tokens in KDF
> **Branch**: `feat/tron-swap-support`
> **Related**: Issue #1542, PRs #2425/#2467/#2712/#2714 (wallet-only complete)

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [TVM vs EVM Differences](#2-tvm-vs-evm-differences)
3. [Etomic Swap Contract Analysis](#3-etomic-swap-contract-analysis)
4. [Contract Compatibility Issues](#4-contract-compatibility-issues)
5. [Existing TRON HTLC Implementations](#5-existing-tron-htlc-implementations)
6. [DEX Landscape on TRON](#6-dex-landscape-on-tron)
7. [Energy & Bandwidth Costs](#7-energy--bandwidth-costs)
8. [KDF Code Changes Required](#8-kdf-code-changes-required)
9. [KDF GitHub Issues](#9-kdf-github-issues)
10. [Recommended Approach](#10-recommended-approach)
11. [Open Questions](#11-open-questions)
12. [Sources](#12-sources)

---

## 1. Executive Summary

TRON swap support requires two parallel workstreams:

1. **Smart Contract (etomic-swap repo)**: Adapt and deploy the existing EtomicSwap Solidity contracts on TVM. The contracts are *largely* compatible but have **two critical issues**:
   - **RIPEMD-160 precompile is broken on TVM** (address 0x03 computes double-SHA256 instead). V1 contract uses `ripemd160()` for payment hash verification.
   - **No `eth_getLogs` equivalent** on TRON full nodes. Event monitoring needs a different approach.

2. **KDF (this repo)**: Add chain-family dispatches throughout the swap pipeline — transaction building (Protobuf vs RLP), signing (SHA256 vs Keccak256), broadcasting, event querying, fee estimation, and orderbook address generation.

**Key conclusion**: The V2 contracts should be the target for TRON. They can be recompiled with TRON's solc (which correctly maps `ripemd160()` to TVM's `0x20003` precompile). The ABI encoding, `keccak256`, `msg.value`/`payable`, and ERC20/TRC20 `approve`/`transferFrom` patterns are all compatible. No `ecrecover` is used in the contracts.

---

## 2. TVM vs EVM Differences

### 2.1 Compatible (No Changes Needed in Contracts)

| Feature | Details |
|---------|---------|
| **Solidity support** | TVM supports Solidity up to 0.8.x via TRON's forked `solc` compiler |
| **keccak256** | Identical behavior to EVM |
| **sha256** | Identical behavior to EVM |
| **ecrecover** | Works at precompile 0x01, returns 20-byte address (no 0x41 prefix inside TVM) |
| **msg.value** | Works in payable functions, denominated in SUN (1 TRX = 1e6 SUN) |
| **ABI encoding** | Identical to EVM. Addresses are 20 bytes inside TVM (no 0x41 prefix) |
| **msg.sender** | Returns 20-byte address (no 0x41 prefix), same as EVM |
| **address(this)** | Returns 20-byte address (no 0x41 prefix) |
| **block.timestamp** | Returns seconds (Unix epoch) inside Solidity, same as EVM |
| **ERC20/TRC20 interface** | `approve`, `transfer`, `transferFrom` are identical |
| **SafeERC20 pattern** | Works on TVM for non-standard tokens like USDT |
| **Reentrancy** | Same vulnerability model; use checks-effects-interactions + ReentrancyGuard |

### 2.2 Different (Requires Attention)

| Feature | EVM | TVM | Impact |
|---------|-----|-----|--------|
| **RIPEMD-160 (0x03)** | Standard RIPEMD-160 | **Double-SHA256** | **CRITICAL** — V1 contract hash mismatch |
| **Address prefix** | None (20 bytes) | 0x41 prefix (21 bytes) externally; 20 bytes inside TVM | Off-chain code must strip/add prefix |
| **CREATE2 prefix** | 0xff | 0x41 (at protocol level; solc emulates 0xff for `new{salt}`) | Manual CREATE2 address computation needs adjustment |
| **Decimals** | 18 (wei) | 6 (sun) | Value calculations, dust thresholds |
| **Block interval** | ~12 seconds | ~3 seconds | Timelock granularity (4x more blocks per hour) |
| **Gas model** | Gas with EIP-1559 | Energy + Bandwidth | Fee estimation completely different |
| **GASPRICE opcode** | Returns gas price | Returns 0 (or energyPrice with compat flag) | Contract logic dependent on gas price breaks |
| **BASEFEE opcode** | Returns base fee | Returns energyPrice | Same concern |
| **DIFFICULTY opcode** | Returns difficulty | Returns 0 | Not used in HTLC contracts |
| **Plain transfers** | Trigger fallback/receive | **Do NOT trigger fallback/receive** | Must use TriggerSmartContract |
| **Transaction encoding** | RLP | Protobuf | All tx serialization/deserialization changes |
| **Tx signing hash** | Keccak256 | SHA256 | Already handled in KDF's `tron/sign.rs` |
| **No EIP-155** | chain_id in sig | No chain_id | Already handled in KDF |
| **API timestamps** | Seconds | **Milliseconds** | Divide by 1000 when comparing with contract timelocks |
| **Event querying** | `eth_getLogs` RPC | Separate event API / TronGrid | Core swap monitoring pattern changes |
| **Blake2F (0x09)** | Blake2F compression | **BatchValidateSign** | Not used in HTLC contracts |
| **ecrecover message prefix** | `"\x19Ethereum Signed Message:\n"` | `"\x19TRON Signed Message:\n"` | Only matters for off-chain signed messages, not HTLC |
| **Call depth limit** | 1024 (effective ~340 with 63/64 rule) | **64** | Not a concern for HTLC (1-2 levels) |
| **Contract compiler** | Standard `solc` | TRON's forked `solc` (`tronsolc`) | Must recompile, cannot deploy EVM bytecode directly |

### 2.3 TRON-Specific Opcodes (Not in EVM)

- `CALLTOKEN (0xd0)`, `TOKENBALANCE (0xd1)`, `CALLTOKENVALUE (0xd2)`, `CALLTOKENID (0xd3)` — TRC-10 native tokens
- `ISCONTRACT (0xd4)` — Check if address is a contract
- `FREEZE`, `UNFREEZE`, `FREEZEEXPIRETIME` — Stake V1
- `FREEZEBALANCEV2`, `UNFREEZEBALANCEV2` — Stake V2
- `VOTEWITNESS`, `WITHDRAWREWARD` — Governance
- `TSTORE`, `TLOAD` — Transient storage (TIP-650, mirrors EIP-1153)

None of these are needed for HTLC contracts.

### 2.4 TIP-712 (TRON's EIP-712)

TRON adapts EIP-712 with:
- Address encoding: strip 0x41 prefix, encode remaining 20 bytes as `uint160`
- `trcToken` type: treated as atomic, encoded as `uint256`
- chainId: `block.chainid & 0xffffffff` — Mainnet: `0x2b6653dc` (728126428), Nile: `0xcd8690dc` (3448148188)

Not directly needed for HTLC but relevant if V2 contracts ever use typed data signing.

---

## 3. Etomic Swap Contract Analysis

### 3.1 V1 Contract (`EtomicSwap.sol`)

Single contract for both maker and taker:

| Function | Purpose | Hashing |
|----------|---------|---------|
| `ethPayment(id, receiver, secretHash, lockTime)` | Lock ETH in HTLC | `ripemd160(receiver, sender, secretHash, tokenAddress, amount)` |
| `erc20Payment(id, amount, tokenAddress, receiver, secretHash, lockTime)` | Lock ERC20 | Same |
| `receiverSpend(id, amount, secret, tokenAddress, sender)` | Claim with secret | Verifies `ripemd160(secret) == secretHash` |
| `senderRefund(id, amount, secretHash, tokenAddress, receiver)` | Refund after timeout | Timelock check |
| `payments(id)` | Query state | Returns `(paymentHash, lockTime, state)` |

**V1 uses `ripemd160()` — BROKEN on TVM without recompilation.**

### 3.2 V2 Contracts (Separate Maker/Taker)

**Maker V2 (`EtomicSwapMakerV2`)**:
- `ethMakerPayment(id, taker, takerSecretHash, makerSecretHash, paymentLockTime)`
- `erc20MakerPayment(id, amount, tokenAddress, taker, takerSecretHash, makerSecretHash, paymentLockTime)`
- `spendMakerPayment(id, amount, maker, takerSecretHash, makerSecret, tokenAddress)`
- `refundMakerPaymentTimelock(...)` / `refundMakerPaymentSecret(...)`
- Uses **dual secret hashes** (maker + taker)
- `paymentLockTime` is `uint32`

**Taker V2 (`EtomicSwapTakerV2`)**:
- Has `dexFeeAddress` set in constructor
- `ethTakerPayment(id, dexFee, receiver, takerSecretHash, makerSecretHash, preApproveLockTime, paymentLockTime)`
- `erc20TakerPayment(id, amount, dexFee, tokenAddress, receiver, takerSecretHash, makerSecretHash, preApproveLockTime, paymentLockTime)`
- `takerPaymentApprove(...)` — extra approval step
- Two timelocks: `preApproveLockTime` + `paymentLockTime`
- Integrates DEX fee directly

**Key V2 properties**:
- No `ecrecover` — no on-chain signature verification
- Uses `keccak256` for payment hash computation (compatible with TVM)
- Uses `ripemd160` for secret hash verification — needs recompilation with TRON solc
- Standard `payable` + `msg.value` for native token
- Standard `transferFrom` for ERC20/TRC20

### 3.3 Contract ABIs in KDF

| File | Contract |
|------|----------|
| `mm2src/coins/eth/swap_contract_abi.json` | V1 EtomicSwap |
| `mm2src/coins/eth/maker_swap_v2_abi.json` | V2 Maker |
| `mm2src/coins/eth/taker_swap_v2_abi.json` | V2 Taker |
| `mm2src/coins/eth/nft_swap_contract_abi.json` | V1 NFT |
| `mm2src/coins/eth/nft_maker_swap_v2_abi.json` | V2 NFT Maker |

---

## 4. Contract Compatibility Issues

### 4.1 RIPEMD-160 — The Critical Issue

**Problem**: TVM's precompile at address `0x03` computes `SHA256(SHA256(data))` truncated to 20 bytes, NOT RIPEMD-160. Standard RIPEMD-160 is at `0x20003` on TVM.

**Impact on etomic swap contracts**:
- V1: `ripemd160()` is used for `paymentHash` computation and secret hash verification
- V2: Also uses `ripemd160()` for secret hash verification (needs source code confirmation)

**Solutions**:
1. **~~Recompile with TRON's solc~~** — ⚠️ **UPDATE (Round 2 research)**: Evidence suggests TRON's solc does NOT automatically remap `ripemd160()` to `0x20003`. The Solidity builtin compiles to a `staticcall` to `0x03` regardless of compiler. TRON's migration docs explicitly say: *"If your smart contract calls any of these addresses, you must modify its logic."* This means recompilation alone is **insufficient**.
2. **Modify contract source** — Replace `ripemd160()` calls with inline assembly `staticcall` to `0x20003`:
   ```solidity
   function ripemd160_tron(bytes memory data) internal view returns (bytes20) {
       (bool ok, bytes memory result) = address(0x20003).staticcall(data);
       require(ok);
       return bytes20(abi.decode(result, (bytes32)) << 96);
   }
   ```
3. **Replace with keccak256** — Modify the contract to use `keccak256` instead of `ripemd160` for the payment hash. This changes the hash size from 20 to 32 bytes.
4. **Use sha256** — Another option, also 32 bytes output.

**Recommendation**: Modify the V1 contract source to use inline assembly `staticcall(0x20003)` for RIPEMD-160, then compile with TRON's solc. This preserves compatibility with the existing secret hash format (20-byte RIPEMD-160) while using the correct TVM precompile. Alternatively, switch to `keccak256` for TRON-specific contracts.

### 4.2 Event Log Querying

**Problem**: TRON full nodes do not support `eth_getLogs` RPC. The KDF swap pipeline relies heavily on `eth_getLogs` for:
- Finding payment transactions by event
- Searching for swap tx spends
- Monitoring payment states

**TRON alternatives**:
- **TronGrid Event API**: `GET /v1/contracts/{address}/events` — paginated, supports filtering by `event_name`, `block_number`, `only_confirmed`. ⚠️ **TronGrid-only** — NOT available on raw java-tron full nodes.
- **Transaction info API**: `/wallet/gettransactioninfobyid` — returns raw event logs (same format as Ethereum receipts: `address`, `topics[]`, `data`) for a specific tx. Available on ALL full nodes.
- **JSON-RPC `eth_getLogs`**: TRON does support this via JSON-RPC compatibility layer, but user preference is HTTP RPC.

**Recommendation**: Primary approach: Use `/v1/contracts/{address}/events` (TronGrid HTTP API) for event scanning. Fallback: Use `/wallet/gettransactioninfobyid` for per-tx event extraction (works on all nodes). The TronGrid event API supports `event_name`, `block_number`, `only_confirmed`, `min_block_timestamp`/`max_block_timestamp`, pagination via `fingerprint` cursor. Results include decoded parameters in `result` field with `result_type` for type info.

### 4.3 Transaction Deserialization (RLP vs Protobuf)

**Problem**: Every swap method that receives `payment_tx` as bytes does `rlp::decode` to extract transaction data (sender, calldata, value). At least 15+ call sites in `eth.rs`.

**Solution**: Chain-family dispatch at each deserialization point. For TRON, decode Protobuf `Transaction` instead of RLP `SignedEthTx`. Extract equivalent fields:
- `sender` → derive from signature + txID
- `data` → `TriggerSmartContract.data` field
- `value` → `TriggerSmartContract.call_value` or `TransferContract.amount`

### 4.4 Nonce vs TAPOS

**Problem**: EVM swap code uses sequential nonces (`get_addr_nonce`, `wait_for_addr_nonce_increase`). TRON uses TAPOS (Transaction as Proof of Stake) with block references instead.

**Solution**: Skip nonce logic for TRON. TAPOS is already handled in KDF's `tron/tx_builder.rs`.

---

## 5. Existing TRON HTLC Implementations

### 5.1 JellySwap (`jelly-swap/jelly-tron-htlc`)
- **The only known dedicated TRON HTLC contract**
- Audited by ABDK Consulting (primarily the ETH version)
- Part of a cross-chain atomic swap protocol (Bitcoin, Ethereum, Aeternity, TRON)
- Project appears largely inactive now
- **Reference**: https://github.com/jelly-swap/jelly-tron-htlc

### 5.2 Academic: `creatloper/AssetExchange`
- Peer-reviewed paper in Springer's *Cluster Computing* journal
- Implemented HTLC atomic swaps between Ethereum and TRON
- Validated time-lock equations for both chains
- **Reference**: https://github.com/creatloper/AssetExchange

### 5.3 Atomix (Hackathon)
- TRON DAO Hackathon Season 7 project
- HTLC-based atomic swaps across chains
- Hackathon-quality, not production-ready

### 5.4 THORChain (Different Approach)
- Uses TSS (Threshold Signature Scheme) vaults, NOT HTLCs
- Completed TRON integration for native TRX and USDT-TRC20 swaps
- Not directly applicable to KDF's HTLC approach but validates market demand

---

## 6. DEX Landscape on TRON

### 6.1 SunSwap (Dominant AMM DEX)
- Fork of Uniswap V2/V3
- Uses WTRX (Wrapped TRX) pattern identical to WETH
- WTRX address: `TNUC9Qb1rRpS5CbWLmNMxXBjyFoydXjWFR`
- Contracts are direct Solidity ports, confirming TVM compatibility for complex DeFi contracts
- V3 has concentrated liquidity (Uniswap V3 fork)

### 6.2 Other TRON DEXes
- **Poloni DEX** (formerly TRXMarket): On-chain order book, TRC-10/TRC-20 only, no cross-chain
- **SUN.io Smart Router**: Meta-router across V1/V1.5/V2/V3/PSM/Curve pools

### 6.3 Cross-Chain Bridges
| Bridge | Mechanism | Uses HTLC? |
|--------|-----------|------------|
| BTTC (BitTorrent Chain) | PoS validator lock/mint/burn | No |
| Multichain/AnySwap | MPC swapin/swapout | No |
| THORChain | TSS vault + CLP pools | No |
| Symbiosis | Cross-chain AMM | No |
| 1inch Fusion+ | Escrow with secret hash + timelock | Yes (HTLC-like) |

### 6.4 Key Takeaway
- No production DEX currently offers HTLC-based atomic swaps for TRON
- KDF would be among the first
- SunSwap's successful Uniswap fork confirms complex Solidity contracts work on TVM

---

## 7. Energy & Bandwidth Costs

### 7.1 Current Pricing (as of 2026-03)
- **Energy unit price**: 100 SUN (reduced from 210 SUN in August 2025 via Proposal #104)
- **Bandwidth price**: 1000 SUN per bandwidth point
- **Energy rental market**: ~34 SUN/energy (hourly rentals via third-party providers)

### 7.2 Estimated HTLC Costs

| Operation | Est. Energy | Est. Cost (TRX) | Notes |
|-----------|-------------|------------------|-------|
| Create HTLC (TRX) | 80,000-150,000 | 8-15 TRX | Multiple SSTORE ops |
| Create HTLC (TRC20) | 100,000-200,000 | 10-20 TRX | + approve + transferFrom |
| Claim/Spend | 50,000-80,000 | 5-8 TRX | Hash verify + SSTORE + transfer |
| Refund | 50,000-80,000 | 5-8 TRX | Timelock check + SSTORE + transfer |
| TRC20 transfer (reference) | 32,000-65,000 | 3.2-6.5 TRX | Existing recipient |
| TRC20 transfer (new recipient) | up to 130,000 | up to 13 TRX | Fresh storage slot |

### 7.3 Fee Model Details
- **fee_limit**: Max TRX willing to burn. Range: 0 to 15,000 TRX (network parameter #47)
- **Failed transactions**: Energy is still consumed (TRX burned), funds not deducted
- **Energy sources (priority)**: Staked energy → Developer contribution → TRX burn
- **Free bandwidth**: 600 points/day (was 5000, reduced via governance)

### 7.4 Implications for KDF
- Fee estimation before submission is critical (failed txs still burn TRX)
- Use `estimateEnergy` API or `triggerConstantContract` for pre-flight checks
- fee_limit of 50-100 TRX provides ample room for HTLC operations
- Consider energy rental integration for power users (future enhancement)

---

## 8. KDF Code Changes Required

### 8.1 Layer 1 — Orderbook Address Generation (Small, Prerequisite)

**File**: `mm2src/mm2_main/src/lp_ordermatch.rs:6675`

Current stub:
```rust
CoinProtocol::TRX { .. } | CoinProtocol::TRC20 { .. } => {
    MmError::err(OrderbookAddrErr::CoinIsNotSupported(coin.to_owned()))
}
```

Implement TRON pubkey → address derivation for orderbook.

### 8.2 Layer 2 — Transaction Building for Contract Calls

Build `TriggerSmartContract` protobuf messages for swap contract interactions:
- `call_value` for TRX payments
- `data` field for ABI-encoded function calls
- `fee_limit` for energy cap
- Extend existing `tron/tx_builder.rs` with contract call builders

### 8.3 Layer 3 — Transaction Serialization Dispatch

Every swap method doing `rlp::decode(payment_tx_bytes)` needs chain-family dispatch:
- EVM: `rlp::decode` → `SignedEthTx`
- TRON: `prost::Message::decode` → `Transaction` (protobuf)

Extract equivalent fields from TRON tx:
- `sender` → derive from signature + txID (or store separately)
- `data` → `TriggerSmartContract.data`
- `value` → `TriggerSmartContract.call_value`

### 8.4 Layer 4 — Contract State Queries

Replace `call_request` (web3) with `triggerConstantContract` (TRON API) for:
- `payments(id)` / `makerPayments(id)` / `takerPayments(id)` state queries
- Allowance checks
- Balance queries (already implemented)

### 8.5 Layer 5 — Event Log Scanning

Replace `eth_getLogs` with TRON event API for:
- `find_transaction_hash_by_event` — searching for swap tx by event
- `search_for_swap_tx_spend` — monitoring for spend/refund events
- Payment confirmation tracking

### 8.6 Layer 6 — Fee Estimation for Swaps

Replace gas estimation with TRON energy/bandwidth estimation:
- `get_sender_trade_fee` / `get_receiver_trade_fee` using energy estimates
- `get_fee_to_send_taker_fee` using bandwidth + energy
- Fee details in `TxFeeDetails::Tron` format (already exists)

### 8.7 Layer 7 — Sign and Send for Contract Calls

**File**: `mm2src/coins/eth.rs:3077`

Current stub:
```rust
// Todo: Add Tron signing logic
ChainSpec::Tron { .. } => Err(TransactionErr::Plain("Tron is not supported for sign_transaction_with_keypair yet"))
```

Wire TRON signing (already in `tron/sign.rs`) into the contract call path:
1. Build `TriggerSmartContract` protobuf
2. Sign with SHA256 + secp256k1
3. Broadcast via `broadcast_hex`

### 8.8 Layer 8 — Activation Changes

**File**: `mm2src/coins/eth/v2_activation.rs`

- Allow swap contract addresses for TRON coins (currently wallet-only)
- Add TRON swap contract address validation
- Remove `wallet_only` restriction when swap support is ready

### 8.9 Summary of Files to Modify

| File | Changes |
|------|---------|
| `mm2src/mm2_main/src/lp_ordermatch.rs` | Orderbook address generation for TRX/TRC20 |
| `mm2src/coins/eth.rs` | `sign_transaction_with_keypair`, swap method chain dispatches |
| `mm2src/coins/eth/tron/tx_builder.rs` | Add contract call transaction builders |
| `mm2src/coins/eth/tron/api.rs` | Add event query methods, contract state query methods |
| `mm2src/coins/eth/tron/proto.rs` | Possibly extend protobuf types |
| `mm2src/coins/eth/eth_swap_v2/eth_maker_swap_v2.rs` | Chain-family dispatches |
| `mm2src/coins/eth/eth_swap_v2/eth_taker_swap_v2.rs` | Chain-family dispatches |
| `mm2src/coins/eth/v2_activation.rs` | Allow swap contracts for TRON |
| `mm2src/coins/lp_coins.rs` | Trade fee trait implementations for TRON |

---

## 9. KDF GitHub Issues

### Issue #1542 — "Add TRON" (OPEN)
Primary tracking issue. Checklist includes:
- [ ] Tron-based DEX or aggregator integration (ref #1287)
- [ ] Full TRON client support (in progress)
- [ ] **Verify etomic swap secret extraction compatibility for TRON swaps**

### Issue #1951 — "Support basic transactions for coins without HTLC support" (OPEN)
Originally requested wallet-only support for TRON. Now implemented via PRs #2425/#2467/#2712/#2714.

### Completed PRs
| PR | Title | Merged |
|----|-------|--------|
| #2425 | Initial TRON groundwork | 2025-05-10 |
| #2467 | HD wallet activation | 2026-01-12 |
| #2712 | TRC20 token activation & balance | 2026-03-04 |
| #2714 | Tx signing, fees & withdrawals | 2026-03-06 |

---

## 10. Recommended Approach

### Phase 1: Contract Deployment (etomic-swap repo)

1. **Recompile V2 contracts** (Maker + Taker) with TRON's `solc` compiler
   - Verify `ripemd160()` maps to correct precompile (0x20003)
   - Verify all other precompiles work correctly
   - Run existing test suite against TVM (Nile testnet)
2. **Deploy to Nile testnet** for integration testing
3. **Audit/review** the compiled bytecode for correctness
4. **Deploy to TRON mainnet** once KDF integration is validated

### Phase 2: KDF Integration (this repo)

Suggested commit order:

1. **Orderbook address generation** — Small, unblocks ordermatching
2. **Contract call transaction builder** — Extend `tron/tx_builder.rs` for `TriggerSmartContract`
3. **Sign and send for contract calls** — Wire `sign_transaction_with_keypair` for TRON
4. **Contract state queries** — `payment_status` via `triggerConstantContract`
5. **Event log scanning** — TRON event API adapter
6. **V2 Maker swap methods** — `send_maker_payment_v2`, `validate_maker_payment_v2`, etc.
7. **V2 Taker swap methods** — `send_taker_funding`, `spend_taker_payment`, etc.
8. **Trade fee estimation** — `get_sender_trade_fee`, `get_receiver_trade_fee`
9. **Activation changes** — Allow swap contracts, remove wallet-only
10. **Integration tests** — End-to-end swap tests on Nile

### Phase 3: Downstream Repos

- **Coins repo**: Add swap contract addresses to TRX/TRC20 coin configs
- **Docs repo**: Document TRON swap support, fee model, limitations

---

## 11. Open Questions

1. **V1 or V2 contracts for TRON?** V2 is recommended (newer, better security model with dual secrets), but V1 may be needed for backward compatibility with existing swap partners.

2. **Secret hash algorithm**: Current contracts use `ripemd160(secret)` for the 20-byte secret hash. Should TRON contracts switch to `keccak256` to avoid any precompile ambiguity? This would make TRON swaps incompatible with EVM swaps using the same contract (different hash), but TRON swaps are already separate contracts.

3. **Event querying reliability**: TronGrid event API has rate limits and may not be available on all nodes. Should we implement fallback mechanisms? The `getTransactionInfoById` approach (polling by tx hash) is an alternative but less efficient for event scanning.

4. **Watcher node support**: The V1 contract has watcher-reward variants (`ethPaymentReward`, etc.). Should these be included in TRON deployment?

5. **fee_limit strategy**: Should KDF estimate energy precisely (via `estimateEnergy`/`triggerConstantContract`) and add a safety margin, or use a generous fixed fee_limit? Failed txs burn TRX, so underestimation is costly.

6. **TRON transaction finality**: TRON has ~3 second blocks with DPoS. How many confirmations should be required for swap safety? Solidified blocks are ~54 seconds behind head.

7. **Contract factory pattern**: Should we deploy one swap contract instance per TRON network, or use a factory pattern for per-swap contracts? The current EVM approach uses a single deployed contract with payment IDs.

---

## 12. Sources

### TRON Developer Documentation
- [TVM vs EVM Differences](https://developers.tron.network/v4.4.0/docs/vm-vs-evm)
- [Migrating Ethereum Smart Contracts to TRON](https://developers.tron.network/docs/migrating-eth-contracts-to-tron)
- [TRON Virtual Machine (TVM)](https://developers.tron.network/docs/tvm)
- [Smart Contract Deployment and Invocation](https://developers.tron.network/docs/smart-contract-deployment-and-invocation)
- [Energy Consumption Mechanism](https://developers.tron.network/v4.0/docs/energy-consumption)
- [Fee Limit on Deploy/Execution](https://developers.tron.network/v4.0/docs/setting-a-fee-limit-on-deployexecution)
- [Frozen Energy and Fee Limit Model](https://developers.tron.network/v4.4.0/docs/frozen-energy-and-fee-limit-model)
- [Parameter Encoding and Decoding](https://developers.tron.network/docs/parameter-encoding-and-decoding)
- [Contract Address in Solidity](https://developers.tron.network/v3.7/docs/contract-address-using-in-solidity-language)
- [TRON Accounts and Addresses](https://developers.tron.network/docs/account)
- [TRON Event API](https://developers.tron.network/docs/event)
- [Get Events by Contract Address](https://developers.tron.network/reference/get-events-by-contract-address)
- [EstimateEnergy API](https://developers.tron.network/reference/estimateenergy)
- [TRC-20 Contract Interaction](https://developers.tron.network/docs/trc20-contract-interaction)
- [TRON Opcodes Reference](https://developers.tron.network/docs/opcodes)
- [Smart Contract Security](https://developers.tron.network/docs/smart-contract-security)

### TRON Improvement Proposals
- [TIP-272: EVM Compatibility](https://github.com/tronprotocol/tips/issues/272)
- [TIP-544: Data Field for HTTP Contract Interfaces](https://github.com/tronprotocol/tips/issues/544)
- [TIP-650: Transient Storage Opcodes](https://github.com/tronprotocol/tips/issues/650)
- [TIP-652: SELFDESTRUCT Deprecation](https://github.com/tronprotocol/tips/issues/652)
- [TIP-712: Typed Structured Data Hashing](https://github.com/tronprotocol/tips/issues/712)
- [TIP-6780: SELFDESTRUCT Behavior Change](https://github.com/tronprotocol/tips/issues/765)

### Etomic Swap Contracts
- [artemii235/etomic-swap (GitHub)](https://github.com/artemii235/etomic-swap)
- [KomodoPlatform/etomic-swap (GitHub)](https://github.com/KomodoPlatform/etomic-swap)

### TRON HTLC Implementations
- [jelly-swap/jelly-tron-htlc (GitHub)](https://github.com/jelly-swap/jelly-tron-htlc)
- [JellySwap TRON Documentation](https://jellyswap.gitbook.io/jelly/tron)
- [Jelly Contracts Audit (ABDK)](https://medium.com/jelly-market/jelly-contracts-audit-70d3c3e27d8)
- [creatloper/AssetExchange (GitHub)](https://github.com/creatloper/AssetExchange)
- [HTLC Asset Exchange Paper (Springer)](https://link.springer.com/article/10.1007/s10586-022-03643-x)
- [chatch/hashed-timelock-contract-ethereum (GitHub)](https://github.com/chatch/hashed-timelock-contract-ethereum)

### DEX References
- [SunSwap V2 Contracts (GitHub)](https://github.com/sunswapteam/sunswap2.0-contracts)
- [SunSwap V3 Contracts (GitHub)](https://github.com/sunswapteam/SunSwap3.0-contracts)
- [SunSwap V3 Overview (docs)](https://docs.sun.io/developers/swap/sunswap-v3-overview)
- [Smart Exchange Router (GitHub)](https://github.com/sun-protocol/smart-exchange-router)

### Cross-Chain and Bridge References
- [THORChain TRON Integration](https://blog.thorchain.org/tron-integration-complete-native-trx-usdt-swaps-live-on-thorchain/)
- [1inch Fusion+ Protocol](https://1inch.network/fusion-plus-protocol)
- [1inch Cross-Chain Swap (GitHub)](https://github.com/1inch/cross-chain-swap)
- [BTTC Cross-Chain Bridge (docs)](https://doc.bt.io/docs/bridge/overview)

### java-tron References
- [java-tron (GitHub)](https://github.com/tronprotocol/java-tron)
- [TRON Solidity Compiler (GitHub)](https://github.com/tronprotocol/solc-bin)
- [Contract Size Limit Discussion](https://github.com/tronprotocol/java-tron/issues/3661)

### Fee and Cost References
- [TRON Energy Price Reduced to 100 SUN (2025)](https://www.rootdata.com/news/351174)
- [TRON Halves Network Fees (Coincub)](https://coincub.com/tron-halves-network-fees-what-the-record-breaking-cut-means-for-users-and-the-ecosystem/)
- [TRON Energy Calculator Guide 2026](https://blog.tronsave.io/2026-tron-energy-and-bandwidth-calculator-guide/)
- [Failed Transaction Fees (TronLink)](https://support.tronlink.org/hc/en-us/articles/17664699393049)

### Security References
- [PositiveSecurity TRON Audit Guide (GitHub)](https://github.com/PositiveSecurity/tron-audit-guide)
- [Security Guide for Smart Contracts (TRON)](https://medium.com/tronnetwork/security-guide-for-smart-contracts-87a7ed6f90f2)
- [Verifying ECDSA Signatures on TRON](https://medium.com/tronnetwork/verifying-elliptic-curve-digital-signature-with-tron-smart-contract-5d11347e7b5b)
- [ecrecover on TRON](https://copyprogramming.com/howto/how-to-get-correct-address-using-ecrecover-in-tron)

### KDF References
- [Komodo Atomic Swaps Using HTLCs](https://komodoplatform.com/en/academy/atomic-swaps-using-htlcs/)
- [Hardhat TRON Plugin](https://www.npmjs.com/package/@layerzerolabs/hardhat-tron)

---

## 13. Round 2 Research — Detailed API & Code Analysis

### 13.1 RIPEMD-160 Precompile — Updated Finding

**CRITICAL UPDATE**: Evidence strongly suggests TRON's solc does **NOT** automatically remap `ripemd160()` to `0x20003`.

- TVM precompile at `0x03` computes `sha256(sha256(data)[:20])` — NOT RIPEMD-160 ([java-tron issue #2272](https://github.com/tronprotocol/java-tron/issues/2272))
- Correct RIPEMD-160 is at `0x20003` (TIP-272)
- TRON migration docs: *"If your smart contract calls any of these addresses, you must modify its logic"*
- No evidence found that tron-solc modifies the `ripemd160()` builtin target address
- **Conclusion**: V1 contract source MUST be modified — either use inline assembly `staticcall(0x20003)` or switch hash algorithm

### 13.2 Secret Extraction — TRON Adaptation

`extract_secret()` in `eth.rs:1736-1782`:
1. Currently does `rlp::decode(spend_tx)` → `UnverifiedTransactionWrapper`
2. Extracts calldata from `unverified.unsigned().data()`
3. Matches function selector against `receiverSpend` / `receiverSpendReward`
4. ABI-decodes params, extracts `tokens[2]` (the secret, 32 bytes)

**For TRON**:
1. `prost::Message::decode(spend_tx)` → `Transaction` protobuf
2. Extract `raw_data.contract[0].parameter.value` → `TriggerSmartContract`
3. Use `tsc.data` as ABI-encoded calldata
4. Same function selector matching and ABI decoding (steps 3-4 are identical)

### 13.3 Contract Interaction APIs — Full Specification

| Endpoint | Purpose | Availability |
|----------|---------|--------------|
| `POST /wallet/triggersmartcontract` | Build unsigned state-changing tx | All nodes + TronGrid |
| `POST /wallet/triggerconstantcontract` | Read-only call + energy estimation | All nodes* + TronGrid |
| `POST /wallet/estimateenergy` | Accurate energy estimation | Not TronGrid, optional on nodes** |
| `POST /wallet/gettransactioninfobyid` | Tx receipt + raw event logs | All nodes + TronGrid |
| `POST /wallet/gettransactionbyid` | Raw transaction data | All nodes + TronGrid |
| `POST /wallet/getcontractinfo` | Contract metadata + runtime bytecode | All nodes + TronGrid |
| `GET /v1/contracts/{addr}/events` | Decoded events by contract | **TronGrid only** |
| `GET /v1/transactions/{txid}/events` | Decoded events by tx | **TronGrid only** |

\* Requires `vm.supportConstant = true`
\*\* Requires `vm.estimateEnergy = true` AND `vm.supportConstant = true`

**`triggersmartcontract` request format**:
```json
{
  "owner_address": "T...",
  "contract_address": "T...",
  "function_selector": "ethPayment(bytes32,address,bytes20,uint64)",
  "parameter": "<ABI-encoded params, hex, no 0x>",
  "fee_limit": 100000000,
  "call_value": 1000000,
  "visible": true
}
```

**`triggerconstantcontract` response** (for reading `payments(id)`):
```json
{
  "result": { "result": true },
  "constant_result": ["<ABI-encoded return value>"],
  "energy_used": 903
}
```

**`gettransactioninfobyid` response** (event logs):
```json
{
  "log": [{
    "address": "41...",
    "topics": ["<keccak256 event sig>", "<indexed param1>"],
    "data": "<ABI-encoded non-indexed params>"
  }],
  "receipt": {
    "result": "SUCCESS",
    "energy_usage_total": 14000,
    "energy_fee": 420000
  }
}
```

### 13.4 Event Scanning — TronGrid Event API Details

`GET /v1/contracts/{address}/events` query parameters:

| Parameter | Description |
|-----------|-------------|
| `event_name` | Filter by event name (e.g., `"PaymentSent"`) |
| `block_number` | Filter by specific block |
| `only_confirmed` | Only confirmed/solidified events |
| `min_block_timestamp` | Min block timestamp (milliseconds!) |
| `max_block_timestamp` | Max block timestamp (milliseconds!) |
| `limit` | Page size (max 200, default 20) |
| `fingerprint` | Pagination cursor from previous response |
| `order_by` | `block_timestamp,asc` or `block_timestamp,desc` |

Response `result` field contains **decoded** parameters (addresses in Base58, values as strings).

**No topic-level filtering** on the REST API — only `event_name` (effectively topic[0] by name). Must filter indexed params client-side.

### 13.5 Energy Estimation — Detailed Analysis

**`estimateenergy` vs `triggerconstantcontract`**:
- `estimateenergy` returns `energy_required` (includes Dynamic Energy Model penalty) — more accurate
- `triggerconstantcontract` returns `energy_used` (includes penalty in newer java-tron) + `energy_penalty` separately
- `estimateenergy` is NOT on TronGrid — must use `triggerconstantcontract` as primary

**Dynamic Energy Model (TIP-491)**:
- Hot contracts get penalized: `actual = base * (1 + energy_factor)`
- `max_factor = 1.2` (120%) — worst case is `base * 2.2`
- Query contract's `energy_factor` via `/wallet/getcontractinfo`
- Factor changes every ~6 hour maintenance period

**Safety margins**:
- `triggerconstantcontract`: Apply 1.2x-1.3x multiplier on `energy_used`
- `fee_limit` is a cap, not actual charge — being conservative is safe
- Failed txs still burn TRX, so underestimation is costly

**Current energy price**: 420 SUN/energy (was 100 SUN — updated from Round 1)

**KDF codebase already has**:
- `TriggerConstantContractRequest/Response` in `api.rs:503-548`
- `parse_chain_prices_sun()` in `api.rs:654-684` (extracts `getEnergyFee`, `getTransactionFee`)
- `estimate_bandwidth()` in `fee.rs:98-125` (with placeholder signature)
- Missing: `energy_penalty` deserialization in response struct

### 13.6 Trade Fee Methods — Gap Analysis

**SwapOps trait** (`lp_coins.rs:1141-1321`) — 34 methods total. Key fee methods:

| Method | EVM Gas | TRON Changes Needed |
|--------|---------|---------------------|
| `get_trade_fee()` | 150,000 gas × gas_price | Energy estimate × energy_price |
| `get_sender_trade_fee()` | 65K-165K gas (ETH) / 150K-300K (ERC20) | Energy via `triggerconstantcontract` |
| `get_receiver_trade_fee()` | 65K gas (ETH) / 150K (ERC20) | Energy estimate for spend |
| `get_fee_to_send_taker_fee()` | `estimate_gas_wrapper()` | Energy + bandwidth estimate |
| `validate_fee()` | Check sender, block, receiver, amount | Same but TronAddress format |

**FeeApproxStage multipliers** (same for TRON):
- `StartSwap`: +3%, `OrderIssue`: +5%, `TradePreimage`: +7%

**ERC20 approve gas**: EVM adds `estimate_gas_for_contract_call()` result for approve when allowance insufficient. TRON: same pattern using `triggerconstantcontract`.

### 13.7 Activation — Swap Contract Changes

Current flow in `v2_activation.rs:694-750`:
- `is_wallet_only_conf(conf)` reads `"wallet_only": true` from coin config
- When `wallet_only=true`: swap contracts accepted as-is (no validation)
- When `wallet_only=false`: V1 contract required + validated non-zero

**Changes needed**:
1. Set `"wallet_only": false` in TRON coin configs (coins repo)
2. Add `swap_contract_address` to TRX/TRC20 configs
3. No code changes needed in `resolve_swap_contracts()` — validation already generic

### 13.8 Etomic Swap Contract — V1 ABI & KDF Integration

**V1 contract functions** (from `swap_contract_abi.json`):
- `ethPayment(bytes32 id, address receiver, bytes20 secretHash, uint64 lockTime)` payable
- `erc20Payment(bytes32 id, uint256 amount, address tokenAddress, address receiver, bytes20 secretHash, uint64 lockTime)`
- `receiverSpend(bytes32 id, uint256 amount, bytes32 secret, address tokenAddress, address sender)`
- `senderRefund(bytes32 id, uint256 amount, bytes20 secretHash, address tokenAddress, address receiver)`
- `payments(bytes32 id)` → `(bytes20 paymentHash, uint64 lockTime, uint8 state)`
- Events: `PaymentSent(id)`, `ReceiverSpent(id, secret)`, `SenderRefunded(id)`

**ABI loading** (`eth.rs:228,584`):
```rust
pub const SWAP_CONTRACT_ABI: &str = include_str!("eth/swap_contract_abi.json");
lazy_static! {
    pub static ref SWAP_CONTRACT: Contract = Contract::load(SWAP_CONTRACT_ABI.as_bytes()).unwrap();
}
```

**Swap ID generation** (`eth.rs:1247-1263`):
- V1: `sha256(timelock_u32_le_bytes ++ secret_hash)` → 32-byte ID

**Deployed V1 contract addresses**: See `docs/plans/etomic-swap-v1-deployment.md` for all 16+ networks. TRON deployment will add one more.
