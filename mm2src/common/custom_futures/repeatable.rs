//! A future that can be repeated if an error occurs or not all conditions are met.
//!
//! # Why `async move` shouldn't be allowed
//!
//! Let's consider the following example:
//!
//! ```rust
//! let mut counter = 0;
//! let res = repeatable!(async move {
//!   counter += 1;
//!   if counter > 1 { Ready(()) } else { Retry(()) }
//! })
//! .repeat_every_secs(0.1)
//! .attempts(10)
//! .await;
//!
//! res.expect_err("'counter' will never be greater than 1");
//! ```
//!
//! This happens due to the fact that the `counter` variable is not shared between attempts,
//! and every time the future starts with `counter = 0`.

use crate::executor::Timer;
use crate::number_type_casting::SafeTypeCastingNumbers;
use crate::{now_ms, wait_until_ms};
use futures::FutureExt;
use log::warn;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

pub use Action::{Ready, Retry};

/// Wraps the given future into `Repeatable` future.
/// The future should return [`Action<T, E>`] with any `T` and `E` types.
#[macro_export]
macro_rules! repeatable {
    (async { $($t:tt)* }) => {
        $crate::custom_futures::repeatable::Repeatable::new(|| Box::pin(async { $($t)* }))
    };
    ($fut:expr) => {
        $crate::custom_futures::repeatable::Repeatable::new(|| $fut)
    };
}

/// Wraps the given future into `Repeatable` future.
/// The future should return [`Result<T, E>`], where
/// * `Ok(T)` => `Action::Ready(T)`
/// * `Err(E)` => `Action::Retry(E)`
#[macro_export]
macro_rules! retry_on_err {
    (async { $($t:tt)* }) => {
        $crate::custom_futures::repeatable::Repeatable::new(|| {
            use $crate::custom_futures::repeatable::RetryOnError;
            use futures::FutureExt;

            let fut = async { $($t)* };
            Box::pin(fut.map(Result::retry_on_err))
        })
    };
    ($fut:expr) => {
        $crate::custom_futures::repeatable::Repeatable::new(|| {
            use $crate::custom_futures::repeatable::RetryOnError;
            use futures::FutureExt;

            $fut.map(Result::retry_on_err)
        })
    };
}

/// Unwraps a result or returns `Action::Retry(E)`.
#[macro_export]
macro_rules! try_or_retry {
    ($exp:expr) => {{
        match $exp {
            Ok(t) => t,
            Err(e) => return $crate::custom_futures::repeatable::Retry(e),
        }
    }};
}

/// Unwraps a result or returns `Action::Ready(E)`.
#[macro_export]
macro_rules! try_or_ready_err {
    ($exp:expr) => {{
        match $exp {
            Ok(t) => t,
            Err(e) => return $crate::custom_futures::repeatable::Ready(Err(e)),
        }
    }};
}

const DEFAULT_REPEAT_EVERY: Duration = Duration::from_secs(1);

pub trait FactoryTrait<F>: Fn() -> F {}

impl<Factory, F> FactoryTrait<F> for Factory where Factory: Fn() -> F {}

pub trait RepeatableTrait<T, E>: Future<Output = Action<T, E>> + Unpin {}

impl<F, T, E> RepeatableTrait<T, E> for F where F: Future<Output = Action<T, E>> + Unpin {}

pub(crate) trait InspectErrorTrait<E>: 'static + Fn(&E) + Send {}

impl<F: 'static + Fn(&E) + Send, E> InspectErrorTrait<E> for F {}

#[derive(Clone, Debug, PartialEq)]
pub enum RepeatError<E> {
    TimeoutExpired {
        until_ms: u64,
        /// An error occurred during the last attempt.
        error: E,
    },
    AttemptsExceed {
        attempts: usize,
        /// An error occurred during the last attempt.
        error: E,
    },
}

impl<E> RepeatError<E> {
    pub fn error(&self) -> &E {
        match self {
            RepeatError::TimeoutExpired { error, .. } | RepeatError::AttemptsExceed { error, .. } => error,
        }
    }

    pub fn into_error(self) -> E {
        match self {
            RepeatError::TimeoutExpired { error, .. } | RepeatError::AttemptsExceed { error, .. } => error,
        }
    }

    fn timeout(until_ms: u64, error: E) -> Self {
        RepeatError::TimeoutExpired { until_ms, error }
    }

    fn attempts(attempts: usize, error: E) -> Self {
        RepeatError::AttemptsExceed { attempts, error }
    }
}

