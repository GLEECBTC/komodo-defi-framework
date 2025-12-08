# Plan: Docker tests refactor & CI split

**Owner:** @Omer  
**Status:** Draft  
**Scope:** Docker-based integration tests (UTXO, ETH, QRC20/Qtum, SLP, Tendermint/Cosmos, ZCoin, Sia, watchers)  
**Entry point:** Linked from `AGENTS.md` → `plans/docker_tests.md`

---

## 1. Goals

1. Stabilize the new Docker infra (Compose/Metadata/Reuse) and fix all correctness issues.
2. Split the monolithic `docker-tests` job into smaller **functional** jobs:
   - Ordermatching
   - Swaps
   - Watchers
   - Chain-specific suites (QRC20, Tendermint, ZCoin, SLP, ETH, Sia)
3. Shorten feedback loop: each job should be reasonably fast and runnable in isolation.
4. Preserve **testcontainers** semantics as the baseline:
   - New modes should behave like the old flow from the perspective of tests.
5. Keep code churn low:
   - Prefer cfg-gating, helpers, and clear grouping over massive file moves.

### 1.1 Non-goals (for now)

- Rewriting tests into a different framework.
- Changing swap / ordermatch implementation logic.
- Removing testcontainers entirely.
- Perfect partitioning; the goal is a good, maintainable split, not theoretical purity.

---

## 2. Current state (snapshot)

### 2.1 Environment modes

`docker_tests_main.rs` currently supports three modes:

- `Testcontainers` (legacy / default)
   - Tests spin up containers via `testcontainers`.
- `ComposeInit`
   - Assumes docker-compose is already running.
   - Initializes nodes (contracts, tokens, IBC, etc.) and writes `DockerEnvMetadata`.
- `ReuseMetadata`
   - Loads `DockerEnvMetadata` and reuses running containers, performing basic health checks.

**Note:** `ComposeInit` always saves metadata to `.docker/container-runtime/docker_env_state.json` (via `default_path()`); `ReuseMetadata` is only entered when `KDF_DOCKER_ENV_STATE_FILE` is set (there is no current default auto-load).

New infra:

- `DockerEnvMetadata`:
   - Captures RPC URLs, ports, conf paths, contract addresses, token IDs, etc.
- `docker_tests::helpers::eth`:
   - `geth_account()`, `swap_contract()`, `watchers_swap_contract()`, `erc20_contract_checksum()`, `eth_coin_with_random_privkey`, `fill_eth_erc20_with_private_key`, etc.

Known issues / risks:

- Geth health check uses the static `GETH_RPC_URL` rather than `metadata.geth.rpc_url`.
- Qtum compose setup writes `qtum.conf` to a temp dir (`temp_dir()`); UTXO uses a stable daemon data dir (`coin_daemon_data_dir()`). Standardize Qtum to a stable path.
- Health checks mostly just test TCP connectivity; they do not validate that contracts are deployed as metadata claims.
- `swap_watcher_tests::test_two_watchers_spend_maker_payment_eth_erc20` has assertions that are effectively no-ops (comparing values to themselves: `assert_eq!(watcher2_eth_balance_after, watcher2_eth_balance_after)`).
- Some helpers assume fixed compose container names (e.g. `kdf-qtum`), which is brittle.
- Metadata file path handling is duplicated and not centralized.

### 2.2 Test modules & jobs

Already split CI jobs:

- `docker-tests-eth` → `eth_docker_tests`
- `docker-tests-slp` → `slp_tests`
- `docker-tests-sia` → `sia_docker_tests`

Main `docker-tests` job still runs:

- `docker_tests_inner`
- `docker_ordermatch_tests`
- `swap_proto_v2_tests`
- `swaps_file_lock_tests`
- `swaps_confs_settings_sync_tests`
- `swap_watcher_tests`
- `qrc20_tests`
- `tendermint_tests`
- `z_coin_docker_tests`
- `swap_tests`
- Sia short-locktime tests (via `sia_tests`)
- `integration_tests_common::test_mm_start`

This currently runs ~200+ tests in ~1800 seconds.

### 2.3 Desired grouping (functional)

We want to group tests by behavior and feature area:

- **Ordermatching**
   - Orderbook, setprice, my_orders, conf settings, min/max volume, etc.
- **Swaps**
   - Swap protocol v1/v2, file locks, conf synchronization.
- **Watchers**
   - Watcher flows, refunds, spends, restart behavior, and watcher rewards.
- **Chain-specific suites**
   - QRC20/Qtum
   - Tendermint/Cosmos
   - ZCoin
   - SLP
   - ETH
   - Sia
- **Cross-chain integration**
   - A small set of “everything together” swaps (e.g. SLP ↔ UTXO ↔ QRC20 ↔ ETH).

---

## 3. Constraints & invariants

- Testcontainers mode must continue to work exactly as before (or strictly better).
- Metadata-based reuse mode must **fail fast** when state is stale or inconsistent:
   - Missing conf files
   - Wrong contract bytecode
   - Broken RPCs
- Tests must not depend on individual dev-local docker quirks or hostnames.
- Use **minimal movement**:
   - We prefer `#[cfg(feature = "...")]` and helper modules over moving test functions around arbitrarily.
- When tests logically belong to multiple categories (e.g. watchers tests touch UTXO + ETH), we group them under their primary behavior (watchers).

---

## 4. Phased plan

Each phase should be implemented in one or more small PRs.

---

### Phase 1 – Stabilize environment & fix bugs

**Goal:** Make Compose/Metadata/Reuse paths correct, robust, and aligned with testcontainers semantics.

#### 4.1.1 Correct Geth health check

**File:** `mm2src/mm2_main/tests/docker_tests_main.rs`

- [x] In `validate_nodes_health()`, replace use of `GETH_WEB3` for the health probe with a new local `Web3` constructed from `metadata.geth.rpc_url`. Leave `GETH_WEB3` alone for now.
- [ ] Optional (separate PR): Add a helper `get_web3_from_metadata()` and use it only in health checks. Reinitializing the global `GETH_WEB3` can wait.
- [x] If metadata has no Geth entry, surface a clear error:
   - e.g. "Geth RPC URL missing in metadata; re-run docker env init."

