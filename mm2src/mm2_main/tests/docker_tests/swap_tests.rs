//! SLP platform swap tests.
//!
//! These tests exercise swaps where a platform coin and its SLP token are traded
//! against each other on the same underlying chain:
//! - `FORSLP` (BCH-like UTXO chain with SLP support)
//! - `ADEXSLP` (SLP token on `FORSLP`)
//!
//! This is not a multi-chain integration scenario; it only requires the `FORSLP`
//! docker node.

use crate::docker_tests::helpers::swap::trade_base_rel;

/// Test atomic swap with SLP token as maker coin.
/// Requires: FORSLP node only (both coins are on the same platform)
#[test]
fn trade_test_with_maker_slp() {
    trade_base_rel(("ADEXSLP", "FORSLP"));
}

/// Test atomic swap with SLP token as taker coin.
/// Requires: FORSLP node only (both coins are on the same platform)
#[test]
fn trade_test_with_taker_slp() {
    trade_base_rel(("FORSLP", "ADEXSLP"));
}
