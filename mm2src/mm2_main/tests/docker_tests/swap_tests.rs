//! Cross-chain atomic swap tests.
//!
//! These tests require multiple blockchain nodes running simultaneously
//! and are executed in the main docker-tests job (not chain-specific jobs).
//!
//! Tests in this module are excluded from chain-specific CI jobs (e.g., docker-tests-slp)
//! because they need multiple chain types to be available.

use crate::docker_tests::docker_tests_common::*;

/// Test atomic swap with SLP token as maker coin.
/// Requires: FORSLP node + counterparty chain node (QTUM for QRC20)
#[test]
fn trade_test_with_maker_slp() {
    trade_base_rel(("ADEXSLP", "FORSLP"));
}

/// Test atomic swap with SLP token as taker coin.
/// Requires: FORSLP node + counterparty chain node (QTUM for QRC20)
#[test]
fn trade_test_with_taker_slp() {
    trade_base_rel(("FORSLP", "ADEXSLP"));
}
