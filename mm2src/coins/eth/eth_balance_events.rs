use super::EthCoin;
use crate::{
    eth::{u256_to_big_decimal, Erc20TokenDetails},
    hd_wallet::AddrToString,
    BalanceError, CoinWithDerivationMethod,
};
use common::{executor::Timer, log, Future01CompatExt};
use mm2_err_handle::prelude::*;
use mm2_event_stream::{Broadcaster, Event, EventStreamer, NoDataIn, StreamHandlerInput, StreamerId};
use mm2_number::BigDecimal;

use async_trait::async_trait;
use compatible_time::Instant;
use ethereum_types::Address;
use futures::{channel::oneshot, stream::FuturesUnordered, StreamExt};
use serde::Deserialize;
use serde_json::Value as Json;
use std::collections::{HashMap, HashSet};

#[derive(Deserialize)]
#[serde(deny_unknown_fields, default)]
struct EthBalanceStreamingConfig {
    /// The time in seconds to wait before re-polling the balance and streaming.
    pub stream_interval_seconds: f64,
}

impl Default for EthBalanceStreamingConfig {
    fn default() -> Self {
        Self {
            stream_interval_seconds: 10.0,
        }
    }
}

pub struct EthBalanceEventStreamer {
    /// The period in seconds between each balance check.
    interval: f64,
    coin: EthCoin,
}

impl EthBalanceEventStreamer {
    pub fn try_new(config: Option<Json>, coin: EthCoin) -> serde_json::Result<Self> {
        let config: EthBalanceStreamingConfig = config.map(serde_json::from_value).unwrap_or(Ok(Default::default()))?;

        Ok(Self {
            interval: config.stream_interval_seconds,
            coin,
        })
    }
}

struct BalanceData {
    ticker: String,
    address: String,
    balance: BigDecimal,
}

#[derive(Serialize)]
struct BalanceFetchError {
    ticker: String,
    address: String,
    error: MmError<BalanceError>,
}

type BalanceResult = Result<BalanceData, BalanceFetchError>;

/// This implementation differs from others, as they immediately return
/// an error if any of the requests fails. This one completes all futures
/// and returns their results individually.
async fn get_all_balance_results_concurrently(coin: &EthCoin, addresses: HashSet<Address>) -> Vec<BalanceResult> {
    let mut tokens = coin.get_erc_tokens_infos();
    // Workaround for performance purposes.
    //
    // Unlike tokens, the platform coin length is constant (=1). Instead of creating a generic
    // type and mapping the platform coin and the entire token list (which can grow at any time), we map
    // the platform coin to Erc20TokenDetails so that we can use the token list right away without
    // additional mapping.
    tokens.insert(
        coin.ticker.clone(),
        Erc20TokenDetails {
            // This is a dummy value, since there is no token address for the platform coin.
            // In the fetch_balance function, we check if the token_ticker is equal to this
            // coin's ticker to avoid using token_address to fetch the balance
            // and to use address_balance instead.
            token_address: Address::default(),
            decimals: coin.decimals,
        },
    );
    drop_mutability!(tokens);

    let mut all_jobs = FuturesUnordered::new();

    for address in addresses {
        let jobs = tokens.iter().map(|(token_ticker, info)| {
            let coin = coin.clone();
            let token_ticker = token_ticker.clone();
            let info = info.clone();
            async move { fetch_balance(&coin, address, token_ticker, &info).await }
        });

        all_jobs.extend(jobs);
    }

    all_jobs.collect().await
}

