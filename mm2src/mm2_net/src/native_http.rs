//! Facilitates execution of http requests
//!
//! # Layout
//!
//! This module contains several service functions like [slurp_post_json] or [slurp_url_with_headers]
//! for executing http protocol requests, allowing you to set certain headers, make a [`Request`] in JSON or [`Body`] format.
//!
//! These methods are wrappers over [`HYPER`], which is actually `Client<HttpsConnector<HttpConnector>>` that implements
//! [`SlurpHttpClient`] trait designed to provide http capabilities through it.
//!
//! There are also facilities for constructing [SlurpError] from the [hyper::Error]
//!

use async_trait::async_trait;
use futures::channel::oneshot::Canceled;
use http::{header, HeaderValue, Request};
use hyper::client::connect::Connect;
use hyper::client::ResponseFuture;
use hyper::{Body, Client};
use serde_json::Value as Json;

use common::wio::{drive03, HYPER};
use common::{APPLICATION_JSON, X_AUTH_PAYLOAD};
use mm2_err_handle::prelude::*;

use super::transport::{GetInfoFromUriError, SlurpError, SlurpResult, SlurpResultJson};

/// Provides requesting http through it
///
/// Initially designed to be used with [hyper::Client] that could be constructed in different specific ways.
/// one of which is using with statically defined [HYPER] that is common client able to request https or https urls
/// In the other case it can be a dangerous client that does not verify self signed signature
///
/// # Examples
///
/// Request over both http or https using common [hyper_rustls::HttpsConnectorBuilder]
///
/// ```rust
/// let https = HttpsConnectorBuilder::new()
///     .with_webpki_roots()
///     .https_or_http()
///     .enable_http1()
///     .enable_http2()
///     .build();
/// let client = Client::builder().pool_max_idle_per_host(0).build(https)
/// client.slurp_url(`https://komodoproject.com`)
/// ```
///
/// Request over https with self-signed certificate
///
/// ```rust
/// let data = serde_json::to_string(&req).map_err(|error| error_anyhow!("Failed to serialize data being sent: {error}"))?;
/// match HYPER_DANGEROUS.slurp_post_json(&self.rpc_uri, data).await {
///     Err(error) => error_bail!("Failed to send json: {error}"),
///     Ok(resp) => resp.process::<OkT, ErrT>(),
/// }
///
/// mod hyper_dangerous {
///     use hyper::{client::HttpConnector, Body, Client};
///     use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
///     use lazy_static::lazy_static;
///     use rustls::client::{ServerCertVerified, ServerCertVerifier};
///     use rustls::{RootCertStore, DEFAULT_CIPHER_SUITES, DEFAULT_VERSIONS};
///     use std::sync::Arc;
///     use std::time::SystemTime;
///
///     lazy_static! {
///         pub(super) static ref HYPER_DANGEROUS: Client<HttpsConnector<HttpConnector>> = get_hyper_client_dangerous();
///     }
///
///     fn get_hyper_client_dangerous() -> Client<HttpsConnector<HttpConnector>> {
///         let mut config = rustls::ClientConfig::builder()
///             .with_cipher_suites(&DEFAULT_CIPHER_SUITES)
///             .with_safe_default_kx_groups()
///             .with_protocol_versions(&DEFAULT_VERSIONS)
///             .expect("inconsistent cipher-suite/versions selected")
///             .with_root_certificates(RootCertStore::empty())
///             .with_no_client_auth();
///
///         config
///             .dangerous()
///             .set_certificate_verifier(Arc::new(NoCertificateVerification {}));
///
///         let https_connector = HttpsConnectorBuilder::default()
///             .with_tls_config(config)
///             .https_or_http()
///             .enable_http1()
///             .build();
///
///         Client::builder().build::<_, Body>(https_connector)
///     }
///
///     struct NoCertificateVerification {}
///
///     impl ServerCertVerifier for NoCertificateVerification {
///         fn verify_server_cert(
///             &self,
///             _: &rustls::Certificate,
///             _: &[rustls::Certificate],
///             _: &rustls::ServerName,
///             _: &mut dyn Iterator<Item = &[u8]>,
///             _: &[u8],
///             _: SystemTime,
///         ) -> Result<ServerCertVerified, rustls::Error> {
///             Ok(ServerCertVerified::assertion())
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait SlurpHttpClient {
    /// Provides a [ResponseFuture] that could be spawned and processed asynchronously
    fn request(&self, req: Request<Body>) -> ResponseFuture;

    /// Executes a POST request, returning the response status, headers and body.
    async fn slurp_post_json(&self, url: &str, body: String) -> SlurpResult {
        let request = Request::builder()
            .method("POST")
            .uri(url)
            .header(header::CONTENT_TYPE, APPLICATION_JSON)
            .body(body.into())?;
        self.slurp_req(request).await
    }

    /// Executes a GET request, returning the response status, headers and body.
    async fn slurp_url(&self, url: &str) -> SlurpResult {
        let req = Request::builder().uri(url).body(Vec::new())?;
        self.slurp_req(req).await
    }

    /// Executes a GET request with additional headers.
    /// Returning the response status, headers and body.
    async fn slurp_url_with_headers(&self, url: &str, headers: Vec<(&'static str, &'static str)>) -> SlurpResult {
        let mut req = Request::builder();
        let h = req
            .headers_mut()
            .or_mm_err(|| SlurpError::Internal("An error occurred when accessing the request headers".to_string()))?;

        for (key, value) in headers {
            h.insert(key, HeaderValue::from_static(value));
        }

        let req = req.uri(url).body(Vec::new())?;
        self.slurp_req(req).await
    }

    /// Executes a Hyper request, requires [`Request<Body>`] and return the response status, headers and body as Json.
    async fn slurp_req_body(&self, request: Request<Body>) -> SlurpResultJson {
        let uri = request.uri().to_string();

        let request_f = self.request(request);
        let response = drive03(request_f)
            .await?
            .map_to_mm(|e| SlurpError::from_hyper_error(e, uri.clone()))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body_bytes = hyper::body::to_bytes(response.into_body())
            .await
            .map_to_mm(|e| SlurpError::from_hyper_error(e, uri.clone()))?;
        let body: Json = serde_json::from_slice(&body_bytes)?;
        Ok((status, headers, body))
    }

    /// Executes a Hyper request, returning the response status, headers and body.
    async fn slurp_req(&self, request: Request<Vec<u8>>) -> SlurpResult {
        let uri = request.uri().to_string();
        let (head, body) = request.into_parts();
        let request = Request::from_parts(head, Body::from(body));
        let request_f = self.request(request);
        let response = drive03(request_f)
            .await?
            .map_to_mm(|e| SlurpError::from_hyper_error(e, uri.clone()))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body();
        let output = hyper::body::to_bytes(body)
            .await
            .map_to_mm(|e| SlurpError::from_hyper_error(e, uri.clone()))?;
        Ok((status, headers, output.to_vec()))
    }
}

