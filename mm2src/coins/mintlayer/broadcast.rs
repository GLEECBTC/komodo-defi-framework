use mintlayer_sdk::node::{Client as MintlayerNodeClient, TrustPolicy};
use thiserror::Error;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum MintlayerBroadcastError {
    #[error("Mintlayer signed transaction hex is empty")]
    EmptyTransaction,
    #[error("Invalid Mintlayer signed transaction hex: {0}")]
    InvalidHex(String),
    #[error("Mintlayer node rejected transaction submission: {0}")]
    Node(String),
}

/// Submits an already-signed Mintlayer transaction through the official SDK.
///
/// Transaction construction and signing deliberately remain outside this
/// adapter. `TrustPolicy::Trusted` requires full validation against the
/// node's current chainstate before the transaction is propagated to peers.
pub async fn broadcast_signed_transaction_hex(
    client: &MintlayerNodeClient,
    transaction_hex: &str,
) -> Result<(), MintlayerBroadcastError> {
    if transaction_hex.is_empty() {
        return Err(MintlayerBroadcastError::EmptyTransaction);
    }

    hex::decode(transaction_hex).map_err(|error| MintlayerBroadcastError::InvalidHex(error.to_string()))?;
    client
        .broadcast_transaction(transaction_hex, TrustPolicy::Trusted)
        .await
        .map_err(|error| MintlayerBroadcastError::Node(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::Duration;

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build Tokio test runtime")
            .block_on(future)
    }

    fn content_length(headers: &str) -> usize {
        headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .expect("JSON-RPC request has Content-Length")
    }

    fn read_json_rpc_request(stream: &mut TcpStream) -> Value {
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 2048];
        let header_end;

        loop {
            let read = stream.read(&mut buffer).expect("read JSON-RPC request");
            assert!(read > 0, "connection closed before request headers");
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                header_end = position + 4;
                break;
            }
        }

        let headers = std::str::from_utf8(&bytes[..header_end]).expect("HTTP headers are UTF-8");
        let body_length = content_length(headers);
        while bytes.len() < header_end + body_length {
            let read = stream.read(&mut buffer).expect("read JSON-RPC body");
            assert!(read > 0, "connection closed before request body");
            bytes.extend_from_slice(&buffer[..read]);
        }

        serde_json::from_slice(&bytes[header_end..header_end + body_length]).expect("valid JSON-RPC request")
    }

    fn json_rpc_server(error_message: Option<&str>) -> (String, Receiver<Value>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let error_message = error_message.map(str::to_owned);
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept SDK request");
            let request = read_json_rpc_request(&mut stream);
            let id = request.get("id").cloned().unwrap_or(Value::Null);
            let response = match error_message {
                Some(message) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32000, "message": message }
                }),
                None => json!({ "jsonrpc": "2.0", "id": id, "result": null }),
            };
            let body = serde_json::to_vec(&response).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
            stream.flush().unwrap();
            sender.send(request).unwrap();
        });

        (format!("http://{address}"), receiver)
    }

    #[test]
    fn submits_signed_hex_through_p2p_rpc_with_trusted_policy() {
        let (endpoint, requests) = json_rpc_server(None);
        let client = MintlayerNodeClient::new(endpoint);

        block_on(broadcast_signed_transaction_hex(&client, "00aa11ff")).unwrap();
        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();

        assert_eq!(request["method"], "p2p_submit_transaction");
        assert_eq!(request["params"]["tx"], "00aa11ff");
        assert_eq!(request["params"]["options"]["trust_policy"], "Trusted");
    }

    #[test]
    fn preserves_node_rejection_as_an_error() {
        let (endpoint, requests) = json_rpc_server(Some("transaction rejected by loopback node"));
        let client = MintlayerNodeClient::new(endpoint);

        let error = block_on(broadcast_signed_transaction_hex(&client, "00aa11ff")).unwrap_err();
        let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();

        assert_eq!(request["method"], "p2p_submit_transaction");
        assert!(error.to_string().contains("transaction rejected by loopback node"));
    }

    #[test]
    fn rejects_invalid_hex_before_contacting_the_node() {
        let client = MintlayerNodeClient::new("http://127.0.0.1:9");

        assert_eq!(
            block_on(broadcast_signed_transaction_hex(&client, "")),
            Err(MintlayerBroadcastError::EmptyTransaction)
        );
        assert!(matches!(
            block_on(broadcast_signed_transaction_hex(&client, "not-hex")),
            Err(MintlayerBroadcastError::InvalidHex(_))
        ));
    }
}
