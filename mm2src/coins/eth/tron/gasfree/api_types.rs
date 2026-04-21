use crate::eth::tron::{TronAddress, TronSignature, TronTxHash};
use ethereum_types::U256;
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

const GASFREE_PROTOCOL_VERSION: u64 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GasfreeRequestId(Uuid);

impl GasfreeRequestId {
    fn try_from_uuid(uuid: Uuid) -> Result<Self, String> {
        match uuid.get_version_num() {
            4 => Ok(GasfreeRequestId(uuid)),
            version => Err(format!("requestId must be a UUIDv4, got version {version}")),
        }
    }
}

impl Serialize for GasfreeRequestId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for GasfreeRequestId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let uuid = deserialize_uuid(deserializer)?;
        GasfreeRequestId::try_from_uuid(uuid).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct GasfreeSubmitRequest {
    #[serde(rename = "requestId", default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<GasfreeRequestId>,
    pub token: TronAddress,
    #[serde(rename = "serviceProvider")]
    pub service_provider: TronAddress,
    pub user: TronAddress,
    pub receiver: TronAddress,
    #[serde(serialize_with = "serialize_u256_as_string")]
    pub value: U256,
    #[serde(rename = "maxFee", serialize_with = "serialize_u256_as_string")]
    pub max_fee: U256,
    #[serde(serialize_with = "serialize_u256_as_string")]
    pub deadline: U256,
    #[serde(
        serialize_with = "serialize_u256_as_string",
        deserialize_with = "deserialize_exact_version_one"
    )]
    pub version: U256,
    #[serde(serialize_with = "serialize_u256_as_string")]
    pub nonce: U256,
    #[serde(rename = "sig")]
    pub signature: TronSignature,
}