async fn fetch_balance(
    coin: &EthCoin,
    address: Address,
    token_ticker: String,
    info: &Erc20TokenDetails,
) -> Result<BalanceData, BalanceFetchError> {
    let (balance_as_u256, decimals) = if token_ticker == coin.ticker {
        (
            coin.address_balance(address)
                .compat()
                .await
                .map_err(|error| BalanceFetchError {
                    ticker: token_ticker.clone(),
                    address: address.addr_to_string(),
                    error,
                })?,
            coin.decimals,
        )
    } else {
        (
            coin.get_token_balance(info.token_address)
                .await
                .map_err(|error| BalanceFetchError {
                    ticker: token_ticker.clone(),
                    address: address.addr_to_string(),
                    error,
                })?,
            info.decimals,
        )
    };

    let balance_as_big_decimal = u256_to_big_decimal(balance_as_u256, decimals).map_err(|e| BalanceFetchError {
        ticker: token_ticker.clone(),
        address: address.addr_to_string(),
        error: e.map(BalanceError::from),
    })?;

    Ok(BalanceData {
        ticker: token_ticker,
        address: address.addr_to_string(),
        balance: balance_as_big_decimal,
    })
}

#[async_trait]
impl EventStreamer for EthBalanceEventStreamer {
    type DataInType = NoDataIn;

    fn streamer_id(&self) -> StreamerId {
        StreamerId::Balance {
            coin: self.coin.ticker.to_string(),
        }
    }

    async fn handle(
        self,
        broadcaster: Broadcaster,
        ready_tx: oneshot::Sender<Result<(), String>>,
        _: impl StreamHandlerInput<NoDataIn>,
    ) {
        async fn start_polling(streamer_id: StreamerId, broadcaster: Broadcaster, coin: EthCoin, interval: f64) {
            async fn sleep_remaining_time(interval: f64, now: Instant) {
                // If the interval is x seconds,
                // our goal is to broadcast changed balances every x seconds.
                // To achieve this, we need to subtract the time complexity of each iteration.
                // Given that an iteration already takes 80% of the interval,
                // this will lead to inconsistency in the events.
                let remaining_time = interval - now.elapsed().as_secs_f64();
                // Not worth to make a call for less than `0.1` durations
                if remaining_time >= 0.1 {
                    Timer::sleep(remaining_time).await;
                }
            }

            let mut cache: HashMap<String, HashMap<String, BigDecimal>> = HashMap::new();

            loop {
                let now = Instant::now();

                let addresses = match coin.all_addresses().await {
                    Ok(addresses) => addresses,
                    Err(e) => {
                        log::error!("Failed getting addresses for {}. Error: {}", coin.ticker, e);
                        let e = serde_json::to_value(e).expect("Serialization shouldn't fail.");
                        broadcaster.broadcast(Event::err(streamer_id.clone(), e));
                        sleep_remaining_time(interval, now).await;
                        continue;
                    },
                };

                let mut balance_updates = vec![];
                for result in get_all_balance_results_concurrently(&coin, addresses).await {
                    match result {
                        Ok(res) => {
                            if Some(&res.balance) == cache.get(&res.ticker).and_then(|map| map.get(&res.address)) {
                                continue;
                            }

                            balance_updates.push(json!({
                                "ticker": res.ticker,
                                "address": res.address,
                                "balance": { "spendable": res.balance, "unspendable": BigDecimal::default() }
                            }));
                            cache
                                .entry(res.ticker.clone())
                                .or_default()
                                .insert(res.address, res.balance);
                        },
                        Err(err) => {
                            log::error!(
                                "Failed getting balance for '{}:{}' with {interval} interval. Error: {}",
                                err.ticker,
                                err.address,
                                err.error
                            );
                            let e = serde_json::to_value(err).expect("Serialization shouldn't fail.");
                            broadcaster.broadcast(Event::err(streamer_id.clone(), e));
                        },
                    };
                }

                if !balance_updates.is_empty() {
                    broadcaster.broadcast(Event::new(streamer_id.clone(), json!(balance_updates)));
                }

                sleep_remaining_time(interval, now).await;
            }
        }

        ready_tx
            .send(Ok(()))
            .expect("Receiver is dropped, which should never happen.");

        start_polling(self.streamer_id(), broadcaster, self.coin, self.interval).await
    }
}
