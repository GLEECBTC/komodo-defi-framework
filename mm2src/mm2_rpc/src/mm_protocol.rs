use common::{HttpStatusCode, SerializationError};
use http::{Response, StatusCode};
use mm2_err_handle::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json as json;
use serde_json::Value as Json;

/// Please note there is no standardized `1.0` version, so this enumeration should not be used in the legacy protocol context.
#[derive(Clone, Copy, Deserialize, Serialize)]
pub enum MmRpcVersion {
    #[serde(rename = "2.0")]
    V2,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmRpcRequest {
    pub mmrpc: MmRpcVersion,
    pub userpass: Option<String>,
    pub method: String,
    #[serde(default)]
    pub params: Json,
    pub id: Option<usize>,
}

pub struct MmRpcBuilder<T: Serialize, E: SerMmErrorType> {
    version: MmRpcVersion,
    result: MmRpcResult<T, E>,
    id: Option<usize>,
}

impl<T: Serialize, E: SerMmErrorType> MmRpcBuilder<T, E> {
    pub fn ok(r: T) -> Self {
        MmRpcBuilder {
            version: MmRpcVersion::V2,
            result: MmRpcResult::Ok { result: r },
            id: None,
        }
    }

    pub fn err(e: MmError<E>) -> Self {
        MmRpcBuilder {
            version: MmRpcVersion::V2,
            result: MmRpcResult::Err(e),
            id: None,
        }
    }

    pub fn from_result(result: Result<T, MmError<E>>) -> Self {
        match result {
            Ok(r) => Self::ok(r),
            Err(e) => Self::err(e),
        }
    }

    pub fn id(mut self, id: Option<usize>) -> Self {
        self.id = id;
        self
    }

    pub fn version(mut self, version: MmRpcVersion) -> Self {
        self.version = version;
        self
    }