#### 4.1.2 Qtum conf path stability in Compose

**File:** `mm2src/mm2_main/tests/docker_tests_main.rs` (`setup_qtum_conf_for_compose`)

**Note:** UTXO already uses stable paths via `coin_daemon_data_dir()`. Only Qtum uses `temp_dir()`.

- [x] Replace `temp_dir()` in `setup_qtum_conf_for_compose` with a stable, repo-relative path. Two safe choices:
   - `coin_daemon_data_dir("QTUM", true)/qtum.conf` (consistent with UTXO), or
   - `project_root/.docker/container-runtime/qtum/qtum.conf`
- [x] Store the chosen `qtum.conf` path into `DockerEnvMetadata.qtum.conf_path` when initializing.
- [x] In Reuse mode, assert the conf path exists:
   - If missing → "Qtum config missing at X; metadata is stale. Re-run docker env init."

#### 4.1.3 Single source of truth for metadata file path (non-breaking)

**File:** `mm2src/mm2_main/tests/docker_tests/docker_env_metadata.rs`

- [x] Keep `get_metadata_file_path()` returning `Option<PathBuf>` from `KDF_DOCKER_ENV_STATE_FILE`.
- [x] Add `fn get_or_default_metadata_path() -> PathBuf` that returns the env path if set, else `default_path()`.
- [x] Use `get_or_default_metadata_path()` when saving the metadata (ComposeInit).
- [x] Keep `ReuseMetadata` gated by `KDF_DOCKER_ENV_STATE_FILE` for now (no behavior change, but the writer side is centralized).

#### 4.1.4 Semantic health checks (minimal slice)

**File:** `docker_tests_main.rs` (`validate_nodes_health`)

Add semantic checks beyond simple port checks:

- [x] Geth: call `eth_getCode` for each address in metadata.geth (`erc20_contract`, `swap_contract`, `watchers_swap_contract`, `erc721_contract`, `erc1155_contract`, `nft_maker_swap_v2`) and assert non-empty code. Start with at least `erc20_contract` and `swap_contract`.
- [x] Leave Qtum/SLP/Cosmos checks for a follow-up PR.
- [x] If any fail, treat metadata as invalid:
   - Clear, actionable error about reinitializing the environment.

#### 4.1.5 Fix watcher test correctness (tautology)

**File:** `swap_watcher_tests.rs` (`test_two_watchers_spend_maker_payment_eth_erc20`)

- [x] Replace the no-op asserts (lines 1223-1228) with:
   ```rust
   let w1_gain = watcher1_eth_balance_after > watcher1_eth_balance_before;
   let w2_gain = watcher2_eth_balance_after > watcher2_eth_balance_before;
   assert_ne!(w1_gain, w2_gain, "exactly one watcher must receive the reward");
   ```
- [x] Keep `#[ignore]` if the test is heavy; assertions should still be correct when it runs.

#### 4.1.6 Container name constants

**File:** `docker_tests_common.rs` (or new `helpers/env.rs`)

- [x] Lift compose container names into constants:
   - `KDF_QTUM_SERVICE`, `KDF_MYCOIN_SERVICE`, `KDF_MYCOIN1_SERVICE`, `KDF_FORSLP_SERVICE`, `KDF_ZOMBIE_SERVICE`, `KDF_IBC_RELAYER_SERVICE`
   - Note: Used `_SERVICE` suffix instead of `_NAME` for clarity (these are service names, not container names)
- [x] Use them in `setup_qtum_conf_for_compose()`, `setup_utxo_conf_for_compose()`, `prepare_ibc_channels_compose()`, `wait_until_relayer_container_is_ready_compose()`.
- [x] Ensure setup functions do not break if the compose project name changes.
   - Added `resolve_compose_container_id()` helper that uses label-based lookup (`com.docker.compose.service`) with fallback to `kdf-{service}` name lookup for compatibility.

#### 4.1.7 ETH helpers adoption & cleanup

- [x] Grep tests for:
   - Raw hex contract addresses
   - Inlined Geth chain IDs or ABIs
- [x] Replace with calls into `helpers::eth`:
   - `swap_contract()`, `watchers_swap_contract()`, `erc20_contract()`, `erc20_contract_checksum()`, etc.
   - Added `swap_contract_checksum()` and `watchers_swap_contract_checksum()` helpers for common checksum formatting pattern
   - Replaced 23 occurrences of `format!("0x{}", hex::encode(swap_contract()))` pattern with `swap_contract_checksum()`
   - Added test address constants in `docker_tests_inner.rs`: `TEST_ARBITRARY_SWAP_ADDR_1`, `TEST_ARBITRARY_SWAP_ADDR_2`, `TEST_WITHDRAW_DEST_ADDR`, `TEST_WITHDRAW_DEST_ADDR_INVALID_CHECKSUM`
- [x] Verify watchers tests consistently use `watchers_swap_contract()` (or equivalent dedicated helper).
   - Updated `swap_watcher_tests.rs` to use `watchers_swap_contract_checksum()` helper
   - Removed unused `checksum_address` import
- [x] Delete any duplicated ETH helper logic from other modules.
   - No duplicated logic found; all ETH helper usage is now centralized

---

### Phase 2 – Introduce minimal gating features and keep code movement low

**Goal:** Make suites selectable at compile time, mirroring the CI split. Prefer cfg-gating over moving test functions.

#### 4.2.1 Helpers layout

Under `mm2src/mm2_main/tests/docker_tests/`:

- `helpers/mod.rs`
- `helpers/env.rs` – metadata loading, health checks, mode selection.
- `helpers/utxo.rs` – UTXO node helpers (MYCOIN/MYCOIN1, FORSLP, ZOMBIE).
- `helpers/eth.rs` – existing ETH helpers moved/refined.
- `helpers/qrc20.rs` – Qtum/QRC20-specific helpers.
- `helpers/tendermint.rs` – Tendermint/Cosmos-specific helpers.
- `helpers/zcoin.rs` – ZCoin-specific helpers (sapling cache, etc.).

