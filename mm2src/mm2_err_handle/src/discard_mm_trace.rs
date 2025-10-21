use crate::mm_error::{MmError, NotMmError};

pub trait DiscardMmTrace<T, E>
where
    E: NotMmError,
{
    fn discard_mm_trace(self) -> Result<T, E>;
}

impl<T, E> DiscardMmTrace<T, E> for Result<T, MmError<E>>
where
    E: NotMmError,
{
    /// Discards the error trace and maps `Err(MmError<E>)` into `Err(E)`.
    ///
    /// This method can be used to match the inner `E` error and at the same time not to loose the trace.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let res: Result<(), _> = MmError::err("Not found");
    /// match res.discard_mm_trace() {
    ///   Ok(_) => (),
    ///   Err("Internal error") => return Err(NewErrorType {}),
    ///   Err("Not found") => return Ok(None),
    /// }
    /// ```
    fn discard_mm_trace(self) -> Result<T, E> {
        self.map_err(MmError::into_inner)
    }
}
