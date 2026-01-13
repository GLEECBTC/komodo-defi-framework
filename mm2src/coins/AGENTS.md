# coins — Multi-Protocol Coin Support

> **Note:** Always follow the root `/CLAUDE.md` for global conventions (fmt, clippy, error handling, etc.).

Abstraction layer for blockchain protocols. Defines traits for swaps, balances, and transactions.

## Responsibilities

- Unified coin interface (`MmCoin` trait, `MmCoinEnum` wrapper)
- Protocol implementations: UTXO, EVM, Tendermint, Zcash, Lightning, Sia, Solana
- Swap trait implementations for atomic swap protocols
- HD wallet management (accounts, addresses, derivation, storage)
- Transaction building, signing, broadcasting, and history
- Balance tracking
- NFT support (EVM chains)
- Price fetching

## Core Traits

`MmCoinEnum` wraps all coin types, derefs to `dyn MmCoin`.

| Trait | Purpose | Key Methods |
|-------|---------|-------------|
| `MmCoin` | Universal interface | Base trait all coins implement |
| `MarketCoinOps` | Balance, fees, addresses | `ticker()`, `my_balance()`, `send_raw_tx()` |
| `SwapOps` | V1 HTLC operations | `send_maker_payment()`, `validate_taker_payment()` |
| `CommonSwapOpsV2` | Shared V2 swap operations | `derive_htlc_pubkey_v2()` |
| `MakerCoinSwapOpsV2` | V2 maker operations | `send_maker_payment_v2()`, `refund_maker_payment_v2()` |
| `TakerCoinSwapOpsV2` | V2 taker operations | `send_taker_funding()`, `sign_and_send_taker_funding_spend()` |
| `MakerNftSwapOpsV2` | V2 NFT maker operations | NFT-specific swap methods |
| `WatcherOps` | Third-party spend/refund | `watcher_validate_taker_fee()` |
| `HDWalletCoinOps` | HD wallet coin operations | `derive_address()`, `derive_addresses()` |
| `CoinWithPrivKeyPolicy` | Key policy access | `priv_key_policy()` |

*Additional specialized traits exist for RPC transport, EIP-1559, balance updates, etc.*

## Adding a New Coin

### 1. Choose Base Implementation

| Type | Base | Examples |
|------|------|----------|
| UTXO | `UtxoStandardCoin` | BTC, LTC, KMD |
| UTXO (Qtum) | `QtumCoin` | QTUM |
| UTXO (BCH) | `BchCoin` | BCH (with SLP support) |
| SLP Token | `SlpToken` | SLP tokens on BCH |
| QRC20 | `Qrc20Coin` | QRC20 tokens on Qtum |
| EVM | `EthCoin` | ETH, MATIC, BNB |
| TRON | `EthCoin` | TRX (wallet-only, via ChainSpec::Tron) |
| ERC20/NFT | `EthCoin` (token) | USDT, WBTC, NFTs |
| Tendermint | `TendermintCoin` | ATOM, OSMO |
| Tendermint Token | `TendermintToken` | IBC tokens |
| Lightning | `LightningCoin` | BTC Lightning (native only) |
| Solana | `SolanaCoin` | SOL |
| SPL Token | `SolanaToken` | SPL tokens on Solana |
| Zcash-based | `ZCoin` | ZEC, ARRR (Pirate) |
| Sia | `SiaCoin` | SIA |

### 2. Implement Traits

Required for `MmCoin` (trait bound: `SwapOps + WatcherOps + MarketCoinOps`):
```rust
impl MarketCoinOps for MyCoin { ... }  // Addresses, balance, tx broadcast, signing
impl SwapOps for MyCoin { ... }        // HTLC operations
impl WatcherOps for MyCoin { ... }     // Default impls available
impl MmCoin for MyCoin { ... }         // Withdraw, history, fees, confirmations
```

See Core Traits table and existing implementations for additional traits (V2 swaps, HD wallet, NFT).

### 3. Add to MmCoinEnum

