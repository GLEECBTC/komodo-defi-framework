# mm2_main — RPC, Swaps, and Application Logic

> **Note:** Always follow the root `/CLAUDE.md` for global conventions (fmt, clippy, error handling, etc.).

Core application crate: RPC dispatcher, atomic swap engines, order matching, streaming.

## Responsibilities

- Application entry point and lifecycle (`lp_native_dex.rs`)
- RPC request routing and handler registration
- Atomic swap state machines (V1 legacy, V2 TPU)
- Order matching and orderbook management
- SSE event streaming
- Swap watcher coordination
- WalletConnect session management

## Module Structure

```
src/
├── mm2.rs                # Library entry point
├── lp_native_dex.rs      # Application lifecycle (lp_main, lp_run)
├── rpc/
│   ├── dispatcher/       # RPC routing
│   │   ├── dispatcher.rs # Main v2 dispatcher
│   │   └── dispatcher_legacy.rs
│   ├── lp_commands/      # Handler implementations
│   │   ├── pubkey.rs, tokens.rs, trezor.rs, db_id.rs, legacy.rs
│   │   ├── one_inch.rs, one_inch/  # 1inch integration
│   │   └── lr_swap.rs, lr_swap/    # Liquidity routing
│   ├── streaming_activations/  # SSE handlers
│   │   ├── balance.rs, orderbook.rs, swaps.rs, orders.rs
│   │   ├── heartbeat.rs, network.rs, shutdown_signal.rs
│   │   ├── tx_history.rs, fee_estimation.rs, disable.rs
│   │   └── mod.rs
│   ├── wc_commands/      # WalletConnect RPCs
│   └── rate_limiter.rs   # Request rate limiting
├── lp_swap/
│   ├── maker_swap.rs     # V1 maker state machine
│   ├── taker_swap.rs     # V1 taker state machine
│   ├── maker_swap_v2.rs  # V2 maker (TPU protocol)
│   ├── taker_swap_v2.rs  # V2 taker (TPU protocol)
│   ├── swap_watcher.rs   # Watcher node logic
│   ├── swap_v2_rpcs.rs   # V2 RPC handlers
│   ├── trade_preimage.rs # Fee estimation
│   ├── swap_lock.rs, swap_events.rs, saved_swap.rs
│   └── pubkey_banning.rs, check_balance.rs
├── lp_ordermatch/        # Order matching engine
│   ├── best_orders.rs    # Best order selection
│   ├── orderbook_rpc.rs  # Orderbook RPCs
│   ├── simple_market_maker.rs  # Market maker bot
│   └── order_events.rs, orderbook_events.rs
├── lp_wallet/            # Wallet management, mnemonic storage
├── lp_init/              # Hardware wallet init (Trezor, MetaMask)
│   ├── init_hw.rs, init_metamask.rs, init_context.rs
├── lp_network.rs         # P2P message handling
├── lp_healthcheck.rs     # Peer health checking
├── lp_stats.rs           # Version statistics
└── database/             # Swap/order persistence
```

## RPC Patterns

### Adding a Handler

1. Create handler in `rpc/lp_commands/<feature>.rs`:
```rust
pub async fn my_handler(ctx: MmArc, req: MyRequest) -> MmResult<MyResponse, MyError> {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await?;
    // Business logic here
    Ok(MyResponse { ... })
}
```

2. Register in `dispatcher/dispatcher.rs`:
```rust
// In dispatcher_v2() match:
"my_method" => handle_mmrpc(ctx, request, my_handler).await,
```

### Namespace Routing

| Prefix | Router | Use Case |
|--------|--------|----------|
| (none) | `dispatcher_v2` | Stable APIs |
| `task::` | `rpc_task_dispatcher` | Long-running ops |
| `stream::` | `rpc_streaming_dispatcher` | SSE subscriptions |
| `gui_storage::` | `gui_storage_dispatcher` | GUI state (planned for removal) |
| `experimental::` | `experimental_rpcs_dispatcher` | Unstable APIs |
| `lightning::` | `lightning_dispatcher` | Lightning Network (native only) |

### Task RPC Pattern

Long-running operations follow four-verb pattern:
```rust
"task::withdraw::init"        // Start, returns task_id
"task::withdraw::status"      // Poll completion
"task::withdraw::user_action" // Handle prompts (PIN, confirm)
"task::withdraw::cancel"      // Abort
```

Use `RpcTaskManager` for task lifecycle management.

## Swap Engines

### V1 (Legacy)
- `MakerSwap` / `TakerSwap` structs
- Message-driven with timeout-based state transitions
- Coin implements `SwapOps` trait
- Use for: critical fixes only

### V2 (TPU Protocol)
- `MakerSwapStateMachine` / `TakerSwapStateMachine`
- Deterministic phases via `mm2_state_machine`
- Persistence and reentrancy support
- Watcher rewards integration
- Both coins implement `MakerCoinSwapOpsV2` + `TakerCoinSwapOpsV2`
- Use for: all new features

### Key Invariants

- **Atomicity**: Swaps complete fully or refund entirely (HTLC guarantee)
- **Timelocks**: Maker locktime > taker locktime (prevents race conditions)
- **Secret flow**: Maker generates secret → Taker reveals via spend
- **State persistence**: Swaps survive restarts via DB checkpointing

## Swap Watcher

Third-party nodes assisting swap completion when participants offline.

- P2P topic: `"swpwtchr"`
- Can spend maker payment or refund taker payment
- Log markers: `MAKER_PAYMENT_SPEND_SENT_LOG`, `TAKER_PAYMENT_REFUND_SENT_LOG`
- Locking prevents duplicate processing (`SwapWatcherLock`)

## Streaming (SSE)

