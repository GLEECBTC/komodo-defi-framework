use crate::mm_error::{MmError, NotMmError};

pub trait MapToMmResult<T, E1, E2>
where
    E1: NotMmError,
    E2: NotMmError,
{
    fn map_to_mm<F>(self, f: F) -> Result<T, MmError<E2>>
    where
        F: FnOnce(E1) -> E2;
}

impl<T, E1, E2> MapToMmResult<T, E1, E2> for Result<T, E1>
where
    E1: NotMmError,
    E2: NotMmError,
{
    /// Maps a [`Result<T, E1>`] to [`Result<T, MmError<E2>>`] by applying a function to a
    /// contained [`Err`] value, leaving an [`Ok`] value untouched.
    ///
    /// # Important
    ///
    /// Please consider using `?` operator if `E2: From<E1>`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let res: Result<(), String> = Err("An error".to_owned());
    /// let mapped_res: Result<(), MmError<usize>> = res.map_to_mm(|e| e.len());
    /// ```
    #[track_caller]
    fn map_to_mm<F>(self, f: F) -> Result<T, MmError<E2>>
    where
        F: FnOnce(E1) -> E2,
    {
        match self {
            Ok(x) => Ok(x),
            Err(e1) => Err(MmError::new(f(e1))),
        }
    }
}
