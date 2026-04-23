# PR #2714 Review Replies

## Comment 1 — `proto.rs:4` (include .proto files and generate Rust structs) ✅ POSTED
> "why don't we include these files in our codebase instead and generate the rust structs from them?"

**Reply:** I know the standard approach is to include `.proto` files and generate from them. But I went with hand-written structs here intentionally. With AI they were easy to generate and verify against the upstream definitions. TRON's `Tron.proto` is 1,090 lines with 45+ message types across multiple files (`Tron.proto`, `balance_contract.proto`, `smart_contract.proto`). We only need 7 structs for TRX transfers and TRC20 interactions.

Hand-writing them avoids the `prost-build`/`protoc` overhead on every build for just 7 structs. These core transaction types haven't changed in TRON's protocol for years. I would have actually preferred the same approach for the SLP/Zcoin protos too. `bchrpc.proto` generates 93 types into `pb.rs` (1,553 lines) but we only use about 8 of them, and the module needs `#[allow(dead_code, clippy::all)]` to suppress warnings for all the unused generated code.

## Comment 2 — `proto.rs:112` (what/which epoch?) ✅ POSTED
> "what/which epoch?"

**Reply:** Unix epoch. Clarified in d1508f7f19.

## Comment 3 — `api.rs:366` (optional fields in struct, when None?) ✅ POSTED
> "Q: since the two fields inside are optional, in what cases would they be None? and can one of them be Some while the other isn't?"

**Reply:** Posted explanation + "Fixed in d1508f7f19."

## Comment 4 — `api.rs:386` (since epoch?) ✅ POSTED
> "same as the other comment in proto.rs: what is 'since epoch'? since the start of the current epoch i presume?"

**Reply:** Same as comment 2 link, fixed in d1508f7f19.

## Comment 5 — `api.rs:907` (check error structure, not just is_err) ✅ POSTED
> "let's be more strict and not just check is_err(). let's check the error structure/variant."

**Reply:** `serde_json::Error` doesn't expose structured variants we can match on, but it does have category methods. Changed both tests to `unwrap_err()` then check `err.is_data()` for the category and `err.to_string().contains(...)` for the specific message. Fixed in COMMIT_LINK.

## Comment 6 — `tx_builder.rs:16` (expiry too short, make customizable) ✅ POSTED
> "or better, let's let this value be customizable in the withdraw request. note that in the GUI, withdraw doesn't automatically broadcast but rather show transaction details and this tx is pending broadcasting. if the user stares at these details for long enough they might miss the expiry deadline and the tx is no longer valid."

**Reply:** Added `expiration_seconds` as an optional field in `WithdrawRequest` in COMMIT_LINK.

## Comment 7 — `tx_builder.rs:25` (block_id already contains block number) ✅ POSTED
> "doesn't the block_id already contain the block number (bytes 0-7)? do we really need the block_number argument?"

**Reply:** Kept as is. Thought we validated that block_id[0..8] matches the block number but we don't. Using the canonical proto field directly.

## Comment 8 — `tx_builder.rs:78` (why not use now_ms) ✅ POSTED
> "why not use now_ms() (as done in earlier commits)"

**Reply:** `block_data.timestamp` is the right choice. TronWeb does the same — uses the block header timestamp, not `Date.now()`. java-tron doesn't validate the timestamp field (only `expiration` is checked against head block time), so both work, but block timestamp is a better anchor than the local clock which could drift from chain time. I changed it from `now_ms()` not only to match the reference implementation but also because the deterministic builder makes golden vector tests work without mocking time.

## Comment 9 — `sign.rs:68` (why is signature a vector of vectors?) ✅ POSTED
> "Q: why is signature a vector of vectors? when i saw that in proto.rs i thought that we have a different vector for each of r, s & v. but this doesn't seem to be the case."

**Reply:** It's a vector of vectors because TRON's proto defines the field as `repeated bytes` — `repeated` means a list (outer vector), `bytes` is a byte array (inner vector). The outer vector holds one signature per signer (for multi-sig), and the inner vector is the 65-byte `r(32)||s(32)||v(1)` of that signer. For single-owner accounts (which is all we support) the outer vector always has one element. This matches how the reference implementation and other wallets handle it.

## Comment 10 — `api.rs:669` (no request struct for response, mention TxByIdRequest) ✅ POSTED
> "a bit confusing why there is no request struct for this response. let's mention that TxByIdRequest is the request struct for clarity."

**Reply:** Added a doc comment cross-referencing `TxByIdRequest` on `GetTransactionInfoByIdResponse` in 650dbece2a.

## Comment 11 — `api.rs:579` (what's AccountResourceMessage?) ✅ POSTED
> "what's AccountResourceMessage?"

