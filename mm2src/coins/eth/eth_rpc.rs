//! This module serves as an abstraction layer for Ethereum RPCs.
//! Unlike the built-in functions in web3, this module dynamically
//! rotates through all transports in case of failures.

use super::web3_transport::FeeHistoryResult;
use super::{web3_transport::Web3Transport, EthCoin};
use common::{custom_futures::timeout::FutureTimerExt, log::debug};
use serde_json::Value;
use std::time::Duration;
use web3::types::{
    Address, Block, BlockId, BlockNumber, Bytes, CallRequest, FeeHistory, Filter, Log, Proof, SyncState, Trace,
    TraceFilter, Transaction, TransactionId, TransactionReceipt, TransactionRequest, Work, H256, H520, H64, U256, U64,
};
use web3::{helpers, Transport};

/// Internal timeout to try an rpc node before switching to next one
const TRY_RPC_NODE_TIMEOUT_S: Duration = Duration::from_secs(10);

impl EthCoin {
    async fn try_rpc_send(&self, method: &str, params: Vec<jsonrpc_core::Value>) -> Result<Value, web3::Error> {
        let mut clients = self.web3_instances.lock().await;

        let mut error = web3::Error::Unreachable;
        for (i, client) in clients.clone().into_iter().enumerate() {
            let execute_fut = match client.as_ref().transport() {
                Web3Transport::Http(http) => http.execute(method, params.clone()),
                Web3Transport::Websocket(socket) => {
                    socket.maybe_spawn_connection_loop(self.clone());
                    socket.execute(method, params.clone())
                },
                #[cfg(target_arch = "wasm32")]
                Web3Transport::Metamask(metamask) => metamask.execute(method, params.clone()),
            };

            match execute_fut.timeout(TRY_RPC_NODE_TIMEOUT_S).await {
                Ok(Ok(r)) => {
                    // Bring the live client to the front of rpc_clients
                    clients.rotate_left(i);
                    return Ok(r);
                },
                Ok(Err(err)) => {
                    debug!("Request on '{method}' failed. Error: {err}");
                    error = err;

                    if let Web3Transport::Websocket(socket_transport) = client.as_ref().transport() {
                        socket_transport.stop_connection_loop().await;
                    };
                },
                Err(timeout_error) => {
                    debug!("Timeout exceed for '{method}' request. Error: {timeout_error}",);

                    if let Web3Transport::Websocket(socket_transport) = client.as_ref().transport() {
                        socket_transport.stop_connection_loop().await;
                    };
                },
            };
        }

        Err(error)
    }
}

