use std::collections::HashMap;

use common::{now_sec, HttpStatusCode};
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::{MapMmError, MmResult, MmResultExt};
use mm2_number::BigDecimal;
use rpc::v1::types::ToTxHash;

use crate::{
    hd_wallet::{AddrToString, HDWalletOps},
    lp_coinfind_or_err,
    utxo::{
        output_script,
        utxo_builder::{merge_utxos, MergeConditions, UtxoMergeError},
        utxo_common::big_decimal_from_sat_unsigned,
        UtxoFeeDetails, UtxoStandardOps,
    },
    CoinFindError, DerivationMethod, MmCoinEnum, Transaction, TransactionData, TransactionDetails,
};

#[derive(Deserialize)]
pub struct ConsolidateUtxoRequest {
    coin: String,
    #[serde(default)]
    merge_conditions: MergeConditions,
    #[serde(default)]
    broadcast: bool,
}

#[derive(Serialize)]
pub struct ConsolidateUtxoResponse {
    tx: TransactionDetails,
    consolidated_utxos: Vec<SpentUtxo>,
}

#[derive(Serialize, Display, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum ConsolidateUtxoError {
    NoSuchCoin,
    CoinNotSupported,
    InvalidAddress(String),
    BadMergeConditions(String),
    InternalError(String),
}

impl HttpStatusCode for ConsolidateUtxoError {
    fn status_code(&self) -> StatusCode {
        match self {
            ConsolidateUtxoError::NoSuchCoin => StatusCode::NOT_FOUND,
            ConsolidateUtxoError::CoinNotSupported => StatusCode::BAD_REQUEST,
            ConsolidateUtxoError::InvalidAddress(_) => StatusCode::BAD_REQUEST,
            ConsolidateUtxoError::BadMergeConditions(_) => StatusCode::BAD_REQUEST,
            ConsolidateUtxoError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CoinFindError> for ConsolidateUtxoError {
    fn from(err: CoinFindError) -> Self {
        match err {
            CoinFindError::NoSuchCoin { .. } => ConsolidateUtxoError::NoSuchCoin,
        }
    }
}

impl From<UtxoMergeError> for ConsolidateUtxoError {
    fn from(err: UtxoMergeError) -> Self {
        match err {
            UtxoMergeError::BadMergeConditions(e) => ConsolidateUtxoError::BadMergeConditions(e),
            UtxoMergeError::InternalError(e) => ConsolidateUtxoError::InternalError(e),
        }
    }
}

#[derive(Serialize)]
struct SpentUtxo {
    txid: String,
    vout: u32,
    value: BigDecimal,
}

pub async fn consolidate_utxos_rpc(
    ctx: MmArc,
    request: ConsolidateUtxoRequest,
) -> MmResult<ConsolidateUtxoResponse, ConsolidateUtxoError> {
    let coin = lp_coinfind_or_err(&ctx, &request.coin).await.map_mm_err()?;
    match coin {
        MmCoinEnum::UtxoCoin(coin) => {
            let from_address = match &coin.as_ref().derivation_method {
                DerivationMethod::SingleAddress(my_address) => my_address.clone(),
                DerivationMethod::HDWallet(wallet) => {
                    let hd_address = wallet.get_enabled_address().await.ok_or_else(|| {
                        ConsolidateUtxoError::InvalidAddress("No enabled address found in HD wallet".to_string())
                    })?;
                    hd_address.address
                },
            };
            let to_script_pubkey = output_script(&from_address).map_err(|e| {
                ConsolidateUtxoError::InvalidAddress(format!("Failed to convert `to_address` to a script_pubkey: {e}"))
            })?;

            let (transaction, spent_utxos) = merge_utxos(
                &coin,
                &from_address,
                &to_script_pubkey,
                &request.merge_conditions,
                request.broadcast,
            )
            .await
            .map_mm_err()?;

            let received_by_me = transaction.outputs.iter().map(|o| o.value).sum();
            let received_by_me = big_decimal_from_sat_unsigned(received_by_me, coin.as_ref().decimals);

            let spent_by_me = spent_utxos.iter().map(|i| i.value).sum();
            let spent_by_me = big_decimal_from_sat_unsigned(spent_by_me, coin.as_ref().decimals);

            let mut tx = TransactionDetails {
                from: vec![from_address.addr_to_string()],
                to: vec![from_address.addr_to_string()],
                received_by_me: received_by_me.clone(),
                spent_by_me: spent_by_me.clone(),
                total_amount: spent_by_me.clone(),
                my_balance_change: &received_by_me - &spent_by_me,
                tx: TransactionData::new_signed(
                    transaction.tx_hex().into(),
                    transaction.hash().reversed().to_vec().to_tx_hash(),
                ),
                coin: coin.as_ref().conf.ticker.clone(),
                internal_id: transaction.hash().reversed().to_vec().into(),
                fee_details: Some(crate::TxFeeDetails::Utxo(UtxoFeeDetails {
                    coin: Some(coin.as_ref().conf.ticker.clone()),
                    amount: spent_by_me - received_by_me,
                })),
                block_height: 0,
                timestamp: now_sec(),
                kmd_rewards: None,
                transaction_type: Default::default(),
                memo: None,
            };

            if tx.should_update_kmd_rewards() {
                coin.update_kmd_rewards(&mut tx, &mut HashMap::new())
                    .await
                    .mm_err(|e| ConsolidateUtxoError::InternalError(format!("Failed to update KMD rewards: {e}")))?;
            }

            Ok(ConsolidateUtxoResponse {
                tx,
                consolidated_utxos: spent_utxos
                    .into_iter()
                    .map(|spent| SpentUtxo {
                        txid: spent.outpoint.hash.reversed().to_string(),
                        vout: spent.outpoint.index,
                        value: big_decimal_from_sat_unsigned(spent.value, coin.as_ref().decimals),
                    })
                    .collect(),
            })
        },
        _ => Err(ConsolidateUtxoError::CoinNotSupported.into()),
    }
}
