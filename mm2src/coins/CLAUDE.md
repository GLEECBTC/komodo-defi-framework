# coins — Multi-Protocol Coin Support

Abstraction layer for blockchain protocols. Defines traits for swaps, balances, and transactions.

## Responsibilities

- Unified coin interface via `MmCoin` and `MmCoinEnum`
- Protocol implementations: UTXO, EVM, Tendermint, Zcash, Lightning, Sia, Solana
- HTLC operations for atomic swaps
- HD wallet address derivation
- Transaction building and signing
- NFT operations (EVM)

## Trait Hierarchy

```
MmCoinEnum (wrapper for all coin types)
    ↓ Deref to dyn MmCoin

MmCoin (universal interface)
├── MarketCoinOps   # Balance, fees, addresses, signing
├── SwapOps         # V1 HTLC: send, validate, refund
├── MakerCoinSwapOpsV2  # V2 maker operations
├── TakerCoinSwapOpsV2  # V2 taker operations
├── WatcherOps      # Third-party spend/refund
└── HDWalletCoinOps # HD address derivation (optional)
```

### Core Traits (lp_coins.rs)

| Trait | Required For | Key Methods |
|-------|--------------|-------------|
| `MarketCoinOps` | All coins | `ticker()`, `my_balance()`, `send_raw_tx()` |
| `SwapOps` | V1 swaps | `send_maker_payment()`, `validate_taker_payment()` |
| `MakerCoinSwapOpsV2` | V2 maker | `send_maker_payment_v2()`, `refund_maker_payment_v2()` |
| `TakerCoinSwapOpsV2` | V2 taker | `send_taker_funding()`, `sign_and_send_taker_funding_spend()` |
| `WatcherOps` | Watcher support | `watcher_validate_taker_fee()` |
| `HDWalletCoinOps` | HD wallets | `derive_address()`, `create_new_account()` |

## Adding a New Coin

### 1. Choose Base Implementation

| Type | Base | Examples |
|------|------|----------|
| UTXO | `UtxoStandardCoin` | BTC, LTC, KMD |
| EVM | `EthCoin` | ETH, MATIC, BNB |
| ERC20 | `EthCoin` (token) | USDT, WBTC |
| Tendermint | `TendermintCoin` | ATOM, OSMO |
| Custom | New struct | ZEC, SIA |

### 2. Implement Traits

Minimum for swap support:
```rust
impl MarketCoinOps for MyCoin { ... }
impl SwapOps for MyCoin { ... }  // Or V2 traits
impl MmCoin for MyCoin { ... }
```

### 3. Add to MmCoinEnum

In `lp_coins.rs`:
```rust
pub enum MmCoinEnum {
    // ...existing
    MyCoin(MyCoin),
}
impl From<MyCoin> for MmCoinEnum { ... }
```

### 4. Add Activation

In `coins_activation/`:
- Platform: `platform_coin_with_tokens.rs`
- Standalone: `standalone_coin/init_standalone_coin.rs`
- Token: `init_token.rs`

## Protocol Specifics

### UTXO (utxo.rs, utxo/)
- `UtxoCoinConf`: Network params, address prefixes
- `UtxoRpcClientEnum`: Electrum or Native RPC
- SPV validation via `mm2_bitcoin/spv_validation`
- Address formats: Standard, Segwit, CashAddress

### EVM (eth.rs, eth/)
- `EthCoinType`: `Eth`, `Erc20 { token_addr }`, `Nft`
- `EthPrivKeyPolicy`: Iguana/HD/Trezor/MetaMask/WalletConnect
- Gas constants: `ETH_PAYMENT = 65_000`, `ERC20_PAYMENT = 150_000`
- NFT swap support via `SwapV2Contracts`

### Tendermint (tendermint/)
- IBC token transfers
- Staking (experimental namespace)
- HTLC via Iris/Nucleus modules

### Zcash (z_coin.rs)
- Shielded transactions (Sapling proofs)
- Lightwalletd or Electrum data source

## HD Wallet Integration

### Key Traits (hd_wallet/)
- `HDWalletCoinOps`: Coin-level derivation
- `HDAccountOps`: Account management
- `HDAddressOps`: Address operations

### Task RPCs
- `task::get_new_address::{init,status,user_action,cancel}`
- `task::init_account_balance::{init,status}`
- `task::init_create_account::{init,status,user_action,cancel}`

## Activation Patterns

### Platform + Tokens
```rust
"enable_eth_with_tokens" → enable_platform_coin_with_tokens()
"enable_erc20" → enable_token()
```

### Task-Based (complex init)
```rust
"task::enable_utxo::init" → init_standalone_coin()
"task::enable_utxo::status" → init_standalone_coin_status()
```

## Interactions

| Crate | Usage |
|-------|-------|
| **mm2_main** | Swap engines call coin traits |
| **crypto** | `PrivKeyBuildPolicy` for key derivation |
| **coins_activation** | Initialization flows |
| **mm2_bitcoin** | UTXO primitives (chain, keys, script, serialization) |
| **mm2_number** | MmNumber for amounts |
| **mm2_core** | MmArc context access, CoinsContext storage |
| **mm2_err_handle** | MmError framework |
| **trezor** | Hardware wallet signing |
| **utxo_signer** | UTXO transaction signing (sub-crate) |
| **mm2_net** | HTTP transport for RPC calls |
| **mm2_db** | IndexedDB storage (WASM) |
| **db_common** | SQLite storage (native) |

## Key Invariants

- `MmCoinEnum` wraps all coin types for unified handling
- Coins must be activated before use (`lp_coinfind_or_err`)
- Token activation requires platform coin first
- HD mode requires `GlobalHDAccountCtx` in crypto context

## Common Errors

| Error | Cause | Fix |
|-------|-------|-----|
| `NoSuchCoin` | Coin not activated | Call activation RPC first |
| `UnexpectedDerivationMethod` | Wrong key policy | Check HD vs Iguana mode |
| `BalanceError` | RPC failure | Verify node connectivity |
| `InvalidAddress` | Format mismatch | Check address prefix/format |

## Tests

- Unit: `cargo test -p coins --lib`
- Integration: Tests in `mm2_main/tests/`
- Docker: Protocol-specific tests in `docker_tests/`
