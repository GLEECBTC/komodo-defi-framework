# coins_activation — Coin Activation Flows

> **Note:** Always follow the root `/CLAUDE.md` for global conventions (fmt, clippy, error handling, etc.).

Manages the lifecycle of cryptocurrency activation. Handles standalone coins, platform coins with tokens, and L2 layers.

## Responsibilities

- Task-based coin activation via `RpcTaskManager`
- Platform coin + token initialization (ETH+ERC20, TRON, BCH+SLP, Tendermint+IBC, Solana+SPL)
- Standalone coin activation (UTXO, Qtum, ZCash, Sia)
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
│   ├── init_standalone_coin_error.rs # Standalone activation errors
│   └── mod.rs                    # Module exports
├── token.rs                      # Token-only activation (enable_token)
├── init_token.rs                 # Task-based token init
├── l2/                           # Lightning/L2 activation
│   ├── init_l2.rs                # L2 activation logic
│   ├── init_l2_error.rs          # L2 activation errors
│   └── mod.rs                    # Module exports
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
│   ├── init_utxo_standard_activation.rs # UTXO standard activation
│   ├── init_utxo_standard_activation_error.rs # UTXO activation errors
│   ├── init_utxo_standard_statuses.rs # UTXO activation status types
│   ├── utxo_standard_activation_result.rs # UTXO activation result types
│   ├── init_bch_activation.rs    # BCH standalone activation
│   ├── init_qtum_activation.rs   # Qtum standalone activation
│   ├── common_impl.rs            # Shared UTXO activation logic
│   └── mod.rs                    # Module exports
├── z_coin_activation.rs          # ZCash
├── sia_coin_activation.rs        # Sia
└── lightning_activation.rs       # Lightning (native only)
```

## Activation Patterns

### 1. Platform Coin with Tokens

For coins that host tokens (ETH, TRON, BCH, Tendermint, Solana):

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

**TRON-specific behavior:**
- Uses same `eth_with_token_activation.rs` flow as ETH
- Identified by `ChainSpec::Tron { network }` (vs `ChainSpec::Evm { chain_id }`)
- Wallet-only mode: rejects token/NFT activation requests
- Address format: `TronAddress` with Base58Check encoding (`T...` prefix)
- RPC abstraction: `ChainRpcClient::Tron` implements `ChainRpcOps` for balance/block queries

### 2. Standalone Coins

For coins without token support (UTXO, Qtum, ZCash, Sia):

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
// Task-based (preferred): "task::enable_erc20::init"
trait InitTokenActivationOps {
    async fn init_token(...) -> Result<Self, Error>;
    async fn get_activation_result(...) -> Result<ActivationResult, Error>;
}

// Request-response: "enable_erc20", "enable_slp"
trait TokenActivationOps {
    async fn enable_token(...) -> Result<Self, Error>;
}
```

### 4. L2 Activation

For Lightning Network (native only):

```rust
// RPC: "task::enable_lightning::init"
trait InitL2ActivationOps {
    async fn init_l2(...) -> Result<Self, Error>;
    async fn get_activation_result(...) -> Result<ActivationResult, Error>;
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
    TemporaryError(String),
    RequestingWalletBalance,
    WaitingForTrezorToConnect,
    FollowHwDeviceInstructions,
    Finishing,
}
```

## RPC Endpoints

Task-based (supports `::init`, `::status`, `::user_action`, `::cancel`):
| Pattern | Example |
|---------|---------|
| Platform+Tokens | `task::enable_eth::init`, `task::enable_tendermint::init` |
| Standalone | `task::enable_utxo::init`, `task::enable_z_coin::init`, `task::enable_sia::init` |
| Token | `task::enable_erc20::init` |
| L2 | `task::enable_lightning::init` |

Request-response (no task management):
| Pattern | Example |
|---------|---------|
| Platform+Tokens | `enable_eth_with_tokens`, `enable_bch_with_tokens` |
| Standalone | `enable_sia` |
| Token | `enable_erc20`, `enable_slp`, `enable_tendermint_token` |

## Key Traits

| Trait | Purpose | Implementors |
|-------|---------|--------------|
| `PlatformCoinWithTokensActivationOps` | Platform + tokens | EthCoin, BchCoin, TendermintCoin, SolanaCoin |
| `InitStandaloneCoinActivationOps` | Standalone coins | UtxoStandardCoin, QtumCoin, BchCoin, ZCoin, SiaCoin |
| `InitTokenActivationOps` | Token (task-based) | EthCoin |
| `TokenActivationOps` | Token (request-response) | EthCoin, SlpToken, TendermintToken, SolanaToken |
| `InitL2ActivationOps` | L2 activation | LightningCoin |
| `TokenInitializer` | Token creation | Erc20Initializer, SlpTokenInitializer, TendermintTokenInitializer |

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
| **common** | Utilities, HttpStatusCode, executor |
| **mm2_number** | BigDecimal for balances |
| **kdf_walletconnect** | WalletConnect context for external wallets |

## Key Invariants

- Platform coin must be activated before its tokens or L2
- Duplicate activation prevented (checked at start of each activation)
- Task-based activation required for hardware wallet flows
- Coin registered with `CoinsContext` only after successful activation
- Activation can be cancelled; partially activated coins are cleaned up

## Error Handling

Common activation errors:
- `PlatformIsAlreadyActivated` — Coin already active
- `CoinIsAlreadyActivated` — Standalone coin already active
- `PlatformCoinIsNotActivated` — Token/L2 activated before platform
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

- Integration: Platform activation tests in `mm2_main/tests/`
- TRON tests: `cargo test --test mm2_tests_main --features tron-network-tests tron_`
