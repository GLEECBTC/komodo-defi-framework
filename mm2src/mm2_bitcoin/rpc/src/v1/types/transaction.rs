use super::bytes::Bytes;
use super::hash::H256;
use super::script::ScriptType;
use keys::Address;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Hex-encoded transaction
pub type RawTransaction = Bytes;

/// Transaction input
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct TransactionInput {
    /// Previous transaction id
    pub txid: H256,
    /// Previous transaction output index
    pub vout: u32,
    /// Sequence number
    pub sequence: Option<u32>,
}

/// Transaction output of form "address": amount
#[derive(Debug, PartialEq)]
pub struct TransactionOutputWithAddress {
    /// Receiver' address
    pub address: Address,
    /// Amount in BTC
    pub amount: f64,
}

/// Transaction output of form "data": serialized(output script data)
#[derive(Debug, PartialEq)]
pub struct TransactionOutputWithScriptData {
    /// Serialized script data
    pub script_data: Bytes,
}

/// Transaction input script
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TransactionInputScript {
    /// Script code
    pub asm: String,
    /// Script hex
    pub hex: Bytes,
}

/// Transaction output script
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TransactionOutputScript {
    /// Script code
    pub asm: String,
    /// Script hex
    pub hex: Bytes,
    /// Number of required signatures
    #[serde(rename = "reqSigs")]
    #[serde(default)]
    pub req_sigs: u32,
    /// Type of script
    #[serde(rename = "type")]
    pub script_type: ScriptType,
    /// Array of bitcoin addresses
    #[serde(default)]
    pub addresses: Vec<String>,
}

