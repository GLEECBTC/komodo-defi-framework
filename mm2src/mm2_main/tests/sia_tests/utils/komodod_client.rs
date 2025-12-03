//! Extremely bare-bones `komodod` client to facilitate Sia functional and integration tests.
//!
//! This client is **not intended for production use**.
//! It offers **no error handling**, and `.unwrap()` is used liberally,
//! as it is expected to **panic on any error**.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use http::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client as ReqwestClient;
use std::net::IpAddr;
use std::time::Duration;
use url::Url;

pub struct KomododClientConf {
    pub ip: IpAddr,
    pub port: u16,
    pub rpcuser: String,
    pub rpcpassword: String,
    pub timeout: Option<u64>,
}

pub struct KomododClient {
    pub client: ReqwestClient,
    pub url: Url,
}

impl KomododClient {
    /// Create a new KomododClient. Note, does not actually ping the server.
    pub async fn new(conf: KomododClientConf) -> Self {
        // Construct default headers for all requests
        let mut headers = HeaderMap::new();

        // Set Authorization header with given rpcuser and rpcpassword
        let auth_value = format!(
            "Basic {}",
            BASE64.encode(format!("{}:{}", conf.rpcuser, conf.rpcpassword))
        );
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&auth_value).unwrap());

        // Set Content-Type header to application/json
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        // set a timeout for http requests
        let timeout = conf.timeout.unwrap_or(15);

        let url: Url = format!("http://{}:{}/", conf.ip, conf.port).parse().unwrap();

        let client = ReqwestClient::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(timeout))
            .build()
            .unwrap();

        let client = KomododClient { client, url };

        let _getinfo = client.rpc("getinfo", json!([])).await;
        client
    }

    pub async fn rpc(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let payload = json!({
            "jsonrpc": "1.0",
            "id": "sia_tests_utils",
            "method": method,
            "params": params
        });
        common::log::debug!("Sending komodod RPC request: {}", payload);

        let mut attempts = 0;
        let max_retries = 3;
        loop {
            match self.client.post(self.url.clone()).json(&payload).send().await {
                Ok(response) => {
                    let json_response: serde_json::Value = response.json().await.unwrap();
                    common::log::debug!("Received komodod RPC response: {}", json_response);
                    return json_response;
                },
                Err(err) => {
                    attempts += 1;
                    if attempts >= max_retries {
                        common::log::debug!("RPC request failed after {} attempts: {}", max_retries, err);
                        panic!("RPC request failed: {}", err);
                    } else {
                        common::log::debug!(
                            "RPC request attempt {}/{} failed: {}. Retrying...",
                            attempts,
                            max_retries,
                            err
                        );
                        tokio::time::sleep(Duration::from_secs(1)).await; // Add a short delay before retrying
                    }
                },
            }
        }
    }
}
