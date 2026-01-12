//! Chain-agnostic RPC abstraction layer for EthCoin.
//!
//! This module provides `ChainRpcOps`, a trait that abstracts over different blockchain
//! RPC backends (EVM JSON-RPC, TRON HTTP API, etc.). The `ChainRpcClient` enum implements
//! explicit match dispatch to route calls to the appropriate backend.
//!
//! # Design Rationale
//!
//! We use an enum + explicit match pattern rather than `Deref<Target = dyn Trait>` because:
//! - Async traits with `dyn` require boxing and have ergonomic issues
//! - Explicit matching is clearer and more maintainable
//! - Each variant can have chain-specific error types converted at the boundary
//!
//! See `docs/plans/tron-hd-activation-v2.md` Section 18 for why generic `EthCoin<B>` isn't feasible.
//!
//! # TODO: Module Structure Refactoring
//!
//! This module should be expanded into a proper submodule structure:
//!
//! ```text
//! mm2src/coins/eth/rpc/
//! ├── mod.rs           # Re-exports, ChainRpcClient enum
//! ├── traits.rs        # RpcPool trait, ChainRpcOps trait
//! ├── evm/
//! │   ├── mod.rs
//! │   ├── client.rs    # Single-node EVM client
//! │   ├── pool.rs      # EvmRpcPool (implements RpcPool)
//! │   └── methods.rs   # EVM-specific RPC methods
//! └── tron/
//!     ├── mod.rs
//!     ├── client.rs    # TronHttpClient (single node)
//!     ├── pool.rs      # TronRpcPool (implements RpcPool)
//!     └── methods.rs   # TRON-specific RPC methods
//! ```
//!
//! See `docs/plans/chain-rpc-client-refactor.md` for the full plan.

use async_trait::async_trait;
use ethereum_types::U256;
use mm2_err_handle::prelude::*;

use super::tron::{TronAddress, TronApiClient};
use super::Web3RpcError;

// ----------------------------------------------------------------------------
// ChainRpcOps Trait
// ----------------------------------------------------------------------------

/// Chain-agnostic RPC operations trait.
///
/// Implementors provide chain-specific RPC functionality while exposing a unified interface.
/// Associated types allow each implementation to define its own error, address, and balance types.
#[async_trait]
pub trait ChainRpcOps: Send + Sync + std::fmt::Debug {
    /// Chain-specific error type.
    type Error;
    /// Chain-specific address type.
    type Address;
    /// Chain-specific balance type.
    type Balance;

    /// Get the current block number.
    async fn current_block(&self) -> Result<u64, Self::Error>;

    /// Get native token balance for an address.
    async fn balance_native(&self, address: Self::Address) -> Result<Self::Balance, Self::Error>;

    /// Check if an address has been used on-chain (basic check).
    ///
    /// For TRON: checks if the account exists meaningfully (has balance, create_time, or permissions).
    /// For EVM: checks transaction count and balance.
    async fn is_address_used_basic(&self, address: Self::Address) -> Result<bool, Self::Error>;
}

// ----------------------------------------------------------------------------
// ChainRpcClient Enum
// ----------------------------------------------------------------------------

/// Unified RPC client that dispatches to chain-specific implementations.
///
/// Uses explicit match dispatch pattern for clarity and to handle async traits cleanly.
#[derive(Debug, Clone)]
pub enum ChainRpcClient {
    /// TRON blockchain RPC client (uses TRON HTTP API).
    Tron(TronApiClient),
    /// EVM-compatible blockchain RPC client (uses JSON-RPC).
    Evm(EvmRpcClient),
}

// ----------------------------------------------------------------------------
// EvmRpcClient Placeholder
// ----------------------------------------------------------------------------

/// Placeholder for EVM JSON-RPC client.
///
/// Full implementation deferred to Phase 4. Currently EVM calls go through
/// existing EthCoin methods directly.
#[derive(Debug, Clone)]
pub struct EvmRpcClient {
    // Will contain Web3 transport and node rotation logic
    _placeholder: (),
}

// ----------------------------------------------------------------------------
// ChainRpcClient Dispatch Implementation
// ----------------------------------------------------------------------------

impl ChainRpcClient {
    /// Get the current block number.
    ///
    /// Dispatches to the appropriate chain-specific implementation.
    pub async fn current_block(&self) -> MmResult<u64, ChainRpcError> {
        match self {
            ChainRpcClient::Tron(client) => client.current_block().await.mm_err(ChainRpcError::Tron),
            ChainRpcClient::Evm(_client) => {
                // TODO: Phase 4 - implement EVM current_block
                MmError::err(ChainRpcError::NotImplemented("EVM current_block".into()))
            },
        }
    }

    /// Get native token balance for an address.
    ///
    /// For TRON addresses, use `TronAddress`. For EVM, use `ethereum_types::Address`.
    pub async fn balance_native_tron(&self, address: &TronAddress) -> MmResult<U256, ChainRpcError> {
        match self {
            ChainRpcClient::Tron(client) => client.balance_native(*address).await.mm_err(ChainRpcError::Tron),
            ChainRpcClient::Evm(_) => MmError::err(ChainRpcError::WrongChain {
                expected: "Tron",
                got: "Evm",
            }),
        }
    }

    /// Check if a TRON address has been used on-chain.
    pub async fn is_address_used_tron(&self, address: &TronAddress) -> MmResult<bool, ChainRpcError> {
        match self {
            ChainRpcClient::Tron(client) => client.is_address_used_basic(*address).await.mm_err(ChainRpcError::Tron),
            ChainRpcClient::Evm(_) => MmError::err(ChainRpcError::WrongChain {
                expected: "Tron",
                got: "Evm",
            }),
        }
    }
}

// ----------------------------------------------------------------------------
// ChainRpcError
// ----------------------------------------------------------------------------

/// Unified error type for ChainRpcClient dispatch layer.
///
/// Wraps chain-specific errors and adds dispatch-level errors.
#[derive(Debug, derive_more::Display)]
pub enum ChainRpcError {
    /// TRON API error (uses Web3RpcError internally).
    #[display(fmt = "TRON error: {}", _0)]
    Tron(Web3RpcError),

    /// EVM RPC error.
    #[display(fmt = "EVM error: {}", _0)]
    Evm(Web3RpcError),

    /// Method not yet implemented.
    #[display(fmt = "Not implemented: {}", _0)]
    NotImplemented(String),

    /// Wrong chain type for the requested operation.
    #[display(fmt = "Wrong chain: expected {}, got {}", expected, got)]
    WrongChain { expected: &'static str, got: &'static str },
}

// NOTE: Intentionally no `impl From<MmError<Web3RpcError>> for ChainRpcError`.
// Such a conversion would ambiguously map to either Tron or Evm variant.
// Callers should explicitly construct ChainRpcError::Tron or ChainRpcError::Evm.
