use super::ser::FeePerGasEstimated;
use crate::eth::EthCoin;
use common::executor::Timer;
use mm2_event_stream::{Broadcaster, Event, EventStreamer, NoDataIn, StreamHandlerInput, StreamerId};

use async_trait::async_trait;
use compatible_time::Instant;
use futures::channel::oneshot;
use serde::Deserialize;
use std::convert::TryFrom;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
/// Types of estimators available.
/// Simple - simple internal gas price estimator based on historical data.
/// Provider - gas price estimator using external provider (using gas api).
pub enum EstimatorType {
    Simple,
    Provider,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EthFeeStreamingConfig {
    /// The time in seconds to wait before re-estimating the gas fees.
    pub estimate_every: f64,
    /// The type of the estimator to use.
    pub estimator_type: EstimatorType,
}

impl Default for EthFeeStreamingConfig {
    fn default() -> Self {
        Self {
            // TODO: https://github.com/KomodoPlatform/komodo-defi-framework/pull/2172#discussion_r1785054117
            estimate_every: 15.0,
            estimator_type: EstimatorType::Simple,
        }
    }
}

pub struct EthFeeEventStreamer {
    config: EthFeeStreamingConfig,
    coin: EthCoin,
}

impl EthFeeEventStreamer {
    #[inline(always)]
    pub fn new(config: EthFeeStreamingConfig, coin: EthCoin) -> Self {
        Self { config, coin }
    }
}

#[async_trait]
impl EventStreamer for EthFeeEventStreamer {
    type DataInType = NoDataIn;

    fn streamer_id(&self) -> StreamerId {
        StreamerId::FeeEstimation {
            coin: self.coin.ticker.to_string(),
        }
    }

    async fn handle(
        self,
        broadcaster: Broadcaster,
        ready_tx: oneshot::Sender<Result<(), String>>,
        _: impl StreamHandlerInput<NoDataIn>,
    ) {
        ready_tx
            .send(Ok(()))
            .expect("Receiver is dropped, which should never happen.");

        let use_simple = matches!(self.config.estimator_type, EstimatorType::Simple);
        loop {
            let now = Instant::now();
            match self
                .coin
                .get_eip1559_gas_fee(use_simple)
                .await
                .map(FeePerGasEstimated::try_from)
            {
                Ok(Ok(fee)) => {
                    let fee = serde_json::to_value(fee).expect("Serialization shouldn't fail");
                    broadcaster.broadcast(Event::new(self.streamer_id(), fee));
                },
                Ok(Err(err)) => {
                    let err = json!({ "error": err.to_string() });
                    broadcaster.broadcast(Event::err(self.streamer_id(), err));
                },
                Err(err) => {
                    let err = serde_json::to_value(err).expect("Serialization shouldn't fail");
                    broadcaster.broadcast(Event::err(self.streamer_id(), err));
                },
            }
            let sleep_time = self.config.estimate_every - now.elapsed().as_secs_f64();
            if sleep_time >= 0.1 {
                Timer::sleep(sleep_time).await;
            }
        }
    }
}
