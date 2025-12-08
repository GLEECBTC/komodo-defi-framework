#![allow(static_mut_refs)]

pub mod docker_env_metadata;

// Helpers are used by all docker tests, and also by some sepolia tests
#[cfg(any(
    feature = "run-docker-tests",
    feature = "sepolia-maker-swap-v2-tests",
    feature = "sepolia-taker-swap-v2-tests",
))]
pub mod helpers;

// ============================================================================
// ORDERMATCHING TESTS
// Tests for the orderbook and order matching engine (lp_ordermatch)
// Future destination: mm2_main::lp_ordermatch/tests
// ============================================================================

// Ordermatching tests - UTXO + ETH cross-chain orderbook
// Tests: best_orders, orderbook depth, price aggregation
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1, ETH, ERC20
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-ordermatch"))]
mod docker_ordermatch_tests;

// ============================================================================
// SWAP TESTS
// Tests for atomic swap execution (lp_swap)
// Future destination: mm2_main::lp_swap/tests or coins::*/tests
// ============================================================================

// Core swap tests - UTXO + ETH cross-chain atomic swaps
// Tests: maker/taker swap flows, swap negotiation, payment validation
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1, ETH, ERC20
// Note: This module is large and mixes swap, orderbook, and coin tests - split recommended
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-eth"))]
mod docker_tests_inner;

// Swap protocol v2 tests - UTXO-only TPU protocol
// Tests: MakerSwapStateMachine, TakerSwapStateMachine, trading protocol upgrade
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-swaps-utxo"))]
mod swap_proto_v2_tests;

// Swap confirmation settings sync tests - UTXO-only
// Tests: confirmation requirements, settings synchronization between maker/taker
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-swaps-utxo"))]
mod swaps_confs_settings_sync_tests;

// Swap file lock tests - UTXO-only infrastructure
// Tests: concurrent swap file locking, race condition prevention
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-swaps-utxo"))]
mod swaps_file_lock_tests;

// BCH-SLP swap tests - main docker job only
// Tests: BCH/SLP atomic swaps (FORSLP, ADEXSLP pairs)
// Chains: BCH-SLP
// Note: Excluded from chain-specific jobs - requires full multi-chain environment
#[cfg(all(
    feature = "run-docker-tests",
    not(feature = "docker-tests-slp"),
    not(feature = "docker-tests-sia"),
    not(feature = "docker-tests-eth"),
    not(feature = "docker-tests-qrc20"),
    not(feature = "docker-tests-tendermint"),
    not(feature = "docker-tests-zcoin"),
    not(feature = "docker-tests-swaps-utxo"),
    not(feature = "docker-tests-watchers"),
    not(feature = "docker-tests-ordermatch"),
))]
mod swap_tests;

// ============================================================================
// WATCHER TESTS
// Tests for swap watcher nodes (lp_swap::watchers)
// Future destination: mm2_main::lp_swap::watchers/tests
// ============================================================================

// Swap watcher tests - UTXO + ETH
// Tests: watcher node functionality, maker payment spend, taker payment refund
// Tests: watcher rewards, restart resilience
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1, ETH, ERC20
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-watchers"))]
mod swap_watcher_tests;

// ============================================================================
// COIN-SPECIFIC TESTS
// Tests for individual coin implementations (coins crate)
// Future destination: coins::*/tests
// ============================================================================

// ETH/ERC20 coin tests
// Tests: gas estimation, nonce management, ERC20 activation, NFT swaps
// Chains: ETH, ERC20, ERC721, ERC1155
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-eth"))]
mod eth_docker_tests;

// QRC20 coin and swap tests
// Tests: QRC20 activation, QTUM gas, QRC20<->UTXO swaps
// Chains: QRC20, UTXO-MYCOIN
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-qrc20"))]
pub mod qrc20_tests;

// SIA coin tests
// Tests: Sia activation, balance, withdraw
// Chains: Sia
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-sia"))]
mod sia_docker_tests;

// SLP/BCH coin tests
// Tests: SLP token activation, BCH-SLP balance
// Chains: BCH-SLP (FORSLP, ADEXSLP)
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-slp"))]
mod slp_tests;

// Tendermint coin and IBC tests
// Tests: ATOM/Nucleus/IRIS activation, staking, IBC transfers, Tendermint<->ETH swaps
// Chains: Tendermint (ATOM, Nucleus, IRIS), ETH
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-tendermint"))]
mod tendermint_tests;

// ZCoin/Zombie coin tests
// Tests: ZCoin activation, shielded transactions, DEX fee collection
// Chains: ZCoin/Zombie
#[cfg(all(feature = "run-docker-tests", feature = "docker-tests-zcoin"))]
mod z_coin_docker_tests;

// dummy test helping IDE to recognize this as test module
#[test]
#[allow(clippy::assertions_on_constants)]
fn dummy() {
    assert!(true)
}
