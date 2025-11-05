use async_trait::async_trait;
use common::{now_sec_i64, PagingOptionsEnum};
use db_common::sqlite::rusqlite::types::FromSqlError;
use derive_more::Display;
use lightning::ln::{PaymentHash, PaymentPreimage};
use secp256k1v24::PublicKey;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DBChannelDetails {
    pub uuid: Uuid,
    pub channel_id: String,
    pub counterparty_node_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub funding_tx: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub funding_value: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closing_tx: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closure_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claiming_tx: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_balance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub funding_generated_in_block: Option<i64>,
    pub is_outbound: bool,
    pub is_public: bool,
    pub is_closed: bool,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<i64>,
}

impl DBChannelDetails {
    #[inline]
    pub fn new(
        uuid: Uuid,
        channel_id: [u8; 32],
        counterparty_node_id: PublicKey,
        is_outbound: bool,
        is_public: bool,
    ) -> Self {
        DBChannelDetails {
            uuid,
            channel_id: hex::encode(channel_id),
            counterparty_node_id: counterparty_node_id.to_string(),
            funding_tx: None,
            funding_value: None,
            funding_generated_in_block: None,
            closing_tx: None,
            closure_reason: None,
            claiming_tx: None,
            claimed_balance: None,
            is_outbound,
            is_public,
            is_closed: false,
            created_at: now_sec_i64(),
            closed_at: None,
        }
    }
}

#[derive(Clone, Deserialize)]
pub enum ChannelType {
    Outbound,
    Inbound,
}

#[derive(Clone, Deserialize)]
pub enum ChannelVisibility {
    Public,
    Private,
}

#[derive(Clone, Deserialize)]
pub struct ClosedChannelsFilter {
    pub channel_id: Option<String>,
    pub counterparty_node_id: Option<String>,
    pub funding_tx: Option<String>,
    pub from_funding_value: Option<i64>,
    pub to_funding_value: Option<i64>,
    pub closing_tx: Option<String>,
    pub closure_reason: Option<String>,
    pub claiming_tx: Option<String>,
    pub from_claimed_balance: Option<f64>,
    pub to_claimed_balance: Option<f64>,
    pub channel_type: Option<ChannelType>,
    pub channel_visibility: Option<ChannelVisibility>,
}

pub struct GetClosedChannelsResult {
    pub channels: Vec<DBChannelDetails>,
    pub skipped: usize,
    pub total: usize,
}

#[derive(Clone, Debug, Deserialize, Display, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HTLCStatus {
    Pending,
    Claimable,
    Succeeded,
    Failed,
}