impl<E: fmt::Display> fmt::Display for RepeatError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepeatError::TimeoutExpired { until_ms, error } => {
                write!(
                    f,
                    "Waited too long until {until_ms}ms for the future to succeed. Error: {error}",
                )
            },
            RepeatError::AttemptsExceed { attempts, error } => {
                write!(f, "Error {error} on retrying the future after {attempts} attempts")
            },
        }
    }
}

/// The future is ether ready (with a `T` result), or not ready (failed with an intermediate `E` error).
#[derive(Debug)]
pub enum Action<T, E> {
    Ready(T),
    Retry(E),
}

pub trait RetryOnError<T, E> {
    fn retry_on_err(self) -> Action<T, E>;
}

impl<T, E> RetryOnError<T, E> for Result<T, E> {
    /// Converts `Result<T, E>` into `Action<T, E>`:
    /// * `Ok(T)` => `Action::Ready(T)`.
    /// * `Err(E)` => `Action::Retry(E)`.
    #[inline]
    fn retry_on_err(self) -> Action<T, E> {
        match self {
            Ok(ready) => Action::Ready(ready),
            Err(e) => Action::Retry(e),
        }
    }
}

/// The result of `repeatable` or `retry_on_err` macros - the first step at the future configuration.
pub struct Repeatable<Factory, F, T, E> {
    factory: Factory,
    /// Currently executable future, i.e. an active attempt.
    exec_fut: F,
    /// A timeout future if we're currently waiting for a timeout.
    timeout_fut: Option<Timer>,
    until: RepeatUntil,
    repeat_every: Duration,
    inspect_err: Option<Box<dyn InspectErrorTrait<E>>>,
    _phantom: PhantomData<(F, T, E)>,
}

impl<Factory, F, T, E> Repeatable<Factory, F, T, E>
where
    Factory: FactoryTrait<F>,
    F: RepeatableTrait<T, E>,
{
    #[inline]
    pub fn new(factory: Factory) -> Self {
        let exec_fut = factory();

        Repeatable {
            factory,
            exec_fut,
            timeout_fut: None,
            until: RepeatUntil::default(),
            repeat_every: DEFAULT_REPEAT_EVERY,
            inspect_err: None,
            _phantom: Default::default(),
        }
    }

    /// Specifies an inspect handler that does something with an error on each unsuccessful attempt.
    #[inline]
    pub fn inspect_err<Inspect>(mut self, inspect: Inspect) -> Self
    where
        Inspect: 'static + Fn(&E) + Send,
    {
        self.inspect_err = Some(Box::new(inspect));
        self
    }

    #[inline]
    pub fn repeat_every(mut self, repeat_every: Duration) -> Self {
        self.repeat_every = repeat_every;
        self
    }

    #[inline]
    pub fn repeat_every_ms(self, repeat_every: u64) -> Self {
        self.repeat_every(Duration::from_millis(repeat_every))
    }

    #[inline]
    pub fn repeat_every_secs(self, repeat_every: f64) -> Self {
        self.repeat_every(Duration::from_secs_f64(repeat_every))
    }

    /// Repeat the future until it's ready.
    ///
    /// # Warning
    ///
    /// This may lead to an endless loop if the future is never ready.
    #[inline]
    pub fn until_ready(mut self) -> Self {
        self.until = RepeatUntil::Ready;
        self
    }

    /// Specifies a total number of attempts to run the future.
    /// So there will be up to `total_attempts`.
    ///
    /// # Panic
    ///
    /// Panics if `total_attempts` is 0.
    #[inline]
    pub fn attempts(mut self, total_attempts: usize) -> Self {
        assert!(total_attempts > 0, "'total_attempts' cannot be 0");

        self.until = RepeatUntil::AttemptsExceed(AttemptsState::new(total_attempts));
        self
    }

    /// Specifies a deadline in milliseconds before that we may try to repeat the future.
    #[inline]
    pub fn until_ms(mut self, until_ms: u64) -> Self {
        let now = now_ms();
        if now >= until_ms {
            warn!("Deadline has already passed: now={now:?} until={until_ms:?}")
        }

        self.until = RepeatUntil::TimeoutMsExpired(until_ms);
        self
    }

    /// Specifies a deadline in seconds before that we may try to repeat the future.
    #[inline]
    pub fn until_s(self, until_s: u64) -> Self {
        let until_ms = until_s * 1000;
        self.until_ms(until_ms)
    }

    /// Specifies a timeout in milliseconds before that we may try to repeat the future.
    /// Note this method name should differ from [`FutureTimerExt::timeout_ms`].
    #[inline]
    pub fn with_timeout_ms(self, timeout_ms: u64) -> Self {
        self.until_ms(wait_until_ms(timeout_ms))
    }

    /// Specifies a timeout in seconds before that we may try to repeat the future.
    /// Note this method name should differ from [`FutureTimerExt::timeout_secs`].
    #[inline]
    pub fn with_timeout_secs(self, timeout_secs: f64) -> Self {
        let timeout_ms = (timeout_secs * 1000.) as u64;
        self.until_ms(wait_until_ms(timeout_ms))
    }

    /// Checks if the deadline is not going to be reached after the `repeat_every` timeout.
    fn check_can_retry_after_timeout(&self, until_ms: u64) -> bool {
        let repeat_every_ms: u64 = self.repeat_every.as_millis().into_or_max();
        let will_be_after_timeout = wait_until_ms(repeat_every_ms);
        will_be_after_timeout < until_ms
    }
}

