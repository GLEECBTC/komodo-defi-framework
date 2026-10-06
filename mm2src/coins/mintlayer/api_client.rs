use crate::mintlayer::{
    MintlayerAddressInfo, MintlayerAmount, MintlayerChainTip, MintlayerFeeRate, MintlayerGenesisInfo,
    MintlayerTransactionInfo, MintlayerTransactionOutput, MintlayerUtxo,
};
use async_std::prelude::FutureExt;
use async_trait::async_trait;
use compatible_time::Duration;
use http::StatusCode;
#[cfg(not(target_arch = "wasm32"))]
use mm2_net::transport::slurp_req;
use mm2_net::transport::slurp_url;
#[cfg(target_arch = "wasm32")]
use mm2_net::wasm::http::FetchRequest;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use thiserror::Error;
use url::Url;

const MINTLAYER_API_VERSION_PATH: &[&str] = &["api", "v2"];
const MINTLAYER_API_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERROR_BODY_PREVIEW: usize = 512;

#[derive(Clone, Debug, Error, PartialEq)]
pub enum MintlayerEndpointError {
    #[error("Request timed out")]
    Timeout,
    #[error("Transport error: {0}")]
    Transport(String),
    #[error("HTTP status {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("Invalid JSON response: {0}")]
    InvalidResponse(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MintlayerEndpointFailure {
    pub endpoint: String,
    pub error: MintlayerEndpointError,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum MintlayerApiError {
    #[error("Mintlayer API path segment cannot be empty")]
    EmptyPathSegment,
    #[error("Cannot construct Mintlayer API URL from '{base_url}'")]
    EndpointUrlConstruction { base_url: String },
    #[error("Mintlayer signed transaction hex is empty or invalid")]
    InvalidSignedTransactionHex,
    #[error("Mintlayer transaction submission rejected by '{endpoint}' with HTTP {status}: {body}")]
    SubmissionRejected {
        endpoint: String,
        status: u16,
        body: String,
    },
    #[error("Mintlayer transaction submission outcome at '{endpoint}' is unknown for txid '{expected}': {reason}")]
    SubmissionOutcomeUnknown {
        endpoint: String,
        expected: String,
        reason: String,
    },
    #[error("Mintlayer transaction submission at '{endpoint}' returned txid '{actual}' but expected '{expected}'")]
    SubmissionTxIdMismatch {
        endpoint: String,
        expected: String,
        actual: String,
    },
    #[error("All Mintlayer API endpoints failed: {failures:?}")]
    AllEndpointsFailed { failures: Vec<MintlayerEndpointFailure> },
}

impl MintlayerApiError {
    pub(crate) fn is_transaction_not_found(&self) -> bool {
        let Self::AllEndpointsFailed { failures } = self else {
            return false;
        };

        !failures.is_empty()
            && failures.iter().all(|failure| {
                let MintlayerEndpointError::HttpStatus { status, body } = &failure.error else {
                    return false;
                };

                *status == StatusCode::NOT_FOUND.as_u16()
                    && serde_json::from_str::<serde_json::Value>(body)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("error")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        })
                        .as_deref()
                        == Some("Transaction not found")
            })
    }
}

#[async_trait]
pub trait MintlayerHttpTransport: Send + Sync + 'static {
    async fn get(&self, url: &str) -> Result<(StatusCode, Vec<u8>), String>;
    async fn post(&self, url: &str, body: &str) -> Result<(StatusCode, Vec<u8>), String>;
}

#[derive(Debug, Default)]
pub struct KdfMintlayerHttpTransport;

#[async_trait]
impl MintlayerHttpTransport for KdfMintlayerHttpTransport {
    async fn get(&self, url: &str) -> Result<(StatusCode, Vec<u8>), String> {
        slurp_url(url)
            .await
            .map(|(status, _headers, body)| (status, body))
            .map_err(|error| error.into_inner().to_string())
    }

    async fn post(&self, url: &str, body: &str) -> Result<(StatusCode, Vec<u8>), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let request = http::Request::builder()
                .method(http::Method::POST)
                .uri(url)
                .header(http::header::CONTENT_TYPE, "text/plain")
                .body(body.as_bytes().to_vec())
                .map_err(|error| error.to_string())?;

