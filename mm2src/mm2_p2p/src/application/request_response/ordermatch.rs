use mm2_number::BigRational;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

type AlbOrderedOrderbookPair = String;
type H64 = [u8; 8];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BestOrdersAction {
    Buy,
    Sell,
}

/// Wraps the different types of order matching requests for the P2P request-response protocol.
///
/// TODO: We should use fixed sizes for dynamic fields (such as strings and maps)
/// and prefer stricter types instead of accepting `String` for nearly everything.
/// See https://github.com/KomodoPlatform/komodo-defi-framework/issues/2236 for reference.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OrdermatchRequest {
    /// Get an orderbook for the given pair.
    GetOrderbook { base: String, rel: String },
    /// Sync specific pubkey orderbook state if our known Patricia trie state doesn't match the latest keep alive message.
    ///
    /// New nodes treat `trie_roots` as from_roots when `expected_roots` is present and require an
    /// exact from→to diff per pair (to the corresponding `expected_roots` entry).
    /// Legacy nodes ignore `expected_roots` (it is optional with serde default).
    SyncPubkeyOrderbookState {
        pubkey: String,
        /// Request using this condition (interpreted as from_roots when `expected_roots` is provided)
        trie_roots: HashMap<AlbOrderedOrderbookPair, H64>,
        /// Optional expected roots to land on exactly (mirrors trie_roots keys).
        /// Backward compatible via serde default so legacy nodes ignore it.
        #[serde(default)]
        expected_roots: Option<HashMap<AlbOrderedOrderbookPair, H64>>,
    },
    /// Request best orders for a specific coin and action.
    BestOrders {
        coin: String,
        action: BestOrdersAction,
        volume: BigRational,
    },
    /// Get orderbook depth for the specified pairs
    OrderbookDepth { pairs: Vec<(String, String)> },
    /// Request best orders for a specific coin and action limited by the number of results.
    ///
    /// Q: Shouldn't we support pagination here?
    BestOrdersByNumber {
        coin: String,
        action: BestOrdersAction,
        number: usize,
    },
}
