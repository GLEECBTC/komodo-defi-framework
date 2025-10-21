use ethereum_types::U256;
use futures::future::BoxFuture;
use jsonrpc_core::Call;
#[cfg(target_arch = "wasm32")]
use mm2_metamask::MetamaskResult;
use serde_json::Value as Json;
use serde_json::Value;
use std::sync::atomic::Ordering;
use web3::{Error, RequestId, Transport};

use crate::RpcTransportEventHandlerShared;

pub(crate) mod http_transport;
#[cfg(target_arch = "wasm32")]
pub(crate) mod metamask_transport;
pub(crate) mod websocket_transport;

pub(crate) type Web3SendOut = BoxFuture<'static, Result<Json, Error>>;

/// The transport layer for interacting with a Web3 provider.
#[derive(Clone, Debug)]
pub enum Web3Transport {
    Http(http_transport::HttpTransport),
    Websocket(websocket_transport::WebsocketTransport),
    #[cfg(target_arch = "wasm32")]
    Metamask(metamask_transport::MetamaskTransport),
}

impl Web3Transport {
    pub fn new_http_with_event_handlers(
        node: http_transport::HttpTransportNode,
        event_handlers: Vec<RpcTransportEventHandlerShared>,
    ) -> Web3Transport {
        http_transport::HttpTransport::with_event_handlers(node, event_handlers).into()
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn new_metamask_with_event_handlers(
        eth_config: metamask_transport::MetamaskEthConfig,
        event_handlers: Vec<RpcTransportEventHandlerShared>,
    ) -> MetamaskResult<Web3Transport> {
        Ok(metamask_transport::MetamaskTransport::detect(eth_config, event_handlers)?.into())
    }

    pub fn is_last_request_failed(&self) -> bool {
        match self {
            Web3Transport::Http(http) => http.last_request_failed.load(Ordering::SeqCst),
            Web3Transport::Websocket(websocket) => websocket.last_request_failed.load(Ordering::SeqCst),
            #[cfg(target_arch = "wasm32")]
            Web3Transport::Metamask(metamask) => metamask.last_request_failed.load(Ordering::SeqCst),
        }
    }

    fn set_last_request_failed(&self, val: bool) {
        match self {
            Web3Transport::Http(http) => http.last_request_failed.store(val, Ordering::SeqCst),
            Web3Transport::Websocket(websocket) => websocket.last_request_failed.store(val, Ordering::SeqCst),
            #[cfg(target_arch = "wasm32")]
            Web3Transport::Metamask(metamask) => metamask.last_request_failed.store(val, Ordering::SeqCst),
        }
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub fn new_http(node: http_transport::HttpTransportNode) -> Web3Transport {
        http_transport::HttpTransport::new(node).into()
    }
}

impl Transport for Web3Transport {
    type Out = Web3SendOut;

    fn prepare(&self, method: &str, params: Vec<Value>) -> (RequestId, Call) {
        match self {
            Web3Transport::Http(http) => http.prepare(method, params),
            Web3Transport::Websocket(websocket) => websocket.prepare(method, params),
            #[cfg(target_arch = "wasm32")]
            Web3Transport::Metamask(metamask) => metamask.prepare(method, params),
        }
    }

    fn send(&self, id: RequestId, request: Call) -> Self::Out {
        let selfi = self.clone();
        let fut = async move {
            let result = match &selfi {
                Web3Transport::Http(http) => http.send(id, request),
                Web3Transport::Websocket(websocket) => websocket.send(id, request),
                #[cfg(target_arch = "wasm32")]
                Web3Transport::Metamask(metamask) => metamask.send(id, request),
            }
            .await;

            selfi.set_last_request_failed(result.is_err());

            result
        };

        Box::pin(fut)
    }
}

impl From<http_transport::HttpTransport> for Web3Transport {
    fn from(http: http_transport::HttpTransport) -> Self {
        Web3Transport::Http(http)
    }
}

impl From<websocket_transport::WebsocketTransport> for Web3Transport {
    fn from(websocket: websocket_transport::WebsocketTransport) -> Self {
        Web3Transport::Websocket(websocket)
    }
}

#[cfg(target_arch = "wasm32")]
impl From<metamask_transport::MetamaskTransport> for Web3Transport {
    fn from(metamask: metamask_transport::MetamaskTransport) -> Self {
        Web3Transport::Metamask(metamask)
    }
}

#[derive(Debug, Deserialize)]
pub struct FeeHistoryResult {
    #[expect(dead_code)]
    #[serde(rename = "oldestBlock")]
    pub oldest_block: U256,
    #[serde(rename = "baseFeePerGas")]
    pub base_fee_per_gas: Vec<U256>,
    #[expect(dead_code)]
    #[serde(rename = "gasUsedRatio")]
    pub gas_used_ratio: Vec<f64>,
    #[serde(rename = "reward")]
    pub priority_rewards: Option<Vec<Vec<U256>>>,
}
