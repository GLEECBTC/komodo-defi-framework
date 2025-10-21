use async_trait::async_trait;
use core::convert::{TryFrom, TryInto};
use core::str::FromStr;
use cosmrs::tendermint::block::Height;
use cosmrs::tendermint::evidence::Evidence;
use cosmrs::tendermint::Genesis;
use cosmrs::tendermint::Hash;
use http::Uri;
use mm2_p2p::Keypair;
use serde::{de::DeserializeOwned, Serialize};
use std::fmt;
use std::time::Duration;
use tendermint_rpc::endpoint::validators::DEFAULT_VALIDATORS_PER_PAGE;
use tendermint_rpc::endpoint::*;
pub use tendermint_rpc::endpoint::{
    abci_query::Request as AbciRequest, health::Request as HealthRequest, tx_search::Request as TxSearchRequest,
};
use tendermint_rpc::Paging;
pub use tendermint_rpc::{query::Query as TendermintQuery, Error, Order, Scheme, SimpleRequest, Url};
use tokio::time;

/// Provides lightweight access to the Tendermint RPC. It gives access to all
/// endpoints with the exception of the event subscription-related ones.
///
/// To access event subscription capabilities, use a client that implements the
/// [`SubscriptionClient`] trait.
///
/// [`SubscriptionClient`]: trait.SubscriptionClient.html
#[async_trait]
#[allow(dead_code)]
pub trait Client {
    /// `/abci_info`: get information about the ABCI application.
    async fn abci_info(&self) -> Result<abci_info::Response, Error> {
        self.perform(abci_info::Request).await
    }

    /// `/abci_query`: query the ABCI application
    async fn abci_query<V>(
        &self,
        path: Option<String>,
        data: V,
        height: Option<Height>,
        prove: bool,
    ) -> Result<abci_query::AbciQuery, Error>
    where
        V: Into<Vec<u8>> + Send,
    {
        Ok(self
            .perform(abci_query::Request::new(path, data, height, prove))
            .await?
            .response)
    }

    /// `/block`: get block at a given height.
    async fn block<H>(&self, height: H) -> Result<block::Response, Error>
    where
        H: Into<Height> + Send,
    {
        self.perform(block::Request::new(height.into())).await
    }

    /// `/block`: get the latest block.
    async fn latest_block(&self) -> Result<block::Response, Error> {
        self.perform(block::Request::default()).await
    }

    /// `/block_results`: get ABCI results for a block at a particular height.
    async fn block_results<H>(&self, height: H) -> Result<block_results::Response, Error>
    where
        H: Into<Height> + Send,
    {
        self.perform(block_results::Request::new(height.into())).await
    }

    /// `/block_results`: get ABCI results for the latest block.
    async fn latest_block_results(&self) -> Result<block_results::Response, Error> {
        self.perform(block_results::Request::default()).await
    }

    /// `/block_search`: search for blocks by BeginBlock and EndBlock events.
    async fn block_search(
        &self,
        query: TendermintQuery,
        page: u32,
        per_page: u8,
        order: Order,
    ) -> Result<block_search::Response, Error> {
        self.perform(block_search::Request::new(query, page, per_page, order))
            .await
    }

    /// `/blockchain`: get block headers for `min` <= `height` <= `max`.
    ///
    /// Block headers are returned in descending order (highest first).
    ///
    /// Returns at most 20 items.
    async fn blockchain<H>(&self, min: H, max: H) -> Result<blockchain::Response, Error>
    where
        H: Into<Height> + Send,
    {
        // TODO(tarcieri): return errors for invalid params before making request?
        self.perform(blockchain::Request::new(min.into(), max.into())).await
    }

    /// `/broadcast_tx_async`: broadcast a transaction, returning immediately.
    async fn broadcast_tx_async(&self, tx: Vec<u8>) -> Result<broadcast::tx_async::Response, Error> {
        self.perform(broadcast::tx_async::Request::new(tx)).await
    }

    /// `/broadcast_tx_sync`: broadcast a transaction, returning the response
    /// from `CheckTx`.
    async fn broadcast_tx_sync(&self, tx: Vec<u8>) -> Result<broadcast::tx_sync::Response, Error> {
        self.perform(broadcast::tx_sync::Request::new(tx)).await
    }

    /// `/broadcast_tx_commit`: broadcast a transaction, returning the response
    /// from `DeliverTx`.
    async fn broadcast_tx_commit(&self, tx: Vec<u8>) -> Result<broadcast::tx_commit::Response, Error> {
        self.perform(broadcast::tx_commit::Request::new(tx)).await
    }

