#![allow(static_mut_refs)]

pub mod runner;

// Helpers are used by all docker tests
#[cfg(feature = "run-docker-tests")]
pub mod helpers;

// ============================================================================
// ORDERMATCHING TESTS
// Tests for the orderbook and order matching engine (lp_ordermatch)
// Future destination: mm2_main::lp_ordermatch/tests
// ============================================================================

// Ordermatching tests - UTXO-only orderbook
// Tests: best_orders, orderbook depth, price aggregation, custom orderbook tickers
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1
#[cfg(feature = "docker-tests-ordermatch")]
mod docker_ordermatch_tests;

// UTXO Ordermatching V1 tests - UTXO-only orderbook mechanics (extracted from docker_tests_inner)
// Tests: order lifecycle, balance-driven cancellations/updates, restart kickstart, best-price matching,
//        RPC response formats, min_volume/dust validation, P2P time sync validation
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1
#[cfg(feature = "docker-tests-ordermatch")]
mod utxo_ordermatch_v1_tests;

// ============================================================================
// SWAP TESTS
// Tests for atomic swap execution (lp_swap)
// Future destination: mm2_main::lp_swap/tests or coins::*/tests
// ============================================================================

// Cross-chain tests - UTXO + ETH cross-chain order matching and validation
// Tests: cross-chain order matching, volume validation, orderbook depth, best_orders
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1, ETH, ERC20
// Note: Contains tests that require BOTH ETH and UTXO chains simultaneously
#[cfg(feature = "docker-tests-integration")]
mod docker_tests_inner;

// ETH Inner tests - ETH-only tests (extracted from docker_tests_inner)
// Tests: ETH/ERC20 activation, disable, withdraw, swap contract negotiation, order management, ERC20 approval
// Chains: ETH, ERC20
// Future: Consider separate feature flag (docker-tests-eth-only) for tests that don't need UTXO
#[cfg(feature = "docker-tests-eth")]
mod eth_inner_tests;

// Swap protocol v2 tests - UTXO-only TPU protocol
// Tests: MakerSwapStateMachine, TakerSwapStateMachine, trading protocol upgrade
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1
#[cfg(feature = "docker-tests-swaps")]
mod swap_proto_v2_tests;

// UTXO Swaps V1 tests - UTXO-only swap mechanics (extracted from docker_tests_inner)
// Tests: swap spend/refund, trade preimage, max taker/maker vol, locked amounts, UTXO merge
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1
#[cfg(feature = "docker-tests-swaps")]
mod utxo_swaps_v1_tests;

// Swap confirmation settings sync tests - UTXO-only
// Tests: confirmation requirements, settings synchronization between maker/taker
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1
#[cfg(feature = "docker-tests-swaps")]
mod swaps_confs_settings_sync_tests;

// Swap file lock tests - UTXO-only infrastructure
// Tests: concurrent swap file locking, race condition prevention
// Chains: UTXO-MYCOIN, UTXO-MYCOIN1
#[cfg(feature = "docker-tests-swaps")]
mod swaps_file_lock_tests;

// ============================================================================
// CROSS-CHAIN INTEGRATION TESTS
// Tests for atomic swaps between different chain families (requires all containers)
// Future destination: Integration test suite
// ============================================================================

// BCH-SLP swap tests
// Tests: BCH/SLP atomic swaps (FORSLP, ADEXSLP pairs)
// Chains: BCH-SLP (FORSLP node only)
#[cfg(feature = "docker-tests-slp")]
mod swap_tests;

// ============================================================================
// WATCHER TESTS
// Tests for swap watcher nodes (lp_swap::watchers)
// Future destination: mm2_main::lp_swap::watchers/tests
// ============================================================================

// Swap watcher tests.
// UTXO watcher tests are enabled with `docker-tests-watchers`.
// ETH/ERC20 watcher tests are behind `docker-tests-watchers-eth` (disabled by default).
#[cfg(feature = "docker-tests-watchers")]
mod swap_watcher_tests;

// ============================================================================
// COIN-SPECIFIC TESTS
// Tests for individual coin implementations (coins crate)
// Future destination: coins::*/tests
// ============================================================================

// ETH/ERC20 coin tests
// Tests: gas estimation, nonce management, ERC20 activation, NFT swaps
// Chains: ETH, ERC20, ERC721, ERC1155
#[cfg(feature = "docker-tests-eth")]
mod eth_docker_tests;

// QRC20 coin and swap tests
// Tests: QRC20 activation, QTUM gas, QRC20<->UTXO swaps
// Chains: QRC20, UTXO-MYCOIN
#[cfg(feature = "docker-tests-qrc20")]
pub mod qrc20_tests;

// SIA coin tests
// Tests: Sia activation, balance, withdraw
// Chains: Sia
#[cfg(feature = "docker-tests-sia")]
mod sia_docker_tests;

// SLP/BCH coin tests
// Tests: SLP token activation, BCH-SLP balance
// Chains: BCH-SLP (FORSLP, ADEXSLP)
#[cfg(feature = "docker-tests-slp")]
mod slp_tests;

// Tendermint coin and IBC tests (Cosmos-only)
// Tests: ATOM/Nucleus/IRIS activation, staking, IBC transfers, withdraw, delegation
// Chains: Tendermint (ATOM, Nucleus, IRIS)
#[cfg(feature = "docker-tests-tendermint")]
mod tendermint_tests;

// Tendermint cross-chain swap tests
// Tests: NUCLEUS<->DOC, NUCLEUS<->ETH, DOC<->IRIS-IBC-NUCLEUS swaps
// Chains: Tendermint (NUCLEUS, IRIS) + ETH/Electrum
// Note: Requires multiple chain families (Tendermint + ETH) - part of integration test suite
#[cfg(feature = "docker-tests-integration")]
mod tendermint_swap_tests;

// ZCoin/Zombie coin tests
// Tests: ZCoin activation, shielded transactions, DEX fee collection
// Chains: ZCoin/Zombie
#[cfg(feature = "docker-tests-zcoin")]
mod z_coin_docker_tests;

// dummy test helping IDE to recognize this as test module
#[test]
#[allow(clippy::assertions_on_constants)]
fn dummy() {
    assert!(true)
}