    pub fn build(self) -> MmRpcResponse<T, E> {
        MmRpcResponse {
            mmrpc: self.version,
            result: self.result,
            id: self.id,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum MmRpcResult<T: Serialize, E: SerMmErrorType> {
    Ok { result: T },
    Err(MmError<E>),
}

impl<T, E> HttpStatusCode for MmRpcResult<T, E>
where
    T: Serialize,
    E: HttpStatusCode + SerMmErrorType,
{
    fn status_code(&self) -> StatusCode {
        match self {
            MmRpcResult::Ok { .. } => StatusCode::OK,
            MmRpcResult::Err(e) => e.status_code(),
        }
    }
}

impl<T: Serialize, E: SerMmErrorType> MmRpcResult<T, E> {
    pub fn ok(result: T) -> MmRpcResult<T, E> {
        MmRpcResult::Ok { result }
    }

    #[track_caller]
    pub fn mm_err(error: E) -> MmRpcResult<T, E> {
        MmRpcResult::Err(MmError::new(error))
    }
}

impl<T, E1> MmRpcResult<T, E1>
where
    T: Serialize,
    E1: SerMmErrorType,
{
    #[track_caller]
    pub fn map_err<E2, F>(self, f: F) -> MmRpcResult<T, E2>
    where
        F: FnOnce(E1) -> E2,
        E2: SerMmErrorType,
    {
        match self {
            MmRpcResult::Ok { result } => MmRpcResult::Ok { result },
            MmRpcResult::Err(mm_e1) => MmRpcResult::Err(mm_e1.map(f)),
        }
    }
}

#[derive(Serialize)]
pub struct MmRpcResponse<T: Serialize, E: SerMmErrorType> {
    mmrpc: MmRpcVersion,
    /// `MmRpcResult` will be flattened into `result` or `error, error_path, error_trace, error_type, error_data` fields.
    #[serde(flatten)]
    result: MmRpcResult<T, E>,
    id: Option<usize>,
}

impl<T: Serialize, E: SerMmErrorType> MmRpcResponse<T, E> {
    #[allow(dead_code)]
    pub fn serialize_json(&self) -> Json {
        match json::to_value(self) {
            Ok(encoded) => encoded,
            Err(e) => self.error_to_json(e),
        }
    }

    fn error_to_json(&self, error: impl serde::ser::Error) -> Json {
        let response: MmRpcResponse<(), _> = MmRpcResponse {
            mmrpc: self.mmrpc,
            result: MmRpcResult::Err(MmError::new(SerializationError::InternalError(error.to_string()))),
            id: self.id,
        };
        serde_json::to_value(response)
            .expect("serialization of `MmRpcResponse<(), SerializationError>` is expected to be successful")
    }
}

impl<T: Serialize, E> MmRpcResponse<T, E>
where
    E: SerMmErrorType + HttpStatusCode,
{
    pub fn serialize_http_response(&self) -> Response<Vec<u8>> {
        let status_code = self.result.status_code();
        let encoded = match json::to_vec(self) {
            Ok(encoded) => encoded,
            Err(e) => self.error_to_json(e).to_string().into_bytes(),
        };
        // [`ResponseBuilder::body`] has not to fail, because there is only one configuration - [`ResponseBuilder::status`].
        // [`ResponseBuilder::status`] may fail only if [`StatusCode::try_from`] fails, but we pass already constructed `StatusCode`.
        Response::builder()
            .status(status_code)
            .body(encoded)
            .expect("ResponseBuilder::body has not to fail")
    }
}

#[cfg(test)]
mod tests {
    use derive_more::Display;
    use mm2_err_handle::prelude::*;
    use ser_error_derive::SerializeErrorType;
    use serde::{Serialize, Serializer};
    use serde_json as json;
    use serde_json::{json, Value as Json};

    use crate::mm_protocol::{MmRpcBuilder, MmRpcResponse};

    #[derive(Display, Serialize, SerializeErrorType)]
    #[serde(tag = "error_type", content = "error_data")]
    enum AnError {
        #[display(fmt = "Not sufficient balance. Top up your balance by {missing}")]
        NotSufficientBalance { missing: u64 },
    }

    #[test]
    fn test_mm_rpc_response_serialize() {
        let ok: MmRpcResponse<_, AnError> = MmRpcBuilder::ok(vec![1, 2, 3]).build();
        let actual = json::to_value(ok).expect("Couldn't serialize MmRpcResponse");
        let expected = json!({
            "mmrpc": "2.0",
            "result": [1, 2, 3],
            "id": Json::Null,
        });
        assert_eq!(actual, expected);

        let ok_with_id: MmRpcResponse<_, AnError> = MmRpcBuilder::ok(vec![1, 2, 3]).id(Some(2)).build();
        let actual = json::to_value(ok_with_id).expect("Couldn't serialize MmRpcResponse");
        let expected = json!({
            "mmrpc": "2.0",
            "result": [1, 2, 3],
            "id": 2,
        });
        assert_eq!(actual, expected);

        let error_type = AnError::NotSufficientBalance { missing: 123 };
        let err_line = line!() + 1;
        let err: MmRpcResponse<String, _> = MmRpcBuilder::err(MmError::new(error_type)).build();
        let actual = json::to_value(err).expect("Couldn't serialize MmRpcResponse");
        let expected = json!({
            "mmrpc": "2.0",
            "error": "Not sufficient balance. Top up your balance by 123",
            "error_path": "mm_protocol",
            "error_trace": format!("mm_protocol:{}]", err_line),
            "error_type": "NotSufficientBalance",
            "error_data": {
                "missing": 123,
            },
            "id": Json::Null,
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_serialization_error() {
        /// An invalid error should cause the serialization error because this enum is not tagged.
        #[derive(Display)]
        enum InvalidError {
            NotSufficientBalance { missing: u64 },
        }

        impl Serialize for InvalidError {
            fn serialize<S>(&self, _serializer: S) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error>
            where
                S: Serializer,
            {
                Err(serde::ser::Error::custom("An expected error"))
            }
        }

        /// Please not [`ser_error::__private::SerializeErrorTypeImpl`] must not be implemented manually outside tests.
        impl ser_error::__private::SerializeErrorTypeImpl for InvalidError {}

        let response: MmRpcResponse<(), _> =
            MmRpcBuilder::err(MmError::new(InvalidError::NotSufficientBalance { missing: 0 })).build();
        let value = response.serialize_json();
        assert!(value["error"]
            .as_str()
            .expect("Expected 'error' field")
            .contains("Internal error: Couldn't serialize an RPC response: An expected error"));
        assert_eq!(value["error_type"].as_str(), Some("InternalError"));
    }
}