            return slurp_req(request)
                .await
                .map(|(status, _headers, body)| (status, body))
                .map_err(|error| error.into_inner().to_string());
        }

        #[cfg(target_arch = "wasm32")]
        {
            FetchRequest::post(url)
                .header(http::header::CONTENT_TYPE.as_str(), "text/plain")
                .body_utf8(body.to_owned())
                .request_str()
                .await
                .map(|(status, body)| (status, body.into_bytes()))
                .map_err(|error| error.into_inner().to_string())
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct MintlayerBlockHeightInfo {
    height: u64,
}

#[derive(Debug, serde::Deserialize)]
struct MintlayerSubmitTransactionResponse {
    tx_id: String,
}

#[derive(Debug)]
pub struct MintlayerApiClientGeneric<T> {
    api_urls: Vec<Url>,
    transport: Arc<T>,
    timeout: Duration,
}

pub type MintlayerApiClient = MintlayerApiClientGeneric<KdfMintlayerHttpTransport>;

impl<T> Clone for MintlayerApiClientGeneric<T> {
    fn clone(&self) -> Self {
        MintlayerApiClientGeneric {
            api_urls: self.api_urls.clone(),
            transport: Arc::clone(&self.transport),
            timeout: self.timeout,
        }
    }
}

impl MintlayerApiClient {
    pub fn new(api_urls: Vec<Url>) -> Self {
        MintlayerApiClientGeneric {
            api_urls,
            transport: Arc::new(KdfMintlayerHttpTransport),
            timeout: MINTLAYER_API_TIMEOUT,
        }
    }
}

impl<T> MintlayerApiClientGeneric<T>
where
    T: MintlayerHttpTransport,
{
    #[cfg(test)]
    fn with_transport(api_urls: Vec<Url>, transport: Arc<T>, timeout: Duration) -> Self {
        MintlayerApiClientGeneric {
            api_urls,
            transport,
            timeout,
        }
    }

    pub fn api_urls(&self) -> &[Url] {
        &self.api_urls
    }

    pub async fn genesis(&self) -> Result<MintlayerGenesisInfo, MintlayerApiError> {
        self.get_json(&["chain", "genesis"]).await
    }

    pub async fn chain_tip(&self) -> Result<MintlayerChainTip, MintlayerApiError> {
        self.get_json(&["chain", "tip"]).await
    }

    pub async fn block_height(&self, block_id: &str) -> Result<u64, MintlayerApiError> {
        validate_path_segment(block_id)?;
        let block: MintlayerBlockHeightInfo = self.get_json(&["block", block_id]).await?;
        Ok(block.height)
    }

    pub async fn main_chain_block_id(&self, block_height: u64) -> Result<String, MintlayerApiError> {
        let block_height = block_height.to_string();
        self.get_json(&["chain", &block_height]).await
    }

    pub async fn block_height_in_main_chain(&self, block_id: &str) -> Result<Option<u64>, MintlayerApiError> {
        let block_height = self.block_height(block_id).await?;
        let main_chain_block_id = self.main_chain_block_id(block_height).await?;

        if main_chain_block_id.eq_ignore_ascii_case(block_id) {
            Ok(Some(block_height))
        } else {
            Ok(None)
        }
    }

    pub async fn fee_rate(&self) -> Result<MintlayerFeeRate, MintlayerApiError> {
        self.get_json(&["feerate"]).await
    }

    pub async fn address_info(&self, address: &str) -> Result<MintlayerAddressInfo, MintlayerApiError> {
        validate_path_segment(address)?;
        match self.get_json(&["address", address]).await {
            Err(error) if is_address_not_found(&error) => Ok(empty_address_info()),
            result => result,
        }
    }

    pub async fn spendable_utxos(&self, address: &str) -> Result<Vec<MintlayerUtxo>, MintlayerApiError> {
        validate_path_segment(address)?;
        self.get_json(&["address", address, "spendable-utxos"]).await
    }

    pub async fn submit_transaction(
        &self,
        signed_transaction_hex: &str,
        expected_transaction_id: &str,
    ) -> Result<String, MintlayerApiError> {
        validate_path_segment(expected_transaction_id)?;

        if signed_transaction_hex.is_empty()
            || !signed_transaction_hex.len().is_multiple_of(2)
            || !signed_transaction_hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(MintlayerApiError::InvalidSignedTransactionHex);
        }

        let mut failures = Vec::with_capacity(self.api_urls.len());

        for base_url in &self.api_urls {
            let endpoint = build_endpoint_url(base_url, &["transaction"])?;
            let endpoint_string = endpoint.to_string();
            let response = self
                .transport
                .post(endpoint.as_str(), signed_transaction_hex)
                .timeout(self.timeout)
                .await;

            let (status, body) = match response {
                Err(_) => {
                    return Err(MintlayerApiError::SubmissionOutcomeUnknown {
                        endpoint: endpoint_string,
                        expected: expected_transaction_id.to_owned(),
                        reason: "request timed out after dispatch; the endpoint may have accepted the transaction"
                            .to_owned(),
                    });
                },
                Ok(Err(error)) => {
                    return Err(MintlayerApiError::SubmissionOutcomeUnknown {
                        endpoint: endpoint_string,
                        expected: expected_transaction_id.to_owned(),
                        reason: format!(
                            "transport error after dispatch; the endpoint may have accepted the transaction: {error}"
                        ),
                    });
                },
                Ok(Ok(response)) => response,
            };

            if !status.is_success() {
                let body_preview = response_body_preview(&body);

                if status == StatusCode::FORBIDDEN
                    || status == StatusCode::NOT_FOUND
                    || status == StatusCode::METHOD_NOT_ALLOWED
                    || status == StatusCode::TOO_MANY_REQUESTS
                    || status == StatusCode::NOT_IMPLEMENTED
                {
                    failures.push(MintlayerEndpointFailure {
                        endpoint: endpoint_string,
                        error: MintlayerEndpointError::HttpStatus {
                            status: status.as_u16(),
                            body: body_preview,
                        },
                    });
                    continue;
                }

                if status.is_server_error() {
                    return Err(MintlayerApiError::SubmissionOutcomeUnknown {
                        endpoint: endpoint_string,
                        expected: expected_transaction_id.to_owned(),
                        reason: format!(
                            "HTTP {} after submission attempt; the endpoint may have accepted the transaction: {}",
                            status.as_u16(),
                            body_preview
                        ),
                    });
                }

                return Err(MintlayerApiError::SubmissionRejected {
                    endpoint: endpoint_string,
                    status: status.as_u16(),
                    body: body_preview,
                });
            }

            let response = serde_json::from_slice::<MintlayerSubmitTransactionResponse>(&body).map_err(|error| {
                MintlayerApiError::SubmissionOutcomeUnknown {
                    endpoint: endpoint_string.clone(),
                    expected: expected_transaction_id.to_owned(),
                    reason: format!(
                        "successful HTTP response could not be parsed; the transaction may have been accepted: {}; body: {}",
                        error,
                        response_body_preview(&body)
                    ),
                }
            })?;

            if response.tx_id != expected_transaction_id {
                return Err(MintlayerApiError::SubmissionTxIdMismatch {
                    endpoint: endpoint_string,
                    expected: expected_transaction_id.to_owned(),
                    actual: response.tx_id,
                });
            }

            return Ok(response.tx_id);
        }

        Err(MintlayerApiError::AllEndpointsFailed { failures })
    }

    pub async fn transaction(&self, transaction_id: &str) -> Result<MintlayerTransactionInfo, MintlayerApiError> {
        validate_path_segment(transaction_id)?;
        self.get_json(&["transaction", transaction_id]).await
    }

    /// Fetches a transaction from an API endpoint that exposes the canonical
    /// encoded SignedTransaction in the additive `tx_hex` field.
    ///
    /// A successful HTTP/JSON response without `tx_hex` is treated as an
    /// endpoint capability miss and failover continues to the next configured
    /// API endpoint.
    pub async fn transaction_with_tx_hex(
        &self,
        transaction_id: &str,
    ) -> Result<MintlayerTransactionInfo, MintlayerApiError> {
        validate_path_segment(transaction_id)?;

        let endpoint_path = ["transaction", transaction_id];
        let mut failures = Vec::with_capacity(self.api_urls.len());

        for base_url in &self.api_urls {
            let endpoint = build_endpoint_url(base_url, &endpoint_path)?;
            let endpoint_string = endpoint.to_string();
            let response = self.transport.get(endpoint.as_str()).timeout(self.timeout).await;

            let (status, body) = match response {
                Err(_) => {
                    failures.push(MintlayerEndpointFailure {
                        endpoint: endpoint_string,
                        error: MintlayerEndpointError::Timeout,
                    });
                    continue;
                },
                Ok(Err(error)) => {
                    failures.push(MintlayerEndpointFailure {
                        endpoint: endpoint_string,
                        error: MintlayerEndpointError::Transport(error),
                    });
                    continue;
                },
                Ok(Ok(response)) => response,
            };

            if !status.is_success() {
                failures.push(MintlayerEndpointFailure {
                    endpoint: endpoint_string,
                    error: MintlayerEndpointError::HttpStatus {
                        status: status.as_u16(),
                        body: response_body_preview(&body),
                    },
                });
                continue;
            }

            match serde_json::from_slice::<MintlayerTransactionInfo>(&body) {
                Ok(response) if response.tx_hex.as_ref().is_some_and(|tx_hex| !tx_hex.is_empty()) => {
                    return Ok(response);
                },
                Ok(_) => failures.push(MintlayerEndpointFailure {
                    endpoint: endpoint_string,
                    error: MintlayerEndpointError::InvalidResponse(
                        "Mintlayer transaction response does not include a non-empty tx_hex field".to_owned(),
                    ),
                }),
                Err(error) => failures.push(MintlayerEndpointFailure {
                    endpoint: endpoint_string,
                    error: MintlayerEndpointError::InvalidResponse(error.to_string()),
                }),
            }
        }

        Err(MintlayerApiError::AllEndpointsFailed { failures })
    }

    pub async fn transaction_output(
        &self,
        transaction_id: &str,
        output_index: u32,
    ) -> Result<MintlayerTransactionOutput, MintlayerApiError> {
        validate_path_segment(transaction_id)?;
        let output_index = output_index.to_string();
        self.get_json(&["transaction", transaction_id, "output", &output_index])
            .await
    }

    async fn get_json<R>(&self, endpoint_path: &[&str]) -> Result<R, MintlayerApiError>
    where
        R: DeserializeOwned,
    {
        for segment in endpoint_path {
            validate_path_segment(segment)?;
        }

        let mut failures = Vec::with_capacity(self.api_urls.len());

        for base_url in &self.api_urls {
            let endpoint = build_endpoint_url(base_url, endpoint_path)?;
            let endpoint_string = endpoint.to_string();

            let response = self.transport.get(endpoint.as_str()).timeout(self.timeout).await;

            let (status, body) = match response {
                Err(_) => {
                    failures.push(MintlayerEndpointFailure {
                        endpoint: endpoint_string,
                        error: MintlayerEndpointError::Timeout,
                    });
                    continue;
                },
                Ok(Err(error)) => {
                    failures.push(MintlayerEndpointFailure {
                        endpoint: endpoint_string,
                        error: MintlayerEndpointError::Transport(error),
                    });
                    continue;
                },
                Ok(Ok(response)) => response,
            };

            if !status.is_success() {
                failures.push(MintlayerEndpointFailure {
                    endpoint: endpoint_string,
                    error: MintlayerEndpointError::HttpStatus {
                        status: status.as_u16(),
                        body: response_body_preview(&body),
                    },
                });
                continue;
            }

            match serde_json::from_slice(&body) {
                Ok(response) => return Ok(response),
                Err(error) => failures.push(MintlayerEndpointFailure {
                    endpoint: endpoint_string,
                    error: MintlayerEndpointError::InvalidResponse(error.to_string()),
                }),
            }
        }

        Err(MintlayerApiError::AllEndpointsFailed { failures })
    }
}