Enable via `stream::<event>::enable`, disable via `stream::disable`:
- `balance::enable` — Account balance updates
- `swap_status::enable` — Swap state changes
- `orderbook::enable` — Orderbook changes
- `order_status::enable` — Order state changes
- `heartbeat::enable` — Keep-alive
- `network::enable` — Network connectivity
- `tx_history::enable` — Transaction history updates
- `fee_estimator::enable` — Gas fee updates
- `shutdown_signal::enable` — Shutdown notifications (native, non-Windows only)

## Interactions

| Crate | Usage |
|-------|-------|
| **coins** | `lp_coinfind_or_err`, coin traits for swap operations |
| **crypto** | Key derivation for swap secrets |
| **mm2_core** | `MmArc` context access |
| **mm2_p2p** | Gossipsub for swap/order messages |
| **coins_activation** | Coin initialization |
| **mm2_event_stream** | StreamingManager for SSE |
| **rpc_task** | RpcTaskManager for long-running operations |
| **kdf_walletconnect** | WalletConnect session management |
| **mm2_number** | MmNumber for amounts |
| **mm2_net** | HTTP transport (native) |
| **mm2_gui_storage** | GUI state persistence (planned for removal, unused) |
| **trading_api** | 1inch integration |

## Common Pitfalls

| Issue | Solution |
|-------|----------|
| Handler not found | Register in correct dispatcher match arm |
| Task never completes | Check `RpcTaskManager` status handling |
| Swap stuck | Verify timelock calculations and coin connectivity |
| Stream not receiving | Confirm subscription via `stream::*::enable` |

## Tests

- Unit tests: `cargo test -p mm2_main --lib`
- Integration: `cargo test --test mm2_tests_main`
- Docker swaps: `cargo test --test docker_tests_main --features run-docker-tests`
- TRON tests: `cargo test --test mm2_tests_main --features tron-network-tests tron_`

### Docker Test Infrastructure

Docker tests run against local blockchain test nodes to verify atomic swap functionality. Tests are split into feature-gated modules for parallel CI execution.

#### Test Module Structure

```
tests/docker_tests/
├── helpers/
│   ├── docker_ops.rs      # CoinDockerOps trait (shared by utxo, zcoin)
│   ├── env.rs             # MM_CTX, service constants, DockerNode
│   ├── eth.rs             # Geth/ERC20 helpers (contracts, funding)
│   ├── mod.rs             # Module index
│   ├── qrc20.rs           # Qtum/QRC20 helpers
│   ├── sia.rs             # Sia helpers
│   ├── swap.rs            # Cross-chain swap orchestration (trade_base_rel)
│   ├── tendermint.rs      # Tendermint/Cosmos/IBC helpers
│   ├── utxo.rs            # UTXO coin helpers (MYCOIN, BCH/SLP)
│   └── zcoin.rs           # ZCoin/Zombie helpers
├── swap_watcher_tests/
│   ├── eth.rs             # ETH watcher tests (disabled by default)
│   ├── mod.rs             # Watcher test helpers
│   └── utxo.rs            # UTXO watcher tests (stable)
├── docker_ordermatch_tests.rs    # Cross-chain ordermatching
├── docker_tests_inner.rs         # Mixed ETH/UTXO integration
├── eth_docker_tests.rs           # ETH/ERC20/NFT coin & swap v2 tests
├── eth_inner_tests.rs            # ETH-only ordermatching/wallet tests
├── qrc20_tests.rs                # Qtum/QRC20 tests
├── runner.rs                     # Container startup/initialization
├── sia_docker_tests.rs           # Sia tests
├── slp_tests.rs                  # SLP/BCH tests
├── swap_proto_v2_tests.rs        # UTXO swap protocol v2
├── swap_tests.rs                 # Cross-chain SLP swaps
├── swaps_confs_settings_sync_tests.rs
├── swaps_file_lock_tests.rs
├── tendermint_swap_tests.rs      # Tendermint cross-chain swaps
├── tendermint_tests.rs           # Cosmos/IBC tests
├── utxo_ordermatch_v1_tests.rs   # UTXO-only ordermatching
├── utxo_swaps_v1_tests.rs        # UTXO swap protocol v1
└── z_coin_docker_tests.rs        # ZCoin/Zombie tests
```

#### Feature Flags

| Feature | Purpose | Containers |
|---------|---------|------------|
| `docker-tests-eth` | ETH/ERC20/NFT tests | Geth |
| `docker-tests-slp` | BCH/SLP token tests | FORSLP |
| `docker-tests-sia` | Sia tests + DSIA swaps | Sia + UTXO |
| `docker-tests-ordermatch` | Orderbook/matching tests | UTXO + Geth |
| `docker-tests-swaps` | Swap protocol tests | UTXO |
| `docker-tests-watchers` | UTXO watcher tests | UTXO |
| `docker-tests-watchers-eth` | ETH watcher tests (unstable) | UTXO + Geth |
| `docker-tests-qrc20` | Qtum/QRC20 tests | Qtum + UTXO |
| `docker-tests-tendermint` | Cosmos/IBC tests | Cosmos |
| `docker-tests-zcoin` | ZCoin/Zombie tests | Zombie |
| `docker-tests-integration` | Cross-chain swaps | ALL |
| `docker-tests-all` | All suites (local dev) | ALL |

#### Running Tests

```bash
# Single suite
cargo test --test docker_tests_main --features docker-tests-eth

# All suites (local development)
cargo test --test docker_tests_main --features docker-tests-all

# With docker-compose (faster iteration)
KDF_DOCKER_COMPOSE_ENV=1 cargo test --test docker_tests_main --features docker-tests-eth
```

See [`docs/DOCKER_TESTS.md`](../../../docs/DOCKER_TESTS.md) for full setup and troubleshooting.
