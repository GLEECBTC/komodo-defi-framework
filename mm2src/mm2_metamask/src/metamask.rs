use crate::eip_1193_provider::Eip1193Provider;
use crate::metamask_error::{MetamaskError, MetamaskResult};
use futures::lock::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
use itertools::Itertools;
use lazy_static::lazy_static;
use mm2_err_handle::prelude::*;
use mm2_eth::{eip712::Eip712, eip712_encode::hash_typed_data};
use serde::Serialize;
use serde_json::{json, Value as Json};
use web3::helpers::CallFuture;
use web3::types::H256;
use web3::Transport;

lazy_static! {
    /// This mutex is used to limit the number of concurrent requests to one.
    /// It's required to avoid switching ETH chain ID during the request.
    static ref METAMASK_MUTEX: AsyncMutex<()> = AsyncMutex::new(());
}

pub fn detect_metamask_provider() -> MetamaskResult<Eip1193Provider> {
    Eip1193Provider::detect().or_mm_err(|| MetamaskError::EthProviderNotFound)
}

/// `MetamaskSession` is designed the way that there can be only one active session at the moment.
pub struct MetamaskSession<'a> {
    transport: &'a Eip1193Provider,
    _guard: AsyncMutexGuard<'a, ()>,
}

impl<'a> MetamaskSession<'a> {
    /// Locks the global `METAMASK_MUTEX` to prevent simultaneous requests.
    pub async fn lock(transport: &'a Eip1193Provider) -> MetamaskSession<'a> {
        MetamaskSession {
            transport,
            _guard: METAMASK_MUTEX.lock().await,
        }
    }

    /// Invokes the `eth_requestAccounts` method. We expect only one active account.
    /// https://docs.metamask.io/guide/rpc-api.html#eth-requestaccounts
    pub async fn eth_request_account(&self) -> MetamaskResult<String> {
        let accounts: Vec<String> = CallFuture::new(self.transport.execute("eth_requestAccounts", vec![])).await?;
        accounts
            .into_iter()
            .exactly_one()
            .map_to_mm(|_| MetamaskError::ExpectedOneEthAccount)
    }

    /// Invokes the `wallet_switchEthereumChain` method.
    /// https://docs.metamask.io/guide/rpc-api.html#wallet-switchethereumchain
    pub async fn wallet_switch_ethereum_chain(&self, chain_id: u64) -> Result<(), web3::Error> {
        let req = json!({
            "chainId": format!("0x{chain_id:x}"),
        });

        CallFuture::new(self.transport.execute("wallet_switchEthereumChain", vec![req])).await
    }

    /// Returns a hash of the `Eip712` request and the signature.
    ///
    /// Note: `user_address` must match user's active address.
    pub async fn sign_typed_data_v4<Domain, SignData>(
        &self,
        user_address: String,
        req: Eip712<Domain, SignData>,
    ) -> MetamaskResult<(H256, String)>
    where
        Domain: Serialize,
        SignData: Serialize,
    {
        let user_address = Json::String(user_address);
        let req_str =
            serde_json::to_string(&req).map_to_mm(|e| MetamaskError::ErrorSerializingArguments(e.to_string()))?;
        let hash = hash_typed_data(req)?;

        let signature = CallFuture::new(
            self.transport
                .execute("eth_signTypedData_v4", vec![user_address, Json::String(req_str)]),
        )
        .await?;

        Ok((hash, signature))
    }
}
