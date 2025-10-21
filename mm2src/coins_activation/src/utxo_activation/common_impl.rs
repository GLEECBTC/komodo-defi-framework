use crate::standalone_coin::{InitStandaloneCoinActivationOps, InitStandaloneCoinTaskHandleShared};
use crate::utxo_activation::init_utxo_standard_activation_error::InitUtxoStandardError;
use crate::utxo_activation::init_utxo_standard_statuses::{
    UtxoStandardAwaitingStatus, UtxoStandardInProgressStatus, UtxoStandardUserAction,
};
use crate::utxo_activation::utxo_standard_activation_result::UtxoStandardActivationResult;
use coins::coin_balance::EnableCoinBalanceOps;
use coins::hd_wallet::RpcTaskXPubExtractor;
use coins::my_tx_history_v2::TxHistoryStorage;
use coins::utxo::utxo_tx_history_v2::{utxo_history_loop, UtxoTxHistoryOps};
use coins::utxo::{UtxoActivationParams, UtxoCoinFields};
use coins::{CoinBalanceMap, MarketCoinOps, PrivKeyActivationPolicy, PrivKeyBuildPolicy};
use common::executor::{AbortSettings, SpawnAbortable};
use crypto::hw_rpc_task::HwConnectStatuses;
use crypto::{CryptoCtxError, HwRpcError};
use futures::compat::Future01CompatExt;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_event_stream::StreamingManager;
use mm2_metrics::MetricsArc;
use mm2_number::BigDecimal;
use std::collections::HashMap;

pub(crate) async fn get_activation_result<Coin>(
    ctx: &MmArc,
    coin: &Coin,
    task_handle: InitStandaloneCoinTaskHandleShared<Coin>,
    activation_params: &UtxoActivationParams,
) -> MmResult<UtxoStandardActivationResult, InitUtxoStandardError>
where
    Coin: InitStandaloneCoinActivationOps<
            ActivationError = InitUtxoStandardError,
            InProgressStatus = UtxoStandardInProgressStatus,
            AwaitingStatus = UtxoStandardAwaitingStatus,
            UserAction = UtxoStandardUserAction,
        > + EnableCoinBalanceOps<BalanceObject = CoinBalanceMap>
        + MarketCoinOps,
{
    let ticker = coin.ticker().to_owned();
    let current_block = coin
        .current_block()
        .compat()
        .await
        .map_to_mm(InitUtxoStandardError::Transport)?;

    let xpub_extractor = if coin.is_trezor() {
        Some(
            RpcTaskXPubExtractor::new_trezor_extractor(
                ctx,
                task_handle.clone(),
                xpub_extractor_rpc_statuses(),
                // Note that the actual UtxoProtocolInfo isn't needed by trezor XPUB extractor.
                coins::CoinProtocol::UTXO(Default::default()),
            )
            .mm_err(|_| InitUtxoStandardError::HwError(HwRpcError::NotInitialized))?,
        )
    } else {
        None
    };
    task_handle
        .update_in_progress_status(UtxoStandardInProgressStatus::RequestingWalletBalance)
        .map_mm_err()?;
    let wallet_balance = coin
        .enable_coin_balance(
            xpub_extractor,
            activation_params.enable_params.clone(),
            &activation_params.path_to_address,
        )
        .await
        .mm_err(|enable_err| InitUtxoStandardError::from_enable_coin_balance_err(enable_err, ticker.clone()))?;
    task_handle
        .update_in_progress_status(UtxoStandardInProgressStatus::ActivatingCoin)
        .map_mm_err()?;

    let result = UtxoStandardActivationResult {
        ticker,
        current_block,
        wallet_balance,
    };
    Ok(result)
}

fn xpub_extractor_rpc_statuses() -> HwConnectStatuses<UtxoStandardInProgressStatus, UtxoStandardAwaitingStatus> {
    HwConnectStatuses {
        on_connect: UtxoStandardInProgressStatus::WaitingForTrezorToConnect,
        on_connected: UtxoStandardInProgressStatus::ActivatingCoin,
        on_connection_failed: UtxoStandardInProgressStatus::Finishing,
        on_button_request: UtxoStandardInProgressStatus::FollowHwDeviceInstructions,
        on_pin_request: UtxoStandardAwaitingStatus::EnterTrezorPin,
        on_passphrase_request: UtxoStandardAwaitingStatus::EnterTrezorPassphrase,
        on_ready: UtxoStandardInProgressStatus::ActivatingCoin,
    }
}

pub(crate) fn priv_key_build_policy(
    ctx: &MmArc,
    activation_policy: &PrivKeyActivationPolicy,
) -> MmResult<PrivKeyBuildPolicy, CryptoCtxError> {
    match activation_policy {
        PrivKeyActivationPolicy::ContextPrivKey => PrivKeyBuildPolicy::detect_priv_key_policy(ctx),
        PrivKeyActivationPolicy::Trezor => Ok(PrivKeyBuildPolicy::Trezor),
        PrivKeyActivationPolicy::WalletConnect { session_topic } => Ok(PrivKeyBuildPolicy::WalletConnect {
            session_topic: session_topic.clone(),
        }),
    }
}

pub(crate) fn start_history_background_fetching<Coin>(
    coin: Coin,
    metrics: MetricsArc,
    storage: impl TxHistoryStorage,
    streaming_manager: StreamingManager,
    current_balances: HashMap<String, BigDecimal>,
) where
    Coin: AsRef<UtxoCoinFields> + UtxoTxHistoryOps,
{
    let spawner = coin.as_ref().abortable_system.weak_spawner();

    let msg = format!("'utxo_history_loop' has been aborted for {}", coin.ticker());
    let fut = utxo_history_loop(coin, storage, metrics, streaming_manager, current_balances);

    let settings = AbortSettings::info_on_abort(msg);
    spawner.spawn_with_settings(fut, settings);
}