impl TransactionOutputScript {
    pub fn is_empty(&self) -> bool {
        self.asm.is_empty() && self.hex.is_empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum TransactionInputEnum {
    Signed(SignedTransactionInput),
    Coinbase(CoinbaseTransactionInput),
    /// FIRO specific
    Sigma(SigmaInput),
    /// FIRO specific
    Lelantus(LelantusInput),
    /// FIRO specific
    Spark(SparkInput),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SigmaInput {
    #[serde(rename = "anonymityGroup")]
    anonymity_group: i64,
    #[serde(rename = "scriptSig")]
    pub script_sig: TransactionInputScript,
    value: f64,
    #[serde(rename = "valueSat")]
    value_sat: u64,
    sequence: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LelantusInput {
    #[serde(rename = "scriptSig")]
    pub script_sig: TransactionInputScript,
    #[serde(rename = "nFees")]
    pub n_fees: f64,
    serials: Vec<String>,
    sequence: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SparkInput {
    #[serde(rename = "scriptSig")]
    pub script_sig: TransactionInputScript,
    #[serde(rename = "nFees")]
    pub n_fees: f64,
    #[serde(rename = "lTags")]
    l_tags: Vec<String>,
    sequence: u32,
}

impl TransactionInputEnum {
    pub fn is_coinbase(&self) -> bool {
        matches!(self, TransactionInputEnum::Coinbase(_))
    }
}

/// Signed transaction input
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SignedTransactionInput {
    /// Previous transaction id
    pub txid: H256,
    /// Previous transaction output index
    pub vout: u32,
    /// Input script
    #[serde(rename = "scriptSig")]
    pub script_sig: TransactionInputScript,
    /// Sequence number
    pub sequence: u32,
    /// Hex-encoded witness data (if any)
    pub txinwitness: Option<Vec<String>>,
}

/// Coinbase transaction input
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CoinbaseTransactionInput {
    /// coinbase
    pub coinbase: Bytes,
    /// Sequence number
    pub sequence: u32,
}

/// Signed transaction output
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SignedTransactionOutput {
    /// Output value in BTC
    pub value: Option<f64>,
    /// Output index
    pub n: u32,
    /// Output script
    #[serde(rename = "scriptPubKey")]
    pub script: TransactionOutputScript,
}

impl SignedTransactionOutput {
    pub fn is_empty(&self) -> bool {
        self.value == Some(0.0) && self.script.is_empty()
    }
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// Transaction
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Transaction {
    /// Raw transaction
    pub hex: RawTransaction,
    /// The transaction id (same as provided)
    pub txid: H256,
    /// The transaction hash (differs from txid for witness transactions)
    pub hash: Option<H256>,
    /// The serialized transaction size
    pub size: Option<usize>,
    /// The virtual transaction size (differs from size for witness transactions)
    pub vsize: Option<usize>,
    /// The version
    pub version: i32,
    /// The lock time
    pub locktime: u32,
    /// Transaction inputs
    pub vin: Vec<TransactionInputEnum>,
    /// Transaction outputs
    pub vout: Vec<SignedTransactionOutput>,
    /// Hash of the block this transaction is included in
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_null_default")]
    pub blockhash: H256,
    /// Number of confirmations of this transaction
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_null_default")]
    pub confirmations: u32,
    /// Number of rawconfirmations of this transaction, KMD specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rawconfirmations: Option<u32>,
    /// The transaction time in seconds since epoch (Jan 1 1970 GMT)
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_null_default")]
    pub time: u32,
    /// The block time in seconds since epoch (Jan 1 1970 GMT)
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_null_default")]
    pub blocktime: u32,
    /// The block height transaction mined in
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u64>,
}

impl Transaction {
    pub fn is_coinbase(&self) -> bool {
        self.vin.iter().any(|input| input.is_coinbase())
    }
}

/// Return value of `getrawtransaction` method
#[derive(Debug, PartialEq)]
pub enum GetRawTransactionResponse {
    /// Return value when asking for raw transaction
    Raw(RawTransaction),
    /// Return value when asking for verbose transaction
    Verbose(Box<Transaction>),
}

impl Serialize for GetRawTransactionResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            GetRawTransactionResponse::Raw(ref raw_transaction) => raw_transaction.serialize(serializer),
            GetRawTransactionResponse::Verbose(ref verbose_transaction) => verbose_transaction.serialize(serializer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::bytes::Bytes;
    use super::super::hash::H256;
    use super::super::script::ScriptType;
    use super::*;
    use lazy_static::lazy_static;
    use serde_json;
    use std::collections::HashMap;

    const TRANSACTIONS_STR: &str = include_str!("../../for_tests/txTestVectors.json");

    lazy_static! {
        static ref TRANSACTIONS_MAP: HashMap<String, serde_json::Value> = parse_transactions();
    }

    fn parse_transactions() -> HashMap<String, serde_json::Value> {
        serde_json::from_str(TRANSACTIONS_STR).unwrap()
    }

    fn get_transaction_json(key: &str) -> serde_json::Value {
        TRANSACTIONS_MAP.get(key).cloned().unwrap()
    }

    #[test]
    fn transaction_input_serialize() {
        let txinput = TransactionInput {
            txid: H256::from(7),
            vout: 33,
            sequence: Some(88),
        };
        assert_eq!(
            serde_json::to_string(&txinput).unwrap(),
            r#"{"txid":"0700000000000000000000000000000000000000000000000000000000000000","vout":33,"sequence":88}"#
        );
    }

    #[test]
    fn transaction_input_deserialize() {
        let txinput = TransactionInput {
            txid: H256::from(7),
            vout: 33,
            sequence: Some(88),
        };

        assert_eq!(
            serde_json::from_str::<TransactionInput>(
                r#"{"txid":"0700000000000000000000000000000000000000000000000000000000000000","vout":33,"sequence":88}"#
            )
            .unwrap(),
            txinput
        );
    }

    #[test]
    fn transaction_input_script_serialize() {
        let txin = TransactionInputScript {
            asm: "Hello, world!!!".to_owned(),
            hex: Bytes::new(vec![1, 2, 3, 4]),
        };
        assert_eq!(
            serde_json::to_string(&txin).unwrap(),
            r#"{"asm":"Hello, world!!!","hex":"01020304"}"#
        );
    }

    #[test]
    fn transaction_input_script_deserialize() {
        let txin = TransactionInputScript {
            asm: "Hello, world!!!".to_owned(),
            hex: Bytes::new(vec![1, 2, 3, 4]),
        };
        assert_eq!(
            serde_json::from_str::<TransactionInputScript>(r#"{"asm":"Hello, world!!!","hex":"01020304"}"#).unwrap(),
            txin
        );
    }

    #[test]
    fn transaction_output_script_serialize() {
        let txout = TransactionOutputScript {
            asm: "Hello, world!!!".to_owned(),
            hex: Bytes::new(vec![1, 2, 3, 4]),
            req_sigs: 777,
            script_type: ScriptType::Multisig,
            addresses: vec![
                "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".into(),
                "1H5m1XzvHsjWX3wwU781ubctznEpNACrNC".into(),
            ],
        };
        assert_eq!(
            serde_json::to_string(&txout).unwrap(),
            r#"{"asm":"Hello, world!!!","hex":"01020304","reqSigs":777,"type":"multisig","addresses":["1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa","1H5m1XzvHsjWX3wwU781ubctznEpNACrNC"]}"#
        );
    }

    #[test]
    fn transaction_output_script_deserialize() {
        let txout = TransactionOutputScript {
            asm: "Hello, world!!!".to_owned(),
            hex: Bytes::new(vec![1, 2, 3, 4]),
            req_sigs: 777,
            script_type: ScriptType::Multisig,
            addresses: vec![
                "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".into(),
                "1H5m1XzvHsjWX3wwU781ubctznEpNACrNC".into(),
            ],
        };

        assert_eq!(
			serde_json::from_str::<TransactionOutputScript>(r#"{"asm":"Hello, world!!!","hex":"01020304","reqSigs":777,"type":"multisig","addresses":["1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa","1H5m1XzvHsjWX3wwU781ubctznEpNACrNC"]}"#).unwrap(),
			txout);
    }

    #[test]
    fn signed_transaction_input_serialize() {
        let txin = SignedTransactionInput {
            txid: H256::from(77),
            vout: 13,
            script_sig: TransactionInputScript {
                asm: "Hello, world!!!".to_owned(),
                hex: Bytes::new(vec![1, 2, 3, 4]),
            },
            sequence: 123,
            txinwitness: None,
        };
        assert_eq!(
            serde_json::to_string(&txin).unwrap(),
            r#"{"txid":"4d00000000000000000000000000000000000000000000000000000000000000","vout":13,"scriptSig":{"asm":"Hello, world!!!","hex":"01020304"},"sequence":123,"txinwitness":null}"#
        );
    }

    #[test]
    fn signed_transaction_input_deserialize() {
        let txin = SignedTransactionInput {
            txid: H256::from(77),
            vout: 13,
            script_sig: TransactionInputScript {
                asm: "Hello, world!!!".to_owned(),
                hex: Bytes::new(vec![1, 2, 3, 4]),
            },
            sequence: 123,
            txinwitness: Some(vec![]),
        };
        assert_eq!(
			serde_json::from_str::<SignedTransactionInput>(r#"{"txid":"4d00000000000000000000000000000000000000000000000000000000000000","vout":13,"scriptSig":{"asm":"Hello, world!!!","hex":"01020304"},"sequence":123,"txinwitness":[]}"#).unwrap(),
			txin);
    }

    #[test]
    fn signed_transaction_output_serialize() {
        let txout = SignedTransactionOutput {
            value: Some(777.79),
            n: 12,
            script: TransactionOutputScript {
                asm: "Hello, world!!!".to_owned(),
                hex: Bytes::new(vec![1, 2, 3, 4]),
                req_sigs: 777,
                script_type: ScriptType::Multisig,
                addresses: vec![
                    "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".into(),
                    "1H5m1XzvHsjWX3wwU781ubctznEpNACrNC".into(),
                ],
            },
        };
        assert_eq!(
            serde_json::to_string(&txout).unwrap(),
            r#"{"value":777.79,"n":12,"scriptPubKey":{"asm":"Hello, world!!!","hex":"01020304","reqSigs":777,"type":"multisig","addresses":["1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa","1H5m1XzvHsjWX3wwU781ubctznEpNACrNC"]}}"#
        );
    }

    #[test]
    fn signed_transaction_output_deserialize() {
        let txout = SignedTransactionOutput {
            value: Some(777.79),
            n: 12,
            script: TransactionOutputScript {
                asm: "Hello, world!!!".to_owned(),
                hex: Bytes::new(vec![1, 2, 3, 4]),
                req_sigs: 777,
                script_type: ScriptType::Multisig,
                addresses: vec![
                    "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa".into(),
                    "1H5m1XzvHsjWX3wwU781ubctznEpNACrNC".into(),
                ],
            },
        };
        assert_eq!(
			serde_json::from_str::<SignedTransactionOutput>(r#"{"value":777.79,"n":12,"scriptPubKey":{"asm":"Hello, world!!!","hex":"01020304","reqSigs":777,"type":"multisig","addresses":["1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa","1H5m1XzvHsjWX3wwU781ubctznEpNACrNC"]}}"#).unwrap(),
			txout);
    }

    #[test]
    fn transaction_serialize() {
        let tx = Transaction {
            hex: "DEADBEEF".into(),
            txid: H256::from(4),
            hash: Some(H256::from(5)),
            size: Some(33),
            vsize: Some(44),
            version: 55,
            locktime: 66,
            vin: vec![],
            vout: vec![],
            blockhash: H256::from(6),
            confirmations: 77,
            rawconfirmations: None,
            time: 88,
            blocktime: 99,
            height: Some(0),
        };
        assert_eq!(
            serde_json::to_string(&tx).unwrap(),
            r#"{"hex":"deadbeef","txid":"0400000000000000000000000000000000000000000000000000000000000000","hash":"0500000000000000000000000000000000000000000000000000000000000000","size":33,"vsize":44,"version":55,"locktime":66,"vin":[],"vout":[],"blockhash":"0600000000000000000000000000000000000000000000000000000000000000","confirmations":77,"time":88,"blocktime":99,"height":0}"#
        );
    }

    #[test]
    fn transaction_deserialize() {
        let tx = Transaction {
            hex: "DEADBEEF".into(),
            txid: H256::from(4),
            hash: Some(H256::from(5)),
            size: Some(33),
            vsize: Some(44),
            version: 55,
            locktime: 66,
            vin: vec![],
            vout: vec![],
            blockhash: H256::from(6),
            confirmations: 77,
            rawconfirmations: None,
            time: 88,
            blocktime: 99,
            height: None,
        };
        assert_eq!(
			serde_json::from_str::<Transaction>(r#"{"hex":"deadbeef","txid":"0400000000000000000000000000000000000000000000000000000000000000","hash":"0500000000000000000000000000000000000000000000000000000000000000","size":33,"vsize":44,"version":55,"locktime":66,"vin":[],"vout":[],"blockhash":"0600000000000000000000000000000000000000000000000000000000000000","confirmations":77,"time":88,"blocktime":99}"#).unwrap(),
			tx);
    }

    #[test]
    // https://kmdexplorer.io/tx/88893f05764f5a781f2e555a5b492c064f2269a4a44c51afdbe98fab54361bb5
    fn test_kmd_json_transaction_parse_fail() {
        let tx_json = get_transaction_json("kmd_parse_fail");
        let _tx: Transaction = serde_json::from_value(tx_json).unwrap();
    }

    #[test]
    fn test_kmd_coinbase_transaction_parse() {
        let tx_json = get_transaction_json("kmd_coinbase_transaction");
        let _tx: Transaction = serde_json::from_value(tx_json).unwrap();
    }

    // https://live.blockcypher.com/btc/tx/4ab5828480046524afa3fac5eb7f93f768c3eeeaeb5d4d6b6ff22801d3dc521e/
    #[test]
    fn test_btc_4ab5828480046524afa3fac5eb7f93f768c3eeeaeb5d4d6b6ff22801d3dc521e() {
        let json = get_transaction_json("btc_4ab5828480046524afa3fac5eb7f93f768c3eeeaeb5d4d6b6ff22801d3dc521e");
        let _tx: Transaction = serde_json::from_value(json).unwrap();
    }

    #[allow(dead_code)]
    fn test_kmd_raw_confirmations() {
        let json = get_transaction_json("kmd_raw_confirmations");
        let tx: Transaction = serde_json::from_value(json).unwrap();
        assert_eq!(tx.rawconfirmations, Some(8));
    }

    #[test]
    fn test_qtum_call_script_pubkey() {
        let json = get_transaction_json("qtum_call_script");
        let tx: Transaction = serde_json::from_value(json).unwrap();
        assert_eq!(tx.vout[0].script.script_type, ScriptType::Call);
    }

    #[test]
    fn test_firo_sigmaspend_input() {
        // https://explorer.firo.org/tx/d4b9f5a01a43b1d592999f9fd6fe64aa8f63ac42abab43090938321064c1ec1f
        let json = get_transaction_json("firo_sigmaspend");
        let _tx: Transaction = serde_json::from_value(json).unwrap();
    }

    #[test]
    fn test_nav_cold_staking_script_pubkey() {
        let nav_cold_staking_vout = get_transaction_json("nav_cold_staking_vout");
        let vout: Vec<SignedTransactionOutput> = serde_json::from_value(nav_cold_staking_vout).unwrap();
        assert_eq!(vout[1].script.script_type, ScriptType::ColdStaking);
    }
}
