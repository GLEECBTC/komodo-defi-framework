use super::taker_swap::TakerSwapCommand;
use super::{AtomicSwap, TakerSavedSwap, TakerSwap};
use crate::lp_swap::taker_swap::{TakerPaymentSpentData, TakerSavedEvent, TakerSwapEvent};
use crate::lp_swap::{SavedSwap, SavedSwapIo, TransactionIdentifier, MAKER_PAYMENT_SPENT_BY_WATCHER_LOG};
use coins::{
    FoundSwapTxSpend, SearchForSwapTxSpendInput, TransactionEnum, ValidateWatcherSpendInput, WatcherSpendType,
};
use common::log::info;
use common::{now_ms, Future01CompatExt};
use mm2_core::mm_ctx::MmArc;
use rpc::v1::types::{Bytes, H256};
use std::sync::atomic::Ordering;

#[cfg(not(any(test, feature = "run-docker-tests")))]
use super::swap_watcher::{default_watcher_maker_payment_spend_factor, default_watcher_refund_factor};

#[cfg(not(any(test, feature = "run-docker-tests")))]
use common::now_sec;

pub async fn get_command_based_on_maker_or_watcher_activity(
    ctx: &MmArc,
    swap: &TakerSwap,
    mut saved: TakerSavedSwap,
    command: TakerSwapCommand,
) -> Result<TakerSwapCommand, String> {
    #[cfg(not(any(test, feature = "run-docker-tests")))]
    {
        let watcher_refund_time = swap.r().data.started_at
            + (default_watcher_maker_payment_spend_factor() * swap.r().data.lock_duration as f64) as u64;
        if now_sec() < watcher_refund_time {
            return Ok(command);
        }
    }

    match command {
        TakerSwapCommand::Start => Ok(command),
        TakerSwapCommand::Negotiate => Ok(command),
        TakerSwapCommand::SendTakerFee => Ok(command),
        TakerSwapCommand::WaitForMakerPayment => Ok(command),
        TakerSwapCommand::ValidateMakerPayment => Ok(command),
        TakerSwapCommand::SendTakerPayment => Ok(command),
        TakerSwapCommand::WaitForTakerPaymentSpend => match check_taker_payment_spend(swap).await {
            Ok(Some(FoundSwapTxSpend::Spent(taker_payment_spend_tx))) => {
                add_taker_payment_spent_event(swap, &mut saved, &taker_payment_spend_tx).await?;
                check_maker_payment_spend_and_add_event(ctx, swap, saved).await
            },
            Ok(Some(FoundSwapTxSpend::Refunded(taker_payment_refund_tx))) => {
                add_taker_payment_refunded_by_watcher_event(ctx, swap, saved, taker_payment_refund_tx).await
            },
            Ok(None) => Ok(command),
            Err(e) => ERR!("Error {} when trying to find taker payment spend", e),
        },
        TakerSwapCommand::SpendMakerPayment => check_maker_payment_spend_and_add_event(ctx, swap, saved).await,
        TakerSwapCommand::ConfirmMakerPaymentSpend => Ok(command),
        TakerSwapCommand::PrepareForTakerPaymentRefund | TakerSwapCommand::RefundTakerPayment => {
            #[cfg(not(any(test, feature = "run-docker-tests")))]
            {
                let watcher_refund_time = swap.r().data.started_at
                    + (default_watcher_refund_factor() * swap.r().data.lock_duration as f64) as u64;
                if now_sec() < watcher_refund_time {
                    return Ok(command);
                }
            }

            match check_taker_payment_spend(swap).await {
                Ok(Some(FoundSwapTxSpend::Spent(_))) => ERR!("Taker payment is not expected to be spent at this point"),
                Ok(Some(FoundSwapTxSpend::Refunded(taker_payment_refund_tx))) => {
                    add_taker_payment_refunded_by_watcher_event(ctx, swap, saved, taker_payment_refund_tx).await
                },
                Ok(None) => Ok(command),
                Err(e) => ERR!("Error {} when trying to find taker payment spend", e),
            }
        },
        TakerSwapCommand::FinalizeTakerPaymentRefund => Ok(command),
        TakerSwapCommand::Finish => Ok(command),
    }
}

