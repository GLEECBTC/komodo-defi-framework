# Mintlayer integration

## Status and review scope

This experimental community integration implements native Mintlayer transfers
and the KDF SwapOps (V1) atomic-swap path. The historical frozen implementation
is `072b38149bbed516c25dbd12c9e60e2330b82ca9`; the preliminary review branch
was published at `7147925226175720df39c6074c4f89abef5570da`.
The PR candidate is based on upstream dev
`e686ef3500585f01c9f0e89c8c01bc036c42253c`, with subsequent lint cleanup
and validation recorded below. It is not an official Mintlayer or KDF release
and upstream acceptance is pending.

Transaction building, signing and private keys remain local to KDF. The normal
HTTPS configuration uses public APIs for transaction submission, observation,
confirmation checks and raw transaction recovery. It does not require a local
Mintlayer node RPC. Optional loopback `node_conf` support remains in the code.

## Implemented capabilities

- MINTLAYER protocol registration and legacy `enable` activation.
- Local identity/address derivation, expected genesis validation and balances.
- Spendable UTXOs, coin selection, change, native signing and fee estimation.
- Withdraw construction and optional signed-transaction submission.
- SwapOps maker/taker HTLC payment, spend and refund paths.
- Payment/DEX-fee validation, secret extraction and confirmation checks.
- Chain observation and swap recovery helpers using canonical `tx_hex` bytes.
- Mintlayer orderbook address integration.
- Ordered API reads and capability-aware raw-transaction endpoint selection.
- Bounded read retries for the observed taker-fee indexer visibility race.

## Runtime configuration

An ML coin entry used for trading must not retain the earlier
`"wallet_only": true` setting. Example:

```json
{
  "coin": "ML",
  "name": "Mintlayer",
  "fname": "Mintlayer",
  "mm2": 1,
  "wallet_only": false,
  "network": "mainnet",
  "decimals": 11,
  "required_confirmations": 2,
  "mature_confirmations": 0,
  "requires_notarization": false,
  "avg_blocktime": 120,
  "genesis_block_id": "2cf01f196066bb6f3a4856deb7999294ff520f633fe48e118e8044390e409870",
  "protocol": {"type": "MINTLAYER"},
  "links": {}
}
```

Activation example (API roots, without `/api/v2`; appended by the client):

```json
{
  "userpass": "<KDF_RPC_PASSWORD>",
  "method": "enable",
  "coin": "ML",
  "tx_history": false,
  "required_confirmations": 2,
  "client_conf": {
    "api_urls": [
      "https://mintlayer-api.pinco-labs.com",
      "https://mintlayer-api2.pinco-labs.com"
    ]
  }
}
```

The example adds API2 to the documented endpoint list. It is a proposed
configuration, not a claim that the commissioning swaps already used API2.

## Backend contract

A full backend must support the observation endpoints used by the client,
including genesis/tip, addresses/UTXOs, fee rate, blocks, transaction outputs
and transactions. In addition:

- `GET /api/v2/transaction/:id` must expose `tx_hex` with the canonical encoded
  signed transaction for swap recovery.
- `POST /api/v2/transaction` must accept the signed transaction payload; POST
  routes must be enabled by the API operator.
- KDF validates API transaction IDs, expected blocks and the canonical ID
  derived locally from raw signed bytes where recovery requires them.

The deployed API extension adds `tx_hex` locally to the transaction endpoint
using `hex(serialization::Encode::encode(&tx))`. The server contribution is a
separate review item from KDF; deployment does not imply upstream acceptance.

API1 and API2 are separately hosted/indexed Pinco Labs deployments using the
same API artifact. They provide operational redundancy, not independent
implementation diversity or trustless chain verification. A read-only API
without `tx_hex` is not a complete recovery backend.

## Retry and failover semantics

Reads can try the next endpoint after a capability miss, including a successful
transaction JSON response with no non-empty `tx_hex`.

Submission must be handled separately: ambiguous dispatch outcomes, such as a
transport error, timeout, server error or invalid success body, stop submission
instead of blindly trying another server. Explicitly handled HTTP rejection or
capability statuses have a separate fallback path. These classifications are
part of the requested review.

Taker-fee validation retries the precise all-endpoint `404 Transaction not found`
case and visible-but-unconfirmed transactions for a bounded window. It retries
GET/validation only. Txid mismatches and unrelated/semantic failures remain
errors. The visibility window is 60 seconds with 2-second polling in this layer.

## Golden compatibility reference

Transaction:
`19266863b91c8433e19fd6a6ecac827e2c64b1abd5c3a22cdf667255c8655828`

