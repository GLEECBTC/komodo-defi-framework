use super::{
    BalanceError, CoinBalance, HistorySyncState, MarketCoinOps, MmCoin, RawTransactionFut, RawTransactionRequest,
    SwapOps, TradeFee, TransactionEnum,
};
use crate::hd_wallet::HDAddressSelector;
use crate::{
    coin_errors::MyAddressError, AddressFromPubkeyError, BalanceFut, CanRefundHtlc, CheckIfMyPaymentSentArgs,
    ConfirmPaymentInput, DexFee, FeeApproxStage, FoundSwapTxSpend, NegotiateSwapContractAddrErr, PrivKeyBuildPolicy,
    PrivKeyPolicy, RawTransactionResult, RefundPaymentArgs, SearchForSwapTxSpendInput, SendPaymentArgs,
    SignRawTransactionRequest, SignatureResult, SpendPaymentArgs, TradePreimageFut, TradePreimageResult,
    TradePreimageValue, TransactionResult, TxMarshalingErr, UnexpectedDerivationMethod, ValidateAddressResult,
    ValidateFeeArgs, ValidateOtherPubKeyErr, ValidatePaymentInput, ValidatePaymentResult, VerificationResult,
    WaitForHTLCTxSpendArgs, WatcherOps, WeakSpawner, WithdrawFut, WithdrawRequest,
};
use crate::{SignatureError, VerificationError};
use async_trait::async_trait;
use common::executor::AbortedError;
use derive_more::Display;
pub use ed25519_dalek::{Keypair, PublicKey, SecretKey, Signature};
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use keys::KeyPair;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, BigInt, MmNumber};
use rpc::v1::types::{Bytes as BytesJson, H264 as H264Json};
use serde_json::Value as Json;
use std::ops::Deref;
use std::sync::Arc;

use sia_rust::http_client::{SiaApiClient, SiaApiClientError, SiaHttpConf};
use sia_rust::spend_policy::SpendPolicy;

pub mod sia_hd_wallet;

#[derive(Clone)]
pub struct SiaCoin(SiaArc);
#[derive(Clone)]
pub struct SiaArc(Arc<SiaCoinFields>);

#[derive(Debug, Display)]
pub enum SiaConfError {
    #[display(fmt = "'foo' field is not found in config")]
    Foo,
    Bar(String),
}

pub type SiaConfResult<T> = Result<T, MmError<SiaConfError>>;

#[derive(Debug)]
pub struct SiaCoinConf {
    ticker: String,
    pub foo: u32,
}

// TODO see https://github.com/KomodoPlatform/komodo-defi-framework/pull/2086#discussion_r1521660384
// for additional fields needed
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SiaCoinActivationParams {
    #[serde(default)]
    pub tx_history: bool,
    pub required_confirmations: Option<u64>,
    pub gap_limit: Option<u32>,
    pub http_conf: SiaHttpConf,
}

pub struct SiaConfBuilder<'a> {
    #[allow(dead_code)]
    conf: &'a Json,
    ticker: &'a str,
}

impl<'a> SiaConfBuilder<'a> {
    pub fn new(conf: &'a Json, ticker: &'a str) -> Self {
        SiaConfBuilder { conf, ticker }
    }

    pub fn build(&self) -> SiaConfResult<SiaCoinConf> {
        Ok(SiaCoinConf {
            ticker: self.ticker.to_owned(),
            foo: 0,
        })
    }
}

// TODO see https://github.com/KomodoPlatform/komodo-defi-framework/pull/2086#discussion_r1521668313
// for additional fields needed
pub struct SiaCoinFields {
    /// SIA coin config
    pub conf: SiaCoinConf,
    pub priv_key_policy: PrivKeyPolicy<Keypair>,
    /// HTTP(s) client
    pub http_client: SiaApiClient,
}

