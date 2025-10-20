use crate::lp_swap::maker_swap::{MakerSwapData, MakerSwapEvent, TakerNegotiationData, MAKER_ERROR_EVENTS,
                                 MAKER_SUCCESS_EVENTS};
use crate::lp_swap::taker_swap::{MakerNegotiationData, TakerPaymentSpentData, TakerSavedEvent, TakerSwapData,
                                 TakerSwapEvent, TAKER_ERROR_EVENTS, TAKER_SUCCESS_EVENTS};
use crate::lp_swap::{wait_for_maker_payment_conf_until, MakerSavedEvent, MakerSavedSwap, SavedSwap, SwapError,
                     TakerSavedSwap};
use coins::{lp_coinfind, MmCoinEnum};
use common::{HttpStatusCode, StatusCode};
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use rpc::v1::types::{Bytes as BytesJson, H256 as H256Json};

pub type RecreateSwapResult<T> = Result<T, MmError<RecreateSwapError>>;

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum RecreateSwapError {
    #[display(fmt = "Swap hasn't been started. Swap not recoverable")]
    SwapIsNotStarted,
    #[display(fmt = "Swap hasn't been negotiated. Swap not recoverable")]
    SwapIsNotNegotiated,
    #[display(fmt = "Expected '{}' event, found '{}'", expected, found)]
    UnexpectedEvent { expected: String, found: String },
    #[display(fmt = "No such coin {}", coin)]
    NoSuchCoin { coin: String },
    #[display(fmt = "'secret_hash' not found in swap data")]
    NoSecretHash,
    #[display(fmt = "Internal error: {}", _0)]
    Internal(String),
}

impl HttpStatusCode for RecreateSwapError {
    fn status_code(&self) -> StatusCode { StatusCode::BAD_REQUEST }
}

impl RecreateSwapError {
    fn unexpected_event(found: String, expected: &str) -> RecreateSwapError {
        RecreateSwapError::UnexpectedEvent {
            expected: expected.to_owned(),
            found,
        }
    }
}

/// The input swap can be either tagged by `type` or not.
#[derive(Deserialize)]
#[serde(untagged)]
#[allow(clippy::enum_variant_names)]
pub enum InputSwap {
    SavedSwap(SavedSwap),
    MakerSavedSwap(MakerSavedSwap),
    TakerSavedSwap(TakerSavedSwap),
}

#[derive(Deserialize)]
pub struct RecreateSwapRequest {
    swap: InputSwap,
}

#[derive(Serialize)]
pub struct RecreateSwapResponse {
    swap: SavedSwap,
}

pub async fn recreate_swap_data(ctx: MmArc, args: RecreateSwapRequest) -> RecreateSwapResult<RecreateSwapResponse> {
    match args.swap {
        InputSwap::SavedSwap(SavedSwap::Maker(maker_swap)) | InputSwap::MakerSavedSwap(maker_swap) => {
            recreate_taker_swap(ctx, maker_swap)
                .await
                .map(SavedSwap::from)
                .map(|swap| RecreateSwapResponse { swap })
        },
        InputSwap::SavedSwap(SavedSwap::Taker(taker_swap)) | InputSwap::TakerSavedSwap(taker_swap) => {
            recreate_maker_swap(ctx, taker_swap)
                .await
                .map(SavedSwap::from)
                .map(|swap| RecreateSwapResponse { swap })
        },
    }
}

