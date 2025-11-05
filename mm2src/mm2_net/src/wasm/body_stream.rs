/// This module handles HTTP response decoding and trailer extraction for gRPC-Web communication/streaming.
/// # gRPC-Web Response Body Handling Module
///
/// gRPC-Web is a protocol that enables web applications to communicate with gRPC services over HTTP/1.1. It is
/// particularly useful for browsers and other environments that do not support HTTP/2. This module provides
/// essential functionality to process and decode gRPC-Web responses in MM2 also support streaming.
///
/// ## Key Components
///
/// - **EncodedBytes**: This struct represents a buffer for encoded bytes. It manages the decoding of base64-encoded data and is used to handle response data and trailers based on the content type. The `new` method initializes an instance based on the content type. Other methods are available for handling encoding and decoding of data.
///
/// - **ReadState**: An enumeration that represents the different states in which the response can be read. It keeps track of the progress of response processing, indicating whether data reading is complete or trailers have been encountered.
///
/// - **ResponseBody**: This struct is the core of response handling. It is designed to work with gRPC-Web responses. It reads response data from a ReadableStream, decodes and processes the response, and extracts trailers if present. The `new` method initializes an instance of ResponseBody based on the ReadableStream and content type. It implements the `Body` trait to provide a standardized interface for reading response data and trailers.
///
/// - **BodyStream**: A struct that represents a stream of bytes for the response body. It is used internally by ResponseBody to read the response data from a web stream. The `new` method creates a new instance based on an `IntoStream`, and the `empty` method creates an empty stream. This struct also implements the `Body` trait, providing methods to read data from the stream and return trailers.
use crate::grpc_web::PostGrpcWebErr;

use base64::prelude::*;
use bytes::{BufMut, Bytes, BytesMut};
use common::{
    APPLICATION_GRPC_WEB, APPLICATION_GRPC_WEB_PROTO, APPLICATION_GRPC_WEB_TEXT, APPLICATION_GRPC_WEB_TEXT_PROTO,
};
use futures_util::{ready, stream};
use futures_util::{stream::empty, Stream};
use http::{header::HeaderName, HeaderMap, HeaderValue};
use http_body::Body;
use httparse::{Status, EMPTY_HEADER};
use js_sys::{Object, Uint8Array};
use pin_project::pin_project;
use std::convert::TryInto;
use std::ops::{Deref, DerefMut};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{ReadableStream, ReadableStreamDefaultReader};

/// If the 8th most significant bit of a frame is `0`, it indicates data; if `1`, it indicates a trailer.
const TRAILER_BIT: u8 = 0b10000000;

/// Manages a buffer for storing response data and provides methods for appending and decoding data based on the content type.
pub struct EncodedBytes {
    is_base64: bool,
    raw_buf: BytesMut,
    buf: BytesMut,
}

impl EncodedBytes {
    /// Creates a new `EncodedBytes` instance based on the content type.
    pub fn new(content_type: &str) -> Result<Self, PostGrpcWebErr> {
        let is_base64 = match content_type {
            APPLICATION_GRPC_WEB_TEXT | APPLICATION_GRPC_WEB_TEXT_PROTO => true,
            APPLICATION_GRPC_WEB | APPLICATION_GRPC_WEB_PROTO => false,
            _ => {
                return Err(PostGrpcWebErr::InvalidRequest(format!(
                    "Unsupported Content-Type: {content_type}"
                )))
            },
        };

        Ok(Self {
            is_base64,
            raw_buf: BytesMut::new(),
            buf: BytesMut::new(),
        })
    }

    // This is to avoid passing a slice of bytes with a length that the base64
    // decoder would consider invalid.
    #[inline]
    fn max_decodable(&self) -> usize {
        (self.raw_buf.len() / 4) * 4
    }

    fn decode_base64_chunk(&mut self) -> Result<(), PostGrpcWebErr> {
        let index = self.max_decodable();

        if self.raw_buf.len() >= index {
            let decoded = BASE64_STANDARD
                .decode(self.raw_buf.split_to(index))
                .map(Bytes::from)
                .map_err(|err| PostGrpcWebErr::DecodeBody(err.to_string()))?;
            self.buf.put(decoded);
        }

        Ok(())
    }

    fn append(&mut self, bytes: Bytes) -> Result<(), PostGrpcWebErr> {
        if self.is_base64 {
            self.raw_buf.put(bytes);
            self.decode_base64_chunk()?;
        } else {
            self.buf.put(bytes)
        }

        Ok(())
    }

    fn take(&mut self, length: usize) -> BytesMut {
        let new_buf = self.buf.split_off(length);
        std::mem::replace(&mut self.buf, new_buf)
    }
}

impl Deref for EncodedBytes {
    type Target = BytesMut;

    fn deref(&self) -> &Self::Target {
        &self.buf
    }
}

impl DerefMut for EncodedBytes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buf
    }
}

