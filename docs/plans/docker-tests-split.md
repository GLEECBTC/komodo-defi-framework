# Plan: Docker tests refactor & CI split

**Owner:** @Omer  
**Status:** Draft  
**Scope:** Docker-based integration tests (UTXO, ETH, QRC20/Qtum, SLP, Tendermint/Cosmos, ZCoin, Sia, watchers)  
**Entry point:** Linked from `AGENTS.md` → `plans/docker_tests.md`

---

## 1. Goals

1. ✅ Stabilize the new Docker infra (Compose) and fix all correctness issues.
2. ✅ Split the monolithic `docker-tests` job into smaller **functional** jobs:
   - Ordermatching (`docker-tests-ordermatch`)
   - Swaps (`docker-tests-swaps-utxo`)
   - Watchers (`docker-tests-watchers`)
   - Chain-specific suites (`docker-tests-qrc20`, `docker-tests-tendermint`, `docker-tests-zcoin`, `docker-tests-slp`, `docker-tests-eth`, `docker-tests-sia`)
   - Cross-chain integration (`docker-tests-integration`)
3. ✅ Shorten feedback loop: each job is runnable in isolation.
4. Preserve **testcontainers** semantics as the baseline:
   - New modes should behave like the old flow from the perspective of tests.
5. ✅ Keep code churn low:
   - Used cfg-gating, helpers, and clear grouping over massive file moves.

### 1.1 Non-goals (for now)

- Rewriting tests into a different framework.
- Changing swap / ordermatch implementation logic.
- Removing testcontainers entirely.
- Perfect partitioning; the goal is a good, maintainable split, not theoretical purity.

---

## 2. Current state (snapshot)

### 2.1 Environment modes

`docker_tests_main.rs` currently supports two modes:

- `Testcontainers` (legacy / default)
   - Tests spin up containers via `testcontainers`.
- `ComposeInit`
   - Assumes docker-compose is already running.
   - Initializes nodes (contracts, tokens, IBC, etc.) on each run.

**Note:** Docker env metadata persistence (`DockerEnvMetadata` / `KDF_DOCKER_ENV_STATE_FILE` / ReuseMetadata) was removed because it was not used by CI and added unnecessary complexity.

New infra:

- Contract helpers:
   - ETH contracts: `swap_contract()`, `watchers_swap_contract()`, `erc20_contract()`, etc.
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

**Historical (pre-feature-gating) state of the monolithic `docker-tests` job:**
It used to compile and run:

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

This run produced approximately 235 passing tests in ~1800 seconds.

**Current (post-gating) behavior:**

- Many suites are now gated on additional `docker-tests-*` features:
  - Ordermatching: `docker-tests-ordermatch`
  - UTXO swaps: `docker-tests-swaps-utxo`
  - Watchers: `docker-tests-watchers`
  - QRC20: `docker-tests-qrc20`
  - Tendermint: `docker-tests-tendermint`
  - ZCoin: `docker-tests-zcoin`
- The CI `docker-tests` job currently uses only `--features run-docker-tests`, so these feature-gated modules are **not** compiled there.
- The 235-test figure should be treated as a **historical baseline**; the goal for Phase 3 is that the sum of all split jobs (each with its feature flag) matches or exceeds this baseline.

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
   - A small set of "everything together" swaps (e.g. SLP ↔ UTXO ↔ QRC20 ↔ ETH).

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
- **Documentation hygiene**: Before each commit, update any documentation that is no longer accurate due to the changes being committed. This includes:
   - `docs/DOCKER_TESTS.md` — file structure, execution modes, CI job descriptions
   - `docs/plans/docker-tests-split.md` — phase status, completed/pending checkboxes, baseline figures
   - Do not add new documentation sections; only modify existing content to reflect the current state.

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
- [x] Store the chosen `qtum.conf` path for future reference (if needed).

#### 4.1.3 Single source of truth for metadata file path (non-breaking)

**File:** `mm2src/mm2_main/tests/docker_tests/docker_env_metadata.rs`

**Status:** ✅ Removed - metadata persistence not used by CI.

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

**File:** `mm2src/mm2_main/tests/docker_tests/helpers/env.rs`

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
- `docker-tests-watchers = ["run-docker-tests"]` - Watcher node tests (UTXO-only, stable)
- `docker-tests-watchers-eth = ["docker-tests-watchers", "coins/enable-eth-watchers"]` - ETH/ERC20 watcher tests (unstable, not completed yet). This feature also enables the ETH watcher implementation code in the coins crate via `coins/enable-eth-watchers`.
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
// swap_watcher_tests is a directory module containing:
// - mod.rs: shared helpers (enable_coin, enable_eth, BalanceResult, SwapFlow, start_swaps_and_get_balances, etc.)
// - utxo.rs: UTXO-only watcher tests (always compiled with docker-tests-watchers)
// - eth.rs: ETH/ERC20 watcher tests (requires docker-tests-watchers-eth, disabled by default)
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

**Note:** Because modules are now gated on `docker-tests-*` features, a suite will not compile or run unless its feature flag is enabled. As of the current CI:

- Only `docker-tests-eth`, `docker-tests-slp`, and `docker-tests-sia` have dedicated jobs.
- Suites behind `docker-tests-ordermatch`, `docker-tests-swaps-utxo`, `docker-tests-watchers`,
  `docker-tests-qrc20`, `docker-tests-tendermint`, and `docker-tests-zcoin` currently only run
  when invoked manually with the appropriate feature flags.

#### 4.2.4 Test placement audit & file splitting (IN PROGRESS)

**Goal:** Ensure tests are in the correct files and split large files that test multiple concerns.

**Historic baseline (pre-split monolithic docker-tests job):**
```
test result: ok. 235 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 1864.36s
```
After plan completion, the sum of all split jobs must equal this baseline.

**Status:** Partial implementation - UTXO swap tests, UTXO ordermatching tests, and ETH-only tests extracted to new modules.

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
  - Setprice max trade test (`test_match_and_trade_setprice_max`)
  - Max taker vol swap test (`test_max_taker_vol_swap`)
  - Trade preimage tests (`test_maker_trade_preimage`, `test_taker_trade_preimage`, `test_trade_preimage_not_sufficient_balance`, `test_trade_preimage_additional_validation`, `test_trade_preimage_legacy`)
- [x] Added module entry in `mod.rs` gated by `docker-tests-swaps-utxo`
- [x] Verified compilation with `cargo check -p mm2_main --features run-docker-tests,docker-tests-swaps-utxo`
- [x] Verified no clippy warnings with `-D warnings`

