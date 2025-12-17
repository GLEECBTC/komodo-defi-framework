use futures::task::Context;
use futures::task::Poll as Poll03;
use futures::Future as Future03;
use futures_timer::Delay;
use std::pin::Pin;
use std::time::Duration;

/// # Important
///
/// The `spawn` function must be used carefully to avoid hanging pointers.
/// Please consider using `AbortableQueue`, `AbortableSimpleMap` or `spawn_abortable` instead.
pub fn spawn(future: impl Future03<Output = ()> + Send + 'static) {
    crate::wio::CORE.0.spawn(future);
}

/// A future that completes at a given time.
#[must_use]
pub struct Timer {
    delay: Delay,
}

impl Timer {
    pub fn sleep(seconds: f64) -> Timer {
        Timer {
            delay: Delay::new(Duration::from_secs_f64(seconds)),
        }
    }

    pub fn sleep_ms(ms: u64) -> Timer {
        Timer {
            delay: Delay::new(Duration::from_millis(ms)),
        }
    }
}

impl Future03 for Timer {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll03<Self::Output> {
        Pin::new(&mut self.delay).poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::now_float;

    #[test]
    fn test_timer() {
        let started = now_float();
        let ti = Timer::sleep(0.2);
        let delta = now_float() - started;
        assert!(delta < 0.04, "{}", delta);
        crate::block_on(ti);
        let delta = now_float() - started;
        println!("time delta is {delta}");
        assert!(delta > 0.2);
        assert!(delta < 0.4)
    }
}
