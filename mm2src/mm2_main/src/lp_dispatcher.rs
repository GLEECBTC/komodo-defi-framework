use crate::lp_ordermatch::TradingBotEvent;
use crate::lp_swap::MakerSwapStatusChanged;
use async_std::sync::RwLock;
use mm2_core::{
    event_dispatcher::{Dispatcher, EventUniqueId},
    mm_ctx::{from_ctx, MmArc},
};
use std::any::TypeId;
use std::sync::Arc;

#[derive(Clone)]
pub struct StopCtxEvent;

impl StopCtxEvent {
    pub fn event_id() -> TypeId {
        TypeId::of::<StopCtxEvent>()
    }
}

#[derive(Clone)]
pub enum LpEvents {
    MakerSwapStatusChanged(MakerSwapStatusChanged),
    StopCtxEvent(StopCtxEvent),
    TradingBotEvent(TradingBotEvent),
}

impl From<TradingBotEvent> for LpEvents {
    fn from(evt: TradingBotEvent) -> Self {
        LpEvents::TradingBotEvent(evt)
    }
}

impl From<StopCtxEvent> for LpEvents {
    fn from(evt: StopCtxEvent) -> Self {
        LpEvents::StopCtxEvent(evt)
    }
}

impl EventUniqueId for LpEvents {
    fn event_id(&self) -> TypeId {
        match self {
            LpEvents::MakerSwapStatusChanged(_) => MakerSwapStatusChanged::event_id(),
            LpEvents::StopCtxEvent(_) => StopCtxEvent::event_id(),
            LpEvents::TradingBotEvent(event) => event.event_id(),
        }
    }
}

#[derive(Default)]
pub struct DispatcherContext {
    pub dispatcher: RwLock<Dispatcher<LpEvents>>,
}

impl DispatcherContext {
    /// Obtains a reference to this crate context, creating it if necessary.
    pub fn from_ctx(ctx: &MmArc) -> Result<Arc<DispatcherContext>, String> {
        Ok(try_s!(from_ctx(&ctx.dispatcher_ctx, move || {
            Ok(DispatcherContext::default())
        })))
    }
}

pub async fn dispatch_lp_event(ctx: MmArc, event: LpEvents) {
    let dispatcher_ctx = DispatcherContext::from_ctx(&ctx).unwrap();
    dispatcher_ctx.dispatcher.read().await.dispatch_async(ctx, event).await;
}
