# trezor — Hardware Wallet Integration

> **Note:** Always follow the root `/CLAUDE.md` for global conventions (fmt, clippy, error handling, etc.).

Trezor hardware wallet API for UTXO and EVM transaction signing. Handles device communication, user interactions (PIN/passphrase/button), and transaction signing protocols.

## Responsibilities

- Device communication via Transport trait (USB, WebUSB, UDP)
- Session management with mutex-protected device access
- User interaction handling (PIN matrix, passphrase, button confirm)
- UTXO address derivation and transaction signing
- EVM address derivation and transaction signing (Legacy, EIP-1559)
- Protobuf message encoding/decoding

## Module Structure

```
src/
├── lib.rs                # Exports, TrezorMessageType enum
├── client.rs             # TrezorClient, TrezorSession
├── response.rs           # TrezorResponse enum, interaction requests
├── response_processor.rs # TrezorRequestProcessor trait
├── result_handler.rs     # ResultHandler for response parsing
├── error.rs              # TrezorError, OperationFailure
├── device_info.rs        # TrezorDeviceInfo from Features
├── user_interaction.rs   # PIN/passphrase response types
├── trezor_rpc_task.rs    # RPC task integration
├── transport/            # Communication layer
│   ├── mod.rs           # Transport trait, device IDs
│   ├── protocol.rs      # Wire protocol
│   ├── usb.rs           # Native USB (rusb)
│   ├── webusb.rs        # WASM WebUSB
│   └── udp.rs           # Emulator (testing)
├── proto/                # Protobuf messages
│   ├── mod.rs           # ProtoMessage, TrezorMessage trait
│   ├── messages.rs      # MessageType enum
│   ├── messages_common.rs
│   ├── messages_management.rs
│   ├── messages_bitcoin.rs
│   ├── messages_ethereum.rs
│   └── messages_ethereum_definitions.rs
├── utxo/                 # Bitcoin/UTXO operations
│   ├── mod.rs
│   ├── utxo_command.rs  # get_utxo_address, get_public_key
│   ├── sign_utxo.rs     # Transaction signing
│   ├── unsigned_tx.rs   # UnsignedUtxoTx types
│   └── prev_tx.rs       # Previous transaction data
└── eth/                  # Ethereum operations
    ├── mod.rs
    ├── eth_command.rs   # get_eth_address, sign_eth_tx
    └── definitions/     # Network definition files (.dat)
```

## Core Types

### TrezorClient & TrezorSession

```rust
// Thread-safe client with mutex-protected access
pub struct TrezorClient {
    inner: Arc<AsyncMutex<TrezorClientImpl>>,
}

// Holds exclusive device access during operations
pub struct TrezorSession<'a> {
    inner: AsyncMutexGuard<'a, TrezorClientImpl>,
    pub processor: Option<Arc<dyn TrezorRequestProcessor<Error = RpcTaskError>>>,
}

// Create and use
let client = TrezorClient::from_transport(usb_transport);
let (device_info, session) = client.init_new_session(processor).await?;
let address = session.get_utxo_address(path, coin, false, None).await?;
```

### TrezorResponse

Device responses can be ready or require user interaction:

```rust
pub enum TrezorResponse<'a, 'b, T> {
    Ready(T),                           // Result available
    ButtonRequest(ButtonRequest),       // Needs button confirm
    PinMatrixRequest(PinMatrixRequest), // Needs PIN entry
    PassphraseRequest(PassphraseRequest), // Needs passphrase
}
```

### TrezorRequestProcessor

Trait for handling user interactions during device operations:

```rust
#[async_trait]
pub trait TrezorRequestProcessor: Send + Sync {
    type Error: NotMmError + Send;

    async fn on_button_request(&self) -> MmResult<(), TrezorProcessingError<Self::Error>>;
    async fn on_pin_request(&self) -> MmResult<TrezorPinMatrix3x3Response, TrezorProcessingError<Self::Error>>;
    async fn on_passphrase_request(&self) -> MmResult<TrezorPassphraseResponse, TrezorProcessingError<Self::Error>>;
    async fn on_ready(&self) -> MmResult<(), TrezorProcessingError<Self::Error>>;
}
```

