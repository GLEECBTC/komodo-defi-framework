use super::*;
use crate::eth::web3_transport::http_transport::de_rpc_response;
use crate::hd_wallet::AddrToString;
// use alloy::rpc::types::eth::erc4337::{SendUserOperation, SendUserOperationResponse};
// use alloy::sol_types::eip712_domain;
use mm2_net::transport::{slurp_post_json, SlurpResult};
use serde_json::{json, Value};
//use url::Url;

pub(crate) struct Eip4337Rpc;

impl Eip4337Rpc {

    async fn call_api(rpc_uri: &Uri, method: &str, params: &Value) -> SlurpResult {
        let req = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        });
        println!("Eip4337Rpc req: {}", serde_json::to_string_pretty(&req).unwrap());
        //post_json(self.api_url.as_str(), serde_json::to_string(&req)?).await
        slurp_post_json(&rpc_uri.to_string(), serde_json::to_string(&req)?).await
    }

    pub(crate) async fn send_user_operation(rpc_uri: &Uri, user_op: Value, entry_point: Address) -> Result<Value, web3::Error> {
        let result = Self::call_api(
            rpc_uri,
            "eth_sendUserOperation",
            &json!([
                &user_op,
                entry_point.addr_to_string()
            ])
        ).await;
        decode_rpc_result(result, rpc_uri)
    }
}



fn decode_rpc_result(result: SlurpResult, uri: &Uri) -> Result<Value, web3::Error> {
    match result {
        Ok((status, _, body)) => {
            if !status.is_success() {
                return Err(web3::Error::Transport(web3::error::TransportError::Code(status.as_u16())));
            }

            match de_rpc_response(body, &uri.to_string()) {
                Ok(val) => Ok(val),
                Err(err) => Err(web3::Error::InvalidResponse(
                    format!("Server: '{}', error: {}", uri, err)
                )),
            }
        },
        Err(err) => Err(web3::Error::Transport(
            web3::error::TransportError::Message(
                format!("Server: '{}', error: {}", uri, err.get_inner())
        ))),
    }
}