- Decoded signed transaction: 210 bytes.
- SHA256 of exact ASCII hex (no newline):
  `97e92615f20e0cdb85425e4cb1aa247e1fcb10b1791e1fc86577973ce8920486`
- SHA256 of decoded bytes:
  `5ffd6c18f496e22b31bbeca76b585723564a8a4e41569cd4be05e1dcfaba408a`

These SHA256 values are comparison fingerprints, not the Mintlayer txid.
Both public endpoints matched them on 2026-10-03. Byte parity was also checked
for the ML payments/spends and RC2 fee listed below. This does not itself prove
KDF runtime failover or accepted transaction submission on API2.

## Validation provenance

Project handovers report:

- ML-maker HTTPS-only swap: `5b898e72-b79c-43ca-a200-0150f124ffce`, before RC2.
- ML-taker HTTPS-only RC2 swap: `4abe88c3-7d2a-4f3f-9d42-4799956132dd`.
- Both peers reached Finished/success=true in each run.
- `node_conf` absent and legacy Mintlayer RPC 13030 absent during these runs.
- Final regression: 143 Mintlayer tests, 1 KDF/SDK adapter test, 5 SDK offline
  tests, 1 orderbook test, cargo check coins and mm2_main with utxo-walletconnect.

These are recorded results, not fresh execution claims. The swaps predate API2.
Public GET validation on 2026-10-03 verified both ML payment-to-spend links:

| Role | Payment | Spend |
| --- | --- | --- |
| ML maker | `a9ac92cc53d405c8558f41799b9f3d2ed90acf12a84b383946888d12ff4845d5` | `abde36db9d8edb8a395d0dd1a0f24bd674c6ec7259ea765aa2730cf65362a36d` |
| ML taker | `167816a3625219f10dfd85d905fd69aa134cdb7ad1a090c8d058f20c87fe1a9f` | `04d28f371edd279c091dc380a3adf2b368e11f6a9c9a9c21724d578955fae7b4` |

The public endpoints also returned the confirmed RC2 ML fee:
`56c7697fd1cc5b0f84b3665d8a23a74a24ba8890a05bd9b6b571aa07c4f7fd30`.

## KDF API2 live GET validation

On 2026-10-03, the review worktree based on
`072b38149bbed516c25dbd12c9e60e2330b82ca9`, with the new uncommitted
`mm2src/coins/tests/mintlayer_api2_https_live.rs` integration test, returned
**2 passed, 0 failed**, Cargo exit status 0.

- API2-only: expected genesis, golden signed bytes, both SHA256 fingerprints
  and the canonical SDK-derived transaction ID passed through the KDF
  production HTTP transport.
- GET failover: a loopback simulated primary returned HTTP 503; the client
  then retrieved and validated the golden transaction from real HTTPS API2.
  The test confirmed that the simulated primary was contacted.

These opt-in tests perform reads. They establish this specific GET failover
case, not a real API1 outage, a POST submission or a new swap through API2.
`--offline` restricts Cargo dependency resolution; these ignored live tests
still require network access to API2 when explicitly enabled.

Reproduce from the repository root:

```bash
cargo test --locked --offline -j 2 -p coins \
  --test mintlayer_api2_https_live -- \
  --ignored --nocapture --test-threads=1
```

Execution provenance (evidence captured at `2026-10-03T10:08:29Z`):

- Rust: `1.97.1 (8bab26f4f 2026-07-14)`.
- Cargo: `1.97.1 (c980f4866 2026-06-30)`.
- Executed test source SHA256, before formatting:
  `f1055af911b1b72dd0b3dd8e9d8355dea393add95f8c1dcd1369700ae03cf1ba`.
- Preserved test log SHA256:
  `fc25c1f787329f241413ac66ff355faf788d13060ec62544613dc1a662ef2004`.
- Subsequently formatted source SHA256 (`rustfmt --check` passed):
  `3a0f93bc760ce9704b39fdce76afd5673aa580a5982f5ed3dcfa6e97c5403d5d`.

The successful execution above refers to the preserved pre-format source;
formatting and execution provenance are recorded separately.

## PR candidate validation — 2026-10-06

The staged code candidate, before this documentation update, has Git tree
`56e948699246f5be89d0bd22bd98202dba3a40be`. This identifies repository
content, not a commit or a compiled binary.

Validation used Rust `1.97.1` and Cargo `1.97.1`, with locked offline
dependency resolution. The following checks passed after lint cleanup:

| Check | Result |
| --- | --- |
| `cargo test --locked --offline -j 2 -p coins --lib mintlayer:: -- --test-threads=2` | 141 passed, 0 failed |
| `mintlayer_kdf_sdk_key_adapter` integration target | 1 passed, 0 failed |
| `mintlayer_sdk_offline_transaction` integration target | 5 passed, 0 failed |
| Opt-in `mintlayer_api2_https_live` integration target | 2 passed, 0 failed |

