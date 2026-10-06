use derive_more::Display;
use mm2_number::{BigDecimal, BigInt};
use serde_derive::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::str::FromStr;

pub const MINTLAYER_AMOUNT_DECIMALS: i64 = 11;

#[derive(Debug, Display, PartialEq)]
pub enum MintlayerAmountConversionError {
    #[display(fmt = "Invalid Mintlayer atoms amount '{}': {}", value, reason)]
    InvalidAtoms { value: String, reason: String },
    #[display(fmt = "Invalid Mintlayer decimal amount '{}': {}", value, reason)]
    InvalidDecimal { value: String, reason: String },
    #[display(fmt = "Mintlayer amount cannot be negative: atoms '{}'", atoms)]
    NegativeAmount { atoms: String },
    #[display(fmt = "Inconsistent Mintlayer amount: atoms '{}', decimal '{}'", atoms, decimal)]
    InconsistentAmount { atoms: String, decimal: String },
}

/// Amount returned by the Mintlayer API.
///
/// `atoms` is kept as a string to avoid precision loss at JSON boundaries.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerAmount {
    pub atoms: String,
    pub decimal: String,
}

impl MintlayerAmount {
    pub fn to_big_decimal(&self) -> Result<BigDecimal, MintlayerAmountConversionError> {
        let atoms = BigInt::from_str(&self.atoms).map_err(|error| MintlayerAmountConversionError::InvalidAtoms {
            value: self.atoms.clone(),
            reason: error.to_string(),
        })?;
        if atoms < BigInt::from(0) {
            return Err(MintlayerAmountConversionError::NegativeAmount {
                atoms: self.atoms.clone(),
            });
        }
        let amount_from_atoms = BigDecimal::new(atoms, MINTLAYER_AMOUNT_DECIMALS);
        let decimal =
            BigDecimal::from_str(&self.decimal).map_err(|error| MintlayerAmountConversionError::InvalidDecimal {
                value: self.decimal.clone(),
                reason: error.to_string(),
            })?;

        if amount_from_atoms != decimal {
            return Err(MintlayerAmountConversionError::InconsistentAmount {
                atoms: self.atoms.clone(),
                decimal: self.decimal.clone(),
            });
        }

        Ok(amount_from_atoms)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerChainTip {
    pub block_height: u64,
    pub block_id: String,
}

/// Minimal information required to identify the network exposed by an API.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerGenesisInfo {
    pub block_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerTokenBalance {
    pub token_id: String,
    pub amount: MintlayerAmount,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerAddressInfo {
    pub coin_balance: MintlayerAmount,
    pub locked_coin_balance: MintlayerAmount,
    pub transaction_history: Vec<String>,
    pub tokens: Vec<MintlayerTokenBalance>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerOutPoint {
    pub source_id: String,
    pub index: u32,
    pub source_type: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerUtxo {
    pub outpoint: MintlayerOutPoint,
    /// Mintlayer outputs are tagged, variant-dependent objects.
    pub utxo: Json,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerHtlcInfo {
    pub secret: Option<Json>,
    pub secret_hash: Json,
    pub spend_key: String,
    pub refund_timelock: Json,
    pub refund_key: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerTransactionOutput {
    #[serde(rename = "type")]
    pub output_type: String,
    pub value: Json,
    pub htlc: Option<MintlayerHtlcInfo>,
    pub spent_at_block_height: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerTransactionInput {
    pub input_type: String,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub index: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerTransactionInputInfo {
    pub input: MintlayerTransactionInput,
    pub utxo: Option<Json>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MintlayerTransactionInfo {
    pub id: String,
    pub block_id: String,
    pub inputs: Vec<MintlayerTransactionInputInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_hex: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserialize_htlc_transaction_output() {
        let response = json!({
            "type": "Htlc",
            "value": {
                "type": "Coin",
                "amount": {
                    "atoms": "50000000000",
                    "decimal": "0.5"
                }
            },
            "htlc": {
                "secret": null,
                "secret_hash": {
                    "string": null,
                    "hex": "112233445566778899aabbccddeeff0010203040"
                },
                "spend_key": "mtc1qspend",
                "refund_timelock": {
                    "UntilTime": 1800000000
                },
                "refund_key": "mtc1qrefund"
            },
            "spent_at_block_height": 700123
        });

        let output: MintlayerTransactionOutput = serde_json::from_value(response).unwrap();

        assert_eq!(output.output_type, "Htlc");
        assert_eq!(output.spent_at_block_height, Some(700123));

        let htlc = output.htlc.expect("HTLC metadata must be present");

        assert!(htlc.secret.is_none());
        assert_eq!(htlc.secret_hash["hex"], "112233445566778899aabbccddeeff0010203040");
        assert_eq!(htlc.spend_key, "mtc1qspend");
        assert_eq!(htlc.refund_timelock["UntilTime"], 1800000000);
        assert_eq!(htlc.refund_key, "mtc1qrefund");
    }

    #[test]
    fn deserialize_genesis_info_ignores_unneeded_fields() {
        let response = json!({
            "block_id":
                "2cf01f196066bb6f3a4856deb7999294ff520f633fe48e118e8044390e409870",
            "genesis_message": "Mintlayer mainnet",
            "timestamp": {
                "timestamp": 1706468400
            },
            "utxos": []
        });

        let genesis: MintlayerGenesisInfo = serde_json::from_value(response).unwrap();

        assert_eq!(
            genesis.block_id,
            "2cf01f196066bb6f3a4856deb7999294ff520f633fe48e118e8044390e409870"
        );
    }

    #[test]
    fn deserialize_chain_tip() {
        let response = json!({
            "block_height": 680824,
            "block_id": "635ce5bab5992e1c6a32cc9983ec4d295b1c89cc05afdf328f73041b9331a8d1"
        });

        let tip: MintlayerChainTip = serde_json::from_value(response.clone()).unwrap();

        assert_eq!(tip.block_height, 680824);
        assert_eq!(serde_json::to_value(tip).unwrap(), response);
    }

    #[test]
    fn deserialize_address_info() {
        let response = json!({
            "coin_balance": {
                "atoms": "6382564000000000",
                "decimal": "63825.64"
            },
            "locked_coin_balance": {
                "atoms": "0",
                "decimal": "0"
            },
            "transaction_history": [
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ],
            "tokens": []
        });

        let info: MintlayerAddressInfo = serde_json::from_value(response.clone()).unwrap();

        assert_eq!(info.coin_balance.decimal, "63825.64");
        assert_eq!(info.locked_coin_balance.atoms, "0");
        assert!(info.tokens.is_empty());
        assert_eq!(serde_json::to_value(info).unwrap(), response);
    }

    #[test]
    fn convert_consistent_amount_to_big_decimal() {
        let amount = MintlayerAmount {
            atoms: "6382564000000000".into(),
            decimal: "63825.64".into(),
        };

        assert_eq!(
            amount.to_big_decimal().unwrap(),
            BigDecimal::from_str("63825.64").unwrap()
        );
    }

    #[test]
    fn accept_equivalent_decimal_with_trailing_zeroes() {
        let amount = MintlayerAmount {
            atoms: "6382564000000000".into(),
            decimal: "63825.640".into(),
        };

        assert_eq!(
            amount.to_big_decimal().unwrap(),
            BigDecimal::from_str("63825.64").unwrap()
        );
    }

    #[test]
    fn reject_inconsistent_amount_representations() {
        let amount = MintlayerAmount {
            atoms: "6382564000000000".into(),
            decimal: "63825.65".into(),
        };

        assert_eq!(
            amount.to_big_decimal(),
            Err(MintlayerAmountConversionError::InconsistentAmount {
                atoms: amount.atoms,
                decimal: amount.decimal,
            })
        );
    }

    #[test]
    fn reject_invalid_atoms_amount() {
        let amount = MintlayerAmount {
            atoms: "not-an-integer".into(),
            decimal: "0".into(),
        };

        assert!(matches!(
            amount.to_big_decimal(),
            Err(MintlayerAmountConversionError::InvalidAtoms { .. })
        ));
    }

    #[test]
    fn reject_invalid_decimal_amount() {
        let amount = MintlayerAmount {
            atoms: "0".into(),
            decimal: "not-a-decimal".into(),
        };

        assert!(matches!(
            amount.to_big_decimal(),
            Err(MintlayerAmountConversionError::InvalidDecimal { .. })
        ));
    }

    #[test]
    fn reject_negative_amount() {
        let amount = MintlayerAmount {
            atoms: "-1".into(),
            decimal: "-0.00000000001".into(),
        };

        assert_eq!(
            amount.to_big_decimal(),
            Err(MintlayerAmountConversionError::NegativeAmount { atoms: "-1".into() })
        );
    }

    #[test]
    fn deserialize_spendable_utxo() {
        let response = json!({
            "outpoint": {
                "source_id":
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "index": 1,
                "source_type": "Transaction"
            },
            "utxo": {
                "destination": "mtc1qexample",
                "type": "Transfer",
                "value": {
                    "amount": {
                        "atoms": "261768000000000",
                        "decimal": "2617.68"
                    },
                    "type": "Coin"
                }
            }
        });

        let utxo: MintlayerUtxo = serde_json::from_value(response.clone()).unwrap();

        assert_eq!(utxo.outpoint.index, 1);
        assert_eq!(utxo.outpoint.source_type, "Transaction");
        assert_eq!(utxo.utxo.get("type").and_then(Json::as_str), Some("Transfer"));
        assert_eq!(serde_json::to_value(utxo).unwrap(), response);
    }
}