Actions:

- [x] Move shared logic out of `docker_tests_common.rs` into the appropriate helpers while keeping a minimal "root" `docker_tests_common` that just wires things together.
   - Created all helper modules with proper organization
   - `docker_tests_common.rs` now re-exports from helpers
   - Test modules updated to import from helpers directly where needed
- [x] Ensure no test module depends on raw docker call patterns; always go through helpers.
  - **Completed:** Moved `qtum_docker_node()` function and `QTUM_REGTEST_DOCKER_IMAGE` constants from `qrc20_tests.rs` to `helpers/qrc20.rs`
  - All raw Docker patterns (`Command::new("docker")`) now encapsulated in helper modules
  - Pattern matches existing helpers (`utxo.rs`, `zcoin.rs`, `eth.rs`)
  - Verified: No test modules contain raw Docker calls

#### 4.2.1.1 Module structure cleanup (completed)

**Status:** ✅ Completed

**Phase 1 - Option B (completed earlier):**
- Removed all `pub use` re-exports from `docker_tests_common.rs` (~90 lines)
- Kept `trade_base_rel` function in `docker_tests_common.rs` as cross-cutting integration test helper
- Updated all test files to use explicit imports from helper modules

**Phase 2 - Full reorganization (completed):**
- Deleted `docker_tests_common.rs` entirely
- Created new helper modules for better separation of concerns:
  - `helpers/swap.rs` - Cross-chain swap orchestration (`trade_base_rel`)
  - `helpers/sia.rs` - Sia-specific helpers (moved from `env.rs`)
  - `helpers/docker_ops.rs` - `CoinDockerOps` trait (extracted from `utxo.rs`)
- Updated `helpers/env.rs` to contain only generic environment setup (contexts, service constants, `DockerNode` type)
- Updated `helpers/utxo.rs` to import `CoinDockerOps` from `docker_ops`
- Updated `helpers/zcoin.rs` to import `CoinDockerOps` from `docker_ops`
- Updated all imports:
  - `docker_tests_main.rs` - imports from `helpers::sia`, `helpers::docker_ops`
  - `sia_tests/utils.rs` - imports from `helpers::sia`
  - `qrc20_tests.rs`, `swap_tests.rs`, `docker_tests_inner.rs` - imports from `helpers::swap`

**Final module structure:**
```
helpers/
├── docker_ops.rs  # CoinDockerOps trait (shared by utxo, zcoin)
├── env.rs         # MM_CTX, service constants, DockerNode, random_secp256k1_secret
├── eth.rs         # Geth/ERC20 helpers
├── mod.rs         # Module index
├── qrc20.rs       # Qtum/QRC20 helpers
├── sia.rs         # Sia helpers (SIA_RPC_PARAMS, sia_docker_node)
├── swap.rs        # Cross-chain swap orchestration (trade_base_rel)
├── tendermint.rs  # Tendermint/Cosmos helpers
├── utxo.rs        # UTXO coin helpers (MYCOIN, BCH/SLP)
└── zcoin.rs       # ZCoin/Zombie helpers
```

**Completed Tasks:**
- [x] Decide on module organization approach → **Full reorganization implemented**
- [x] Update test files to import from specific helpers
- [x] Move `trade_base_rel` to `helpers/swap.rs`
- [x] Extract `CoinDockerOps` to `helpers/docker_ops.rs`
- [x] Move Sia helpers to `helpers/sia.rs`
- [x] Delete `docker_tests_common.rs`
- [x] Run clippy with `-D warnings` to ensure no warnings

#### 4.2.2 Behavioral labeling of tests (no big moves yet)

Within `docker_tests_inner.rs`:

- Mark / group logically (by comments + internal sections):

1. **Ordermatching / wallet behavior:**

   - `order_should_be_cancelled_when_entire_balance_is_withdrawn`
   - `order_should_be_updated_when_balance_is_decreased_*`
   - `test_order_should_be_updated_when_matched_partially`
   - `test_buy_min_volume`, `test_sell_min_volume`
   - `test_setprice_min_volume_dust`, `test_sell_min_volume_dust`
   - `test_set_price_max`
   - `test_orderbook_depth`
   - `test_my_orders_response_format`, `test_my_orders_after_matched`
   - `test_set_price_must_save_order_to_db`
   - `test_set_price_response_format`
   - `test_set_price_conf_settings`, `test_buy_conf_settings`, `test_sell_conf_settings`

2. **Swaps / balances (UTXO-only):**

   - `test_search_for_swap_tx_spend_*`
   - `test_for_non_existent_tx_hex_utxo`
   - `test_one_hundred_maker_payments_in_a_row_native`
   - `test_match_and_trade_setprice_max`
   - `test_get_max_taker_vol*`, `test_get_max_maker_vol*`
   - `test_trade_preimage_*`, `test_taker_trade_preimage`, `test_maker_trade_preimage`
   - `test_max_taker_vol_swap`
   - `test_buy_when_coins_locked_by_other_swap`, `test_sell_when_coins_locked_by_other_swap`
   - `test_fill_or_kill_taker_order_should_not_transform_to_maker`
   - `test_gtc_taker_order_should_transform_to_maker`
   - `test_trade_preimage_not_sufficient_balance`, `test_trade_preimage_additional_validation`, `test_trade_preimage_legacy`
   - `test_trade_base_rel_mycoin_mycoin1_coins`, `test_trade_base_rel_mycoin_mycoin1_coins_burnkey_as_alice`
   - `test_utxo_merge`, `test_utxo_merge_max_merge_at_once`
   - `test_consolidate_utxos_rpc`, `test_fetch_utxos_rpc`
   - `test_withdraw_not_sufficient_balance`
   - `test_locked_amount`
   - `swaps_should_stop_on_stop_rpc`
   - `test_swaps_should_kick_start_if_process_was_killed` (from swaps_file_lock_tests)
   - etc.