pub async fn sia_coin_from_conf_and_params(
    ctx: &MmArc,
    ticker: &str,
    conf: &Json,
    params: &SiaCoinActivationParams,
    priv_key_policy: PrivKeyBuildPolicy,
) -> Result<SiaCoin, MmError<SiaCoinBuildError>> {
    let priv_key = match priv_key_policy {
        PrivKeyBuildPolicy::IguanaPrivKey(priv_key) => priv_key,
        _ => return Err(SiaCoinBuildError::UnsupportedPrivKeyPolicy.into()),
    };
    let key_pair = generate_keypair_from_slice(priv_key.as_slice())?;
    let builder = SiaCoinBuilder::new(ctx, ticker, conf, key_pair, params);
    builder.build().await
}

pub struct SiaCoinBuilder<'a> {
    ctx: &'a MmArc,
    ticker: &'a str,
    conf: &'a Json,
    key_pair: Keypair,
    params: &'a SiaCoinActivationParams,
}

impl<'a> SiaCoinBuilder<'a> {
    pub fn new(
        ctx: &'a MmArc,
        ticker: &'a str,
        conf: &'a Json,
        key_pair: Keypair,
        params: &'a SiaCoinActivationParams,
    ) -> Self {
        SiaCoinBuilder {
            ctx,
            ticker,
            conf,
            key_pair,
            params,
        }
    }
}

fn generate_keypair_from_slice(priv_key: &[u8]) -> Result<Keypair, SiaCoinBuildError> {
    let secret_key = SecretKey::from_bytes(priv_key).map_err(SiaCoinBuildError::EllipticCurveError)?;
    let public_key = PublicKey::from(&secret_key);
    Ok(Keypair {
        secret: secret_key,
        public: public_key,
    })
}

/// Convert hastings amount to siacoin amount
fn siacoin_from_hastings(hastings: u128) -> BigDecimal {
    let hastings = BigInt::from(hastings);
    let decimals = BigInt::from(10u128.pow(24));
    BigDecimal::from(hastings) / BigDecimal::from(decimals)
}

impl From<SiaConfError> for SiaCoinBuildError {
    fn from(e: SiaConfError) -> Self {
        SiaCoinBuildError::ConfError(e)
    }
}

#[derive(Debug, Display)]
pub enum SiaCoinBuildError {
    ConfError(SiaConfError),
    UnsupportedPrivKeyPolicy,
    ClientError(SiaApiClientError),
    EllipticCurveError(ed25519_dalek::ed25519::Error),
}

impl SiaCoinBuilder<'_> {
    #[allow(dead_code)]
    fn ctx(&self) -> &MmArc {
        self.ctx
    }

    #[allow(dead_code)]
    fn conf(&self) -> &Json {
        self.conf
    }

    fn ticker(&self) -> &str {
        self.ticker
    }

    async fn build(self) -> MmResult<SiaCoin, SiaCoinBuildError> {
        let conf = SiaConfBuilder::new(self.conf, self.ticker()).build().map_mm_err()?;
        let sia_fields = SiaCoinFields {
            conf,
            http_client: SiaApiClient::new(self.params.http_conf.clone())
                .map_err(SiaCoinBuildError::ClientError)
                .await?,
            priv_key_policy: PrivKeyPolicy::Iguana(self.key_pair),
        };
        let sia_arc = SiaArc::new(sia_fields);

        Ok(SiaCoin::from(sia_arc))
    }
}

impl Deref for SiaArc {
    type Target = SiaCoinFields;
    fn deref(&self) -> &SiaCoinFields {
        &self.0
    }
}

impl From<SiaCoinFields> for SiaArc {
    fn from(coin: SiaCoinFields) -> SiaArc {
        SiaArc::new(coin)
    }
}

impl From<Arc<SiaCoinFields>> for SiaArc {
    fn from(arc: Arc<SiaCoinFields>) -> SiaArc {
        SiaArc(arc)
    }
}

impl From<SiaArc> for SiaCoin {
    fn from(coin: SiaArc) -> SiaCoin {
        SiaCoin(coin)
    }
}

