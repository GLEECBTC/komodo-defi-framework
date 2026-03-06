//! Chain-aware address types for EVM and TRON.
//!
//! This module provides `ChainTaggedAddress`, a wrapper that carries chain family
//! context alongside the raw 20-byte address, enabling correct user-facing formatting
//! (EVM checksum vs TRON Base58).

use super::tron::TronAddress;
use super::{valid_addr_from_str, ChainFamily};
use crate::hd_wallet::{AddrToString, DisplayAddress};
use ethereum_types::Address as EthAddress;
use std::fmt;
use std::str::FromStr;

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// ADDRESS FORMATTING POLICY
// ═══════════════════════════════════════════════════════════════════════════════════════════════
//
// Single source of truth: `ChainFamily::format(raw)` is the canonical formatter.
// All other formatting methods delegate to it.
//
// ┌─────────────────────────────────────┬────────────────────────────────────────────────────────┐
// │ Method                              │ Use Case                                               │
// ├─────────────────────────────────────┼────────────────────────────────────────────────────────┤
// │ ChainFamily::format(raw)            │ Canonical formatter. EVM→0x checksum, TRON→T... base58 │
// ├─────────────────────────────────────┼────────────────────────────────────────────────────────┤
// │ ChainTaggedAddress::display_address │ Wallet-owned addresses from HD derivation.             │
// │                                     │ Delegates to ChainFamily::format.                      │
// ├─────────────────────────────────────┼────────────────────────────────────────────────────────┤
// │ EthCoin::format_raw_address(raw)    │ External/RPC-sourced addresses (logs, receipts,        │
// │                                     │ ownerOf, contract calls). Delegates to                 │
// │                                     │ ChainFamily::format using coin's chain spec.           │
// └─────────────────────────────────────┴────────────────────────────────────────────────────────┘
//
// NOTE: There is no `impl DisplayAddress for ethereum_types::Address`. Raw addresses cannot
// call `.display_address()` — you MUST use one of the chain-aware methods above.
//
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// Address tagged with chain family solely to format it correctly for user-facing outputs.
/// The inner bytes remain `ethereum_types::Address` (20 bytes).
///
/// This follows the UTXO pattern where different address formats have different types
/// (e.g., `Address` vs `CashAddress`). For ETH/TRON, we use the same underlying
/// 20-byte address but display it differently based on the chain family.
///
/// To get the underlying `Address`, use `.inner()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChainTaggedAddress {
    inner: EthAddress,
    family: ChainFamily,
}

impl ChainTaggedAddress {
    /// Creates a new chain-tagged address with the specified address and chain family.
    pub fn new(address: EthAddress, family: ChainFamily) -> Self {
        Self { inner: address, family }
    }

    /// Creates a chain-tagged address from a TRON address.
    ///
    /// Extracts the 20-byte EVM address (dropping the 0x41 prefix) and sets family to TRON.
    pub fn from_tron(tron_addr: TronAddress) -> Self {
        Self {
            inner: tron_addr.to_evm_address(),
            family: ChainFamily::Tron,
        }
    }

    /// Returns the underlying raw address bytes.
    #[inline]
    pub fn inner(&self) -> EthAddress {
        self.inner
    }

    /// Returns the chain family this address belongs to.
    #[inline]
    pub fn family(&self) -> ChainFamily {
        self.family
    }

    /// Parses an address string using chain-aware parsing rules.
    ///
    /// - `Evm`: Parses via `valid_addr_from_str` (validates EIP-55 checksum)
    /// - `Tron`: Parses via `TronAddress::from_str` (accepts Base58 or hex formats)
    ///
    /// This centralizes parsing so call sites don't need to import `FromStr`.
    pub fn from_str_with_family(s: &str, family: ChainFamily) -> Result<Self, String> {
        match family {
            ChainFamily::Evm => {
                let raw = valid_addr_from_str(s)?;
                Ok(Self::new(raw, ChainFamily::Evm))
            },
            ChainFamily::Tron => {
                let tron_addr = TronAddress::from_str(s).map_err(|e| e.to_string())?;
                Ok(Self::from(tron_addr))
            },
        }
    }
}

impl DisplayAddress for ChainTaggedAddress {
    /// Formats the address according to its chain family.
    ///
    /// Delegates to the canonical `ChainFamily::format` method:
    /// - EVM: EIP-55 mixed-case checksum format (`0xAbCd...`)
    /// - TRON: Base58Check format (`T...`)
    fn display_address(&self) -> String {
        self.family.format(self.inner)
    }
}

