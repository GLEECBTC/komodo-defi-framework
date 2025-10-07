use futures::future::abortable;
use futures::{Future as Future03, FutureExt};

#[cfg(not(target_arch = "wasm32"))]
mod native_executor;
#[cfg(not(target_arch = "wasm32"))]
pub use native_executor::{spawn, Timer};

mod abortable_system;
pub use abortable_system::{abortable_queue, graceful_shutdown, simple_map, AbortableSystem, AbortedError};

mod spawner;
pub use spawner::{BoxFutureSpawner, SpawnAbortable, SpawnFuture};

mod abort_on_drop;
pub use abort_on_drop::AbortOnDropHandle;

#[cfg(target_arch = "wasm32")]
mod wasm_executor;
#[cfg(target_arch = "wasm32")]
pub use wasm_executor::{spawn, spawn_local, spawn_local_abortable, Timer};

#[derive(Clone, Default)]
pub struct AbortSettings {
    on_finish: Option<SpawnMsg>,
    on_abort: Option<SpawnMsg>,
    critical_timeout_s: Option<f64>,
}

impl AbortSettings {
    pub fn info_on_any_stop(msg: String) -> AbortSettings {
        let msg = SpawnMsg {
            level: log::Level::Info,
            msg,
        };
        AbortSettings {
            on_finish: Some(msg.clone()),
            on_abort: Some(msg),
            critical_timeout_s: None,
        }
    }

    pub fn info_on_finish(msg: String) -> AbortSettings {
        let msg = SpawnMsg {
            level: log::Level::Info,
            msg,
        };
        AbortSettings {
            on_finish: Some(msg),
            on_abort: None,
            critical_timeout_s: None,
        }
    }

    pub fn info_on_abort(msg: String) -> AbortSettings {
        let msg = SpawnMsg {
            level: log::Level::Info,
            msg,
        };
        AbortSettings {
            on_finish: None,
            on_abort: Some(msg),
            critical_timeout_s: None,
        }
    }

    pub fn critical_timout_s(mut self, critical_timout_s: f64) -> AbortSettings {
        self.critical_timeout_s = Some(critical_timout_s);
        self
    }
}

#[derive(Clone)]
struct SpawnMsg {
    level: log::Level,
    msg: String,
}

#[must_use]
pub fn spawn_abortable(fut: impl Future03<Output = ()> + Send + 'static) -> AbortOnDropHandle {
    let (abortable, handle) = abortable(fut);
    spawn(abortable.then(|_| futures::future::ready(())));
    AbortOnDropHandle::from(handle)
}