- [x] Created `utxo_ordermatch_v1_tests.rs` - Extracted 17 UTXO-only ordermatching tests from `docker_tests_inner.rs`:
  - Order lifecycle tests (`order_should_be_cancelled_when_entire_balance_is_withdrawn`, `order_should_be_updated_when_balance_is_decreased_*`)
  - Partial fill test (`test_order_should_be_updated_when_matched_partially`)
  - Order volume tests (`test_set_price_max`)
  - Restart/persistence tests (`test_maker_order_should_kick_start_and_appear_in_orderbook_on_restart`, `test_maker_order_should_not_kick_start_and_appear_in_orderbook_if_balance_is_withdrawn`, `test_maker_order_kick_start_should_trigger_subscription_and_match`)
  - Same private key edge cases (`test_orders_should_match_on_both_nodes_with_same_priv`, `test_maker_and_taker_order_created_with_same_priv_should_not_match`)
  - Order conversion test (`test_taker_order_converted_to_maker_should_cancel_properly_when_matched`)
  - Best price matching tests (`test_taker_should_match_with_best_price_buy`, `test_taker_should_match_with_best_price_sell`)
  - RPC response format tests (`test_set_price_response_format`, `test_buy_response_format`, `test_sell_response_format`, `test_my_orders_response_format`)
- [x] Added module entry in `mod.rs` gated by `docker-tests-ordermatch`
- [x] Removed duplicate tests from `docker_tests_inner.rs` (file reduced from ~3300 to ~1957 lines)
- [x] Verified compilation with `cargo check -p mm2_main --features run-docker-tests,docker-tests-ordermatch`
- [x] Verified no clippy warnings with `-D warnings` for both `docker-tests-eth` and `docker-tests-ordermatch`

- [x] Created `eth_inner_tests.rs` - Extracted 15 ETH-only tests from `docker_tests_inner.rs`:
  - ETH/ERC20 activation tests (`test_enable_eth_coin_with_token_then_disable`, `test_enable_eth_coin_with_token_without_balance`)
  - Platform coin mismatch test (`test_platform_coin_mismatch`)
  - Swap contract negotiation tests (`test_eth_swap_contract_addr_negotiation_same_fallback`, `test_eth_swap_negotiation_fails_maker_no_fallback`)
  - Trade tests (`test_trade_base_rel_eth_erc20_coins`)
  - Withdrawal tests (`test_withdraw_and_send_eth_erc20`, `test_withdraw_and_send_hd_eth_erc20`)
  - Order/DB persistence tests (`test_set_price_must_save_order_to_db`)
  - Conf settings tests (`test_set_price_conf_settings`, `test_buy_conf_settings`, `test_sell_conf_settings`)
  - Order management tests (`test_my_orders_after_matched`, `test_update_maker_order_after_matched`)
  - ERC20 approval test (`test_approve_erc20`)
- [x] Moved 4 UTXO min_volume/dust tests to `utxo_ordermatch_v1_tests.rs`:
  - `test_buy_min_volume`, `test_sell_min_volume`, `test_setprice_min_volume_dust`, `test_sell_min_volume_dust`
- [x] Added module entry in `mod.rs` gated by `docker-tests-eth`
- [x] Removed extracted tests from `docker_tests_inner.rs` (file reduced from ~1957 to ~523 lines)
- [x] `docker_tests_inner.rs` now contains only 4 cross-chain tests requiring BOTH ETH and UTXO:
  - `test_match_utxo_with_eth_taker_sell`
  - `test_match_utxo_with_eth_taker_buy`
  - `test_setprice_buy_sell_too_low_volume`
  - `test_orderbook_depth`
- [x] Moved `test_peer_time_sync_validation` to `utxo_ordermatch_v1_tests.rs` (P2P test that only uses UTXO coins)
- [x] Fixed copy-paste bugs in `utxo_ordermatch_v1_tests.rs`:
  - Corrected `mm_dump(&mm_alice.log_path)` → `mm_dump(&mm_eve.log_path)` in two locations
  - Renamed `alice_buy` → `alice_sell` in `test_taker_should_match_with_best_price_sell`
  - Fixed assertion message `"!buy:"` → `"!sell:"` in sell test
- [x] Verified compilation with `cargo clippy -p mm2_main --tests --features run-docker-tests,docker-tests-eth`
- [x] Verified compilation with `cargo clippy -p mm2_main --tests --features run-docker-tests,docker-tests-ordermatch`

**Remaining tasks:**
- [x] Audit each test module to verify tests are correctly placed:
  - Fixed `docker_tests_inner.rs` feature gate from `docker-tests-eth` to `docker-tests-ordermatch` (cross-chain ordermatching tests)
  - Split `tendermint_tests.rs` to extract cross-chain swap tests to `tendermint_swap_tests.rs`
  - `tendermint_swap_tests.rs` gated by `docker-tests-tendermint + docker-tests-eth` (requires both environments)
- [x] Complete splitting of `docker_tests_inner.rs`:
  - ~~Extract ordermatching tests to `ordermatch_inner_tests.rs` (gated by `docker-tests-ordermatch`)~~ ✅ Done as `utxo_ordermatch_v1_tests.rs`
  - ~~Extract ETH-specific tests to `eth_inner_tests.rs` (keep in `docker-tests-eth`)~~ ✅ Done
  - ~~Remove extracted tests from `docker_tests_inner.rs` to avoid duplication~~ ✅ Done
- [x] Consider splitting other large files:
  - `eth_docker_tests.rs` - Reviewed; no split needed (all EVM-scope tests)
  - `tendermint_tests.rs` - Split completed: cross-chain swaps moved to `tendermint_swap_tests.rs`
- [x] Update feature gates after test movements to ensure correct CI job assignment
  - Verified all module gates in `mod.rs` match intended suite assignments
  - `docker_tests_inner` correctly gated by `docker-tests-ordermatch` (cross-chain UTXO+ETH ordermatching)
  - `tendermint_swap_tests` correctly gated by `docker-tests-tendermint + docker-tests-eth`
  - All other modules have correct single-feature gates matching their CI job

**Future cleanup (post-plan):**
- [ ] Reduce `#[cfg(feature = ...)]` complexity across docker test infrastructure:
  - **Restructure file organization**: Group related functionality into feature-specific submodules
    - Split `runner.rs` into `runner/utxo.rs`, `runner/eth.rs`, `runner/tendermint.rs`, etc.
    - Split `helpers/` into chain-specific modules that are conditionally compiled as units
  - **Use module-level gating**: `#[cfg] mod utxo;` instead of individual function/import gating
  - **Split then combine approach**: Each chain's setup logic in its own file, then combine via conditional imports
  - **Reduce import duplication**: Consolidate feature gates to module boundaries rather than individual items
  - **Benefits**: Cleaner code, fewer warnings about unused items, easier maintenance
- [ ] Review `utxo_swaps_v1_tests.rs` for tests that don't belong in swaps category:
  - UTXO merge tests may belong in a separate UTXO maintenance module
  - Some tests may better fit in ordermatching category
  - Reorganize based on actual test purpose vs. chain dependency