    /// `/commit`: get block commit at a given height.
    async fn commit<H>(&self, height: H) -> Result<commit::Response, Error>
    where
        H: Into<Height> + Send,
    {
        self.perform(commit::Request::new(height.into())).await
    }

    /// `/consensus_params`: get current consensus parameters at the specified
    /// height.
    async fn consensus_params<H>(&self, height: H) -> Result<consensus_params::Response, Error>
    where
        H: Into<Height> + Send,
    {
        self.perform(consensus_params::Request::new(Some(height.into()))).await
    }

    /// `/consensus_state`: get current consensus state
    async fn consensus_state(&self) -> Result<consensus_state::Response, Error> {
        self.perform(consensus_state::Request::new()).await
    }

    // TODO(thane): Simplify once validators endpoint removes pagination.
    /// `/validators`: get validators a given height.
    async fn validators<H>(&self, height: H, paging: Paging) -> Result<validators::Response, Error>
    where
        H: Into<Height> + Send,
    {
        let height = height.into();
        match paging {
            Paging::Default => self.perform(validators::Request::new(Some(height), None, None)).await,
            Paging::Specific { page_number, per_page } => {
                self.perform(validators::Request::new(
                    Some(height),
                    Some(page_number),
                    Some(per_page),
                ))
                .await
            },
            Paging::All => {
                let mut page_num = 1_usize;
                let mut validators = Vec::new();
                let per_page = DEFAULT_VALIDATORS_PER_PAGE.into();
                loop {
                    let response = self
                        .perform(validators::Request::new(
                            Some(height),
                            Some(page_num.into()),
                            Some(per_page),
                        ))
                        .await?;
                    validators.extend(response.validators);
                    if validators.len() as i32 == response.total {
                        return Ok(validators::Response::new(
                            response.block_height,
                            validators,
                            response.total,
                        ));
                    }
                    page_num += 1;
                }
            },
        }
    }

    /// `/consensus_params`: get the latest consensus parameters.
    async fn latest_consensus_params(&self) -> Result<consensus_params::Response, Error> {
        self.perform(consensus_params::Request::new(None)).await
    }

    /// `/commit`: get the latest block commit
    async fn latest_commit(&self) -> Result<commit::Response, Error> {
        self.perform(commit::Request::default()).await
    }

    /// `/health`: get node health.
    ///
    /// Returns empty result (200 OK) on success, no response in case of an error.
    async fn health(&self) -> Result<(), Error> {
        self.perform(health::Request).await?;
        Ok(())
    }

    /// `/genesis`: get genesis file.
    async fn genesis<AppState>(&self) -> Result<Genesis<AppState>, Error>
    where
        AppState: fmt::Debug + Serialize + DeserializeOwned + Send,
    {
        Ok(self.perform(genesis::Request::default()).await?.genesis)
    }

    /// `/net_info`: obtain information about P2P and other network connections.
    async fn net_info(&self) -> Result<net_info::Response, Error> {
        self.perform(net_info::Request).await
    }

    /// `/status`: get Tendermint status including node info, pubkey, latest
    /// block hash, app hash, block height and time.
    async fn status(&self) -> Result<status::Response, Error> {
        self.perform(status::Request).await
    }

    /// `/broadcast_evidence`: broadcast an evidence.
    async fn broadcast_evidence(&self, e: Evidence) -> Result<evidence::Response, Error> {
        self.perform(evidence::Request::new(e)).await
    }

    /// `/tx`: find transaction by hash.
    async fn tx(&self, hash: Hash, prove: bool) -> Result<tx::Response, Error> {
        self.perform(tx::Request::new(hash, prove)).await
    }

    /// `/tx_search`: search for transactions with their results.
    async fn tx_search(
        &self,
        query: TendermintQuery,
        prove: bool,
        page: u32,
        per_page: u8,
        order: Order,
    ) -> Result<tx_search::Response, Error> {
        self.perform(tx_search::Request::new(query, prove, page, per_page, order))
            .await
    }

