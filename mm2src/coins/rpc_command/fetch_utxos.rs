use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use keys::{Address, Public};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::{MapMmError, MmResult, MmResultExt};
use mm2_number::BigDecimal;
use std::collections::HashMap;

use crate::{
    hd_wallet::{AddrToString, HDAddress, HDWalletOps},
    lp_coinfind_or_err,
    utxo::{
        utxo_common::big_decimal_from_sat_unsigned, utxo_standard::UtxoStandardCoin, GetUtxoListOps, GetUtxoMapOps,
    },
    CoinFindError, DerivationMethod, MmCoinEnum,
};

#[derive(Deserialize)]
pub struct FetchUtxosRequest {
    pub coin: String,
}

#[derive(Serialize)]
pub struct AddressUtxos {
    pub address: String,
    pub count: usize,
    pub utxos: Vec<UnspentOutputs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derivation_path: Option<String>,
}

#[derive(Serialize)]
pub struct FetchUtxosResponse {
    pub total_count: usize,
    pub addresses: Vec<AddressUtxos>,
}

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum FetchUtxosError {
    NoSuchCoin,
    CoinNotSupported,
    InvalidAddress(String),
    Internal(String),
}

impl HttpStatusCode for FetchUtxosError {
    fn status_code(&self) -> StatusCode {
        match self {
            FetchUtxosError::NoSuchCoin => StatusCode::NOT_FOUND,
            FetchUtxosError::CoinNotSupported => StatusCode::BAD_REQUEST,
            FetchUtxosError::InvalidAddress(_) => StatusCode::BAD_REQUEST,
            FetchUtxosError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CoinFindError> for FetchUtxosError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { .. } => FetchUtxosError::NoSuchCoin,
        }
    }
}

#[derive(Serialize)]
pub struct UnspentOutputs {
    txid: String,
    vout: u32,
    value: BigDecimal,
}

pub async fn fetch_utxos_rpc(ctx: MmArc, req: FetchUtxosRequest) -> MmResult<FetchUtxosResponse, FetchUtxosError> {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;

    match coin {
        MmCoinEnum::UtxoCoin(coin) => match &coin.as_ref().derivation_method {
            DerivationMethod::SingleAddress(my_address) => {
                let addresses_utxos = get_utxos(&coin, UtxosFrom::Single(my_address.clone())).await?;
                let total_count = addresses_utxos.iter().map(|addr| addr.count).sum();
                Ok(FetchUtxosResponse {
                    total_count,
                    addresses: addresses_utxos,
                })
            },
            DerivationMethod::HDWallet(wallet) => {
                let accounts = wallet.get_accounts().await;
                let mut addresses = Vec::new();
                for (_, account) in accounts {
                    let addresses_in_account = account.derived_addresses.lock().await.clone();
                    addresses.extend(addresses_in_account.into_values());
                }
                let addresses_utxos = get_utxos(&coin, UtxosFrom::HDWallet(addresses)).await?;
                let total_count = addresses_utxos.iter().map(|addr| addr.count).sum();
                Ok(FetchUtxosResponse {
                    total_count,
                    addresses: addresses_utxos,
                })
            },
        },
        _ => Err(FetchUtxosError::CoinNotSupported.into()),
    }
}

enum UtxosFrom {
    Single(Address),
    HDWallet(Vec<HDAddress<Address, Public>>),
}

async fn get_utxos(coin: &UtxoStandardCoin, from: UtxosFrom) -> MmResult<Vec<AddressUtxos>, FetchUtxosError> {
    let unspents = match from {
        UtxosFrom::Single(address) => {
            let (unspents, _) = coin.get_unspent_ordered_list(&address).await.mm_err(|e| {
                FetchUtxosError::Internal(format!("Couldn't fetch unspent UTXOs (address={address}): {e}"))
            })?;
            // In a single address mode, the address doesn't have a derivation path.
            vec![(address, unspents, None)]
        },
        UtxosFrom::HDWallet(addresses) => {
            // From an HDAddress, we only care about the address itself and the derivation path.
            let mut addresses: HashMap<_, _> = addresses
                .into_iter()
                .map(|addr| (addr.address, addr.derivation_path))
                .collect();

            let (unspent_map, _) = coin
                .get_unspent_ordered_map(addresses.keys().cloned().collect())
                .await
                .mm_err(|e| {
                    FetchUtxosError::Internal(format!("Couldn't fetch unspent UTXOs (addresses={addresses:?}): {e}"))
                })?;

            // Essentially, convert the (address -> unspents) map to (address -> unspents + derivation_path) map/vector.
            unspent_map
                .into_iter()
                .map(|(addr, unspent)| match addresses.remove(&addr) {
                    Some(derivation_path) => Ok((addr, unspent, Some(derivation_path))),
                    None => Err(FetchUtxosError::Internal(format!(
                        "Unknown address={addr} returned by electrum"
                    ))),
                })
                .collect::<Result<_, _>>()?
        },
    };

    Ok(unspents
        .into_iter()
        .filter(|(_, unspents, _)| !unspents.is_empty())
        .map(|(addr, unspents, derivation_path)| AddressUtxos {
            address: addr.addr_to_string(),
            count: unspents.len(),
            utxos: unspents
                .into_iter()
                .map(|unspent| UnspentOutputs {
                    txid: unspent.outpoint.hash.reversed().to_string(),
                    vout: unspent.outpoint.index,
                    value: big_decimal_from_sat_unsigned(unspent.value, coin.as_ref().decimals),
                })
                .collect(),
            derivation_path: derivation_path.map(|d| d.to_string()),
        })
        .collect())
}
