use std::convert::TryFrom;

use crate::proto::messages_bitcoin as proto_bitcoin;
use crate::result_handler::ResultHandler;
use crate::utxo::unsigned_tx::UnsignedUtxoTx;
use crate::utxo::Signature;
use crate::{ProcessTrezorResponse, TrezorError, TrezorResponse, TrezorResult, TrezorSession};
use common::log::{debug, info};
use mm2_err_handle::prelude::*;

const NO_DETAILS_ERROR: &str = "'TxRequest::details' is expected to be set";
const NO_REQUEST_INDEX_ERROR: &str = "'TxRequestDetailsType::request_index' is expected to be set";
const NO_EXTRA_DATA_OFFSET_ERROR: &str = "'TxRequestDetailsType::extra_data_offset' is expected to be set";
const NO_EXTRA_DATA_LEN_ERROR: &str = "'TxRequestDetailsType::extra_data_len' is expected to be set";
const NO_SIGNATURE_INDEX_ERROR: &str = "'TxRequestSerializedType::signature_index' is expected to be set";

pub struct TxSignResult {
    pub signatures: Vec<Signature>,
    pub serialized_tx: Vec<u8>,
}

impl TxSignResult {
    fn new_with_inputs_count(inputs_count: usize) -> TxSignResult {
        TxSignResult {
            signatures: vec![Signature::new(); inputs_count],
            serialized_tx: Vec::new(),
        }
    }
}

impl<'a> TrezorSession<'a> {
    /// https://docs.trezor.io/trezor-firmware/common/communication/bitcoin-signing.html#pseudo-code
    /// TODO add a `timeout` param.
    ///
    /// # Fail
    ///
    /// Currently, this method fails if a device requests a PIN.
    pub async fn sign_utxo_tx(&mut self, unsigned: UnsignedUtxoTx) -> TrezorResult<TxSignResult> {
        use proto_bitcoin::tx_request::RequestType as ProtoTxRequestType;

        let mut result = TxSignResult::new_with_inputs_count(unsigned.inputs.len());
        let processor = self
            .processor
            .as_ref()
            .or_mm_err(|| TrezorError::InternalNoProcessor)?
            .clone();
        // Please note `tx_request` is changed within the following loop.
        let mut tx_request = self
            .sign_tx(unsigned.sign_tx_message())
            .await?
            .process(processor)
            .await
            .mm_err(|e| TrezorError::Internal(e.to_string()))?;

        info!(
            "Start transaction signing: COIN={} INPUTS_COUNT={} OUTPUTS_COUNT={} OVERWINTERED={}",
            unsigned.coin,
            unsigned.inputs.len(),
            unsigned.outputs.len(),
            unsigned.version_group_id.is_some() || unsigned.branch_id.is_some()
        );

        loop {
            extract_serialized_data(&tx_request, &mut result)?;

            let request_type = tx_request
                .request_type
                .and_then(|t| ProtoTxRequestType::try_from(t).ok());
            let request_type = match request_type {
                Some(ProtoTxRequestType::Txfinished) => return Ok(result),
                Some(req_type) => req_type,
                None => {
                    let error = format!(
                        "Received unexpected 'TxRequest::request_type': {:?}",
                        tx_request.request_type
                    );
                    return MmError::err(TrezorError::ProtocolError(error));
                },
            };

            let tx_request_details = match tx_request.details {
                Some(ref details) => details,
                None => return MmError::err(TrezorError::ProtocolError(NO_DETAILS_ERROR.to_owned())),
            };

            let is_prev = tx_request_details.tx_hash.is_some();
            debug!("TxRequest: REQUEST_TYPE={:?} PREV={}", request_type, is_prev);

            tx_request = match (request_type, &tx_request_details.tx_hash) {
                (ProtoTxRequestType::Txinput, Some(prev_hash)) => {
                    self.send_prev_input(&unsigned, tx_request_details, prev_hash).await?
                },
                (ProtoTxRequestType::Txinput, None) => self.send_input(&unsigned, tx_request_details).await?,
                (ProtoTxRequestType::Txoutput, Some(prev_hash)) => {
                    self.send_prev_output(&unsigned, tx_request_details, prev_hash).await?
                },
                (ProtoTxRequestType::Txoutput, None) => self.send_output(&unsigned, tx_request_details).await?,
                (ProtoTxRequestType::Txmeta, Some(prev_hash)) => self.send_prev_tx_meta(&unsigned, prev_hash).await?,
                (ProtoTxRequestType::Txextradata, Some(prev_hash)) => {
                    self.send_extra_data(&unsigned, tx_request_details, prev_hash).await?
                },
                _ => {
                    let error = format!("Unexpected tx request: {request_type:?}, is_prev: {is_prev}");
                    return MmError::err(TrezorError::ProtocolError(error));
                },
            };
        }
    }