/// Represents the state of reading the response body, including compression flags, data lengths, trailers, and the done state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadState {
    CompressionFlag,
    DataLength,
    Data(u32),
    TrailerLength,
    Trailer(u32),
    Done,
}

impl ReadState {
    fn is_done(&self) -> bool {
        matches!(self, ReadState::Done)
    }

    fn finished_data(&self) -> bool {
        matches!(self, ReadState::TrailerLength)
            || matches!(self, ReadState::Trailer(_))
            || matches!(self, ReadState::Done)
    }
}

/// Handles the HTTP response body, decoding data, and extracting trailers
#[pin_project]
pub struct ResponseBody {
    #[pin]
    body_stream: BodyStream,
    buf: EncodedBytes,
    incomplete_data: BytesMut,
    data: Option<BytesMut>,
    trailer: Option<HeaderMap>,
    state: ReadState,
    finished_stream: bool,
}

impl ResponseBody {
    /// Creates a new `ResponseBody` based on a ReadableStream and content type.
    pub(crate) async fn new(body_stream: ReadableStream, content_type: &str) -> Result<Self, PostGrpcWebErr> {
        let body_stream: ReadableStreamDefaultReader = body_stream
            .get_reader()
            .dyn_into()
            .map_err(|err| PostGrpcWebErr::BadResponse(format!("{err:?}")))?;

        Ok(Self {
            body_stream: BodyStream::new(body_stream).await?,
            buf: EncodedBytes::new(content_type)?,
            incomplete_data: BytesMut::new(),
            data: None,
            trailer: None,
            state: ReadState::CompressionFlag,
            finished_stream: false,
        })
    }

    fn read_stream(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), PostGrpcWebErr>> {
        if self.finished_stream {
            return Poll::Ready(Ok(()));
        }

        let this = self.project();

        match ready!(this.body_stream.poll_data(cx)) {
            Some(Ok(data)) => {
                if let Err(e) = this.buf.append(data) {
                    return Poll::Ready(Err(e));
                }

                Poll::Ready(Ok(()))
            },
            Some(Err(e)) => Poll::Ready(Err(e)),
            None => {
                *this.finished_stream = true;
                Poll::Ready(Ok(()))
            },
        }
    }

    fn step(self: Pin<&mut Self>) -> Result<(), PostGrpcWebErr> {
        let this = self.project();

        loop {
            match this.state {
                ReadState::CompressionFlag => {
                    if this.buf.is_empty() {
                        // Can't read compression flag right now
                        return Ok(());
                    };

                    let compression_flag = this.buf.take(1);
                    if compression_flag[0] & TRAILER_BIT == 0 {
                        this.incomplete_data.unsplit(compression_flag);
                        *this.state = ReadState::DataLength;
                    } else {
                        *this.state = ReadState::TrailerLength;
                    }
                },
                ReadState::DataLength => {
                    if this.buf.len() < 4 {
                        // Can't read data length right now
                        return Ok(());
                    };

                    let data_length_bytes = this.buf.take(4);
                    let data_length = u32::from_be_bytes(data_length_bytes.to_vec().try_into().unwrap());

                    this.incomplete_data.extend_from_slice(&data_length_bytes);
                    *this.state = ReadState::Data(data_length);
                },
                ReadState::Data(data_length) => {
                    let data_length = *data_length as usize;
                    if this.buf.len() < data_length {
                        // Can't read data right now
                        return Ok(());
                    };

                    this.incomplete_data.unsplit(this.buf.take(data_length));

                    let new_data = this.incomplete_data.split();
                    if let Some(data) = this.data {
                        data.unsplit(new_data);
                    } else {
                        *this.data = Some(new_data);
                    }

                    *this.state = ReadState::CompressionFlag;
                },
                ReadState::TrailerLength => {
                    if this.buf.len() < 4 {
                        // Can't read data length right now
                        return Ok(());
                    };

                    *this.state = ReadState::Trailer(u32::from_be_bytes(this.buf.take(4).to_vec().try_into().unwrap()));
                },
                ReadState::Trailer(trailer_length) => {
                    let trailer_length = *trailer_length as usize;
                    if this.buf.len() < trailer_length {
                        // Can't read trailer right now
                        return Ok(());
                    };

                    let mut trailer_bytes = this.buf.take(trailer_length);
                    trailer_bytes.put_u8(b'\n');

                    *this.trailer = Some(Self::parse_trailer(&trailer_bytes)?);
                    *this.state = ReadState::Done;
                },
                ReadState::Done => return Ok(()),
            }
        }
    }