async fn recreate_maker_swap(ctx: MmArc, taker_swap: TakerSavedSwap) -> RecreateSwapResult<MakerSavedSwap> {
    let mut maker_swap = MakerSavedSwap {
        uuid: taker_swap.uuid,
        #[cfg(all(not(target_arch = "wasm32"), feature = "new-db-arch"))]
        maker_address: String::new(),
        my_order_uuid: taker_swap.my_order_uuid,
        events: Vec::new(),
        maker_amount: taker_swap.maker_amount,
        maker_coin: taker_swap.maker_coin,
        maker_coin_usd_price: taker_swap.maker_coin_usd_price,
        taker_amount: taker_swap.taker_amount,
        taker_coin: taker_swap.taker_coin,
        taker_coin_usd_price: taker_swap.taker_coin_usd_price,
        gui: ctx.gui().map(|s| s.to_owned()),
        mm_version: Some(ctx.mm_version.clone()),
        success_events: MAKER_SUCCESS_EVENTS.iter().map(|event| event.to_string()).collect(),
        error_events: MAKER_ERROR_EVENTS.iter().map(|event| event.to_string()).collect(),
    };

    let mut event_it = taker_swap.events.into_iter();

    let (started_event_timestamp, started_event) = {
        let TakerSavedEvent { event, timestamp } = event_it.next().or_mm_err(|| RecreateSwapError::SwapIsNotStarted)?;
        match event {
            TakerSwapEvent::Started(started) => (timestamp, started),
            event => return MmError::err(RecreateSwapError::unexpected_event(event.status_str(), "Started")),
        }
    };

    let (negotiated_event_timestamp, negotiated_event) = {
        let TakerSavedEvent { event, timestamp } =
            event_it.next().or_mm_err(|| RecreateSwapError::SwapIsNotNegotiated)?;
        match event {
            TakerSwapEvent::Negotiated(negotiated) => (timestamp, negotiated),
            event => return MmError::err(RecreateSwapError::unexpected_event(event.status_str(), "Negotiated")),
        }
    };

    // Generate `Started` event

    let mut taker_p2p_pubkey = [0; 32];
    taker_p2p_pubkey.copy_from_slice(&started_event.my_persistent_pub.0[1..33]);

    let maker_started_event = MakerSwapEvent::Started(MakerSwapData {
        taker_coin: started_event.taker_coin,
        maker_coin: started_event.maker_coin.clone(),
        taker_pubkey: H256Json::from(taker_p2p_pubkey),
        // We could parse the `TakerSwapEvent::TakerPaymentSpent` event.
        // As for now, don't try to find the secret in the events since we can refund without it.
        secret: H256Json::default(),
        secret_hash: Some(negotiated_event.secret_hash),
        my_persistent_pub: negotiated_event.maker_pubkey,
        lock_duration: started_event.lock_duration,
        maker_amount: started_event.maker_amount,
        taker_amount: started_event.taker_amount,
        maker_payment_confirmations: started_event.maker_payment_confirmations,
        maker_payment_requires_nota: started_event.maker_payment_requires_nota,
        taker_payment_confirmations: started_event.taker_payment_confirmations,
        taker_payment_requires_nota: started_event.taker_payment_requires_nota,
        maker_payment_lock: negotiated_event.maker_payment_locktime,
        uuid: started_event.uuid,
        started_at: started_event.started_at,
        maker_coin_start_block: started_event.maker_coin_start_block,
        taker_coin_start_block: started_event.taker_coin_start_block,
        // Don't set the fee since the value is used when we calculate locked by other swaps amount only.
        maker_payment_trade_fee: None,
        // Don't set the fee since the value is used when we calculate locked by other swaps amount only.
        taker_payment_spend_trade_fee: None,
        maker_coin_swap_contract_address: negotiated_event.maker_coin_swap_contract_addr.clone(),
        taker_coin_swap_contract_address: negotiated_event.taker_coin_swap_contract_addr.clone(),
        maker_coin_htlc_pubkey: negotiated_event.maker_coin_htlc_pubkey,
        taker_coin_htlc_pubkey: negotiated_event.taker_coin_htlc_pubkey,
        p2p_privkey: None,
    });
    maker_swap.events.push(MakerSavedEvent {
        timestamp: started_event_timestamp,
        event: maker_started_event,
    });

    // Generate `Negotiated` event

    let maker_negotiated_event = MakerSwapEvent::Negotiated(TakerNegotiationData {
        taker_payment_locktime: started_event.taker_payment_lock,
        taker_pubkey: started_event.my_persistent_pub,
        maker_coin_swap_contract_addr: negotiated_event.maker_coin_swap_contract_addr,
        taker_coin_swap_contract_addr: negotiated_event.taker_coin_swap_contract_addr,
        maker_coin_htlc_pubkey: started_event.maker_coin_htlc_pubkey,
        taker_coin_htlc_pubkey: started_event.taker_coin_htlc_pubkey,
    });
    maker_swap.events.push(MakerSavedEvent {
        timestamp: negotiated_event_timestamp,
        event: maker_negotiated_event,
    });

    // Then we can continue to process success Taker events.
    let wait_refund_until = negotiated_event.maker_payment_locktime + 3700;
    maker_swap
        .events
        .extend(convert_taker_to_maker_events(event_it, wait_refund_until));

    #[cfg(all(not(target_arch = "wasm32"), feature = "new-db-arch"))]
    {
        // TODO(new-db-arch): Execute this plan: https://github.com/KomodoPlatform/komodo-defi-framework/pull/2398#discussion_r2036035916
        //                    instead of making the maker_address/address_dir available for the importer (i.e. let them find it themselves).
        let maker_coin_ticker = started_event.maker_coin;
        let maker_coin = lp_coinfind(&ctx, &maker_coin_ticker)
            .await
            .map_to_mm(RecreateSwapError::Internal)?
            .or_mm_err(move || RecreateSwapError::NoSuchCoin {
                coin: maker_coin_ticker,
            })?;
        maker_swap.maker_address = negotiated_event
            .maker_coin_htlc_pubkey
            .and_then(|pubkey| maker_coin.address_from_pubkey(&pubkey).ok())
            .unwrap_or("Couldn't get the maker coin address. Please set it manually.".to_string());
    }

    Ok(maker_swap)
}