pub async fn check_maker_payment_spend_and_add_event(
    ctx: &MmArc,
    swap: &TakerSwap,
    mut saved: TakerSavedSwap,
) -> Result<TakerSwapCommand, String> {
    let other_maker_coin_htlc_pub = swap.r().other_maker_coin_htlc_pub;
    let secret_hash = swap.r().secret_hash.0.clone();
    let maker_coin_start_block = swap.r().data.maker_coin_start_block;
    let maker_coin_swap_contract_address = swap.r().data.maker_coin_swap_contract_address.clone();

    let maker_payment = match &swap.r().maker_payment {
        Some(tx) => tx.tx_hex.0.clone(),
        None => return ERR!("No info about maker payment, swap is not recoverable"),
    };
    let unique_data = swap.unique_swap_data();
    let watcher_reward = swap.r().watcher_reward;

    let maker_payment_spend_tx = match swap.maker_coin
        .search_for_swap_tx_spend_other(SearchForSwapTxSpendInput {
            time_lock: swap.maker_payment_lock.load(Ordering::Relaxed),
            other_pub: other_maker_coin_htlc_pub.as_slice(),
            secret_hash: &secret_hash,
            tx: &maker_payment,
            search_from_block: maker_coin_start_block,
            swap_contract_address: &maker_coin_swap_contract_address,
            swap_unique_data: &unique_data,
            watcher_reward,
        })
        .await {
            Ok(Some(FoundSwapTxSpend::Spent(maker_payment_spend_tx))) => maker_payment_spend_tx,
            Ok(Some(FoundSwapTxSpend::Refunded(maker_payment_refund_tx))) => return ERR!("Maker has cheated by both spending the taker payment, and refunding the maker payment with transaction {:#?}", maker_payment_refund_tx.tx_hash_as_bytes()),
            Ok(None) => return Ok(TakerSwapCommand::SpendMakerPayment),
            Err(e) => return ERR!("Error {} when trying to find maker payment spend", e)
        };

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend_tx.tx_hex(),
        maker_pub: other_maker_coin_htlc_pub.to_vec(),
        swap_contract_address: maker_coin_swap_contract_address,
        time_lock: swap.maker_payment_lock.load(Ordering::Relaxed),
        secret_hash: secret_hash.clone(),
        amount: swap.maker_amount.to_decimal(),
        watcher_reward: None,
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };
    swap.maker_coin
        .taker_validates_payment_spend_or_refund(validate_input)
        .compat()
        .await
        .map_err(|e| e.to_string())?;

    let tx_hash = maker_payment_spend_tx.tx_hash_as_bytes();
    info!("Watcher maker payment spend tx {:02x}", tx_hash);
    let tx_ident = TransactionIdentifier {
        tx_hex: Bytes::from(maker_payment_spend_tx.tx_hex()),
        tx_hash,
    };

    let event = TakerSwapEvent::MakerPaymentSpentByWatcher(tx_ident);
    let to_save = TakerSavedEvent {
        timestamp: now_ms(),
        event,
    };
    swap.apply_event(to_save.event.clone());
    saved.events.push(to_save);
    let new_swap = SavedSwap::Taker(saved);
    try_s!(new_swap.save_to_db(ctx).await);
    info!("{}", MAKER_PAYMENT_SPENT_BY_WATCHER_LOG);
    Ok(TakerSwapCommand::ConfirmMakerPaymentSpend)
}

