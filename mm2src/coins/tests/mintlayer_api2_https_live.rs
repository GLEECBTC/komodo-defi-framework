#![cfg(not(target_arch = "wasm32"))]

use coins::mintlayer::{canonical_transaction_id_from_signed_bytes, MintlayerApiClient};
use common::block_on;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

const API2: &str = "https://mintlayer-api2.pinco-labs.com";
const TXID: &str = "19266863b91c8433e19fd6a6ecac827e2c64b1abd5c3a22cdf667255c8655828";
const GENESIS: &str = "2cf01f196066bb6f3a4856deb7999294ff520f633fe48e118e8044390e409870";
const ASCII_SHA: &str = "97e92615f20e0cdb85425e4cb1aa247e1fcb10b1791e1fc86577973ce8920486";
const RAW_SHA: &str = "5ffd6c18f496e22b31bbeca76b585723564a8a4e41569cd4be05e1dcfaba408a";

fn check_golden(client: MintlayerApiClient) {
    let tx = block_on(client.transaction_with_tx_hex(TXID)).expect("KDF must obtain golden tx_hex");
    assert_eq!(tx.id, TXID);
    assert!(!tx.block_id.is_empty());
    let encoded = tx.tx_hex.expect("tx_hex must be present");
    assert_eq!(encoded.len(), 420);
    assert_eq!(hex::encode(Sha256::digest(encoded.as_bytes())), ASCII_SHA);
    let raw = hex::decode(&encoded).expect("tx_hex must decode");
    assert_eq!(raw.len(), 210);
    assert_eq!(hex::encode(Sha256::digest(&raw)), RAW_SHA);
    let canonical = canonical_transaction_id_from_signed_bytes(&raw).expect("SDK canonical txid");
    assert_eq!(canonical, TXID);
    println!("GOLDEN: 210 bytes; ASCII SHA256, RAW SHA256 and canonical txid PASS");
}

#[test]
#[ignore = "Opt-in public HTTPS GETs to Pinco API2; no transaction submission"]
fn mintlayer_api2_only_live_golden() {
    let client = MintlayerApiClient::new(vec![Url::parse(API2).unwrap()]);
    let genesis = block_on(client.genesis()).expect("API2 mainnet genesis through KDF");
    assert_eq!(genesis.block_id, GENESIS);
    check_golden(client);
    println!("API2_ONLY: PASS via KDF production HTTP transport");
}

// A temporary loopback server provides a deterministic unavailable primary.
// It accepts exactly the expected GET and returns HTTP 503. API1 is never contacted.
fn unavailable_primary() -> (Url, thread::JoinHandle<Result<(), String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind temporary loopback port");
    let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err("KDF did not contact the simulated primary".into());
                    }
                    thread::sleep(Duration::from_millis(20));
                },
                Err(error) => return Err(error.to_string()),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| e.to_string())?;
        let mut request = Vec::new();
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let mut buffer = [0_u8; 1024];
            let count = stream.read(&mut buffer).map_err(|e| e.to_string())?;
            if count == 0 || request.len() + count > 8192 {
                return Err("Incomplete or oversized request headers".into());
            }
            request.extend_from_slice(&buffer[..count]);
        }
        let expected = format!("GET /api/v2/transaction/{TXID} HTTP/1.1\r\n");
        if !request.starts_with(expected.as_bytes()) {
            return Err("Unexpected request: only the golden transaction GET is allowed".into());
        }
        stream
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .map_err(|e| e.to_string())?;
        Ok(())
    });
    (url, worker)
}

#[test]
#[ignore = "Opt-in loopback 503 plus real public HTTPS GET to Pinco API2"]
fn mintlayer_api2_after_unavailable_primary_live_golden() {
    let (primary, worker) = unavailable_primary();
    let client = MintlayerApiClient::new(vec![primary, Url::parse(API2).unwrap()]);
    check_golden(client);
    worker
        .join()
        .expect("primary server thread")
        .expect("primary received GET and returned 503");
    println!("FAILOVER: simulated primary GET -> HTTP 503 -> real API2 HTTPS -> golden PASS");
}