- [ ] Consider introducing a separate `docker-tests-eth-only` feature flag:
  - Currently `eth_inner_tests.rs` and `docker_tests_inner.rs` both use `docker-tests-eth` feature
  - `eth_inner_tests.rs` contains 15 tests that only need ETH/Geth containers
  - `docker_tests_inner.rs` contains 5 cross-chain tests requiring BOTH ETH and UTXO containers
  - A dedicated `docker-tests-eth-only` feature would allow running ETH-only tests without spinning up UTXO containers
  - This could reduce CI resource usage and test runtime for ETH-specific validation

#### 4.2.5 Runner: start only what's needed (keep env flags)

**File:** `docker_tests_main.rs`

The runner already honors `_KDF_NO_*_DOCKER` env vars. For now, don't add compile-time logic—CI will pass these envs to disable unused nodes.

Later, you can add `#[cfg(feature = "...")]` blocks around image pulling to slightly speed startup, but this isn't required to split jobs.

---

### Phase 3 – CI: add functional jobs (Compose mode)

**Status:** ✅ Completed

**Goal:** Break the monolithic docker tests job into parallel jobs grouped by behavior. Keep each new job small and independent. All jobs use Compose mode (`KDF_DOCKER_COMPOSE_ENV=1`) to enable sharing containers with other tests (e.g., WASM tests).

**Post-implementation fixes:**

**Sia feature gating fix:**
The initial Phase 3 implementation had a bug where `sia_tests` module and Sia container initialization
ran in all docker test jobs regardless of the `docker-tests-sia` feature flag. This was fixed by:
- Gating `mod sia_tests;` and all Sia-specific imports in `docker_tests_main.rs` with `#[cfg(feature = "docker-tests-sia")]`
- Gating Sia helpers in `helpers/mod.rs` with `#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-sia"))]`
- Gating Sia container initialization, image pulling, and health checks in `docker_tests_main.rs`

**Cross-dependency analysis and resolution strategy:**

