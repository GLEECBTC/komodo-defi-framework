use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

const NETWORK: &str = "NETWORK";
const HEARTBEAT: &str = "HEARTBEAT";
const SWAP_STATUS: &str = "SWAP_STATUS";
const ORDER_STATUS: &str = "ORDER_STATUS";

const TASK_PREFIX: &str = "TASK:";
const BALANCE_PREFIX: &str = "BALANCE:";
const TX_HISTORY_PREFIX: &str = "TX_HISTORY:";
const FEE_ESTIMATION_PREFIX: &str = "FEE_ESTIMATION:";
const DATA_NEEDED_PREFIX: &str = "DATA_NEEDED:";
const ORDERBOOK_UPDATE_PREFIX: &str = "ORDERBOOK_UPDATE:";
#[cfg(any(test, target_arch = "wasm32"))]
const FOR_TESTING_PREFIX: &str = "TEST_STREAMER:";

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum StreamerId {
    Network,
    Heartbeat,
    SwapStatus,
    OrderStatus,
    Task {
        task_id: u64, // TODO: should be TaskId (from rpc_task)
    },
    Balance {
        coin: String,
    },
    DataNeeded {
        data_type: String,
    },
    TxHistory {
        coin: String,
    },
    FeeEstimation {
        coin: String,
    },
    OrderbookUpdate {
        topic: String,
    },
    #[cfg(any(test, target_arch = "wasm32"))]
    ForTesting {
        test_streamer: String,
    },
}

impl fmt::Display for StreamerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamerId::Network => write!(f, "{NETWORK}"),
            StreamerId::Heartbeat => write!(f, "{HEARTBEAT}"),
            StreamerId::SwapStatus => write!(f, "{SWAP_STATUS}"),
            StreamerId::OrderStatus => write!(f, "{ORDER_STATUS}"),
            StreamerId::Task { task_id } => write!(f, "{TASK_PREFIX}{task_id}"),
            StreamerId::Balance { coin } => write!(f, "{BALANCE_PREFIX}{coin}"),
            StreamerId::TxHistory { coin } => write!(f, "{TX_HISTORY_PREFIX}{coin}"),
            StreamerId::FeeEstimation { coin } => write!(f, "{FEE_ESTIMATION_PREFIX}{coin}"),
            StreamerId::DataNeeded { data_type } => write!(f, "{DATA_NEEDED_PREFIX}{data_type}"),
            StreamerId::OrderbookUpdate { topic } => write!(f, "{ORDERBOOK_UPDATE_PREFIX}{topic}"),
            #[cfg(any(test, target_arch = "wasm32"))]
            StreamerId::ForTesting { test_streamer } => write!(f, "{FOR_TESTING_PREFIX}{test_streamer}"),
        }
    }
}

impl Serialize for StreamerId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for StreamerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StreamerIdVisitor;

        impl Visitor<'_> for StreamerIdVisitor {
            type Value = StreamerId;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string representing a StreamerId")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    NETWORK => Ok(StreamerId::Network),
                    HEARTBEAT => Ok(StreamerId::Heartbeat),
                    SWAP_STATUS => Ok(StreamerId::SwapStatus),
                    ORDER_STATUS => Ok(StreamerId::OrderStatus),
                    v if v.starts_with(TASK_PREFIX) => Ok(StreamerId::Task {
                        task_id: v[TASK_PREFIX.len()..].parse().map_err(de::Error::custom)?,
                    }),
                    v if v.starts_with(BALANCE_PREFIX) => Ok(StreamerId::Balance {
                        coin: v[BALANCE_PREFIX.len()..].to_string(),
                    }),
                    v if v.starts_with(TX_HISTORY_PREFIX) => Ok(StreamerId::TxHistory {
                        coin: v[TX_HISTORY_PREFIX.len()..].to_string(),
                    }),
                    v if v.starts_with(FEE_ESTIMATION_PREFIX) => Ok(StreamerId::FeeEstimation {
                        coin: v[FEE_ESTIMATION_PREFIX.len()..].to_string(),
                    }),
                    v if v.starts_with(DATA_NEEDED_PREFIX) => Ok(StreamerId::DataNeeded {
                        data_type: v[DATA_NEEDED_PREFIX.len()..].to_string(),
                    }),
                    v if v.starts_with(ORDERBOOK_UPDATE_PREFIX) => Ok(StreamerId::OrderbookUpdate {
                        topic: v[ORDERBOOK_UPDATE_PREFIX.len()..].to_string(),
                    }),
                    #[cfg(any(test, target_arch = "wasm32"))]
                    v if v.starts_with(FOR_TESTING_PREFIX) => Ok(StreamerId::ForTesting {
                        test_streamer: v[FOR_TESTING_PREFIX.len()..].to_string(),
                    }),
                    _ => Err(de::Error::custom(format!("Invalid StreamerId: {value}"))),
                }
            }
        }

        deserializer.deserialize_str(StreamerIdVisitor)
    }
}
