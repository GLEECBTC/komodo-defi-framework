use crate::mm_error::{MmError, NotMmError};
use common::SerializationError;
use ser_error::SerializeErrorType;
use serde::{Deserialize, Serialize};
use serde_json::{self as json, Error as JsonError, Value as Json};
use std::fmt;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MmJsonError(Json);

impl fmt::Display for MmJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// We are sure that `MmJsonError` is constructed from a type that implements `SerializeErrorTypeImpl`.
/// See [`MmJsonError::new`].
impl ser_error::__private::SerializeErrorTypeImpl for MmJsonError {}

impl MmJsonError {
    pub fn new<E: SerializeErrorType>(error: E) -> Result<MmJsonError, JsonError> {
        json::to_value(error).map(MmJsonError)
    }

    pub fn new_or_serialization_error<E: SerializeErrorType>(error: E) -> MmJsonError {
        match MmJsonError::new(error) {
            Ok(mm_json_error) => mm_json_error,
            Err(serialization_error) => MmJsonError::serialization_error(serialization_error),
        }
    }

    pub fn from_mm_error<E: SerializeErrorType + NotMmError>(
        error: MmError<E>,
    ) -> Result<MmError<MmJsonError>, JsonError> {
        let (etype, trace) = error.split();
        let etype_json = MmJsonError::new(etype)?;
        Ok(MmError::new_with_trace(etype_json, trace))
    }

    /// Generate `MmJsonError` from a serialization error.
    pub fn serialization_error<E: serde::ser::Error>(e: E) -> MmJsonError {
        MmJsonError::new(SerializationError::from_error(e))
            .expect("serialization of 'SerializationError' is expected to be successful")
    }
}
