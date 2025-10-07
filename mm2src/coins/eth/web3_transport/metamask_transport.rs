use crate::eth::web3_transport::Web3SendOut;
use crate::RpcTransportEventHandlerShared;
use jsonrpc_core::Call;
use mm2_metamask::{detect_metamask_provider, Eip1193Provider, MetamaskResult, MetamaskSession};
use serde_json::Value as Json;
use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use web3::{RequestId, Transport};

/// Configuration for working with the MetaMask wallet.
pub struct MetamaskEthConfig {
    /// The `ChainId` that the MetaMask wallet should be targeted on each RPC.
    pub chain_id: u64,
}

/// Transport layer for interacting with the MetaMask wallet.
#[derive(Clone)]
pub struct MetamaskTransport {
    inner: Arc<MetamaskTransportInner>,
    pub(crate) last_request_failed: Arc<AtomicBool>,
}

struct MetamaskTransportInner {
    eth_config: MetamaskEthConfig,
    eip1193: Eip1193Provider,
    // TODO use `event_handlers` properly.
    _event_handlers: Vec<RpcTransportEventHandlerShared>,
}

impl MetamaskTransport {
    pub fn detect(
        eth_config: MetamaskEthConfig,
        event_handlers: Vec<RpcTransportEventHandlerShared>,
    ) -> MetamaskResult<MetamaskTransport> {
        let eip1193 = detect_metamask_provider()?;
        let inner = MetamaskTransportInner {
            eth_config,
            eip1193,
            _event_handlers: event_handlers,
        };
        Ok(MetamaskTransport {
            inner: Arc::new(inner),
            last_request_failed: Arc::new(AtomicBool::new(false)),
        })
    }
}

impl Transport for MetamaskTransport {
    type Out = Web3SendOut;

    fn prepare(&self, method: &str, params: Vec<Json>) -> (RequestId, Call) {
        self.inner.eip1193.prepare(method, params)
    }

    fn send(&self, id: RequestId, request: Call) -> Self::Out {
        let selfi = self.clone();
        let fut = async move { selfi.send_impl(id, request).await };
        Box::pin(fut)
    }
}

impl fmt::Debug for MetamaskTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MetamaskTransport")
    }
}

impl MetamaskTransport {
    async fn send_impl(&self, id: RequestId, request: Call) -> Result<Json, web3::Error> {
        // Hold the mutex guard until the request is finished.
        let _rpc_lock = self.request_preparation().await?;
        self.inner.eip1193.send(id, request).await
    }

    /// Ensures that the MetaMask wallet is targeted to [`EthConfig::chain_id`].
    async fn request_preparation(&self) -> Result<MetamaskSession<'_>, web3::Error> {
        // Lock the MetaMask session and keep it until the RPC is finished.
        let metamask_session = MetamaskSession::lock(&self.inner.eip1193).await;
        metamask_session
            .wallet_switch_ethereum_chain(self.inner.eth_config.chain_id)
            .await?;

        Ok(metamask_session)
    }
}
