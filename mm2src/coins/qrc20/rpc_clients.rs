use super::*;
use crate::utxo::rpc_clients::{UtxoRpcError, UtxoRpcFut};
use rpc::v1::types::H256;

impl From<ethabi::Error> for UtxoRpcError {
    fn from(e: ethabi::Error) -> Self {
        // Currently, we use the `ethabi` crate to work with a smart contract ABI known at compile time.
        // It's an internal error if there are any issues during working with a smart contract ABI.
        UtxoRpcError::Internal(e.to_string())
    }
}

pub mod for_tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    pub struct ContractCreateResult {
        /// The transaction id.
        pub txid: H256Json,
        /// QTUM address of the sender.
        pub sender: String,
        /// ripemd-160 hash of the sender.
        pub hash160: H160Json,
        /// Expected contract address.
        pub address: H160Json,
    }

    #[derive(Debug, Deserialize)]
    pub struct SendToContractResult {
        /// The transaction id.
        pub txid: H256Json,
        /// QTUM address of the sender.
        pub sender: String,
        /// ripemd-160 hash of the sender.
        pub hash160: H160Json,
    }

    /// QRC20 Native RPC operations that may change the wallet state.
    pub trait Qrc20NativeWalletOps {
        /// Create contract with bytecode and specified sender.
        /// https://docs.qtum.site/en/Qtum-RPC-API/#createcontract
        fn create_contract(
            &self,
            bytecode: &BytesJson,
            gas_limit: u64,
            gas_price: BigDecimal,
            sender: &str,
        ) -> RpcRes<ContractCreateResult>;

        /// Send data to a contract.
        /// https://docs.qtum.site/en/Qtum-RPC-API/#sendtocontract
        fn send_to_contract(
            &self,
            contract_addr: H160Json,
            bytecode: &BytesJson,
            qtum_amount: u64,
            gas_limit: u64,
            gas_price: BigDecimal,
            from_addr: &str,
        ) -> RpcRes<SendToContractResult>;

        /// Send `transfer` contract call to the `token_addr`.
        /// This method uses [`Qrc20NativeWallerOps::send_to_contract`] to send the encoded contract call params.
        /// Note qtum_amount = 0, gas_limit = QRC20_GAS_LIMIT_DEFAULT, gas_price = QRC20_GAS_PRICE_DEFAULT will be used.
        fn transfer_tokens(
            &self,
            token_addr: &H160,
            from_addr: &str,
            to_addr: H160,
            amount: U256,
            decimals: u8,
        ) -> Box<dyn Future<Item = SendToContractResult, Error = String> + Send> {
            let token_addr = contract_addr_into_rpc_format(token_addr);
            let qtum_amount = 0;
            let gas_price = big_decimal_from_sat(QRC20_GAS_PRICE_DEFAULT as i64, decimals);

            let function = try_fus!(eth::ERC20_CONTRACT.function("transfer"));
            let params = try_fus!(function.encode_input(&[Token::Address(to_addr), Token::Uint(amount)]));
            Box::new(
                self.send_to_contract(
                    token_addr,
                    &params.into(),
                    qtum_amount,
                    QRC20_GAS_LIMIT_DEFAULT,
                    gas_price,
                    from_addr,
                )
                .map_err(|e| ERRL!("{}", e)),
            )
        }
    }

    impl Qrc20NativeWalletOps for NativeClient {
        fn create_contract(
            &self,
            bytecode: &BytesJson,
            gas_limit: u64,
            gas_price: BigDecimal,
            sender: &str,
        ) -> RpcRes<ContractCreateResult> {
            rpc_func!(self, "createcontract", bytecode, gas_limit, gas_price, sender)
        }

        fn send_to_contract(
            &self,
            contract_addr: H160Json,
            bytecode: &BytesJson,
            qtum_amount: u64,
            gas_limit: u64,
            gas_price: BigDecimal,
            sender: &str,
        ) -> RpcRes<SendToContractResult> {
            rpc_func!(
                self,
                "sendtocontract",
                contract_addr,
                bytecode,
                qtum_amount,
                gas_limit,
                gas_price,
                sender
            )
        }
    }
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct TokenInfo {
    pub name: String,
    pub decimals: u8,
    pub total_supply: f64,
    pub symbol: String,
}