3. **Cross-chain / ETH / QRC20 / watchers-adjacent:**

   - `test_match_utxo_with_eth_taker_sell`
   - `test_match_utxo_with_eth_taker_buy`
   - `test_trade_base_rel_eth_erc20_coins`
   - `test_withdraw_and_send_eth_erc20`
   - `test_withdraw_and_send_hd_eth_erc20`
   - `test_enable_eth_coin_with_token_then_disable`
   - `test_enable_eth_coin_with_token_without_balance`
   - `test_enable_eth_coin_with_token_without_balance`
   - `test_platform_coin_mismatch`
   - `test_eth_swap_contract_addr_negotiation_same_fallback`
   - `test_eth_swap_negotiation_fails_maker_no_fallback`
   - `test_approve_erc20`
   - `test_peer_time_sync_validation`

This categorization is just a preparation step and will guide what goes into which CI job in Phase 3.

#### 4.2.3 `mod.rs` gating

**Status:** ✅ Completed

**File:** `mm2src/mm2_main/tests/docker_tests/mod.rs`

**New feature flags added to `Cargo.toml`:**
- `docker-tests-qrc20 = ["run-docker-tests"]` - QRC20 coin tests
- `docker-tests-tendermint = ["run-docker-tests"]` - Tendermint/IBC coin tests
- `docker-tests-zcoin = ["run-docker-tests"]` - ZCoin/Zombie coin tests
- `docker-tests-swaps-utxo = ["run-docker-tests"]` - UTXO swap protocol tests
- `docker-tests-watchers = ["run-docker-tests"]` - Watcher node tests
- `docker-tests-ordermatch = ["run-docker-tests"]` - Orderbook and matching tests

**Module gating implemented:**

```rust
// ORDERMATCHING TESTS
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-ordermatch"))]
mod docker_ordermatch_tests;

// SWAP TESTS
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-eth"))]
mod docker_tests_inner;

#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-swaps-utxo"))]
mod swap_proto_v2_tests;
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-swaps-utxo"))]
mod swaps_confs_settings_sync_tests;
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-swaps-utxo"))]
mod swaps_file_lock_tests;

// BCH-SLP swap tests - main docker job only (exclusion logic)
#[cfg(all(feature = "run-docker-tests", not(feature = "docker-tests-slp"), ...))]
mod swap_tests;

// WATCHER TESTS
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-watchers"))]
mod swap_watcher_tests;

// COIN-SPECIFIC TESTS
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-eth"))]
mod eth_docker_tests;
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-qrc20"))]
pub mod qrc20_tests;
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-sia"))]
mod sia_docker_tests;
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-slp"))]
mod slp_tests;
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-tendermint"))]
mod tendermint_tests;
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-zcoin"))]
mod z_coin_docker_tests;
```

**Additional cleanup:**
- Moved `QtumDockerOps` from `qrc20_tests.rs` to `helpers/qrc20.rs`
- Helper modules gated on `run-docker-tests` (with `env` and `eth` also available for sepolia tests)

**All feature combinations verified to compile successfully.**

#### 4.2.4 Test placement audit & file splitting (IN PROGRESS)

**Goal:** Ensure tests are in the correct files and split large files that test multiple concerns.

**Baseline test count (monolithic docker-tests job):**
```
test result: ok. 235 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 1864.36s
```
After plan completion, the sum of all split jobs must equal this baseline.

**Status:** Partial implementation - UTXO swap tests extracted to new module.

**Completed tasks:**
- [x] Created `utxo_swaps_v1_tests.rs` - Extracted UTXO-only swap tests from `docker_tests_inner.rs`:
  - Swap spend/refund mechanics tests (`test_search_for_swap_tx_spend_*`)
  - Non-existent tx hex test (`test_for_non_existent_tx_hex_utxo`)
  - Payment throughput test (`test_one_hundred_maker_payments_in_a_row_native`)
  - Max taker/maker volume tests (`test_get_max_taker_vol*`, `test_get_max_maker_vol*`)
  - UTXO merge tests (`test_utxo_merge*`, `test_consolidate_utxos_rpc`, `test_fetch_utxos_rpc`)
  - Withdraw balance tests (`test_withdraw_not_sufficient_balance`)
  - Locked amount tests (`test_locked_amount`)
  - Swap lifecycle tests (`swaps_should_stop_on_stop_rpc`, `test_fill_or_kill_*`, `test_gtc_*`)
  - Buy/sell with locked coins tests (`test_buy_when_coins_locked_*`, `test_sell_when_coins_locked_*`)
  - UTXO-only trade tests (`test_trade_base_rel_mycoin_mycoin1_*`, `test_buy_max`)
- [x] Added module entry in `mod.rs` gated by `docker-tests-swaps-utxo`
- [x] Verified compilation with `cargo check -p mm2_main --features run-docker-tests,docker-tests-swaps-utxo`
- [x] Verified no clippy warnings with `-D warnings`

**Remaining tasks:**
- [ ] Extract remaining UTXO-only tests from `docker_tests_inner.rs` to `utxo_swaps_v1_tests.rs`:
  - `test_match_and_trade_setprice_max`
  - `test_max_taker_vol_swap`
  - `test_trade_preimage_*` (6 tests: `test_taker_trade_preimage`, `test_maker_trade_preimage`, `test_trade_preimage_not_sufficient_balance`, `test_trade_preimage_additional_validation`, `test_trade_preimage_legacy`, and related)
- [ ] Audit each test module to verify tests are correctly placed:
  - Check if tests match their feature gate (e.g., ETH tests in `docker-tests-eth` gated module)
  - Identify tests that should be moved to different feature categories
- [ ] Complete splitting of `docker_tests_inner.rs`:
  - Extract ordermatching tests to `ordermatch_inner_tests.rs` (gated by `docker-tests-ordermatch`)
  - Extract ETH-specific tests to `eth_inner_tests.rs` (keep in `docker-tests-eth`)
  - Remove extracted tests from `docker_tests_inner.rs` to avoid duplication
- [ ] Consider splitting other large files:
  - `eth_docker_tests.rs` - May benefit from splitting coin-specific vs swap tests
  - `tendermint_tests.rs` - Contains activation, staking, IBC, and swap tests
