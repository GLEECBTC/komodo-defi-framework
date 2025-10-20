/// The small module implementing gRPC-WEB support
/// Implementation was taken from https://github.com/hyperium/tonic/blob/ddab65ede90f503360b7adb0d7afe6d5b7bb8b02/examples/src/grpc-web/client.rs
/// with minor refactoring
use crate::transport::SlurpError;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use common::{cfg_native, cfg_wasm32};
use derive_more::Display;
use http::header::{ACCEPT, CONTENT_TYPE};
use mm2_err_handle::prelude::*;
use prost::DecodeError;

cfg_native! {
    use common::APPLICATION_GRPC_WEB;
    use crate::transport::slurp_req;
}

cfg_wasm32! {
    use common::{X_GRPC_WEB, APPLICATION_GRPC_WEB_PROTO};
    use crate::wasm::http::FetchRequest;
}

// one byte for the compression flag plus four bytes for the length
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
const GRPC_HEADER_SIZE: usize = 5;

#[derive(Debug)]
pub enum EncodeBodyError {
    Encode(prost::EncodeError),
}

impl From<prost::EncodeError> for EncodeBodyError {
    fn from(err: prost::EncodeError) -> Self {
        EncodeBodyError::Encode(err)
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn encode_body<T>(msg: &T) -> Result<Vec<u8>, MmError<EncodeBodyError>>
where
    T: prost::Message,
{
    let mut buf = BytesMut::with_capacity(1024);

    // first skip past the header
    // cannot write it yet since we don't know the size of the
    // encoded message
    buf.put_slice(&[0; 5]);

    // write the message
    msg.encode(&mut buf)?;

    // now we know the size of encoded message and can write the
    // header
    let len = buf.len() - GRPC_HEADER_SIZE;
    {
        let mut buf = &mut buf[..GRPC_HEADER_SIZE];

        // compression flag, 0 means "no compression"
        buf.put_u8(0);

        buf.put_u32(len as u32);
    }

    Ok(buf.split_to(len + GRPC_HEADER_SIZE).freeze().to_vec())
}

#[derive(Debug)]
pub enum DecodeBodyError {
    PayloadTooShort,
    DecodeError(prost::DecodeError),
}

impl From<prost::DecodeError> for DecodeBodyError {
    fn from(err: DecodeError) -> Self {
        DecodeBodyError::DecodeError(err)
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn decode_body<T>(mut body: Bytes) -> Result<T, MmError<DecodeBodyError>>
where
    T: Default + prost::Message,
{
    if body.len() < GRPC_HEADER_SIZE {
        return MmError::err(DecodeBodyError::PayloadTooShort);
    }
    // ignore the compression flag
    body.advance(1);

    let len = body.get_u32() as usize;
    if body.len() < len {
        return MmError::err(DecodeBodyError::PayloadTooShort);
    }

    let msg = T::decode(&mut body.split_to(len))?;

    Ok(msg)
}

#[derive(Debug, thiserror::Error, Display)]
pub enum PostGrpcWebErr {
    DecodeBody(String),
    EncodeBody(String),
    InvalidRequest(String),
    BadResponse(String),
    Internal(String),
    PayloadTooShort(String),
    Status(String),
    #[display(fmt = "Transport Error — uri: {uri} — error: {error}")]
    Transport {
        uri: String,
        error: String,
    },
}

impl From<EncodeBodyError> for PostGrpcWebErr {
    fn from(err: EncodeBodyError) -> Self {
        PostGrpcWebErr::EncodeBody(format!("{err:?}"))
    }
}

impl From<DecodeBodyError> for PostGrpcWebErr {
    fn from(err: DecodeBodyError) -> Self {
        match err {
            DecodeBodyError::PayloadTooShort => PostGrpcWebErr::PayloadTooShort(format!("{err:?}")),
            DecodeBodyError::DecodeError(_) => PostGrpcWebErr::DecodeBody(format!("{err:?}")),
        }
    }
}

/// `http::Error` can appear on an HTTP request [`http::Builder::build`] building.
impl From<http::Error> for PostGrpcWebErr {
    fn from(err: http::Error) -> Self {
        PostGrpcWebErr::InvalidRequest(err.to_string())
    }
}

impl From<SlurpError> for PostGrpcWebErr {
    fn from(e: SlurpError) -> Self {
        let error = e.to_string();
        match e {
            SlurpError::ErrorDeserializing { .. } => PostGrpcWebErr::DecodeBody(error),
            SlurpError::Transport { uri, .. } | SlurpError::Timeout { uri, .. } => {
                PostGrpcWebErr::Transport { uri, error }
            },
            SlurpError::Internal(_) | SlurpError::InvalidRequest(_) => PostGrpcWebErr::Internal(error),
        }
    }
}

/// Send POST gRPC WEB HTTPS request and parse response
#[cfg(not(target_arch = "wasm32"))]
pub async fn post_grpc_web<Req, Res>(url: &str, req: &Req) -> Result<Res, MmError<PostGrpcWebErr>>
where
    Req: prost::Message + Send + 'static,
    Res: prost::Message + Default + Send + 'static,
{
    let request = http::Request::builder()
        .version(http::Version::HTTP_11)
        .method(http::Method::POST)
        .uri(url)
        .header(CONTENT_TYPE, APPLICATION_GRPC_WEB)
        .header(ACCEPT, APPLICATION_GRPC_WEB)
        .body(encode_body(req).map_mm_err()?)?;

    let response = slurp_req(request).await.map_mm_err()?;

    let reply = decode_body(response.2.into()).map_mm_err()?;

    Ok(reply)
}

#[cfg(target_arch = "wasm32")]
pub async fn post_grpc_web<Req, Res>(url: &str, req: &Req) -> Result<Res, MmError<PostGrpcWebErr>>
where
    Req: prost::Message + Send + 'static,
    Res: prost::Message + Default + Send + 'static,
{
    let body = encode_body(req).map_mm_err()?;
    let request = FetchRequest::post(url)
        .body_bytes(body)
        .header(CONTENT_TYPE.as_str(), APPLICATION_GRPC_WEB_PROTO)
        .header(ACCEPT.as_str(), APPLICATION_GRPC_WEB_PROTO)
        // https://github.com/grpc/grpc-web/issues/85#issue-217223001
        .header(X_GRPC_WEB, "1");

    let response = request.request_array().await.map_mm_err()?;

    let reply = decode_body(response.1.into()).map_mm_err()?;

    Ok(reply)
}
