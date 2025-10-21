use super::{orderbook_topic_from_base_rel, subscribe_to_orderbook_topic, OrderbookP2PItem};
use coins::{is_wallet_only_ticker, lp_coinfind};
use mm2_core::mm_ctx::MmArc;
use mm2_event_stream::{Broadcaster, DeriveStreamerId, Event, EventStreamer, StreamHandlerInput, StreamerId};

use async_trait::async_trait;
use futures::channel::oneshot;
use futures::StreamExt;
use uuid::Uuid;

pub struct OrderbookStreamer {
    ctx: MmArc,
    base: String,
    rel: String,
}

type BaseAndRel<'a> = (&'a str, &'a str);
impl<'a> DeriveStreamerId<'a> for OrderbookStreamer {
    type InitParam = (MmArc, String, String);
    type DeriveParam = BaseAndRel<'a>;

    fn new((ctx, base, rel): Self::InitParam) -> Self {
        Self { ctx, base, rel }
    }

    fn derive_streamer_id((base, rel): Self::DeriveParam) -> StreamerId {
        StreamerId::OrderbookUpdate {
            topic: orderbook_topic_from_base_rel(base, rel),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "order_type", content = "order_data")]
pub enum OrderbookItemChangeEvent {
    // NOTE(clippy): This is box-ed due to in-balance of the size of enum variants.
    /// New or updated orderbook item.
    NewOrUpdatedItem(Box<OrderbookP2PItem>),
    /// Removed orderbook item (only UUID is relevant in this case).
    RemovedItem(Uuid),
}

#[async_trait]
impl EventStreamer for OrderbookStreamer {
    type DataInType = OrderbookItemChangeEvent;

    fn streamer_id(&self) -> StreamerId {
        Self::derive_streamer_id((&self.base, &self.rel))
    }

    async fn handle(
        self,
        broadcaster: Broadcaster,
        ready_tx: oneshot::Sender<Result<(), String>>,
        mut data_rx: impl StreamHandlerInput<Self::DataInType>,
    ) {
        const RECEIVER_DROPPED_MSG: &str = "Receiver is dropped, which should never happen.";
        if let Err(err) = sanity_checks(&self.ctx, &self.base, &self.rel).await {
            ready_tx.send(Err(err.clone())).expect(RECEIVER_DROPPED_MSG);
            panic!("{}", err);
        }
        // We need to subscribe to the orderbook, otherwise we won't get any updates from the P2P network.
        if let Err(err) = subscribe_to_orderbook_topic(&self.ctx, &self.base, &self.rel, false).await {
            let err = format!("Subscribing to orderbook topic failed: {err:?}");
            ready_tx.send(Err(err.clone())).expect(RECEIVER_DROPPED_MSG);
            panic!("{}", err);
        }
        ready_tx.send(Ok(())).expect(RECEIVER_DROPPED_MSG);

        while let Some(orderbook_update) = data_rx.next().await {
            let event_data = serde_json::to_value(orderbook_update).expect("Serialization shouldn't fail.");
            let event = Event::new(self.streamer_id(), event_data);
            broadcaster.broadcast(event);
        }
    }
}

async fn sanity_checks(ctx: &MmArc, base: &str, rel: &str) -> Result<(), String> {
    // TODO: This won't work with no-login mode.
    lp_coinfind(ctx, base)
        .await
        .map_err(|e| format!("Coin {base} not found: {e}"))?;
    if is_wallet_only_ticker(ctx, base) {
        return Err(format!("Coin {base} is wallet-only."));
    }
    lp_coinfind(ctx, rel)
        .await
        .map_err(|e| format!("Coin {base} not found: {e}"))?;
    if is_wallet_only_ticker(ctx, rel) {
        return Err(format!("Coin {rel} is wallet-only."));
    }
    Ok(())
}

impl Drop for OrderbookStreamer {
    fn drop(&mut self) {
        // TODO: Do we want to unsubscribe from the orderbook topic when streaming is dropped?
        //       Also, we seem to never unsubscribe from an orderbook topic after doing an orderbook RPC!
    }
}
