//
//  lp_bot.rs
//  marketmaker
//

use async_trait::async_trait;
use common::log::info;
use derive_more::Display;
use futures::lock::Mutex as AsyncMutex;
use mm2_core::{
    event_dispatcher::{EventListener, EventUniqueId},
    mm_ctx::{from_ctx, MmArc},
};
use mm2_number::MmNumber;
#[cfg(test)]
use mocktopus::macros::*;
use std::any::TypeId;
use std::ops::Deref;
use std::{collections::HashMap, sync::Arc};

#[path = "simple_market_maker.rs"]
mod simple_market_maker_bot;
use crate::lp_dispatcher::{LpEvents, StopCtxEvent};
use crate::lp_message_service::{MessageServiceContext, MAKER_BOT_ROOM_ID};
use crate::lp_ordermatch::lp_bot::simple_market_maker_bot::{
    tear_down_bot, BOT_DEFAULT_REFRESH_RATE, PRECISION_FOR_NOTIFICATION,
};
use crate::lp_swap::MakerSwapStatusChanged;
pub use simple_market_maker_bot::{
    start_simple_market_maker_bot, stop_simple_market_maker_bot, StartSimpleMakerBotRequest,
};

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "simple_market_maker_tests.rs"]
pub mod simple_market_maker_tests;

#[derive(Clone, Display)]
#[display(fmt = "simple_market_maker_bot will stop within {bot_refresh_rate} seconds")]
pub struct TradingBotStopping {
    bot_refresh_rate: f64,
}

impl TradingBotStopping {
    fn event_id() -> TypeId {
        TypeId::of::<TradingBotStopping>()
    }
}

#[derive(Clone, Display)]
#[display(fmt = "simple_market_maker_bot successfully started with {nb_pairs} pairs")]
pub struct TradingBotStarted {
    nb_pairs: usize,
}

impl TradingBotStarted {
    fn event_id() -> TypeId {
        TypeId::of::<TradingBotStarted>()
    }
}

#[derive(Clone, Display)]
#[display(fmt = "simple_market_maker_bot successfully stopped - cancelled {nb_orders} orders")]
pub struct TradingBotStopped {
    nb_orders: usize,
}

impl TradingBotStopped {
    fn event_id() -> TypeId {
        TypeId::of::<TradingBotStopped>()
    }
}

#[derive(Clone, Display)]
pub enum TradingBotEvent {
    Started(TradingBotStarted),
    Stopping(TradingBotStopping),
    Stopped(TradingBotStopped),
}

impl EventUniqueId for TradingBotEvent {
    fn event_id(&self) -> TypeId {
        match self {
            TradingBotEvent::Started(_) => TradingBotStarted::event_id(),
            TradingBotEvent::Stopping(_) => TradingBotStopping::event_id(),
            TradingBotEvent::Stopped(_) => TradingBotStopped::event_id(),
        }
    }
}

impl From<TradingBotStopping> for TradingBotEvent {
    fn from(trading_bot_stopping: TradingBotStopping) -> Self {
        TradingBotEvent::Stopping(trading_bot_stopping)
    }
}

impl From<TradingBotStopped> for TradingBotEvent {
    fn from(trading_bot_stopped: TradingBotStopped) -> Self {
        TradingBotEvent::Stopped(trading_bot_stopped)
    }
}

impl From<TradingBotStarted> for TradingBotEvent {
    fn from(trading_bot_started: TradingBotStarted) -> Self {
        TradingBotEvent::Started(trading_bot_started)
    }
}

pub struct RunningState {
    trading_bot_cfg: SimpleMakerBotRegistry,
    bot_refresh_rate: f64,
    price_urls: Vec<String>,
}

pub struct StoppingState {
    trading_bot_cfg: SimpleMakerBotRegistry,
}

#[derive(Default)]
pub struct StoppedState {
    trading_bot_cfg: SimpleMakerBotRegistry,
}

enum TradingBotState {
    Running(RunningState),
    Stopping(StoppingState),
    Stopped(StoppedState),
}

impl From<RunningState> for TradingBotState {
    fn from(running_state: RunningState) -> Self {
        Self::Running(running_state)
    }
}

impl From<StoppingState> for TradingBotState {
    fn from(stopping_state: StoppingState) -> Self {
        Self::Stopping(stopping_state)
    }
}

impl From<StoppedState> for TradingBotState {
    fn from(stopped_state: StoppedState) -> Self {
        Self::Stopped(stopped_state)
    }
}

impl Default for TradingBotState {
    fn default() -> Self {
        StoppedState::default().into()
    }
}

