use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use std::convert::TryFrom;
use thiserror::Error;

const BYTES_PER_KILOBYTE: u128 = 1000;
const CEILING_ADDITION: u128 = BYTES_PER_KILOBYTE - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MintlayerFeeRate {
    atoms_per_kb: u128,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum MintlayerFeeError {
    #[error("Invalid Mintlayer fee rate '{value}': {reason}")]
    InvalidFeeRate { value: String, reason: String },
    #[error("Mintlayer fee calculation overflow")]
    FeeOverflow,
    #[error("Mintlayer required amount overflow")]
    RequiredAmountOverflow,
}

impl MintlayerFeeRate {
    pub const fn from_atoms_per_kb(atoms_per_kb: u128) -> Self {
        Self { atoms_per_kb }
    }

    pub fn from_atoms_per_kb_str(value: &str) -> Result<Self, MintlayerFeeError> {
        let atoms_per_kb = value
            .parse::<u128>()
            .map_err(|error| MintlayerFeeError::InvalidFeeRate {
                value: value.to_owned(),
                reason: error.to_string(),
            })?;
        Ok(Self::from_atoms_per_kb(atoms_per_kb))
    }

    pub const fn atoms_per_kb(self) -> u128 {
        self.atoms_per_kb
    }

    /// Matches Mintlayer Core's `FeeRate::compute_fee` semantics:
    /// `ceil(atoms_per_kb * size_bytes / 1000)`.
    pub fn compute_fee_atoms(self, size_bytes: usize) -> Result<u128, MintlayerFeeError> {
        let size_bytes = u128::try_from(size_bytes).expect("usize always fits into u128");
        let scaled_fee = self
            .atoms_per_kb
            .checked_mul(size_bytes)
            .ok_or(MintlayerFeeError::FeeOverflow)?;
        let rounded_fee = scaled_fee
            .checked_add(CEILING_ADDITION)
            .ok_or(MintlayerFeeError::FeeOverflow)?;
        Ok(rounded_fee / BYTES_PER_KILOBYTE)
    }

    pub fn required_atoms(self, send_atoms: u128, size_bytes: usize) -> Result<u128, MintlayerFeeError> {
        let fee_atoms = self.compute_fee_atoms(size_bytes)?;
        send_atoms
            .checked_add(fee_atoms)
            .ok_or(MintlayerFeeError::RequiredAmountOverflow)
    }
}

impl<'de> Deserialize<'de> for MintlayerFeeRate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_atoms_per_kb_str(&value).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mintlayer::{select_spendable_coin_utxos, MintlayerUtxo, MintlayerUtxoSelectionError};
    use serde_json::json;

    const ADDRESS: &str = "mtc1qxlpcx3rzm4nlqw2a2atsw9gtuv6lvaeasdrqxjz";
    const SOURCE_ID: &str = "9381eb0c0a083415e3ac98969d611359f394d89d390c540b5f9fbded39f00753";
    const MAINNET_MIN_RATE: u128 = 100_000_000_000;

    #[test]
    fn deserialize_public_api_fee_rate_string() {
        let fee_rate: MintlayerFeeRate = serde_json::from_str(r#""100000000000""#).unwrap();

        assert_eq!(fee_rate.atoms_per_kb(), MAINNET_MIN_RATE);
    }

    #[test]
    fn reject_non_string_and_invalid_fee_rates() {
        assert!(serde_json::from_str::<MintlayerFeeRate>("100000000000").is_err());
        assert!(serde_json::from_str::<MintlayerFeeRate>(r#""not-an-integer""#).is_err());
        assert!(serde_json::from_str::<MintlayerFeeRate>(r#""-1""#).is_err());
    }

    #[test]
    fn compute_fee_uses_canonical_ceiling_rounding() {
        let fee_rate = MintlayerFeeRate::from_atoms_per_kb(1);

        assert_eq!(fee_rate.compute_fee_atoms(0), Ok(0));
        assert_eq!(fee_rate.compute_fee_atoms(1), Ok(1));
        assert_eq!(fee_rate.compute_fee_atoms(999), Ok(1));
        assert_eq!(fee_rate.compute_fee_atoms(1000), Ok(1));
        assert_eq!(fee_rate.compute_fee_atoms(1001), Ok(2));
    }

    #[test]
    fn compute_observed_mainnet_fee_for_offline_transaction_sizes() {
        let fee_rate = MintlayerFeeRate::from_atoms_per_kb(MAINNET_MIN_RATE);

        assert_eq!(fee_rate.compute_fee_atoms(175), Ok(17_500_000_000));
        assert_eq!(fee_rate.compute_fee_atoms(176), Ok(17_600_000_000));
    }

    #[test]
    fn report_fee_and_required_amount_overflow() {
        let fee_rate = MintlayerFeeRate::from_atoms_per_kb(u128::MAX);
        assert_eq!(fee_rate.compute_fee_atoms(2), Err(MintlayerFeeError::FeeOverflow));

        let one_atom_per_kb = MintlayerFeeRate::from_atoms_per_kb(1);
        assert_eq!(
            one_atom_per_kb.required_atoms(u128::MAX, 1),
            Err(MintlayerFeeError::RequiredAmountOverflow)
        );
    }

    #[test]
    fn real_point_one_ml_utxo_cannot_cover_mainnet_fee() {
        let utxo: MintlayerUtxo = serde_json::from_value(json!({
            "outpoint": {
                "index": 0,
                "source_id": SOURCE_ID,
                "source_type": "Transaction"
            },
            "utxo": {
                "destination": ADDRESS,
                "type": "Transfer",
                "value": {
                    "amount": {
                        "atoms": "10000000000",
                        "decimal": "0.1"
                    },
                    "type": "Coin"
                }
            }
        }))
        .unwrap();
        let fee_rate = MintlayerFeeRate::from_atoms_per_kb(MAINNET_MIN_RATE);
        let required_atoms = fee_rate.required_atoms(0, 175).unwrap();

        assert_eq!(
            select_spendable_coin_utxos(&[utxo], ADDRESS, required_atoms),
            Err(MintlayerUtxoSelectionError::InsufficientFunds {
                required_atoms: 17_500_000_000,
                available_atoms: 10_000_000_000,
            })
        );
    }
}
