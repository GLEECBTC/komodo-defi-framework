//! Unified fee details enum across all supported blockchain protocols.
//!
//! `TxFeeDetails` serializes with a `"type"` tag for outbound JSON, but deserializes
//! as untagged to accept responses without the discriminator field.

use crate::eth::tron::fee::TronTxFeeDetails;
use crate::eth::EthTxFeeDetails;
use crate::qrc20::Qrc20FeeDetails;
use crate::siacoin::SiaFeeDetails;
use crate::solana::SolanaFeeDetails;
use crate::tendermint::TendermintFeeDetails;
use crate::utxo::UtxoFeeDetails;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum TxFeeDetails {
    Utxo(UtxoFeeDetails),
    Eth(EthTxFeeDetails),
    Tron(TronTxFeeDetails),
    Qrc20(Qrc20FeeDetails),
    Slp(crate::utxo::slp::SlpFeeDetails),
    Tendermint(TendermintFeeDetails),
    Sia(SiaFeeDetails),
    Solana(SolanaFeeDetails),
}

/// Deserialize the TxFeeDetails as an untagged enum.
impl<'de> Deserialize<'de> for TxFeeDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, <D as Deserializer<'de>>::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum TxFeeDetailsUnTagged {
            Utxo(UtxoFeeDetails),
            Eth(EthTxFeeDetails),
            Tron(TronTxFeeDetails),
            Qrc20(Qrc20FeeDetails),
            Slp(crate::utxo::slp::SlpFeeDetails),
            Tendermint(TendermintFeeDetails),
            Sia(SiaFeeDetails),
            Solana(SolanaFeeDetails),
        }

        match Deserialize::deserialize(deserializer)? {
            TxFeeDetailsUnTagged::Utxo(f) => Ok(TxFeeDetails::Utxo(f)),
            TxFeeDetailsUnTagged::Eth(f) => Ok(TxFeeDetails::Eth(f)),
            TxFeeDetailsUnTagged::Tron(f) => Ok(TxFeeDetails::Tron(f)),
            TxFeeDetailsUnTagged::Qrc20(f) => Ok(TxFeeDetails::Qrc20(f)),
            TxFeeDetailsUnTagged::Slp(f) => Ok(TxFeeDetails::Slp(f)),
            TxFeeDetailsUnTagged::Tendermint(f) => Ok(TxFeeDetails::Tendermint(f)),
            TxFeeDetailsUnTagged::Sia(f) => Ok(TxFeeDetails::Sia(f)),
            TxFeeDetailsUnTagged::Solana(f) => Ok(TxFeeDetails::Solana(f)),
        }
    }
}

impl From<EthTxFeeDetails> for TxFeeDetails {
    fn from(eth_details: EthTxFeeDetails) -> Self {
        TxFeeDetails::Eth(eth_details)
    }
}

impl From<TronTxFeeDetails> for TxFeeDetails {
    fn from(tron_details: TronTxFeeDetails) -> Self {
        TxFeeDetails::Tron(tron_details)
    }
}

impl From<UtxoFeeDetails> for TxFeeDetails {
    fn from(utxo_details: UtxoFeeDetails) -> Self {
        TxFeeDetails::Utxo(utxo_details)
    }
}

impl From<Qrc20FeeDetails> for TxFeeDetails {
    fn from(qrc20_details: Qrc20FeeDetails) -> Self {
        TxFeeDetails::Qrc20(qrc20_details)
    }
}

impl From<SiaFeeDetails> for TxFeeDetails {
    fn from(sia_details: SiaFeeDetails) -> Self {
        TxFeeDetails::Sia(sia_details)
    }
}

impl From<TendermintFeeDetails> for TxFeeDetails {
    fn from(tendermint_details: TendermintFeeDetails) -> Self {
        TxFeeDetails::Tendermint(tendermint_details)
    }
}