fn is_address_not_found(error: &MintlayerApiError) -> bool {
    let MintlayerApiError::AllEndpointsFailed { failures } = error else {
        return false;
    };

    !failures.is_empty()
        && failures.iter().all(|failure| {
            let MintlayerEndpointError::HttpStatus { status, body } = &failure.error else {
                return false;
            };

            *status == StatusCode::NOT_FOUND.as_u16()
                && serde_json::from_str::<serde_json::Value>(body)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("error")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .as_deref()
                    == Some("Address not found")
        })
}

fn empty_address_info() -> MintlayerAddressInfo {
    let zero = MintlayerAmount {
        atoms: "0".into(),
        decimal: "0".into(),
    };

    MintlayerAddressInfo {
        coin_balance: zero.clone(),
        locked_coin_balance: zero,
        transaction_history: Vec::new(),
        tokens: Vec::new(),
    }
}

fn validate_path_segment(segment: &str) -> Result<(), MintlayerApiError> {
    if segment.is_empty() {
        return Err(MintlayerApiError::EmptyPathSegment);
    }

    Ok(())
}

fn build_endpoint_url(base_url: &Url, endpoint_path: &[&str]) -> Result<Url, MintlayerApiError> {
    let mut endpoint = base_url.clone();

    {
        let mut segments = endpoint
            .path_segments_mut()
            .map_err(|_| MintlayerApiError::EndpointUrlConstruction {
                base_url: base_url.to_string(),
            })?;

        segments.pop_if_empty();

        for segment in MINTLAYER_API_VERSION_PATH {
            segments.push(segment);
        }

        for segment in endpoint_path {
            segments.push(segment);
        }
    }

    Ok(endpoint)
}

