use common::executor::Timer;
use mm2_core::mm_ctx::MmArc;
use mm2_event_stream::{Broadcaster, Event, EventStreamer, NoDataIn, StreamHandlerInput, StreamerId};

use async_trait::async_trait;
use futures::channel::oneshot;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NetworkEventConfig {
    /// The time in seconds to wait after sending network info before sending another one.
    pub stream_interval_seconds: f64,
    /// Always (force) send network info data, even if it's the same as the previous one sent.
    pub always_send: bool,
}

impl Default for NetworkEventConfig {
    fn default() -> Self {
        Self {
            stream_interval_seconds: 5.0,
            always_send: false,
        }
    }
}

pub struct NetworkEvent {
    config: NetworkEventConfig,
    ctx: MmArc,
}

impl NetworkEvent {
    pub fn new(config: NetworkEventConfig, ctx: MmArc) -> Self {
        Self { config, ctx }
    }
}

#[async_trait]
impl EventStreamer for NetworkEvent {
    type DataInType = NoDataIn;

    fn streamer_id(&self) -> StreamerId {
        StreamerId::Network
    }

    async fn handle(
        self,
        broadcaster: Broadcaster,
        ready_tx: oneshot::Sender<Result<(), String>>,
        _: impl StreamHandlerInput<NoDataIn>,
    ) {
        let p2p_ctx = crate::p2p_ctx::P2PContext::fetch_from_mm_arc(&self.ctx);
        let mut previously_sent = json!({});

        ready_tx.send(Ok(())).unwrap();

        loop {
            let p2p_cmd_tx = p2p_ctx.cmd_tx.lock().clone();

            let directly_connected_peers = crate::get_directly_connected_peers(p2p_cmd_tx.clone()).await;
            let gossip_mesh = crate::get_gossip_mesh(p2p_cmd_tx.clone()).await;
            let gossip_peer_topics = crate::get_gossip_peer_topics(p2p_cmd_tx.clone()).await;
            let gossip_topic_peers = crate::get_gossip_topic_peers(p2p_cmd_tx.clone()).await;
            let relay_mesh = crate::get_relay_mesh(p2p_cmd_tx).await;

            let event_data = json!({
                "directly_connected_peers": directly_connected_peers,
                "gossip_mesh": gossip_mesh,
                "gossip_peer_topics": gossip_peer_topics,
                "gossip_topic_peers": gossip_topic_peers,
                "relay_mesh": relay_mesh,
            });

            if previously_sent != event_data || self.config.always_send {
                broadcaster.broadcast(Event::new(self.streamer_id(), event_data.clone()));

                previously_sent = event_data;
            }

            Timer::sleep(self.config.stream_interval_seconds).await;
        }
    }
}
