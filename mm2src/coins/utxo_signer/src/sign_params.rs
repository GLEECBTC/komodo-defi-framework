use crate::{UtxoSignTxError, UtxoSignTxResult};
use chain::TransactionOutput;
use crypto::trezor::utxo::TrezorOutputScriptType;
use crypto::DerivationPath;
use keys::{AddressFormat, Public as PublicKey};
use mm2_err_handle::prelude::*;
use script::{SignatureVersion, TransactionInputSigner, UnsignedTransactionInput};

impl UtxoSignTxError {
    fn no_param(param: &str) -> UtxoSignTxError {
        UtxoSignTxError::InvalidSignParam {
            param: param.to_owned(),
            description: "not set".to_owned(),
        }
    }
}

/// An additional info of a spending input.
pub enum SpendingInputInfo {
    P2PKH {
        address_derivation_path: DerivationPath,
        address_pubkey: PublicKey,
    },
    P2WPKH {
        address_derivation_path: DerivationPath,
        address_pubkey: PublicKey,
    },
    // The fields are used to generate `trezor::proto::messages_bitcoin::MultisigRedeemScriptType`
    // P2SH {}
}

/// Either plain destination address or derivation path of a change address.
pub enum OutputDestination {
    Plain {
        address: String,
    },
    Change {
        derivation_path: DerivationPath,
        addr_format: AddressFormat,
    },
}

impl OutputDestination {
    pub fn plain(address: String) -> OutputDestination {
        OutputDestination::Plain { address }
    }

    #[inline]
    pub fn change(derivation_path: DerivationPath, addr_format: AddressFormat) -> OutputDestination {
        OutputDestination::Change {
            derivation_path,
            addr_format,
        }
    }
}

/// An additional info of a sending output.
pub struct SendingOutputInfo {
    pub destination_address: OutputDestination,
}

impl SendingOutputInfo {
    /// For now, returns [`TrezorOutputScriptType::PayToAddress`] since we don't support SLP tokens yet.
    #[inline]
    pub fn trezor_output_script_type(&self) -> TrezorOutputScriptType {
        match self.destination_address {
            OutputDestination::Change { ref addr_format, .. } if *addr_format == AddressFormat::Segwit => {
                TrezorOutputScriptType::PayToWitness
            },
            OutputDestination::Change { .. } | OutputDestination::Plain { .. } => TrezorOutputScriptType::PayToAddress,
        }
    }
}

pub struct UtxoSignTxParamsBuilder {
    signature_version: Option<SignatureVersion>,
    unsigned_tx: Option<TransactionInputSigner>,
    /// The number of elements is expected to be the same as `unsigned_tx.inputs.len()`.
    inputs_infos: Vec<SpendingInputInfo>,
    /// The number of elements is expected to be the same as `unsigned_tx.outputs.len()`.
    outputs_infos: Vec<SendingOutputInfo>,
}

impl Default for UtxoSignTxParamsBuilder {
    fn default() -> Self {
        UtxoSignTxParamsBuilder::new()
    }
}

impl UtxoSignTxParamsBuilder {
    pub fn new() -> UtxoSignTxParamsBuilder {
        UtxoSignTxParamsBuilder {
            signature_version: None,
            unsigned_tx: None,
            inputs_infos: Vec::new(),
            outputs_infos: Vec::new(),
        }
    }

    pub fn with_signature_version(&mut self, sig_ver: SignatureVersion) -> &mut UtxoSignTxParamsBuilder {
        self.signature_version = Some(sig_ver);
        self
    }

    pub fn with_unsigned_tx(&mut self, unsigned_tx: TransactionInputSigner) -> &mut UtxoSignTxParamsBuilder {
        self.unsigned_tx = Some(unsigned_tx);
        self
    }

    pub fn add_inputs_infos<I>(&mut self, inputs: I) -> &mut UtxoSignTxParamsBuilder
    where
        I: IntoIterator<Item = SpendingInputInfo>,
    {
        self.inputs_infos.extend(inputs);
        self
    }

    pub fn add_outputs_infos<I>(&mut self, outputs: I) -> &mut UtxoSignTxParamsBuilder
    where
        I: IntoIterator<Item = SendingOutputInfo>,
    {
        self.outputs_infos.extend(outputs);
        self
    }

    pub fn build(self) -> UtxoSignTxResult<UtxoSignTxParams> {
        let unsigned_tx = self
            .unsigned_tx
            .or_mm_err(|| UtxoSignTxError::no_param("unsigned_tx"))?;

        if self.inputs_infos.len() != unsigned_tx.inputs.len() {
            let description = format!(
                "found '{}' inputs, expected '{}'",
                self.inputs_infos.len(),
                unsigned_tx.inputs.len()
            );
            let param = "inputs_infos".to_owned();
            return MmError::err(UtxoSignTxError::InvalidSignParam { param, description });
        }

        if self.outputs_infos.len() != unsigned_tx.outputs.len() {
            let description = format!(
                "found '{}' outputs, expected '{}'",
                self.outputs_infos.len(),
                unsigned_tx.outputs.len()
            );
            let param = "outputs_infos".to_owned();
            return MmError::err(UtxoSignTxError::InvalidSignParam { param, description });
        }

        let params = UtxoSignTxParams {
            signature_version: self
                .signature_version
                .or_mm_err(|| UtxoSignTxError::no_param("signature_version"))?,
            unsigned_tx,
            inputs_infos: self.inputs_infos,
            outputs_infos: self.outputs_infos,
        };
        Ok(params)
    }
}

pub struct UtxoSignTxParams {
    pub(crate) signature_version: SignatureVersion,
    pub(crate) unsigned_tx: TransactionInputSigner,
    /// The number of elements is exactly the same as `unsigned_tx.inputs.len()`.
    pub(crate) inputs_infos: Vec<SpendingInputInfo>,
    /// The number of elements is exactly the same as `unsigned_tx.outputs.len()`.
    pub(crate) outputs_infos: Vec<SendingOutputInfo>,
}

impl UtxoSignTxParams {
    pub fn inputs_count(&self) -> usize {
        self.unsigned_tx.inputs.len()
    }

    /// We are sure that the number of `unsigned_tx.inputs.len()` is the same as `inputs_infos.len()`.
    /// Please see [`UtxoSignTxParamsBuilder::build`].
    pub fn inputs(&self) -> impl Iterator<Item = (&UnsignedTransactionInput, &SpendingInputInfo)> {
        assert_eq!(
            self.unsigned_tx.inputs.len(),
            self.inputs_infos.len(),
            "'unsigned_tx.inputs' and 'inputs_infos' must be checked"
        );
        self.unsigned_tx.inputs.iter().zip(self.inputs_infos.iter())
    }

    /// We are sure that the number of `unsigned_tx.outputs.len()` is the same as `outputs_infos.len()`.
    /// Please see [`UtxoSignTxParamsBuilder::build`].
    pub fn outputs(&self) -> impl Iterator<Item = (&TransactionOutput, &SendingOutputInfo)> {
        assert_eq!(
            self.unsigned_tx.outputs.len(),
            self.outputs_infos.len(),
            "'unsigned_tx.outputs' and 'outputs_infos' must be checked"
        );
        self.unsigned_tx.outputs.iter().zip(self.outputs_infos.iter())
    }
}