- [ ] Update feature gates after test movements to ensure correct CI job assignment

**Future cleanup (post-plan):**
- [ ] Review `utxo_swaps_v1_tests.rs` for tests that don't belong in swaps category:
  - UTXO merge tests may belong in a separate UTXO maintenance module
  - Some tests may better fit in ordermatching category
  - Reorganize based on actual test purpose vs. chain dependency

#### 4.2.5 Runner: start only what's needed (keep env flags)

**File:** `docker_tests_main.rs`

The runner already honors `_KDF_NO_*_DOCKER` env vars. For now, don't add compile-time logic—CI will pass these envs to disable unused nodes.

Later, you can add `#[cfg(feature = "...")]` blocks around image pulling to slightly speed startup, but this isn't required to split jobs.

---

### Phase 3 – CI: add functional jobs (Compose mode)

**Goal:** Break the monolithic docker tests job into parallel jobs grouped by behavior. Keep each new job small and independent. All jobs use Compose mode (`KDF_DOCKER_COMPOSE_ENV=1`) to enable sharing containers with other tests (e.g., WASM tests).

#### 4.3.1 CI job matrix & features

**Current state:** Only `docker-tests-eth`, `docker-tests-slp`, and `docker-tests-sia` feature flags exist today. The other flags listed below must be introduced and wired in this phase.

Add new feature flags in `mm2_main/Cargo.toml`:

- `docker-tests-eth` (existing)
- `docker-tests-slp` (existing)
- `docker-tests-sia` (existing)
- `docker-tests-ordermatch` (added in Phase 2)
- `docker-tests-swaps-utxo` (added in Phase 2) - UTXO-only swap tests
- `docker-tests-watchers` (added in Phase 2)
- `docker-tests-qrc20` (added in Phase 2)
- `docker-tests-tendermint` (added in Phase 2)
- `docker-tests-zcoin` (added in Phase 2)
- `docker-tests-integration` (to be added, cross-chain heavy flows)

CI jobs mapping:

| Job                        | Feature flag              | Primary content                                           |
|---------------------------|---------------------------|-----------------------------------------------------------|
| `docker-tests-eth`        | `docker-tests-eth`        | ETH/ERC20/721/1155 tests                                 |
| `docker-tests-slp`        | `docker-tests-slp`        | SLP-only tests                                           |
| `docker-tests-sia`        | `docker-tests-sia`        | Sia client & DSIA/Mycoin swaps                           |
| `docker-tests-ordermatch` | `docker-tests-ordermatch` | Ordermatching & wallet/order lifecycle                   |
| `docker-tests-swaps-utxo` | `docker-tests-swaps-utxo` | UTXO swap protocol v1/v2, file locking, conf sync        |
| `docker-tests-watchers`   | `docker-tests-watchers`   | Watcher flows and rewards                                |
| `docker-tests-qrc20`      | `docker-tests-qrc20`      | Qtum/QRC20-specific tests                                |
| `docker-tests-tendermint` | `docker-tests-tendermint` | Cosmos/Tendermint/IBC tests                              |
| `docker-tests-zcoin`      | `docker-tests-zcoin`      | ZCoin (Zombie) tests                                     |
| `docker-tests-integration`| `docker-tests-integration`| Cross-chain, multi-chain swap integration scenarios      |

#### 4.3.2 Assign modules to jobs

**Ordermatching (`docker-tests-ordermatch`)**

- `docker_ordermatch_tests::*` (except the Zombie-specific test below).
- From `docker_tests_inner.rs` (order-related subset):
   - `order_should_be_cancelled_when_entire_balance_is_withdrawn`
   - `order_should_be_updated_when_balance_is_decreased_*`
   - `test_order_should_be_updated_when_matched_partially`
   - `test_buy_min_volume`, `test_sell_min_volume`
   - `test_setprice_min_volume_dust`, `test_sell_min_volume_dust`
   - `test_set_price_max`
   - `test_orderbook_depth`
   - `test_my_orders_response_format`, `test_my_orders_after_matched`
   - `test_set_price_must_save_order_to_db`
   - `test_set_price_response_format`
   - `test_set_price_conf_settings`, `test_buy_conf_settings`, `test_sell_conf_settings`

**Swaps (`docker-tests-swaps-utxo`)**

- `utxo_swaps_v1_tests::*` (extracted from `docker_tests_inner.rs`)
- `swap_proto_v2_tests::*`
- `swaps_file_lock_tests::*`
- `swaps_confs_settings_sync_tests::*`
- Tests include (UTXO-only swap tests):
   - `test_search_for_swap_tx_spend_*`
   - `test_for_non_existent_tx_hex_utxo`
   - `test_one_hundred_maker_payments_in_a_row_native`
   - `test_match_and_trade_setprice_max`
   - `test_get_max_taker_vol*`, `test_get_max_maker_vol*`
   - `test_trade_preimage_*`, `test_taker_trade_preimage`, `test_maker_trade_preimage`
   - `test_max_taker_vol_swap`
   - `test_buy_when_coins_locked_by_other_swap`, `test_sell_when_coins_locked_by_other_swap`
   - `test_fill_or_kill_taker_order_should_not_transform_to_maker`
   - `test_gtc_taker_order_should_transform_to_maker`
   - `test_trade_preimage_not_sufficient_balance`, `test_trade_preimage_additional_validation`, `test_trade_preimage_legacy`
   - `test_trade_base_rel_mycoin_mycoin1_coins`, `test_trade_base_rel_mycoin_mycoin1_coins_burnkey_as_alice`
   - `test_utxo_merge`, `test_utxo_merge_max_merge_at_once`
   - `test_consolidate_utxos_rpc`, `test_fetch_utxos_rpc`
   - `test_withdraw_not_sufficient_balance`
   - `test_locked_amount`
   - `swaps_should_stop_on_stop_rpc`

**Watchers (`docker-tests-watchers`)**

- `swap_watcher_tests::*`

**QRC20 (`docker-tests-qrc20`)**

- `qrc20_tests::*` (all QRC20/Qtum-only tests).

**Tendermint (`docker-tests-tendermint`)**