impl GasfreeSubmitRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_version_is_one(&self.version, "version")
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct GasfreeSupportedToken {
    #[serde(rename = "tokenAddress")]
    pub token_address: TronAddress,
    #[serde(rename = "activateFee", deserialize_with = "deserialize_u256")]
    pub activate_fee: U256,
    #[serde(rename = "transferFee", deserialize_with = "deserialize_u256")]
    pub transfer_fee: U256,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub symbol: String,
    #[serde(rename = "decimal", deserialize_with = "deserialize_decimal_u8")]
    pub decimal: u8,
    pub supported: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct GasfreeAccountAsset {
    #[serde(rename = "tokenAddress")]
    pub token_address: TronAddress,
    #[serde(rename = "tokenSymbol", deserialize_with = "deserialize_non_empty_string")]
    pub token_symbol: String,
    #[serde(rename = "activateFee", deserialize_with = "deserialize_u256")]
    pub activate_fee: U256,
    #[serde(rename = "transferFee", deserialize_with = "deserialize_u256")]
    pub transfer_fee: U256,
    #[serde(rename = "decimal", deserialize_with = "deserialize_decimal_u8")]
    pub decimal: u8,
    #[serde(deserialize_with = "deserialize_u256")]
    pub frozen: U256,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct GasfreeAccountInfo {
    #[serde(rename = "accountAddress")]
    pub account_address: TronAddress,
    #[serde(rename = "gasFreeAddress")]
    pub gas_free_address: TronAddress,
    pub active: bool,
    #[serde(deserialize_with = "deserialize_u256")]
    pub nonce: U256,
    #[serde(rename = "allowSubmit")]
    pub allow_submit: bool,
    pub assets: Vec<GasfreeAccountAsset>,
}

/// Unknown provider states are rejected during deserialization so API drift is surfaced immediately.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum GasfreeTransferState {
    #[serde(rename = "WAITING")]
    Waiting,
    #[serde(rename = "INPROGRESS")]
    InProgress,
    #[serde(rename = "CONFIRMING")]
    Confirming,
    #[serde(rename = "SUCCEED")]
    Succeed,
    #[serde(rename = "FAILED")]
    Failed,
}

/// Unknown provider states are rejected during deserialization so API drift is surfaced immediately.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GasfreeTransactionState {
    Init,
    NotOnChain,
    OnChain,
    Solidity,
    OnChainFailed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct GasfreeSubmitResponse {
    pub id: Uuid,
    #[serde(rename = "accountAddress")]
    pub account_address: TronAddress,
    #[serde(rename = "gasFreeAddress")]
    pub gas_free_address: TronAddress,
    #[serde(rename = "providerAddress")]
    pub provider_address: TronAddress,
    #[serde(rename = "targetAddress")]
    pub target_address: TronAddress,
    #[serde(rename = "tokenAddress")]
    pub token_address: TronAddress,
    #[serde(deserialize_with = "deserialize_u256")]
    pub amount: U256,
    #[serde(rename = "maxFee", deserialize_with = "deserialize_u256")]
    pub max_fee: U256,
    #[serde(
        rename = "signature",
        default,
        deserialize_with = "deserialize_optional_tron_signature"
    )]
    pub signature: Option<TronSignature>,
    #[serde(deserialize_with = "deserialize_exact_version_one")]
    pub version: U256,
    #[serde(deserialize_with = "deserialize_u256")]
    pub nonce: U256,
    #[serde(rename = "expiredAt", deserialize_with = "deserialize_u64")]
    pub expired_at: u64,
    #[serde(rename = "state")]
    pub state: GasfreeTransferState,
    #[serde(rename = "estimatedActivateFee", deserialize_with = "deserialize_u256")]
    pub estimated_activate_fee: U256,
    #[serde(
        rename = "estimatedTransferFee",
        alias = "estimateTransferFee",
        deserialize_with = "deserialize_u256"
    )]
    pub estimated_transfer_fee: U256,
    #[serde(rename = "createdAt", default, deserialize_with = "deserialize_option_u64")]
    pub created_at: Option<u64>,
    #[serde(rename = "updatedAt", default, deserialize_with = "deserialize_option_u64")]
    pub updated_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct GasfreeTraceResponse {
    pub id: Uuid,
    #[serde(rename = "accountAddress")]
    pub account_address: TronAddress,
    #[serde(rename = "gasFreeAddress")]
    pub gas_free_address: TronAddress,
    #[serde(rename = "providerAddress")]
    pub provider_address: TronAddress,
    #[serde(rename = "targetAddress")]
    pub target_address: TronAddress,
    #[serde(rename = "tokenAddress")]
    pub token_address: TronAddress,
    #[serde(deserialize_with = "deserialize_u256")]
    pub amount: U256,
    #[serde(rename = "state")]
    pub state: GasfreeTransferState,
    #[serde(rename = "expiredAt", deserialize_with = "deserialize_u64")]
    pub expired_at: u64,
    #[serde(rename = "estimatedActivateFee", deserialize_with = "deserialize_u256")]
    pub estimated_activate_fee: U256,
    #[serde(
        rename = "estimatedTransferFee",
        alias = "estimateTransferFee",
        deserialize_with = "deserialize_u256"
    )]
    pub estimated_transfer_fee: U256,
    #[serde(rename = "estimatedTotalFee", deserialize_with = "deserialize_u256")]
    pub estimated_total_fee: U256,
    #[serde(rename = "estimatedTotalCost", deserialize_with = "deserialize_u256")]
    pub estimated_total_cost: U256,
    #[serde(rename = "txnHash", default, deserialize_with = "deserialize_optional_tron_tx_hash")]
    pub txn_hash: Option<TronTxHash>,
    #[serde(rename = "txnBlockNum", default, deserialize_with = "deserialize_option_u64")]
    pub txn_block_num: Option<u64>,
    #[serde(rename = "txnBlockTimestamp", default, deserialize_with = "deserialize_option_u64")]
    pub txn_block_timestamp: Option<u64>,
    #[serde(rename = "txnState", default)]
    pub txn_state: Option<GasfreeTransactionState>,
    #[serde(rename = "txnActivateFee", default, deserialize_with = "deserialize_option_u256")]
    pub txn_activate_fee: Option<U256>,
    #[serde(rename = "txnTransferFee", default, deserialize_with = "deserialize_option_u256")]
    pub txn_transfer_fee: Option<U256>,
    #[serde(rename = "txnTotalFee", default, deserialize_with = "deserialize_option_u256")]
    pub txn_total_fee: Option<U256>,
    #[serde(rename = "txnAmount", default, deserialize_with = "deserialize_option_u256")]
    pub txn_amount: Option<U256>,
    #[serde(rename = "txnTotalCost", default, deserialize_with = "deserialize_option_u256")]
    pub txn_total_cost: Option<U256>,
    #[serde(deserialize_with = "deserialize_u256")]
    pub nonce: U256,
    #[serde(rename = "createdAt", default, deserialize_with = "deserialize_option_u64")]
    pub created_at: Option<u64>,
    #[serde(rename = "updatedAt", default, deserialize_with = "deserialize_option_u64")]
    pub updated_at: Option<u64>,
    #[serde(rename = "maxFee", default, deserialize_with = "deserialize_option_u256")]
    pub max_fee: Option<U256>,
    #[serde(default, deserialize_with = "deserialize_option_exact_version_one")]
    pub version: Option<U256>,
    #[serde(default, deserialize_with = "deserialize_optional_tron_signature")]
    pub signature: Option<TronSignature>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum FlexibleInteger {
    String(String),
    U64(u64),
    I64(i64),
}