#[derive(Debug, Deserialize)]
pub struct ExecutionResult {
    pub output: BytesJson,
}

#[derive(Debug, Deserialize)]
pub struct ContractCallResult {
    #[allow(dead_code)]
    address: H160Json,
    #[serde(rename = "executionResult")]
    pub execution_result: ExecutionResult,
}

#[derive(Debug, Deserialize)]
pub struct TxHistoryItem {
    pub tx_hash: H256Json,
    pub height: u64,
    pub log_index: u64,
}

/// Functions of ERC20/EtomicSwap smart contracts that don't change the blockchain state.
pub enum ViewContractCallType {
    /// Erc20 function.
    BalanceOf,
    /// Erc20 function.
    Allowance,
    /// Erc20 function.
    Decimals,
    /// EtomicSwap function.
    Payments,
}

impl ViewContractCallType {
    fn as_function_name(&self) -> &'static str {
        match self {
            ViewContractCallType::BalanceOf => "balanceOf",
            ViewContractCallType::Allowance => "allowance",
            ViewContractCallType::Decimals => "decimals",
            ViewContractCallType::Payments => "payments",
        }
    }

    fn as_function(&self) -> &'static Function {
        match self {
            ViewContractCallType::BalanceOf | ViewContractCallType::Allowance | ViewContractCallType::Decimals => {
                eth::ERC20_CONTRACT.function(self.as_function_name()).unwrap()
            },
            ViewContractCallType::Payments => eth::SWAP_CONTRACT.function(self.as_function_name()).unwrap(),
        }
    }
}

