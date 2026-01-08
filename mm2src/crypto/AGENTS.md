# crypto — Key Management and HD Derivation

> **Note:** Always follow the root `/CLAUDE.md` for global conventions (fmt, clippy, error handling, etc.).

**Security-critical crate.** Handles mnemonics, seeds, key derivation, and hardware wallet integration.

## Security Rules (Non-Negotiable)

1. **NEVER log**: mnemonics, seeds, private keys, extended keys
2. **NEVER serialize** sensitive data in error messages
3. **Zeroize** secrets on drop (use `zeroize` crate)
4. **Validate** all derivation paths before use

## Responsibilities

- Cryptographic context management (`CryptoCtx`)
- BIP39/BIP32/SLIP-10/SLIP-21 HD derivation (`GlobalHDAccountCtx`)
- Key policy detection and enforcement
- Hardware wallet context (Trezor, MetaMask)
- Mnemonic encryption/decryption
- Secret hash algorithm selection for swaps

## Core Types

### CryptoCtx (crypto_ctx.rs)

Central crypto context stored in `MmArc`.

```rust
pub enum KeyPairPolicy {
    Iguana,                              // Single key from passphrase
    GlobalHDAccount(GlobalHDAccountArc), // BIP39 HD wallet
}
```

**Access patterns:**
```rust
CryptoCtx::is_init(&ctx)?;           // Check initialized
let crypto = CryptoCtx::from_ctx(&ctx)?;  // Get context
```

**Key access methods:**
- `mm2_internal_key_pair()` — Internal mm2 keypair
- `mm2_internal_pubkey()` — Public key for P2P
- `mm2_internal_public_id()` — 32-byte public ID
- `hw_wallet_rmd160()` — Hardware wallet address hash (if HW active)

### GlobalHDAccountCtx (global_hd_ctx.rs)

HD wallet context. **Internal state is never exposed.**

```rust
// Initialization (happens once at startup)
let (keypair, hd_ctx) = GlobalHDAccountCtx::new(mnemonic)?;
```

**Derivation methods:**
```rust
// secp256k1 (BIP32) — Bitcoin, Ethereum, etc.
let secret = hd_ctx.derive_secp256k1_secret(&path)?;

// ed25519 (SLIP-10) — Solana, etc.
let key = hd_ctx.derive_ed25519_signing_key(&path)?;
```

### PrivKeyBuildPolicy (in coins crate)

Determines key source during coin activation. Defined in `coins/lp_coins.rs`:

```rust
pub enum PrivKeyBuildPolicy {
    IguanaPrivKey(IguanaPrivKey),
    GlobalHDAccount(GlobalHDAccountArc),
    Trezor,
    WalletConnect { session_topic },
}

// Auto-detect from context
let policy = PrivKeyBuildPolicy::detect_priv_key_policy(&ctx)?;
```

## BIP44 Derivation Paths

```
m / purpose' / coin_type' / account' / change / address_index
m / 44'      / 60'        / 0'       / 0      / 0   (ETH first address)
m / 44'      / 141'       / 0'       / 0      / 0   (KMD first address)
```

**Path types:**
- `HDPathToCoin`: Coin-level path (purpose + coin_type)
- `HDPathToAccount`: Account-level path (purpose + coin_type + account_id)
- `DerivationPath`: Full path including address index

## Hardware Wallets

### Trezor (Native Only)
```rust
#[cfg(not(target_arch = "wasm32"))]
let crypto_ctx = CryptoCtx::from_ctx(&ctx)?;
crypto_ctx.init_hw_ctx_with_trezor(processor, expected_pubkey).await?;
```

### MetaMask (WASM Only)
```rust
#[cfg(target_arch = "wasm32")]
let crypto_ctx = CryptoCtx::from_ctx(&ctx)?;
crypto_ctx.init_metamask_ctx(project_name).await?;
```

## Common Patterns

### Deriving Coin Keys
```rust
// During coin activation:
let path = coin_conf.derivation_path()?;
let secret = hd_ctx.derive_secp256k1_secret(&path)?;
let keypair = key_pair_from_secret(&secret)?;
```

### Checking HD Mode
```rust
if ctx.enable_hd() {
    // HD wallet mode
} else {
    // Iguana legacy mode
}
```

## Interactions

| Crate | Usage |
|-------|-------|
| **coins** | Coin builders use `PrivKeyBuildPolicy` |
| **mm2_core** | `CryptoCtx` stored in `MmArc` |
| **trezor** | Hardware wallet integration |
| **mm2_metamask** | MetaMask WASM integration |
| **mm2_err_handle** | MmError framework |
| **hw_common** | Hardware wallet abstractions |
| **rpc_task** | Task-based hardware wallet flows |

## Error Types

```rust
pub enum CryptoCtxError {
    NotInitialized,
    Internal(String),
}

pub enum CryptoInitError {
    NotInitialized,
    InitializedAlready,
    EmptyPassphrase,
    InvalidPassphrase(PrivKeyError),
    Internal(String),
}
```

## Key Files

| File | Purpose |
|------|---------|
| `crypto_ctx.rs` | CryptoCtx, KeyPairPolicy |
| `global_hd_ctx.rs` | GlobalHDAccountCtx, derivation |
| `privkey.rs` | Key generation from seed |
| `hw_ctx.rs` | Hardware wallet context |
| `hw_client.rs` | Hardware wallet client traits |
| `hw_error.rs` | Hardware wallet error types |
| `hw_rpc_task.rs` | Hardware wallet RPC task types |
| `metamask_ctx.rs` | MetaMask context (WASM) |
| `metamask_login.rs` | MetaMask login request types (WASM) |
| `mnemonic.rs` | BIP39 mnemonic handling |
| `encrypt.rs` / `decrypt.rs` | Mnemonic encryption |
| `secret_hash_algo.rs` | Swap secret hash algorithm |
| `slip21.rs` | SLIP-21 symmetric key derivation |
| `standard_hd_path.rs` | BIP44 path types (StandardHDPath, HDPathToCoin) |
| `bip32_child.rs` | BIP32 derivation path building blocks |
| `shared_db_id.rs` | Database namespace derivation |
| `xpub.rs` | Extended public key handling |
| `key_derivation.rs` | Key derivation utilities |

## Tests

- Unit: `cargo test -p crypto --lib`
- Integration: HD wallet tests in `mm2_main/tests/`