fn parse_u256_value(value: FlexibleInteger, field_name: &str) -> Result<U256, String> {
    match value {
        FlexibleInteger::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(format!("{field_name} must not be empty"));
            }
            if trimmed.starts_with('-') {
                return Err(format!("{field_name} must not be negative"));
            }

            U256::from_dec_str(trimmed).map_err(|e| format!("Invalid {field_name}: {e}"))
        },
        FlexibleInteger::U64(value) => Ok(U256::from(value)),
        FlexibleInteger::I64(value) => {
            if value < 0 {
                Err(format!("{field_name} must not be negative"))
            } else {
                Ok(U256::from(value as u64))
            }
        },
    }
}

fn parse_u64_value(value: FlexibleInteger, field_name: &str) -> Result<u64, String> {
    match value {
        FlexibleInteger::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(format!("{field_name} must not be empty"));
            }
            if trimmed.starts_with('-') {
                return Err(format!("{field_name} must not be negative"));
            }
            trimmed.parse::<u64>().map_err(|e| format!("Invalid {field_name}: {e}"))
        },
        FlexibleInteger::U64(value) => Ok(value),
        FlexibleInteger::I64(value) => {
            if value < 0 {
                Err(format!("{field_name} must not be negative"))
            } else {
                Ok(value as u64)
            }
        },
    }
}

fn validate_version_is_one(value: &U256, field_name: &str) -> Result<(), String> {
    if *value == U256::from(GASFREE_PROTOCOL_VERSION) {
        Ok(())
    } else {
        Err(format!(
            "{field_name} must equal {GASFREE_PROTOCOL_VERSION}, got {}",
            value
        ))
    }
}

fn deserialize_uuid<'de, D>(deserializer: D) -> Result<Uuid, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    Uuid::parse_str(raw.trim()).map_err(D::Error::custom)
}

fn deserialize_u256<'de, D>(deserializer: D) -> Result<U256, D::Error>
where
    D: Deserializer<'de>,
{
    let value = FlexibleInteger::deserialize(deserializer)?;
    parse_u256_value(value, "decimal number").map_err(D::Error::custom)
}

fn deserialize_exact_version_one<'de, D>(deserializer: D) -> Result<U256, D::Error>
where
    D: Deserializer<'de>,
{
    let value = deserialize_u256(deserializer)?;
    validate_version_is_one(&value, "version").map_err(D::Error::custom)?;
    Ok(value)
}

fn deserialize_option_exact_version_one<'de, D>(deserializer: D) -> Result<Option<U256>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<FlexibleInteger>::deserialize(deserializer)?;
    value
        .map(|val| parse_u256_value(val, "version"))
        .transpose()
        .and_then(|opt| {
            opt.map(|version| validate_version_is_one(&version, "version").map(|_| version))
                .transpose()
        })
        .map_err(D::Error::custom)
}