    /// Poll the `/health` endpoint until it returns a successful result or
    /// the given `timeout` has elapsed.
    async fn wait_until_healthy<T>(&self, timeout: T) -> Result<(), Error>
    where
        T: Into<Duration> + Send,
    {
        let timeout = timeout.into();
        let poll_interval = Duration::from_millis(200);
        let mut attempts_remaining = timeout.as_millis() / poll_interval.as_millis();

        while self.health().await.is_err() {
            if attempts_remaining == 0 {
                return Err(Error::timeout(timeout));
            }

            attempts_remaining -= 1;
            time::sleep(poll_interval).await;
        }

        Ok(())
    }

    /// Perform a request against the RPC endpoint
    async fn perform<R>(&self, request: R) -> Result<R::Output, Error>
    where
        R: SimpleRequest;
}

/// A JSON-RPC/HTTP Tendermint RPC client (implements [`crate::Client`]).
///
/// Supports both HTTP and HTTPS connections to Tendermint RPC endpoints, and
/// allows for the use of HTTP proxies (see [`HttpClient::new_with_proxy`] for
/// details).
///
/// Does not provide [`crate::event::Event`] subscription facilities (see
/// [`crate::WebSocketClient`] for a client that does).
///
/// ## Examples
///
/// ```rust,ignore
/// use tendermint_rpc::{HttpClient, Client};
///
/// #[tokio::main]
/// async fn main() {
///     let client = HttpClient::new("http://127.0.0.1:26657")
///         .unwrap();
///
///     let abci_info = client.abci_info()
///         .await
///         .unwrap();
///
///     println!("Got ABCI info: {:?}", abci_info);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: sealed::HttpClient,
}

impl HttpClient {
    /// Construct a new Tendermint RPC HTTP/S client connecting to the given
    /// URL.
    pub fn new<U>(url: U, proxy_sign_keypair: Option<Keypair>) -> Result<Self, Error>
    where
        U: TryInto<HttpClientUrl, Error = Error>,
    {
        let url = url.try_into()?;
        Ok(Self {
            inner: if url.0.is_secure() {
                sealed::HttpClient::new_https(url.try_into()?, proxy_sign_keypair)
            } else {
                sealed::HttpClient::new_http(url.try_into()?, proxy_sign_keypair)
            },
        })
    }

    #[inline]
    pub fn uri(&self) -> Uri {
        self.inner.uri()
    }

    #[inline]
    pub fn proxy_sign_keypair(&self) -> &Option<Keypair> {
        self.inner.proxy_sign_keypair()
    }
}

#[async_trait]
impl Client for HttpClient {
    async fn perform<R>(&self, request: R) -> Result<R::Output, Error>
    where
        R: SimpleRequest,
    {
        self.inner.perform(request).await.map(From::from)
    }
}

/// A URL limited to use with HTTP clients.
///
/// Facilitates useful type conversions and inferences.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HttpClientUrl(Url);

impl TryFrom<Url> for HttpClientUrl {
    type Error = Error;

    fn try_from(value: Url) -> Result<Self, Error> {
        match value.scheme() {
            Scheme::Http | Scheme::Https => Ok(Self(value)),
            _ => Err(Error::invalid_url(value)),
        }
    }
}

impl FromStr for HttpClientUrl {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let url: Url = s.parse()?;
        url.try_into()
    }
}

impl TryFrom<&str> for HttpClientUrl {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self, Error> {
        value.parse()
    }
}

impl From<HttpClientUrl> for Url {
    fn from(url: HttpClientUrl) -> Self {
        url.0
    }
}

impl TryFrom<HttpClientUrl> for hyper::Uri {
    type Error = Error;

    fn try_from(value: HttpClientUrl) -> Result<Self, Error> {
        value
            .0
            .to_string()
            .parse()
            .map_err(|e: http::uri::InvalidUri| Error::parse(e.to_string()))
    }
}

mod sealed {
    use common::log::debug;
    use common::X_AUTH_PAYLOAD;
    use http::HeaderValue;
    use hyper::body::Buf;
    use hyper::client::connect::Connect;
    use hyper::client::HttpConnector;
    use hyper::{header, Uri};
    use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
    use mm2_p2p::Keypair;
    use proxy_signature::RawMessage;
    use std::io::Read;
    use tendermint_rpc::{Error, Response, SimpleRequest};

    fn https_connector() -> HttpsConnector<HttpConnector> {
        HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build()
    }

    /// A wrapper for a `hyper`-based client, generic over the connector type.
    #[derive(Debug, Clone)]
    pub struct HyperClient<C> {
        uri: Uri,
        inner: hyper::Client<C>,
        proxy_sign_keypair: Option<Keypair>,
    }