    fn parse_trailer(trailer_bytes: &[u8]) -> Result<HeaderMap, PostGrpcWebErr> {
        let mut trailers_buf = [EMPTY_HEADER; 64];
        let parsed_trailers = match httparse::parse_headers(trailer_bytes, &mut trailers_buf)
            .map_err(|err| PostGrpcWebErr::InvalidRequest(err.to_string()))?
        {
            Status::Complete((_, headers)) => Ok(headers),
            Status::Partial => Err(PostGrpcWebErr::InvalidRequest(
                "parse header not completed!".to_string(),
            )),
        }?;

        let mut trailers = HeaderMap::with_capacity(parsed_trailers.len());

        for parsed_trailer in parsed_trailers {
            let header_name = HeaderName::from_bytes(parsed_trailer.name.as_bytes())
                .map_err(|err| PostGrpcWebErr::InvalidRequest(err.to_string()))?;
            let header_value = HeaderValue::from_bytes(parsed_trailer.value)
                .map_err(|err| PostGrpcWebErr::InvalidRequest(err.to_string()))?;
            trailers.insert(header_name, header_value);
        }

        Ok(trailers)
    }
}

impl Body for ResponseBody {
    type Data = Bytes;

    type Error = PostGrpcWebErr;

    fn poll_data(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        // If reading data is finished return `None`
        if self.state.finished_data() {
            return Poll::Ready(self.data.take().map(|d| Ok(d.freeze())));
        }

        loop {
            // Read bytes from stream
            if let Err(e) = ready!(self.as_mut().read_stream(cx)) {
                return Poll::Ready(Some(Err(e)));
            }

            // Step the state machine
            if let Err(e) = self.as_mut().step() {
                return Poll::Ready(Some(Err(e)));
            }

            if self.state.finished_data() {
                // If we finished reading data continue return `None`
                return Poll::Ready(self.data.take().map(|d| Ok(d.freeze())));
            } else if self.finished_stream {
                // If stream is finished but data is not finished return error
                return Poll::Ready(Some(Err(PostGrpcWebErr::InvalidRequest("Bad response".to_string()))));
            }
        }
    }

    fn poll_trailers(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<Option<HeaderMap>, Self::Error>> {
        // If the state machine is complete, return trailer
        if self.state.is_done() {
            return Poll::Ready(Ok(self.trailer.take()));
        }

        loop {
            // Read bytes from stream
            if let Err(e) = ready!(self.as_mut().read_stream(cx)) {
                return Poll::Ready(Err(e));
            }

            // Step the state machine
            if let Err(e) = self.as_mut().step() {
                return Poll::Ready(Err(e));
            }

            if self.state.is_done() {
                // If state machine is done, return trailer
                return Poll::Ready(Ok(self.trailer.take()));
            } else if self.finished_stream {
                // If stream is finished but state machine is not done, return error
                return Poll::Ready(Err(PostGrpcWebErr::InvalidRequest("Bad response".to_string())));
            }
        }
    }
}

/// Represents a stream of bytes for the response body.
pub struct BodyStream {
    body_stream: Pin<Box<dyn Stream<Item = Result<Bytes, PostGrpcWebErr>>>>,
}

impl BodyStream {
    /// Creates a new `BodyStream` based on an `ReadableStreamDefaultReader`.
    pub async fn new(body_stream: ReadableStreamDefaultReader) -> Result<Self, PostGrpcWebErr> {
        let mut chunks = vec![];
        loop {
            let value = JsFuture::from(body_stream.read())
                .await
                .map_err(|err| PostGrpcWebErr::InvalidRequest(format!("{err:?}")))?;
            let object: Object = value
                .dyn_into()
                .map_err(|err| PostGrpcWebErr::BadResponse(format!("{err:?}")))?;
            let object_value = js_sys::Reflect::get(&object, &JsValue::from_str("value"))
                .map_err(|err| PostGrpcWebErr::BadResponse(format!("{err:?}")))?;
            let object_progress = js_sys::Reflect::get(&object, &JsValue::from_str("done"))
                .map_err(|err| PostGrpcWebErr::BadResponse(format!("{err:?}")))?;
            let chunk = Uint8Array::new(&object_value).to_vec();
            chunks.extend_from_slice(&chunk);

            if object_progress.as_bool().ok_or_else(|| {
                PostGrpcWebErr::BadResponse("Expected done(bool) field in json object response".to_string())
            })? {
                break;
            }
        }

        Ok(Self {
            body_stream: Box::pin(stream::once(async { Ok(Bytes::from(chunks)) })),
        })
    }

    /// Creates an empty `BodyStream`.
    pub fn empty() -> Self {
        let body_stream = empty();

        Self {
            body_stream: Box::pin(body_stream),
        }
    }
}

// Implementations of the Body trait for ResponseBody and BodyStream.
impl Body for BodyStream {
    type Data = Bytes;

    type Error = PostGrpcWebErr;

    fn poll_data(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Self::Data, Self::Error>>> {
        self.body_stream.as_mut().poll_next(cx)
    }

    fn poll_trailers(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<Option<http::HeaderMap>, Self::Error>> {
        Poll::Ready(Ok(None))
    }
}

// Additional safety traits for BodyStream.
unsafe impl Send for BodyStream {}
// Additional safety traits for BodyStream.
unsafe impl Sync for BodyStream {}