impl<Factory, F: Unpin, T, E> Unpin for Repeatable<Factory, F, T, E> {}

impl<Factory, F, T, E> Future for Repeatable<Factory, F, T, E>
where
    Factory: FactoryTrait<F>,
    F: RepeatableTrait<T, E>,
{
    type Output = Result<T, RepeatError<E>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            if poll_timeout(&mut self.timeout_fut, cx).is_pending() {
                return Poll::Pending;
            }

            match self.exec_fut.poll_unpin(cx) {
                Poll::Ready(Ready(ready)) => return Poll::Ready(Ok(ready)),
                Poll::Ready(Retry(error)) => {
                    if let Some(ref inspect) = self.inspect_err {
                        inspect(&error);
                    }

                    match self.until {
                        RepeatUntil::TimeoutMsExpired(until_ms) => {
                            if !self.check_can_retry_after_timeout(until_ms) {
                                return Poll::Ready(Err(RepeatError::timeout(until_ms, error)));
                            }
                        },
                        RepeatUntil::AttemptsExceed(ref mut attempts) => {
                            // Check if we have one more attempt to retry to execute the future.
                            attempts.current_attempt += 1;
                            if attempts.current_attempt >= attempts.total_attempts {
                                return Poll::Ready(Err(RepeatError::attempts(attempts.current_attempt, error)));
                            }
                        },
                        // Repeat until the future is ready.
                        RepeatUntil::Ready => (),
                    }

                    // Create a new future attempt.
                    self.exec_fut = (self.factory)();
                    // Reset the timeout future.
                    self.timeout_fut = Some(Timer::sleep(self.repeat_every.as_secs_f64()));
                },
                // We should proceed with this `exec` future attempt later.
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

struct AttemptsState {
    current_attempt: usize,
    total_attempts: usize,
}

impl AttemptsState {
    fn new(total_attempts: usize) -> AttemptsState {
        AttemptsState {
            current_attempt: 0,
            total_attempts,
        }
    }
}

enum RepeatUntil {
    TimeoutMsExpired(u64),
    AttemptsExceed(AttemptsState),
    Ready,
}

impl Default for RepeatUntil {
    fn default() -> Self {
        RepeatUntil::AttemptsExceed(AttemptsState::new(1))
    }
}

/// Returns `Poll::Ready(())` if there is no need to wait for the timeout.
fn poll_timeout(timeout_fut: &mut Option<Timer>, cx: &mut Context<'_>) -> Poll<()> {
    let mut timeout = match timeout_fut.take() {
        Some(timeout) => timeout,
        None => return Poll::Ready(()),
    };

    match timeout.poll_unpin(cx) {
        Poll::Ready(_) => Poll::Ready(()),
        Poll::Pending => {
            *timeout_fut = Some(timeout);
            Poll::Pending
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_on;
    use futures::lock::Mutex as AsyncMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::Instant;

    async fn an_operation(counter: &AsyncMutex<usize>, finish_if: usize) -> Result<usize, &str> {
        let mut counter = counter.lock().await;
        *counter += 1;
        if *counter == finish_if {
            Ok(*counter)
        } else {
            Err("Not ready")
        }
    }

    #[test]
    fn test_attempts_success() {
        const ATTEMPTS_TO_FINISH: usize = 3;

        let counter = AsyncMutex::new(0);

        let fut = repeatable!(async { an_operation(&counter, ATTEMPTS_TO_FINISH).await.retry_on_err() })
            .repeat_every(Duration::from_millis(100))
            .attempts(ATTEMPTS_TO_FINISH);

        let actual = block_on(fut);
        // If the counter is 3, then there were exactly 3 attempts to finish the future.
        assert_eq!(actual, Ok(ATTEMPTS_TO_FINISH));
    }

    #[test]
    fn test_attempts_exceed() {
        const ATTEMPTS_TO_FINISH: usize = 3;
        const ACTUAL_ATTEMPTS: usize = 2;

        let counter = AsyncMutex::new(0);

        let fut = repeatable!(async { an_operation(&counter, ATTEMPTS_TO_FINISH).await.retry_on_err() })
            .repeat_every(Duration::from_millis(100))
            .attempts(ACTUAL_ATTEMPTS);

        let actual = block_on(fut);
        assert_eq!(
            actual,
            Err(RepeatError::AttemptsExceed {
                attempts: ACTUAL_ATTEMPTS,
                error: "Not ready"
            })
        );

        // If the counter is 2, then there were exactly 2 attempts to finish the future.
        let actual_attempts = block_on(counter.lock());
        assert_eq!(*actual_attempts, ACTUAL_ATTEMPTS);
    }

    #[test]
    fn test_attempts_retry_on_err() {
        const ATTEMPTS_TO_FINISH: usize = 3;

        let counter = AsyncMutex::new(0);

        let fut = repeatable!(async { an_operation(&counter, ATTEMPTS_TO_FINISH).await.retry_on_err() })
            .repeat_every(Duration::from_millis(100))
            .attempts(ATTEMPTS_TO_FINISH);

        let actual = block_on(fut);
        assert_eq!(actual, Ok(ATTEMPTS_TO_FINISH));
    }

    #[test]
    fn test_attempts_retry_on_err_macro() {
        const ATTEMPTS_TO_FINISH: usize = 3;

        let counter = AsyncMutex::new(0);

        let fut = retry_on_err!(async { an_operation(&counter, ATTEMPTS_TO_FINISH).await })
            .repeat_every(Duration::from_millis(100))
            .attempts(ATTEMPTS_TO_FINISH);

        let actual = block_on(fut);
        assert_eq!(actual, Ok(ATTEMPTS_TO_FINISH));
    }

    #[test]
    fn test_attempts_inspect_err() {
        const ATTEMPTS_TO_FINISH: usize = 3;
        const FAILED_ATTEMPTS: usize = 2;

        let inspect_counter = Arc::new(AtomicUsize::new(0));
        let inspect_counter_c = inspect_counter.clone();
        let counter = AsyncMutex::new(0);

        let fut = repeatable!(async { an_operation(&counter, ATTEMPTS_TO_FINISH).await.retry_on_err() })
            .repeat_every(Duration::from_millis(100))
            .inspect_err(move |_| {
                inspect_counter.fetch_add(1, Ordering::Relaxed);
            })
            .attempts(ATTEMPTS_TO_FINISH);

        let actual = block_on(fut);
        // If the counter is 3, then there were exactly 3 attempts to finish the future.
        assert_eq!(actual, Ok(ATTEMPTS_TO_FINISH));
        // There should be 2 errors.
        assert_eq!(inspect_counter_c.load(Ordering::Relaxed), FAILED_ATTEMPTS);
    }

    #[test]
    #[cfg(not(target_os = "macos"))] // https://github.com/KomodoPlatform/komodo-defi-framework/issues/1712#issuecomment-2669934159
    fn test_until_success() {
        const ATTEMPTS_TO_FINISH: usize = 5;
        const LOWEST_TIMEOUT: Duration = Duration::from_millis(350);
        const HIGHEST_TIMEOUT: Duration = Duration::from_millis(800);

        let counter = AsyncMutex::new(0);

        let fut = repeatable!(async { an_operation(&counter, ATTEMPTS_TO_FINISH).await.retry_on_err() })
            .repeat_every(Duration::from_millis(100))
            .until_ms(wait_until_ms(HIGHEST_TIMEOUT.as_millis() as u64));

        let before = Instant::now();
        let actual = block_on(fut);
        let took = before.elapsed();

        // If the counter is 3, then there were exactly 3 attempts to finish the future.
        assert_eq!(actual, Ok(ATTEMPTS_TO_FINISH));

        assert!(
            LOWEST_TIMEOUT <= took && took <= HIGHEST_TIMEOUT,
            "Expected [{:?}, {:?}], but took {:?}",
            LOWEST_TIMEOUT,
            HIGHEST_TIMEOUT,
            took
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))] // https://github.com/KomodoPlatform/atomicDEX-API/issues/1712
    fn test_until_expired() {
        const ATTEMPTS_TO_FINISH: usize = 10;
        const LOWEST_TIMEOUT: Duration = Duration::from_millis(350);
        const HIGHEST_TIMEOUT: Duration = Duration::from_millis(800);

        let counter = AsyncMutex::new(0);

        let until_ms = wait_until_ms(HIGHEST_TIMEOUT.as_millis() as u64);

        let fut = repeatable!(async { an_operation(&counter, ATTEMPTS_TO_FINISH).await.retry_on_err() })
            .repeat_every(Duration::from_millis(100))
            .until_ms(until_ms);

        let before = Instant::now();
        let actual = block_on(fut);
        let took = before.elapsed();

        // If the counter is 3, then there were exactly 3 attempts to finish the future.
        let error = RepeatError::TimeoutExpired {
            until_ms,
            error: "Not ready",
        };
        assert_eq!(actual, Err(error));

        assert!(
            LOWEST_TIMEOUT <= took && took <= HIGHEST_TIMEOUT,
            "Expected [{:?}, {:?}], but took {:?}",
            LOWEST_TIMEOUT,
            HIGHEST_TIMEOUT,
            took
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))] // https://github.com/KomodoPlatform/atomicDEX-API/issues/1712
    fn test_until_ms() {
        const ATTEMPTS_TO_FINISH: usize = 5;
        const LOWEST_TIMEOUT: u64 = 350;
        const HIGHEST_TIMEOUT: u64 = 800;

        let counter = AsyncMutex::new(0);

        let fut = repeatable!(async { an_operation(&counter, ATTEMPTS_TO_FINISH).await.retry_on_err() })
            .repeat_every(Duration::from_millis(100))
            .until_ms(now_ms() + HIGHEST_TIMEOUT);

        let before = Instant::now();
        let actual = block_on(fut);
        let took = before.elapsed();

        // If the counter is 3, then there were exactly 3 attempts to finish the future.
        assert_eq!(actual, Ok(ATTEMPTS_TO_FINISH));

        let lowest = Duration::from_millis(LOWEST_TIMEOUT);
        let highest = Duration::from_millis(HIGHEST_TIMEOUT);
        assert!(
            lowest <= took && took <= highest,
            "Expected [{:?}, {:?}], but took {:?}",
            lowest,
            highest,
            took
        );
    }

    /// `Repeatable` future should be executed the only once
    /// if neither [`Repeatable::until`] nor [`Repeatable::attempts`] are specified.
    ///
    /// The first case within the following:
    /// https://github.com/KomodoPlatform/atomicDEX-API/pull/1564#discussion_r1040989842
    #[test]
    fn test_without_attempts_and_timeout() {
        const ATTEMPTS_TO_FINISH: usize = 5;

        let counter = AsyncMutex::new(0);

        let fut = repeatable!(async { an_operation(&counter, ATTEMPTS_TO_FINISH).await.retry_on_err() });

        let actual = block_on(fut);

        assert_eq!(
            actual,
            Err(RepeatError::AttemptsExceed {
                attempts: 1,
                error: "Not ready"
            })
        );
    }

    /// `Repeatable` future should be executed the only once
    /// if neither [`Repeatable::until`] nor [`Repeatable::attempts`] are specified.
    /// Please note that in this case [`Repeatable::repeat_every`] should have no effect.
    ///
    /// The first case within the following:
    /// https://github.com/KomodoPlatform/atomicDEX-API/pull/1564#discussion_r1040989842
    #[test]
    fn test_repeat_every_without_attempts_and_timeout() {
        const ATTEMPTS_TO_FINISH: usize = 5;
        const LOWEST_TIMEOUT: Duration = Duration::from_micros(0);
        const HIGHEST_TIMEOUT: Duration = Duration::from_millis(100);

        let counter = AsyncMutex::new(0);

        let fut = repeatable!(async { an_operation(&counter, ATTEMPTS_TO_FINISH).await.retry_on_err() })
            .repeat_every_secs(10.);

        let before = Instant::now();
        let actual = block_on(fut);
        let took = before.elapsed();

        assert_eq!(
            actual,
            Err(RepeatError::AttemptsExceed {
                attempts: 1,
                error: "Not ready"
            })
        );

        assert!(
            LOWEST_TIMEOUT <= took && took <= HIGHEST_TIMEOUT,
            "Expected [{:?}, {:?}], but took {:?}",
            LOWEST_TIMEOUT,
            HIGHEST_TIMEOUT,
            took
        );
    }
}
