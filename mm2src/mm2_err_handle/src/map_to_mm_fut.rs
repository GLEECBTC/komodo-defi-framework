use crate::mm_error::{MmError, NotMmError, TraceLocation};
use futures01::{Future, Poll};
use std::panic::Location;

pub trait MapToMmFutureExt<'a, T, E1: NotMmError, E2: NotMmError> {
    fn map_to_mm_fut<F>(self, f: F) -> MapToMmFuture<'a, T, E1, E2>
    where
        F: FnOnce(E1) -> E2 + Send + 'a;
}

impl<'a, Fut, T, E1, E2> MapToMmFutureExt<'a, T, E1, E2> for Fut
where
    Fut: Future<Item = T, Error = E1> + Send + 'static,
    E1: NotMmError,
    E2: NotMmError,
{
    /// Maps a [`Future01<Item=T, Error=E1>`] to [`Future01<Item=T, Error=MmError<E2>>`] by applying a function to a
    /// contained [`Error`] value, leaving an [`Item`] value untouched.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let fut = futures01::future::err("An error".to_owned());
    /// let mapped_res: Result<(), MmError<usize>> = block_on_f01(fut.map_to_mm_fut(|e| e.len()));
    /// ```
    #[track_caller]
    fn map_to_mm_fut<F>(self, f: F) -> MapToMmFuture<'a, T, E1, E2>
    where
        F: FnOnce(E1) -> E2 + Send + 'a,
    {
        MapToMmFuture {
            inner: Box::new(self),
            location: Some(TraceLocation::from(Location::caller())),
            closure: Some(Box::new(f)),
        }
    }
}

#[must_use = "futures do nothing unless polled"]
pub struct MapToMmFuture<'a, T, E1: NotMmError, E2: NotMmError> {
    inner: Box<dyn Future<Item = T, Error = E1> + Send>,
    location: Option<TraceLocation>,
    closure: Option<Box<dyn FnOnce(E1) -> E2 + Send + 'a>>,
}

impl<T, E1: NotMmError, E2: NotMmError> Future for MapToMmFuture<'_, T, E1, E2> {
    type Item = T;
    type Error = MmError<E2>;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.inner.poll() {
            Ok(t) => Ok(t),
            Err(e) => {
                let location = self
                    .location
                    .take()
                    .expect("Attempted to poll IntoMmFutureAnd after completion");
                let closure = self
                    .closure
                    .take()
                    .expect("Attempted to poll IntoMmFutureAnd after completion");
                Poll::Err(MmError {
                    etype: closure(e),
                    trace: vec![location],
                })
            },
        }
    }
}
