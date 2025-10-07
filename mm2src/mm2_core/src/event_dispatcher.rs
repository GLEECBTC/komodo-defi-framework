use crate::mm_ctx::MmArc;
use async_trait::async_trait;
use std::any::TypeId;
use std::collections::{HashMap, HashSet};

pub type Listeners<EventType> = Vec<Box<dyn EventListener<Event = EventType>>>;
pub type DispatchTable = HashMap<TypeId, Vec<usize>>;

#[async_trait]
pub trait EventListener: 'static + Send + Sync {
    type Event;
    async fn process_event_async(&self, ctx: MmArc, event: Self::Event);
    fn get_desired_events(&self) -> Vec<TypeId>;
    fn listener_id(&self) -> &'static str;
}

pub trait EventUniqueId {
    fn event_id(&self) -> TypeId;
}

pub struct Dispatcher<EventType> {
    listeners: Listeners<EventType>,
    listeners_id: HashSet<String>,
    dispatch_table: DispatchTable,
}

impl<EventType> Default for Dispatcher<EventType>
where
    EventType: Clone + EventUniqueId,
{
    fn default() -> Self {
        Dispatcher {
            listeners: vec![],
            listeners_id: Default::default(),
            dispatch_table: Default::default(),
        }
    }
}

impl<EventType: 'static> Dispatcher<EventType>
where
    EventType: Clone + EventUniqueId,
{
    pub fn add_listener(&mut self, listen: impl EventListener<Event = EventType>) {
        if self.listeners_id.contains(listen.listener_id()) {
            return;
        }
        self.listeners_id.insert(listen.listener_id().to_string());
        let id = self.listeners.len();
        let events = listen.get_desired_events();
        self.listeners.push(Box::new(listen));
        for ev in events {
            self.dispatch_table.entry(ev).or_default().push(id);
        }
    }

    pub async fn dispatch_async(&self, ctx: MmArc, ev: EventType) {
        if let Some(interested) = self.dispatch_table.get(&ev.event_id()) {
            for id in interested.iter().copied() {
                self.listeners[id].process_event_async(ctx.clone(), ev.clone()).await;
            }
        }
    }

    #[cfg(test)]
    pub fn nb_listeners(&self) -> usize {
        self.listeners.len()
    }
}

#[cfg(test)]
mod event_dispatcher_tests {
    use crate::event_dispatcher::{Dispatcher, EventListener, EventUniqueId};
    use crate::mm_ctx::{MmArc, MmCtxBuilder};
    use async_trait::async_trait;
    use common::block_on;
    use std::any::TypeId;
    use std::ops::Deref;
    use std::sync::Arc;
    use uuid::Uuid;

    #[derive(Clone, Default)]
    struct EventSwapStatusChanged {
        uuid: Uuid,
        status: String,
    }

    impl EventSwapStatusChanged {
        fn event_id() -> TypeId {
            TypeId::of::<EventSwapStatusChanged>()
        }
    }

    #[derive(Clone)]
    enum AppEvents {
        EventSwapStatusChanged(EventSwapStatusChanged),
    }

    impl EventUniqueId for AppEvents {
        fn event_id(&self) -> TypeId {
            match self {
                AppEvents::EventSwapStatusChanged(_) => EventSwapStatusChanged::event_id(),
            }
        }
    }

    #[derive(Default)]
    struct ListenerSwapStatusChanged;

    #[derive(Clone)]
    struct ListenerSwapStatusChangedArc(Arc<ListenerSwapStatusChanged>);

    impl Deref for ListenerSwapStatusChangedArc {
        type Target = ListenerSwapStatusChanged;
        fn deref(&self) -> &ListenerSwapStatusChanged {
            &self.0
        }
    }

    #[async_trait]
    impl EventListener for ListenerSwapStatusChangedArc {
        type Event = AppEvents;

        async fn process_event_async(&self, _ctx: MmArc, event: Self::Event) {
            match event {
                AppEvents::EventSwapStatusChanged(swap) => {
                    assert_eq!(swap.uuid.to_string(), "00000000-0000-0000-0000-000000000000");
                    assert_eq!(swap.status, "Started");
                },
            }
        }

        fn get_desired_events(&self) -> Vec<TypeId> {
            vec![EventSwapStatusChanged::event_id()]
        }

        fn listener_id(&self) -> &'static str {
            "listener_swap_status_changed"
        }
    }

    #[test]
    fn test_dispatcher() {
        let mut dispatcher: Dispatcher<AppEvents> = Default::default();
        let listener = ListenerSwapStatusChanged;
        let res = ListenerSwapStatusChangedArc(Arc::new(listener));
        dispatcher.add_listener(res.clone());
        assert_eq!(dispatcher.nb_listeners(), 1);

        let event = AppEvents::EventSwapStatusChanged(EventSwapStatusChanged {
            uuid: Default::default(),
            status: "Started".to_string(),
        });
        block_on(dispatcher.dispatch_async(MmCtxBuilder::new().into_mm_arc(), event));
        dispatcher.add_listener(res);
        assert_eq!(dispatcher.nb_listeners(), 1);
    }
}