/// Converts `TakerSwapEvent` to `MakerSwapEvent`.
/// Please note that this method ignores the [`TakerSwapEvent::Started`] and [`TakerSwapEvent::Negotiated`] events
/// because they are used outside of this function to generate `MakerSwap` and the initial [`MakerSwapEvent::Started`] and [`MakerSwapEvent::Negotiated`] events.
fn convert_taker_to_maker_events(
    event_it: impl Iterator<Item = TakerSavedEvent>,
    wait_refund_until: u64,
) -> Vec<MakerSavedEvent> {
    let mut events = Vec::new();
    let mut taker_fee_ident = None;
    for TakerSavedEvent { event, timestamp } in event_it {
        macro_rules! push_event {
            ($maker_event:expr) => {
                events.push(MakerSavedEvent {
                    timestamp,
                    event: $maker_event,
                })
            };
        }

        // This is used only if an error occurs.
        let swap_error = SwapError {
            error: format!("Origin Taker error event: {:?}", event),
        };
        match event {
            // Even if we considered Taker fee as invalid, then we shouldn't have sent Maker payment.
            TakerSwapEvent::TakerFeeSent(tx_ident) => taker_fee_ident = Some(tx_ident),
            TakerSwapEvent::TakerFeeSendFailed(_) => {
                // Maker shouldn't send MakerPayment in this case.
                push_event!(MakerSwapEvent::TakerFeeValidateFailed(swap_error));
                push_event!(MakerSwapEvent::Finished);
                // Finish processing Taker events.
                return events;
            },
            TakerSwapEvent::MakerPaymentReceived(tx_ident) => {
                if let Some(taker_fee_ident) = taker_fee_ident.take() {
                    push_event!(MakerSwapEvent::TakerFeeValidated(taker_fee_ident));
                }
                push_event!(MakerSwapEvent::MakerPaymentSent(tx_ident))
            },
            TakerSwapEvent::MakerPaymentValidateFailed(_)
            | TakerSwapEvent::MakerPaymentWaitConfirmFailed(_)
            | TakerSwapEvent::TakerPaymentTransactionFailed(_)
            | TakerSwapEvent::TakerPaymentDataSendFailed(_)
            // We actually could confirm TakerPayment and spend it, but for now we don't know about the transaction.
            | TakerSwapEvent::TakerPaymentWaitConfirmFailed(_) => {
                // Maker shouldn't receive an info about TakerPayment.
                push_event!(MakerSwapEvent::TakerPaymentValidateFailed(swap_error));
                push_event!(MakerSwapEvent::MakerPaymentWaitRefundStarted {
                    wait_until: wait_refund_until
                });
                // Finish processing Taker events.
                return events;
            },
            TakerSwapEvent::TakerPaymentSent(tx_ident) => {
                push_event!(MakerSwapEvent::TakerPaymentReceived(tx_ident));
                // Please note we have not to push `TakerPaymentValidatedAndConfirmed` since we could actually decline it.
                push_event!(MakerSwapEvent::TakerPaymentWaitConfirmStarted);
            },
            TakerSwapEvent::TakerPaymentSpent(payment_spent_data) => {
                push_event!(MakerSwapEvent::TakerPaymentValidatedAndConfirmed);
                push_event!(MakerSwapEvent::TakerPaymentSpent(payment_spent_data.transaction));
                push_event!(MakerSwapEvent::TakerPaymentSpendConfirmStarted);
                // We can consider the spent transaction validated and confirmed since the taker found it on the blockchain.
                push_event!(MakerSwapEvent::TakerPaymentSpendConfirmed);
                push_event!(MakerSwapEvent::Finished);
                // Finish the function, because we spent TakerPayment
                return events;
            },
            TakerSwapEvent::TakerPaymentWaitForSpendFailed(_) => {
                // Maker hasn't spent TakerPayment for some reason.
                push_event!(MakerSwapEvent::TakerPaymentSpendFailed(swap_error));
                push_event!(MakerSwapEvent::MakerPaymentWaitRefundStarted {
                    wait_until: wait_refund_until
                });
                // Finish processing Taker events.
                return events;
            },
            TakerSwapEvent::Started(_)
            | TakerSwapEvent::StartFailed(_)
            | TakerSwapEvent::Negotiated(_)
            | TakerSwapEvent::NegotiateFailed(_)
            | TakerSwapEvent::TakerPaymentInstructionsReceived(_)
            | TakerSwapEvent::MakerPaymentWaitConfirmStarted
            | TakerSwapEvent::MakerPaymentValidatedAndConfirmed
            | TakerSwapEvent::MakerPaymentSpent(_)
            | TakerSwapEvent::MakerPaymentSpendConfirmed
            | TakerSwapEvent::MakerPaymentSpendConfirmFailed(_)
            | TakerSwapEvent::MakerPaymentSpentByWatcher(_)
            | TakerSwapEvent::MakerPaymentSpendFailed(_)
            // We don't know the reason at the moment, so we rely on the errors handling above.
            | TakerSwapEvent::WatcherMessageSent(_,_)
            | TakerSwapEvent::TakerPaymentWaitRefundStarted { .. }
            | TakerSwapEvent::TakerPaymentRefundStarted
            | TakerSwapEvent::TakerPaymentRefunded(_)
            | TakerSwapEvent::TakerPaymentRefundFailed(_)
            | TakerSwapEvent::TakerPaymentRefundFinished
            | TakerSwapEvent::TakerPaymentRefundedByWatcher(_)
            | TakerSwapEvent::Finished => {}
        }
    }
    events
}

