//! Minimal TRON transaction protobuf types.
//!
//! Hand-written `prost::Message` structs matching TRON's `Tron.proto`,
//! `balance_contract.proto`, and `smart_contract.proto` definitions.
//! Only the types needed for TRX transfers and TRC20 interactions are included.
//!
//! Field tags are non-sequential in several messages (notably `TransactionRaw`)
//! and must match the upstream TRON protocol exactly — signatures and transaction
//! IDs are computed over the raw protobuf bytes.

/// Type URL for `TransferContract` (native TRX transfer).
pub const TYPE_URL_TRANSFER_CONTRACT: &str = "type.googleapis.com/protocol.TransferContract";

/// Type URL for `TriggerSmartContract` (TRC20 / smart contract calls).
pub const TYPE_URL_TRIGGER_SMART_CONTRACT: &str = "type.googleapis.com/protocol.TriggerSmartContract";

/// Equivalent of `google.protobuf.Any`.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct Any {
    #[prost(string, tag = "1")]
    pub type_url: prost::alloc::string::String,
    #[prost(bytes = "vec", tag = "2")]
    pub value: prost::alloc::vec::Vec<u8>,
}

/// `protocol.TransferContract` — native TRX transfer.
///
/// Address fields are raw 21-byte TRON addresses (`0x41` prefix + 20-byte EVM address).
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct TransferContract {
    #[prost(bytes = "vec", tag = "1")]
    pub owner_address: prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub to_address: prost::alloc::vec::Vec<u8>,
    /// Amount in SUN (1 TRX = 1,000,000 SUN). Must be non-negative.
    #[prost(int64, tag = "3")]
    pub amount: i64,
}

/// `protocol.TriggerSmartContract` — smart contract invocation (TRC20 transfers, etc.).
///
/// Address fields are raw 21-byte TRON addresses (`0x41` prefix + 20-byte EVM address).
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct TriggerSmartContract {
    #[prost(bytes = "vec", tag = "1")]
    pub owner_address: prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub contract_address: prost::alloc::vec::Vec<u8>,
    /// TRX amount sent with the call, in SUN. Must be non-negative.
    #[prost(int64, tag = "3")]
    pub call_value: i64,
    /// ABI-encoded function call data.
    #[prost(bytes = "vec", tag = "4")]
    pub data: prost::alloc::vec::Vec<u8>,
    /// TRC10 token value sent with the call. Must be non-negative.
    #[prost(int64, tag = "5")]
    pub call_token_value: i64,
    /// TRC10 token ID. Must be non-negative.
    #[prost(int64, tag = "6")]
    pub token_id: i64,
}

/// Contract types used in `TransactionContract.type`.
///
/// The `Unspecified` variant (value 0) is required by proto3 convention and ensures
/// prost correctly encodes non-zero variants on the wire (prost skips encoding the
/// first variant as the "default").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum ContractType {
    Unspecified = 0,
    TransferContract = 1,
    TriggerSmartContract = 31,
}

/// `Transaction.Contract` — wraps a typed contract inside a transaction.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct TransactionContract {
    /// `ContractType` discriminant.
    #[prost(enumeration = "ContractType", tag = "1")]
    pub r#type: i32,
    /// The contract body serialized as `google.protobuf.Any`.
    #[prost(message, optional, tag = "2")]
    pub parameter: Option<Any>,
    #[prost(int32, tag = "5")]
    pub permission_id: i32,
}

/// `Transaction.raw` — the unsigned transaction payload.
///
/// **Tags are non-sequential** (1, 4, 8, 10, 11, 14, 18) matching `Tron.proto`.
/// Deprecated fields (`ref_block_num`=3, `auths`=9, `scripts`=12) are intentionally omitted.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct TransactionRaw {
    /// Last 2 bytes of the block number (TAPOS).
    #[prost(bytes = "vec", tag = "1")]
    pub ref_block_bytes: prost::alloc::vec::Vec<u8>,
    /// Bytes 8..16 of the block ID (TAPOS).
    #[prost(bytes = "vec", tag = "4")]
    pub ref_block_hash: prost::alloc::vec::Vec<u8>,
    /// Transaction expiration time in milliseconds since Unix epoch.
    #[prost(int64, tag = "8")]
    pub expiration: i64,
    /// Optional memo/data field.
    #[prost(bytes = "vec", tag = "10")]
    pub data: prost::alloc::vec::Vec<u8>,
    /// One or more contracts (typically exactly one).
    #[prost(message, repeated, tag = "11")]
    pub contract: prost::alloc::vec::Vec<TransactionContract>,
    /// Transaction creation time in milliseconds since Unix epoch.
    #[prost(int64, tag = "14")]
    pub timestamp: i64,
    /// Maximum TRX (in SUN) willing to spend on energy. 0 for native TRX transfers.
    /// Must be non-negative.
    #[prost(int64, tag = "18")]
    pub fee_limit: i64,
}