    async fn send_prev_tx_meta(
        &mut self,
        unsigned: &UnsignedUtxoTx,
        prev_tx_hash: &[u8],
    ) -> TrezorResult<proto_bitcoin::TxRequest> {
        let prev_tx = unsigned.prev_tx(prev_tx_hash)?;
        let req = prev_tx.meta_message();

        let result_handler = ResultHandler::<proto_bitcoin::TxRequest>::new(Ok);
        let processor = self
            .processor
            .as_ref()
            .or_mm_err(|| TrezorError::InternalNoProcessor)?
            .clone();
        self.call(req, result_handler)
            .await?
            .process(processor)
            .await
            .mm_err(|e| TrezorError::Internal(e.to_string()))
    }

    async fn send_prev_input(
        &mut self,
        unsigned: &UnsignedUtxoTx,
        request_details: &proto_bitcoin::tx_request::TxRequestDetailsType,
        prev_tx_hash: &[u8],
    ) -> TrezorResult<proto_bitcoin::TxRequest> {
        let prev_input_index = request_details
            .request_index
            .or_mm_err(|| TrezorError::ProtocolError(NO_REQUEST_INDEX_ERROR.to_owned()))?
            as usize;

        let prev_tx = unsigned.prev_tx(prev_tx_hash)?;
        let req = prev_tx.input_message(prev_input_index)?;

        let result_handler = ResultHandler::<proto_bitcoin::TxRequest>::new(Ok);
        let processor = self
            .processor
            .as_ref()
            .or_mm_err(|| TrezorError::InternalNoProcessor)?
            .clone();
        self.call(req, result_handler)
            .await?
            .process(processor)
            .await
            .mm_err(|e| TrezorError::Internal(e.to_string()))
    }

    async fn send_prev_output(
        &mut self,
        unsigned: &UnsignedUtxoTx,
        request_details: &proto_bitcoin::tx_request::TxRequestDetailsType,
        prev_tx_hash: &[u8],
    ) -> TrezorResult<proto_bitcoin::TxRequest> {
        let prev_output_index = request_details
            .request_index
            .or_mm_err(|| TrezorError::ProtocolError(NO_REQUEST_INDEX_ERROR.to_owned()))?
            as usize;

        let prev_tx = unsigned.prev_tx(prev_tx_hash)?;
        let req = prev_tx.output_message(prev_output_index)?;

        let result_handler = ResultHandler::<proto_bitcoin::TxRequest>::new(Ok);
        let processor = self
            .processor
            .as_ref()
            .or_mm_err(|| TrezorError::InternalNoProcessor)?
            .clone();
        self.call(req, result_handler)
            .await?
            .process(processor)
            .await
            .mm_err(|e| TrezorError::Internal(e.to_string()))
    }