The total is 149 passing tests across these selected runs, not the full
workspace suite. The 141 count refers specifically to the `mintlayer::`
library-test filter; historical handover counts above are separate records.

SDK integration checks can be reproduced with:

```bash
cargo test --locked --offline -j 2 -p coins \
  --test mintlayer_kdf_sdk_key_adapter \
  --test mintlayer_sdk_offline_transaction -- --test-threads=2
```

The API2 command in the preceding section was rerun successfully against the
formatted test source and updated candidate. Golden bytes, both SHA256
fingerprints, canonical txid, API2-only retrieval and simulated-primary
HTTP 503 GET failover passed. No new mainnet swap or POST validation is
claimed by these runs.

Lint cleanup removed redundant conversions and copies, simplified conditions,
used the KDF executor in synchronous tests and awaited compatible futures in
asynchronous tests, and moved the Transaction implementation before tests.
Two offline planner functions retain explicit inputs with narrowly scoped,
commented `too_many_arguments` allowances.

Checks still open:

- `cargo clippy --locked --offline -j 2 -p coins --all-targets --no-deps -- -D warnings`
  exited 101. Its emitted diagnostics contained no Mintlayer file locations
  after cleanup; it reported 36 errors for the library and 38 for the library
  test build in other modules. These overlapping counts must not be summed.
  This is not a passing Clippy gate, nor proof that every integration target
  completed linting. A baseline-only Clippy comparison has not been run.
- Workspace formatting check exited 1 on import ordering in
  `mm2src/derives/enum_derives/src/from_stringify.rs`, verified unchanged from
  the upstream dev base. The three edited Mintlayer source files were formatted.
- The full required workspace/Docker suite, all-feature checks and
  multi-platform/native/WASM CI have not been established for this candidate.
- Build output still reports the unused Trezor `StaticAddressBytes` alias and
  the `nom 6.1.2` future-incompatibility notice.

Preserved local logs and candidate patches are indexed by the preparation
evidence manifest. The unit-test log SHA256 is
`887c09eae8b30f6d66dd254804e65dc01b1b05ec223170dab7e16f0619803372`;
the post-cleanup strict Clippy log SHA256 is
`8fd8a3d3fa35aff60d241167e4fefbdc911c9d39f19be6dd2177d8bfd6c3b809`.
Publication of sanitized supporting logs remains a separate preparation task.

## Current limitations

- Scope is the V1 swap path; no Mintlayer-specific V2/TPU or watcher support
  is claimed. Upstream guidance favors V2 for new features, requiring review.
- No transaction-history sync, message signing/verification or address conversion.
- Generic get_raw_transaction/get_tx_hex_by_hash RPCs remain unimplemented;
  internal swap recovery via `tx_hex` is a separate implemented path.
- Withdrawal max, alternate from address, custom fee and memo are unsupported.
- Mintlayer Trezor and WalletConnect key policies are unsupported.
- Multi-platform CI, runtime interruption/recovery and live timed-refund coverage
  are not established by the public byte-parity test.

## Dependencies to review

- mintlayer-sdk: `d462098043d6962a76b405f86cf5ab19a6f104a4`, features `crypto`, `node`.
- Workspace parity-scale-codec patch:
  `5021525697edc0661591ebc71392c48d950a10b0`.
- Cargo.lock additions/replacements and native/WASM feature compatibility.

## Remaining plan

- [x] Native transaction and SwapOps HTLC paths implemented.
- [x] HTTPS-only mainnet swaps recorded with ML in maker and taker roles.
- [x] Indexer visibility regression covered and RC2 reverse swap recorded.
- [x] API2 commissioned as a separate golden validation target.
- [x] Public API1/API2 golden and swap-transaction byte parity verified.
- [x] Prepare a dedicated PR candidate from upstream dev per contribution rules.
- [ ] Confirm V1 scope and acceptance requirements with maintainers.
- [x] Rerun selected Mintlayer unit, SDK and API2 GET tests after lint cleanup.
- [ ] Review current documentation and API1/API2 configuration examples.
- [x] Capture KDF API2-only and simulated-primary HTTP 503 GET failover evidence.
- [ ] Link original sanitized regression/runtime audit logs to the candidate.
- [ ] Run required CI, formatter, lint and dependency checks for the chosen base.
- [ ] Agree commit organization and submit the reviewed KDF candidate.
- [ ] Prepare the separate canonical Mintlayer API `tx_hex` contribution.
- [ ] Obtain maintainer review and release approval.
