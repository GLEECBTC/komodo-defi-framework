use crate::context::CoinsActivationContext;
use crate::prelude::TryFromCoinProtocol;
use crate::standalone_coin::{
    InitStandaloneCoinActivationOps, InitStandaloneCoinTaskHandleShared, InitStandaloneCoinTaskManagerShared,
};
use crate::utxo_activation::common_impl::{
    get_activation_result, priv_key_build_policy, start_history_background_fetching,
};
use crate::utxo_activation::init_utxo_standard_activation_error::InitUtxoStandardError;
use crate::utxo_activation::init_utxo_standard_statuses::{
    UtxoStandardAwaitingStatus, UtxoStandardInProgressStatus, UtxoStandardUserAction,
};
use crate::utxo_activation::utxo_standard_activation_result::UtxoStandardActivationResult;
use async_trait::async_trait;
use coins::my_tx_history_v2::TxHistoryStorage;
use coins::utxo::bch::CashAddrPrefix;
use coins::utxo::bch::{BchActivationRequest, BchCoin};
use coins::utxo::utxo_builder::{UtxoArcBuilder, UtxoCoinBuilder};
use coins::CoinProtocol;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_event_stream::StreamingManager;
use mm2_metrics::MetricsArc;
use mm2_number::BigDecimal;
use serde_json::Value as Json;
use std::collections::HashMap;
use std::str::FromStr;

pub type BchTaskManagerShared = InitStandaloneCoinTaskManagerShared<BchCoin>;
pub type BchRpcTaskHandleShared = InitStandaloneCoinTaskHandleShared<BchCoin>;

#[derive(Clone)]
pub struct BchProtocolInfo {
    slp_prefix: String,
}

impl TryFromCoinProtocol for BchProtocolInfo {
    fn try_from_coin_protocol(proto: CoinProtocol) -> Result<Self, MmError<CoinProtocol>>
    where
        Self: Sized,
    {
        match proto {
            CoinProtocol::BCH { slp_prefix } => Ok(BchProtocolInfo { slp_prefix }),
            protocol => MmError::err(protocol),
        }
    }
}

#[async_trait]
impl InitStandaloneCoinActivationOps for BchCoin {
    type ActivationRequest = BchActivationRequest;
    type StandaloneProtocol = BchProtocolInfo;
    type ActivationResult = UtxoStandardActivationResult;
    type ActivationError = InitUtxoStandardError;
    type InProgressStatus = UtxoStandardInProgressStatus;
    type AwaitingStatus = UtxoStandardAwaitingStatus;
    type UserAction = UtxoStandardUserAction;

    fn rpc_task_manager(activation_ctx: &CoinsActivationContext) -> &BchTaskManagerShared {
        &activation_ctx.init_bch_task_manager
    }

    async fn init_standalone_coin(
        ctx: MmArc,
        ticker: String,
        coin_conf: Json,
        activation_request: &Self::ActivationRequest,
        protocol_info: Self::StandaloneProtocol,
        _task_handle: BchRpcTaskHandleShared,
    ) -> Result<Self, MmError<Self::ActivationError>> {
        if activation_request.bchd_urls.is_empty() && !activation_request.allow_slp_unsafe_conf {
            Err(InitUtxoStandardError::CoinCreationError {
                ticker: ticker.clone(),
                error: "Using empty bchd_urls is unsafe for SLP users!".into(),
            })?;
        }
        let prefix = CashAddrPrefix::from_str(&protocol_info.slp_prefix).map_err(|e| {
            InitUtxoStandardError::CoinCreationError {
                ticker: ticker.clone(),
                error: format!("Couldn't parse cash address prefix: {e:?}"),
            }
        })?;
        let priv_key_policy =
            priv_key_build_policy(&ctx, &activation_request.utxo_params.priv_key_policy).map_mm_err()?;

        let bchd_urls = activation_request.bchd_urls.clone();
        let constructor = { move |utxo_arc| BchCoin::new(utxo_arc, prefix.clone(), bchd_urls.clone()) };

        let coin = UtxoArcBuilder::new(
            &ctx,
            &ticker,
            &coin_conf,
            &activation_request.utxo_params,
            priv_key_policy,
            constructor,
        )
        .build()
        .await
        .mm_err(|e| InitUtxoStandardError::from_build_err(e, ticker.clone()))?;

        Ok(coin)
    }

    async fn get_activation_result(
        &self,
        ctx: MmArc,
        task_handle: BchRpcTaskHandleShared,
        activation_request: &Self::ActivationRequest,
    ) -> MmResult<Self::ActivationResult, InitUtxoStandardError> {
        get_activation_result(&ctx, self, task_handle, &activation_request.utxo_params).await
    }

    fn start_history_background_fetching(
        &self,
        metrics: MetricsArc,
        storage: impl TxHistoryStorage,
        streaming_manager: StreamingManager,
        current_balances: HashMap<String, BigDecimal>,
    ) {
        start_history_background_fetching(self.clone(), metrics, storage, streaming_manager, current_balances)
    }
}
