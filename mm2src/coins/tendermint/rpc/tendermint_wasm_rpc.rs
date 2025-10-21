use common::{APPLICATION_JSON, PROXY_REQUEST_EXPIRATION_SEC, X_AUTH_PAYLOAD};
use cosmrs::tendermint::block::Height;
use derive_more::Display;
use http::header::{ACCEPT, CONTENT_TYPE};
use http::uri::InvalidUri;
use http::{StatusCode, Uri};
use mm2_net::transport::SlurpError;
use mm2_net::wasm::http::FetchRequest;
use mm2_p2p::Keypair;
use proxy_signature::RawMessage;
use std::str::FromStr;
use tendermint_rpc::endpoint::{abci_info, broadcast};
pub use tendermint_rpc::endpoint::{
    abci_query::{AbciQuery, Request as AbciRequest},
    health::Request as HealthRequest,
    tx_search::Request as TxSearchRequest,
};
use tendermint_rpc::error::Error as TendermintRpcError;
use tendermint_rpc::request::SimpleRequest;
pub use tendermint_rpc::Order;
use tendermint_rpc::Response;

#[derive(Debug, Clone)]
pub struct HttpClient {
    uri: String,
    proxy_sign_keypair: Option<Keypair>,
}

#[derive(Debug, Display)]
pub(crate) enum HttpClientInitError {
    InvalidUri(InvalidUri),
}

impl From<InvalidUri> for HttpClientInitError {
    fn from(err: InvalidUri) -> Self {
        HttpClientInitError::InvalidUri(err)
    }
}

#[derive(Debug, Display)]
pub enum PerformError {
    TendermintRpc(TendermintRpcError),
    Slurp(SlurpError),
    Internal(String),
    #[display(fmt = "Request failed with status code {status_code}, response {response}")]
    StatusCode {
        status_code: StatusCode,
        response: String,
    },
}

impl From<SlurpError> for PerformError {
    fn from(err: SlurpError) -> Self {
        PerformError::Slurp(err)
    }
}

impl From<TendermintRpcError> for PerformError {
    fn from(err: TendermintRpcError) -> Self {
        PerformError::TendermintRpc(err)
    }
}

impl HttpClient {
    pub(crate) fn new(url: &str, proxy_sign_keypair: Option<Keypair>) -> Result<Self, HttpClientInitError> {
        Uri::from_str(url)?;
        Ok(HttpClient {
            uri: url.to_owned(),
            proxy_sign_keypair,
        })
    }

    #[inline]
    pub fn uri(&self) -> http::Uri {
        Uri::from_str(&self.uri).expect("This should never happen.")
    }

    #[inline]
    pub fn proxy_sign_keypair(&self) -> &Option<Keypair> {
        &self.proxy_sign_keypair
    }

    pub(crate) async fn perform<R>(&self, request: R) -> Result<R::Output, PerformError>
    where
        R: SimpleRequest,
    {
        let body_bytes = request.into_json().into_bytes();
        let body_size = body_bytes.len();

        let mut req = FetchRequest::post(&self.uri).cors().body_bytes(body_bytes);
        req = req.header(ACCEPT.as_str(), APPLICATION_JSON);
        req = req.header(CONTENT_TYPE.as_str(), APPLICATION_JSON);

        if let Some(proxy_sign_keypair) = &self.proxy_sign_keypair {
            let proxy_sign = RawMessage::sign(proxy_sign_keypair, &self.uri(), body_size, PROXY_REQUEST_EXPIRATION_SEC)
                .map_err(|e| PerformError::Internal(e.to_string()))?;

            let proxy_sign_serialized =
                serde_json::to_string(&proxy_sign).map_err(|e| PerformError::Internal(e.to_string()))?;

            req = req.header(X_AUTH_PAYLOAD, &proxy_sign_serialized);
        }

        let (status_code, response_str) = req.request_str().await.map_err(|e| e.into_inner())?;

        if !status_code.is_success() {
            return Err(PerformError::StatusCode {
                status_code,
                response: response_str,
            });
        }
        Ok(R::Response::from_string(response_str)?.into())
    }

    /// `/abci_info`: get information about the ABCI application.
    pub async fn abci_info(&self) -> Result<abci_info::Response, PerformError> {
        self.perform(abci_info::Request).await
    }

    /// `/abci_query`: query the ABCI application
    pub async fn abci_query<V>(
        &self,
        path: Option<String>,
        data: V,
        height: Option<Height>,
        prove: bool,
    ) -> Result<AbciQuery, PerformError>
    where
        V: Into<Vec<u8>> + Send,
    {
        Ok(self
            .perform(AbciRequest::new(path, data, height, prove))
            .await?
            .response)
    }

    /// `/broadcast_tx_commit`: broadcast a transaction, returning the response
    /// from `DeliverTx`.
    pub async fn broadcast_tx_commit(&self, tx: Vec<u8>) -> Result<broadcast::tx_commit::Response, PerformError> {
        self.perform(broadcast::tx_commit::Request::new(tx)).await
    }
}

mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    async fn test_get_abci_info() {
        let client = HttpClient::new("http://34.80.202.172:26657", None).unwrap();
        client.abci_info().await.unwrap();
    }
}