/// The structure is the same as Qtum Core RPC gettransactionreceipt returned data.
/// https://docs.qtum.site/en/Qtum-RPC-API/#gettransactionreceipt
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TxReceipt {
    /// Hash of the block this transaction was included within.
    #[serde(rename = "blockHash")]
    pub block_hash: H256Json,
    /// Number of the block this transaction was included within.
    #[serde(rename = "blockNumber")]
    pub block_number: u64,
    /// Transaction hash.
    #[serde(rename = "transactionHash")]
    pub transaction_hash: H256Json,
    /// Index within the block.
    #[serde(rename = "transactionIndex")]
    pub transaction_index: u64,
    /// Index within the outputs.
    #[serde(rename = "outputIndex")]
    pub output_index: u64,
    /// 20 bytes，the sender address of this tx.
    pub from: String,
    /// 20 bytes，the receiver address of this tx. if this  address is created by a contract, return null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// The total amount of gas used after execution of the current transaction.
    #[serde(rename = "cumulativeGasUsed")]
    pub cumulative_gas_used: u64,
    /// The gas cost alone to execute the current transaction.
    #[serde(rename = "gasUsed")]
    pub gas_used: u64,
    /// Contract address created, or `None` if not a deployment.
    #[serde(rename = "contractAddress")]
    pub contract_address: Option<String>,
    /// Logs generated within this transaction.
    pub log: Vec<LogEntry>,
    /// Whether corresponding contract call (specified in UTXO outputs[output_index]) was failed.
    /// If None or Some("None") - completed, else failed.
    pub excepted: Option<String>,
    #[serde(rename = "exceptedMessage")]
    pub excepted_message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LogEntry {
    /// Contract address.
    pub address: String,
    /// Vector of 0x-prefixed hex strings with length of 64.
    pub topics: Vec<String>,
    /// In other words the data means a transaction value.
    pub data: String,
}

impl LogEntry {
    pub fn parse_address(&self) -> Result<H160, String> {
        if self.address.starts_with("0x") {
            qtum::contract_addr_from_str(&self.address)
        } else {
            let address = format!("0x{}", self.address);
            qtum::contract_addr_from_str(&address)
        }
    }
}

#[derive(Clone, Debug)]
pub enum TopicFilter {
    Match(String),
    Skip,
}

impl From<&str> for TopicFilter {
    fn from(topic: &str) -> Self {
        TopicFilter::Match(topic.to_string())
    }
}

/// Qrc20 specific RPC ops
pub trait Qrc20ElectrumOps {
    /// This can be used to get the basic information(name, decimals, total_supply, symbol) of a QRC20 token.
    /// https://github.com/qtumproject/qtum-electrumx-server/blob/master/docs/qrc20-integration.md#blockchaintokenget_infotoken_address
    fn blockchain_token_get_info(&self, token_addr: &H160Json) -> RpcRes<TokenInfo>;

    fn blockchain_contract_call(&self, contract_addr: &H160Json, data: BytesJson) -> RpcRes<ContractCallResult>;

    /// This can be used to retrieve QRC20 token transfer history, params are the same as blockchain.contract.event.subscribe,
    /// and it returns a list of map{tx_hash, height, log_index}, where log_index is the position for this event log in its transaction.
    /// https://github.com/qtumproject/qtum-electrumx-server/blob/master/docs/qrc20-integration.md#blockchaincontracteventget_historyhash160-contract_addr-topic
    fn blockchain_contract_event_get_history(
        &self,
        address: &H160Json,
        contract_addr: &H160Json,
        topic: &str,
    ) -> RpcRes<Vec<TxHistoryItem>>;

    /// This can be used to get eventlogs in the transaction, the returned data is the same as Qtum Core RPC gettransactionreceipt.
    /// from the eventlogs, we can get QRC20 Token transafer informations(from, to, amount).
    /// https://github.com/qtumproject/qtum-electrumx-server/blob/master/docs/qrc20-integration.md#blochchaintransactionget_receipttxid
    fn blockchain_transaction_get_receipt(&self, hash: &H256Json) -> RpcRes<Vec<TxReceipt>>;
}

pub trait Qrc20NativeOps {
    /// https://docs.qtum.site/en/Qtum-RPC-API/#callcontract
    fn call_contract(&self, contract_addr: &H160Json, data: BytesJson) -> RpcRes<ContractCallResult>;

    /// Similar to [`Qrc20ElectrumOps::blochchain_transaction_get_receipt`]
    /// https://docs.qtum.site/en/Qtum-RPC-API/#gettransactionreceipt
    fn get_transaction_receipt(&self, hash: &H256Json) -> RpcRes<Vec<TxReceipt>>;

    /// This can be used to retrieve QRC20 transaction history.
    /// https://docs.qtum.site/en/Qtum-RPC-API/#searchlogs
    fn search_logs(
        &self,
        from_block: u64,
        to_block: Option<u64>,
        addresses: Vec<H160Json>,
        topics: Vec<TopicFilter>,
    ) -> UtxoRpcFut<Vec<TxReceipt>>;
}

impl Qrc20NativeOps for NativeClient {
    fn call_contract(&self, contract_addr: &H160Json, data: BytesJson) -> RpcRes<ContractCallResult> {
        rpc_func!(self, "callcontract", contract_addr, data)
    }

    fn get_transaction_receipt(&self, hash: &H256Json) -> RpcRes<Vec<TxReceipt>> {
        rpc_func!(self, "gettransactionreceipt", hash)
    }

    fn search_logs(
        &self,
        from_block: u64,
        to_block: Option<u64>,
        addresses: Vec<H160Json>,
        topics: Vec<TopicFilter>,
    ) -> UtxoRpcFut<Vec<TxReceipt>> {
        let to_block = to_block.map(|x| x as i64).unwrap_or(-1);
        let addr_block = json!({ "addresses": addresses });
        let topics: Vec<Json> = topics
            .into_iter()
            .map(|t| match t {
                TopicFilter::Match(s) => Json::String(s),
                TopicFilter::Skip => Json::Null,
            })
            .collect();
        let topic_block = json!({
            "topics": topics,
        });
        Box::new(
            rpc_func!(self, "searchlogs", from_block, to_block, addr_block, topic_block)
                .map_to_mm_fut(UtxoRpcError::from),
        )
    }
}

impl Qrc20ElectrumOps for ElectrumClient {
    fn blockchain_token_get_info(&self, token_addr: &H160Json) -> RpcRes<TokenInfo> {
        rpc_func!(self, "blockchain.token.get_info", token_addr)
    }

    fn blockchain_contract_call(&self, contract_addr: &H160Json, data: BytesJson) -> RpcRes<ContractCallResult> {
        let sender = "";
        rpc_func!(self, "blockchain.contract.call", contract_addr, data, sender)
    }

    fn blockchain_contract_event_get_history(
        &self,
        address: &H160Json,
        contract_addr: &H160Json,
        topic: &str,
    ) -> RpcRes<Vec<TxHistoryItem>> {
        rpc_func!(
            self,
            "blockchain.contract.event.get_history",
            address,
            contract_addr,
            topic
        )
    }

    fn blockchain_transaction_get_receipt(&self, hash: &H256Json) -> RpcRes<Vec<TxReceipt>> {
        rpc_func!(self, "blochchain.transaction.get_receipt", hash)
    }
}

pub trait Qrc20RpcOps {
    fn get_transaction_receipts(&self, tx_hash: &H256Json) -> RpcRes<Vec<TxReceipt>>;

    fn rpc_contract_call(
        &self,
        func: ViewContractCallType,
        contract_addr: &H160,
        tokens: &[Token],
    ) -> UtxoRpcFut<Vec<Token>>;

    fn token_decimals(&self, token_address: &H160) -> Box<dyn Future<Item = u8, Error = String> + Send>;
}

impl Qrc20RpcOps for UtxoRpcClientEnum {
    fn get_transaction_receipts(&self, tx_hash: &H256) -> RpcRes<Vec<TxReceipt>> {
        match self {
            UtxoRpcClientEnum::Electrum(electrum) => electrum.blockchain_transaction_get_receipt(tx_hash),
            UtxoRpcClientEnum::Native(native) => native.get_transaction_receipt(tx_hash),
        }
    }

    fn rpc_contract_call(
        &self,
        func: ViewContractCallType,
        contract_addr: &H160,
        tokens: &[Token],
    ) -> UtxoRpcFut<Vec<Token>> {
        let function = func.as_function().clone();
        let params = try_f!(function.encode_input(tokens).map_to_mm(UtxoRpcError::from));
        let contract_addr = contract_addr_into_rpc_format(contract_addr);

        let rpc_client = self.clone();
        let fut = async move {
            let fut = match rpc_client {
                UtxoRpcClientEnum::Native(native) => native.call_contract(&contract_addr, params.into()),
                UtxoRpcClientEnum::Electrum(electrum) => {
                    electrum.blockchain_contract_call(&contract_addr, params.into())
                },
            };
            let result = fut.compat().await?;
            let decoded = function.decode_output(&result.execution_result.output)?;
            Ok(decoded)
        };
        Box::new(fut.boxed().compat())
    }

    fn token_decimals(&self, token_address: &H160) -> Box<dyn Future<Item = u8, Error = String> + Send> {
        let fut = self
            .rpc_contract_call(ViewContractCallType::Decimals, token_address, &[])
            .map_err(|e| ERRL!("{}", e))
            .and_then(|tokens| {
                let decimals = match tokens.first() {
                    Some(Token::Uint(decimals)) => decimals.as_u64(),
                    Some(_) => return ERR!(r#"Expected Uint as "decimals" result but got {:?}"#, tokens),
                    None => return ERR!(r#"Expected Uint as "decimals" result but got nothing"#),
                };
                if decimals <= (u8::MAX as u64) {
                    Ok(decimals as u8)
                } else {
                    ERR!("decimals {} is not u8", decimals)
                }
            });
        Box::new(fut)
    }
}
