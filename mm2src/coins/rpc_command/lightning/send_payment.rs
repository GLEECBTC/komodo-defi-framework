use crate::lightning::ln_p2p::connect_to_ln_node;
use crate::lightning::ln_serialization::PublicKeyForRPC;
use crate::lightning::ln_utils::PaymentError;
use crate::{lp_coinfind_or_err, CoinFindError, H256Json, MmCoinEnum};
use common::log::LogOnError;
use common::HttpStatusCode;
use db_common::sqlite::rusqlite::Error as SqlError;
use derive_more::Display;
use http::StatusCode;
use lightning_invoice::Invoice;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;

type SendPaymentResult<T> = Result<T, MmError<SendPaymentError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum SendPaymentError {
    #[display(fmt = "Lightning network is not supported for {_0}")]
    UnsupportedCoin(String),
    #[display(fmt = "No such coin {_0}")]
    NoSuchCoin(String),
    #[display(fmt = "Couldn't parse destination pubkey: {_0}")]
    NoRouteFound(String),
    #[display(fmt = "Payment error: {_0}")]
    PaymentError(String),
    #[display(fmt = "DB error {_0}")]
    DbError(String),
}

impl HttpStatusCode for SendPaymentError {
    fn status_code(&self) -> StatusCode {
        match self {
            SendPaymentError::UnsupportedCoin(_) => StatusCode::BAD_REQUEST,
            SendPaymentError::NoSuchCoin(_) => StatusCode::NOT_FOUND,
            SendPaymentError::PaymentError(_) | SendPaymentError::NoRouteFound(_) | SendPaymentError::DbError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
        }
    }
}

impl From<CoinFindError> for SendPaymentError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => SendPaymentError::NoSuchCoin(coin),
        }
    }
}

impl From<SqlError> for SendPaymentError {
    fn from(err: SqlError) -> SendPaymentError {
        SendPaymentError::DbError(err.to_string())
    }
}

impl From<PaymentError> for SendPaymentError {
    fn from(err: PaymentError) -> SendPaymentError {
        SendPaymentError::PaymentError(err.to_string())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Payment {
    #[serde(rename = "invoice")]
    Invoice { invoice: Invoice },
    #[serde(rename = "keysend")]
    Keysend {
        // The recieving node pubkey (node ID)
        destination: PublicKeyForRPC,
        // Amount to send in millisatoshis
        amount_in_msat: u64,
        // The number of blocks the payment will be locked for if not claimed by the destination,
        // It's can be assumed that 6 blocks = 1 hour. We can claim the payment amount back after this cltv expires.
        // Minmum value allowed is MIN_FINAL_CLTV_EXPIRY which is currently 24 for rust-lightning.
        expiry: u32,
    },
}

#[derive(Deserialize)]
pub struct SendPaymentReq {
    pub coin: String,
    pub payment: Payment,
}

#[derive(Serialize)]
pub struct SendPaymentResponse {
    payment_hash: H256Json,
}

pub async fn send_payment(ctx: MmArc, req: SendPaymentReq) -> SendPaymentResult<SendPaymentResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(SendPaymentError::UnsupportedCoin(e.ticker().to_string())),
    };
    let open_channels_nodes = ln_coin.open_channels_nodes.lock().clone();
    for (node_pubkey, node_addr) in open_channels_nodes {
        connect_to_ln_node(node_pubkey, node_addr, ln_coin.peer_manager.clone())
            .await
            .error_log_with_msg(&format!(
                "Channel with node: {node_pubkey} can't be used to route this payment due to connection error."
            ));
    }
    let payment_info = match req.payment {
        Payment::Invoice { invoice } => ln_coin.pay_invoice(invoice, None).await.map_mm_err()?,
        Payment::Keysend {
            destination,
            amount_in_msat,
            expiry,
        } => ln_coin
            .keysend(destination.into(), amount_in_msat, expiry)
            .await
            .map_mm_err()?,
    };

    Ok(SendPaymentResponse {
        payment_hash: payment_info.payment_hash.0.into(),
    })
}