- `tendermint_tests::*` including nested `swap` module:
   - `swap_nucleus_with_doc`
   - `swap_nucleus_with_eth`
   - and the Tendermint balance/withdraw/IBC/delegation/validators/tx history tests.

**ZCoin (`docker-tests-zcoin`)**

- `z_coin_docker_tests::*`
- `docker_ordermatch_tests::test_zombie_order_after_balance_reduce_and_mm_restart`

**Integration (`docker-tests-integration`)**

- `swap_tests::trade_test_with_maker_slp`
- `swap_tests::trade_test_with_taker_slp`
- Optionally: a very small curated subset of cross-chain tests from `docker_tests_inner` if coverage is missing elsewhere.

#### 4.3.3 Runner profiles per job

In `docker_tests_main.rs`, adjust container startup based on enabled features:

- **Ordermatching/Swaps only:**
   - Start UTXO containers (`MYCOIN`, `MYCOIN1`) and minimum deps.
- **Watchers:**
   - Start UTXO + Geth (no Cosmos/Sia/etc).
- **QRC20:**
   - Start Qtum/QRC20 only (and UTXO if needed for some tests).
- **Tendermint:**
   - Start Cosmos nodes (Nucleus, Atom) and relayer; prepare IBC channels.
- **ZCoin:**
   - Start Zombie node and ensure zcash params are present.
- **Integration:**
   - Start everything required (UTXO, SLP, QRC20, ETH, Cosmos, Sia, etc).

Mechanics:

- Use `_KDF_NO_*_DOCKER` env vars to disable unrelated groups per job.
- Use feature flags to gate test modules:
   - If `docker-tests-watchers` is not enabled, `swap_watcher_tests` should not even compile into that run.

#### 4.3.4 CI wiring (GitHub Actions)

Follow the existing pattern from `docker-tests-eth`, `docker-tests-slp`, and `docker-tests-sia` jobs in `.github/workflows/test.yml`.

**Pattern for new jobs:**

```yaml
docker-tests-<suite>:
  timeout-minutes: <appropriate timeout>
  runs-on: ubuntu-latest
  env:
    BOB_PASSPHRASE: ${{ secrets.BOB_PASSPHRASE_LINUX }}
    BOB_USERPASS: ${{ secrets.BOB_USERPASS_LINUX }}
    ALICE_PASSPHRASE: ${{ secrets.ALICE_PASSPHRASE_LINUX }}
    ALICE_USERPASS: ${{ secrets.ALICE_USERPASS_LINUX }}
    TELEGRAM_API_KEY: ${{ secrets.TELEGRAM_API_KEY }}
  steps:
    - uses: actions/checkout@v3
    - name: Install toolchain
      run: |
        rustup toolchain install stable --no-self-update --profile=minimal
        rustup default stable

    - name: Install build deps
      uses: ./.github/actions/deps-install
      with:
        deps: ('protoc')

    - name: Build cache
      uses: ./.github/actions/build-cache

    # Optional: Fetch zcash params (for UTXO/ZCoin tests)
    - name: Fetch zcash params
      run: wget -O - https://raw.githubusercontent.com/KomodoPlatform/komodo/v0.8.1/zcutil/fetch-params-alt.sh | bash

    # Optional: Prepare environment (if Cosmos/IBC needed)
    - name: Prepare docker test environment
      run: ./scripts/ci/docker-test-nodes-setup.sh

    - name: Start docker nodes
      run: |
        docker compose -f .docker/test-nodes.yml --profile <profile> up -d
        echo "Waiting for containers..."
        sleep <wait_time>
        docker compose -f .docker/test-nodes.yml ps

    - name: Test
      env:
        KDF_DOCKER_COMPOSE_ENV: "1"
        _KDF_NO_UTXO_DOCKER: "1"  # Disable unused container groups
        _KDF_NO_SLP_DOCKER: "1"
        _KDF_NO_QTUM_DOCKER: "1"
        _KDF_NO_ETH_DOCKER: "1"
        _KDF_NO_COSMOS_DOCKER: "1"
        _KDF_NO_ZOMBIE_DOCKER: "1"
        _KDF_NO_SIA_DOCKER: "1"
      run: |
        cargo test --test 'docker_tests_main' --features docker-tests-<suite> --no-fail-fast -- <test_module>::

    - name: Stop docker nodes
      if: always()
      run: docker compose -f .docker/test-nodes.yml down -v
```

**New jobs to add:**

| Job | Feature Flag | Docker Profile | Required Env | Notes |
|-----|--------------|----------------|--------------|-------|
| `docker-tests-watchers` | `docker-tests-watchers` | `utxo,evm` | No UTXO/Cosmos/SIA/SLP/Qtum/Zombie | Needs UTXO + Geth |
| `docker-tests-ordermatch` | `docker-tests-ordermatch` | `utxo` | No ETH/SLP/Qtum/Cosmos/Zombie/SIA | UTXO only |
| `docker-tests-swaps-utxo` | `docker-tests-swaps-utxo` | `utxo` | No ETH/SLP/Qtum/Cosmos/Zombie/SIA | Needs zcash params |
| `docker-tests-qrc20` | `docker-tests-qrc20` | `qtum` | No UTXO/ETH/SLP/Cosmos/Zombie/SIA | Qtum only |
| `docker-tests-tendermint` | `docker-tests-tendermint` | `cosmos` | No UTXO/ETH/SLP/Qtum/Zombie/SIA | Needs IBC setup |
| `docker-tests-zcoin` | `docker-tests-zcoin` | `zombie` | No UTXO/ETH/SLP/Qtum/Cosmos/SIA | Needs zcash params |

- Run jobs in parallel.
- After first iteration, record duration per job and adjust if needed.

---

### Phase 4 – Simplify modes & metadata

**Goal:** Reduce complexity to a minimal set of environment modes and clarify what metadata is responsible for.

#### 4.4.1 Dedicated “docker env init” command

- Extract Compose-related initialization into a dedicated binary or subcommand, for example:

   - `cargo run -p mm2 --bin docker_env_init`