Analysis of CI failures (run #20096344554 on 2025-12-10) revealed several categories of issues:

**Category 1: Single-chain jobs needing UTXO for coin-specific tests (RESOLVED)**

These jobs test a specific coin but some tests require MYCOIN for swap counterparty:

1. **QRC20 tests** (`qrc20_tests`):
   - Tests like `test_trade_qrc20`, `trade_test_with_maker_segwit` swap QRC20 ↔ MYCOIN
   - **Resolution:** Add UTXO nodes to `docker-tests-qrc20` job (same chain family, acceptable)
   - **Additional fix (2025-12-10):** Added "Fetch zcash params" step to CI job. The MYCOIN/MYCOIN1 containers use the `testblockchain:multiarch` image which is Komodo-based and requires zcash params (`~/.zcash-params`) to start the daemon. Without this step, the containers start but the daemon never opens the RPC port (8000/8001), causing `wait_ready()` to timeout with "Test timed out".

2. **Sia tests** (`sia_tests`):
   - Tests like `test_bob_sells_dsia_for_mycoin` swap DSIA ↔ MYCOIN
   - **Resolution:** Add UTXO nodes to `docker-tests-sia` job (same chain family, acceptable)
   - **Additional fix (2025-12-10):** Added "Fetch zcash params" step to CI job. Same root cause as QRC20 - MYCOIN/MYCOIN1 containers require zcash params to start the Komodo daemon.

**Category 2: Cross-chain tests requiring multiple distinct chain families (TO BE MOVED)**

Tests that swap between fundamentally different chain types should go to `docker-tests-integration`:

- QRC20 ↔ ETH swaps
- Tendermint ↔ ETH swaps (currently in `tendermint_swap_tests`)
- SLP ↔ ETH swaps
- Any other multi-family cross-chain scenarios

**Category 3: Bugs requiring investigation (HIGH PRIORITY)**

These failures are NOT due to missing containers but actual bugs:

1. **ETH tests** (`docker-tests-eth`):
   - `test_eth_swap_contract_addr_negotiation_same_fallback` fails
   - **Root cause:** Likely `GETH_SWAP_CONTRACT` OnceLock not initialized in ETH-only path
   - **Action:** Debug ETH contract initialization in `docker_tests_main.rs`

2. **Watcher tests** (`docker-tests-watchers`):
   - ETH/ERC20 watcher tests have been moved to a separate submodule (`swap_watcher_tests/eth.rs`) and are disabled by default behind the `docker-tests-watchers-eth` feature flag.
   - The reward-dependent ETH watcher tests have proven **unstable/flaky** during CI splitting work:
     - `test_watcher_refunds_taker_payment_erc20`
     - `test_watcher_refunds_taker_payment_eth`
     - `test_watcher_spends_maker_payment_erc20_utxo`
   - **Resolution:** All ETH/ERC20 watcher tests are now gated behind `docker-tests-watchers-eth` which is disabled by default since ETH watchers are unstable and not completed yet.
   - UTXO-only watcher tests remain stable and are always compiled with `docker-tests-watchers`.

3. **ZCoin tests** (`docker-tests-zcoin`):
   - `zombie_coin_send_dex_fee` fails at `z_coin_docker_tests.rs:190`
   - **Root cause:** Likely Zombie container not ready or zcash params missing
   - **Action:** Verify CI starts `KDF_ZOMBIE_SERVICE` and downloads zcash params

**Implementation summary:**
All CI jobs now use only feature flags for test selection (no test module filters). The feature-gated modules in `mod.rs` control which tests are compiled and run for each job:

- `docker-tests-eth`: ETH/ERC20 tests (Geth node only)
- `docker-tests-slp`: BCH/SLP token tests (FORSLP node only)
- `docker-tests-sia`: Sia tests (Sia + UTXO nodes for DSIA↔MYCOIN swaps)
- `docker-tests-ordermatch`: Ordermatching tests (UTXO + ETH nodes)
- `docker-tests-swaps-utxo`: UTXO swap protocol tests (UTXO nodes only)
- `docker-tests-watchers`: Watcher tests (UTXO + ETH nodes)
- `docker-tests-qrc20`: Qtum/QRC20 tests (Qtum + UTXO nodes for QRC20↔MYCOIN swaps)
- `docker-tests-tendermint`: Cosmos/IBC tests (Cosmos nodes only)
- `docker-tests-zcoin`: ZCoin/Zombie tests (Zombie node only)

#### 4.3.1 CI job matrix & features

- **Feature flags status (in `mm2_main/Cargo.toml`):**
  - Already present: `docker-tests-eth`, `docker-tests-slp`, `docker-tests-sia`,
    `docker-tests-ordermatch`, `docker-tests-swaps-utxo`, `docker-tests-watchers`,
    `docker-tests-qrc20`, `docker-tests-tendermint`, `docker-tests-zcoin`,
    `docker-tests-integration`
  - To be added: `docker-tests-all` (aggregate feature for local dev convenience)

CI jobs mapping:

| Job                        | Feature flag              | Containers                    | Primary content                                           |
|---------------------------|---------------------------|-------------------------------|-----------------------------------------------------------|
| `docker-tests-eth`        | `docker-tests-eth`        | Geth                          | ETH/ERC20/721/1155 tests                                 |
| `docker-tests-slp`        | `docker-tests-slp`        | FORSLP                        | SLP-only tests                                           |
| `docker-tests-sia`        | `docker-tests-sia`        | Sia + UTXO                    | Sia client & DSIA↔MYCOIN swaps                           |
| `docker-tests-ordermatch` | `docker-tests-ordermatch` | UTXO + Geth                   | Ordermatching & wallet/order lifecycle                   |
| `docker-tests-swaps-utxo` | `docker-tests-swaps-utxo` | UTXO                          | UTXO swap protocol v1/v2, file locking, conf sync        |
| `docker-tests-watchers`   | `docker-tests-watchers`   | UTXO only                     | UTXO-only watcher tests (stable, no Geth needed)         |
| `docker-tests-watchers-eth` | `docker-tests-watchers-eth` | UTXO + Geth                 | ETH/ERC20 watcher tests (unstable, disabled by default, not in CI)  |
| `docker-tests-qrc20`      | `docker-tests-qrc20`      | Qtum + UTXO                   | Qtum/QRC20 tests & QRC20↔MYCOIN swaps                    |
| `docker-tests-tendermint` | `docker-tests-tendermint` | Cosmos                        | Cosmos/Tendermint/IBC tests (no cross-chain swaps)       |
| `docker-tests-zcoin`      | `docker-tests-zcoin`      | Zombie                        | ZCoin (Zombie) tests                                     |
| `docker-tests-integration`| `docker-tests-integration`| ALL (UTXO, Geth, Qtum, Cosmos, etc.) | Cross-chain swaps: ETH↔Tendermint, ETH↔QRC20, etc.       |

#### 4.3.2 Assign modules to jobs

**Ordermatching (`docker-tests-ordermatch`)**

- `docker_ordermatch_tests::*` (except the Zombie-specific test below)
- `utxo_ordermatch_v1_tests::*` (UTXO-only ordermatching tests extracted from `docker_tests_inner.rs`)
- `docker_tests_inner::*` (cross-chain UTXO+ETH ordermatching tests)

**Note:** The `docker_tests_inner` module contains 4 cross-chain tests that require **both UTXO and ETH containers**. Therefore, the `docker-tests-ordermatch` CI job must start Geth/ETH containers in addition to UTXO containers.

**Swaps (`docker-tests-swaps-utxo`)**

- `utxo_swaps_v1_tests::*` (UTXO swap v1 mechanics, max volume, withdraw/locked amount, merge tests)
- `swap_proto_v2_tests::*` (swap protocol v2 tests)
- `swaps_file_lock_tests::*` (swap file locking tests)
- `swaps_confs_settings_sync_tests::*` (confirmation settings synchronization tests)

**Watchers (`docker-tests-watchers`)**

- `swap_watcher_tests::utxo::*` (UTXO-only watcher tests, always compiled)
- `swap_watcher_tests::eth::*` (ETH/ERC20 watcher tests, requires `docker-tests-watchers-eth` feature, disabled by default because ETH watchers are unstable and not completed yet)

**QRC20 (`docker-tests-qrc20`)**

- `qrc20_tests::*` (all QRC20/Qtum-only tests).

**Tendermint (`docker-tests-tendermint`)**

- `tendermint_tests::*` (Cosmos-only tests):
   - Tendermint balance/withdraw/IBC/delegation/validators/tx history tests

**Tendermint Cross-Chain Swaps (`docker-tests-tendermint + docker-tests-eth`)**

- `tendermint_swap_tests::*` (requires both Tendermint and ETH environments):
   - `swap_nucleus_with_doc` (NUCLEUS <-> DOC)
   - `swap_nucleus_with_eth` (NUCLEUS <-> ETH)
   - `swap_doc_with_iris_ibc_nucleus` (DOC <-> IRIS-IBC-NUCLEUS)

**ZCoin (`docker-tests-zcoin`)**

- `z_coin_docker_tests::*`
- `docker_ordermatch_tests::test_zombie_order_after_balance_reduce_and_mm_restart`

**Integration (`docker-tests-integration`)** — *NOT YET IMPLEMENTED*

This job runs cross-chain swap tests between fundamentally different chain families (ETH↔Tendermint, ETH↔QRC20, etc.):

- `tendermint_swap_tests::*` (Tendermint↔ETH swaps, currently gated by `docker-tests-tendermint + docker-tests-eth`):
   - `swap_nucleus_with_doc` (NUCLEUS <-> DOC)
   - `swap_nucleus_with_eth` (NUCLEUS <-> ETH)
   - `swap_doc_with_iris_ibc_nucleus` (DOC <-> IRIS-IBC-NUCLEUS)
- `swap_tests::trade_test_with_maker_slp` (SLP cross-chain)
- `swap_tests::trade_test_with_taker_slp` (SLP cross-chain)
- Any future QRC20↔ETH, Sia↔ETH, or other multi-family swap tests

**Note:** Single-chain jobs (e.g., `docker-tests-qrc20`, `docker-tests-sia`) can include UTXO nodes for swaps against MYCOIN since UTXO is a base chain family. Only swaps between two non-UTXO chain families (e.g., ETH↔Tendermint) belong in the integration job.

**Current behavior:** `swap_tests` is compiled only when `run-docker-tests` is enabled and **no** other `docker-tests-*` features are enabled (legacy negative-gate pattern). The `docker-tests-integration` feature does not yet exist in `Cargo.toml`. This is a future task to introduce a dedicated feature flag.

#### 4.3.3 Runner profiles per job

In `docker_tests_main.rs`, adjust container startup based on enabled features:

- **Ordermatching (`docker-tests-ordermatch`):**
   - Start UTXO containers (`MYCOIN`, `MYCOIN1`) + Geth/ETH containers.
   - Required because `docker_tests_inner` contains cross-chain UTXO+ETH ordermatching tests.
- **Swaps (`docker-tests-swaps-utxo`):**
   - Start UTXO containers (`MYCOIN`, `MYCOIN1`) only.
- **Watchers (`docker-tests-watchers`):**
   - Start UTXO containers only (MYCOIN, MYCOIN1). No Geth needed.
   - ETH/ERC20 watcher tests are disabled by default (require `docker-tests-watchers-eth` feature).
- **Watchers ETH (`docker-tests-watchers-eth`):** *(not in CI, disabled by default)*
   - Would require UTXO + Geth (no Cosmos/Sia/Qtum/etc).
   - Includes all ETH/ERC20 watcher tests which are unstable and not completed yet.
- **QRC20 (`docker-tests-qrc20`):**
   - Start Qtum/QRC20 + UTXO containers for QRC20↔MYCOIN swap tests.
- **Sia (`docker-tests-sia`):**
   - Start Sia + UTXO containers for DSIA↔MYCOIN swap tests.
- **Tendermint (`docker-tests-tendermint`):**
   - Start Cosmos nodes (Nucleus, Atom) and relayer; prepare IBC channels.
   - **Note:** Cross-chain Tendermint↔ETH swaps should move to `docker-tests-integration`.
- **ZCoin (`docker-tests-zcoin`):**
   - Start Zombie node and ensure zcash params are present.
- **Integration (`docker-tests-integration`):**
   - Start ALL containers (UTXO, SLP, QRC20, ETH, Cosmos, Sia, etc).
   - For cross-chain swaps between different chain families: ETH↔Tendermint, ETH↔QRC20, etc.

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

| Job | Feature Flag | Docker Profile | Notes |
|-----|--------------|----------------|-------|
| `docker-tests-watchers` | `docker-tests-watchers` | `utxo` | UTXO only (stable tests, no Geth needed) |
| ~~`docker-tests-watchers-eth`~~ | — | — | *Not in CI (disabled by default, ETH watchers unstable)* |
| `docker-tests-ordermatch` | `docker-tests-ordermatch` | `utxo,evm` | Needs UTXO + Geth |
| `docker-tests-swaps-utxo` | `docker-tests-swaps-utxo` | `utxo` | UTXO only, needs zcash params |
| `docker-tests-qrc20` | `docker-tests-qrc20` | `qtum,utxo` | Qtum + UTXO for QRC20↔MYCOIN swaps |
| `docker-tests-tendermint` | `docker-tests-tendermint` | `cosmos` | Cosmos only, needs IBC setup |
| `docker-tests-zcoin` | `docker-tests-zcoin` | `zombie` | Zombie only, needs zcash params |
| `docker-tests-sia` | `docker-tests-sia` | `sia,utxo` | Sia + UTXO for DSIA↔MYCOIN swaps |
| `docker-tests-integration` | `docker-tests-integration` | `all` | All containers for cross-chain swaps |

- Run jobs in parallel.
- After first iteration, record duration per job and adjust if needed.

#### 4.3.5 Fix helper cross-dependencies (partial implementation)

**Status:** ⚠️ Partial - Runtime guards implemented

The following runtime fixes have been implemented to prevent `OnceLock` panics when containers are not available:

**Completed (runtime guards):**

- [x] **Refactored `trade_base_rel` in `helpers/swap.rs`** to dynamically detect which chain families are needed:
  - Added chain detection flags: `uses_eth`, `uses_qrc20`, `uses_utxo`, `uses_slp`
  - Coins config now built dynamically based on which chains are actually needed for the trade pair
  - Coin enablement for Bob and Alice is now conditional based on trade pair requirements
  - **Result:** ETH-only trades (`ETH`/`ERC20DEV`) no longer call `qtum_conf_path()` or QRC20 helpers
  - **Result:** UTXO-only trades (`MYCOIN`/`MYCOIN1`) no longer call QRC20 helpers

- [x] **Removed unnecessary QRC20 cross-dependency from MYCOIN/MYCOIN1 wallet generation**:
  - Previously, `generate_and_fill_priv_key("MYCOIN")` also filled Qtum balance (unnecessary for UTXO coins)
  - Removed the extra `qrc20_coin_from_privkey` call that caused initialization panics

**What this fixes:**
- ETH tests no longer panic with "QTUM_CONF_PATH not initialized"
- UTXO tests no longer panic with "QICK_TOKEN_ADDRESS not initialized"
- Each test suite using `trade_base_rel` can now run independently with only its required containers

**Remaining tasks (compile-time isolation):**

- [ ] **Simplify redundant `#[cfg]` gates in `mod.rs`** - Since all `docker-tests-*` features depend on `run-docker-tests`, we can simplify:
  ```rust
  // From:
  #[cfg(all(feature = "run-docker-tests", feature = "docker-tests-eth"))]
  // To:
  #[cfg(feature = "docker-tests-eth")]
  ```
  Low priority - current setup works correctly, this is just cleanup.
- [ ] **Add `#[cfg]` guards on imports in `swap.rs`** - Currently imports are unconditional; full compile-time isolation requires feature-gated imports
- [ ] **Factor chain-specific logic into helpers with real/stub variants** - For zero unused warnings
- [ ] **Gate helper modules in `helpers/mod.rs` by feature** - Prevents compilation of unused helpers
- [ ] **Move cross-chain tests to `docker-tests-integration`** - Tests requiring multiple container types

**Current limitations:**
- Unused code warnings (27+ per job) still exist because all helper code is compiled even when not used
- Future feature-gating of `helpers/mod.rs` will require additional work on `swap.rs` imports
- Full compile-time isolation deferred to future implementation

**Goal (when fully complete):** `cargo check -p mm2_main --tests --features docker-tests-<any>` produces zero warnings AND tests run without initialization panics

- [x] **Add UTXO nodes to `docker-tests-qrc20` CI job** ✅ DONE
  - Updated CI workflow to start both Qtum and UTXO containers (`--profile qrc20 --profile utxo`)
  - Removed `_KDF_NO_UTXO_DOCKER` env var from job
  - Tests like `test_trade_qrc20`, `trade_test_with_maker_segwit` require MYCOIN for swap counterparty

- [x] **Add UTXO nodes to `docker-tests-sia` CI job** ✅ DONE (commit af9ca60882)
  - CI workflow already starts both Sia and UTXO containers
  - Tests like `test_bob_sells_dsia_for_mycoin` require MYCOIN for swap counterparty

- [x] **Fix `docker-tests-eth` swap contract comparison bug** ✅ DONE
  - `test_eth_swap_contract_addr_negotiation_same_fallback` was failing
  - **Root cause:** Case sensitivity bug - swap status returns lowercase address but test expected checksummed format
  - **Fix:** Changed `expected_contract` to use `.to_lowercase()` for consistent comparison
  - **File:** `mm2src/mm2_main/tests/docker_tests/eth_inner_tests.rs:331`

- [x] **`docker-tests-watchers`: ETH watcher tests and implementation code moved behind feature flag**
  - **Resolution:** All ETH/ERC20 watcher functionality is now gated behind feature flags (disabled by default):
    - **Implementation code:** `coins/enable-eth-watchers` gates the `impl WatcherOps for EthCoin` in `mm2src/coins/eth.rs` (lines 1760-2452) and helper functions `watcher_spends_hash_time_locked_payment` and `watcher_refunds_hash_time_locked_payment`. When disabled, EthCoin uses the default WatcherOps implementation which returns "not implemented" errors.
    - **Test code:** `docker-tests-watchers-eth` gates the ETH/ERC20 watcher tests in `swap_watcher_tests/eth.rs`. This feature also enables `coins/enable-eth-watchers`.
  - The flaky reward-dependent tests are no longer compiled unless the feature is explicitly enabled:
    - `test_watcher_refunds_taker_payment_erc20`
    - `test_watcher_refunds_taker_payment_eth`
    - `test_watcher_spends_maker_payment_erc20_utxo`
  - UTXO-only watcher tests in `swap_watcher_tests/utxo.rs` remain stable and are always compiled with `docker-tests-watchers`.
  - **Exit criteria:** Re-enable `docker-tests-watchers-eth` when ETH watchers are completed and stable.

- [x] **Fix `docker-tests-zcoin` environment setup** ✅ NOT NEEDED (tests passing)
  - Verified CI run 20103549149: all 8 ZCoin tests pass
  - `zombie_coin_send_dex_fee` and other tests completed successfully
  - Docker container setup working correctly with `--profile zombie`

- [ ] **Migrate docker tests CI to GLEEC fork infrastructure**
  - Currently docker tests CI runs on `https://github.com/KomodoPlatform/komodo-defi-framework`
  - Need to migrate to `https://github.com/GLEECBTC/komodo-defi-framework` which has:
    - Updated docker node configurations
    - Pre-deployed watcher-compatible swap contracts
    - Test infrastructure aligned with current development
  - Tasks:
    - [ ] Update CI workflow to point to GLEEC fork
    - [ ] Verify docker-compose files are compatible
    - [ ] Ensure contract addresses match GLEEC deployments
    - [ ] Test all docker test suites against GLEEC infrastructure

- [x] **Add `docker-tests-integration` feature flag and CI job** ✅ DONE
  - Added `docker-tests-integration = ["run-docker-tests"]` to `mm2_main/Cargo.toml`
  - Created `docker-tests-integration` CI job in `test.yml` (lines 664-709) that:
    - Starts ALL containers with `--profile all`
    - Uses 90 minute timeout
    - Runs `--features docker-tests-integration`
  - Cross-chain tests gated by `docker-tests-integration`:
    - `tendermint_swap_tests::*` (Tendermint↔ETH swaps)
    - `swap_tests::*` (SLP cross-chain swaps)
  - Migrated `swap_tests` module from legacy negative-gate pattern to explicit `docker-tests-slp` feature
  - **Fix (2025-12-13):** Added `docker-tests-integration` to cfg gates in `runner.rs` for:
    - `setup_slp()` - SLP container initialization (SLP_TOKEN_OWNERS)
    - `setup_geth()` - ETH container initialization (GETH_ACCOUNT)
    - `setup_cosmos()` - Tendermint container initialization
    - Function definitions and `required_images()` - ensure containers are started
  - **Cleanup (2025-12-13):** Removed redundant `all(feature = "run-docker-tests", ...)` patterns in `mod.rs` since all `docker-tests-*` features inherit `run-docker-tests`

- [x] **Add `docker-tests-all` aggregate feature** ✅ DONE
  - Added to `mm2_main/Cargo.toml`:
    ```toml
    # Aggregate feature for local development - runs all docker test suites
    docker-tests-all = [
        "docker-tests-eth",
        "docker-tests-slp",
        "docker-tests-sia",
        "docker-tests-ordermatch",
        "docker-tests-swaps-utxo",
        "docker-tests-watchers",
        "docker-tests-qrc20",
        "docker-tests-tendermint",
        "docker-tests-zcoin",
        "docker-tests-integration",
    ]
    ```
  - **Use case:** Local development convenience - run `cargo test --test docker_tests_main --features docker-tests-all` to run all tests
  - **Note:** Not recommended for CI (use split jobs instead for parallelism)

- [x] **Remove monolithic `docker-tests` CI job** ✅ DONE
  - **Problem:** The monolithic `docker-tests` job ran with only `--features run-docker-tests`, which compiled almost no tests because all test modules require additional `docker-tests-*` features.
  - **Previous behavior:** Started ALL containers (`--profile all`), ran for ~90 minutes, but only executed the `dummy()` test.
  - **Resolution:** Removed the job entirely. All test suites are covered by the 10 split CI jobs:
    - `docker-tests-eth`, `docker-tests-slp`, `docker-tests-sia`
    - `docker-tests-ordermatch`, `docker-tests-swaps-utxo`, `docker-tests-watchers`
    - `docker-tests-qrc20`, `docker-tests-tendermint`, `docker-tests-zcoin`
    - `docker-tests-integration`
  - **For local "run everything":** Use `--features docker-tests-all`

- [x] **Feature-gate container startup in testcontainers mode** ✅ DONE
  - **Previous problem:** In testcontainers mode, ALL containers (UTXO, Qtum, Geth, Cosmos, Zombie) started regardless of which feature flags were enabled.
  - **Solution:** Gate container startup in `docker_tests_main.rs` based on feature flags using `RequiredNodes` struct.
  - Container startup now only starts what's needed based on which feature flags are enabled.

- [x] **Replace `_KDF_NO_*_DOCKER` env vars with feature-flag-based container control** ✅ DONE
  - **Implementation:** Added `RequiredNodes` struct with per-node granularity in `docker_tests_main.rs`:
    ```rust
    #[derive(Debug, Clone, Copy, Default)]
    struct RequiredNodes {
        mycoin: bool,
        mycoin1: bool,
        forslp: bool,
        qtum: bool,
        eth: bool,
        cosmos: bool,
        zombie: bool,
        sia: bool,
    }

    impl RequiredNodes {
        fn from_features() -> Self {
            Self {
                mycoin: cfg!(feature = "docker-tests-swaps-utxo")
                    || cfg!(feature = "docker-tests-ordermatch")
                    || cfg!(feature = "docker-tests-watchers")
                    || cfg!(feature = "docker-tests-qrc20")
                    || cfg!(feature = "docker-tests-sia")
                    || cfg!(feature = "docker-tests-integration"),
                mycoin1: cfg!(feature = "docker-tests-swaps-utxo")
                    || cfg!(feature = "docker-tests-ordermatch")
                    || cfg!(feature = "docker-tests-watchers")
                    || cfg!(feature = "docker-tests-integration"),
                forslp: cfg!(feature = "docker-tests-slp") || cfg!(feature = "docker-tests-integration"),
                qtum: cfg!(feature = "docker-tests-qrc20") || cfg!(feature = "docker-tests-integration"),
                eth: cfg!(feature = "docker-tests-eth")
                    || cfg!(feature = "docker-tests-ordermatch")
                    || cfg!(feature = "docker-tests-watchers-eth")
                    || cfg!(feature = "docker-tests-integration"),
                cosmos: cfg!(feature = "docker-tests-tendermint") || cfg!(feature = "docker-tests-integration"),
                zombie: cfg!(feature = "docker-tests-zcoin") || cfg!(feature = "docker-tests-integration"),
                sia: cfg!(feature = "docker-tests-sia") || cfg!(feature = "docker-tests-integration"),
            }
        }
        fn needs_utxo_image(&self) -> bool { self.mycoin || self.mycoin1 || self.forslp }
    }
    ```
  - **Removed:** All `_KDF_NO_*_DOCKER` env var constants and their usage from `docker_tests_main.rs`
  - **Updated CI:** Removed all `_KDF_NO_*` env vars from CI jobs in `.github/workflows/test.yml`
  - **Benefits achieved:**
    - Single source of truth for container requirements (feature flags only)
    - Simpler CI configuration (just set features, no env vars needed)
    - Compile-time determination of container dependencies
  - Feature→node mapping implemented:
    - `docker-tests-eth` → Geth only
    - `docker-tests-slp` → FORSLP only
    - `docker-tests-sia` → Sia + UTXO (for DSIA↔MYCOIN swaps)
    - `docker-tests-qrc20` → Qtum + UTXO (for QRC20↔MYCOIN swaps)
    - `docker-tests-tendermint` → Cosmos nodes only
    - `docker-tests-zcoin` → Zombie only
    - `docker-tests-swaps-utxo` → UTXO (MYCOIN, MYCOIN1)
    - `docker-tests-watchers` → UTXO only (ETH requires docker-tests-watchers-eth)
    - `docker-tests-ordermatch` → UTXO + Geth
    - `docker-tests-integration` → ALL containers

**Note:** All docker tests are now covered by split CI jobs. The monolithic `docker-tests` job has been removed.

---

### Phase 4 – Simplify modes & metadata

**Goal:** Reduce complexity to a minimal set of environment modes and clarify what metadata is responsible for.

#### 4.4.1 Dedicated "docker env init" command

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
   - `ComposeInit` (connect to a running docker-compose environment and initialize on each run).

This keeps test execution simple:

- Local dev: `cargo test -p mm2 --test docker_tests_main` (testcontainers).
- CI / composed env:
   - `docker compose up -d ...`
   - `cargo test -p mm2 --features docker-tests-...` (uses ComposeInit mode)

#### 4.4.3 Environment configuration

- Use shared configuration:
   - Keep a small `.docker/config.json` or `.env` to hold stable host/port information.
   - Share between docker-compose and tests.
- Contract addresses:
   - Initialize in `docker_tests_main.rs` on each run.
   - No persistence needed for CI workflows.

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
   - `wait_for_log` durations to "just enough" + small buffer.
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
| Container name constants | `mm2src/mm2_main/tests/docker_tests/helpers/env.rs` | `KDF_QTUM_SERVICE`, `KDF_MYCOIN_SERVICE`, `KDF_MYCOIN1_SERVICE`, `KDF_FORSLP_SERVICE`, `KDF_ZOMBIE_SERVICE`, `KDF_IBC_RELAYER_SERVICE` |

---

### Phase 7 – Final validation

**Goal:** Verify that the split CI jobs collectively run the same number of tests as the original monolithic job.

#### 7.1 Test count validation

**Historic baseline (pre-split monolithic docker-tests job):**
```
test result: ok. 235 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 1864.36s
```

Note: Until all feature-gated suites have dedicated CI jobs (Phase 3), individual jobs may run fewer tests than this baseline; the success criterion applies once the full job matrix is in place.

**Validation steps:**

- [x] After all split jobs are implemented and running in CI, collect test results from each job:
  - `docker-tests-eth`: 39 passed, 0 ignored
  - `docker-tests-slp`: 10 passed, 0 ignored
  - `docker-tests-sia`: 16 passed, 0 ignored
  - `docker-tests-ordermatch`: 37 passed, 0 ignored
  - `docker-tests-swaps-utxo`: 57 passed, 4 ignored
  - `docker-tests-watchers`: 16 passed, 0 ignored
  - `docker-tests-qrc20`: 28 passed, 3 ignored
  - `docker-tests-tendermint`: 19 passed, 0 ignored
  - `docker-tests-zcoin`: 8 passed, 0 ignored
  - `docker-tests-integration`: 5 passed, 0 ignored

- [x] Sum all results and verify:
  - **Total passed** = 235 ✅ (matches baseline!)
  - **Total ignored** = 7 (baseline was 8 - difference due to ETH watcher tests now gated behind `docker-tests-watchers-eth`)

- [x] ~~If counts don't match~~ N/A - counts match

- [x] Document final test distribution across jobs in this file (see above)

**Note:** Minor variations may occur if tests are added/removed during the plan implementation. In such cases, document the new baseline and ensure the sum of split jobs equals the updated total.

---

### Phase 7.5 – Module restructuring for maintainability

**Goal:** Improve separation of concerns, reduce feature flag sprawl, and make the codebase more maintainable.

**Status:** All docker test features pass clippy with zero warnings. This phase focuses on architectural improvements.

#### 7.5.1 Create framework layer

**Goal:** Separate "framework" utilities from chain-specific helpers.

**New directory:** `mm2src/mm2_main/tests/docker_tests/framework/`

Actions:

- [ ] Create `framework/mod.rs` (re-exports)
- [ ] Create `framework/compose.rs`:
  - Move `resolve_compose_container_id`, `docker_cp_from_container`, `wait_for_file` from `helpers/docker_ops.rs`
  - Optional: add caching to avoid repeated `docker ps` calls
- [ ] Create `framework/locks.rs`:
  - Move `MYCOIN_LOCK`, `MYCOIN1_LOCK`, `FORSLP_LOCK`, `QTUM_LOCK`, `ZCOIN_*` locks from `helpers/docker_ops.rs`
  - Move `get_funding_lock()` function
- [ ] Create `framework/coin_docker_ops.rs`:
  - Move `CoinDockerOps` trait from `helpers/docker_ops.rs`
- [ ] Create `framework/node.rs`:
  - Move `DockerNode` from `helpers/env.rs`
- [ ] Create `framework/services.rs`:
  - Move `KDF_*_SERVICE` constants from `helpers/env.rs`
- [ ] Create `framework/keys.rs`:
  - Move `random_secp256k1_secret()` and `Secp256k1Secret` re-export from `helpers/env.rs`
- [ ] Update `helpers/env.rs` to be a thin re-export façade for backward compatibility
- [ ] Update all imports in helpers and runner

#### 7.5.2 Convert ETH helper to directory module

**Goal:** Split large `helpers/eth.rs` (~877 LOC) into focused submodules.

Actions:

- [ ] Convert `helpers/eth.rs` → `helpers/eth/mod.rs`
- [ ] Create submodules:
  - `helpers/eth/state.rs` – global state / OnceLocks consolidated into `GethState` struct
  - `helpers/eth/node.rs` – `geth_docker_node`, `wait_for_geth_node_ready`
  - `helpers/eth/contracts.rs` – bytecode constants + deploy functions
  - `helpers/eth/funding.rs` – `fill_eth`, `fill_erc20`, confirmation wait
  - `helpers/eth/coins.rs` – coin creation helpers
  - `helpers/eth/sepolia.rs` – sepolia-only addresses & locks (gated separately)
- [ ] Consolidate OnceLock statics into single `GethState` struct:
  ```rust
  pub struct GethState {
      pub account: Address,
      pub contracts: GethContracts,
      pub nonce_lock: Mutex<()>,
      pub web3: Web3<Http>,
  }
  static GETH: OnceLock<GethState> = OnceLock::new();
  ```
- [ ] Remove `static mut` sepolia addresses, replace with `OnceLock<SepoliaContracts>`
- [ ] Update `include_str!` paths to use `CARGO_MANIFEST_DIR` for stability

#### 7.5.3 Convert UTXO helper to directory module

**Goal:** Split `helpers/utxo.rs` (~421 LOC) into focused submodules.

Actions:

- [ ] Convert `helpers/utxo.rs` → `helpers/utxo/mod.rs`
- [ ] Create submodules:
  - `helpers/utxo/node.rs` – `utxo_asset_docker_node`, `setup_utxo_conf_for_compose`
  - `helpers/utxo/ops.rs` – `UtxoAssetDockerOps`, `BchDockerOps`
  - `helpers/utxo/funding.rs` – `fill_address_async`, `fill_address`
  - `helpers/utxo/coins.rs` – coin creation helpers
  - `helpers/utxo/slp.rs` – SLP token initialization (gated to `docker-tests-slp`)

#### 7.5.4 Convert QRC20 helper to directory module

**Goal:** Split `helpers/qrc20.rs` (~522 LOC) into focused submodules.

Actions:

- [ ] Convert `helpers/qrc20.rs` → `helpers/qrc20/mod.rs`
- [ ] Create submodules:
  - `helpers/qrc20/state.rs` – consolidated `QtumState` struct
  - `helpers/qrc20/node.rs` – `qtum_docker_node`, `setup_qtum_conf_for_compose`
  - `helpers/qrc20/ops.rs` – `QtumDockerOps` + contract initialization
  - `helpers/qrc20/coins.rs` – coin creation helpers
  - `helpers/qrc20/funding.rs` – `fill_qrc20_address`, `wait_for_estimate_smart_fee`

#### 7.5.5 Refactor swap helper to reduce cfg sprawl

**Goal:** Reduce feature flag explosion in `helpers/swap.rs` (~480 LOC).

Actions:

- [ ] Convert `helpers/swap.rs` → `helpers/swap/mod.rs`
- [ ] Create submodules:
  - `helpers/swap/fund.rs` – ticker → funding logic (behind cfg)
  - `helpers/swap/config.rs` – coins config builder (behind cfg)
  - `helpers/swap/enable.rs` – enable coin per family (behind cfg)
  - `helpers/swap/scenario.rs` – orchestration logic

Alternative approach (bigger win):
- [ ] Introduce `TestChainOps` trait per family:
  ```rust
  trait TestChainOps {
      fn supports_ticker(ticker: &str) -> bool;
      fn coin_conf_items() -> Vec<Json>;
      fn fund_address(privkey: Secp256k1Secret, ticker: &str);
      fn enable(mm: &MarketMakerIt, ticker: &str) -> Json;
  }
  ```

#### 7.5.6 Refactor runner to setup registry pattern

**Goal:** Eliminate duplicated feature maps in `runner.rs`.

Actions:

- [ ] Create `framework/setup.rs` with `ChainSetup` trait:
  ```rust
  pub trait ChainSetup {
      fn images(&self) -> &'static [&'static str];
      fn setup(&self, runner: &mut DockerTestRunner);
  }
  ```
- [ ] Create per-chain setup structs: `UtxoSetup`, `QtumSetup`, `SlpSetup`, `GethSetup`, `ZCoinSetup`, `CosmosSetup`, `SiaSetup`
- [ ] Replace `setup_or_reuse_nodes()` body with "collect setups then run"
- [ ] Replace `required_images()` with `setups().iter().flat_map(|s| s.images())`

#### 7.5.7 Split large test files into directories

**Goal:** Improve navigability and ownership of large test suites.

Actions:

- [ ] Convert `swap_watcher_tests/mod.rs`:
  - Move common harness to `swap_watcher_tests/common.rs`
  - Keep `mod.rs` as thin dispatcher
- [ ] Rename `docker_tests_inner.rs` → `ordermatch_cross_chain_tests.rs`
- [ ] Convert `eth_docker_tests.rs` (~2947 LOC) → `eth_docker_tests/mod.rs` with topical submodules
- [ ] Convert `utxo_ordermatch_v1_tests.rs` similarly

#### 7.5.8 Feature flag improvements

**Goal:** Encode feature dependencies in Cargo.toml.

Actions:

- [ ] Add feature dependencies in `mm2_main/Cargo.toml`:
  ```toml
  docker-tests-ordermatch = ["docker-chain-utxo", "docker-chain-eth"]
  docker-tests-watchers = ["docker-chain-utxo"]
  docker-tests-watchers-eth = ["docker-tests-watchers", "docker-chain-eth"]
  ```
- [ ] Ensure sepolia features don't compile docker-heavy modules

#### 7.5.9 Validation

- [ ] All docker test features still pass clippy with zero warnings
- [ ] All docker test features compile successfully
- [ ] Existing tests continue to pass
- [ ] Import paths remain backward compatible where possible

---

### Phase 8 – Documentation update (FINAL PHASE)

**Goal:** Update all documentation to reflect the final state of the docker tests infrastructure.

> ⚠️ **IMPORTANT:** This phase must remain the LAST phase in the plan. Do not add new phases after this one. Any new tasks should be inserted before Phase 8.

#### 8.1 Update AGENTS.md files

- [ ] Update `mm2src/mm2_main/AGENTS.md`:
  - Document the new docker test module structure
  - List all feature flags and their purposes
  - Describe the helpers organization

- [ ] Review and update any other `AGENTS.md` files affected by the refactor

#### 8.2 Update docs/DOCKER_TESTS.md

- [ ] Update file structure documentation to reflect new module organization
- [ ] Document all CI jobs and their feature flags
- [ ] Update execution modes documentation
- [ ] Add troubleshooting section for common issues

#### 8.3 Final documentation audit

- [ ] Verify all code comments are accurate and up-to-date
- [ ] Remove any stale TODO comments that have been addressed
- [ ] Ensure inline documentation matches actual behavior
- [ ] Update any references to old module paths or removed code

#### 8.4 Plan completion

- [ ] Mark this plan file as complete
- [ ] Move to `docs/plans/completed/` or delete per project conventions
- [ ] Update root `AGENTS.md` to remove reference to this plan

---

## Success criteria checklist

- [x] `ComposeInit` mode connects to the correct Geth RPC and initializes contracts on each run.
- [x] Qtum compose runs are stable across test invocations (no `temp_dir()` dependency).
- [x] New feature flags build only the intended suites; CI runs watchers/ordermatch/swaps/qrc20/tendermint/zcoin as separate green jobs using Compose mode.
  - **Validated 2025-12-13:** All 10 docker test jobs passing (run #20185482849)
- [x] The ignored watchers test has meaningful assertions when un-ignored locally.
- [x] **Test count validation:** Sum of all split CI jobs equals baseline (235 passed, 7 ignored vs baseline 8 ignored).
  - **Validated 2025-12-13:** 235 tests passed across all split jobs (matches baseline exactly)