impl FromStr for HTLCStatus {
    type Err = FromSqlError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(HTLCStatus::Pending),
            "Claimable" => Ok(HTLCStatus::Claimable),
            "Succeeded" => Ok(HTLCStatus::Succeeded),
            "Failed" => Ok(HTLCStatus::Failed),
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaymentType {
    OutboundPayment { destination: PublicKey },
    InboundPayment,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaymentInfo {
    pub payment_hash: PaymentHash,
    pub payment_type: PaymentType,
    pub description: String,
    pub preimage: Option<PaymentPreimage>,
    pub amt_msat: Option<i64>,
    pub fee_paid_msat: Option<i64>,
    pub status: HTLCStatus,
    pub created_at: i64,
    pub last_updated: i64,
}

impl PaymentInfo {
    #[inline]
    pub fn new(
        payment_hash: PaymentHash,
        payment_type: PaymentType,
        description: String,
        amt_msat: Option<i64>,
    ) -> PaymentInfo {
        PaymentInfo {
            payment_hash,
            payment_type,
            description,
            preimage: None,
            amt_msat,
            fee_paid_msat: None,
            status: HTLCStatus::Pending,
            created_at: now_sec_i64(),
            last_updated: now_sec_i64(),
        }
    }

    #[inline]
    pub fn with_preimage(mut self, preimage: PaymentPreimage) -> Self {
        self.preimage = Some(preimage);
        self
    }

    #[inline]
    pub fn with_status(mut self, status: HTLCStatus) -> Self {
        self.status = status;
        self
    }

    pub(crate) fn is_outbound(&self) -> bool {
        matches!(self.payment_type, PaymentType::OutboundPayment { .. })
    }
}

#[derive(Clone)]
pub struct DBPaymentsFilter {
    pub is_outbound: Option<bool>,
    pub destination: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub from_amount_msat: Option<i64>,
    pub to_amount_msat: Option<i64>,
    pub from_fee_paid_msat: Option<i64>,
    pub to_fee_paid_msat: Option<i64>,
    pub from_timestamp: Option<i64>,
    pub to_timestamp: Option<i64>,
}

pub struct GetPaymentsResult {
    pub payments: Vec<PaymentInfo>,
    pub skipped: usize,
    pub total: usize,
}

#[async_trait]
pub trait LightningDB {
    type Error;

    /// Initializes tables in DB.
    async fn init_db(&self) -> Result<(), Self::Error>;

    /// Checks if tables have been initialized or not in DB.
    async fn is_db_initialized(&self) -> Result<bool, Self::Error>;

    /// Inserts a new channel record in the DB. The record's data is completed using add_funding_tx_to_db,
    /// add_closing_tx_to_db, add_claiming_tx_to_db when this information is available.
    async fn add_channel_to_db(&self, details: &DBChannelDetails) -> Result<(), Self::Error>;

    /// Updates a channel's DB record with the channel's funding transaction information.
    async fn add_funding_tx_to_db(
        &self,
        uuid: Uuid,
        funding_tx: String,
        funding_value: i64,
        funding_generated_in_block: i64,
    ) -> Result<(), Self::Error>;

    /// Updates funding_tx_block_height value for a channel in the DB. Should be used to update the block height of
    /// the funding tx when the transaction is confirmed on-chain.
    async fn update_funding_tx_block_height(&self, funding_tx: String, block_height: i64) -> Result<(), Self::Error>;

    /// Updates the is_closed value for a channel in the DB to 1.
    async fn update_channel_to_closed(
        &self,
        uuid: Uuid,
        closure_reason: String,
        close_at: i64,
    ) -> Result<(), Self::Error>;

    /// Gets the list of closed channels records in the DB that have funding tx hashes saved with no closing
    /// tx hashes saved yet.
    /// Can be used to check if the closing tx hash needs to be fetched from the chain and saved to DB
    /// when initializing the persister.
    async fn get_closed_channels_with_no_closing_tx(&self) -> Result<Vec<DBChannelDetails>, Self::Error>;

    /// Updates a channel's DB record with the channel's closing transaction hash.
    async fn add_closing_tx_to_db(&self, uuid: Uuid, closing_tx: String) -> Result<(), Self::Error>;

    /// Updates a channel's DB record with information about the transaction responsible for claiming the channel's
    /// closing balance back to the user's address.
    async fn add_claiming_tx_to_db(
        &self,
        closing_tx: String,
        claiming_tx: String,
        claimed_balance: f64,
    ) -> Result<(), Self::Error>;

    /// Gets a channel record from DB by the channel's uuid.
    async fn get_channel_from_db(&self, uuid: Uuid) -> Result<Option<DBChannelDetails>, Self::Error>;

    /// Gets the list of closed channels that match the provided filter criteria. The number of requested records is
    /// specified by the limit parameter, the starting record to list from is specified by the paging parameter. The
    /// total number of matched records along with the number of skipped records are also returned in the result.
    async fn get_closed_channels_by_filter(
        &self,
        filter: Option<ClosedChannelsFilter>,
        paging: PagingOptionsEnum<Uuid>,
        limit: usize,
    ) -> Result<GetClosedChannelsResult, Self::Error>;

    /// Inserts a new payment record in the DB.
    async fn add_payment_to_db(&self, info: &PaymentInfo) -> Result<(), Self::Error>;

    /// Inserts or updates a payment record in the DB.
    async fn add_or_update_payment_in_db(&self, info: &PaymentInfo) -> Result<(), Self::Error>;

    /// Updates a payment's preimage in DB by the payment's hash.
    async fn update_payment_preimage_in_db(
        &self,
        hash: PaymentHash,
        preimage: PaymentPreimage,
    ) -> Result<(), Self::Error>;

    /// Updates a payment's status in DB by the payment's hash.
    async fn update_payment_status_in_db(&self, hash: PaymentHash, status: &HTLCStatus) -> Result<(), Self::Error>;

    /// Updates a payment's status to claimable in DB by the payment's hash. Also, adds the payment preimage to the db.
    async fn update_payment_to_claimable_in_db(
        &self,
        hash: PaymentHash,
        preimage: PaymentPreimage,
    ) -> Result<(), Self::Error>;

    /// Updates a sent payment status to succeeded in DB by the payment's hash. Also, adds the payment preimage and the amount of fees paid to the db.
    async fn update_payment_to_sent_in_db(
        &self,
        hash: PaymentHash,
        preimage: PaymentPreimage,
        fee_paid_msat: Option<u64>,
    ) -> Result<(), Self::Error>;

    /// Gets a payment's record from DB by the payment's hash.
    async fn get_payment_from_db(&self, hash: PaymentHash) -> Result<Option<PaymentInfo>, Self::Error>;

    /// Gets the list of payments that match the provided filter criteria. The number of requested records is specified
    /// by the limit parameter, the starting record to list from is specified by the paging parameter. The total number
    /// of matched records along with the number of skipped records are also returned in the result.
    async fn get_payments_by_filter(
        &self,
        filter: Option<DBPaymentsFilter>,
        paging: PagingOptionsEnum<PaymentHash>,
        limit: usize,
    ) -> Result<GetPaymentsResult, Self::Error>;
}