### Transport Trait

Platform-agnostic device communication:

```rust
#[async_trait]
pub trait Transport {
    async fn session_begin(&mut self) -> TrezorResult<()>;
    async fn session_end(&mut self) -> TrezorResult<()>;
    async fn write_message(&mut self, message: ProtoMessage) -> TrezorResult<()>;
    async fn read_message(&mut self) -> TrezorResult<ProtoMessage>;
}
```

## Device Operations

### UTXO Operations

```rust
// Get UTXO address
session.get_utxo_address(
    path,           // DerivationPath
    "Bitcoin",      // Coin name for Trezor
    false,          // show_display
    Some(script_type),
).await?

// Get public key (xpub)
session.get_public_key(
    path,
    "Bitcoin",
    EcdsaCurve::Secp256k1,
    false,          // show_display
    true,           // ignore_xpub_magic
).await?
```

### EVM Operations

```rust
// Get Ethereum address
session.get_eth_address(&derivation_path, false).await?

// Sign transaction (Legacy or EIP-1559)
session.sign_eth_tx(&derivation_path, &unsigned_tx, chain_id).await?
```

## Platform Support

| Transport | Platform | Feature Flag |
|-----------|----------|--------------|
| USB (rusb) | Native (Linux, macOS, Windows) | default |
| WebUSB | WASM (browser) | default |
| UDP | Native (emulator testing) | `trezor-udp` |

Note: iOS not supported (no USB access).

## Error Handling

```rust
pub enum TrezorError {
    TransportNotSupported { transport },
    ErrorRequestingAccessPermission(String),  // Browser permission denied
    DeviceDisconnected,
    UnderlyingError(String),
    ProtocolError(String),
    UnexpectedMessageType(MessageType),
    Failure(OperationFailure),
    UnexpectedInteractionRequest(TrezorUserInteraction),
    Internal(String),
    PongMessageMismatch,
    InternalNoProcessor,
}

pub enum OperationFailure {
    InvalidPin,
    UnexpectedMessage,
    ButtonExpected,
    DataError,
    PinExpected,
    InvalidSignature,
    ProcessError,
    NotEnoughFunds,
    NotInitialized,
    WipeCodeMismatch,
    InvalidSession,
    FirmwareError,
    FailureMessageNotFound,
    UserCancelled,
}
```

## Interactions

| Crate | Usage |
|-------|-------|
| **coins/utxo** | Hardware wallet signing via `PrivKeyBuildPolicy::Trezor` |
| **coins/eth** | EVM hardware wallet signing |
| **crypto** | `PrivKeyBuildPolicy` determines when Trezor is used |
| **rpc_task** | Integrates with task system for async user interaction |
| **hw_common** | Shared hardware wallet transport abstractions |
| **mm2_bitcoin** | Key and transaction types (chain, keys, script) |
| **mm2_err_handle** | MmError framework |

## Key Invariants

- Device access is mutex-protected; only one operation at a time
- Session must be initialized before operations (`init_new_session`)
- All signing operations require a `TrezorRequestProcessor` for user interactions
- PIN is entered via 3x3 matrix mapping (not actual digits)
- EIP-2930 transactions not supported

## Adding New Coin Support

For UTXO coins:
1. Ensure coin name is recognized by Trezor firmware
2. Use existing `get_utxo_address` / `get_public_key` methods
3. Script type must match derivation path (e.g., `m/84'` → SegWit)

For EVM chains:
1. Add network definition file to `eth/definitions/`
2. Register in `ETH_NETWORK_DEFS` map
3. For tokens, add to `ETH_TOKEN_DEFS` map

## Tests

- Unit: `cargo test -p trezor --lib`
- With emulator: Enable `trezor-udp` feature, run Trezor emulator

## Common Pitfalls

| Issue | Solution |
|-------|----------|
| "Device disconnected" during operation | Ensure session lock held throughout |
| Wrong address for script type | Derivation path must match script type |
| Signing hangs | Implement `TrezorRequestProcessor` to handle button/PIN prompts |
| EVM signing fails on custom chain | Add network definition to `ETH_NETWORK_DEFS` |