In `lp_coins.rs`:
```rust
pub enum MmCoinEnum {
    // ...existing
    MyCoinVariant(MyCoin),
}
impl From<MyCoin> for MmCoinEnum { ... }
```

### 4. Add Activation

See `coins_activation/AGENTS.md`. Activation traits (task-based `Init*` traits take precedence as they support all wallet types):
- Platform: `PlatformCoinWithTokensActivationOps`
- Standalone: `InitStandaloneCoinActivationOps` (preferred), `StandaloneCoinActivationOps`
- Token: `InitTokenActivationOps` (preferred), `TokenActivationOps`

## Protocol Specifics

### UTXO (utxo.rs, utxo/)
- `UtxoCoinConf`: Network params, address prefixes
- `UtxoRpcClientEnum`: Electrum or Native RPC
- SPV validation via `mm2_bitcoin/spv_validation`
- Address formats: Standard, Segwit, CashAddress
- WalletConnect: P2PKH/P2WPKH/P2SH signing via PSBT (`utxo/wallet_connect.rs`)

### EVM (eth.rs, eth/)
- `ChainSpec`: `Evm { chain_id }` or `Tron { network }` - determines chain behavior
- `EthCoinType`: `Eth`, `Erc20 { token_addr }`, `Nft`
- `EthPrivKeyPolicy`: Iguana/HD/Trezor/MetaMask/WalletConnect
- Gas constants: `ETH_PAYMENT = 65_000`, `ERC20_PAYMENT = 150_000`
- NFT swap support via `SwapV2Contracts`

### TRON (eth/tron/)
- Reuses `EthCoin` with `ChainSpec::Tron { network }`
- `TronAddress`: Base58Check encoding (`T...` format)
- `TronApiClient`: HTTP RPC client (native + WASM)
- `ChainRpcClient::Tron`: Implements `ChainRpcOps` for balance, block, address-used checks
- Wallet-only mode (no swap contracts yet)
- HD activation via `enable_eth_with_tokens` / `task::enable_eth::*`

### Tendermint (tendermint/)
- IBC token transfers
- Staking (experimental namespace)
- HTLC via Iris/Nucleus modules

### Zcash (z_coin.rs)
- Shielded transactions (Sapling proofs)
- Lightwalletd or Electrum data source

### Lightning (lightning.rs)
- Native only, uses rust-lightning (LDK)
- Channel management, invoice payments
- Swap support via HTLC invoices

### Sia (siacoin.rs)
- Minimal implementation
- Basic wallet operations, HTLC swaps
- Missing: watchers, tx history v2, dynamic fees

### Solana (solana/)
- Work in progress
- Basic wallet operations implemented
- Missing: swap operations, most MmCoin methods

## HD Wallet Integration

Key traits in `hd_wallet/`:
- `HDWalletCoinOps`: Coin-level derivation
- `HDWalletOps`: Wallet operations (accounts, gap limit)
- `HDAccountOps`: Account management
- `HDAddressOps`: Address operations

## Interactions

| Crate | Usage |
|-------|-------|
| **mm2_main** | Swap engines call coin traits |
| **crypto** | `PrivKeyBuildPolicy` for key derivation |
| **coins_activation** | Initialization flows |
| **common** | Time utilities, DEX fee constants |
| **mm2_bitcoin** | UTXO primitives (chain, keys, script, serialization) |
| **mm2_number** | MmNumber for amounts |
| **mm2_core** | MmArc context access, CoinsContext storage |
| **mm2_err_handle** | MmError framework |
| **trezor** | Hardware wallet signing |
| **kdf_walletconnect** | WalletConnect v2 signing (EVM + UTXO) |
| **utxo_signer** | UTXO transaction signing (sub-crate) |
| **mm2_net** | HTTP transport for RPC calls |
| **mm2_db** | IndexedDB storage (WASM) |
| **db_common** | SQLite storage (native) |

## Key Invariants

- Coins must be activated before use (`lp_coinfind_or_err`)
- Token activation requires platform coin first
- HD mode requires `GlobalHDAccountCtx` in crypto context

## Common Pitfalls

*Add pitfalls here when the same issue is encountered multiple times.*