impl SiaArc {
    pub fn new(fields: SiaCoinFields) -> SiaArc {
        SiaArc(Arc::new(fields))
    }

    pub fn with_arc(inner: Arc<SiaCoinFields>) -> SiaArc {
        SiaArc(inner)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SiaCoinProtocolInfo;

#[async_trait]
impl MmCoin for SiaCoin {
    fn is_asset_chain(&self) -> bool {
        false
    }

    fn spawner(&self) -> WeakSpawner {
        unimplemented!()
    }

    fn get_raw_transaction(&self, _req: RawTransactionRequest) -> RawTransactionFut<'_> {
        unimplemented!()
    }

    fn get_tx_hex_by_hash(&self, _tx_hash: Vec<u8>) -> RawTransactionFut<'_> {
        unimplemented!()
    }

    fn withdraw(&self, _req: WithdrawRequest) -> WithdrawFut {
        unimplemented!()
    }

    fn decimals(&self) -> u8 {
        unimplemented!()
    }

    fn convert_to_address(&self, _from: &str, _to_address_format: Json) -> Result<String, String> {
        unimplemented!()
    }

    fn validate_address(&self, _address: &str) -> ValidateAddressResult {
        unimplemented!()
    }

    fn process_history_loop(&self, _ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn history_sync_status(&self) -> HistorySyncState {
        unimplemented!()
    }

    /// Get fee to be paid per 1 swap transaction
    fn get_trade_fee(&self) -> Box<dyn Future<Item = TradeFee, Error = String> + Send> {
        unimplemented!()
    }

    async fn get_sender_trade_fee(
        &self,
        _value: TradePreimageValue,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        unimplemented!()
    }

    fn get_receiver_trade_fee(&self, _stage: FeeApproxStage) -> TradePreimageFut<TradeFee> {
        unimplemented!()
    }

    async fn get_fee_to_send_taker_fee(
        &self,
        _dex_fee_amount: DexFee,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        unimplemented!()
    }

    fn required_confirmations(&self) -> u64 {
        unimplemented!()
    }

    fn requires_notarization(&self) -> bool {
        false
    }

    fn set_required_confirmations(&self, _confirmations: u64) {
        unimplemented!()
    }

    fn set_requires_notarization(&self, _requires_nota: bool) {
        unimplemented!()
    }

    fn swap_contract_address(&self) -> Option<BytesJson> {
        unimplemented!()
    }

    fn fallback_swap_contract(&self) -> Option<BytesJson> {
        unimplemented!()
    }

    fn mature_confirmations(&self) -> Option<u32> {
        unimplemented!()
    }

    fn coin_protocol_info(&self, _amount_to_receive: Option<MmNumber>) -> Vec<u8> {
        Vec::new()
    }

    fn is_coin_protocol_supported(
        &self,
        _info: &Option<Vec<u8>>,
        _amount_to_send: Option<MmNumber>,
        _locktime: u64,
        _is_maker: bool,
    ) -> bool {
        true
    }

    fn on_disabled(&self) -> Result<(), AbortedError> {
        Ok(())
    }

    fn on_token_deactivated(&self, _ticker: &str) {}
}

// TODO Alright - Dummy values for these functions allow minimal functionality to produce signatures
#[async_trait]
impl MarketCoinOps for SiaCoin {
    fn ticker(&self) -> &str {
        &self.0.conf.ticker
    }