**Reply:** `AccountResourceMessage` is the proto message name from TRON's protocol definition. It's what `/wallet/getaccountresource` returns as proto3 JSON. It has 17 fields total but we only need 6. I guess you already figured this out in the next comment.

## Comment 12 — `api.rs:598` (why intermediate struct?) ✅ POSTED
> "why did we need this intermediate struct?"

**Reply:** The intermediate struct exists because the API JSON has mixed-case field names from proto3 serialization (`freeNetUsed` vs `NetUsed` vs `EnergyUsed`) and needs `#[serde(default)]` for proto3 zero-omission. I wanted to keep `TronAccountResources` in `fee.rs` as a clean domain type without serde attributes so it stays simple for fee calculations and tests. I can eliminate it and put the serde attributes directly on `TronAccountResources` if you think that's better.

## Comment 13 — `api.rs:446` (document what visible means) ✅ POSTED
> "could we document what this visible means"

**Reply:** `visible` is a TRON HTTP API parameter that controls address format. `visible: true` means addresses in request and response are Base58Check format (`T...`), `visible: false` means hex format (`41...`). Added a doc comment in 5cea08d2d9.

## Comment 14 — `api.rs:756` (visible changed from false to true, effect?) ✅ POSTED
> "the visible here changed from false to true. what effect does this have?"

**Reply:** Originally `get_account` used `visible: false` with `address.to_hex()` (hex format). I changed it to `visible: true` with `&TronAddress` directly (which serializes as Base58Check) to standardize all request structs to the same format. No functional difference — the server handles both, it's just consistency.

## Comment 15 — `eth_withdraw.rs:47` (do we already have this func somewhere?) ✅ POSTED
> "are we sure we don't have this func already implemented somewhere O_o"

**Reply:** The inline pattern `format!("{:02x}", signed.tx_hash_as_bytes())` exists in `eth.rs` but there's no reusable function for it. I extracted it to avoid repetition since it's used in two places in `eth_withdraw.rs` (TRON and EVM paths).

## Comment 16 — `api.rs:389` (BadResponse should kick faulty node) ✅ POSTED
> "i feel like BadResponse should (aside from triggering a rotation) kick the faulty node from the rpc server pool."

**Reply:** Agree it would be nice, but the EVM path (`try_rpc_send` in `eth_rpc.rs`) doesn't kick nodes either — it just rotates on success and retries on failure. I matched that pattern for consistency. A node that's syncing or temporarily behind could produce a `BadResponse` and recover later, so permanently kicking it would be too aggressive. The rotation already moves it to the back. Node eviction would be a good addition but should be done for both EVM and TRON together when we unify the node rotation logic into a common abstraction.

## Comment 17 — `api.rs:744` (verify timestamp in validated_header like block number) ✅ POSTED
> "why not also verify the timestamp in validated_header() like what we did with the block number."

**Reply:** The timestamp is already validated in `get_block_for_tapos()`, just not in `validated_header()`. I kept them separate because `validated_header()` has two callers — `get_block_for_tapos()` and `current_block()` — and `current_block()` only needs the block number. I can move the timestamp check into `validated_header()` if you prefer, and `current_block()` would just ignore it.

## Comment 18 — `withdraw.rs:4` (what does Free mean?)
> "what does Free mean here?"

**Reply:** "Free function" is a common term for functions not inside an `impl` block (as opposed to associated functions/methods). Rephrased to "Standalone functions" for clarity in 68cd4ebe2b.

## Comment 19 — `withdraw.rs:82` (can't use == for non-max withdraws) ✅ POSTED
> "i don't think we can even use ==. this will break for non-max withdraws."

**Reply:** Right, `==` would never match for non-max since `affordable` is typically much larger than the requested amount. Cleaned up the comment to not over-explain the `>=` choice in 9fb7ddba80.

## Comment 20 — `withdraw.rs:84` (max always needs two iterations) ✅ POSTED
> "i think based on the structure of the logic here, this will always happen. if the request is setting max: true, we can never match affordable >= amount_sun from the first try, so we need at least two iterations."

**Reply:** Yes, in the typical case (nonzero fee) max always needs two iterations — first iteration has `amount_sun == balance_sun` so `affordable` is always less. It can converge in one iteration only when the fee is zero (account has enough free bandwidth to cover the tx).

## Comment 21 — `withdraw.rs:133` (why no max support for TRC20?) ✅ POSTED
> "why no max support for trc20?"

**Reply:** TRC20 max is supported — it just doesn't need the iterative loop. The caller already sets `amount_base_units = my_balance` when `req.max` is true, and `build_tron_trc20_withdraw` passes it through unchanged. Since TRC20 fees are paid in TRX (not the token), "max" simply means "send all tokens" — no fee deduction from the token amount. The fee sufficiency check is against the TRX balance separately.