fn deserialize_option_u256<'de, D>(deserializer: D) -> Result<Option<U256>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<FlexibleInteger>::deserialize(deserializer)?;
    value
        .map(|val| parse_u256_value(val, "decimal number"))
        .transpose()
        .map_err(D::Error::custom)
}

fn deserialize_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = FlexibleInteger::deserialize(deserializer)?;
    parse_u64_value(value, "integer").map_err(D::Error::custom)
}

fn deserialize_option_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<FlexibleInteger>::deserialize(deserializer)?;
    value
        .map(|val| parse_u64_value(val, "integer"))
        .transpose()
        .map_err(D::Error::custom)
}

fn deserialize_decimal_u8<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: Deserializer<'de>,
{
    let value = deserialize_u64(deserializer)?;
    if value > u8::MAX as u64 {
        return Err(D::Error::custom(format!(
            "decimal value {value} is out of range for u8"
        )));
    }
    Ok(value as u8)
}

fn deserialize_non_empty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(D::Error::custom("string must not be empty"));
    }
    Ok(trimmed.to_string())
}

fn deserialize_optional_tron_signature<'de, D>(deserializer: D) -> Result<Option<TronSignature>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    match value {
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                TronSignature::from_hex_str(trimmed).map(Some).map_err(D::Error::custom)
            }
        },
        None => Ok(None),
    }
}

fn deserialize_optional_tron_tx_hash<'de, D>(deserializer: D) -> Result<Option<TronTxHash>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    match value {
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                TronTxHash::from_hex_str(trimmed).map(Some).map_err(D::Error::custom)
            }
        },
        None => Ok(None),
    }
}