async fn recreate_taker_swap(ctx: MmArc, maker_swap: MakerSavedSwap) -> RecreateSwapResult<TakerSavedSwap> {
    let mut taker_swap = TakerSavedSwap {
        uuid: maker_swap.uuid,
        #[cfg(all(not(target_arch = "wasm32"), feature = "new-db-arch"))]
        maker_address: String::new(),
        my_order_uuid: Some(maker_swap.uuid),
        events: Vec::new(),
        maker_amount: maker_swap.maker_amount,
        maker_coin: maker_swap.maker_coin,
        maker_coin_usd_price: maker_swap.maker_coin_usd_price,
        taker_amount: maker_swap.taker_amount,
        taker_coin: maker_swap.taker_coin,
        taker_coin_usd_price: maker_swap.taker_coin_usd_price,
        gui: ctx.gui().map(|s| s.to_owned()),
        mm_version: Some(ctx.mm_version.clone()),
        success_events: TAKER_SUCCESS_EVENTS.iter().map(|event| event.to_string()).collect(),
        error_events: TAKER_ERROR_EVENTS.iter().map(|event| event.to_string()).collect(),
    };

    let mut event_it = maker_swap.events.into_iter();

    let (started_event_timestamp, started_event) = {
        let MakerSavedEvent { event, timestamp } = event_it.next().or_mm_err(|| RecreateSwapError::SwapIsNotStarted)?;
        match event {
            MakerSwapEvent::Started(started) => (timestamp, started),
            event => return MmError::err(RecreateSwapError::unexpected_event(event.status_str(), "Started")),
        }
    };

    let (negotiated_timestamp, negotiated_event) = {
        let MakerSavedEvent { event, timestamp } =
            event_it.next().or_mm_err(|| RecreateSwapError::SwapIsNotNegotiated)?;
        match event {
            MakerSwapEvent::Negotiated(negotiated) => (timestamp, negotiated),
            event => return MmError::err(RecreateSwapError::unexpected_event(event.status_str(), "Negotiated")),
        }
    };

    let mut maker_p2p_pubkey = [0; 32];
    maker_p2p_pubkey.copy_from_slice(&started_event.my_persistent_pub.0[1..33]);
    let taker_started_event = TakerSwapEvent::Started(TakerSwapData {
        taker_coin: started_event.taker_coin,
        maker_coin: started_event.maker_coin.clone(),
        maker_pubkey: H256Json::from(maker_p2p_pubkey),
        my_persistent_pub: negotiated_event.taker_pubkey,
        lock_duration: started_event.lock_duration,
        maker_amount: started_event.maker_amount,
        taker_amount: started_event.taker_amount,
        maker_payment_confirmations: started_event.maker_payment_confirmations,
        maker_payment_requires_nota: started_event.maker_payment_requires_nota,
        taker_payment_confirmations: started_event.taker_payment_confirmations,
        taker_payment_requires_nota: started_event.taker_payment_requires_nota,
        taker_payment_lock: negotiated_event.taker_payment_locktime,
        uuid: started_event.uuid,
        started_at: started_event.started_at,
        maker_payment_wait: wait_for_maker_payment_conf_until(started_event.started_at, started_event.lock_duration),
        maker_coin_start_block: started_event.maker_coin_start_block,
        taker_coin_start_block: started_event.taker_coin_start_block,
        // Don't set the fee since the value is used when we calculate locked by other swaps amount only.
        fee_to_send_taker_fee: None,
        // Don't set the fee since the value is used when we calculate locked by other swaps amount only.
        taker_payment_trade_fee: None,
        // Don't set the fee since the value is used when we calculate locked by other swaps amount only.
        maker_payment_spend_trade_fee: None,
        maker_coin_swap_contract_address: negotiated_event.maker_coin_swap_contract_addr.clone(),
        taker_coin_swap_contract_address: negotiated_event.taker_coin_swap_contract_addr.clone(),
        maker_coin_htlc_pubkey: negotiated_event.maker_coin_htlc_pubkey,
        taker_coin_htlc_pubkey: negotiated_event.taker_coin_htlc_pubkey,
        p2p_privkey: None,
    });
    taker_swap.events.push(TakerSavedEvent {
        timestamp: started_event_timestamp,
        event: taker_started_event,
    });

    let secret_hash = started_event
        .secret_hash
        .or_mm_err(|| RecreateSwapError::NoSecretHash)?;

    let taker_negotiated_event = TakerSwapEvent::Negotiated(MakerNegotiationData {
        maker_payment_locktime: started_event.maker_payment_lock,
        maker_pubkey: started_event.my_persistent_pub,
        secret_hash: secret_hash.clone(),
        maker_coin_swap_contract_addr: negotiated_event.maker_coin_swap_contract_addr,
        taker_coin_swap_contract_addr: negotiated_event.taker_coin_swap_contract_addr,
        maker_coin_htlc_pubkey: started_event.maker_coin_htlc_pubkey,
        taker_coin_htlc_pubkey: started_event.taker_coin_htlc_pubkey,
    });
    taker_swap.events.push(TakerSavedEvent {
        timestamp: negotiated_timestamp,
        event: taker_negotiated_event,
    });

    // Can be used to extract a secret from [`MakerSwapEvent::TakerPaymentSpent`].
    let maker_coin_ticker = started_event.maker_coin;
    let maker_coin = lp_coinfind(&ctx, &maker_coin_ticker)
        .await
        .map_to_mm(RecreateSwapError::Internal)?
        .or_mm_err(move || RecreateSwapError::NoSuchCoin {
            coin: maker_coin_ticker,
        })?;

    #[cfg(all(not(target_arch = "wasm32"), feature = "new-db-arch"))]
    {
        taker_swap.maker_address = negotiated_event
            .maker_coin_htlc_pubkey
            .and_then(|pubkey| maker_coin.address_from_pubkey(&pubkey).ok())
            .unwrap_or("Couldn't get the maker coin address. Please set it manually.".to_string());
    }

    // Then we can continue to process success Maker events.
    let wait_refund_until = negotiated_event.taker_payment_locktime + 3700;
    taker_swap
        .events
        .extend(convert_maker_to_taker_events(event_it, maker_coin, secret_hash, wait_refund_until).await);

    Ok(taker_swap)
}

