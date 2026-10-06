use crate::mintlayer::{MintlayerAmount, MintlayerOutPoint, MintlayerUtxo};
use serde_json::Value as Json;
use thiserror::Error;

const TRANSACTION_SOURCE: &str = "Transaction";
const BLOCK_REWARD_SOURCE: &str = "BlockReward";
const TRANSFER_OUTPUT: &str = "Transfer";
const COIN_VALUE: &str = "Coin";
const SOURCE_ID_HEX_LENGTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct MintlayerCoinUtxo {
    pub outpoint: MintlayerOutPoint,
    pub destination: String,
    pub amount: MintlayerAmount,
    pub atoms: u128,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MintlayerUtxoSelection {
    pub selected: Vec<MintlayerCoinUtxo>,
    pub total_atoms: u128,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum MintlayerUtxoSelectionError {
    #[error("Malformed Mintlayer UTXO: missing or invalid field '{0}'")]
    MalformedOutput(&'static str),
    #[error("Unsupported Mintlayer outpoint source type '{0}'")]
    UnsupportedSourceType(String),
    #[error("Invalid Mintlayer source id '{0}'")]
    InvalidSourceId(String),
    #[error("Mintlayer UTXO destination mismatch: expected '{expected}', found '{actual}'")]
    DestinationMismatch { expected: String, actual: String },
    #[error("Invalid Mintlayer UTXO amount: {0}")]
    InvalidAmount(String),
    #[error("Mintlayer UTXO amount total overflow")]
    AmountOverflow,
    #[error("Insufficient Mintlayer funds: required {required_atoms} atoms, available {available_atoms} atoms")]
    InsufficientFunds {
        required_atoms: u128,
        available_atoms: u128,
    },
}

/// Validates and deterministically selects spendable `Transfer/Coin` UTXOs.
///
/// Non-coin outputs are ignored. Coin UTXOs are ordered by source type,
/// source id and output index before selection, so equivalent API responses
/// always produce the same input set regardless of response order.
pub fn select_spendable_coin_utxos(
    utxos: &[MintlayerUtxo],
    expected_destination: &str,
    required_atoms: u128,
) -> Result<MintlayerUtxoSelection, MintlayerUtxoSelectionError> {
    if required_atoms == 0 {
        return Ok(MintlayerUtxoSelection {
            selected: Vec::new(),
            total_atoms: 0,
        });
    }

    let mut coin_utxos = utxos
        .iter()
        .filter_map(|utxo| match parse_coin_utxo(utxo, expected_destination) {
            Ok(Some(coin_utxo)) => Some(Ok(coin_utxo)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()?;

    coin_utxos.sort_by(|left, right| {
        left.outpoint
            .source_type
            .cmp(&right.outpoint.source_type)
            .then_with(|| left.outpoint.source_id.cmp(&right.outpoint.source_id))
            .then_with(|| left.outpoint.index.cmp(&right.outpoint.index))
    });

    let mut selected = Vec::new();
    let mut total_atoms = 0_u128;

    for utxo in coin_utxos {
        total_atoms = total_atoms
            .checked_add(utxo.atoms)
            .ok_or(MintlayerUtxoSelectionError::AmountOverflow)?;
        selected.push(utxo);

        if total_atoms >= required_atoms {
            return Ok(MintlayerUtxoSelection { selected, total_atoms });
        }
    }

    Err(MintlayerUtxoSelectionError::InsufficientFunds {
        required_atoms,
        available_atoms: total_atoms,
    })
}

fn parse_coin_utxo(
    utxo: &MintlayerUtxo,
    expected_destination: &str,
) -> Result<Option<MintlayerCoinUtxo>, MintlayerUtxoSelectionError> {
    let output_type = json_string(&utxo.utxo, "type")?;
    if output_type != TRANSFER_OUTPUT {
        return Ok(None);
    }

    let value = utxo
        .utxo
        .get("value")
        .ok_or(MintlayerUtxoSelectionError::MalformedOutput("utxo.value"))?;
    let value_type = json_string(value, "type")?;
    if value_type != COIN_VALUE {
        return Ok(None);
    }

    validate_outpoint(&utxo.outpoint)?;

    let destination = json_string(&utxo.utxo, "destination")?.to_owned();
    if destination != expected_destination {
        return Err(MintlayerUtxoSelectionError::DestinationMismatch {
            expected: expected_destination.to_owned(),
            actual: destination,
        });
    }

    let amount_json = value
        .get("amount")
        .ok_or(MintlayerUtxoSelectionError::MalformedOutput("utxo.value.amount"))?;
    let amount: MintlayerAmount = serde_json::from_value(amount_json.clone())
        .map_err(|error| MintlayerUtxoSelectionError::InvalidAmount(error.to_string()))?;
    amount
        .to_big_decimal()
        .map_err(|error| MintlayerUtxoSelectionError::InvalidAmount(error.to_string()))?;
    let atoms = amount
        .atoms
        .parse::<u128>()
        .map_err(|error| MintlayerUtxoSelectionError::InvalidAmount(error.to_string()))?;

    Ok(Some(MintlayerCoinUtxo {
        outpoint: utxo.outpoint.clone(),
        destination,
        amount,
        atoms,
    }))
}

fn validate_outpoint(outpoint: &MintlayerOutPoint) -> Result<(), MintlayerUtxoSelectionError> {
    if outpoint.source_type != TRANSACTION_SOURCE && outpoint.source_type != BLOCK_REWARD_SOURCE {
        return Err(MintlayerUtxoSelectionError::UnsupportedSourceType(
            outpoint.source_type.clone(),
        ));
    }

    if outpoint.source_id.len() != SOURCE_ID_HEX_LENGTH
        || !outpoint.source_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(MintlayerUtxoSelectionError::InvalidSourceId(outpoint.source_id.clone()));
    }

    Ok(())
}

fn json_string<'a>(value: &'a Json, field: &'static str) -> Result<&'a str, MintlayerUtxoSelectionError> {
    value
        .get(field)
        .and_then(Json::as_str)
        .ok_or(MintlayerUtxoSelectionError::MalformedOutput(field))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ADDRESS: &str = "mtc1qxlpcx3rzm4nlqw2a2atsw9gtuv6lvaeasdrqxjz";
    const REAL_SOURCE_ID: &str = "9381eb0c0a083415e3ac98969d611359f394d89d390c540b5f9fbded39f00753";

    fn coin_utxo(source_id: &str, index: u32, atoms: &str, decimal: &str) -> MintlayerUtxo {
        serde_json::from_value(json!({
            "outpoint": {
                "index": index,
                "source_id": source_id,
                "source_type": "Transaction"
            },
            "utxo": {
                "destination": ADDRESS,
                "type": "Transfer",
                "value": {
                    "amount": {
                        "atoms": atoms,
                        "decimal": decimal
                    },
                    "type": "Coin"
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn selects_real_mainnet_utxo_vector() {
        let utxo = coin_utxo(REAL_SOURCE_ID, 0, "10000000000", "0.1");

        let selection = select_spendable_coin_utxos(&[utxo], ADDRESS, 9_000_000_000).unwrap();

        assert_eq!(selection.selected.len(), 1);
        assert_eq!(selection.selected[0].outpoint.source_id, REAL_SOURCE_ID);
        assert_eq!(selection.total_atoms, 10_000_000_000);
    }

    #[test]
    fn selection_order_is_independent_from_api_order() {
        let high = coin_utxo(&"b".repeat(64), 0, "20", "0.0000000002");
        let low = coin_utxo(&"a".repeat(64), 1, "10", "0.0000000001");

        let selection = select_spendable_coin_utxos(&[high, low], ADDRESS, 15).unwrap();

        assert_eq!(selection.selected.len(), 2);
        assert_eq!(selection.selected[0].outpoint.source_id, "a".repeat(64));
        assert_eq!(selection.total_atoms, 30);
    }

    #[test]
    fn ignores_non_coin_outputs() {
        let token: MintlayerUtxo = serde_json::from_value(json!({
            "outpoint": {
                "index": 0,
                "source_id": "c".repeat(64),
                "source_type": "Transaction"
            },
            "utxo": {
                "destination": ADDRESS,
                "type": "Transfer",
                "value": {
                    "amount": { "atoms": "100", "decimal": "100" },
                    "token_id": "token",
                    "type": "TokenV1"
                }
            }
        }))
        .unwrap();
        let coin = coin_utxo(REAL_SOURCE_ID, 0, "10", "0.0000000001");

        let selection = select_spendable_coin_utxos(&[token, coin], ADDRESS, 10).unwrap();

        assert_eq!(selection.selected.len(), 1);
        assert_eq!(selection.total_atoms, 10);
    }

    #[test]
    fn rejects_invalid_source_id() {
        let utxo = coin_utxo("not-a-source-id", 0, "10", "0.0000000001");

        assert!(matches!(
            select_spendable_coin_utxos(&[utxo], ADDRESS, 1),
            Err(MintlayerUtxoSelectionError::InvalidSourceId(_))
        ));
    }

    #[test]
    fn reports_insufficient_funds() {
        let utxo = coin_utxo(REAL_SOURCE_ID, 0, "10", "0.0000000001");

        assert_eq!(
            select_spendable_coin_utxos(&[utxo], ADDRESS, 11),
            Err(MintlayerUtxoSelectionError::InsufficientFunds {
                required_atoms: 11,
                available_atoms: 10,
            })
        );
    }
}
