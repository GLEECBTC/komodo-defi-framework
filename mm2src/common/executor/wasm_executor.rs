use crate::executor::AbortOnDropHandle;
use crate::now_float;
use crate::number_type_casting::SafeTypeCastingNumbers;
use futures::future::{abortable, FutureExt};
use futures::task::{Context, Poll};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::Duration;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    // https://developer.mozilla.org/en-US/docs/Web/API/WindowOrWorkerGlobalScope/setTimeout
    fn setTimeout(closure: &Closure<dyn FnMut()>, delay_ms: u32) -> i32;

    // https://developer.mozilla.org/en-US/docs/Web/API/WindowOrWorkerGlobalScope/clearTimeout
    fn clearTimeout(id: i32);
}

/// # Important
///
/// The `spawn` function must be used carefully to avoid hanging pointers.
/// Please consider using `AbortableQueue`, `AbortableSimpleMap` or `spawn_abortable` instead.
pub fn spawn(future: impl Future<Output = ()> + Send + 'static) {
    spawn_local(future)
}

/// # Important
///
/// The `spawn` function must be used carefully to avoid hanging pointers.
/// Please consider using `AbortableQueue`, `AbortableSimpleMap` or `spawn_abortable` instead.
pub fn spawn_local(future: impl Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(future)
}

pub fn spawn_local_abortable(future: impl Future<Output = ()> + 'static) -> AbortOnDropHandle {
    let (abortable, handle) = abortable(future);
    spawn_local(abortable.then(|_| futures::future::ready(())));
    AbortOnDropHandle::from(handle)
}

/// The timer uses [`setTimeout`] and [`clearTimeout`] for scheduling.
/// See the [example](https://rustwasm.github.io/docs/wasm-bindgen/reference/passing-rust-closures-to-js.html#heap-allocated-closures).
///
/// According to the [blogpost](https://rustwasm.github.io/2018/10/24/multithreading-rust-and-wasm.html),
/// very few types in [`wasm_bindgen`] are `Send` and `Sync`, and [`wasm_bindgen::closure::Closure`] is not an exception.
/// Although wasm is currently single-threaded, we can implement the `Send` trait for the `Timer`,
/// but it won't be safe when wasm becomes multi-threaded.
#[must_use = "futures do nothing unless polled"]
pub struct Timer {
    timeout_id: i32,
    _closure: Closure<dyn FnMut()>,
    state: Arc<Mutex<TimerState>>,
}

unsafe impl Send for Timer {}

impl Timer {
    pub fn till(till_utc: f64) -> Timer {
        let secs = till_utc - now_float();
        Timer::sleep(secs)
    }

    pub fn sleep(secs: f64) -> Timer {
        let dur = Duration::from_secs_f64(secs);
        let delay_ms = gstuff::duration_to_ms(dur);
        Timer::sleep_ms(delay_ms)
    }

    pub fn sleep_ms(delay_ms: u64) -> Timer {
        fn on_timeout(state: &Arc<Mutex<TimerState>>) {
            let mut state = match state.lock() {
                Ok(s) => s,
                Err(e) => {
                    log::error!("!on_timeout: {}", e);
                    return;
                },
            };
            state.completed = true;
            if let Some(waker) = state.waker.take() {
                waker.wake();
            }
        }

        let state = Arc::new(Mutex::new(TimerState::default()));
        let state_c = state.clone();
        // we should hold the closure until the callback function is called
        let closure = Closure::new(move || on_timeout(&state_c));

        let timeout_id = setTimeout(&closure, delay_ms.into_or_max());
        Timer {
            timeout_id,
            _closure: closure,
            state,
        }
    }
}

/// When the `Timer` is destroyed, cancel its `setTimeout` timer.
impl Drop for Timer {
    fn drop(&mut self) {
        clearTimeout(self.timeout_id)
    }
}

impl Future for Timer {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        let mut state = match self.state.lock() {
            Ok(s) => s,
            Err(e) => {
                log::error!("!Timer::poll: {}", e);
                // if the mutex is poisoned, this error will appear every poll iteration
                return Poll::Ready(());
            },
        };
        if state.completed {
            return Poll::Ready(());
        }

        // NB: We should get a new `Waker` on every `poll` in case the future migrates between executors.
        // cf. https://rust-lang.github.io/async-book/02_execution/03_wakeups.html
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

#[derive(Default)]
struct TimerState {
    completed: bool,
    waker: Option<Waker>,
}