/// Converts `MakerSwapEvent` to `TakerSwapEvent`.
/// Please note that this method ignores the [`MakerSwapEvent::Started`] and [`MakerSwapEvent::Negotiated`] events
/// since they are used outside of this function to generate `TakerSwap` and the initial [`TakerSwapEvent::Started`] and [`TakerSwapEvent::Negotiated`] events.
///
/// The `maker_coin` and `secret_hash` function arguments are used to extract a secret from `TakerPaymentSpent`.
async fn convert_maker_to_taker_events(
    event_it: impl Iterator<Item = MakerSavedEvent>,
    maker_coin: MmCoinEnum,
    secret_hash: BytesJson,
    wait_refund_until: u64,
) -> Vec<TakerSavedEvent> {
    let mut events = Vec::new();
    for MakerSavedEvent { event, timestamp } in event_it {
        macro_rules! push_event {
            ($maker_event:expr) => {
                events.push(TakerSavedEvent {
                    timestamp,
                    event: $maker_event,
                })
            };
        }

        // This is used only if an error occurs.
        let swap_error = SwapError {
            error: format!("Origin Maker error event: {:?}", event),
        };
        match event {
            MakerSwapEvent::TakerFeeValidated(tx_ident) => push_event!(TakerSwapEvent::TakerFeeSent(tx_ident)),
            MakerSwapEvent::TakerFeeValidateFailed(_) => {
                push_event!(TakerSwapEvent::MakerPaymentValidateFailed(swap_error));
                push_event!(TakerSwapEvent::Finished);
                // Finish processing Maker events.
                return events;
            },
            MakerSwapEvent::MakerPaymentSent(tx_ident) => {
                push_event!(TakerSwapEvent::MakerPaymentReceived(tx_ident));
                // Please note we have not to push `MakerPaymentValidatedAndConfirmed` since we could actually decline it.
                push_event!(TakerSwapEvent::MakerPaymentWaitConfirmStarted);
            },
            MakerSwapEvent::MakerPaymentTransactionFailed(_)
            | MakerSwapEvent::MakerPaymentDataSendFailed(_)
            // We actually could confirm MakerPayment and send TakerPayment, but for now we don't know about the transaction.
            | MakerSwapEvent::MakerPaymentWaitConfirmFailed(_) => {
                push_event!(TakerSwapEvent::MakerPaymentValidateFailed(swap_error));
                push_event!(TakerSwapEvent::Finished);
                // Finish processing Maker events.
                return events;
            },
            MakerSwapEvent::TakerPaymentReceived(tx_ident) => {
                push_event!(TakerSwapEvent::MakerPaymentValidatedAndConfirmed);
                push_event!(TakerSwapEvent::TakerPaymentSent(tx_ident.clone()));
            },
            MakerSwapEvent::TakerPaymentValidateFailed(_)
            | MakerSwapEvent::TakerPaymentSpendFailed(_)
            | MakerSwapEvent::TakerPaymentWaitConfirmFailed(_) => {
                push_event!(TakerSwapEvent::TakerPaymentWaitForSpendFailed(swap_error));
                push_event!(TakerSwapEvent::TakerPaymentWaitRefundStarted { wait_until: wait_refund_until });
                // Finish processing Maker events.
                return events;
            },
            MakerSwapEvent::TakerPaymentSpent(tx_ident) => {
                //Is the watcher_reward argument important here?
                let secret = match maker_coin.extract_secret(&secret_hash.0, &tx_ident.tx_hex, false).await {
                    Ok(secret) => H256Json::from(secret),
                    Err(e) => {
                        push_event!(TakerSwapEvent::TakerPaymentWaitForSpendFailed(ERRL!("{}", e).into()));
                        push_event!(TakerSwapEvent::TakerPaymentWaitRefundStarted { wait_until: wait_refund_until });
                        return events;
                    },
                };
                push_event!(TakerSwapEvent::TakerPaymentSpent(TakerPaymentSpentData {
                    secret,
                    transaction: tx_ident,
                }));
                return events;
            },
            MakerSwapEvent::Started(_)
            | MakerSwapEvent::StartFailed(_)
            | MakerSwapEvent::Negotiated(_)
            | MakerSwapEvent::NegotiateFailed(_)
            | MakerSwapEvent::MakerPaymentInstructionsReceived(_)
            | MakerSwapEvent::TakerPaymentWaitConfirmStarted
            | MakerSwapEvent::TakerPaymentValidatedAndConfirmed
            | MakerSwapEvent::TakerPaymentSpendConfirmStarted
            | MakerSwapEvent::TakerPaymentSpendConfirmed
            // I think we can't be sure if the spend transaction is declined by the network.
            | MakerSwapEvent::TakerPaymentSpendConfirmFailed(_)
            // We don't know the reason at the moment, so we rely on the errors handling above.
            | MakerSwapEvent::MakerPaymentWaitRefundStarted { .. }
            | MakerSwapEvent::MakerPaymentRefundStarted
            | MakerSwapEvent::MakerPaymentRefunded(_)
            | MakerSwapEvent::MakerPaymentRefundFailed(_)
            | MakerSwapEvent::MakerPaymentRefundFinished
            | MakerSwapEvent::Finished => {}
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use coins::{CoinsContext, MarketCoinOps, SwapOps, TestCoin};
    use common::block_on;
    use hex::FromHex;
    use mm2_core::mm_ctx::MmCtxBuilder;
    use mocktopus::mocking::{MockResult, Mockable};
    use serde_json as json;

    #[test]
    fn test_recreate_maker_swap() {
        let taker_saved_swap: TakerSavedSwap =
            json::from_str(include_str!("../for_tests/recreate_maker_swap_taker_saved.json")).unwrap();
        let maker_expected_swap: MakerSavedSwap =
            json::from_str(include_str!("../for_tests/recreate_maker_swap_maker_expected.json")).unwrap();

        let ctx = MmCtxBuilder::default().into_mm_arc();

        let maker_actual_swap = block_on(recreate_maker_swap(ctx, taker_saved_swap)).expect("!recreate_maker_swap");
        println!("{}", json::to_string(&maker_actual_swap).unwrap());
        assert_eq!(maker_actual_swap, maker_expected_swap);
    }

    #[test]
    fn test_recreate_maker_swap_maker_payment_wait_confirm_failed() {
        let taker_saved_swap: TakerSavedSwap = json::from_str(include_str!(
            "../for_tests/recreate_maker_swap_maker_payment_wait_confirm_failed_taker_saved.json"
        ))
        .unwrap();
        let maker_expected_swap: MakerSavedSwap = json::from_str(include_str!(
            "../for_tests/recreate_maker_swap_maker_payment_wait_confirm_failed_maker_expected.json"
        ))
        .unwrap();

        let ctx = MmCtxBuilder::default().into_mm_arc();

        let maker_actual_swap = block_on(recreate_maker_swap(ctx, taker_saved_swap)).expect("!recreate_maker_swap");
        println!("{}", json::to_string(&maker_actual_swap).unwrap());
        assert_eq!(maker_actual_swap, maker_expected_swap);
    }

    #[test]
    fn test_recreate_taker_swap() {
        TestCoin::extract_secret.mock_safe(|_coin, _secret_hash, _spend_tx, _watcher_reward| {
            let secret =
                <[u8; 32]>::from_hex("23a6bb64bc0ab2cc14cb84277d8d25134b814e5f999c66e578c9bba3c5e2d3a4").unwrap();
            MockResult::Return(Box::pin(async move { Ok(secret) }))
        });
        TestCoin::platform_ticker.mock_safe(|_| MockResult::Return("TestCoin"));

        let maker_saved_swap: MakerSavedSwap =
            json::from_str(include_str!("../for_tests/recreate_taker_swap_maker_saved.json")).unwrap();
        let taker_expected_swap: TakerSavedSwap =
            json::from_str(include_str!("../for_tests/recreate_taker_swap_taker_expected.json")).unwrap();

        let ctx = MmCtxBuilder::default().into_mm_arc();
        let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
        block_on(coins_ctx.add_token(MmCoinEnum::Test(TestCoin::new("RICK")))).unwrap();

        let taker_actual_swap = block_on(recreate_taker_swap(ctx, maker_saved_swap)).expect("!recreate_maker_swap");
        println!("{}", json::to_string(&taker_actual_swap).unwrap());
        assert_eq!(taker_actual_swap, taker_expected_swap);
    }

    #[test]
    fn test_recreate_taker_swap_taker_payment_wait_confirm_failed() {
        TestCoin::platform_ticker.mock_safe(|_| MockResult::Return("TestCoin"));
        let maker_saved_swap: MakerSavedSwap = json::from_str(include_str!(
            "../for_tests/recreate_taker_swap_taker_payment_wait_confirm_failed_maker_saved.json"
        ))
        .unwrap();
        let taker_expected_swap: TakerSavedSwap = json::from_str(include_str!(
            "../for_tests/recreate_taker_swap_taker_payment_wait_confirm_failed_taker_expected.json"
        ))
        .unwrap();

        let ctx = MmCtxBuilder::default().into_mm_arc();
        let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
        block_on(coins_ctx.add_token(MmCoinEnum::Test(TestCoin::new("RICK")))).unwrap();

        let taker_actual_swap = block_on(recreate_taker_swap(ctx, maker_saved_swap)).expect("!recreate_maker_swap");
        println!("{}", json::to_string(&taker_actual_swap).unwrap());
        assert_eq!(taker_actual_swap, taker_expected_swap);
    }
}
