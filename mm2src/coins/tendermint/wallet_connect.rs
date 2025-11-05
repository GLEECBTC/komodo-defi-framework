/// https://docs.reown.com/advanced/multichain/rpc-reference/cosmos-rpc
use base64::engine::general_purpose;
use base64::Engine;
use cosmrs::proto::cosmos::tx::v1beta1::TxRaw;
use kdf_walletconnect::chain::WcChainId;
use kdf_walletconnect::error::WalletConnectError;
use kdf_walletconnect::WalletConnectOps;
use kdf_walletconnect::{chain::WcRequestMethods, WalletConnectCtx, WcTopic};
use mm2_err_handle::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

use super::{CosmosTransaction, TendermintCoin, TendermintWalletConnectionType};
use crate::MarketCoinOps;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct CosmosTxSignedData {
    pub(crate) signature: CosmosTxSignature,
    pub(crate) signed: CosmosSignData,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct CosmosTxSignature {
    pub(crate) pub_key: CosmosTxPublicKey,
    pub(crate) signature: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct CosmosTxPublicKey {
    #[serde(rename = "type")]
    pub(crate) key_type: String,
    pub(crate) value: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CosmosSignData {
    pub(crate) chain_id: String,
    pub(crate) account_number: String,
    #[serde(deserialize_with = "deserialize_vec_field")]
    pub(crate) auth_info_bytes: Vec<u8>,
    #[serde(deserialize_with = "deserialize_vec_field")]
    pub(crate) body_bytes: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum CosmosAccountAlgo {
    #[serde(rename = "secp256k1")]
    Secp256k1,
    #[serde(rename = "tendermint/PubKeySecp256k1")]
    TendermintSecp256k1,
}

impl FromStr for CosmosAccountAlgo {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "secp256k1" => Ok(Self::Secp256k1),
            "tendermint/PubKeySecp256k1" => Ok(Self::TendermintSecp256k1),
            _ => Err(format!("Unknown pubkey type: {s}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CosmosAccount {
    pub address: String,
    #[serde(deserialize_with = "deserialize_vec_field")]
    pub pubkey: Vec<u8>,
    pub algo: CosmosAccountAlgo,
    #[serde(default)]
    pub is_ledger: Option<bool>,
}

#[async_trait::async_trait]
impl WalletConnectOps for TendermintCoin {
    type Error = MmError<WalletConnectError>;
    type Params<'a> = serde_json::Value;
    type SignTxData = TxRaw;
    type SendTxData = CosmosTransaction;

    async fn wc_chain_id(&self, wc: &WalletConnectCtx) -> Result<WcChainId, Self::Error> {
        let chain_id = WcChainId::new_cosmos(self.protocol_info.chain_id.to_string());
        let session_topic = self.session_topic()?;
        wc.validate_update_active_chain_id(session_topic, &chain_id).await?;
        Ok(chain_id)
    }

    async fn wc_sign_tx<'a>(
        &self,
        wc: &WalletConnectCtx,
        params: Self::Params<'a>,
    ) -> Result<Self::SignTxData, Self::Error> {
        let chain_id = self.wc_chain_id(wc).await?;
        let session_topic = self.session_topic()?;
        let method = if wc.is_ledger_connection(session_topic) {
            WcRequestMethods::CosmosSignAmino
        } else {
            WcRequestMethods::CosmosSignDirect
        };
        let data: CosmosTxSignedData = wc
            .send_session_request_and_wait(session_topic, &chain_id, method, params)
            .await?;
        let signature = general_purpose::STANDARD
            .decode(data.signature.signature)
            .map_to_mm(|err| WalletConnectError::PayloadError(err.to_string()))?;

        Ok(TxRaw {
            body_bytes: data.signed.body_bytes,
            auth_info_bytes: data.signed.auth_info_bytes,
            signatures: vec![signature],
        })
    }

    async fn wc_send_tx<'a>(
        &self,
        _ctx: &WalletConnectCtx,
        _params: Self::Params<'a>,
    ) -> Result<Self::SendTxData, Self::Error> {
        todo!()
    }

    fn session_topic(&self) -> Result<&WcTopic, Self::Error> {
        match self.wallet_type {
            TendermintWalletConnectionType::WcLedger(ref session_topic)
            | TendermintWalletConnectionType::Wc(ref session_topic) => Ok(session_topic),
            _ => MmError::err(WalletConnectError::SessionError(format!(
                "{} is not activated via WalletConnect",
                self.ticker()
            ))),
        }
    }
}

pub async fn cosmos_get_accounts_impl(
    wc: &WalletConnectCtx,
    session_topic: &WcTopic,
    chain_id: &str,
) -> MmResult<CosmosAccount, WalletConnectError> {
    let chain_id = WcChainId::new_cosmos(chain_id.to_string());
    wc.validate_update_active_chain_id(session_topic, &chain_id).await?;

    let (account, properties) = wc.get_account_and_properties_for_chain_id(session_topic, &chain_id)?;

    // Check if session has session_properties and return wallet account;
    if let Some(props) = properties {
        if let Some(keys) = &props.keys {
            if let Some(key) = keys.iter().next() {
                let pubkey = decode_data(&key.pub_key).map_to_mm(|err| {
                    WalletConnectError::PayloadError(format!("error decoding pubkey payload: {err:?}"))
                })?;
                let address = decode_data(&key.address).map_to_mm(|err| {
                    WalletConnectError::PayloadError(format!("error decoding address payload: {err:?}"))
                })?;
                let address = hex::encode(address);
                let algo = CosmosAccountAlgo::from_str(&key.algo).map_to_mm(|err| {
                    WalletConnectError::PayloadError(format!("error decoding algo payload: {err:?}"))
                })?;

                return Ok(CosmosAccount {
                    address,
                    pubkey,
                    algo,
                    is_ledger: Some(key.is_nano_ledger),
                });
            }
        }
    }

    let params = serde_json::to_value(&account).unwrap();
    let accounts: Vec<CosmosAccount> = wc
        .send_session_request_and_wait(session_topic, &chain_id, WcRequestMethods::CosmosGetAccounts, params)
        .await?;

    accounts.first().cloned().or_mm_err(|| {
        WalletConnectError::NoAccountFound("Expected atleast an account from connected wallet".to_string())
    })
}

fn deserialize_vec_field<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;

    match value {
        Value::Object(map) => map
            .iter()
            .map(|(_, value)| {
                value
                    .as_u64()
                    .ok_or_else(|| serde::de::Error::custom("Invalid byte value"))
                    .and_then(|n| {
                        if n <= 255 {
                            Ok(n as u8)
                        } else {
                            Err(serde::de::Error::custom("Invalid byte value"))
                        }
                    })
            })
            .collect(),
        Value::Array(arr) => arr
            .into_iter()
            .map(|v| {
                v.as_u64()
                    .ok_or_else(|| serde::de::Error::custom("Invalid byte value"))
                    .map(|n| n as u8)
            })
            .collect(),
        Value::String(data) => {
            let data = decode_data(&data).map_err(|err| serde::de::Error::custom(err.to_string()))?;
            Ok(data)
        },
        _ => Err(serde::de::Error::custom("Pubkey must be an string, object or array")),
    }
}

fn decode_data(encoded: &str) -> Result<Vec<u8>, &'static str> {
    if encoded.chars().all(|c| c.is_ascii_hexdigit()) && encoded.len().is_multiple_of(2) {
        hex::decode(encoded).map_err(|_| "Invalid hex encoding")
    } else if encoded.contains('=') || encoded.contains('/') || encoded.contains('+') || encoded.len().is_multiple_of(4)
    {
        general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| "Invalid base64 encoding")
    } else {
        Err("Unknown encoding format")
    }
}

#[cfg(test)]
mod test_cosmos_walletconnect {
    use serde_json::json;

    use super::{decode_data, CosmosSignData, CosmosTxPublicKey, CosmosTxSignature, CosmosTxSignedData};

    #[test]
    fn test_decode_base64() {
        // "Hello world" in base64
        let base64_data = "SGVsbG8gd29ybGQ=";
        let expected = b"Hello world".to_vec();
        let result = decode_data(base64_data);
        assert_eq!(result.unwrap(), expected, "Base64 decoding failed");
    }

    #[test]
    fn test_decode_hex() {
        // "Hello world" in hex
        let hex_data = "48656c6c6f20776f726c64";
        let expected = b"Hello world".to_vec();
        let result = decode_data(hex_data);
        assert_eq!(result.unwrap(), expected, "Hex decoding failed");
    }

    #[test]
    fn test_deserialize_sign_message_response() {
        let json = json!({
        "signature": {
          "signature": "eGrmDGKTmycxJO56yTQORDzTFjBEBgyBmHc8ey6FbHh9WytzgsJilYBywz5uludhyKePZdRwznamg841fXw50Q==",
          "pub_key": {
            "type": "tendermint/PubKeySecp256k1",
            "value": "AjqZ1rq/EsPAb4SA6l0qjpVMHzqXotYXz23D5kOceYYu"
          }
        },
        "signed": {
          "chainId": "cosmoshub-4",
          "authInfoBytes": "0a500a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a21023a99d6babf12c3c06f8480ea5d2a8e954c1f3a97a2d617cf6dc3e6439c79862e12040a020801180212140a0e0a057561746f6d1205313837353010c8d007",
          "bodyBytes": "0a8e010a1c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e64126e0a2d636f736d6f7331376c386432737973646e3667683636786d366664666b6575333634703836326a68396c6e6667122d636f736d6f7331376c386432737973646e3667683636786d366664666b6575333634703836326a68396c6e66671a0e0a057561746f6d12053430303030189780e00a",
          "accountNumber": "2934714"
        }
              });
        let expected_tx = CosmosTxSignedData {
            signature: CosmosTxSignature {
                pub_key: CosmosTxPublicKey {
                    key_type: "tendermint/PubKeySecp256k1".to_owned(),
                    value: "AjqZ1rq/EsPAb4SA6l0qjpVMHzqXotYXz23D5kOceYYu".to_owned(),
                },
                signature: "eGrmDGKTmycxJO56yTQORDzTFjBEBgyBmHc8ey6FbHh9WytzgsJilYBywz5uludhyKePZdRwznamg841fXw50Q=="
                    .to_owned(),
            },
            signed: CosmosSignData {
                chain_id: "cosmoshub-4".to_owned(),
                account_number: "2934714".to_owned(),
                auth_info_bytes: vec![
                    10, 80, 10, 70, 10, 31, 47, 99, 111, 115, 109, 111, 115, 46, 99, 114, 121, 112, 116, 111, 46, 115,
                    101, 99, 112, 50, 53, 54, 107, 49, 46, 80, 117, 98, 75, 101, 121, 18, 35, 10, 33, 2, 58, 153, 214,
                    186, 191, 18, 195, 192, 111, 132, 128, 234, 93, 42, 142, 149, 76, 31, 58, 151, 162, 214, 23, 207,
                    109, 195, 230, 67, 156, 121, 134, 46, 18, 4, 10, 2, 8, 1, 24, 2, 18, 20, 10, 14, 10, 5, 117, 97,
                    116, 111, 109, 18, 5, 49, 56, 55, 53, 48, 16, 200, 208, 7,
                ],
                body_bytes: vec![
                    10, 142, 1, 10, 28, 47, 99, 111, 115, 109, 111, 115, 46, 98, 97, 110, 107, 46, 118, 49, 98, 101,
                    116, 97, 49, 46, 77, 115, 103, 83, 101, 110, 100, 18, 110, 10, 45, 99, 111, 115, 109, 111, 115, 49,
                    55, 108, 56, 100, 50, 115, 121, 115, 100, 110, 54, 103, 104, 54, 54, 120, 109, 54, 102, 100, 102,
                    107, 101, 117, 51, 54, 52, 112, 56, 54, 50, 106, 104, 57, 108, 110, 102, 103, 18, 45, 99, 111, 115,
                    109, 111, 115, 49, 55, 108, 56, 100, 50, 115, 121, 115, 100, 110, 54, 103, 104, 54, 54, 120, 109,
                    54, 102, 100, 102, 107, 101, 117, 51, 54, 52, 112, 56, 54, 50, 106, 104, 57, 108, 110, 102, 103,
                    26, 14, 10, 5, 117, 97, 116, 111, 109, 18, 5, 52, 48, 48, 48, 48, 24, 151, 128, 224, 10,
                ],
            },
        };

        let actual_tx = serde_json::from_value::<CosmosTxSignedData>(json).unwrap();
        assert_eq!(expected_tx, actual_tx);
    }
}