fn serialize_u256_as_string<S>(value: &U256, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_request_payload() -> serde_json::Value {
        json!({
            "requestId": "550e8400-e29b-41d4-a716-446655440000",
            "token": "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
            "serviceProvider": "TKtWbdzEq5ss9vTS9kwRhBp5mXmBfBns3E",
            "user": "TUUSMd58eC3fKx3fn7whxJyr1FR56tgaP8",
            "receiver": "TEkj3ndMVEmFLYaFrATMwMjBRZ1EAZkucT",
            "value": "100000",
            "maxFee": "2000000",
            "deadline": "1747909695",
            "version": "1",
            "nonce": "8",
            "sig": "11".repeat(65)
        })
    }

    fn base_submit_response_payload() -> serde_json::Value {
        json!({
            "id": "6c3ff67e-0bf4-4c09-91ca-0c7c254b01a0",
            "accountAddress": "TUUSMd58eC3fKx3fn7whxJyr1FR56tgaP8",
            "gasFreeAddress": "TNER12mMVWruqopsW9FQtKxCGfZcEtb3ER",
            "providerAddress": "TKtWbdzEq5ss9vTS9kwRhBp5mXmBfBns3E",
            "targetAddress": "TEkj3ndMVEmFLYaFrATMwMjBRZ1EAZkucT",
            "tokenAddress": "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
            "amount": 100000,
            "maxFee": 2000000,
            "signature": "",
            "version": 1,
            "nonce": 8,
            "expiredAt": 1747909695000u64,
            "state": "WAITING",
            "estimatedActivateFee": 0,
            "estimatedTransferFee": 2000,
            "createdAt": 1747909635678u64,
            "updatedAt": 1747909635678u64
        })
    }

    fn base_trace_response_payload() -> serde_json::Value {
        json!({
            "id": "6c3ff67e-0bf4-4c09-91ca-0c7c254b01a0",
            "accountAddress": "TUUSMd58eC3fKx3fn7whxJyr1FR56tgaP8",
            "gasFreeAddress": "TNER12mMVWruqopsW9FQtKxCGfZcEtb3ER",
            "providerAddress": "TKtWbdzEq5ss9vTS9kwRhBp5mXmBfBns3E",
            "targetAddress": "TEkj3ndMVEmFLYaFrATMwMjBRZ1EAZkucT",
            "tokenAddress": "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
            "amount": 100000,
            "state": "CONFIRMING",
            "expiredAt": 1747909695000u64,
            "estimatedActivateFee": 0,
            "estimatedTransferFee": 2000,
            "estimatedTotalFee": 2000,
            "estimatedTotalCost": 102000,
            "txnHash": "22".repeat(32),
            "txnBlockNum": 57175988,
            "txnBlockTimestamp": 1747909638000u64,
            "txnState": "ON_CHAIN",
            "txnActivateFee": 0,
            "txnTransferFee": 2000,
            "txnTotalFee": 2000,
            "txnAmount": 100000,
            "txnTotalCost": 102000,
            "nonce": 8,
            "version": 1,
            "signature": "33".repeat(65)
        })
    }

    #[test]
    fn submit_request_protocol_contract() {
        let request = GasfreeSubmitRequest {
            request_id: Some(
                GasfreeRequestId::try_from_uuid(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap())
                    .unwrap(),
            ),
            token: "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf".parse().unwrap(),
            service_provider: "TKtWbdzEq5ss9vTS9kwRhBp5mXmBfBns3E".parse().unwrap(),
            user: "TUUSMd58eC3fKx3fn7whxJyr1FR56tgaP8".parse().unwrap(),
            receiver: "TEkj3ndMVEmFLYaFrATMwMjBRZ1EAZkucT".parse().unwrap(),
            value: U256::from(100_000u64),
            max_fee: U256::from(2_000_000u64),
            deadline: U256::from(1_747_909_695u64),
            version: U256::from(1u64),
            nonce: U256::from(8u64),
            signature: TronSignature::from_hex_str(&"11".repeat(65)).unwrap(),
        };

        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["value"], "100000");
        assert_eq!(json["maxFee"], "2000000");
        assert_eq!(json["nonce"], "8");
        assert_eq!(json["sig"], json!("11".repeat(65).to_lowercase()));

        let invalid_cases = [
            (
                "non-v4 requestId",
                json!("f81d4fae-7dec-11d0-a765-00a0c91e6bf6"),
                "requestId",
            ),
            ("version != 1", json!(2), "version"),
            ("prefixed signature", json!(format!("0x{}", "11".repeat(65))), "sig"),
            ("short signature", json!("11".repeat(64)), "sig"),
            ("long signature", json!("11".repeat(66)), "sig"),
            ("odd signature", json!("1".repeat(129)), "sig"),
        ];

        for (label, value, field) in invalid_cases {
            let mut payload = base_request_payload();
            payload[field] = value;
            assert!(
                serde_json::from_value::<GasfreeSubmitRequest>(payload).is_err(),
                "expected request rejection for {}",
                label
            );
        }
    }

    #[test]
    fn provider_response_protocol_validation() {
        let mut submit_payload = base_submit_response_payload();
        submit_payload["version"] = json!(2);
        assert!(serde_json::from_value::<GasfreeSubmitResponse>(submit_payload).is_err());

        let mut submit_payload = base_submit_response_payload();
        submit_payload["state"] = json!("UNKNOWN_STATE");
        assert!(serde_json::from_value::<GasfreeSubmitResponse>(submit_payload).is_err());

        for invalid in [
            json!(format!("0x{}", "22".repeat(32))),
            json!("22".repeat(31)),
            json!("22".repeat(33)),
            json!("2".repeat(63)),
        ] {
            let mut payload = base_trace_response_payload();
            payload["txnHash"] = invalid;
            assert!(serde_json::from_value::<GasfreeTraceResponse>(payload).is_err());
        }

        for invalid in [json!("33".repeat(64)), json!("33".repeat(66)), json!("3".repeat(129))] {
            let mut payload = base_trace_response_payload();
            payload["signature"] = invalid;
            assert!(serde_json::from_value::<GasfreeTraceResponse>(payload).is_err());
        }

        let mut trace_payload = base_trace_response_payload();
        trace_payload["txnState"] = json!("MYSTERY");
        assert!(serde_json::from_value::<GasfreeTraceResponse>(trace_payload).is_err());
    }
}
