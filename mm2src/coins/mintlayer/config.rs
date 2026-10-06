use serde_derive::{Deserialize, Serialize};

/// Mintlayer network names defined by Mintlayer Core.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MintlayerNetwork {
    Mainnet,
    Testnet,
    Regtest,
    Signet,
}

/// Static configuration loaded from the KDF coins configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MintlayerCoinConf {
    #[serde(rename = "coin")]
    pub ticker: String,
    pub network: MintlayerNetwork,
    pub decimals: u8,
    pub required_confirmations: u64,
    pub genesis_block_id: String,
}

/// Public Mintlayer API endpoints used by the coin instance.
///
/// Endpoints will be validated and normalized when the API client is built.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MintlayerApiClientConfig {
    pub api_urls: Vec<String>,
}

/// Optional Mintlayer node RPC configuration.
///
/// During the staged transaction integration this endpoint is deliberately
/// restricted to loopback. Remote node submission remains disabled.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MintlayerNodeClientConfig {
    pub rpc_url: String,
    #[serde(default)]
    pub rpc_cookie_file: Option<String>,
}

/// Parameters supplied when activating Mintlayer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MintlayerActivationRequest {
    #[serde(default)]
    pub tx_history: bool,
    pub required_confirmations: Option<u64>,
    pub client_conf: MintlayerApiClientConfig,
    #[serde(default)]
    pub node_conf: Option<MintlayerNodeClientConfig>,
}

/// Standalone protocol information used by the modern activation framework.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MintlayerProtocolInfo;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserialize_all_supported_networks() {
        let cases = [
            ("mainnet", MintlayerNetwork::Mainnet),
            ("testnet", MintlayerNetwork::Testnet),
            ("regtest", MintlayerNetwork::Regtest),
            ("signet", MintlayerNetwork::Signet),
        ];

        for (network_name, expected) in cases {
            let network: MintlayerNetwork = serde_json::from_value(json!(network_name)).unwrap();

            assert_eq!(network, expected);
        }
    }

    #[test]
    fn reject_unknown_network() {
        let result = serde_json::from_value::<MintlayerNetwork>(json!("unknown"));

        assert!(result.is_err());
    }

    #[test]
    fn deserialize_coin_configuration() {
        let response = json!({
            "coin": "ML",
            "network": "mainnet",
            "decimals": 11,
            "required_confirmations": 2,
            "genesis_block_id":
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        });

        let conf: MintlayerCoinConf = serde_json::from_value(response.clone()).unwrap();

        assert_eq!(conf.ticker, "ML");
        assert_eq!(conf.network, MintlayerNetwork::Mainnet);
        assert_eq!(conf.decimals, 11);
        assert_eq!(conf.required_confirmations, 2);
        assert_eq!(serde_json::to_value(conf).unwrap(), response);
    }

    #[test]
    fn deserialize_activation_request_with_defaults() {
        let response = json!({
            "client_conf": {
                "api_urls": [
                    concat!("https", "://mintlayer-api-1.example"),
                    concat!("https", "://mintlayer-api-2.example")
                ]
            },
            "required_confirmations": null
        });

        let request: MintlayerActivationRequest = serde_json::from_value(response).unwrap();

        assert!(!request.tx_history);
        assert_eq!(request.required_confirmations, None);
        assert_eq!(request.client_conf.api_urls.len(), 2);
        assert_eq!(
            request.client_conf.api_urls,
            vec![
                concat!("https", "://mintlayer-api-1.example").to_string(),
                concat!("https", "://mintlayer-api-2.example").to_string(),
            ]
        );
    }
}
