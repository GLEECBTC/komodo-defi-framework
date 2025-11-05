use async_std::prelude::FutureExt;
use base64::{engine::general_purpose::STANDARD, Engine};
use bincode::serialize;
use compatible_time::Duration;
use mm2_net::transport::slurp_post_json;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use solana_account::Account;
use solana_account_decoder_client_types::{token::UiTokenAmount, UiAccount, UiAccountEncoding};
use solana_hash::Hash;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_rpc_client_types::{
    config::RpcTokenAccountsFilter,
    request::RpcRequest,
    response::{Response, RpcBlockhash},
};
use solana_signature::Signature;
use solana_transaction::Transaction;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

type ClientResult<T> = Result<T, ClientError>;

#[derive(Debug)]
pub(crate) struct RpcClient {
    url: String,
    request_id: AtomicU64,
}

#[derive(Debug, Error)]
#[error("{kind}")]
pub(crate) struct ClientError {
    pub(crate) kind: ClientErrorKind,
}

impl From<ClientErrorKind> for ClientError {
    fn from(kind: ClientErrorKind) -> Self {
        ClientError { kind }
    }
}

#[derive(Debug, Error)]
pub(crate) enum ClientErrorKind {
    #[error("Transport error: {0}")]
    Transport(String),
    #[error("RPC error {0}")]
    Rpc(#[from] RpcResponseError),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("{0}")]
    Custom(String),
}

#[derive(Debug, Error)]
#[error("{message} (code {code})")]
pub(crate) struct RpcResponseError {
    pub(crate) code: i64,
    pub(crate) message: String,
    pub(crate) data: Option<Value>,
}

impl RpcClient {
    pub(crate) fn new(url: String) -> Self {
        RpcClient {
            url,
            request_id: AtomicU64::new(0),
        }
    }

    pub(crate) async fn get_health(&self) -> ClientResult<()> {
        let result: String = self.send(RpcRequest::GetHealth, json!([])).await?;

        if result == "ok" {
            Ok(())
        } else {
            Err(ClientErrorKind::Custom(format!("Health check failed: {result}")).into())
        }
    }

    pub(crate) async fn get_token_accounts_by_owner(
        &self,
        owner: &Pubkey,
        filter: RpcTokenAccountsFilter,
    ) -> ClientResult<()> {
        self.send::<Value>(
            RpcRequest::GetTokenAccountsByOwner,
            json!([owner.to_string(), filter, { "encoding": "jsonParsed" }]),
        )
        .await
        .map(|_| ())
    }

    pub(crate) async fn get_token_account_balance(&self, account: &Pubkey) -> ClientResult<UiTokenAmount> {
        let response: Response<UiTokenAmount> = self
            .send(RpcRequest::GetTokenAccountBalance, json!([account.to_string()]))
            .await?;
        Ok(response.value)
    }

    pub(crate) async fn get_latest_blockhash(&self) -> ClientResult<Hash> {
        let response: Response<RpcBlockhash> = self.send(RpcRequest::GetLatestBlockhash, json!([])).await?;
        Hash::from_str(&response.value.blockhash)
            .map_err(|e| ClientErrorKind::Parse(format!("Invalid blockhash: {e}")).into())
    }

    pub(crate) async fn get_fee_for_message(&self, message: &Message) -> ClientResult<u64> {
        let encoded = encode_bincode_base64(message)?;
        let response: Response<Option<u64>> = self.send(RpcRequest::GetFeeForMessage, json!([encoded])).await?;
        let value = response.value;
        value.ok_or_else(|| ClientErrorKind::Custom("Fee unavailable for provided message".to_string()).into())
    }

    pub(crate) async fn get_balance(&self, address: &Pubkey) -> ClientResult<u64> {
        let response: Response<u64> = self.send(RpcRequest::GetBalance, json!([address.to_string()])).await?;
        Ok(response.value)
    }

    pub(crate) async fn get_account(&self, address: &Pubkey) -> ClientResult<Account> {
        let response = self
            .send::<Response<Option<UiAccount>>>(
                RpcRequest::GetAccountInfo,
                json!([
                    address.to_string(),
                    {
                        "encoding": UiAccountEncoding::Base64,
                    }
                ]),
            )
            .await?;

        let ui_account = response
            .value
            .ok_or_else(|| ClientErrorKind::Custom(format!("AccountNotFound: pubkey={address}")))?;

        ui_account
            .decode()
            .ok_or_else(|| ClientErrorKind::Parse(format!("Failed to decode account data for pubkey {address}")).into())
    }

    pub(crate) async fn get_minimum_balance_for_rent_exemption(&self, data_len: usize) -> ClientResult<u64> {
        self.send(RpcRequest::GetMinimumBalanceForRentExemption, json!([data_len]))
            .await
    }

    pub(crate) async fn send_transaction(&self, transaction: &Transaction) -> ClientResult<Signature> {
        let encoded = encode_bincode_base64(transaction)?;
        let signature: String = self
            .send(
                RpcRequest::SendTransaction,
                json!([
                    encoded,
                    {
                        "encoding": "base64"
                    }
                ]),
            )
            .await?;

        Signature::from_str(&signature).map_err(|e| ClientErrorKind::Parse(format!("Invalid signature: {e}")).into())
    }

    pub(crate) async fn get_block_height(&self) -> ClientResult<u64> {
        self.send(RpcRequest::GetBlockHeight, json!([])).await
    }

    async fn send<T>(&self, request: RpcRequest, params: Value) -> ClientResult<T>
    where
        T: DeserializeOwned,
    {
        let id = self.next_request_id();
        let payload = request.build_request_json(id, params);

        let (status_code, _, response_bytes) = slurp_post_json(&self.url, payload.to_string())
            .timeout(Duration::from_secs(5))
            .await
            .map_err(|e| ClientError::from(ClientErrorKind::Transport(e.to_string())))?
            .map_err(|e| ClientError::from(ClientErrorKind::Transport(e.to_string())))?;

        if !status_code.is_success() {
            return Err(ClientErrorKind::Transport(format!(
                "Expected 200, got '{status_code}' status code from '{}' node. Payload: '{payload}'",
                self.url
            ))
            .into());
        }

        let response: Value = serde_json::from_slice(&response_bytes)
            .map_err(|e| ClientError::from(ClientErrorKind::Parse(e.to_string())))?;

        if let Some(error) = response.get("error") {
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string();
            let data = error.get("data").cloned();
            return Err(ClientErrorKind::Rpc(RpcResponseError { code, message, data }).into());
        }

        let result = response.get("result").cloned().ok_or_else(|| {
            ClientError::from(ClientErrorKind::Parse(format!(
                "Missing result field in response for {}",
                request
            )))
        })?;

        serde_json::from_value(result).map_err(|e| {
            ClientError::from(ClientErrorKind::Parse(format!(
                "Failed to parse response for {}: {e}",
                request
            )))
        })
    }

    fn next_request_id(&self) -> u64 {
        self.request_id.fetch_add(1, Ordering::Relaxed)
    }
}

fn encode_bincode_base64<T: Serialize + std::fmt::Debug>(value: &T) -> ClientResult<String> {
    serialize(value)
        .map(|bytes| STANDARD.encode(bytes))
        .map_err(|e| ClientError::from(ClientErrorKind::Parse(format!("{e}: failed to serialize: {value:?}"))))
}