    async fn send_input(
        &mut self,
        unsigned: &UnsignedUtxoTx,
        request_details: &proto_bitcoin::tx_request::TxRequestDetailsType,
    ) -> TrezorResult<proto_bitcoin::TxRequest> {
        let input_index = request_details
            .request_index
            .or_mm_err(|| TrezorError::ProtocolError(NO_REQUEST_INDEX_ERROR.to_owned()))?
            as usize;
        let req = unsigned.input_message(input_index)?;

        let result_handler = ResultHandler::<proto_bitcoin::TxRequest>::new(Ok);
        let processor = self
            .processor
            .as_ref()
            .or_mm_err(|| TrezorError::InternalNoProcessor)?
            .clone();
        self.call(req, result_handler)
            .await?
            .process(processor)
            .await
            .mm_err(|e| TrezorError::Internal(e.to_string()))
    }

    async fn send_output(
        &mut self,
        unsigned: &UnsignedUtxoTx,
        request_details: &proto_bitcoin::tx_request::TxRequestDetailsType,
    ) -> TrezorResult<proto_bitcoin::TxRequest> {
        let output_index = request_details
            .request_index
            .or_mm_err(|| TrezorError::ProtocolError(NO_REQUEST_INDEX_ERROR.to_owned()))?
            as usize;
        let req = unsigned.output_message(output_index)?;

        let result_handler = ResultHandler::<proto_bitcoin::TxRequest>::new(Ok);
        let processor = self
            .processor
            .as_ref()
            .or_mm_err(|| TrezorError::InternalNoProcessor)?
            .clone();
        self.call(req, result_handler)
            .await?
            .process(processor)
            .await
            .mm_err(|e| TrezorError::Internal(e.to_string()))
    }

    async fn send_extra_data(
        &mut self,
        unsigned: &UnsignedUtxoTx,
        request_details: &proto_bitcoin::tx_request::TxRequestDetailsType,
        prev_tx_hash: &[u8],
    ) -> TrezorResult<proto_bitcoin::TxRequest> {
        let offset = request_details
            .extra_data_offset
            .or_mm_err(|| TrezorError::ProtocolError(NO_EXTRA_DATA_OFFSET_ERROR.to_owned()))?
            as usize;
        let len = request_details
            .extra_data_len
            .or_mm_err(|| TrezorError::ProtocolError(NO_EXTRA_DATA_LEN_ERROR.to_owned()))? as usize;

        let prev_tx = unsigned.prev_tx(prev_tx_hash)?;
        let req = prev_tx.extra_data_message(offset, len)?;

        let result_handler = ResultHandler::<proto_bitcoin::TxRequest>::new(Ok);
        let processor = self
            .processor
            .as_ref()
            .or_mm_err(|| TrezorError::InternalNoProcessor)?
            .clone();
        self.call(req, result_handler)
            .await?
            .process(processor)
            .await
            .mm_err(|e| TrezorError::Internal(e.to_string()))
    }

    async fn sign_tx(
        &mut self,
        req: proto_bitcoin::SignTx,
    ) -> TrezorResult<TrezorResponse<'a, '_, proto_bitcoin::TxRequest>> {
        let result_handler = ResultHandler::<proto_bitcoin::TxRequest>::new(Ok);
        self.call(req, result_handler).await
    }
}

fn extract_serialized_data(tx_request: &proto_bitcoin::TxRequest, result: &mut TxSignResult) -> TrezorResult<()> {
    let serialized = match tx_request.serialized {
        Some(ref serialized) => serialized,
        None => return Ok(()),
    };

    if let Some(signature) = serialized.signature.clone() {
        let input_index = serialized
            .signature_index
            .or_mm_err(|| TrezorError::ProtocolError(NO_SIGNATURE_INDEX_ERROR.to_owned()))?
            as usize;
        if input_index >= result.signatures.len() {
            let error = format!(
                "Received a signature of unknown Transaction Input: {}. Number of inputs: {}",
                input_index,
                result.signatures.len()
            );
            return MmError::err(TrezorError::ProtocolError(error));
        }

        result.signatures[input_index] = signature;
    }

    if let Some(serialized_tx) = serialized.serialized_tx.as_ref() {
        result.serialized_tx.extend_from_slice(serialized_tx);
    }

    Ok(())
}
