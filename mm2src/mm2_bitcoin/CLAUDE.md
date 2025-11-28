# mm2_bitcoin — UTXO Primitives

Low-level primitives for all UTXO-based coins (Bitcoin, Komodo, Litecoin, etc.). Named "bitcoin" historically but used across all UTXO protocols.

**Note:** This is a workspace of sub-crates, not a single crate. Each sub-crate is published separately.

## Responsibilities

- Cryptographic hash functions (SHA256, RIPEMD160, Groestl, Keccak)
- Key management (private, public, keypairs)
- Address encoding (Legacy, SegWit, CashAddress)
- Transaction structures and serialization
- Script building and signing
- SPV validation and proof verification
- Block header handling

## Sub-Crate Structure

```
mm2_bitcoin/
├── primitives/           # Core types: H160, H256, U256, bytes, compact
├── crypto/               # Hash functions (crate: bitcrypto)
├── keys/                 # Address and key management
├── chain/                # Block and transaction structures
├── script/               # Bitcoin scripting language
├── serialization/        # Binary encoding/decoding
├── serialization_derive/ # Derive macros for serialization
├── rpc/                  # RPC response types
├── spv_validation/       # SPV proof verification
└── test_helpers/         # Testing utilities
```

## primitives

Core data types used throughout:

```rust
// Hash types
pub struct H160([u8; 20]);  // RIPEMD160, address hash
pub struct H256([u8; 32]);  // SHA256, tx/block hash
pub struct H512([u8; 64]);  // Groestl512

// Big integer for difficulty
pub struct U256(4);  // 256-bit unsigned

// Compact difficulty representation
pub struct Compact(u32);
```

## crypto (bitcrypto)

Cryptographic hash functions:

| Function | Output | Usage |
|----------|--------|-------|
| `sha256(data)` | H256 | Single SHA256 |
| `dhash256(data)` | H256 | Double SHA256 (Bitcoin standard) |
| `ripemd160(data)` | H160 | RIPEMD160 |
| `dhash160(data)` | H160 | SHA256 + RIPEMD160 (address hash) |
| `groestl512(data)` | H512 | Groestl (GRS) |
| `keccak256(data)` | H256 | Keccak (SMART) |
| `checksum(data, type)` | H32 | 4-byte checksum |

Checksum variants:
- `DSHA256` — Most coins (Bitcoin default)
- `DGROESTL512` — Groestlcoin
- `KECCAK256` — SmartCash

## keys

Address and key management:

### Address Types
```rust
pub enum AddressFormat {
    Standard,      // Legacy P2PKH/P2SH
    Segwit,        // Native SegWit (bech32)
    CashAddress,   // Bitcoin Cash format
}

pub struct Address {
    pub format: AddressFormat,
    pub network: Network,
    pub script_type: AddressScriptType,
    pub hash: AddressHashEnum,
}
```

### Key Types
```rust
pub struct Private { /* 32-byte secret */ }
pub struct Public { /* 33 or 65 byte pubkey */ }
pub struct KeyPair { private: Private, public: Public }
pub type Secret = H256;
pub type Message = H256;
```

### Address Hash
```rust
pub enum AddressHashEnum {
    AddressHash(H160),        // P2PKH, P2SH, P2WPKH
    WitnessScriptHash(H256),  // P2WSH
}
```

## chain

Block and transaction structures:

```rust
// Block header
pub struct BlockHeader {
    pub version: u32,
    pub previous_header_hash: H256,
    pub merkle_root_hash: H256,
    pub time: u32,
    pub bits: BlockHeaderBits,
    pub nonce: BlockHeaderNonce,
}

// Transaction output reference
pub struct OutPoint {
    pub hash: H256,
    pub index: u32,
}

// Full transaction
pub struct Transaction {
    pub version: i32,
    pub inputs: Vec<TransactionInput>,
    pub outputs: Vec<TransactionOutput>,
    pub lock_time: u32,
}
```

## script

Bitcoin scripting:

```rust
// Build scripts
let script = Builder::default()
    .push_opcode(Opcode::OP_DUP)
    .push_opcode(Opcode::OP_HASH160)
    .push_bytes(&pubkey_hash)
    .push_opcode(Opcode::OP_EQUALVERIFY)
    .push_opcode(Opcode::OP_CHECKSIG)
    .into_script();

// Script types
pub enum ScriptType {
    NonStandard,
    PubKey,
    PubKeyHash,
    ScriptHash,
    Multisig,
    NullData,
    WitnessScript,
    WitnessKey,
}

// Transaction signing
pub struct TransactionInputSigner { /* ... */ }
pub enum SignatureVersion { Base, WitnessV0 }
```

## serialization

Binary encoding for network protocol:

```rust
// Serialize to bytes
let bytes: Vec<u8> = serialize(&transaction);

// Deserialize from bytes
let tx: Transaction = deserialize(&bytes)?;

// Compact integer encoding
pub struct CompactInteger(u64);

// Streaming
let mut stream = Stream::new();
transaction.serialize(&mut stream);
```

## spv_validation

Simplified Payment Verification:

```rust
// SPV configuration
pub struct SPVConf {
    pub starting_block_header: BlockHeader,
    pub validation_params: BlockHeaderValidationParams,
}

// Validate headers
helpers_validation::validate_headers(&headers, &params)?;

// Storage trait for headers
pub trait BlockHeaderStorageOps {
    fn get_header(&self, height: u64) -> Result<BlockHeader>;
    fn add_headers(&self, headers: &[BlockHeader]) -> Result<()>;
}
```

## Interactions

| Crate | Usage |
|-------|-------|
| **coins/utxo** | Primary consumer for UTXO coin implementations |
| **coins/z_coin** | Zcash uses extended transaction types |
| **mm2_main/lp_swap** | Transaction building for atomic swaps |
| **utxo_signer** | UTXO transaction signing |
| **trezor** | Key types for hardware wallet signing |
| **crypto** | Key pair types, address hashing |

## Coin-Specific Variants

| Coin Type | Hash | Address | Notes |
|-----------|------|---------|-------|
| Bitcoin | DSHA256 | Legacy/SegWit | Standard |
| Komodo | DSHA256 | Legacy | Zcash-derived |
| Litecoin | DSHA256 | Legacy/SegWit | Different prefixes |
| Bitcoin Cash | DSHA256 | CashAddress | Different format |
| Groestlcoin | DGROESTL512 | Legacy/SegWit | Different hash |
| SmartCash | KECCAK256 | Legacy | Different hash |
| Zcash | DSHA256 | Legacy | Shielded extensions |

## Common Patterns

### Creating Address from Public Key
```rust
let pubkey_hash = dhash160(&public_key.serialize());
let address = AddressBuilder::new()
    .network(Network::Mainnet)
    .address_type(AddressScriptType::P2PKH)
    .hash(pubkey_hash.into())
    .build()?;
```

### Signing Transaction
```rust
let signer = TransactionInputSigner::new(tx, script_pubkey, amount);
let signature = signer.sign(
    &private_key,
    input_index,
    SignatureVersion::Base,
)?;
```

## Tests

Each sub-crate has its own tests:
```bash
cargo test -p primitives
cargo test -p bitcrypto
cargo test -p keys
cargo test -p chain
cargo test -p script
cargo test -p serialization
cargo test -p spv_validation
```