- Responsibilities:
   - Assume docker-compose containers are already up.
   - Initialize:
      - Contracts (swap, watchers, NFTs, ERC20/721/1155)
      - QRC20 contracts
      - SLP tokens
      - Cosmos IBC channels
   - Write `docker_env_metadata.json` (only generated artifacts):
      - Contract addresses
      - Token IDs
      - Any generated keys/seeds strictly required by tests.

CI usage:

- Compose job:
   - `docker compose up -d ...`
   - `cargo run -p mm2 --bin docker_env_init`
   - `cargo test -p mm2 --features docker-tests-...`

#### 4.4.2 Reduce modes in the main test runner

In `docker_tests_runner`:

- Keep only two modes:
   - `Testcontainers` (self-contained; legacy behavior).
   - `ReuseMetadata` (connect to a pre-initialized environment using metadata).
- Remove `ComposeInit` as a runtime mode:
   - That logic now lives exclusively in `docker_env_init`.

This keeps test execution simple:

- Local dev: `cargo test -p mm2 --test docker_tests_main` (testcontainers).
- CI / composed env:
   - Run env init once → then always `ReuseMetadata`.

#### 4.4.3 Slim down `DockerEnvMetadata`

- Retain only “generated” artifacts that are expensive or impossible to infer:
   - Contract addresses (swap, watcher, NFTs, ERC20).
   - QRC20 swap contracts, token contracts.
   - SLP token IDs & owners (if required).
- Remove:
   - Hard-coded ports/hosts that can be read from env or shared config.
   - Direct file paths that follow a pre-known directory layout where possible.
- Keep a small `.docker/config.json` or `.env` to hold stable host/port information, shared between:
   - docker-compose
   - `docker_env_init`
   - tests.

#### 4.4.4 Guard global statics

- In `load_metadata_into_globals()`:
   - Ensure it is only called once:
      - Maintain a static `OnceCell`/flag; panic or log error if called again.
- Longer-term direction:
   - Introduce a `TestEnv` object that encapsulates:
      - RPC clients
      - Contract addresses
      - Paths
   - Pass `&TestEnv` or `Arc<TestEnv>` into helpers instead of heavy use of mutable `static mut` for Geth/Qtum/SLP/WATCHERS state.

---

### Phase 5 – Runtime & flakiness optimization

**Goal:** Once jobs are functionally separated, squeeze down runtimes and make tests more deterministic.

#### 5.1 Watchers job

- Reduce:
   - Locktimes (since these are local test networks).
   - Confirmation counts where safe (e.g. 1 conf instead of 3 if semantics permit).
- Tighten:
   - `wait_for_log` durations to “just enough” + small buffer.
- Remove or merge redundant scenarios:
   - If multiple tests cover effectively the same pattern, keep one representative.

#### 5.2 Swaps / UTXO job

- UTXO is regtest; safe to:
   - Shorten timeouts & locktimes.
   - Increase mining cadence (background miner).
- For long-running tests (`test_v2_swap_utxo_utxo_kickstart`, etc.):
   - Confirm they really need current durations; otherwise trim.

#### 5.3 Tendermint job

- Configure:
   - Lower block times for test chains (if possible).
   - Shorter IBC timeouts where semantics allow.
- Evaluate:
   - Whether all swap permutations (e.g. NUCLEUS ↔ DOC, DOC ↔ IRIS-IBC) are strictly necessary or can be reduced.

#### 5.4 ZCoin / Sia jobs

- Ensure:
   - One-time initialization pre-warms:
      - Sapling cache
      - Sia chain height / initial funding
   - Tests do not re-mine or re-cache more than necessary.

---

### Phase 6 – Remove Sepolia testnet dependency

**Goal:** Eliminate dependency on external Sepolia testnet and migrate all swap v2 tests to use local Geth dev node.

**Context:**

Currently, swap v2 tests are split across two networks:
- **Sepolia testnet** (external, requires internet, slower, less reliable):
  - ~14 test functions gated by `sepolia-maker-swap-v2-tests` / `sepolia-taker-swap-v2-tests` features
  - Uses real testnet with deployed contracts: `SEPOLIA_MAKER_SWAP_V2`, `SEPOLIA_TAKER_SWAP_V2`, `SEPOLIA_ETOMIC_MAKER_NFT_SWAP_V2`, `SEPOLIA_ERC20_CONTRACT`
  - Requires Sepolia RPC endpoint (`https://ethereum-sepolia-rpc.publicnode.com`)
  - Has separate nonce lock (`SEPOLIA_NONCE_LOCK`) and test lock (`SEPOLIA_TESTS_LOCK`)
- **Local Geth dev node** (docker, fast, deterministic):
  - Already supports swap v2 contracts: `GETH_MAKER_SWAP_V2`, `GETH_TAKER_SWAP_V2`, `GETH_NFT_MAKER_SWAP_V2`
  - Initialized in `docker_tests_main.rs`
  - Used by most other ETH/ERC20 tests

**Benefits of migration:**

1. **Reliability**: No dependency on external RPC endpoints or testnet availability
2. **Speed**: Local dev node is faster and has instant block mining
3. **Determinism**: Controlled environment without testnet state variability
4. **Cost**: No need to manage testnet ETH faucets or deal with rate limits
5. **Simplicity**: Single ETH test environment instead of two parallel setups
6. **CI stability**: Eliminates network-related flakiness

#### 6.1 Preparation

**Files affected:**
- `mm2src/mm2_main/tests/docker_tests/helpers/eth.rs`
- `mm2src/mm2_main/tests/docker_tests/eth_docker_tests.rs`
- `mm2src/mm2_main/Cargo.toml`

Actions:

- [ ] Audit all 14 Sepolia test functions to identify any Sepolia-specific requirements:
  - Are there testnet-specific contract behaviors?
  - Do any tests rely on testnet block times or gas costs?
  - Are there hardcoded Sepolia addresses that need replacement?