pub type SimpleMakerBotRegistry = HashMap<String, SimpleCoinMarketMakerCfg>;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum VolumeSettings {
    #[serde(rename = "percentage")]
    Percentage(MmNumber),
    #[serde(rename = "usd")]
    Usd(MmNumber),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SimpleCoinMarketMakerCfg {
    pub base: String,
    pub rel: String,
    pub min_volume: Option<VolumeSettings>,
    pub max_volume: Option<VolumeSettings>,
    pub max: Option<bool>,
    pub spread: MmNumber,
    pub base_confs: Option<u64>,
    pub base_nota: Option<bool>,
    pub rel_confs: Option<u64>,
    pub rel_nota: Option<bool>,
    pub enable: bool,
    pub price_elapsed_validity: Option<f64>,
    pub check_last_bidirectional_trade_thresh_hold: Option<bool>,
    pub min_base_price: Option<MmNumber>,
    pub min_rel_price: Option<MmNumber>,
    pub min_pair_price: Option<MmNumber>,
}

#[derive(Default)]
pub struct TradingBotContext {
    trading_bot_states: AsyncMutex<TradingBotState>,
}

impl TradingBotContext {
    async fn get_refresh_rate(&self) -> f64 {
        let state = self.trading_bot_states.lock().await;
        if let TradingBotState::Running(running_state) = &*state {
            return running_state.bot_refresh_rate;
        }
        BOT_DEFAULT_REFRESH_RATE
    }
}

#[derive(Clone)]
pub struct ArcTradingBotContext(Arc<TradingBotContext>);

impl Deref for ArcTradingBotContext {
    type Target = TradingBotContext;
    fn deref(&self) -> &TradingBotContext {
        &self.0
    }
}

#[allow(clippy::single_match)]
impl TradingBotContext {
    async fn on_trading_bot_event(&self, ctx: &MmArc, trading_bot_event: &TradingBotEvent) {
        let msg_format = format!("{trading_bot_event}");
        info!("{}", msg_format);
        let message_service_ctx = MessageServiceContext::from_ctx(ctx).unwrap();
        let message_service = message_service_ctx.message_service.lock().await;
        let _ = message_service.send_message(msg_format, MAKER_BOT_ROOM_ID, false).await;
    }

    async fn on_maker_swap_status_changed(&self, ctx: &MmArc, swap_infos: &MakerSwapStatusChanged) {
        let msg = format!(
            "[{}: {} ({}) <-> {} ({})] status changed: {}",
            swap_infos.uuid,
            swap_infos.taker_coin,
            swap_infos.taker_amount.with_prec(PRECISION_FOR_NOTIFICATION),
            swap_infos.maker_coin,
            swap_infos.maker_amount.with_prec(PRECISION_FOR_NOTIFICATION),
            swap_infos.event_status
        );
        info!("event received: {}", msg);
        let state = self.trading_bot_states.lock().await;
        match &*state {
            TradingBotState::Running(_) => {
                let message_service_ctx = MessageServiceContext::from_ctx(ctx).unwrap();
                let message_service = message_service_ctx.message_service.lock().await;
                let _ = message_service
                    .send_message(msg.to_string(), MAKER_BOT_ROOM_ID, false)
                    .await;
            },
            _ => {},
        }
    }

    async fn on_ctx_stop(&self, ctx: &MmArc) {
        info!("on_ctx_stop event received");
        let simple_market_maker_bot_ctx = TradingBotContext::from_ctx(ctx).unwrap();
        let mut state = simple_market_maker_bot_ctx.trading_bot_states.lock().await;
        match &*state {
            TradingBotState::Running(running_state) => {
                *state = StoppedState {
                    trading_bot_cfg: running_state.trading_bot_cfg.clone(),
                }
                .into();
                drop(state);
                tear_down_bot(ctx.clone()).await
            },
            _ => {},
        }
    }
}

#[async_trait]
impl EventListener for ArcTradingBotContext {
    type Event = LpEvents;

    async fn process_event_async(&self, ctx: MmArc, event: Self::Event) {
        match &event {
            LpEvents::MakerSwapStatusChanged(swap_infos) => self.on_maker_swap_status_changed(&ctx, swap_infos).await,
            LpEvents::StopCtxEvent(_) => self.on_ctx_stop(&ctx).await,
            LpEvents::TradingBotEvent(trading_bot_event) => self.on_trading_bot_event(&ctx, trading_bot_event).await,
        }
    }

    fn get_desired_events(&self) -> Vec<TypeId> {
        vec![
            MakerSwapStatusChanged::event_id(),
            StopCtxEvent::event_id(),
            TradingBotStopping::event_id(),
            TradingBotStarted::event_id(),
            TradingBotStopped::event_id(),
        ]
    }

    fn listener_id(&self) -> &'static str {
        "lp_bot_listener"
    }
}

#[cfg_attr(test, mockable)]
impl TradingBotContext {
    /// Obtains a reference to this crate context, creating it if necessary.
    fn from_ctx(ctx: &MmArc) -> Result<ArcTradingBotContext, String> {
        let arc_bot_context = try_s!(from_ctx(&ctx.simple_market_maker_bot_ctx, move || {
            Ok(TradingBotContext::default())
        }));
        Ok(ArcTradingBotContext(arc_bot_context))
    }
}
