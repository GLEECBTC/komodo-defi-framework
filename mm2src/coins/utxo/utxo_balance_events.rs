use super::{utxo_standard::UtxoStandardCoin, UtxoArc};

use crate::utxo::rpc_clients::UtxoRpcClientEnum;
use crate::{
    utxo::{
        output_script,
        rpc_clients::electrum_script_hash,
        utxo_common::{address_balance, address_to_scripthash},
        ScripthashNotification, UtxoCoinFields,
    },
    CoinWithDerivationMethod, MarketCoinOps,
};

use async_trait::async_trait;
use common::log;
use futures::channel::oneshot;
use futures::StreamExt;
use keys::Address;
use mm2_event_stream::{Broadcaster, DeriveStreamerId, Event, EventStreamer, StreamHandlerInput, StreamerId};
use std::collections::{HashMap, HashSet};

macro_rules! try_or_continue {
    ($exp:expr) => {
        match $exp {
            Ok(t) => t,
            Err(e) => {
                log::error!("{}", e);
                continue;
            },
        }
    };
}

pub struct UtxoBalanceEventStreamer {
    coin: UtxoStandardCoin,
}

impl<'a> DeriveStreamerId<'a> for UtxoBalanceEventStreamer {
    type InitParam = UtxoArc;
    type DeriveParam = &'a str;

    fn new(utxo_arc: Self::InitParam) -> Self {
        Self {
            // We wrap the UtxoArc in a UtxoStandardCoin for easier method accessibility.
            // The UtxoArc might belong to a different coin type though.
            coin: UtxoStandardCoin::from(utxo_arc),
        }
    }

    fn derive_streamer_id(coin: Self::DeriveParam) -> StreamerId {
        StreamerId::Balance { coin: coin.to_string() }
    }
}

#[async_trait]
impl EventStreamer for UtxoBalanceEventStreamer {
    type DataInType = ScripthashNotification;

    fn streamer_id(&self) -> StreamerId {
        StreamerId::Balance {
            coin: self.coin.ticker().to_string(),
        }
    }

    async fn handle(
        self,
        broadcaster: Broadcaster,
        ready_tx: oneshot::Sender<Result<(), String>>,
        mut data_rx: impl StreamHandlerInput<Self::DataInType>,
    ) {
        const RECEIVER_DROPPED_MSG: &str = "Receiver is dropped, which should never happen.";
        let streamer_id = self.streamer_id();
        let coin = self.coin;
        let mut scripthash_to_address_map = HashMap::new();

        // Make sure the RPC client is not native. That doesn't support balance streaming.
        if coin.as_ref().rpc_client.is_native() {
            let msg = "Balance streaming is not supported for native RPC client.";
            ready_tx.send(Err(msg.to_string())).expect(RECEIVER_DROPPED_MSG);
            panic!("{}", msg);
        };
        // Get all the addresses to subscribe to their balance updates.
        let all_addresses = match coin.all_addresses().await {
            Ok(addresses) => addresses,
            Err(e) => {
                let msg = format!("Failed to get all addresses: {e}");
                ready_tx.send(Err(msg.clone())).expect(RECEIVER_DROPPED_MSG);
                panic!("{}", msg);
            },
        };
        ready_tx.send(Ok(())).expect(RECEIVER_DROPPED_MSG);

        // Initially, subscribe to all the addresses we currently have.
        let tracking_list = subscribe_to_addresses(coin.as_ref(), all_addresses).await;
        scripthash_to_address_map.extend(tracking_list);

        while let Some(message) = data_rx.next().await {
            let notified_scripthash = match message {
                ScripthashNotification::Triggered(t) => t,
                ScripthashNotification::SubscribeToAddresses(addresses) => {
                    let tracking_list = subscribe_to_addresses(coin.as_ref(), addresses).await;
                    scripthash_to_address_map.extend(tracking_list);
                    continue;
                },
            };

            let address = match scripthash_to_address_map.get(&notified_scripthash) {
                Some(t) => Some(t.clone()),
                None => try_or_continue!(coin.all_addresses().await)
                    .into_iter()
                    .find_map(|addr| {
                        let script = match output_script(&addr) {
                            Ok(script) => script,
                            Err(e) => {
                                log::error!("{e}");
                                return None;
                            },
                        };
                        let script_hash = electrum_script_hash(&script);
                        let scripthash = hex::encode(script_hash);

                        if notified_scripthash == scripthash {
                            scripthash_to_address_map.insert(notified_scripthash.clone(), addr.clone());
                            Some(addr)
                        } else {
                            None
                        }
                    }),
            };

            let address = match address {
                Some(t) => t,
                None => {
                    log::debug!(
                        "Couldn't find the relevant address for {} scripthash.",
                        notified_scripthash
                    );
                    continue;
                },
            };

            let balance = match address_balance(&coin, &address).await {
                Ok(t) => t,
                Err(e) => {
                    let ticker = coin.ticker();
                    log::error!("Failed getting balance for '{ticker}'. Error: {e}");
                    let e = serde_json::to_value(e).expect("Serialization should't fail.");

                    broadcaster.broadcast(Event::err(streamer_id.clone(), e));

                    continue;
                },
            };

            let payload = json!({
                "ticker": coin.ticker(),
                "address": address.to_string(),
                "balance": { "spendable": balance.spendable, "unspendable": balance.unspendable }
            });

            broadcaster.broadcast(Event::new(streamer_id.clone(), json!(vec![payload])));
        }
    }
}

async fn subscribe_to_addresses(utxo: &UtxoCoinFields, addresses: HashSet<Address>) -> HashMap<String, Address> {
    match utxo.rpc_client.clone() {
        UtxoRpcClientEnum::Electrum(client) => {
            // Collect the scripthash for every address into a map.
            let scripthash_to_address_map = addresses
                .into_iter()
                .filter_map(|address| {
                    let scripthash = address_to_scripthash(&address)
                        .map_err(|e| log::error!("Failed to get scripthash for address {address}: {e}"))
                        .ok()?;
                    Some((scripthash, address))
                })
                .collect();
            // Add these subscriptions to the connection manager. It will choose whatever connections
            // it sees fit to subscribe each of these addresses to.
            client
                .connection_manager
                .add_subscriptions(&scripthash_to_address_map)
                .await;
            scripthash_to_address_map
        },
        UtxoRpcClientEnum::Native(_) => {
            // Unreachable: The caller should have checked that the RPC client isn't native.
            HashMap::new()
        },
    }
}