    // needs test coverage FIXME COME BACK
    fn my_address(&self) -> MmResult<String, MyAddressError> {
        let key_pair = match &self.0.priv_key_policy {
            PrivKeyPolicy::Iguana(key_pair) => key_pair,
            PrivKeyPolicy::Trezor => {
                return Err(MyAddressError::UnexpectedDerivationMethod(
                    "Trezor not yet supported. Must use iguana seed.".to_string(),
                )
                .into());
            },
            PrivKeyPolicy::HDWallet { .. } => {
                return Err(MyAddressError::UnexpectedDerivationMethod(
                    "HDWallet not yet supported. Must use iguana seed.".to_string(),
                )
                .into());
            },
            #[cfg(target_arch = "wasm32")]
            PrivKeyPolicy::Metamask(_) => {
                return Err(MyAddressError::UnexpectedDerivationMethod(
                    "Metamask not supported. Must use iguana seed.".to_string(),
                )
                .into());
            },
            PrivKeyPolicy::WalletConnect { .. } => {
                return Err(MyAddressError::UnexpectedDerivationMethod(
                    "WalletConnect not yet supported. Must use iguana seed.".to_string(),
                )
                .into())
            },
        };
        let address = SpendPolicy::PublicKey(key_pair.public).address();
        Ok(address.to_string())
    }