    impl<C> HyperClient<C> {
        pub fn new(uri: Uri, inner: hyper::Client<C>, proxy_sign_keypair: Option<Keypair>) -> Self {
            Self {
                uri,
                inner,
                proxy_sign_keypair,
            }
        }
    }

    impl<C> HyperClient<C>
    where
        C: Connect + Clone + Send + Sync + 'static,
    {
        pub async fn perform<R>(&self, request: R) -> Result<R::Response, Error>
        where
            R: SimpleRequest,
        {
            let request = self.build_request(request)?;
            let response = self
                .inner
                .request(request)
                .await
                .map_err(|e| Error::client_internal(e.to_string()))?;
            let response_body = response_to_string(response).await?;
            debug!("Incoming response: {}", response_body);
            R::Response::from_string(&response_body)
        }
    }

    impl<C> HyperClient<C> {
        /// Build a request using the given Tendermint RPC request.
        pub fn build_request<R: SimpleRequest>(&self, request: R) -> Result<hyper::Request<hyper::Body>, Error> {
            let body_bytes = request.into_json().into_bytes();
            let body_size = body_bytes.len();

            let mut request = hyper::Request::builder()
                .method("POST")
                .uri(&self.uri)
                .body(hyper::Body::from(body_bytes))
                .map_err(|e| Error::client_internal(e.to_string()))?;

            {
                let request_uri = request.uri().clone();
                let headers = request.headers_mut();
                headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(common::APPLICATION_JSON));
                headers.insert(
                    header::USER_AGENT,
                    format!("tendermint.rs/{}", env!("CARGO_PKG_VERSION")).parse().unwrap(),
                );

                if let Some(proxy_sign_keypair) = &self.proxy_sign_keypair {
                    let proxy_sign = RawMessage::sign(
                        proxy_sign_keypair,
                        &request_uri,
                        body_size,
                        common::PROXY_REQUEST_EXPIRATION_SEC,
                    )
                    .map_err(|e| Error::client_internal(e.to_string()))?;

                    let proxy_sign_serialized =
                        serde_json::to_string(&proxy_sign).map_err(|e| Error::client_internal(e.to_string()))?;

                    let header_value = HeaderValue::from_str(&proxy_sign_serialized)
                        .map_err(|e| Error::client_internal(e.to_string()))?;

                    headers.insert(X_AUTH_PAYLOAD, header_value);
                }
            }

            Ok(request)
        }
    }

    /// We offer several variations of `hyper`-based client.
    ///
    /// Here we erase the type signature of the underlying `hyper`-based
    /// client, allowing the higher-level HTTP client to operate via HTTP or
    /// HTTPS, and with or without a proxy.
    #[derive(Debug, Clone)]
    pub enum HttpClient {
        Http(HyperClient<HttpConnector>),
        Https(HyperClient<HttpsConnector<HttpConnector>>),
    }

    impl HttpClient {
        pub fn new_http(uri: Uri, proxy_sign_keypair: Option<Keypair>) -> Self {
            Self::Http(HyperClient::new(uri, hyper::Client::new(), proxy_sign_keypair))
        }

        pub fn new_https(uri: Uri, proxy_sign_keypair: Option<Keypair>) -> Self {
            Self::Https(HyperClient::new(
                uri,
                hyper::Client::builder().build(https_connector()),
                proxy_sign_keypair,
            ))
        }

        pub async fn perform<R>(&self, request: R) -> Result<R::Response, Error>
        where
            R: SimpleRequest,
        {
            match self {
                HttpClient::Http(c) => c.perform(request).await,
                HttpClient::Https(c) => c.perform(request).await,
            }
        }

        pub fn uri(&self) -> Uri {
            match self {
                HttpClient::Http(client) => client.uri.clone(),
                HttpClient::Https(client) => client.uri.clone(),
            }
        }

        pub fn proxy_sign_keypair(&self) -> &Option<Keypair> {
            match self {
                HttpClient::Http(client) => &client.proxy_sign_keypair,
                HttpClient::Https(client) => &client.proxy_sign_keypair,
            }
        }
    }

    async fn response_to_string(response: hyper::Response<hyper::Body>) -> Result<String, Error> {
        let mut response_body = String::new();
        hyper::body::aggregate(response.into_body())
            .await
            .map_err(|e| Error::client_internal(e.to_string()))?
            .reader()
            .read_to_string(&mut response_body)
            .map_err(Error::io)?;

        Ok(response_body)
    }
}
