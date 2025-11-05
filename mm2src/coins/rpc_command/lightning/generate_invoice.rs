use crate::lightning::ln_db::{LightningDB, PaymentInfo, PaymentType};
use crate::lightning::ln_p2p::connect_to_ln_node;
use crate::lightning::DEFAULT_INVOICE_EXPIRY;
use crate::{lp_coinfind_or_err, CoinFindError, H256Json, MmCoinEnum};
use bitcoin_hashes::Hash;
use common::log::LogOnError;
use common::{async_blocking, HttpStatusCode};
use db_common::sqlite::rusqlite::Error as SqlError;
use derive_more::Display;
use http::StatusCode;
use lightning::ln::PaymentHash;
use lightning_invoice::utils::create_invoice_from_channelmanager;
use lightning_invoice::{Invoice, SignOrCreationError};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;

type GenerateInvoiceResult<T> = Result<T, MmError<GenerateInvoiceError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum GenerateInvoiceError {
    #[display(fmt = "Lightning network is not supported for {_0}")]
    UnsupportedCoin(String),
    #[display(fmt = "No such coin {_0}")]
    NoSuchCoin(String),
    #[display(fmt = "Invoice signing or creation error: {_0}")]
    SignOrCreationError(String),
    #[display(fmt = "DB error {_0}")]
    DbError(String),
}

impl HttpStatusCode for GenerateInvoiceError {
    fn status_code(&self) -> StatusCode {
        match self {
            GenerateInvoiceError::UnsupportedCoin(_) => StatusCode::BAD_REQUEST,
            GenerateInvoiceError::NoSuchCoin(_) => StatusCode::NOT_FOUND,
            GenerateInvoiceError::SignOrCreationError(_) | GenerateInvoiceError::DbError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
        }
    }
}

impl From<CoinFindError> for GenerateInvoiceError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => GenerateInvoiceError::NoSuchCoin(coin),
        }
    }
}

impl From<SignOrCreationError> for GenerateInvoiceError {
    fn from(e: SignOrCreationError) -> Self {
        GenerateInvoiceError::SignOrCreationError(e.to_string())
    }
}

impl From<SqlError> for GenerateInvoiceError {
    fn from(err: SqlError) -> GenerateInvoiceError {
        GenerateInvoiceError::DbError(err.to_string())
    }
}

#[derive(Deserialize)]
pub struct GenerateInvoiceRequest {
    pub coin: String,
    pub amount_in_msat: Option<u64>,
    pub description: String,
    pub expiry: Option<u32>,
}

#[derive(Serialize)]
pub struct GenerateInvoiceResponse {
    payment_hash: H256Json,
    invoice: Invoice,
}

/// Generates an invoice (request for payment) that can be paid on the lightning network by another node using send_payment.
pub async fn generate_invoice(
    ctx: MmArc,
    req: GenerateInvoiceRequest,
) -> GenerateInvoiceResult<GenerateInvoiceResponse> {
    let ln_coin = match lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()? {
        MmCoinEnum::LightningCoin(c) => c,
        e => return MmError::err(GenerateInvoiceError::UnsupportedCoin(e.ticker().to_string())),
    };
    let open_channels_nodes = ln_coin.open_channels_nodes.lock().clone();
    for (node_pubkey, node_addr) in open_channels_nodes {
        connect_to_ln_node(node_pubkey, node_addr, ln_coin.peer_manager.clone())
            .await
            .error_log_with_msg(&format!(
                "Channel with node: {node_pubkey} can't be used for invoice routing hints due to connection error."
            ));
    }

    let network = ln_coin.platform.network.clone().into();
    let channel_manager = ln_coin.channel_manager.clone();
    let keys_manager = ln_coin.keys_manager.clone();
    let logger = ln_coin.logger.clone();
    let amount_in_msat = req.amount_in_msat;
    let description = req.description.clone();
    let expiry = req.expiry.unwrap_or(DEFAULT_INVOICE_EXPIRY);
    let invoice = async_blocking(move || {
        create_invoice_from_channelmanager(
            &channel_manager,
            keys_manager,
            logger,
            network,
            amount_in_msat,
            description,
            expiry,
        )
    })
    .await?;

    let payment_hash = invoice.payment_hash().into_inner();
    let payment_info = PaymentInfo::new(
        PaymentHash(payment_hash),
        PaymentType::InboundPayment,
        req.description,
        req.amount_in_msat.map(|a| a as i64),
    );
    // Note: Although the preimage can be recreated from the keymanager and the invoice secret, the payment info is added to db at invoice generation stage
    // to save the description. Although it's not ideal to keep track of invoices before they are paid since they may never be paid, but this is the only way
    // to have the invoice description saved in the db.
    ln_coin.db.add_payment_to_db(&payment_info).await?;

    Ok(GenerateInvoiceResponse {
        payment_hash: payment_hash.into(),
        invoice,
    })
}