pub async fn check_taker_payment_spend(swap: &TakerSwap) -> Result<Option<FoundSwapTxSpend>, String> {
    let taker_payment = match &swap.r().taker_payment {
        Some(tx) => tx.tx_hex.0.clone(),
        None => return ERR!("No info about taker payment, swap is not recoverable"),
    };

    let other_taker_coin_htlc_pub = swap.r().other_taker_coin_htlc_pub;
    let taker_coin_start_block = swap.r().data.taker_coin_start_block;
    let taker_coin_swap_contract_address = swap.r().data.taker_coin_swap_contract_address.clone();
    let taker_payment_lock = swap.r().data.taker_payment_lock;
    let secret_hash = swap.r().secret_hash.0.clone();
    let unique_data = swap.unique_swap_data();
    let watcher_reward = swap.r().watcher_reward;

    swap.taker_coin
        .search_for_swap_tx_spend_my(SearchForSwapTxSpendInput {
            time_lock: taker_payment_lock,
            other_pub: other_taker_coin_htlc_pub.as_slice(),
            secret_hash: &secret_hash,
            tx: &taker_payment,
            search_from_block: taker_coin_start_block,
            swap_contract_address: &taker_coin_swap_contract_address,
            swap_unique_data: &unique_data,
            watcher_reward,
        })
        .await
}

pub async fn add_taker_payment_spent_event(
    swap: &TakerSwap,
    saved: &mut TakerSavedSwap,
    taker_payment_spend_tx: &TransactionEnum,
) -> Result<(), String> {
    let secret_hash = swap.r().secret_hash.0.clone();
    let watcher_reward = swap.r().watcher_reward;

    let tx_hash = taker_payment_spend_tx.tx_hash_as_bytes();
    info!("Taker payment spend tx {:02x}", tx_hash);
    let tx_ident = TransactionIdentifier {
        tx_hex: Bytes::from(taker_payment_spend_tx.tx_hex()),
        tx_hash,
    };
    let secret = match swap
        .taker_coin
        .extract_secret(&secret_hash, &tx_ident.tx_hex, watcher_reward)
        .await
    {
        Ok(secret) => H256::from(secret),
        Err(_) => {
            return ERR!("Could not extract secret from taker payment spend transaction");
        },
    };

    let event = TakerSwapEvent::TakerPaymentSpent(TakerPaymentSpentData {
        transaction: tx_ident,
        secret,
    });
    let to_save = TakerSavedEvent {
        timestamp: now_ms(),
        event,
    };
    swap.apply_event(to_save.event.clone());
    saved.events.push(to_save);
    Ok(())
}

pub async fn add_taker_payment_refunded_by_watcher_event(
    ctx: &MmArc,
    swap: &TakerSwap,
    mut saved: TakerSavedSwap,
    taker_payment_refund_tx: TransactionEnum,
) -> Result<TakerSwapCommand, String> {
    let other_maker_coin_htlc_pub = swap.r().other_maker_coin_htlc_pub;
    let taker_coin_swap_contract_address = swap.r().data.taker_coin_swap_contract_address.clone();
    let taker_payment_lock = swap.r().data.taker_payment_lock;
    let secret_hash = swap.r().secret_hash.0.clone();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund_tx.tx_hex(),
        maker_pub: other_maker_coin_htlc_pub.to_vec(),
        swap_contract_address: taker_coin_swap_contract_address,
        time_lock: taker_payment_lock,
        secret_hash: secret_hash.clone(),
        amount: swap.taker_amount.to_decimal(),
        watcher_reward: None,
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    swap.taker_coin
        .taker_validates_payment_spend_or_refund(validate_input)
        .compat()
        .await
        .map_err(|e| e.to_string())?;

    let tx_hash = taker_payment_refund_tx.tx_hash_as_bytes();
    info!("Taker refund tx hash {:02x}", tx_hash);
    let tx_ident = TransactionIdentifier {
        tx_hex: Bytes::from(taker_payment_refund_tx.tx_hex()),
        tx_hash,
    };

    let event = TakerSwapEvent::TakerPaymentRefundedByWatcher(Some(tx_ident));
    let to_save = TakerSavedEvent {
        timestamp: now_ms(),
        event,
    };
    swap.apply_event(to_save.event.clone());
    saved.events.push(to_save);

    let new_swap = SavedSwap::Taker(saved);
    try_s!(new_swap.save_to_db(ctx).await);
    info!("Taker payment is refunded by the watcher");
    Ok(TakerSwapCommand::Finish)
}
