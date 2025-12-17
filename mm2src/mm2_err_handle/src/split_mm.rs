use crate::mm_error::{MmError, MmErrorTrace, NotMmError};

pub trait SplitMmResult<T, E>
where
    E: NotMmError,
{
    fn split_mm(self) -> Result<T, (E, MmErrorTrace)>;
}

impl<T, E> SplitMmResult<T, E> for Result<T, MmError<E>>
where
    E: NotMmError,
{
    /// Splits the inner `Err(MmError<E>)` into inner `E` and the error trace.
    ///
    /// This method can be used to match the inner `E` error and at the same time not to loose the trace.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let res: Result<(), _> = MmError::err("Not found");
    /// match res.split_mm() {
    ///   Ok(_) => (),
    ///   Err("Internal", trace) => return MmError::err_with_trace(NewErrorType {}, trace),
    ///   Err("Not found", _trace) => return Ok(None),
    /// }
    /// ```
    fn split_mm(self) -> Result<T, (E, MmErrorTrace)> {
        self.map_err(MmError::split)
    }
}
