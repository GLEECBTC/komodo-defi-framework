# coins_activation — Coin Activation Flows

Manages the lifecycle of cryptocurrency activation. Handles standalone coins, platform coins with tokens, and L2 layers.

## Responsibilities

- Task-based coin activation via `RpcTaskManager`
- Platform coin + token initialization (ETH+ERC20, BCH+SLP, Tendermint+IBC, Solana+SPL)
- Standalone coin activation (ZCash, Sia)
- L2/Lightning activation (native only)
- Hardware wallet interaction during activation
- Transaction history fetching initiation

## Module Structure

```
src/
├── lib.rs                        # Exports all activation functions
├── prelude.rs                    # Common imports
├── context.rs                    # CoinsActivationContext with task managers
├── platform_coin_with_tokens.rs  # Platform + tokens activation trait/impl
├── standalone_coin/              # Standalone coin activation
│   ├── init_standalone_coin.rs   # Generic standalone activation
│   └── init_standalone_coin_error.rs
├── token.rs                      # Token-only activation (enable_token)
├── init_token.rs                 # Task-based token init
├── l2/                           # Lightning/L2 activation
│   ├── init_l2.rs
│   └── init_l2_error.rs
├── eth_with_token_activation.rs  # ETH + ERC20/NFT
├── erc20_token_activation.rs     # ERC20 token activation
├── init_erc20_token_activation.rs # Task-based ERC20 init
├── bch_with_tokens_activation.rs # BCH + SLP
├── slp_token_activation.rs       # SLP token activation
├── tendermint_with_assets_activation.rs # Tendermint + IBC
├── tendermint_token_activation.rs # Tendermint token activation
├── solana_with_assets.rs         # Solana + SPL (experimental)
├── solana_token_activation.rs    # SPL token activation
├── utxo_activation/              # UTXO coin activation
│   ├── init_utxo_standard_activation.rs
│   ├── init_bch_activation.rs
│   ├── init_qtum_activation.rs
│   └── common_impl.rs
├── z_coin_activation.rs          # ZCash
├── sia_coin_activation.rs        # Sia
└── lightning_activation.rs       # Lightning (native only)
```

## Activation Patterns

### 1. Platform Coin with Tokens

For coins that host tokens (ETH, BCH, Tendermint):

```rust
// RPC: "task::enable_eth_with_tokens::init"
trait PlatformCoinWithTokensActivationOps {
    async fn enable_platform_coin(...) -> Result<Self, Error>;
    async fn enable_global_nft(...) -> Result<Option<MmCoinEnum>, Error>;
    fn token_initializers(&self) -> Vec<Box<dyn TokenAsMmCoinInitializer>>;
    async fn get_activation_result(...) -> Result<ActivationResult, Error>;
}
```

Flow:
1. Check if already activated
2. Load platform config and protocol
3. Create platform coin instance
4. Initialize tokens via `token_initializers()`
5. Enable global NFT if applicable
6. Get activation result (block height, balances)
7. Start tx history fetching if enabled
8. Register with `CoinsContext`

### 2. Standalone Coins

For coins without token support (ZCash, Sia):

```rust
// RPC: "task::enable_z_coin::init"
trait InitStandaloneCoinActivationOps {
    async fn init_standalone_coin(...) -> Result<Self, Error>;
    async fn get_activation_result(...) -> Result<ActivationResult, Error>;
    fn start_history_background_fetching(...);
}
```

### 3. Token-Only Activation

For adding tokens to already-active platform:

```rust
// RPC: "enable_erc20", "enable_slp"
trait TokenActivationOps {
    async fn enable_token(...) -> Result<Self, Error>;
}
```

## Core Types

### CoinsActivationContext

Central context holding all task managers:

```rust
struct CoinsActivationContext {
    init_utxo_standard_task_manager: UtxoStandardTaskManagerShared,
    init_eth_task_manager: EthTaskManagerShared,
    init_z_coin_task_manager: ZcoinTaskManagerShared,
    init_tendermint_coin_task_manager: TendermintCoinTaskManagerShared,
    // ... more task managers
}
```

### RpcTaskManager

Handles async activation lifecycle:
- `spawn_rpc_task()` — Start activation, returns `task_id`
- `task_status()` — Poll completion status
- `on_user_action()` — Handle HW wallet prompts
- `cancel_task()` — Abort activation

### Activation Status States

```rust
enum InitPlatformCoinWithTokensInProgressStatus {
    ActivatingCoin,
    SyncingBlockHeaders { current_scanned_block, last_block },
    RequestingWalletBalance,
    WaitingForTrezorToConnect,
    FollowHwDeviceInstructions,
    Finishing,
}
```

## RPC Endpoints

| Pattern | Init | Status | User Action | Cancel |
|---------|------|--------|-------------|--------|
| Platform+Tokens | `task::enable_eth_with_tokens::init` | `::status` | `::user_action` | `::cancel` |
| Standalone | `task::enable_z_coin::init` | `::status` | `::user_action` | `::cancel` |
| Token | `enable_erc20` | - | - | - |
| L2 | `task::enable_lightning::init` | `::status` | `::user_action` | `::cancel` |

## Key Traits

| Trait | Purpose | Implementors |
|-------|---------|--------------|
| `PlatformCoinWithTokensActivationOps` | Platform + tokens | EthCoin, BchCoin, TendermintCoin |
| `InitStandaloneCoinActivationOps` | Standalone coins | ZCoin, SiaCoin |
| `TokenActivationOps` | Token-only | Erc20Token, SlpToken |
| `TokenInitializer` | Token creation | Erc20TokenActivator, SlpTokenActivator |

## Interactions

| Crate | Usage |
|-------|-------|
| **coins** | Coin types implement activation traits |
| **mm2_main** | RPC dispatcher routes to activation functions |
| **crypto** | `PrivKeyBuildPolicy` detection for key source |
| **rpc_task** | RpcTaskManager for task lifecycle |
| **mm2_core** | MmArc context, CoinsContext registration |
| **mm2_err_handle** | MmError framework |
| **mm2_event_stream** | Progress event streaming |
| **trezor** | Hardware wallet prompts during activation |

## Key Invariants

- Platform coin must be activated before its tokens
- Task-based activation required for hardware wallet flows
- Coin registered with `CoinsContext` only after successful activation
- Activation can be cancelled; partially activated coins are cleaned up

## Error Handling

Common activation errors:
- `PlatformIsAlreadyActivated` — Coin already active
- `PlatformConfigIsNotFound` — Missing coin config
- `TokenConfigIsNotFound` — Missing token config
- `UnexpectedPlatformProtocol` — Protocol mismatch
- `TaskTimedOut` — Activation took too long

All errors implement `HttpStatusCode` for proper RPC responses.

## Adding New Coin Activation

1. Implement appropriate trait (`PlatformCoinWithTokensActivationOps` or `InitStandaloneCoinActivationOps`)
2. Add task manager to `CoinsActivationContext`
3. Create activation module (e.g., `my_coin_activation.rs`)
4. Wire up RPC endpoints in dispatcher

## Tests

- Unit: `cargo test -p coins_activation --lib`
- Integration: Platform activation tests in `mm2_main/tests/`