#[allow(dead_code)]
impl EthCoin {
    /// Get list of available accounts.
    pub(crate) async fn accounts(&self) -> Result<Vec<Address>, web3::Error> {
        self.try_rpc_send("eth_accounts", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get current block number
    pub(crate) async fn block_number(&self) -> Result<U64, web3::Error> {
        self.try_rpc_send("eth_blockNumber", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Call a constant method of contract without changing the state of the blockchain.
    pub(crate) async fn call(&self, req: CallRequest, block: Option<BlockId>) -> Result<Bytes, web3::Error> {
        let req = helpers::serialize(&req);
        let block = helpers::serialize(&block.unwrap_or_else(|| BlockNumber::Latest.into()));

        self.try_rpc_send("eth_call", vec![req, block])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get coinbase address
    pub(crate) async fn coinbase(&self) -> Result<Address, web3::Error> {
        self.try_rpc_send("eth_coinbase", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Compile LLL
    pub(crate) async fn compile_lll(&self, code: String) -> Result<Bytes, web3::Error> {
        let code = helpers::serialize(&code);
        self.try_rpc_send("eth_compileLLL", vec![code])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Compile Solidity
    pub(crate) async fn compile_solidity(&self, code: String) -> Result<Bytes, web3::Error> {
        let code = helpers::serialize(&code);
        self.try_rpc_send("eth_compileSolidity", vec![code])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Compile Serpent
    pub(crate) async fn compile_serpent(&self, code: String) -> Result<Bytes, web3::Error> {
        let code = helpers::serialize(&code);
        self.try_rpc_send("eth_compileSerpent", vec![code])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Call a contract without changing the state of the blockchain to estimate gas usage.
    pub(crate) async fn estimate_gas(&self, req: CallRequest, block: Option<BlockNumber>) -> Result<U256, web3::Error> {
        let req = helpers::serialize(&req);

        let args = match block {
            Some(block) => vec![req, helpers::serialize(&block)],
            None => vec![req],
        };

        self.try_rpc_send("eth_estimateGas", args)
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get current recommended gas price
    pub(crate) async fn gas_price(&self) -> Result<U256, web3::Error> {
        self.try_rpc_send("eth_gasPrice", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Returns a collection of historical gas information. This can be used for evaluating the max_fee_per_gas
    /// and max_priority_fee_per_gas to send the future transactions.
    pub(crate) async fn fee_history(
        &self,
        block_count: U256,
        newest_block: BlockNumber,
        reward_percentiles: Option<Vec<f64>>,
    ) -> Result<FeeHistory, web3::Error> {
        let block_count = helpers::serialize(&block_count);
        let newest_block = helpers::serialize(&newest_block);
        let reward_percentiles = helpers::serialize(&reward_percentiles);

        self.try_rpc_send("eth_feeHistory", vec![block_count, newest_block, reward_percentiles])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get balance of given address
    pub(crate) async fn balance(&self, address: Address, block: Option<BlockNumber>) -> Result<U256, web3::Error> {
        let address = helpers::serialize(&address);
        let block = helpers::serialize(&block.unwrap_or(BlockNumber::Latest));

        self.try_rpc_send("eth_getBalance", vec![address, block])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get all logs matching a given filter object
    pub(crate) async fn logs(&self, filter: Filter) -> Result<Vec<Log>, web3::Error> {
        let filter = helpers::serialize(&filter);
        self.try_rpc_send("eth_getLogs", vec![filter])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get block details with transaction hashes.
    pub(crate) async fn block(&self, block: BlockId) -> Result<Option<Block<H256>>, web3::Error> {
        let include_txs = helpers::serialize(&false);

        let result = match block {
            BlockId::Hash(hash) => {
                let hash = helpers::serialize(&hash);
                self.try_rpc_send("eth_getBlockByHash", vec![hash, include_txs])
            },
            BlockId::Number(num) => {
                let num = helpers::serialize(&num);
                self.try_rpc_send("eth_getBlockByNumber", vec![num, include_txs])
            },
        };

        result.await.and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get block details with full transaction objects.
    pub(crate) async fn block_with_txs(&self, block: BlockId) -> Result<Option<Block<Transaction>>, web3::Error> {
        let include_txs = helpers::serialize(&true);

        let result = match block {
            BlockId::Hash(hash) => {
                let hash = helpers::serialize(&hash);
                self.try_rpc_send("eth_getBlockByHash", vec![hash, include_txs])
            },
            BlockId::Number(num) => {
                let num = helpers::serialize(&num);
                self.try_rpc_send("eth_getBlockByNumber", vec![num, include_txs])
            },
        };

        result.await.and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get number of transactions in block
    pub(crate) async fn block_transaction_count(&self, block: BlockId) -> Result<Option<U256>, web3::Error> {
        let result = match block {
            BlockId::Hash(hash) => {
                let hash = helpers::serialize(&hash);
                self.try_rpc_send("eth_getBlockTransactionCountByHash", vec![hash])
            },
            BlockId::Number(num) => {
                let num = helpers::serialize(&num);

                self.try_rpc_send("eth_getBlockTransactionCountByNumber", vec![num])
            },
        };

        result.await.and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get code under given address
    pub(crate) async fn code(&self, address: Address, block: Option<BlockNumber>) -> Result<Bytes, web3::Error> {
        let address = helpers::serialize(&address);
        let block = helpers::serialize(&block.unwrap_or(BlockNumber::Latest));

        self.try_rpc_send("eth_getCode", vec![address, block])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get supported compilers
    pub(crate) async fn compilers(&self) -> Result<Vec<String>, web3::Error> {
        self.try_rpc_send("eth_getCompilers", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get chain id from network
    pub(crate) async fn network_chain_id(&self) -> Result<U256, web3::Error> {
        self.try_rpc_send("eth_chainId", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get available user accounts. This method is only available in the browser. With MetaMask,
    /// this will cause the popup that prompts the user to allow or deny access to their accounts
    /// to your app.
    pub(crate) async fn request_accounts(&self) -> Result<Vec<Address>, web3::Error> {
        self.try_rpc_send("eth_requestAccounts", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get storage entry
    pub(crate) async fn storage(
        &self,
        address: Address,
        idx: U256,
        block: Option<BlockNumber>,
    ) -> Result<H256, web3::Error> {
        let address = helpers::serialize(&address);
        let idx = helpers::serialize(&idx);
        let block = helpers::serialize(&block.unwrap_or(BlockNumber::Latest));

        self.try_rpc_send("eth_getStorageAt", vec![address, idx, block])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get nonce
    pub(crate) async fn transaction_count(
        &self,
        address: Address,
        block: Option<BlockNumber>,
    ) -> Result<U256, web3::Error> {
        let address = helpers::serialize(&address);
        let block = helpers::serialize(&block.unwrap_or(BlockNumber::Latest));

        self.try_rpc_send("eth_getTransactionCount", vec![address, block])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get transaction
    pub(crate) async fn transaction(&self, id: TransactionId) -> Result<Option<Transaction>, web3::Error> {
        let result = match id {
            TransactionId::Hash(hash) => {
                let hash = helpers::serialize(&hash);
                self.try_rpc_send("eth_getTransactionByHash", vec![hash])
            },
            TransactionId::Block(BlockId::Hash(hash), index) => {
                let hash = helpers::serialize(&hash);
                let idx = helpers::serialize(&index);
                self.try_rpc_send("eth_getTransactionByBlockHashAndIndex", vec![hash, idx])
            },
            TransactionId::Block(BlockId::Number(number), index) => {
                let number = helpers::serialize(&number);
                let idx = helpers::serialize(&index);
                self.try_rpc_send("eth_getTransactionByBlockNumberAndIndex", vec![number, idx])
            },
        };

        result.await.and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get transaction receipt
    pub(crate) async fn transaction_receipt(&self, hash: H256) -> Result<Option<TransactionReceipt>, web3::Error> {
        let hash = helpers::serialize(&hash);

        self.try_rpc_send("eth_getTransactionReceipt", vec![hash])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get work package
    pub(crate) async fn work(&self) -> Result<Work, web3::Error> {
        self.try_rpc_send("eth_getWork", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get hash rate
    pub(crate) async fn hashrate(&self) -> Result<U256, web3::Error> {
        self.try_rpc_send("eth_hashrate", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get mining status
    pub(crate) async fn mining(&self) -> Result<bool, web3::Error> {
        self.try_rpc_send("eth_mining", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Start new block filter
    pub(crate) async fn new_block_filter(&self) -> Result<U256, web3::Error> {
        self.try_rpc_send("eth_newBlockFilter", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Start new pending transaction filter
    pub(crate) async fn new_pending_transaction_filter(&self) -> Result<U256, web3::Error> {
        self.try_rpc_send("eth_newPendingTransactionFilter", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Start new pending transaction filter
    pub(crate) async fn protocol_version(&self) -> Result<String, web3::Error> {
        self.try_rpc_send("eth_protocolVersion", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Sends a rlp-encoded signed transaction
    pub(crate) async fn send_raw_transaction(&self, rlp: Bytes) -> Result<H256, web3::Error> {
        let rlp = helpers::serialize(&rlp);
        self.try_rpc_send("eth_sendRawTransaction", vec![rlp])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Sends a transaction transaction
    pub(crate) async fn send_transaction(&self, tx: TransactionRequest) -> Result<H256, web3::Error> {
        let tx = helpers::serialize(&tx);
        self.try_rpc_send("eth_sendTransaction", vec![tx])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Signs a hash of given data
    pub(crate) async fn sign(&self, address: Address, data: Bytes) -> Result<H520, web3::Error> {
        let address = helpers::serialize(&address);
        let data = helpers::serialize(&data);
        self.try_rpc_send("eth_sign", vec![address, data])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Submit hashrate of external miner
    pub(crate) async fn submit_hashrate(&self, rate: U256, id: H256) -> Result<bool, web3::Error> {
        let rate = helpers::serialize(&rate);
        let id = helpers::serialize(&id);
        self.try_rpc_send("eth_submitHashrate", vec![rate, id])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Submit work of external miner
    pub(crate) async fn submit_work(&self, nonce: H64, pow_hash: H256, mix_hash: H256) -> Result<bool, web3::Error> {
        let nonce = helpers::serialize(&nonce);
        let pow_hash = helpers::serialize(&pow_hash);
        let mix_hash = helpers::serialize(&mix_hash);
        self.try_rpc_send("eth_submitWork", vec![nonce, pow_hash, mix_hash])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Get syncing status
    pub(crate) async fn syncing(&self) -> Result<SyncState, web3::Error> {
        self.try_rpc_send("eth_syncing", vec![])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Returns the account- and storage-values of the specified account including the Merkle-proof.
    pub(crate) async fn proof(
        &self,
        address: Address,
        keys: Vec<U256>,
        block: Option<BlockNumber>,
    ) -> Result<Option<Proof>, web3::Error> {
        let add = helpers::serialize(&address);
        let ks = helpers::serialize(&keys);
        let blk = helpers::serialize(&block.unwrap_or(BlockNumber::Latest));
        self.try_rpc_send("eth_getProof", vec![add, ks, blk])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    pub(crate) async fn eth_fee_history(
        &self,
        count: U256,
        block: BlockNumber,
        reward_percentiles: &[f64],
    ) -> Result<FeeHistoryResult, web3::Error> {
        let count = helpers::serialize(&count);
        let block = helpers::serialize(&block);
        let reward_percentiles = helpers::serialize(&reward_percentiles);
        let params = vec![count, block, reward_percentiles];

        self.try_rpc_send("eth_feeHistory", params)
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }

    /// Return traces matching the given filter
    ///
    /// See [TraceFilterBuilder](../types/struct.TraceFilterBuilder.html)
    pub(crate) async fn trace_filter(&self, filter: TraceFilter) -> Result<Vec<Trace>, web3::Error> {
        let filter = helpers::serialize(&filter);

        self.try_rpc_send("trace_filter", vec![filter])
            .await
            .and_then(|t| serde_json::from_value(t).map_err(Into::into))
    }
}