/// Enables direct use in format strings: `format!("{}", tagged_address)`
impl fmt::Display for ChainTaggedAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_address())
    }
}

/// **WARNING**: For `ChainTaggedAddress`, `addr_to_string()` returns USER-FACING display format
/// (EIP-55 checksum for EVM, Base58 for TRON), NOT raw lowercase hex.
///
/// This differs from `impl AddrToString for ethereum_types::Address` which returns raw hex.
///
/// If you need raw hex, use `tagged.inner().addr_to_string()` instead.
///
/// This impl is required for swap negotiation/storage where addresses are serialized as strings.
impl AddrToString for ChainTaggedAddress {
    fn addr_to_string(&self) -> String {
        self.display_address()
    }
}

/// Converts a TRON address to a chain-tagged address.
/// Extracts the 20-byte address (dropping the 0x41 prefix) and sets family to TRON.
impl From<TronAddress> for ChainTaggedAddress {
    fn from(tron_addr: TronAddress) -> Self {
        Self::from_tron(tron_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known TRON address used across all tests:
    /// - Base58: TNPeeaaFB7K9cmo4uQpcU32zGK8G1NYqeL
    /// - Full hex (21 bytes with prefix): 418840e6c55b9ada326d211d818c34a994aeced808
    /// - Raw EVM (20 bytes): 8840e6c55b9ada326d211d818c34a994aeced808
    const KNOWN_TRON_BASE58: &str = "TNPeeaaFB7K9cmo4uQpcU32zGK8G1NYqeL";
    const KNOWN_RAW_HEX: &str = "8840e6c55b9ada326d211d818c34a994aeced808";

    fn known_raw_address() -> EthAddress {
        let bytes = hex::decode(KNOWN_RAW_HEX).unwrap();
        EthAddress::from_slice(&bytes)
    }

    /// Verifies TRON address parsing and formatting with a known test vector.
    /// Tests: Base58 parsing, family tagging, raw byte extraction, and roundtrip.
    /// Also verifies that TronAddress::from_str accepts hex formats.
    #[test]
    fn tron_known_vector_roundtrip() {
        // Parse from Base58
        let tagged = ChainTaggedAddress::from_str_with_family(KNOWN_TRON_BASE58, ChainFamily::Tron).unwrap();

        // Verify family is TRON
        assert_eq!(tagged.family(), ChainFamily::Tron);

        // Verify raw 20-byte address matches expected hex
        assert_eq!(hex::encode(tagged.inner().as_bytes()), KNOWN_RAW_HEX);

        // Verify display_address returns original Base58
        assert_eq!(tagged.display_address(), KNOWN_TRON_BASE58);

        // Verify TronAddress::from_str also accepts hex formats (with and without 0x prefix)
        let hex_with_prefix = format!("41{}", KNOWN_RAW_HEX);
        let hex_0x_prefix = format!("0x41{}", KNOWN_RAW_HEX);

        let from_hex = TronAddress::from_str(&hex_with_prefix).expect("should parse hex without 0x");
        let from_hex_0x = TronAddress::from_str(&hex_0x_prefix).expect("should parse hex with 0x");

        // Both hex forms should produce the same raw address
        assert_eq!(from_hex.to_evm_address(), tagged.inner());
        assert_eq!(from_hex_0x.to_evm_address(), tagged.inner());
    }

    /// Verifies EVM address formatting and parsing roundtrip.
    /// Tests: EIP-55 checksum formatting, parsing, and raw byte preservation.
    #[test]
    fn evm_checksum_roundtrip() {
        let raw = known_raw_address();

        // Format as EVM checksum
        let formatted = ChainFamily::Evm.format(raw);
        assert!(formatted.starts_with("0x"), "EVM format must start with 0x");

        // Parse back
        let tagged = ChainTaggedAddress::from_str_with_family(&formatted, ChainFamily::Evm).unwrap();

        // Verify roundtrip preserves raw address
        assert_eq!(tagged.inner(), raw);

        // Verify family is EVM
        assert_eq!(tagged.family(), ChainFamily::Evm);

        // Verify formatting is stable
        assert_eq!(tagged.display_address(), formatted);
    }
}
