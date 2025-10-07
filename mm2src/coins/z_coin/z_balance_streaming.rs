use crate::common::Future01CompatExt;
use crate::z_coin::ZCoin;
use crate::MarketCoinOps;

use async_trait::async_trait;
use common::log::error;
use futures::channel::oneshot;
use futures_util::StreamExt;
use mm2_event_stream::{Broadcaster, DeriveStreamerId, Event, EventStreamer, StreamHandlerInput, StreamerId};

pub struct ZCoinBalanceEventStreamer {
    coin: ZCoin,
}

impl<'a> DeriveStreamerId<'a> for ZCoinBalanceEventStreamer {
    type InitParam = ZCoin;
    type DeriveParam = &'a str;

    fn new(coin: Self::InitParam) -> Self {
        Self { coin }
    }

    #[inline(always)]
    fn derive_streamer_id(coin: Self::DeriveParam) -> StreamerId {
        StreamerId::Balance { coin: coin.to_string() }
    }
}

#[async_trait]
impl EventStreamer for ZCoinBalanceEventStreamer {
    type DataInType = ();

    fn streamer_id(&self) -> StreamerId {
        StreamerId::Balance {
            coin: self.coin.ticker().to_string(),
        }
    }

    async fn handle(
        self,
        broadcaster: Broadcaster,
        ready_tx: oneshot::Sender<Result<(), String>>,
        mut data_rx: impl StreamHandlerInput<()>,
    ) {
        let streamer_id = self.streamer_id();
        let coin = self.coin;

        ready_tx
            .send(Ok(()))
            .expect("Receiver is dropped, which should never happen.");

        // Iterates through received events, and updates balance changes accordingly.
        while (data_rx.next().await).is_some() {
            match coin.my_balance().compat().await {
                Ok(balance) => {
                    let payload = json!({
                        "ticker": coin.ticker(),
                        "address": coin.my_z_address_encoded(),
                        "balance": { "spendable": balance.spendable, "unspendable": balance.unspendable }
                    });

                    broadcaster.broadcast(Event::new(streamer_id.clone(), payload));
                },
                Err(err) => {
                    let ticker = coin.ticker();
                    error!("Failed getting balance for '{ticker}'. Error: {err}");
                    let e = serde_json::to_value(err).expect("Serialization should't fail.");
                    return broadcaster.broadcast(Event::err(streamer_id.clone(), e));
                },
            };
        }
    }
}
