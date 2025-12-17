/// https://bchd.cash/
/// https://bchd.fountainhead.cash/
use super::bchd_pb::*;
use crate::{coin_errors::ValidatePaymentError, utxo::slp::SlpUnspent};
use chain::OutPoint;
use derive_more::Display;
use futures::future::join_all;
use futures::FutureExt;
use get_slp_trusted_validation_response::validity_result::ValidityResultType;
use keys::hash::H256;
use mm2_err_handle::prelude::*;
use mm2_net::grpc_web::{post_grpc_web, PostGrpcWebErr};
use std::convert::TryInto;

#[derive(Debug, Display)]
#[display(fmt = "Error {err:?} on request to the url {to_url}")]
pub struct GrpcWebMultiUrlReqErr {
    to_url: String,
    err: PostGrpcWebErr,
}

/// This fn will simply return Ok() if urls are empty.
/// It is intended behaviour to make "unsafe" mode possible for BCH.
#[allow(clippy::needless_lifetimes)]
async fn grpc_web_multi_url_request<'a, Req, Res, Url>(
    urls: &'a [Url],
    req: &'a Req,
) -> Result<Vec<(&'a Url, Res)>, MmError<GrpcWebMultiUrlReqErr>>
where
    Req: prost::Message + Send + 'static,
    Res: prost::Message + Default + Send + 'static,
    Url: AsRef<str>,
{
    let futures = urls
        .iter()
        .map(|url| post_grpc_web::<_, Res>(url.as_ref(), req).map(move |res| (url, res)));

    join_all(futures)
        .await
        .into_iter()
        .map(|(url, response)| {
            Ok((
                url,
                response.mm_err(|err| GrpcWebMultiUrlReqErr {
                    to_url: url.as_ref().to_string(),
                    err,
                })?,
            ))
        })
        .collect()
}

#[derive(Debug, Display)]
pub enum ValidateSlpUtxosErrKind {
    MultiReqErr(GrpcWebMultiUrlReqErr),
    #[display(fmt = "Expected {expected} token id, but got {actual}")]
    UnexpectedTokenId {
        expected: H256,
        actual: H256,
    },
    #[display(fmt = "Unexpected validity_result {validity_result:?} for unspent {for_unspent:?}")]
    UnexpectedValidityResultType {
        for_unspent: SlpUnspent,
        validity_result: Option<ValidityResultType>,
    },
    #[display(fmt = "Unexpected utxo {outpoint:?} in response")]
    UnexpectedUtxoInResponse {
        outpoint: OutPoint,
    },
    InvalidSlpTxData(String),
}

#[derive(Debug, Display)]
#[display(fmt = "Error {kind} on request to the url {to_url}")]
pub struct ValidateSlpUtxosErr {
    to_url: String,
    kind: ValidateSlpUtxosErrKind,
}

impl From<ValidateSlpUtxosErr> for ValidatePaymentError {
    fn from(err: ValidateSlpUtxosErr) -> Self {
        match err.kind {
            ValidateSlpUtxosErrKind::MultiReqErr(_) => Self::Transport(err.to_string()),
            ValidateSlpUtxosErrKind::InvalidSlpTxData(_) => Self::WrongPaymentTx(err.to_string()),
            _ => Self::InvalidRpcResponse(err.to_string()),
        }
    }
}

impl From<GrpcWebMultiUrlReqErr> for ValidateSlpUtxosErr {
    fn from(err: GrpcWebMultiUrlReqErr) -> Self {
        match err.err {
            // For some reason, BCHD responds with empty payload (Which is mapped to PostGrpcWebErr::PayloadTooShort error) in cases where an invalid UTXO is used with grpc-web.
            // while it does provide meaningful error response when use with gRPC HTTP2 TLS.
            PostGrpcWebErr::PayloadTooShort(e) => ValidateSlpUtxosErr {
                to_url: err.to_url.clone(),
                kind: ValidateSlpUtxosErrKind::InvalidSlpTxData(e),
            },
            _ => ValidateSlpUtxosErr {
                to_url: err.to_url.clone(),
                kind: ValidateSlpUtxosErrKind::MultiReqErr(err),
            },
        }
    }
}

pub async fn validate_slp_utxos(
    bchd_urls: &[impl AsRef<str>],
    utxos: &[SlpUnspent],
    token_id: &H256,
) -> Result<(), MmError<ValidateSlpUtxosErr>> {
    let queries = utxos
        .iter()
        .map(|utxo| get_slp_trusted_validation_request::Query {
            prev_out_hash: utxo.bch_unspent.outpoint.hash.take().into(),
            prev_out_vout: utxo.bch_unspent.outpoint.index,
            graphsearch_valid_hashes: Vec::new(),
        })
        .collect();
    let request = GetSlpTrustedValidationRequest {
        queries,
        include_graphsearch_count: false,
    };

    let urls: Vec<_> = bchd_urls
        .iter()
        .map(|url| url.as_ref().to_owned() + "/pb.bchrpc/GetSlpTrustedValidation")
        .collect();
    let responses: Vec<(_, GetSlpTrustedValidationResponse)> =
        grpc_web_multi_url_request(&urls, &request).await.map_mm_err()?;
    for (url, response) in responses {
        for validation_result in response.results {
            let actual_token_id = {
                let token_id_len = validation_result.token_id.len();
                let arr: [u8; 32] = validation_result
                    .token_id
                    .try_into()
                    .map_to_mm(|_| ValidateSlpUtxosErr {
                        to_url: url.clone(),
                        kind: ValidateSlpUtxosErrKind::InvalidSlpTxData(format!(
                            "Invalid token_id length: expected 32 bytes, got {token_id_len}"
                        )),
                    })?;
                arr.into()
            };
            if actual_token_id != *token_id {
                return MmError::err(ValidateSlpUtxosErr {
                    to_url: url.clone(),
                    kind: ValidateSlpUtxosErrKind::UnexpectedTokenId {
                        expected: *token_id,
                        actual: actual_token_id,
                    },
                });
            }

            let prev_out_hash = {
                let prev_out_hash_len = validation_result.prev_out_hash.len();
                let arr: [u8; 32] = validation_result
                    .prev_out_hash
                    .try_into()
                    .map_to_mm(|_| ValidateSlpUtxosErr {
                        to_url: url.clone(),
                        kind: ValidateSlpUtxosErrKind::InvalidSlpTxData(format!(
                            "Invalid prev_out_hash length: expected 32 bytes, got {prev_out_hash_len}"
                        )),
                    })?;
                arr.into()
            };

            let outpoint = OutPoint {
                hash: prev_out_hash,
                index: validation_result.prev_out_vout,
            };

            let initial_unspent = utxos
                .iter()
                .find(|unspent| unspent.bch_unspent.outpoint == outpoint)
                .or_mm_err(|| ValidateSlpUtxosErr {
                    to_url: url.clone(),
                    kind: ValidateSlpUtxosErrKind::UnexpectedUtxoInResponse { outpoint },
                })?;

            match validation_result.validity_result_type {
                Some(ValidityResultType::V1TokenAmount(slp_amount)) => {
                    if slp_amount != initial_unspent.slp_amount {
                        return MmError::err(ValidateSlpUtxosErr {
                            to_url: url.clone(),
                            kind: ValidateSlpUtxosErrKind::UnexpectedValidityResultType {
                                for_unspent: initial_unspent.clone(),
                                validity_result: validation_result.validity_result_type,
                            },
                        });
                    }
                },
                _ => {
                    return MmError::err(ValidateSlpUtxosErr {
                        to_url: url.clone(),
                        kind: ValidateSlpUtxosErrKind::UnexpectedValidityResultType {
                            for_unspent: initial_unspent.clone(),
                            validity_result: validation_result.validity_result_type,
                        },
                    })
                },
            }
        }
    }
    Ok(())
}

#[derive(Debug, Display)]
pub enum CheckSlpTransactionErrKind {
    MultiReqErr(GrpcWebMultiUrlReqErr),
    #[display(fmt = "Transaction {transaction:?} is not valid with reason {reason}")]
    InvalidTransaction {
        transaction: Vec<u8>,
        reason: String,
    },
}

#[derive(Debug, Display)]
#[display(fmt = "Error {kind} on request to the url {to_url}")]
pub struct CheckSlpTransactionErr {
    to_url: String,
    kind: CheckSlpTransactionErrKind,
}

impl From<GrpcWebMultiUrlReqErr> for CheckSlpTransactionErr {
    fn from(err: GrpcWebMultiUrlReqErr) -> Self {
        CheckSlpTransactionErr {
            to_url: err.to_url.clone(),
            kind: CheckSlpTransactionErrKind::MultiReqErr(err),
        }
    }
}

pub async fn check_slp_transaction(
    bchd_urls: &[impl AsRef<str>],
    transaction: Vec<u8>,
) -> Result<(), MmError<CheckSlpTransactionErr>> {
    let request = CheckSlpTransactionRequest {
        transaction,
        required_slp_burns: Vec::new(),
        use_spec_validity_judgement: false,
    };

    let urls: Vec<_> = bchd_urls
        .iter()
        .map(|url| url.as_ref().to_owned() + "/pb.bchrpc/CheckSlpTransaction")
        .collect();

    let responses: Vec<(_, CheckSlpTransactionResponse)> =
        grpc_web_multi_url_request(&urls, &request).await.map_mm_err()?;
    for (url, response) in responses {
        if !response.is_valid {
            return MmError::err(CheckSlpTransactionErr {
                to_url: url.clone(),
                kind: CheckSlpTransactionErrKind::InvalidTransaction {
                    transaction: request.transaction,
                    reason: response.invalid_reason,
                },
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod bchd_grpc_tests {
    use super::*;
    use crate::utxo::rpc_clients::UnspentInfo;
    use common::block_on;
    use mm2_test_helpers::for_tests::BCHD_TESTNET_URLS;

    #[test]
    #[ignore]
    fn test_validate_slp_utxos_valid() {
        let tx_hash = H256::from_reversed_str("0ba1b91abbfceaa0777424165edb2928dace87d59669c913989950da31968032");

        let slp_utxos = [
            SlpUnspent {
                bch_unspent: UnspentInfo {
                    outpoint: OutPoint {
                        hash: tx_hash,
                        index: 1,
                    },
                    value: 0,
                    height: None,
                    script: Vec::new().into(),
                },
                slp_amount: 1000,
            },
            SlpUnspent {
                bch_unspent: UnspentInfo {
                    outpoint: OutPoint {
                        hash: tx_hash,
                        index: 2,
                    },
                    value: 0,
                    height: None,
                    script: Vec::new().into(),
                },
                slp_amount: 8999,
            },
        ];

        let token_id = H256::from("bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb7");
        block_on(validate_slp_utxos(BCHD_TESTNET_URLS, &slp_utxos, &token_id)).unwrap();
    }

    #[test]
    #[ignore]
    fn test_validate_slp_utxos_non_slp_input() {
        let tx_hash = H256::from_reversed_str("0ba1b91abbfceaa0777424165edb2928dace87d59669c913989950da31968032");

        let slp_utxos = [
            SlpUnspent {
                bch_unspent: UnspentInfo {
                    outpoint: OutPoint {
                        hash: tx_hash,
                        index: 1,
                    },
                    value: 0,
                    height: None,
                    script: Vec::new().into(),
                },
                slp_amount: 1000,
            },
            SlpUnspent {
                bch_unspent: UnspentInfo {
                    outpoint: OutPoint {
                        hash: tx_hash,
                        index: 2,
                    },
                    value: 0,
                    height: None,
                    script: Vec::new().into(),
                },
                slp_amount: 8999,
            },
            SlpUnspent {
                bch_unspent: UnspentInfo {
                    outpoint: OutPoint {
                        hash: tx_hash,
                        index: 3,
                    },
                    value: 0,
                    height: None,
                    script: Vec::new().into(),
                },
                slp_amount: 8999,
            },
        ];

        let token_id = H256::from("bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb7");
        let err = block_on(validate_slp_utxos(BCHD_TESTNET_URLS, &slp_utxos, &token_id)).unwrap_err();
        match err.into_inner().kind {
            ValidateSlpUtxosErrKind::InvalidSlpTxData(_) => (),
            err => panic!("Unexpected error {:?}", err),
        }
    }

    #[test]
    #[ignore]
    fn test_validate_slp_utxos_invalid_amount() {
        let tx_hash = H256::from_reversed_str("0ba1b91abbfceaa0777424165edb2928dace87d59669c913989950da31968032");
        let invalid_utxo = SlpUnspent {
            bch_unspent: UnspentInfo {
                outpoint: OutPoint {
                    hash: tx_hash,
                    index: 1,
                },
                value: 0,
                height: None,
                script: Vec::new().into(),
            },
            slp_amount: 999,
        };

        let slp_utxos = [
            invalid_utxo.clone(),
            SlpUnspent {
                bch_unspent: UnspentInfo {
                    outpoint: OutPoint {
                        hash: tx_hash,
                        index: 2,
                    },
                    value: 0,
                    height: None,
                    script: Vec::new().into(),
                },
                slp_amount: 8999,
            },
        ];

        let token_id = H256::from("bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb7");
        let err = block_on(validate_slp_utxos(BCHD_TESTNET_URLS, &slp_utxos, &token_id)).unwrap_err();
        match err.into_inner().kind {
            ValidateSlpUtxosErrKind::UnexpectedValidityResultType {
                for_unspent,
                validity_result,
            } => {
                let expected_validity = Some(ValidityResultType::V1TokenAmount(1000));
                assert_eq!(invalid_utxo, for_unspent);
                assert_eq!(expected_validity, validity_result);
            },
            err => panic!("Unexpected error {:?}", err),
        }
    }

    #[test]
    #[ignore]
    fn test_validate_slp_utxos_unexpected_token_id() {
        let tx_hash = H256::from_reversed_str("0ba1b91abbfceaa0777424165edb2928dace87d59669c913989950da31968032");

        let slp_utxos = [
            SlpUnspent {
                bch_unspent: UnspentInfo {
                    outpoint: OutPoint {
                        hash: tx_hash,
                        index: 1,
                    },
                    value: 0,
                    height: None,
                    script: Vec::new().into(),
                },
                slp_amount: 1000,
            },
            SlpUnspent {
                bch_unspent: UnspentInfo {
                    outpoint: OutPoint {
                        hash: tx_hash,
                        index: 2,
                    },
                    value: 0,
                    height: None,
                    script: Vec::new().into(),
                },
                slp_amount: 8999,
            },
        ];

        let valid_token_id = H256::from("bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb7");
        let invalid_token_id = H256::from("bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb8");
        let err = block_on(validate_slp_utxos(BCHD_TESTNET_URLS, &slp_utxos, &invalid_token_id)).unwrap_err();
        match err.into_inner().kind {
            ValidateSlpUtxosErrKind::UnexpectedTokenId { expected, actual } => {
                assert_eq!(invalid_token_id, expected);
                assert_eq!(valid_token_id, actual);
            },
            err => panic!("Unexpected error {:?}", err),
        }
    }

    #[test]
    #[ignore]
    fn test_check_slp_transaction_valid() {
        // https://testnet.simpleledger.info/tx/c5f46ccc5431687154335d5b6526f1b9cfa961c44b97956b7bec77f884f56c73
        let tx = hex::decode("010000000232809631da50999813c96996d587ceda2829db5e16247477a0eafcbb1ab9a10b020000006a473044022057c88d815fa563eda8ef7d0dd5c522f4501ffa6110df455b151b31609f149c22022048fecfc9b16e983fbfd05b0d2b7c011c3dbec542577fa00cd9bd192b81961f8e4121036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202cffffffff32809631da50999813c96996d587ceda2829db5e16247477a0eafcbb1ab9a10b030000006a4730440220539e1204d2805c0474111a1f233ff82c0ab06e6e2bfc0cbe4975eacae64a0b1f02200ec83d32c2180f5567d0f760e85f1efc99d9341cfebd86c9a334310f6d4381494121036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202cffffffff040000000000000000406a04534c500001010453454e4420bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb7080000000000000001080000000000002326e8030000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ace8030000000000001976a9148cfffc2409d063437d6aa8b75a009b9ba51b71fc88ac9f694801000000001976a9148cfffc2409d063437d6aa8b75a009b9ba51b71fc88ac8983d460").unwrap();
        block_on(check_slp_transaction(BCHD_TESTNET_URLS, tx)).unwrap();
    }

    #[test]
    #[ignore]
    fn test_check_slp_transaction_invalid() {
        // https://www.blockchain.com/bch-testnet/tx/d76723c092b64bc598d5d2ceafd6f0db37dce4032db569d6f26afb35491789a7
        let tx = hex::decode("010000000190e35c09c83b5818b441c18a2d5ec54734851e5581fb21bde7936e77c6c3dca8030000006b483045022100e6b1415cbd81f2d04360597fba65965bc77ab5a972f5b8f8d5c0f1b1912923c402206a63f305f03e9c49ffba6c71c7a76ef60631f67dce7631f673a0e8485b86898d4121036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202cffffffff020000000000000000376a04534c500001010453454e4420bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb70800000000000003e82500ae00000000001976a9148cfffc2409d063437d6aa8b75a009b9ba51b71fc88ac62715161").unwrap();
        let err = block_on(check_slp_transaction(BCHD_TESTNET_URLS, tx)).unwrap_err();
        match err.into_inner().kind {
            CheckSlpTransactionErrKind::InvalidTransaction { reason, .. } => {
                println!("{reason}");
            },
            err => panic!("Unexpected error {:?}", err),
        }
    }
}

// Todo: once BCHD issues are solved or we use another node implementation for SLP validation, we can re-enable this test.
// #[cfg(target_arch = "wasm32")]
// mod wasm_tests {
//     use super::*;
//     use mm2_test_helpers::for_tests::BCHD_TESTNET_URLS;
//     use wasm_bindgen_test::*;
//
//     wasm_bindgen_test_configure!(run_in_browser);
//
//     #[wasm_bindgen_test]
//     async fn test_check_slp_transaction_valid() {
//         // https://testnet.simpleledger.info/tx/c5f46ccc5431687154335d5b6526f1b9cfa961c44b97956b7bec77f884f56c73
//         let tx = hex::decode("010000000232809631da50999813c96996d587ceda2829db5e16247477a0eafcbb1ab9a10b020000006a473044022057c88d815fa563eda8ef7d0dd5c522f4501ffa6110df455b151b31609f149c22022048fecfc9b16e983fbfd05b0d2b7c011c3dbec542577fa00cd9bd192b81961f8e4121036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202cffffffff32809631da50999813c96996d587ceda2829db5e16247477a0eafcbb1ab9a10b030000006a4730440220539e1204d2805c0474111a1f233ff82c0ab06e6e2bfc0cbe4975eacae64a0b1f02200ec83d32c2180f5567d0f760e85f1efc99d9341cfebd86c9a334310f6d4381494121036879df230663db4cd083c8eeb0f293f46abc460ad3c299b0089b72e6d472202cffffffff040000000000000000406a04534c500001010453454e4420bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb7080000000000000001080000000000002326e8030000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ace8030000000000001976a9148cfffc2409d063437d6aa8b75a009b9ba51b71fc88ac9f694801000000001976a9148cfffc2409d063437d6aa8b75a009b9ba51b71fc88ac8983d460").unwrap();
//         check_slp_transaction(BCHD_TESTNET_URLS, tx).await.unwrap();
//     }
// }