#[async_trait]
impl<C> SlurpHttpClient for Client<C>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    fn request(&self, req: Request<Body>) -> ResponseFuture {
        Client::<C>::request(self, req)
    }
}

/// Executes a Hyper request, returning the response status, headers and body.
pub async fn slurp_req(request: Request<Vec<u8>>) -> SlurpResult {
    HYPER.slurp_req(request).await
}

/// Executes a Hyper request, requires [`Request<Body>`] and return the response status, headers and body as Json.
pub async fn slurp_req_body(request: Request<Body>) -> SlurpResultJson {
    HYPER.slurp_req_body(request).await
}

/// Executes a GET request, returning the response status, headers and body.
pub async fn slurp_url(url: &str) -> SlurpResult {
    HYPER.slurp_url(url).await
}

/// Executes a GET request with additional headers.
/// Returning the response status, headers and body.
pub async fn slurp_url_with_headers(url: &str, headers: Vec<(&'static str, &'static str)>) -> SlurpResult {
    HYPER.slurp_url_with_headers(url, headers).await
}

/// Executes a POST request, returning the response status, headers and body.
pub async fn slurp_post_json(url: &str, body: String) -> SlurpResult {
    HYPER.slurp_post_json(url, body).await
}

impl From<Canceled> for SlurpError {
    fn from(_: Canceled) -> Self {
        SlurpError::Internal("Spawned Slurp future has been canceled".to_owned())
    }
}

impl SlurpError {
    fn from_hyper_error(e: hyper::Error, uri: String) -> SlurpError {
        let error = e.to_string();
        if e.is_parse() || e.is_parse_status() || e.is_parse_too_large() {
            SlurpError::ErrorDeserializing { uri, error }
        } else if e.is_user() {
            SlurpError::InvalidRequest(error)
        } else if e.is_timeout() {
            SlurpError::Timeout { uri, error }
        } else {
            SlurpError::Transport { uri, error }
        }
    }
}

/// `http::Error` can appear on an HTTP request [`http::Builder::build`] building.
impl From<http::Error> for SlurpError {
    fn from(e: http::Error) -> Self {
        SlurpError::InvalidRequest(e.to_string())
    }
}

/// Sends a GET request to the given URI and expects a 2xx status code in response.
///
/// # Errors
///
/// Returns an error if the HTTP status code of the response is not in the 2xx range.
pub async fn send_request_to_uri(uri: &str, auth_header: Option<&str>) -> MmResult<Json, GetInfoFromUriError> {
    let mut request_builder = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::ACCEPT, HeaderValue::from_static(APPLICATION_JSON));
    if let Some(auth_header) = auth_header {
        request_builder = request_builder.header(X_AUTH_PAYLOAD, HeaderValue::from_str(auth_header)?);
    }
    let request = request_builder.body(Body::empty())?;

    let (status, _header, body) = slurp_req_body(request).await.map_mm_err()?;
    if !status.is_success() {
        return Err(MmError::new(GetInfoFromUriError::Transport(format!(
            "Status code not in 2xx range from {uri}: {status}, {body}"
        ))));
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use crate::native_http::slurp_url;
    use common::block_on;

    #[test]
    fn test_slurp_req() {
        let (status, headers, body) = block_on(slurp_url("https://postman-echo.com/get")).unwrap();
        assert!(status.is_success(), "{status:?} {headers:?} {body:?}");
    }
}