- [ ] Verify Geth dev node has all required contracts deployed during initialization:
  - `GETH_MAKER_SWAP_V2` ✓ (already exists)
  - `GETH_TAKER_SWAP_V2` ✓ (already exists)
  - `GETH_NFT_MAKER_SWAP_V2` ✓ (already exists)
  - `GETH_ERC20_CONTRACT` ✓ (already exists)
- [ ] Document any Sepolia-specific test behaviors that need adaptation

#### 6.2 Migration

Actions:

- [ ] **Phase 6.2.1**: Migrate Sepolia helper infrastructure to Geth equivalents
  - In `helpers/eth.rs`:
    - Remove `SEPOLIA_WEB3`, `SEPOLIA_RPC_URL`, `SEPOLIA_NONCE_LOCK`, `SEPOLIA_TESTS_LOCK`
    - Remove Sepolia contract address statics: `SEPOLIA_TAKER_SWAP_V2`, `SEPOLIA_MAKER_SWAP_V2`, `SEPOLIA_ETOMIC_MAKER_NFT_SWAP_V2`, `SEPOLIA_ERC20_CONTRACT`
    - Update any Sepolia-specific funding helpers to use Geth equivalents

- [ ] **Phase 6.2.2**: Migrate test functions one-by-one or in small batches
  - For each Sepolia test in `eth_docker_tests.rs`:
    - Remove `#[cfg(feature = "sepolia-*-swap-v2-tests")]` gate
    - Replace Sepolia contract address calls with Geth equivalents:
      - `sepolia_maker_swap_v2()` → `maker_swap_v2()`
      - `sepolia_taker_swap_v2()` → `taker_swap_v2()`
      - `sepolia_etomic_maker_nft_swap_v2()` → `nft_maker_swap_v2()`
    - Replace `SEPOLIA_NONCE_LOCK` → `GETH_NONCE_LOCK`
    - Replace `SEPOLIA_TESTS_LOCK` usage (if any) with appropriate test coordination
    - Update RPC client initialization to use `GETH_WEB3` / `GETH_RPC_URL`
  - Run each migrated test to ensure it passes with Geth
  - Commit after each successful migration or small batch

- [ ] **Phase 6.2.3**: Clean up feature flags
  - Remove from `mm2src/mm2_main/Cargo.toml`:
    - `sepolia-maker-swap-v2-tests` feature
    - `sepolia-taker-swap-v2-tests` feature
  - Search codebase for any remaining references to these features
  - Update CI workflows if they reference Sepolia test jobs

- [ ] **Phase 6.2.4**: Remove Sepolia infrastructure
  - Delete all Sepolia-related code from `helpers/eth.rs`:
    - Static variables
    - Helper functions
    - Comments/documentation
  - Update module documentation to reflect single Geth-based environment
  - Run full docker test suite to verify no regressions

#### 6.3 Validation

- [ ] All previously Sepolia-gated tests pass using Geth
- [ ] `cargo test --test docker_tests_main --features docker-tests-eth` runs without Sepolia dependencies
- [ ] No references to Sepolia remain in docker test code (除非在注释中作为历史记录)
- [ ] Geth initialization in `docker_tests_main.rs` is sufficient for all swap v2 scenarios
- [ ] Test runtime improves (measure before/after for representative test)

---

## Appendix — Concrete code pointers for Phase 1

| Task | File | Location |
|------|------|----------|
| Geth metadata URL in health | `docker_tests_main.rs` | `validate_nodes_health()` → replace `block_on(GETH_WEB3.eth().block_number()...)` with a `Web3` constructed from `metadata.geth.rpc_url` |
| Qtum conf path | `docker_tests_main.rs` | `setup_qtum_conf_for_compose()` → write to `coin_daemon_data_dir("QTUM", true)/qtum.conf` (or `.docker/container-runtime/qtum/qtum.conf`), store in metadata, assert exists in Reuse |
| Watchers assert fix | `swap_watcher_tests.rs` | `test_two_watchers_spend_maker_payment_eth_erc20()` lines 1223-1228 → implement `w1_gain`/`w2_gain` boolean logic and `assert_ne!(w1_gain, w2_gain)` |

---

### Phase 7 – Final validation

**Goal:** Verify that the split CI jobs collectively run the same number of tests as the original monolithic job.

#### 7.1 Test count validation

**Baseline (monolithic docker-tests job):**
```
test result: ok. 235 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 1864.36s
```

**Validation steps:**

- [ ] After all split jobs are implemented and running in CI, collect test results from each job:
  - `docker-tests-eth`: X passed, Y ignored
  - `docker-tests-slp`: X passed, Y ignored
  - `docker-tests-sia`: X passed, Y ignored
  - `docker-tests-ordermatch`: X passed, Y ignored
  - `docker-tests-swaps-utxo`: X passed, Y ignored
  - `docker-tests-watchers`: X passed, Y ignored
  - `docker-tests-qrc20`: X passed, Y ignored
  - `docker-tests-tendermint`: X passed, Y ignored
  - `docker-tests-zcoin`: X passed, Y ignored
  - `docker-tests-integration` (if created): X passed, Y ignored

- [ ] Sum all results and verify:
  - **Total passed** = 235 (must match baseline)
  - **Total ignored** = 8 (must match baseline)

- [ ] If counts don't match:
  - Investigate for missing tests (tests not gated by any feature)
  - Check for duplicate tests (tests running in multiple jobs)
  - Verify feature gate configurations in `mod.rs`

- [ ] Document final test distribution across jobs in this file

**Note:** Minor variations may occur if tests are added/removed during the plan implementation. In such cases, document the new baseline and ensure the sum of split jobs equals the updated total.

---

## Success criteria checklist

- [x] `ReuseMetadata` mode connects to the correct Geth RPC from metadata and fails fast if contract bytecode is missing.
- [x] Qtum compose runs are stable across test invocations (no `temp_dir()` dependency).
- [ ] New feature flags build only the intended suites; CI runs watchers/ordermatch/swaps/qrc20/tendermint/zcoin as separate green jobs using Compose mode.
- [x] The ignored watchers test has meaningful assertions when un-ignored locally.
- [ ] **Test count validation:** Sum of all split CI jobs equals baseline (235 passed, 8 ignored).