# crypto — Key Management and HD Derivation

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
    Iguana,           // Single key from passphrase
    GlobalHDAccount,  // BIP39 HD wallet
}
```

**Access patterns:**
```rust
CryptoCtx::is_init(&ctx)?;           // Check initialized
let crypto = CryptoCtx::from_ctx(&ctx)?;  // Get context
```

**Exposed (safe) data:**
- `secp256k1_key_pair` — Internal mm2 keypair
- `rmd160` — Public key hash (address ID)
- `shared_db_id` — Database namespace

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

### PrivKeyBuildPolicy

Determines key source during coin activation:

```rust
pub enum PrivKeyBuildPolicy {
    IguanaPrivKey(IguanaPrivKey),    // Legacy single-key
    GlobalHDAccount(GlobalHDAccountArc), // HD derivation
    Trezor,                          // Hardware wallet
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
- `HDPathToCoin`: Account-level path (purpose + coin_type)
- `DerivationPath`: Full path including address index

## Hardware Wallets

### Trezor (Native Only)
```rust
#[cfg(not(target_arch = "wasm32"))]
CryptoCtx::init_hw_ctx_with_trezor(processor, expected_pubkey)?;
```

### MetaMask (WASM Only)
```rust
#[cfg(target_arch = "wasm32")]
CryptoCtx::init_metamask_ctx(project_name)?;
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
| `metamask_ctx.rs` | MetaMask context (WASM) |
| `mnemonic.rs` | BIP39 mnemonic handling |
| `encrypt.rs` / `decrypt.rs` | Mnemonic encryption |
| `secret_hash_algo.rs` | Swap secret hash algorithm |
| `slip21.rs` | SLIP-21 symmetric key derivation |
| `standard_hd_path.rs` | BIP44 path types |
| `shared_db_id.rs` | Database namespace derivation |

## Tests

- Unit: `cargo test -p crypto --lib`
- Integration: HD wallet tests in `mm2_main/tests/`
