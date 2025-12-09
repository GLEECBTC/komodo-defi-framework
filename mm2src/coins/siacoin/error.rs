use crate::siacoin::client_error::{
    BroadcastTransactionError, ClientError, CurrentHeightError, FindWhereUtxoSpentError, GetMedianTimestampError,
    GetUnconfirmedTransactionError, UtxoFromTxidError,
};
use crate::siacoin::{
    Address, Currency, Event, EventDataWrapper, Hash256, Hash256Error, KeypairError, PreimageError, PublicKeyError,
    SiaTransaction, SiacoinOutput, TransactionId, V2TransactionBuilderError,
};
use crate::{DexFee, TransactionEnum};
use common::executor::AbortedError;
use crypto::privkey::PrivKeyError;
use mm2_number::BigDecimal;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum SiacoinToHastingsError {
    #[error("Sia Failed to convert BigDecimal:{0} to BigInt")]
    BigDecimalToBigInt(BigDecimal),
    #[error("Sia Failed to convert BigDecimal:{0} to u128")]
    BigIntToU128(BigDecimal),
}

#[derive(Debug, Error)]
pub enum SendTakerFeeError {
    #[error("SiaCoin::new_send_taker_fee: failed to parse uuid from bytes {0}")]
    ParseUuid(#[from] uuid::Error),
    #[error("SiaCoin::new_send_taker_fee: Unexpected Uuid version {0}")]
    UuidVersion(usize),
    #[error("SiaCoin::new_send_taker_fee: failed to convert trade_fee_amount to Currency {0}")]
    SiacoinToHastings(#[from] SiacoinToHastingsError),
    #[error("SiaCoin::new_send_taker_fee: unexpected DexFee variant: {0:?}")]
    DexFeeVariant(DexFee),
    #[error("SiaCoin::new_send_taker_fee: failed to fetch my_keypair {0}")]
    MyKeypair(#[from] SiaCoinMyKeypairError),
    #[error("SiaCoin::new_send_taker_fee: failed to fund transaction {0}")]
    FundTx(#[from] V2TransactionBuilderError),
    #[error("SiaCoin::new_send_taker_fee: failed to broadcast taker_fee transaction {0}")]
    BroadcastTx(#[from] BroadcastTransactionError),
}

#[derive(Debug, Error)]
pub enum SendMakerPaymentError {
    #[error("SiaCoin::new_send_maker_payment: invalid taker pubkey, expected 33 bytes found: {0:?}")]
    InvalidTakerPublicKeyLength(Vec<u8>),
    #[error("SiaCoin::new_send_maker_payment: invalid taker pubkey {0}")]
    InvalidTakerPublicKey(#[from] PublicKeyError),
    #[error("SiaCoin::new_send_maker_payment: failed to fetch my_keypair {0}")]
    MyKeypair(#[from] SiaCoinMyKeypairError),
    #[error("SiaCoin::new_send_maker_payment: failed to convert trade amount to Currency {0}")]
    SiacoinToHastings(#[from] SiacoinToHastingsError),
    #[error("SiaCoin::new_send_maker_payment: failed to fund transaction {0}")]
    FundTx(#[from] V2TransactionBuilderError),
    #[error("SiaCoin::new_send_maker_payment: failed to parse secret_hash {0}")]
    ParseSecretHash(#[from] Hash256Error),
    #[error("SiaCoin::new_send_maker_payment: failed to broadcast maker_payment transaction {0}")]
    BroadcastTx(#[from] BroadcastTransactionError),
}

#[derive(Debug, Error)]
pub enum SendTakerPaymentError {
    #[error("SiaCoin::new_send_taker_payment: invalid maker pubkey, expected 33 bytes found: {0:?}")]
    InvalidMakerPublicKeyLength(Vec<u8>),
    #[error("SiaCoin::new_send_taker_payment: invalid maker pubkey {0}")]
    InvalidMakerPublicKey(#[from] PublicKeyError),
    #[error("SiaCoin::new_send_taker_payment: failed to fetch my_keypair {0}")]
    MyKeypair(#[from] SiaCoinMyKeypairError),
    #[error("SiaCoin::new_send_taker_payment: failed to convert trade amount to Currency {0}")]
    SiacoinToHastings(#[from] SiacoinToHastingsError),
    #[error("SiaCoin::new_send_taker_payment: failed to fund transaction {0}")]
    FundTx(#[from] V2TransactionBuilderError),
    #[error("SiaCoin::new_send_taker_payment: invalid secret_hash length {0}")]
    SecretHashLength(#[from] Hash256Error),
    #[error("SiaCoin::new_send_taker_payment: failed to broadcast taker_payment transaction {0}")]
    BroadcastTx(#[from] BroadcastTransactionError),
}

/// Wrapper around SendRefundHltcError to allow indicating Maker or Taker context within the error
#[derive(Debug, Error)]
pub enum SendRefundHltcMakerOrTakerError {
    #[error("SiaCoin::send_refund_hltc: maker: {0}")]
    Maker(SendRefundHltcError),
    #[error("SiaCoin::send_refund_hltc: taker: {0}")]
    Taker(SendRefundHltcError),
}

#[derive(Debug, Error)]
pub enum SendRefundHltcError {
    #[error("SiaCoin::send_refund_hltc: failed to fetch my_keypair: {0}")]
    MyKeypair(#[from] SiaCoinMyKeypairError),
    #[error("SiaCoin::send_refund_hltc: failed to parse RefundPaymentArgs: {0}")]
    ParseArgs(#[from] SiaRefundPaymentArgsError),
    #[error("SiaCoin::send_refund_hltc: failed to fetch SiacoinElement from txid {0}")]
    // TODO: This is boxed since it's very large compared to the other variants.
    //       This shows up in many different enums where this embedded field is used as a variant.
    //       We should consider boxing the `EventVariant` within this field instead (requires changes in sia-rust).
    UtxoFromTxid(#[from] Box<UtxoFromTxidError>),
    #[error("SiaCoin::send_refund_hltc: failed to satisfy HTLC SpendPolicy {0}")]
    SatisfyHtlc(#[from] V2TransactionBuilderError),
    #[error("SiaCoin::send_refund_hltc: failed to broadcast transaction {0}")]
    BroadcastTx(#[from] BroadcastTransactionError),
}

#[derive(Debug, Error)]
pub enum ValidateFeeError {
    #[error("SiaCoin::new_validate_fee: failed to parse ValidateFeeArgs {0}")]
    ParseArgs(#[from] SiaValidateFeeArgsError),
    #[error("SiaCoin::new_validate_fee: failed to fetch mempool: {0}")]
    FetchMempool(#[from] GetUnconfirmedTransactionError),
    #[error("SiaCoin::new_validate_fee: fee_tx:{0} not found on chain or in mempool")]
    TxNotFound(TransactionId),
    #[error("SiaCoin::new_validate_fee: unexpected event variant: {0:?}")]
    EventVariant(Event),
    #[error("SiaCoin::new_validate_fee: tx confirmed before min_block_number:{min_block_number} txid:{txid}")]
    MininumConfirmedHeight { txid: TransactionId, min_block_number: u64 },
    #[error("SiaCoin::new_validate_fee: failed to fetch current_height: {0}")]
    FetchHeight(#[from] CurrentHeightError),
    #[error("SiaCoin::new_validate_fee: tx in mempool before height:{min_block_number} txid:{txid}")]
    MininumMempoolHeight { txid: TransactionId, min_block_number: u64 },
    #[error("SiaCoin::new_validate_fee: all inputs do not originate from taker address txid:{0}")]
    InputsOrigin(TransactionId),
    #[error("SiaCoin::new_validate_fee: fee_tx:{txid} has {outputs_length} outputs, expected 1 or 2")]
    VoutLength { txid: TransactionId, outputs_length: usize },
    #[error("SiaCoin::new_validate_fee: fee_tx:{txid} pays wrong address:{address}")]
    InvalidFeeAddress { txid: TransactionId, address: Address },
    #[error("SiaCoin::new_validate_fee: fee_tx:{txid} pays wrong amount. expected:{expected} actual:{actual}")]
    InvalidFeeAmount {
        txid: TransactionId,
        expected: Currency,
        actual: Currency,
    },
    #[error("SiaCoin::new_validate_fee: failed to parse uuid from arbitrary_bytes {0}")]
    ParseUuid(#[from] uuid::Error),
    #[error("SiaCoin::new_validate_fee: fee_tx:{txid} wrong uuid. expected:{expected} actual:{actual}")]
    InvalidUuid {
        txid: TransactionId,
        expected: Uuid,
        actual: Uuid,
    },
}

// TODO Alright - nearly identical to MakerSpendsTakerPaymentError
// refactor similar to SendRefundHltcMakerOrTakerError
#[derive(Debug, Error)]
pub enum TakerSpendsMakerPaymentError {
    #[error("SiaCoin::new_send_taker_spends_maker_payment: failed to fetch my_keypair {0}")]
    MyKeypair(#[from] SiaCoinMyKeypairError),
    #[error("SiaCoin::new_send_taker_spends_maker_payment: invalid maker pubkey, expected 33 bytes found: {0:?}")]
    InvalidMakerPublicKeyLength(Vec<u8>),
    #[error("SiaCoin::new_send_taker_spends_maker_payment: invalid maker pubkey {0}")]
    InvalidMakerPublicKey(#[from] PublicKeyError),
    #[error("SiaCoin::new_send_taker_spends_maker_paymentt: failed to parse taker_payment_tx {0}")]
    ParseTx(#[from] SiaTransactionError),
    #[error("SiaCoin::new_send_taker_spends_maker_payment: failed to parse secret {0}")]
    ParseSecret(#[from] PreimageError),
    #[error("SiaCoin::new_send_taker_spends_maker_payment: failed to parse secret_hash {0}")]
    ParseSecretHash(#[from] Hash256Error),
    #[error("SiaCoin::new_send_taker_spends_maker_payment: failed to fetch SiacoinElement from txid {0}")]
    UtxoFromTxid(#[from] Box<UtxoFromTxidError>),
    #[error("SiaCoin::new_send_taker_spends_maker_payment: failed to satisfy HTLC SpendPolicy {0}")]
    SatisfyHtlc(#[from] V2TransactionBuilderError),
    #[error("SiaCoin::new_send_taker_spends_maker_payment: failed to broadcast spend_maker_payment transaction {0}")]
    BroadcastTx(#[from] BroadcastTransactionError),
}

#[derive(Debug, Error)]
pub enum MakerSpendsTakerPaymentError {
    #[error("SiaCoin::new_send_maker_spends_taker_payment: failed to fetch my_keypair {0}")]
    MyKeypair(#[from] SiaCoinMyKeypairError),
    #[error("SiaCoin::new_send_maker_spends_taker_payment: invalid taker pubkey, expected 33 bytes found: {0:?}")]
    InvalidTakerPublicKeyLength(Vec<u8>),
    #[error("SiaCoin::new_send_maker_spends_taker_payment: invalid taker pubkey {0}")]
    InvalidTakerPublicKey(#[from] PublicKeyError),
    #[error("SiaCoin::new_send_maker_spends_taker_payment: failed to parse taker_payment_tx {0}")]
    ParseTx(#[from] SiaTransactionError),
    #[error("SiaCoin::new_send_maker_spends_taker_payment: failed to parse secret {0}")]
    ParseSecret(#[from] PreimageError),
    #[error("SiaCoin::new_send_maker_spends_taker_payment: failed to parse secret_hash {0}")]
    ParseSecretHash(#[from] Hash256Error),
    #[error("SiaCoin::new_send_maker_spends_taker_payment: failed to fetch SiacoinElement from txid {0}")]
    UtxoFromTxid(#[from] Box<UtxoFromTxidError>),
    #[error("SiaCoin::new_send_maker_spends_taker_payment: failed to satisfy HTLC SpendPolicy {0}")]
    SatisfyHtlc(#[from] V2TransactionBuilderError),
    #[error("SiaCoin::new_send_maker_spends_taker_payment: failed to broadcast spend_taker_payment transaction {0}")]
    BroadcastTx(#[from] BroadcastTransactionError),
}

#[derive(Debug, Error)]
pub enum SiaRefundPaymentArgsError {
    #[error("SiaRefundPaymentArgs::TryFrom<RefundPaymentArgs>: failed to parse payment_tx {0}")]
    ParseTx(#[from] SiaTransactionError),
    #[error("SiaRefundPaymentArgs::TryFrom<RefundPaymentArgs>: invalid other_pubkey, expected 33 bytes found: {0:?}")]
    InvalidOtherPublicKeyLength(Vec<u8>),
    #[error("SiaRefundPaymentArgs::TryFrom<RefundPaymentArgs>: failed to parse other_pubkey {0}")]
    ParseOtherPublicKey(#[from] PublicKeyError),
    #[error("SiaRefundPaymentArgs::TryFrom<RefundPaymentArgs>: failed to parse secret_hash {0}")]
    ParseSecretHash(#[from] Hash256Error),
    // SwapTxTypeVariant uses String Debug trait representation to avoid explicit lifetime annotations
    // otherwise this should be SwapTxTypeVariant(SwapTxTypeWithSecretHash) and displayed via {0:?}
    #[error("SiaRefundPaymentArgs::TryFrom<RefundPaymentArgs>: unexpected SwapTxTypeWithSecretHash variant {0}")]
    SwapTxTypeVariant(String),
}

#[derive(Debug, Error)]
#[allow(clippy::large_enum_variant)]
pub enum SiaValidateFeeArgsError {
    #[error("SiaValidateFeeArgs::TryFrom<ValidateFeeArgs>: failed to parse uuid from bytes {0}")]
    ParseUuid(#[from] uuid::Error),
    #[error("SiaValidateFeeArgs::TryFrom<ValidateFeeArgs>: Unexpected Uuid version {0}")]
    UuidVersion(usize),
    #[error("SiaValidateFeeArgs::TryFrom<ValidateFeeArgs>: invalid taker pubkey, expected 33 bytes found: {0:?}")]
    InvalidTakerPublicKeyLength(Vec<u8>),
    #[error("SiaValidateFeeArgs::TryFrom<ValidateFeeArgs>: invalid taker pubkey {0}")]
    InvalidTakerPublicKey(#[from] PublicKeyError),
    #[error("SiaValidateFeeArgs::TryFrom<ValidateFeeArgs>: failed to convert trade_fee_amount to Currency {0}")]
    SiacoinToHastings(#[from] SiacoinToHastingsError),
    #[error("SiaValidateFeeArgs::TryFrom<ValidateFeeArgs>: unexpected DexFee variant {0:?}")]
    DexFeeVariant(DexFee),
    #[error("SiaValidateFeeArgs::TryFrom<ValidateFeeArgs>: unexpected TransactionEnum variant {0:?}")]
    TxEnumVariant(TransactionEnum),
}

#[derive(Debug, Error)]
pub enum SiaTransactionError {
    #[error("Vec<u8>::TryFrom<SiaTransaction>: failed to convert to Vec<u8>")]
    ToVec(serde_json::Error),
    #[error("SiaTransaction::TryFrom<Vec<u8>>: failed to convert from Vec<u8>")]
    FromVec(serde_json::Error),
}

#[derive(Debug, Error)]
pub enum SiaCoinBuilderError {
    #[error("SiaCoinBuilder::build: failed to create abortable system: {0}")]
    AbortableSystem(AbortedError),
    #[error("SiaCoinBuilder::build: failed to initialize client {0}")]
    Client(#[from] ClientError),
}

// This is required because AbortedError doesn't impl Error
impl From<AbortedError> for SiaCoinBuilderError {
    fn from(e: AbortedError) -> Self {
        SiaCoinBuilderError::AbortableSystem(e)
    }
}

#[derive(Debug, Error)]
pub enum SiaCoinNewError {
    #[error("SiaCoin::new: failed to parse SiaCoinConf from JSON: {0}")]
    InvalidConf(#[from] serde_json::Error),
    #[error("SiaCoin::new: invalid private key: {0}")]
    InvalidPrivateKey(#[from] KeypairError),
    #[error("SiaCoin::new: invalid private key policy, must use iguana seed")]
    UnsupportedPrivKeyPolicy,
    #[error("SiaCoin::new: failed to build SiaCoin: {0}")]
    Builder(#[from] SiaCoinBuilderError),
    #[error("SiaCoin::new: failed to derive address from master extended key: {0}")]
    DeriveExtendedKey(#[from] PrivKeyError),
}

#[derive(Debug, Error)]
pub enum SiaCoinMyKeypairError {
    #[error("SiaCoin::my_keypair: invalid private key policy, must use iguana seed")]
    PrivKeyPolicy,
}

#[derive(Debug, Error)]
pub enum SiaCheckIfMyPaymentSentArgsError {
    #[error("SiaCheckIfMyPaymentSentArgs::TryFrom<CheckIfMyPaymentSentArgs>: invalid other_pub, expected 33 bytes found: {0:?}")]
    InvalidOtherPublicKeyLength(Vec<u8>),
    #[error("SiaCheckIfMyPaymentSentArgs::TryFrom<CheckIfMyPaymentSentArgs>: failed to parse other_pub {0}")]
    ParseOtherPublicKey(#[from] PublicKeyError),
    #[error("SiaCheckIfMyPaymentSentArgs::TryFrom<CheckIfMyPaymentSentArgs>: failed to parse secret_hash {0}")]
    ParseSecretHash(#[from] Hash256Error),
    #[error(
        "SiaCheckIfMyPaymentSentArgs::TryFrom<CheckIfMyPaymentSentArgs>: failed to convert amount to Currency {0}"
    )]
    SiacoinToHastings(#[from] SiacoinToHastingsError),
}

#[derive(Debug, Error)]
#[allow(clippy::large_enum_variant)]
pub enum SiaCheckIfMyPaymentSentError {
    #[error("SiaCoin::new_check_if_my_payment_sent: failed to parse CheckIfMyPaymentSentArgs: {0}")]
    ParseArgs(#[from] SiaCheckIfMyPaymentSentArgsError),
    #[error("SiaCoin::new_check_if_my_payment_sent: invalid private key policy, must use iguana seed")]
    MyKeypair(#[from] SiaCoinMyKeypairError),
    #[error("SiaCoin::new_check_if_my_payment_sent: unexpected event variant: {0:?}")]
    EventVariant(EventDataWrapper),
}

#[derive(Debug, Error)]
#[allow(clippy::large_enum_variant)]
pub enum SiaCoinSiaExtractSecretError {
    #[error("SiaCoin::sia_extract_secret: failed to parse spend_tx {0}")]
    ParseTx(#[from] SiaTransactionError),
    #[error("SiaCoin::sia_extract_secret: failed to parse secret_hash {0}")]
    ParseSecretHash(#[from] Hash256Error),
    #[error(
        "SiaCoin::sia_extract_secret: failed to extract secret of secret_hash:{expected_hash} from spend_tx: {tx}"
    )]
    FailedToExtract { expected_hash: Hash256, tx: SiaTransaction },
}

#[derive(Debug, Error)]
pub enum SiaCoinSiaCanRefundHtlcError {
    #[error("SiaCoin::sia_can_refund_htlc: failed to fetch median_timestamp: {0}")]
    FetchTimestamp(#[from] GetMedianTimestampError),
}

#[derive(Debug, Error)]
pub enum SiaWaitForHTLCTxSpendArgsError {
    #[error("SiaWaitForHTLCTxSpendArgs::TryFrom<WaitForHTLCTxSpendArgs>: Failed to parse transaction: {0}")]
    ParseTx(#[from] SiaTransactionError),
    #[error("SiaWaitForHTLCTxSpendArgs::TryFrom<WaitForHTLCTxSpendArgs>: Failed to parse secret hash: {0}")]
    ParseSecretHash(#[from] Hash256Error),
}

#[derive(Debug, Error)]
pub enum SiaWaitForHTLCTxSpendError {
    #[error("SiaCoin::sia_wait_for_htlc_tx_spend: Failed to parse arguments: {0}")]
    ParseArgs(#[from] SiaWaitForHTLCTxSpendArgsError),
    #[error("SiaCoin::sia_wait_for_htlc_tx_spend: timed out waiting for spend of txid:{txid} vout 0")]
    Timeout { txid: TransactionId },
    #[error("SiaCoin::sia_wait_for_htlc_tx_spend: find_where_utxo_spent failed: {0}")]
    FindWhereUtxoSpent(#[from] Box<FindWhereUtxoSpentError>),
}

#[derive(Debug, Error)]
pub enum SiaValidatePaymentInputError {
    #[error("SiaValidatePaymentInput::TryFrom<ValidatePaymentInput>: Failed to parse payment_tx: {0}")]
    ParseTx(#[from] SiaTransactionError),
    #[error("SiaValidateFeeArgs::TryFrom<ValidateFeeArgs>: invalid other_pub, expected 33 bytes found: {0:?}")]
    InvalidOtherPublicKeyLength(Vec<u8>),
    #[error("SiaValidateFeeArgs::TryFrom<ValidateFeeArgs>: Failed to parse other_pub: {0}")]
    ParseOtherPublicKey(#[from] PublicKeyError),
    #[error("SiaValidateFeeArgs::TryFrom<ValidateFeeArgs>: Failed to parse secret_hash: {0}")]
    ParseSecretHash(#[from] Hash256Error),
    #[error("SiaValidateFeeArgs::TryFrom<ValidateFeeArgs>: failed to convert amount to Currency: {0}")]
    SiacoinToHastings(#[from] SiacoinToHastingsError),
}

#[derive(Debug, Error)]
#[allow(clippy::large_enum_variant)]
pub enum SiaValidateHtlcPaymentError {
    #[error("SiaCoin::validate_htlc_payment: failed to parse ValidatePaymentInput: {0}")]
    ParseArgs(#[from] SiaValidatePaymentInputError),
    #[error("SiaCoin::validate_htlc_payment: failed to fetch my_keypair {0}")]
    MyKeypair(#[from] SiaCoinMyKeypairError),
    #[error("SiaCoin::validate_htlc_payment: unexpected event variant, expected V2Transaction, found: {0:?}")]
    EventVariant(Event),
    #[error("SiaCoin::validate_htlc_payment: txid:{txid} has {actual} inputs, expected at least:{expected}")]
    InvalidOutputLength {
        expected: u32,
        actual: u32,
        txid: TransactionId,
    },
    #[error("SiaCoin::validate_htlc_payment: txid:{txid} has unexpected output:{actual:?}, expected:{expected:?}")]
    InvalidOutput {
        expected: SiacoinOutput,
        actual: SiacoinOutput,
        txid: TransactionId,
    },
}

#[derive(Debug, Error)]
pub enum SiaValidateMakerPaymentError {
    #[error("SiaCoin::sia_validate_maker_payment: validation failed: {0}")]
    ValidatePayment(#[from] SiaValidateHtlcPaymentError),
}

#[derive(Debug, Error)]
pub enum SiaValidateTakerPaymentError {
    #[error("SiaCoin::sia_validate_taker_payment: validation failed: {0}")]
    ValidatePayment(#[from] SiaValidateHtlcPaymentError),
}