fn response_body_preview(body: &[u8]) -> String {
    let preview_length = body.len().min(MAX_ERROR_BODY_PREVIEW);
    let mut preview = String::from_utf8_lossy(&body[..preview_length]).into_owned();

    if body.len() > MAX_ERROR_BODY_PREVIEW {
        preview.push_str("...");
    }

    preview
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::block_on;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    type MockResponse = Result<(StatusCode, Vec<u8>), String>;

    struct MockTransport {
        responses: Mutex<VecDeque<MockResponse>>,
        requested_urls: Mutex<Vec<String>>,
        requested_posts: Mutex<Vec<(String, String)>>,
    }

    impl MockTransport {
        fn new(responses: Vec<MockResponse>) -> Self {
            MockTransport {
                responses: Mutex::new(responses.into()),
                requested_urls: Mutex::new(Vec::new()),
                requested_posts: Mutex::new(Vec::new()),
            }
        }

        fn requested_urls(&self) -> Vec<String> {
            self.requested_urls.lock().unwrap().clone()
        }

        fn requested_posts(&self) -> Vec<(String, String)> {
            self.requested_posts.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl MintlayerHttpTransport for MockTransport {
        async fn get(&self, url: &str) -> Result<(StatusCode, Vec<u8>), String> {
            self.requested_urls.lock().unwrap().push(url.to_string());

            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("No mocked response configured".into()))
        }

        async fn post(&self, url: &str, body: &str) -> Result<(StatusCode, Vec<u8>), String> {
            self.requested_posts
                .lock()
                .unwrap()
                .push((url.to_string(), body.to_owned()));

            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("No mocked response configured".into()))
        }
    }

    fn api_url(host: &str) -> Url {
        Url::parse(&format!("https://{host}")).unwrap()
    }

    fn client_with_responses(
        api_urls: Vec<Url>,
        responses: Vec<MockResponse>,
    ) -> (MintlayerApiClientGeneric<MockTransport>, Arc<MockTransport>) {
        let transport = Arc::new(MockTransport::new(responses));
        let client =
            MintlayerApiClientGeneric::with_transport(api_urls, Arc::clone(&transport), Duration::from_secs(1));

        (client, transport)
    }

    #[test]
    fn deserialize_genesis_from_first_endpoint() {
        let response = br#"{
            "block_id": "2cf01f196066bb6f3a4856deb7999294ff520f633fe48e118e8044390e409870",
            "genesis_message": "Mintlayer mainnet",
            "timestamp": { "timestamp": 1706468400 },
            "utxos": []
        }"#
        .to_vec();

        let (client, transport) =
            client_with_responses(vec![api_url("api-1.example")], vec![Ok((StatusCode::OK, response))]);

        let genesis = block_on(client.genesis()).unwrap();

        assert_eq!(
            genesis.block_id,
            "2cf01f196066bb6f3a4856deb7999294ff520f633fe48e118e8044390e409870"
        );
        assert_eq!(
            transport.requested_urls(),
            vec!["https://api-1.example/api/v2/chain/genesis"]
        );
    }

    #[test]
    fn deserialize_chain_tip_from_first_endpoint() {
        let response = br#"{
            "block_height": 680824,
            "block_id": "635ce5bab5992e1c6a32cc9983ec4d295b1c89cc05afdf328f73041b9331a8d1"
        }"#
        .to_vec();

        let (client, transport) =
            client_with_responses(vec![api_url("api-1.example")], vec![Ok((StatusCode::OK, response))]);

        let tip = block_on(client.chain_tip()).unwrap();

        assert_eq!(tip.block_height, 680824);
        assert_eq!(
            transport.requested_urls(),
            vec!["https://api-1.example/api/v2/chain/tip"]
        );
    }

    #[test]
    fn block_height_in_main_chain_accepts_matching_id_case_insensitively() {
        let block_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let uppercase_block_id = block_id.to_ascii_uppercase();

        let (client, transport) = client_with_responses(
            vec![api_url("api.example")],
            vec![
                Ok((StatusCode::OK, br#"{"height":700000}"#.to_vec())),
                Ok((StatusCode::OK, serde_json::to_vec(&uppercase_block_id).unwrap())),
            ],
        );

        let height = block_on(client.block_height_in_main_chain(block_id)).unwrap();

        assert_eq!(height, Some(700000));
        assert_eq!(
            transport.requested_urls(),
            vec![
                format!("https://api.example/api/v2/block/{block_id}"),
                "https://api.example/api/v2/chain/700000".to_owned(),
            ]
        );
    }

    #[test]
    fn block_height_in_main_chain_rejects_competing_block_at_same_height() {
        let block_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let competing_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        let (client, transport) = client_with_responses(
            vec![api_url("api.example")],
            vec![
                Ok((StatusCode::OK, br#"{"height":700000}"#.to_vec())),
                Ok((StatusCode::OK, serde_json::to_vec(competing_id).unwrap())),
            ],
        );

        let height = block_on(client.block_height_in_main_chain(block_id)).unwrap();

        assert_eq!(height, None);
        assert_eq!(
            transport.requested_urls(),
            vec![
                format!("https://api.example/api/v2/block/{block_id}"),
                "https://api.example/api/v2/chain/700000".to_owned(),
            ]
        );
    }

    #[test]
    fn request_fee_rate() {
        let response = br#""100000000000""#.to_vec();
        let (client, transport) =
            client_with_responses(vec![api_url("api.example")], vec![Ok((StatusCode::OK, response))]);

        let fee_rate = block_on(client.fee_rate()).unwrap();

        assert_eq!(fee_rate.atoms_per_kb(), 100_000_000_000);
        assert_eq!(transport.requested_urls(), vec!["https://api.example/api/v2/feerate"]);
    }

    #[test]
    fn fee_rate_fails_over_after_invalid_schema() {
        let (client, transport) = client_with_responses(
            vec![api_url("api-1.example"), api_url("api-2.example")],
            vec![
                Ok((StatusCode::OK, b"100000000000".to_vec())),
                Ok((StatusCode::OK, br#""100000000000""#.to_vec())),
            ],
        );

        let fee_rate = block_on(client.fee_rate()).unwrap();

        assert_eq!(fee_rate.atoms_per_kb(), 100_000_000_000);
        assert_eq!(
            transport.requested_urls(),
            vec![
                "https://api-1.example/api/v2/feerate",
                "https://api-2.example/api/v2/feerate",
            ]
        );
    }

    #[test]
    fn fail_over_after_unsuccessful_http_status() {
        let valid_response = br#"{
            "block_height": 680825,
            "block_id": "735ce5bab5992e1c6a32cc9983ec4d295b1c89cc05afdf328f73041b9331a8d1"
        }"#
        .to_vec();

        let (client, transport) = client_with_responses(
            vec![api_url("api-1.example"), api_url("api-2.example")],
            vec![
                Ok((StatusCode::SERVICE_UNAVAILABLE, b"maintenance".to_vec())),
                Ok((StatusCode::OK, valid_response)),
            ],
        );

        let tip = block_on(client.chain_tip()).unwrap();

        assert_eq!(tip.block_height, 680825);
        assert_eq!(
            transport.requested_urls(),
            vec![
                "https://api-1.example/api/v2/chain/tip",
                "https://api-2.example/api/v2/chain/tip",
            ]
        );
    }

    #[test]
    fn fail_over_after_transport_error() {
        let valid_response = br#"{
            "block_height": 680826,
            "block_id": "835ce5bab5992e1c6a32cc9983ec4d295b1c89cc05afdf328f73041b9331a8d1"
        }"#
        .to_vec();

        let (client, transport) = client_with_responses(
            vec![api_url("api-1.example"), api_url("api-2.example")],
            vec![Err("connection refused".into()), Ok((StatusCode::OK, valid_response))],
        );

        let tip = block_on(client.chain_tip()).unwrap();

        assert_eq!(tip.block_height, 680826);
        assert_eq!(transport.requested_urls().len(), 2);
    }

    #[test]
    fn report_invalid_json_after_all_endpoints_fail() {
        let (client, _transport) = client_with_responses(
            vec![api_url("api-1.example")],
            vec![Ok((StatusCode::OK, b"{invalid-json".to_vec()))],
        );

        let error = block_on(client.chain_tip()).unwrap_err();

        match error {
            MintlayerApiError::AllEndpointsFailed { failures } => {
                assert_eq!(failures.len(), 1);
                assert!(matches!(failures[0].error, MintlayerEndpointError::InvalidResponse(_)));
            },
            unexpected => panic!("Unexpected error: {:?}", unexpected),
        }
    }

    #[test]
    fn preserve_reverse_proxy_prefix() {
        let base_url = Url::parse("https://api.example/mintlayer").unwrap();
        let response = br#"{
            "block_height": 680827,
            "block_id": "935ce5bab5992e1c6a32cc9983ec4d295b1c89cc05afdf328f73041b9331a8d1"
        }"#
        .to_vec();

        let (client, transport) = client_with_responses(vec![base_url], vec![Ok((StatusCode::OK, response))]);

        block_on(client.chain_tip()).unwrap();

        assert_eq!(
            transport.requested_urls(),
            vec!["https://api.example/mintlayer/api/v2/chain/tip"]
        );
    }

    #[test]
    fn address_not_found_returns_empty_address_info() {
        let (client, transport) = client_with_responses(
            vec![api_url("api-1.example")],
            vec![Ok((
                StatusCode::NOT_FOUND,
                br#"{"error":"Address not found"}"#.to_vec(),
            ))],
        );

        let info = block_on(client.address_info("mtc1qnew")).unwrap();

        assert_eq!(info.coin_balance.atoms, "0");
        assert_eq!(info.coin_balance.decimal, "0");
        assert_eq!(info.locked_coin_balance.atoms, "0");
        assert_eq!(info.locked_coin_balance.decimal, "0");
        assert!(info.transaction_history.is_empty());
        assert!(info.tokens.is_empty());
        assert_eq!(
            transport.requested_urls(),
            vec!["https://api-1.example/api/v2/address/mtc1qnew"]
        );
    }

    #[test]
    fn unrelated_address_404_remains_an_error() {
        let (client, _transport) = client_with_responses(
            vec![api_url("api-1.example")],
            vec![Ok((StatusCode::NOT_FOUND, br#"{"error":"Route not found"}"#.to_vec()))],
        );

        let error = block_on(client.address_info("mtc1qnew")).unwrap_err();

        assert!(matches!(error, MintlayerApiError::AllEndpointsFailed { .. }));
    }

    #[test]
    fn mixed_address_not_found_and_transport_failure_remains_an_error() {
        let (client, _transport) = client_with_responses(
            vec![api_url("api-1.example"), api_url("api-2.example")],
            vec![
                Ok((StatusCode::NOT_FOUND, br#"{"error":"Address not found"}"#.to_vec())),
                Err("second endpoint down".into()),
            ],
        );

        let error = block_on(client.address_info("mtc1qnew")).unwrap_err();

        assert!(matches!(error, MintlayerApiError::AllEndpointsFailed { .. }));
    }

    #[test]
    fn request_spendable_utxos() {
        let response = br#"[{
            "outpoint": {
                "source_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "index": 1,
                "source_type": "Transaction"
            },
            "utxo": {
                "destination": "mtc1qexample",
                "type": "Transfer",
                "value": {
                    "amount": {
                        "atoms": "261768000000000",
                        "decimal": "2617.68"
                    },
                    "type": "Coin"
                }
            }
        }]"#
        .to_vec();

        let (client, transport) =
            client_with_responses(vec![api_url("api.example")], vec![Ok((StatusCode::OK, response))]);

        let utxos = block_on(client.spendable_utxos("mtc1qexample")).unwrap();

        assert_eq!(utxos.len(), 1);
        assert_eq!(utxos[0].outpoint.index, 1);
        assert_eq!(
            transport.requested_urls(),
            vec!["https://api.example/api/v2/address/mtc1qexample/spendable-utxos"]
        );
    }

    #[test]
    fn request_transaction_observation() {
        let txid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

        let response = br#"{
            "id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "version_byte":1,
            "is_replaceable":false,
            "flags":0,
            "fee":{"atoms":"100","decimal":"0.000000001"},
            "inputs":[{
                "input":{
                    "input_type":"UTXO",
                    "source_type":"Transaction",
                    "source_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "index":0
                },
                "utxo":{
                    "type":"Htlc",
                    "htlc":{
                        "secret":{"string":null,"hex":"00112233"},
                        "secret_hash":{"string":null,"hex":"aabbccdd"},
                        "spend_key":"mtc1qspend",
                        "refund_timelock":{"UntilTime":1800000000},
                        "refund_key":"mtc1qrefund"
                    }
                }
            }],
            "outputs":[],
            "block_id":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "timestamp":"1800000100",
            "confirmations":"7"
        }"#
        .to_vec();

        let (client, transport) =
            client_with_responses(vec![api_url("api.example")], vec![Ok((StatusCode::OK, response))]);

        let transaction = block_on(client.transaction(txid)).unwrap();

        assert_eq!(transaction.id, txid);
        assert_eq!(transaction.block_id, "cc".repeat(32));
        assert_eq!(transaction.inputs.len(), 1);

        let input = &transaction.inputs[0];
        assert_eq!(input.input.input_type, "UTXO");
        assert_eq!(input.input.source_type.as_deref(), Some("Transaction"));
        assert_eq!(
            input.input.source_id.as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(input.input.index, Some(0));

        let utxo = input.utxo.as_ref().unwrap();
        assert_eq!(utxo["type"], "Htlc");
        assert_eq!(utxo["htlc"]["secret"]["hex"], "00112233");

        assert_eq!(
            transport.requested_urls(),
            vec![format!("https://api.example/api/v2/transaction/{txid}")]
        );
    }

    #[test]
    fn submit_transaction_stops_after_transport_error_because_outcome_is_unknown() {
        let txid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let signed_hex = "01020304";
        let success = serde_json::json!({ "tx_id": txid }).to_string().into_bytes();
        let (client, transport) = client_with_responses(
            vec![api_url("api-1.example"), api_url("api-2.example")],
            vec![Err("connection reset".to_owned()), Ok((StatusCode::OK, success))],
        );

        assert!(matches!(
            block_on(client.submit_transaction(signed_hex, txid)).unwrap_err(),
            MintlayerApiError::SubmissionOutcomeUnknown {
                endpoint,
                expected,
                ..
            } if endpoint == "https://api-1.example/api/v2/transaction" && expected == txid
        ));
        assert_eq!(transport.requested_posts().len(), 1);
    }

    #[test]
    fn submit_transaction_stops_after_server_error_because_outcome_is_unknown() {
        let txid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let signed_hex = "01020304";
        let success = serde_json::json!({ "tx_id": txid }).to_string().into_bytes();
        let (client, transport) = client_with_responses(
            vec![api_url("api-1.example"), api_url("api-2.example")],
            vec![
                Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    br#"{"error":"upstream RPC failed"}"#.to_vec(),
                )),
                Ok((StatusCode::OK, success)),
            ],
        );

        assert!(matches!(
            block_on(client.submit_transaction(signed_hex, txid)).unwrap_err(),
            MintlayerApiError::SubmissionOutcomeUnknown {
                endpoint,
                expected,
                ..
            } if endpoint == "https://api-1.example/api/v2/transaction" && expected == txid
        ));
        assert_eq!(transport.requested_posts().len(), 1);
    }

    #[test]
    fn submit_transaction_stops_after_invalid_success_body_because_outcome_is_unknown() {
        let txid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let signed_hex = "01020304";
        let success = serde_json::json!({ "tx_id": txid }).to_string().into_bytes();
        let (client, transport) = client_with_responses(
            vec![api_url("api-1.example"), api_url("api-2.example")],
            vec![
                Ok((StatusCode::OK, b"not-json".to_vec())),
                Ok((StatusCode::OK, success)),
            ],
        );

        assert!(matches!(
            block_on(client.submit_transaction(signed_hex, txid)).unwrap_err(),
            MintlayerApiError::SubmissionOutcomeUnknown {
                endpoint,
                expected,
                ..
            } if endpoint == "https://api-1.example/api/v2/transaction" && expected == txid
        ));
        assert_eq!(transport.requested_posts().len(), 1);
    }

    #[test]
    fn submit_transaction_posts_raw_hex_and_checks_txid() {
        let txid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let signed_hex = "01020304";
        let response = serde_json::json!({ "tx_id": txid }).to_string().into_bytes();
        let (client, transport) =
            client_with_responses(vec![api_url("api.example")], vec![Ok((StatusCode::OK, response))]);

        assert_eq!(block_on(client.submit_transaction(signed_hex, txid)).unwrap(), txid);
        assert_eq!(
            transport.requested_posts(),
            vec![(
                "https://api.example/api/v2/transaction".to_owned(),
                signed_hex.to_owned(),
            )]
        );
    }

    #[test]
    fn submit_transaction_fails_over_but_not_after_semantic_rejection() {
        let txid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let signed_hex = "01020304";
        let success = serde_json::json!({ "tx_id": txid }).to_string().into_bytes();

        let (client, transport) = client_with_responses(
            vec![api_url("api-1.example"), api_url("api-2.example")],
            vec![
                Ok((StatusCode::FORBIDDEN, br#"{"error":"POST disabled"}"#.to_vec())),
                Ok((StatusCode::OK, success)),
            ],
        );

        assert_eq!(block_on(client.submit_transaction(signed_hex, txid)).unwrap(), txid);
        assert_eq!(transport.requested_posts().len(), 2);

        let (client, transport) = client_with_responses(
            vec![api_url("api-1.example"), api_url("api-2.example")],
            vec![
                Ok((
                    StatusCode::BAD_REQUEST,
                    br#"{"error":"Invalid signed transaction"}"#.to_vec(),
                )),
                Ok((
                    StatusCode::OK,
                    serde_json::json!({ "tx_id": txid }).to_string().into_bytes(),
                )),
            ],
        );

        assert!(matches!(
            block_on(client.submit_transaction(signed_hex, txid)).unwrap_err(),
            MintlayerApiError::SubmissionRejected { status: 400, .. }
        ));
        assert_eq!(transport.requested_posts().len(), 1);
    }

    #[test]
    fn submit_transaction_rejects_response_txid_mismatch() {
        let expected = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let actual = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let response = serde_json::json!({ "tx_id": actual }).to_string().into_bytes();
        let (client, transport) =
            client_with_responses(vec![api_url("api.example")], vec![Ok((StatusCode::OK, response))]);

        assert_eq!(
            block_on(client.submit_transaction("01020304", expected)).unwrap_err(),
            MintlayerApiError::SubmissionTxIdMismatch {
                endpoint: "https://api.example/api/v2/transaction".to_owned(),
                expected: expected.to_owned(),
                actual: actual.to_owned(),
            }
        );
        assert_eq!(transport.requested_posts().len(), 1);
    }

    #[test]
    fn transaction_with_tx_hex_skips_endpoint_without_capability() {
        let txid = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

        let without_tx_hex = br#"{
            "id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "block_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "inputs":[]
        }"#
        .to_vec();

        let with_tx_hex = br#"{
            "id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "block_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "inputs":[],
            "tx_hex":"01020304"
        }"#
        .to_vec();

        let (client, transport) = client_with_responses(
            vec![api_url("api-1.example"), api_url("api-2.example")],
            vec![Ok((StatusCode::OK, without_tx_hex)), Ok((StatusCode::OK, with_tx_hex))],
        );

        let transaction = block_on(client.transaction_with_tx_hex(txid)).unwrap();

        assert_eq!(transaction.id, txid);
        assert_eq!(transaction.tx_hex.as_deref(), Some("01020304"));
        assert_eq!(
            transport.requested_urls(),
            vec![
                format!("https://api-1.example/api/v2/transaction/{txid}"),
                format!("https://api-2.example/api/v2/transaction/{txid}"),
            ]
        );
    }

    #[test]
    fn reject_empty_address_without_requesting_endpoint() {
        let (client, transport) = client_with_responses(vec![api_url("api.example")], Vec::new());

        let error = block_on(client.address_info("")).unwrap_err();

        assert_eq!(error, MintlayerApiError::EmptyPathSegment);
        assert!(transport.requested_urls().is_empty());
    }

    #[test]
    fn truncate_http_error_body() {
        let large_body = vec![b'x'; MAX_ERROR_BODY_PREVIEW + 100];
        let (client, _transport) = client_with_responses(
            vec![api_url("api.example")],
            vec![Ok((StatusCode::INTERNAL_SERVER_ERROR, large_body))],
        );

        let error = block_on(client.chain_tip()).unwrap_err();

        match error {
            MintlayerApiError::AllEndpointsFailed { failures } => match &failures[0].error {
                MintlayerEndpointError::HttpStatus { body, .. } => {
                    assert_eq!(body.len(), MAX_ERROR_BODY_PREVIEW + 3);
                    assert!(body.ends_with("..."));
                },
                unexpected => panic!("Unexpected endpoint error: {:?}", unexpected),
            },
            unexpected => panic!("Unexpected API error: {:?}", unexpected),
        }
    }
}
