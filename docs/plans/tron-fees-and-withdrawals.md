# TRON Fees, Signing & Withdrawals

## Context

TRON HD wallet activation and TRC20 token support is complete (PR #2712, branch `tron-tokens-activation`). This plan implements the next phase: local transaction building, signing, fee estimation, and withdrawals for TRX and TRC20 tokens. Branch: `tron-fees-and-withdrawals`.

**Key architectural decisions:**
- **Local protobuf tx building** using `prost::Message` derive macros (already a workspace dep at `0.12`)
- **SHA256 + secp256k1** signing (same curve as ETH, different hash — NOT Keccak256)
- **No EIP-155 chain_id** in signatures, no RLP, no nonces (TRON uses TAPOS)
- **Broadcast via `/wallet/broadcasthex`** (protobuf hex, avoids JSON ↔ protobuf sync)
- **New `TxFeeDetails::Tron` variant** for fee reporting
- **Defer trade fee methods** (wallet-only, no swaps yet)

---

## Research Findings (Reconciled With Current Code)

### Baseline Already Implemented (No Work Needed)
- TRON address type + parsing/formatting/serde: `mm2src/coins/eth/tron/address.rs`
- Chain-aware display rules: `mm2src/coins/eth/chain_address.rs` (`ChainFamily::format`)
- TRON HTTP API client + node rotation + retry classification: `mm2src/coins/eth/tron/api.rs`
- TRX/TRC20 activation (V2) + HD scanning and balances: `mm2src/coins/eth/v2_activation.rs`, `eth_hd_wallet.rs`
- Integration coverage: `mm2src/mm2_main/tests/mm2_tests/tron_tests.rs`

### Corrections From Original Plan
1. **Module root**: The module root is `mm2src/coins/eth/tron.rs` (not `tron/mod.rs`). New modules go under `mm2src/coins/eth/tron/*` and are re-exported from `tron.rs`.

2. **Bandwidth estimation**: Use `Transaction::encode_to_vec().len() + 64` (NOT `raw_data + 65`).
   - Bandwidth = 1 byte = 1 bandwidth point of the full on-chain tx size.
   - The `+64` is `MAX_RESULT_SIZE_IN_TX` — the `ret` field the node appends on-chain.
   - Matches java-tron's actual charging logic and TronWeb's estimation.

3. **Energy estimation**: Use `triggerconstantcontract` (already implemented), NOT `/wallet/estimateenergy`.
   - `estimateenergy` is disabled by default on most nodes (incl. TronGrid) — unreliable with node rotation.
   - `triggerconstantcontract` is universally available and accurate for standard TRC20 tokens.
   - `energy_penalty` (Dynamic Energy Model) is already included in `energy_used` — don't add them.
   - No safety multiplier — use `energy_used` directly.
   - Always use actual recipient address in simulation (new holders cost ~15k more energy due to SSTORE).

4. **TAPOS**: Use `/wallet/getnowblock` (latest block), consistent with TronWeb.
   - Solidified blocks (~54s behind) not needed — TRON DPoS forks are near-impossible.
   - TAPOS validity window is 65,536 blocks (~54 hours).
   - No retry logic on TAPOS_ERROR for initial implementation.
   - Expiration: 60 seconds (plan's original value is correct; max allowed is 24h).

### Protocol Reference (Validated Against java-tron via DeepWiki)

**Protobuf field tags (from Tron.proto, balance_contract.proto, smart_contract.proto):**
```
Transaction:
  raw_data: tag 1 (Transaction.raw)
  signature: tag 2 (repeated bytes)
  ret:       tag 5 (repeated Result — we don't build this, node appends it)

TransactionRaw:
  ref_block_bytes: tag 1  (bytes)
  ref_block_num:   tag 3  (int64, DEPRECATED — do not use)
  ref_block_hash:  tag 4  (bytes)
  expiration:      tag 8  (int64)
  auths:           tag 9  (repeated Authority, DEPRECATED)
  data:            tag 10 (bytes)
  contract:        tag 11 (repeated Contract)
  scripts:         tag 12 (bytes, DEPRECATED)
  timestamp:       tag 14 (int64)
  fee_limit:       tag 18 (int64)

Transaction.Contract:
  type:           tag 1 (ContractType enum)
  parameter:      tag 2 (google.protobuf.Any)
  provider:       tag 3 (bytes — unused by us)
  ContractName:   tag 4 (bytes — unused by us)
  Permission_id:  tag 5 (int32)

TransferContract:
  owner_address: tag 1 (bytes — 21-byte TRON address, 0x41 prefix)
  to_address:    tag 2 (bytes — 21-byte TRON address)
  amount:        tag 3 (int64 — in SUN)

TriggerSmartContract:
  owner_address:    tag 1 (bytes — 21-byte TRON address)
  contract_address: tag 2 (bytes — 21-byte TRON address)
  call_value:       tag 3 (int64)
  data:             tag 4 (bytes — ABI-encoded function call)
  call_token_value: tag 5 (int64)
  token_id:         tag 6 (int64)
```

**Signature format:** `r(32) || s(32) || v(1)` = 65 bytes.
- `ECKey.sign()` produces v = recId + 27 (so 27/28)
- `toByteArray()` subtracts 27 → stored as **0 or 1** in `Transaction.signature`
- Since `ethkey::sign()` returns v = 27/28, we must subtract 27 before storing.

**blockID computation** (added by HTTP servlet, not in protobuf):
```
sha256_hash = SHA256(block_header.raw_data.toByteArray())
blockID[0..8]  = block_number as big-endian i64
blockID[8..32] = sha256_hash[8..32]
```
- ref_block_hash uses `blockID[8..16]` — which is `sha256_hash[8..16]` (correct).

**Bandwidth formula** (when supportVM=true, always true on mainnet):
```
bandwidth = transaction.clearRet().getSerializedSize() + (num_contracts * 64)
```
- For freshly built tx (no ret field): `Transaction::encode_to_vec().len() + 64`

**fee_limit:** Set to 0 for TRX TransferContract. For TRC20 TriggerSmartContract, set to
`energy_used * energy_price_sun`. If too low → OUT_OF_ENERGY (energy still charged).

**Account resource response fields** (`/wallet/getaccountresource`):
- `freeNetUsed`, `freeNetLimit` — free bandwidth (lowerCamelCase)
- `NetUsed`, `NetLimit` — staked bandwidth (**PascalCase** — inconsistent with above)
- `EnergyUsed`, `EnergyLimit` — staked energy (**PascalCase**)
- Proto3 JSON omits zero-value fields; empty `{}` is valid (unactivated account = all zeros)
- 17 total fields in `AccountResourceMessage` proto; we only need these 6

**Chain parameters** (`/wallet/getchainparameters`): returns `[{key, value}, ...]` array.
- `getTransactionFee` — bandwidth price in SUN (currently 1000)
- `getEnergyFee` — energy price in SUN (currently 420 on mainnet)

---

## Commit A01: Minimal TRON Transaction Protobuf Types

**New file:** `mm2src/coins/eth/tron/proto.rs`

Define minimal TRON transaction protobuf types using `#[derive(prost::Message)]`, following the trezor crate pattern (manual structs, no `.proto` files):

```
Any               { type_url: String, value: Vec<u8> }
TransferContract  { owner_address, to_address, amount: i64 }
TriggerSmartContract { owner_address, contract_address, call_value, data, call_token_value, token_id }
TransactionContract { type: ContractType, parameter: Option<Any>, permission_id }
TransactionRaw    { ref_block_bytes, ref_block_hash, expiration, data, contract[], timestamp, fee_limit }
Transaction       { raw_data: Option<TransactionRaw>, signature: Vec<Vec<u8>> }
ContractType      { Unspecified = 0, TransferContract = 1, TriggerSmartContract = 31 }
```

Critical: field tag numbers must match TRON proto (non-sequential in `TransactionRaw`: 1,4,8,10,11,14,18).

Type URLs: `type.googleapis.com/protocol.TransferContract`, `type.googleapis.com/protocol.TriggerSmartContract`.

**`ContractType` must have `Unspecified = 0`**: `prost::Enumeration` uses the first variant as the encoding default — without a zero variant, `TransferContract = 1` is silently skipped during encoding, producing invalid wire format.

**Modify:** `mm2src/coins/eth/tron.rs` — add `pub(crate) mod proto;`.

**Tests:** Roundtrip encode/decode + golden vector tests using real TRON `raw_data_hex` from developer docs (TransferContract + TriggerSmartContract), verifying field values and byte-exact re-encode.

**Status: DONE** ✓

---

## Commit A02: Extend `/wallet/getnowblock` Parsing for TAPOS Inputs

**Modify:** `mm2src/coins/eth/tron/api.rs`

Extend `GetNowBlockResponse` to parse:
- `blockID` (string) → decode to `[u8; 32]`
- `block_header.raw_data.timestamp` (ms)

Add: `TronApiClient::get_block_for_tapos() -> TaposBlockData { number, block_id, timestamp }`
- Reuses `try_clients` rotation via `/wallet/getnowblock`

**Tests:** JSON parse tests for `GetNowBlockResponse` including `blockID` and timestamp.

---

## Commit A03: TAPOS Helper + Transaction Builder (Unsigned)

**New file:** `mm2src/coins/eth/tron/tx_builder.rs`

### TAPOS Calculation
```rust
fn tapos_from_block(block_number: u64, block_id: &[u8; 32]) -> (Vec<u8>, Vec<u8>) {
    let num_bytes = block_number.to_be_bytes();
    let ref_block_bytes = num_bytes[6..8].to_vec();   // last 2 bytes of block number
    let ref_block_hash = block_id[8..16].to_vec();    // bytes 8-15 of block hash
    (ref_block_bytes, ref_block_hash)
}
```

### Address Helper
- `tron_addr_bytes(addr: &TronAddress) -> Vec<u8>` — converts `TronAddress` to raw 21-byte proto format
- Ensures proto structs always get correctly-prefixed 21-byte addresses

### Transaction Builders
- `build_trx_transfer(from, to, amount_sun, block_data)` → `TransactionRaw`
- `build_trc20_transfer(from, contract, recipient, amount, fee_limit, block_data)` → `TransactionRaw`
  - ABI-encodes `transfer(address,uint256)` with `0xa9059cbb` selector
  - Addresses in ABI: 20-byte EVM format (strip 0x41 prefix), left-padded to 32 bytes
- Sets `expiration = block_timestamp + 60_000` (60 seconds)
- Sets `timestamp = now_ms()`
- Packs contract into `Any { type_url, value: contract.encode_to_vec() }`

**Modify:** `mm2src/coins/eth/tron.rs` — add `pub mod tx_builder;`

**Tests:** Deterministic builder tests with fixed TAPOS block data.

---

## Commit A04: TRON Signing Module (SHA256 + secp256k1)

**New file:** `mm2src/coins/eth/tron/sign.rs`

### Signing Flow
```
raw_data_bytes = TransactionRaw::encode_to_vec()
tx_id = SHA256(raw_data_bytes)                    // bitcrypto::sha256 (already in codebase)
(r, s, recovery_id) = secp256k1_sign(tx_id, private_key)
signature = r (32B) || s (32B) || recovery_id (1B) = 65 bytes
```

### Key Functions
- `sign_tron_transaction(raw: &TransactionRaw, secret: &ethkey::Secret) -> Result<(H256, Transaction)>`
  - Returns `(tx_hash, signed_transaction)`
  - **Confirmed:** `ethkey::sign(secret, &H256)` takes a **pre-hashed** `H256` digest (does NOT hash internally)
  - Flow: `sha256(raw.encode_to_vec())` → `H256::from(hash)` → `ethkey::sign(secret, &hash)` → extract 65-byte sig
  - Recovery id handling: current `ethkey::sign` in this codebase returns `v` as `0/1`; keep normalization logic to also accept `27/28` as a compatibility guard.

### Reuse from codebase
- `bitcrypto::sha256` for hashing (mm2_bitcoin crate, already imported in eth.rs)
- `ethkey::sign(&secret, &H256)` for secp256k1 signing (pre-hashed, returns 65-byte `Signature`)
- `KeyPair` from `EthPrivKeyPolicy` for the private key

**Modify:** `mm2src/coins/eth/tron.rs` — add `pub(crate) mod sign;`

**Tests:** Sign known test vectors, verify signature and txID.

**Temporary lint allowance:** `tron/sign.rs` may carry scoped `#![allow(dead_code)]` until A09 wires the module into withdraw flow.

**Status: DONE** ✓

---

## Commit A05: Fee Estimation + `TxFeeDetails::Tron`

**New file:** `mm2src/coins/eth/tron/fee.rs`

### Bandwidth Estimation (in fee.rs)
```rust
fn estimate_bandwidth(tx: &Transaction) -> u64 {
    const RESULT_BYTES_OVERHEAD_PER_CONTRACT: u64 = 64;
    let contracts = tx.raw_data.as_ref().map(|r| r.contract.len().max(1) as u64).unwrap_or(1);
    tx.encoded_len() as u64 + RESULT_BYTES_OVERHEAD_PER_CONTRACT * contracts
}
```
For pre-signing estimation, build a `Transaction` with a 65-byte placeholder signature.

### Fee Calculation Logic

**TRX transfer (bandwidth only):**
```
estimated_bandwidth = estimate_bandwidth(tx_with_placeholder_sig)
available_bandwidth = (free_net_limit - free_net_used) + (net_limit - net_used)
deficit = max(0, estimated_bandwidth - available_bandwidth)
fee_sun = deficit * bandwidth_price_sun  // bandwidth_price = 1000 SUN
```

**TRC20 transfer (bandwidth + energy):**
```
energy_needed = trigger_constant_contract(transfer call).energy_used  // existing method, no multiplier
bandwidth_needed = estimate_bandwidth(tx_with_placeholder_sig)
energy_deficit = max(0, energy_needed - available_energy)
bw_deficit = max(0, bandwidth_needed - available_bandwidth)
fee_sun = (energy_deficit * energy_price_sun) + (bw_deficit * bandwidth_price_sun)
```

### New TxFeeDetails Variant

**Modify:** `mm2src/coins/lp_coins.rs`

```rust
pub enum TxFeeDetails {
    // ... existing variants
    Tron(TronTxFeeDetails),
}

pub struct TronTxFeeDetails {
    pub coin: String,           // "TRX"
    pub bandwidth_used: u64,    // bandwidth points consumed
    pub energy_used: u64,       // energy units consumed (0 for TRX transfers)
    pub bandwidth_fee: BigDecimal,  // TRX cost for bandwidth
    pub energy_fee: BigDecimal,     // TRX cost for energy
    pub total_fee: BigDecimal,      // total TRX cost
}
```

**Modify:** `mm2src/coins/eth/tron.rs` — add `pub mod fee;`

**Tests:** Fee calculation with various resource states, overflow-saturation coverage, contract-count bandwidth coverage, and serialization/deserialization of `TxFeeDetails::Tron` (including untagged no-type acceptance and near-miss rejection).

**Status: DONE** ✓

**Progress note (2026-02-13):**
- Updated bandwidth estimation to avoid allocation (`encoded_len`) and account for per-contract overhead to prevent underestimation on multi-contract payloads.
- Added regression tests in `tron/fee.rs` for multi-contract bandwidth overhead and saturation behavior on large values.
- Added `TxFeeDetails::Tron` serde tests for untagged deserialization without `type` and rejection of near-miss shapes.
- Enforced fixed 6-decimal TRX scale in `sun_to_trx_decimal` and added a scale-stability regression test.

---

## Commit A06: TRON RPC Methods for Fees + Broadcasting

**Modify:** `mm2src/coins/eth/tron/api.rs`

### New TronApiClient Methods
- `get_account_resource(address)` → maps to existing `TronAccountResources` from `fee.rs`
  - POST to `/wallet/getaccountresource` with `{"address": "<hex>", "visible": false}`
  - Deserialization struct uses exact `#[serde(rename = "...")]` for TRON's mixed-case proto field names
  - All fields `#[serde(default)]` — proto3 JSON omits zero-value fields
  - Empty `{}` is valid (unactivated account or all-zero resources), not an error
- `get_chain_parameters()` → ✓ Already implemented as `get_chain_prices()`
- `broadcast_hex(tx_hex: &str)` → `BroadcastHexResponse { txid: String }`
  - POST to `/wallet/broadcasthex` with `{ "transaction": tx_hex }`
  - Error responses (`result: false`) already handled by `tron_error_from_value()` in `post()`
  - Success struct only needs `txid` (64-char hex, always present — computed before broadcast)

All methods behind `try_clients` rotation. Error shapes already recognized by existing `tron_error_from_value()` and `is_retryable()`.

**Tests:** Serde JSON parsing tests for each new response type.

**Status: DONE** ✓

**Progress note (2026-02-13):**
- Implemented `TronHttpClient::get_chain_prices()` and `TronApiClient::get_chain_prices()` with strict extraction of `getTransactionFee` and `getEnergyFee`.
- Invalid/missing/zero fee params are now returned as `Web3RpcError::BadResponse` (retryable), enabling fallback to next node via existing `try_clients` rotation logic.
- Added unit tests in `api.rs` for valid parsing, zero-value rejection (retryable), and real-response compatibility where some `chainParameter` entries omit `value`.

**Progress note (2026-02-19):**
- Implemented `get_account_resource(address)` on both `TronHttpClient` and `TronApiClient` with `AccountResourceResponse` intermediate serde struct using exact `#[serde(rename)]` for TRON's mixed-case proto field names and `#[serde(default)]` for proto3 zero-omission.
- Implemented `broadcast_hex(tx_hex)` on both `TronHttpClient` and `TronApiClient` with `BroadcastHexResponse { txid }` success-only struct (errors caught by `tron_error_from_value()`).
- Added 8 unit tests: canonical response, empty `{}`, partial response, negative rejection, wrong-type rejection, snake_case rejection, broadcast success parsing, broadcast error detection.
- Added 2 Nile integration tests: known address resource query (verifies `freeNetLimit > 0`), unactivated address (verifies all-zero defaults from `{}` response).

**Research grounding (2026-02-19):**

*Validated against java-tron source, TRON protocol protobufs, TRON HTTP API docs, and live node behavior.*

- Endpoint wiring: `/wallet/getaccountresource` and `/wallet/broadcasthex` are first-class wallet HTTP routes registered in `FullNodeHttpApiService`.
- `getaccountresource` request/response:
  - Request body: `{"address": "...", "visible": true|false}`. We use hex format (`visible: false`), same pattern as existing `get_account()`.
  - `GetAccountResourceServlet` normalizes address via `visible` flag before calling `wallet.getAccountResource(...)`.
  - Response is proto3 JSON of `AccountResourceMessage` (17 fields total, we need 6).
  - **Mixed casing in field names** — proto fields were not defined in standard `snake_case`, so JSON keys preserve the original casing verbatim:
    - lowerCamelCase: `freeNetUsed`, `freeNetLimit`
    - PascalCase: `NetUsed`, `NetLimit`, `EnergyUsed`, `EnergyLimit`
    - This requires explicit `#[serde(rename = "...")]` per field. TRON never sends snake_case variants — do NOT add aliases like `net_used`.
  - Proto field types are `int64`. Using `u64` in the deserialization struct is safe: serde_json rejects negative JSON numbers for `u64`, which flows through `post()` as `BadResponse` (retryable). No need for explicit `i64` intermediate parsing.
  - Empty `{}` response = unactivated account or all-zero resources. Not an error — `#[serde(default)]` on all fields handles this correctly, producing `TronAccountResources` with all zeros.
  - `freeNetLimit` is currently 600 on mainnet (was 5000, reduced to 1500 then 600 via governance proposals).
  - `NetLimit` and `EnergyLimit` already include delegated resources (Stake V2) in java-tron's calculation — no adjustment needed on our side.
- `getchainparameters`: ✓ Already handled. Heterogeneous `chainParameter` array with optional `value` fields, `getTransactionFee`/`getEnergyFee` extraction validated.
- `broadcasthex` request/response:
  - Request field is `"transaction"` (hex-encoded signed protobuf `Transaction` bytes). Confirmed from `BroadcastHexServlet.java`: `JSONObject.parseObject(input).getString("transaction")`.
  - Internally calls `wallet.broadcastTransaction(...)` — same path as `/wallet/broadcasttransaction`.
  - Response fields: `result` (bool), `code` (string), `message` (UTF-8 text), `txid` (64-char hex), `transaction` (JSON-stringified decoded tx).
  - `txid` is always present (success and failure) — computed via `TransactionCapsule.getTransactionId()` before broadcast.
  - `message` is plain UTF-8 in broadcasthex (unlike broadcasttransaction which may hex-encode it). Our `tron_error_from_value()` handles both via `value_to_string()`.
  - `BANDWITH_ERROR` (missing 'd') is the official proto spelling — matches our existing `is_retryable()`.
  - `DUP_TRANSACTION_ERROR` is non-retryable but may indicate tx was already accepted by the network.
- Retry semantics validated against `Return.response_code` enum (14 values). Our existing `is_retryable()` correctly classifies:
  - Retryable/transient: `SERVER_BUSY`, `NO_CONNECTION`, `NOT_ENOUGH_EFFECTIVE_CONNECTION`, `BLOCK_UNSOLIDIFIED`.
  - Non-retryable/deterministic: `SIGERROR`, `CONTRACT_VALIDATE_ERROR`, `CONTRACT_EXE_ERROR`, `BANDWITH_ERROR`, `DUP_TRANSACTION_ERROR`, `TAPOS_ERROR`, `TOO_BIG_TRANSACTION_ERROR`, `TRANSACTION_EXPIRATION_ERROR`, `OTHER_ERROR`.

**A06 implementation constraints derived from research:**
- `get_account_resource(address)`:
  - Add `GetAccountResourceRequest { address, visible }` (same pattern as `GetAccountRequest`).
  - Add deserialization struct with exact `#[serde(rename)]` per field for mixed-case proto names and `#[serde(default)]` on all fields for proto3 zero-omission.
  - Convert parsed response to existing `TronAccountResources` from `fee.rs` — no new domain type needed.
  - Malformed payloads and structurally invalid JSON are classified as retryable via `post()`'s existing `json::from_value` → `BadResponse` path.
  - Add methods on both `TronHttpClient` and `TronApiClient` (with `try_clients` rotation).
- `broadcast_hex(tx_hex)`:
  - Add `BroadcastHexRequest { transaction }` and `BroadcastHexResponse { txid }` (success-only; errors caught by `tron_error_from_value()` before deserialization).
  - Add methods on both `TronHttpClient` and `TronApiClient`.
- Serde/validation tests:
  - Canonical response shape (all 6 fields present).
  - Omitted zero/default fields (minimal `{}` and partial responses like `{"freeNetLimit": 600}`).
  - Negative value rejection (serde rejects negative for `u64` → `BadResponse`).
  - Wrong-type field rejection (string instead of number).
  - Exact casing enforcement (verify `NetUsed` works; do not add snake_case aliases).
  - `BroadcastHexResponse` txid parsing.

**Sources:**
- TRON HTTP API docs: <https://tronprotocol.github.io/documentation-en/api/http/>
- TRON protocol proto (`AccountResourceMessage`, `Return.response_code`): <https://github.com/tronprotocol/protocol/blob/master/api/api.proto>
- java-tron `GetAccountResourceServlet`: <https://github.com/tronprotocol/java-tron/blob/develop/framework/src/main/java/org/tron/core/services/http/GetAccountResourceServlet.java>
- java-tron `BroadcastHexServlet`: <https://github.com/tronprotocol/java-tron/blob/develop/framework/src/main/java/org/tron/core/services/http/BroadcastHexServlet.java>
- java-tron `Wallet.getAccountResource()`: <https://github.com/tronprotocol/java-tron/blob/develop/framework/src/main/java/org/tron/core/Wallet.java>
- java-tron `Wallet.broadcastTransaction()`: <https://github.com/tronprotocol/java-tron/blob/develop/framework/src/main/java/org/tron/core/Wallet.java>
- TRON Developer Hub (getaccountresource): <https://developers.tron.network/reference/getaccountresource>
- TRON Developer Hub (broadcasthex): <https://developers.tron.network/reference/broadcasthex>

---

## Commit A07: Unused (reserved, merged into A05+A06)

*Bandwidth estimation is in fee.rs (Commit A05). API methods and broadcast are in Commit A06. This commit number is skipped to keep the A01-A10 numbering stable.*

---

## Commit A08: Broadcasting Hook in `send_raw_transaction`

**Modify:** `mm2src/coins/eth.rs` — `send_raw_tx()` and `send_raw_tx_bytes()`

Add chain dispatch:
```rust
fn send_raw_tx(&self, tx: &str) -> ... {
    match ChainFamily::from(&self.chain_spec) {
        ChainFamily::Tron => {
            let tron = self.tron_rpc().ok_or(...)?;
            tron.broadcast_hex(tx).await
        },
        ChainFamily::Evm => { /* existing eth_sendRawTransaction path */ },
    }
}
```

**Input validation:** When wiring `broadcast_hex` to the dispatcher (`send_raw_tx`), validate the incoming `tx_hex` string:
- Strip `0x` prefix if present (TRON expects raw hex).
- Validate hex-only characters.
- Enforce a maximum length to prevent DoS via oversized payloads.

**Tests:** Integration test that broadcasts a known-good signed tx on Nile (feature-gated).

---

## Commit A09: TRON Withdraw Implementation

**Modify:** `mm2src/coins/eth/eth_withdraw.rs` — replace the TRON stub.

### TRON Signing Path in `sign_withdraw_tx`

Replace:
```rust
ChainSpec::Tron { .. } => Err(WithdrawError::ProtocolNotSupported("Tron is not supported for withdraw yet"))
```

With TRON-specific flow:
```rust
ChainSpec::Tron { .. } => {
    let key_pair = self.get_key_pair(req)?;
    let tron_rpc = coin.tron_rpc().ok_or(WithdrawError::InternalError(...))?;

    // 1. Get latest block for TAPOS
    let block_data = tron_rpc.get_block_for_tapos().await?;

    // 2. Build unsigned transaction
    let raw = match &coin.coin_type {
        EthCoinType::Eth => build_trx_transfer(from, to, amount_sun, &block_data),
        EthCoinType::Erc20 { token_addr, .. } => {
            build_trc20_transfer(from, *token_addr, to, amount, fee_limit, &block_data)
        },
        EthCoinType::Nft { .. } => return Err(WithdrawError::ProtocolNotSupported(...)),
    };

    // 3. Sign
    let (tx_hash, signed_tx) = sign_tron_transaction(&raw, key_pair.secret())?;

    // 4. Encode to protobuf hex
    let tx_bytes = signed_tx.encode_to_vec();
    Ok((tx_hash, BytesJson::from(tx_bytes)))
}
```

### Withdraw `build()` Adjustments for TRON

1. **Fee estimation**: Use TRON fee calculation (fee.rs) instead of gas estimation
   - Call `get_account_resource()` + `get_chain_parameters()` + `trigger_constant_contract()` (for TRC20)
   - No `get_eth_gas_details_from_withdraw_fee` for TRON

2. **No nonce**: Skip `get_addr_nonce()` and nonce lock for TRON (uses TAPOS instead)

3. **Max withdraw (TRX)**: Deduct estimated bandwidth fee from amount
   - `max_amount = balance - estimated_bandwidth_fee`

4. **Max withdraw (TRC20)**: No deduction (fee paid in TRX, not the token)
   - But must verify sufficient TRX balance for energy + bandwidth fees

5. **Fee details**: Return `TxFeeDetails::Tron` instead of `TxFeeDetails::Eth`

6. **Custom fees**: Initially skip `WithdrawFee` support for TRON — always auto-estimate
   - If `req.fee` is `Some(WithdrawFee::EthGas { .. })`, return error for TRON

7. **Cleanup temporary dead code allowances**:
   - Remove scoped `dead_code` allowances added for pre-integration modules once they are used by runtime paths (A04 signing module and any similar temporary allowances from A05/A06 scaffolding).
   - Keep `-D warnings` clean without broad crate-level allows.

**Tests:** Unit tests for fee policy rejection, TRON-signed tx bytes are NOT RLP (sanity check).

**Open follow-up (A09 dependency on A06):**
- Wire `get_account_resource()` + `get_chain_prices()` outputs into withdraw fee estimation path. Invalid payloads trigger `BadResponse` → node rotation via `try_clients` before building withdraw tx.
- Wire `broadcast_hex()` into the signing/broadcast flow via `send_raw_tx` chain dispatch (A08).

**Status update (2026-02-23):** A09 hardening is complete in this cycle.
- TRON tx timestamp uses block timestamp (not validated by java-tron, matches TronWeb).
  Expiration uses `block_timestamp + 60s` — block timestamp is network-authoritative;
  if the node is stale, TAPOS ref_block data is also stale and the tx fails regardless.
- TRON RPC errors mapped via `map_tron_rpc_err` (delegates to `From<Web3RpcError> for WithdrawError`, consistent with EVM).
- Kept TRC20 `fee_limit` semantics as full-energy max cap (`energy_used * energy_price`) with explicit documentation.
- Completed low-risk cleanup:
  - Removed duplicate TRON NFT unsupported guard.
  - Replaced misleading TRON `try_join!` fetch with explicit sequential flow.
  - Deduplicated TRC20 transfer ABI token construction into a shared helper.
- HD `from=None` uses `activated_key` — this is intentional and consistent with EVM withdraw behavior.
- TRON memo rejected with error (TRON charges 1 TRX burn fee per TIP-387; proper support deferred).
- Removed `WithdrawError::NodeRejected` — RPC errors during withdraw are pre-flight, not broadcast rejections. Uses `Transport` via `From` impl like EVM.
- Moved `trc20_transfer_tokens` to `tron.rs` (parent module) to fix inverted layering between api.rs and tx_builder.rs.
- Added concurrency warning doc comment on `TronApiClient`.

---

## Commit A10: Integration Tests (Withdraw + Send)

**Status update (2026-02-23):** Deferred for this cycle. No A10 integration tests are included in the current implementation set.

**Modify:** `mm2src/mm2_main/tests/mm2_tests/tron_tests.rs`
**Modify:** `mm2src/mm2_test_helpers/src/for_tests.rs`

### New Test Helpers
- Poll `task::withdraw::status` until Ok/Error
- `withdraw_trx(mm, to, amount)` → calls `withdraw` RPC for TRX
- `withdraw_trc20(mm, coin, to, amount)` → calls `withdraw` RPC for TRC20
- `send_raw_tron_tx(mm, coin, tx_hex)` → calls `send_raw_transaction` RPC

### Integration Tests (feature-gated `tron-network-tests`)
- `test_trx_withdraw_and_send()` — TRX withdraw + broadcast on Nile
- `test_trc20_withdraw_and_send()` — TRC20 USDT withdraw + broadcast on Nile
- `test_trx_withdraw_max()` — Max TRX withdraw (fee deduction)
- `test_trx_withdraw_hd()` — HD wallet TRX withdraw from specific derivation path
- `test_trc20_withdraw_hd()` — HD wallet TRC20 withdraw
- `test_trx_withdraw_invalid_address()` — Reject invalid TRON addresses
- `test_trx_fee_details_structure()` — Verify `TxFeeDetails::Tron` fields
- `tron_nile_known_trc20_tx_fee_receipt` / `tron_nile_known_trc20_tx_fee_receipt_retry_failover` — Validate fee/resource receipt fields against a real Nile TRC20 transfer hash and verify retry/failover path for fee-related fetches
- `tron_nile_chain_fee_parameters_are_present_and_valid` + `parse_chain_prices_rejects_zero_values_as_retryable_bad_response` — Validate chain fee parameters and enforce retryable handling for invalid fee parameter payloads

### Unit Tests (always run)
- Proto encode/decode roundtrip
- TAPOS calculation from known block data
- Transaction builder output verification
- SHA256 signing with known test vectors
- Fee calculation with various resource states
- Bandwidth estimation accuracy

**Progress note (2026-02-13):** Added Nile reference-transaction fee receipt checks and retry-policy assertions in `mm2src/coins/eth/tron/api_integration_tests.rs` to strengthen fee accuracy and invalid-parameter retry behavior coverage ahead of A06/A09 wiring.

**Open follow-up (A10 extension):**
- Add an end-to-end Nile test that computes estimated TRC20 fee inputs from live pre-state (`get_account_resource`, `get_chain_prices`, `trigger_constant_contract`) and compares estimator outputs against receipt fields with an explicit tolerance policy.
- Add a dispatch-verification integration test: call `send_raw_tx` with known-invalid hex on a TRON coin and assert the error originates from TRON `/wallet/broadcasthex` (not EVM RPC). Proves A08 chain dispatch works end-to-end. (Deferred from A08.)
- Validate stale-node failure mode: connect to a deliberately lagging Nile node and confirm that a transaction built from stale block data fails with `TAPOS_ERROR` (not `TRANSACTION_EXPIRATION_ERROR`), confirming that block_data.timestamp is sufficient for expiration.

---

## Commit A11: Cross-Target Unit Tests (Native + WASM)

**Modify:** `mm2src/coins/eth/tron/sign.rs`
**Modify:** `mm2src/coins/eth/tron/tx_builder.rs`
**Modify:** `mm2src/coins/eth/tron/proto.rs`

### Goal
- Ensure TRON deterministic unit tests run on both native and `wasm32` targets.
- Replace/augment plain `#[test]` coverage for core deterministic tests with `cross_test!` macro usage where feasible.

### Scope
- Convert deterministic, pure tests (no network, no filesystem) to `cross_test!`.
- Keep network-gated tests (`tron-network-tests`) native/feature-gated as-is.
- Avoid introducing WASM-only conditional logic in production code paths.

### Notes
- In this codebase the macro is `cross_test!` (singular) from `common`.
- Prioritize signing and protobuf/serialization vectors first, then builder vectors.

**Tests:** Verify affected tests pass on native and wasm targets.

---

## Critical Files

| File | Action |
|------|--------|
| `mm2src/coins/eth/tron/proto.rs` | **Create** — Protobuf message types |
| `mm2src/coins/eth/tron/tx_builder.rs` | **Create** — Transaction construction |
| `mm2src/coins/eth/tron/sign.rs` | **Create** — SHA256 + secp256k1 signing |
| `mm2src/coins/eth/tron/fee.rs` | **Create** — Fee estimation (bandwidth/energy) |
| `mm2src/coins/eth/tron.rs` | **Modify** — Export new modules |
| `mm2src/coins/eth/tron/api.rs` | **Modify** — New API methods (broadcast_hex, get_account_resource, get_chain_parameters, extended get_now_block) |
| `mm2src/coins/eth/eth_withdraw.rs` | **Modify** — Replace TRON stub with signing + build flow |
| `mm2src/coins/eth.rs` | **Modify** — `send_raw_tx` chain dispatch |
| `mm2src/coins/lp_coins.rs` | **Modify** — `TxFeeDetails::Tron` variant |
| `mm2src/mm2_main/tests/mm2_tests/tron_tests.rs` | **Modify** — Withdraw integration tests |
| `mm2src/mm2_test_helpers/src/for_tests.rs` | **Modify** — Withdraw test helpers |

## Reusable Functions

| Function | Location | Purpose |
|----------|----------|---------|
| `bitcrypto::sha256()` | `mm2_bitcoin/crypto` | SHA256 hashing for txID |
| `ethkey::KeyPair` | `ethkey` crate | secp256k1 private key access |
| `TronAddress::from/to_evm_address()` | `eth/tron/address.rs` | Address format conversion |
| `ChainFamily::from(&chain_spec)` | `eth.rs` | Chain dispatch pattern |
| `coin.tron_rpc()` | `eth.rs:1152` | TronApiClient accessor |
| `ERC20_CONTRACT.function("transfer")` | `eth.rs` | ABI encoding (also works for TRC20) |
| `abi_encode_address_param()` | `eth/tron/api.rs` | ABI address padding for TRON |
| `trigger_constant_contract()` | `eth/tron/api.rs` | Dry-run for energy estimation |

## Verification

1. **Unit tests:** `cargo test -p coins --lib tron` (proto, builder, signing, fee)
2. **Clippy:** `cargo clippy -p coins -p mm2_main -- -D warnings`
3. **Integration tests:** `cargo test --test mm2_tests_main --features tron-network-tests tron_withdraw`
4. **WASM tests (cross-target deterministic coverage):** `cargo test -p coins --target wasm32-unknown-unknown --lib tron`
5. **Cleanup check:** remove temporary scoped `dead_code` allowances introduced for pre-integration modules once runtime wiring lands (A09/A11 scope).
6. **Doc comments coverage:** verify new/changed public TRON-facing structs/functions/modules include concise rustdoc explaining purpose, units, and chain-specific behavior (especially fee fields and signing/serialization semantics).
7. **Manual verification:**
   - Enable TRX on Nile testnet
   - Withdraw small TRX amount → verify on tronscan.org (Nile)
   - Enable TRC20 USDT → Withdraw → verify on tronscan.org (Nile)
   - Check fee_details in response has `TxFeeDetails::Tron` with bandwidth/energy breakdown
