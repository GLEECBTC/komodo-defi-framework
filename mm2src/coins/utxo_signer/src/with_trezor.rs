use crate::sign_common::{complete_tx, p2pkh_spend_with_signature, p2wpkh_spend_with_signature};
use crate::sign_params::{OutputDestination, SendingOutputInfo, SpendingInputInfo, UtxoSignTxParams};
use crate::{TxProvider, UtxoSignTxError, UtxoSignTxResult};
use chain::{Transaction as UtxoTx, TransactionOutput};
use common::log::debug;
use crypto::trezor::utxo::{
    PrevTx, PrevTxInput, PrevTxOutput, TrezorInputScriptType, TxOutput, TxSignResult, UnsignedTxInput, UnsignedUtxoTx,
};
use crypto::trezor::TrezorSession;
use keys::bytes::Bytes;
use mm2_err_handle::prelude::*;
use rpc::v1::types::H256 as H256Json;
use script::UnsignedTransactionInput;
use serialization::deserialize;

pub struct TrezorTxSigner<'a, TxP> {
    pub trezor: TrezorSession<'a>,
    pub tx_provider: TxP,
    pub trezor_coin: String,
    pub params: UtxoSignTxParams,
    pub fork_id: u32,
    pub branch_id: u32,
}

impl<TxP: TxProvider + Send + Sync> TrezorTxSigner<'_, TxP> {
    pub async fn sign_tx(mut self) -> UtxoSignTxResult<UtxoTx> {
        let trezor_unsigned_tx = self.get_trezor_unsigned_tx().await.map_mm_err()?;

        let TxSignResult {
            signatures,
            serialized_tx,
        } = self.trezor.sign_utxo_tx(trezor_unsigned_tx).await.map_mm_err()?;
        debug!("Transaction signed by Trezor: {}", hex::encode(serialized_tx));
        if signatures.len() != self.params.inputs_count() {
            return MmError::err(UtxoSignTxError::InvalidSignaturesNumber {
                actual: signatures.len(),
                expected: self.params.inputs_count(),
            });
        }

        let signed_inputs = self
            .params
            .inputs()
            .zip(signatures.into_iter())
            .map(|((unsigned_input, input_info), signature)| match input_info {
                SpendingInputInfo::P2PKH { address_pubkey, .. } => {
                    p2pkh_spend_with_signature(unsigned_input, address_pubkey, self.fork_id, Bytes::from(signature))
                },
                SpendingInputInfo::P2WPKH { address_pubkey, .. } => {
                    p2wpkh_spend_with_signature(unsigned_input, address_pubkey, self.fork_id, Bytes::from(signature))
                },
            })
            .collect();
        Ok(complete_tx(self.params.unsigned_tx, signed_inputs))
    }

    async fn get_trezor_unsigned_tx(&self) -> UtxoSignTxResult<UnsignedUtxoTx> {
        let mut inputs = Vec::with_capacity(self.params.unsigned_tx.inputs.len());
        for (unsigned_input, input_info) in self.params.inputs() {
            let unsigned_input = self
                .get_trezor_unsigned_input(unsigned_input, input_info)
                .await
                .map_mm_err()?;
            inputs.push(unsigned_input);
        }

        let outputs = self
            .params
            .outputs()
            .map(|(tx_output, output_info)| self.get_trezor_output(tx_output, output_info))
            .collect();

        Ok(UnsignedUtxoTx {
            coin: self.trezor_coin.clone(),
            inputs,
            outputs,
            version: self.params.unsigned_tx.version as u32,
            lock_time: self.params.unsigned_tx.lock_time,
            expiry: self.expiry_if_required(self.params.unsigned_tx.expiry_height),
            version_group_id: self.version_group_id(),
            branch_id: self.branch_id(),
        })
    }

    fn get_trezor_output(&self, tx_output: &TransactionOutput, output_info: &SendingOutputInfo) -> TxOutput {
        let (address, address_derivation_path) = match output_info.destination_address {
            OutputDestination::Plain { ref address } => (Some(address.clone()), None),
            OutputDestination::Change {
                ref derivation_path, ..
            } => (None, Some(derivation_path.clone())),
        };
        TxOutput {
            address,
            address_derivation_path,
            amount: tx_output.value,
            script_type: output_info.trezor_output_script_type(),
        }
    }

    async fn get_trezor_unsigned_input(
        &self,
        unsigned_input: &UnsignedTransactionInput,
        input_info: &SpendingInputInfo,
    ) -> UtxoSignTxResult<UnsignedTxInput> {
        let prev_tx_hash_json = H256Json::from(unsigned_input.previous_output.hash.reversed());
        let prev_tx = self.get_trezor_prev_tx(&prev_tx_hash_json).await.map_mm_err()?;

        let (address_derivation_path, input_script_type) = match input_info {
            SpendingInputInfo::P2PKH {
                address_derivation_path,
                ..
            } => (
                Some(address_derivation_path.clone()),
                TrezorInputScriptType::SpendAddress,
            ),
            SpendingInputInfo::P2WPKH {
                address_derivation_path,
                ..
            } => (
                Some(address_derivation_path.clone()),
                TrezorInputScriptType::SpendWitness,
            ),
        };

        Ok(UnsignedTxInput {
            address_derivation_path,
            prev_tx,
            prev_hash: unsigned_input.previous_output.hash.reversed().to_vec(),
            prev_index: unsigned_input.previous_output.index,
            sequence: unsigned_input.sequence,
            input_script_type,
            amount: unsigned_input.amount,
        })
    }

    async fn get_trezor_prev_tx(&self, prev_tx_hash: &H256Json) -> UtxoSignTxResult<PrevTx> {
        let prev_verbose = self.tx_provider.get_rpc_transaction(prev_tx_hash).await.map_mm_err()?;
        let prev_utxo: UtxoTx = deserialize(prev_verbose.hex.as_slice())
            .map_to_mm(|e| UtxoSignTxError::Transport(e.to_string()))
            .map_mm_err()?;

        let prev_tx_inputs = prev_utxo
            .inputs
            .into_iter()
            .map(|prev_tx_input| PrevTxInput {
                prev_hash: prev_tx_input.previous_output.hash.reversed().to_vec(),
                prev_index: prev_tx_input.previous_output.index,
                script_sig: prev_tx_input.script_sig.to_vec(),
                sequence: prev_tx_input.sequence,
            })
            .collect();
        let prev_tx_outputs = prev_utxo
            .outputs
            .into_iter()
            .map(|prev_tx_output| PrevTxOutput {
                amount: prev_tx_output.value,
                script_pubkey: prev_tx_output.script_pubkey.to_vec(),
            })
            .collect();
        Ok(PrevTx {
            inputs: prev_tx_inputs,
            outputs: prev_tx_outputs,
            version: prev_utxo.version as u32,
            lock_time: prev_utxo.lock_time,
            expiry: self.expiry_if_required(prev_utxo.expiry_height),
            version_group_id: self.version_group_id(),
            branch_id: self.branch_id(),
            extra_data: self.extra_data(),
        })
    }

    /// `version_group_id` must be set for Zcash coins *only*.
    fn version_group_id(&self) -> Option<u32> {
        if self.is_overwinter_compatible() {
            Some(self.params.unsigned_tx.version_group_id)
        } else {
            None
        }
    }

    /// `branch_id` must be set for Zcash coins *only*.
    fn branch_id(&self) -> Option<u32> {
        if self.is_overwinter_compatible() {
            Some(self.branch_id)
        } else {
            None
        }
    }

    /// `expiry` must be set for Decred and Zcash coins *only*.
    /// This fixes : https://github.com/KomodoPlatform/atomicDEX-API/issues/1626
    fn expiry_if_required(&self, tx_expiry: u32) -> Option<u32> {
        if self.is_overwinter_compatible() {
            Some(tx_expiry)
        } else {
            None
        }
    }

    /// Temporary use `0000000000000000000000` extra data for Zcash coins *only*.
    /// https://github.com/trezor/connect/issues/610#issuecomment-646022404
    fn extra_data(&self) -> Vec<u8> {
        if self.is_overwinter_compatible() {
            vec![0; 11]
        } else {
            Vec::new()
        }
    }

    /// https://github.com/trezor/trezor-utxo-lib/blob/trezor/src/transaction.js#L405
    fn is_overwinter_compatible(&self) -> bool {
        self.is_zcash_type() && self.params.unsigned_tx.version > 3
    }

    /// https://github.com/trezor/trezor-utxo-lib/blob/trezor/src/coins.js#L55
    fn is_zcash_type(&self) -> bool {
        matches!(self.trezor_coin.as_str(), "Komodo" | "Zcash" | "Zcash Testnet")
    }
}