    fn address_from_pubkey(&self, pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError> {
        let pubkey = PublicKey::from_bytes(&pubkey.0[..32]).map_err(|e| {
            AddressFromPubkeyError::InternalError(format!("Couldn't parse bytes into ed25519 pubkey: {e:?}"))
        })?;
        let address = SpendPolicy::PublicKey(pubkey).address();
        Ok(address.to_string())
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        MmError::err(UnexpectedDerivationMethod::InternalError("Not implemented".into()))
    }

    fn sign_message_hash(&self, _message: &str) -> Option<[u8; 32]> {
        None
    }

    fn sign_message(&self, _message: &str, _address: Option<HDAddressSelector>) -> SignatureResult<String> {
        MmError::err(SignatureError::InternalError("Not implemented".into()))
    }

    fn verify_message(&self, _signature: &str, _message: &str, _address: &str) -> VerificationResult<bool> {
        MmError::err(VerificationError::InternalError("Not implemented".into()))
    }

    fn my_balance(&self) -> BalanceFut<CoinBalance> {
        let coin = self.clone();
        let fut = async move {
            let my_address = match &coin.0.priv_key_policy {
                PrivKeyPolicy::Iguana(key_pair) => SpendPolicy::PublicKey(key_pair.public).address(),
                _ => {
                    return MmError::err(BalanceError::UnexpectedDerivationMethod(
                        UnexpectedDerivationMethod::ExpectedSingleAddress,
                    ))
                },
            };
            let balance = coin
                .0
                .http_client
                .address_balance(my_address)
                .await
                .map_to_mm(|e| BalanceError::Transport(e.to_string()))?;
            Ok(CoinBalance {
                spendable: siacoin_from_hastings(balance.siacoins.to_u128()),
                unspendable: siacoin_from_hastings(balance.immature_siacoins.to_u128()),
            })
        };
        Box::new(fut.boxed().compat())
    }

    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal> {
        unimplemented!()
    }

    fn platform_ticker(&self) -> &str {
        "FOO"
    } // TODO Alright

    /// Receives raw transaction bytes in hexadecimal format as input and returns tx hash in hexadecimal format
    fn send_raw_tx(&self, _tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send> {
        unimplemented!()
    }

    fn send_raw_tx_bytes(&self, _tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        unimplemented!()
    }

    #[inline(always)]
    async fn sign_raw_tx(&self, _args: &SignRawTransactionRequest) -> RawTransactionResult {
        unimplemented!()
    }

    fn wait_for_confirmations(&self, _input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        unimplemented!()
    }

    async fn wait_for_htlc_tx_spend(&self, _args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult {
        unimplemented!()
    }

    fn tx_enum_from_bytes(&self, _bytes: &[u8]) -> Result<TransactionEnum, MmError<TxMarshalingErr>> {
        MmError::err(TxMarshalingErr::NotSupported(
            "tx_enum_from_bytes is not supported for Sia coin yet.".to_string(),
        ))
    }

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send> {
        let http_client = self.0.http_client.clone(); // Clone the client

        let height_fut = async move { http_client.current_height().await.map_err(|e| e.to_string()) }
            .boxed() // Make the future 'static by boxing
            .compat(); // Convert to a futures 0.1-compatible future

        Box::new(height_fut)
    }

    fn display_priv_key(&self) -> Result<String, String> {
        unimplemented!()
    }

    fn min_tx_amount(&self) -> BigDecimal {
        unimplemented!()
    }

    fn min_trading_vol(&self) -> MmNumber {
        unimplemented!()
    }

    fn should_burn_dex_fee(&self) -> bool {
        false
    }

    fn is_trezor(&self) -> bool {
        self.0.priv_key_policy.is_trezor()
    }
}

#[async_trait]
impl SwapOps for SiaCoin {
    async fn send_taker_fee(&self, _dex_fee: DexFee, _uuid: &[u8], _expire_at: u64) -> TransactionResult {
        unimplemented!()
    }

    async fn send_maker_payment(&self, _maker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        unimplemented!()
    }

    async fn send_taker_payment(&self, _taker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        unimplemented!()
    }

    async fn send_maker_spends_taker_payment(
        &self,
        _maker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        unimplemented!()
    }

    async fn send_taker_spends_maker_payment(
        &self,
        _taker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        unimplemented!()
    }

    async fn send_taker_refunds_payment(
        &self,
        _taker_refunds_payment_args: RefundPaymentArgs<'_>,
    ) -> TransactionResult {
        unimplemented!()
    }

    async fn send_maker_refunds_payment(
        &self,
        _maker_refunds_payment_args: RefundPaymentArgs<'_>,
    ) -> TransactionResult {
        unimplemented!()
    }

    async fn validate_fee(&self, _validate_fee_args: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()> {
        unimplemented!()
    }

    async fn validate_maker_payment(&self, _input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        unimplemented!()
    }

    async fn validate_taker_payment(&self, _input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        unimplemented!()
    }

    async fn check_if_my_payment_sent(
        &self,
        _if_my_payment_sent_args: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<TransactionEnum>, String> {
        unimplemented!()
    }

    async fn search_for_swap_tx_spend_my(
        &self,
        _: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        unimplemented!()
    }

    async fn search_for_swap_tx_spend_other(
        &self,
        _: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        unimplemented!()
    }

    async fn extract_secret(
        &self,
        _secret_hash: &[u8],
        _spend_tx: &[u8],
        _watcher_reward: bool,
    ) -> Result<[u8; 32], String> {
        unimplemented!()
    }

    fn negotiate_swap_contract_addr(
        &self,
        _other_side_address: Option<&[u8]>,
    ) -> Result<Option<BytesJson>, MmError<NegotiateSwapContractAddrErr>> {
        Ok(None)
    }

    fn derive_htlc_key_pair(&self, _swap_unique_data: &[u8]) -> KeyPair {
        unimplemented!()
    }

    fn derive_htlc_pubkey(&self, _swap_unique_data: &[u8]) -> [u8; 33] {
        unimplemented!()
    }

    async fn can_refund_htlc(&self, _locktime: u64) -> Result<CanRefundHtlc, String> {
        unimplemented!()
    }

    fn validate_other_pubkey(&self, _raw_pubkey: &[u8]) -> MmResult<(), ValidateOtherPubKeyErr> {
        unimplemented!()
    }
}

#[async_trait]
impl WatcherOps for SiaCoin {}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_number::BigDecimal;
    use std::str::FromStr;

    #[test]
    fn test_siacoin_from_hastings() {
        let hastings = u128::MAX;
        let siacoin = siacoin_from_hastings(hastings);
        assert_eq!(
            siacoin,
            BigDecimal::from_str("340282366920938.463463374607431768211455").unwrap()
        );

        let hastings = 0;
        let siacoin = siacoin_from_hastings(hastings);
        assert_eq!(siacoin, BigDecimal::from_str("0").unwrap());

        // Total supply of Siacoin
        let hastings = 57769875000000000000000000000000000;
        let siacoin = siacoin_from_hastings(hastings);
        assert_eq!(siacoin, BigDecimal::from_str("57769875000").unwrap());
    }
}