/// `protocol.Transaction` — a complete (optionally signed) TRON transaction.
#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct Transaction {
    #[prost(message, optional, tag = "1")]
    pub raw_data: Option<TransactionRaw>,
    /// Each entry is a 65-byte signature: `r(32) || s(32) || v(1)`.
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub signature: prost::alloc::vec::Vec<prost::alloc::vec::Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::tron::tx_builder::wrap_contract;
    use common::cross_test;
    use prost::Message;

    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::*;

    /// Dummy 21-byte TRON address (0x41 prefix + 20 bytes filled with `fill`).
    fn dummy_tron_address(fill: u8) -> Vec<u8> {
        let mut addr = vec![0x41];
        addr.extend_from_slice(&[fill; 20]);
        addr
    }

    // -----------------------------------------------------------------------
    // Roundtrip encode/decode tests
    // -----------------------------------------------------------------------

    cross_test!(any_roundtrip, {
        let original = Any {
            type_url: TYPE_URL_TRANSFER_CONTRACT.to_string(),
            value: vec![1, 2, 3, 4],
        };
        let bytes = original.encode_to_vec();
        let decoded = Any::decode(bytes.as_slice()).unwrap();
        assert_eq!(original, decoded);
    });

    cross_test!(transfer_contract_roundtrip, {
        let original = TransferContract {
            owner_address: dummy_tron_address(0xAA),
            to_address: dummy_tron_address(0xBB),
            amount: 1_000_000, // 1 TRX
        };
        let bytes = original.encode_to_vec();
        let decoded = TransferContract::decode(bytes.as_slice()).unwrap();
        assert_eq!(original, decoded);
    });

    cross_test!(trigger_smart_contract_roundtrip, {
        let original = TriggerSmartContract {
            owner_address: dummy_tron_address(0xAA),
            contract_address: dummy_tron_address(0xCC),
            call_value: 0,
            data: vec![0xa9, 0x05, 0x9c, 0xbb, 0x00, 0x01, 0x02, 0x03],
            call_token_value: 0,
            token_id: 0,
        };
        let bytes = original.encode_to_vec();
        let decoded = TriggerSmartContract::decode(bytes.as_slice()).unwrap();
        assert_eq!(original, decoded);
    });

    cross_test!(transaction_contract_with_nested_any_roundtrip, {
        let transfer = TransferContract {
            owner_address: dummy_tron_address(0xAA),
            to_address: dummy_tron_address(0xBB),
            amount: 5_000_000,
        };
        let any = Any {
            type_url: TYPE_URL_TRANSFER_CONTRACT.to_string(),
            value: transfer.encode_to_vec(),
        };
        let original = TransactionContract {
            r#type: ContractType::TransferContract as i32,
            parameter: Some(any),
            permission_id: 0,
        };
        let bytes = original.encode_to_vec();
        let decoded = TransactionContract::decode(bytes.as_slice()).unwrap();
        assert_eq!(original, decoded);

        // Also verify the nested Any.value decodes back to the original TransferContract.
        let inner = TransferContract::decode(decoded.parameter.unwrap().value.as_slice()).unwrap();
        assert_eq!(inner.amount, 5_000_000);
    });

    cross_test!(transaction_raw_non_sequential_tags_roundtrip, {
        let transfer = TransferContract {
            owner_address: dummy_tron_address(0xAA),
            to_address: dummy_tron_address(0xBB),
            amount: 10_000_000,
        };
        let contract = wrap_contract(
            ContractType::TransferContract,
            TYPE_URL_TRANSFER_CONTRACT,
            transfer.encode_to_vec(),
        );

        let original = TransactionRaw {
            ref_block_bytes: vec![0x12, 0x34],
            ref_block_hash: vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22],
            expiration: 1_700_000_060_000,
            data: Vec::new(),
            contract: vec![contract],
            timestamp: 1_700_000_000_000,
            fee_limit: 0,
        };
        let bytes = original.encode_to_vec();
        let decoded = TransactionRaw::decode(bytes.as_slice()).unwrap();
        assert_eq!(original, decoded);
    });

    cross_test!(full_transaction_roundtrip, {
        let transfer = TransferContract {
            owner_address: dummy_tron_address(0xAA),
            to_address: dummy_tron_address(0xBB),
            amount: 1_000_000,
        };
        let contract = wrap_contract(
            ContractType::TransferContract,
            TYPE_URL_TRANSFER_CONTRACT,
            transfer.encode_to_vec(),
        );
        let raw = TransactionRaw {
            ref_block_bytes: vec![0x56, 0x78],
            ref_block_hash: vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
            expiration: 1_700_000_060_000,
            data: Vec::new(),
            contract: vec![contract],
            timestamp: 1_700_000_000_000,
            fee_limit: 0,
        };

        // 65-byte placeholder signature (r || s || v).
        let fake_sig = vec![0xFFu8; 65];

        let original = Transaction {
            raw_data: Some(raw),
            signature: vec![fake_sig],
        };
        let bytes = original.encode_to_vec();
        let decoded = Transaction::decode(bytes.as_slice()).unwrap();
        assert_eq!(original, decoded);
    });

    cross_test!(trigger_smart_contract_transaction_roundtrip, {
        let trigger = TriggerSmartContract {
            owner_address: dummy_tron_address(0xAA),
            contract_address: dummy_tron_address(0xCC),
            call_value: 0,
            // Simulated transfer(address,uint256) ABI call: 4-byte selector + 64 bytes params.
            data: {
                let mut d = vec![0xa9, 0x05, 0x9c, 0xbb];
                d.extend_from_slice(&[0x00; 64]);
                d
            },
            call_token_value: 0,
            token_id: 0,
        };
        let contract = wrap_contract(
            ContractType::TriggerSmartContract,
            TYPE_URL_TRIGGER_SMART_CONTRACT,
            trigger.encode_to_vec(),
        );
        let raw = TransactionRaw {
            ref_block_bytes: vec![0xAB, 0xCD],
            ref_block_hash: vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80],
            expiration: 1_700_000_060_000,
            data: Vec::new(),
            contract: vec![contract],
            timestamp: 1_700_000_000_000,
            fee_limit: 100_000_000, // 100 TRX fee limit for energy.
        };
        let fake_sig = vec![0xAAu8; 65];
        let original = Transaction {
            raw_data: Some(raw),
            signature: vec![fake_sig],
        };
        let bytes = original.encode_to_vec();
        let decoded = Transaction::decode(bytes.as_slice()).unwrap();
        assert_eq!(original, decoded);
    });

    cross_test!(contract_type_values, {
        assert_eq!(ContractType::TransferContract as i32, 1);
        assert_eq!(ContractType::TriggerSmartContract as i32, 31);
    });

    cross_test!(default_transaction_raw_has_zero_fee_limit, {
        let raw = TransactionRaw::default();
        assert_eq!(raw.fee_limit, 0);
        assert_eq!(raw.expiration, 0);
        assert_eq!(raw.timestamp, 0);
        assert!(raw.contract.is_empty());
        assert!(raw.ref_block_bytes.is_empty());
        assert!(raw.ref_block_hash.is_empty());
    });

    // -----------------------------------------------------------------------
    // Golden vector tests — real TRON transactions from developer docs.
    // These validate wire-format compatibility against known-good raw_data_hex
    // produced by TRON nodes, ensuring field tags and types are correct.
    // -----------------------------------------------------------------------

    // Golden vector: TransferContract (native TRX transfer).
    // Source: https://developers.tron.network/docs/tron-protocol-transaction
    cross_test!(golden_vector_transfer_contract, {
        let raw_data_hex = concat!(
            "0a020add",                               // ref_block_bytes: 0add
            "22086c2763abadf9ed29",                   // ref_block_hash: 6c2763abadf9ed29
            "40c8d5deea822e",                         // expiration: 1581308685000
            "5a65",                                   // contract (length-delimited)
            "0801",                                   //   type: 1 (TransferContract)
            "1261",                                   //   parameter (Any, length-delimited)
            "0a2d",                                   //     type_url (length-delimited)
            "747970652e676f6f676c65617069732e636f6d", //   "type.googleapis.com"
            "2f70726f746f636f6c2e",                   //     "/protocol."
            "5472616e73666572436f6e7472616374",       //     "TransferContract"
            "1230",                                   //     value (length-delimited)
            "0a15",                                   //       owner_address
            "418840e6c55b9ada326d211d818c34a994aeced808",
            "1215", //       to_address
            "41d3136787e667d1e055d2cd5db4b5f6c880563049",
            "1864",           //       amount: 100
            "70ac89dbea822e", // timestamp: 1581308626092
        );
        let raw_bytes = hex::decode(raw_data_hex).unwrap();

        // Decode and verify every field.
        let raw = TransactionRaw::decode(raw_bytes.as_slice()).unwrap();
        assert_eq!(hex::encode(&raw.ref_block_bytes), "0add");
        assert_eq!(hex::encode(&raw.ref_block_hash), "6c2763abadf9ed29");
        assert_eq!(raw.expiration, 1_581_308_685_000);
        assert_eq!(raw.timestamp, 1_581_308_626_092);
        assert_eq!(raw.fee_limit, 0);
        assert!(raw.data.is_empty());
        assert_eq!(raw.contract.len(), 1);

        let contract = &raw.contract[0];
        assert_eq!(contract.r#type, ContractType::TransferContract as i32);
        assert_eq!(contract.permission_id, 0);

        let any = contract.parameter.as_ref().unwrap();
        assert_eq!(any.type_url, TYPE_URL_TRANSFER_CONTRACT);

        let transfer = TransferContract::decode(any.value.as_slice()).unwrap();
        assert_eq!(
            hex::encode(&transfer.owner_address),
            "418840e6c55b9ada326d211d818c34a994aeced808"
        );
        assert_eq!(
            hex::encode(&transfer.to_address),
            "41d3136787e667d1e055d2cd5db4b5f6c880563049"
        );
        assert_eq!(transfer.amount, 100);

        // Re-encode must produce identical bytes (canonical protobuf encoding).
        assert_eq!(raw.encode_to_vec(), raw_bytes);
    });

    // Golden vector: TriggerSmartContract (TRC20 token transfer).
    // Source: https://developers.tron.network/docs/smart-contract-deployment-and-invocation
    cross_test!(golden_vector_trigger_smart_contract, {
        let raw_data_hex = concat!(
            "0a021c51",                                 // ref_block_bytes: 1c51
            "220874912b480b7b887c",                     // ref_block_hash: 74912b480b7b887c
            "40c8d2e7e78a30",                           // expiration: 1652169501000
            "5aae01",                                   // contract (length-delimited)
            "081f",                                     //   type: 31 (TriggerSmartContract)
            "12a901",                                   //   parameter (Any, length-delimited)
            "0a31",                                     //     type_url (length-delimited)
            "747970652e676f6f676c65617069732e636f6d",   //   "type.googleapis.com"
            "2f70726f746f636f6c2e",                     //     "/protocol."
            "54726967676572536d617274436f6e7472616374", // "TriggerSmartContract"
            "1274",                                     //     value (length-delimited)
            "0a15",                                     //       owner_address
            "41977c20977f412c2a1aa4ef3d49fee5ec4c31cdfb",
            "1215", //       contract_address
            "419e62be7f4f103c36507cb2a753418791b1cdc182",
            "2244",     //       data (68 bytes)
            "a9059cbb", //         selector: transfer(address,uint256)
            "00000000000000000000004115208ef33a926919ed270e2fa61367b2da3753da",
            "0000000000000000000000000000000000000000000000000000000000000032",
            "70b286e4e78a30", // timestamp: 1652169442098
            "900180c2d72f",   // fee_limit: 100000000
        );
        let raw_bytes = hex::decode(raw_data_hex).unwrap();

        // Decode and verify every field.
        let raw = TransactionRaw::decode(raw_bytes.as_slice()).unwrap();
        assert_eq!(hex::encode(&raw.ref_block_bytes), "1c51");
        assert_eq!(hex::encode(&raw.ref_block_hash), "74912b480b7b887c");
        assert_eq!(raw.expiration, 1_652_169_501_000);
        assert_eq!(raw.timestamp, 1_652_169_442_098);
        assert_eq!(raw.fee_limit, 100_000_000);
        assert!(raw.data.is_empty());
        assert_eq!(raw.contract.len(), 1);

        let contract = &raw.contract[0];
        assert_eq!(contract.r#type, ContractType::TriggerSmartContract as i32);
        assert_eq!(contract.permission_id, 0);

        let any = contract.parameter.as_ref().unwrap();
        assert_eq!(any.type_url, TYPE_URL_TRIGGER_SMART_CONTRACT);

        let trigger = TriggerSmartContract::decode(any.value.as_slice()).unwrap();
        assert_eq!(
            hex::encode(&trigger.owner_address),
            "41977c20977f412c2a1aa4ef3d49fee5ec4c31cdfb"
        );
        assert_eq!(
            hex::encode(&trigger.contract_address),
            "419e62be7f4f103c36507cb2a753418791b1cdc182"
        );
        assert_eq!(trigger.call_value, 0);
        assert_eq!(trigger.call_token_value, 0);
        assert_eq!(trigger.token_id, 0);

        // Verify ABI-encoded data: transfer(address,uint256).
        let expected_data = concat!(
            "a9059cbb",                                                         // selector
            "00000000000000000000004115208ef33a926919ed270e2fa61367b2da3753da", // address param
            "0000000000000000000000000000000000000000000000000000000000000032", // uint256 = 50
        );
        assert_eq!(hex::encode(&trigger.data), expected_data);

        // Re-encode must produce identical bytes (canonical protobuf encoding).
        assert_eq!(raw.encode_to_vec(), raw_bytes);
    });
}
