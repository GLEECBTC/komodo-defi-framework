use super::*;
use crate::lightning::ln_db::{DBChannelDetails, HTLCStatus, LightningDB, PaymentType};
use crate::lightning::ln_errors::{SaveChannelClosingError, SaveChannelClosingResult};
use crate::lightning::ln_sql::SqliteLightningDB;
use bitcoin::blockdata::script::Script;
use bitcoin::blockdata::transaction::Transaction;
use bitcoin::consensus::encode::serialize_hex;
use common::executor::{AbortSettings, SpawnAbortable, SpawnFuture, Timer};
use common::log::{error, info};
use common::{new_uuid, now_sec_i64};
use core::time::Duration;
use derive_more::Display;
use futures::compat::Future01CompatExt;
use lightning::chain::chaininterface::{ConfirmationTarget, FeeEstimator};
use lightning::chain::keysinterface::SpendableOutputDescriptor;
use lightning::util::events::{Event, EventHandler, PaymentPurpose};
use rand::Rng;
use script::{Builder, SignatureVersion};
use secp256k1v24::Secp256k1;
use std::convert::TryInto;
use std::sync::Arc;
use utxo_signer::with_key_pair::sign_tx;

const TRY_LOOP_INTERVAL: f64 = 60.;
/// 1 second.
const CRITICAL_FUTURE_TIMEOUT: f64 = 1.0;
pub const CHANNEL_READY_LOG: &str = "Handling ChannelReady event for channel with uuid";
pub const PAYMENT_CLAIMABLE_LOG: &str = "Handling PaymentClaimable event";
pub const SUCCESSFUL_CLAIM_LOG: &str = "Successfully claimed payment";
pub const SUCCESSFUL_SEND_LOG: &str = "Successfully sent payment";

pub struct LightningEventHandler {
    platform: Arc<Platform>,
    channel_manager: Arc<ChannelManager>,
    keys_manager: Arc<KeysManager>,
    db: SqliteLightningDB,
    trusted_nodes: TrustedNodesShared,
}

impl EventHandler for LightningEventHandler {
    fn handle_event(&self, event: Event) {
        match event {
            Event::FundingGenerationReady {
                temporary_channel_id,
                channel_value_satoshis,
                output_script,
                user_channel_id,
                counterparty_node_id,
            } => self.handle_funding_generation_ready(
                temporary_channel_id,
                channel_value_satoshis,
                output_script,
                user_channel_id,
                counterparty_node_id,
            ),

            Event::PaymentClaimable {
                payment_hash,
                amount_msat,
                purpose,
                ..
            } => self.handle_payment_claimable(payment_hash, amount_msat, purpose),

            Event::PaymentSent {
                payment_preimage,
                payment_hash,
                fee_paid_msat,
                ..
            } => self.handle_payment_sent(payment_preimage, payment_hash, fee_paid_msat),

            Event::PaymentClaimed { payment_hash, amount_msat, .. } => self.handle_payment_claimed(payment_hash, amount_msat),

            Event::PaymentFailed { payment_hash, .. } => self.handle_payment_failed(payment_hash),

            Event::PendingHTLCsForwardable { time_forwardable } => self.handle_pending_htlcs_forwards(time_forwardable),

            Event::SpendableOutputs { outputs } => self.handle_spendable_outputs(outputs),

            // Todo: an RPC for total amount earned
            Event::PaymentForwarded { fee_earned_msat, claim_from_onchain_tx,  prev_channel_id, next_channel_id} => info!(
                "Received a fee of {} milli-satoshis for a successfully forwarded payment from {} to {} through our {} lightning node. Was the forwarded HTLC claimed by our counterparty via an on-chain transaction?: {}",
                fee_earned_msat.unwrap_or_default(),
                prev_channel_id.map(hex::encode).unwrap_or_else(|| "unknown".into()),
                next_channel_id.map(hex::encode).unwrap_or_else(|| "unknown".into()),
                self.platform.coin.ticker(),
                claim_from_onchain_tx,
            ),

            Event::ChannelClosed {
                channel_id,
                user_channel_id,
                reason,
            } => self.handle_channel_closed(channel_id, user_channel_id, reason.to_string()),

            // Todo: Add spent UTXOs to RecentlySpentOutPoints if it's not discarded
            Event::DiscardFunding { channel_id, transaction } => info!(
                "Discarding funding tx: {} for channel {}",
                transaction.txid(),
                hex::encode(channel_id),
            ),

            // Handling updating channel penalties after successfully routing a payment along a path is done by the InvoicePayer.
            // Todo: Maybe add information to db about why a payment succeeded using this event
            Event::PaymentPathSuccessful {
                payment_id,
                payment_hash,
                path,
            } => info!(
                "Payment path: {:?}, successful for payment hash: {}, payment id: {}",
                path.iter().map(|hop| hop.pubkey.to_string()).collect::<Vec<_>>(),
                payment_hash.map(|h| hex::encode(h.0)).unwrap_or_default(),
                hex::encode(payment_id.0)
            ),

            // Handling updating channel penalties after a payment fails to route through a channel is done by the InvoicePayer.
            // Also abandoning or retrying a payment is handled by the InvoicePayer.
            // Todo: Add information to db about why a payment failed using this event
            Event::PaymentPathFailed {
                payment_hash,
                payment_failed_permanently,
                all_paths_failed,
                path,
                ..
            } => info!(
                "Payment path: {:?}, failed for payment hash: {}, permanent failure?: {}, All paths failed?: {}",
                path.iter().map(|hop| hop.pubkey.to_string()).collect::<Vec<_>>(),
                hex::encode(payment_hash.0),
                payment_failed_permanently,
                all_paths_failed,
            ),

            Event::OpenChannelRequest {
                temporary_channel_id,
                counterparty_node_id,
                funding_satoshis,
                push_msat,
                channel_type: _,
            } => self.handle_open_channel_request(temporary_channel_id, counterparty_node_id, funding_satoshis, push_msat),

            // Just log an error for now, but this event can be used along PaymentForwarded for a new RPC that shows stats about how a node
            // forward payments over it's outbound channels which can be useful for a user that wants to run a forwarding node for some profits.
            Event::HTLCHandlingFailed {
                prev_channel_id, failed_next_destination
            } => error!(
                "Failed to handle htlc from {} to {:?}",
                hex::encode(prev_channel_id),
                failed_next_destination,
            ),

            // ProbeSuccessful and ProbeFailed are events in response to a send_probe function call which sends a payment that probes a given route for liquidity.
            // send_probe is not used for now but may be used in order matching in the future to check if a swap can happen or not.
            Event::ProbeSuccessful { .. } => (),
            Event::ProbeFailed { .. } => (),
            Event::HTLCIntercepted { .. } => (),
            Event::ChannelReady { user_channel_id, .. } => info!("{}: {}", CHANNEL_READY_LOG, Uuid::from_u128(user_channel_id)),
        }
    }
}

pub async fn init_abortable_events(platform: Arc<Platform>, db: SqliteLightningDB) -> EnableLightningResult<()> {
    let closed_channels_without_closing_tx = db.get_closed_channels_with_no_closing_tx().await?;
    for channel_details in closed_channels_without_closing_tx {
        let platform_c = platform.clone();
        let db = db.clone();
        let uuid = channel_details.uuid;
        platform.spawner().spawn(async move {
            if let Ok(closing_tx_hash) = platform_c
                .get_channel_closing_tx(channel_details)
                .await
                .error_log_passthrough()
            {
                if let Err(e) = db.add_closing_tx_to_db(uuid, closing_tx_hash).await {
                    log::error!("Unable to update channel {} closing details in DB: {}", uuid, e);
                }
            }
        });
    }
    Ok(())
}

#[derive(Display)]
pub enum SignFundingTransactionError {
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
    #[display(fmt = "Error signing transaction: {_0}")]
    TxSignFailed(String),
}

// Generates the raw funding transaction with one output equal to the channel value.
async fn sign_funding_transaction(
    uuid: Uuid,
    output_script_pubkey: &Script,
    platform: Arc<Platform>,
) -> Result<Transaction, SignFundingTransactionError> {
    let coin = &platform.coin;
    let mut unsigned = {
        let unsigned_funding_txs = platform.unsigned_funding_txs.lock();
        unsigned_funding_txs
            .get(&uuid)
            .ok_or_else(|| {
                SignFundingTransactionError::Internal(format!(
                    "Unsigned funding tx not found for channel with uuid: {uuid}"
                ))
            })?
            .clone()
    };
    unsigned.outputs[0].script_pubkey = output_script_pubkey.to_bytes().into();

    let key_pair = coin
        .as_ref()
        .priv_key_policy
        .activated_key_or_err()
        .map_err(|e| SignFundingTransactionError::Internal(e.to_string()))?;

    let signed = sign_tx(
        unsigned,
        key_pair,
        SignatureVersion::WitnessV0,
        coin.as_ref().conf.fork_id,
    )
    .map_err(|e| SignFundingTransactionError::TxSignFailed(e.to_string()))?;

    Ok(Transaction::from(signed))
}

async fn save_channel_closing_details(
    db: SqliteLightningDB,
    platform: Arc<Platform>,
    uuid: Uuid,
    reason: String,
) -> SaveChannelClosingResult<()> {
    db.update_channel_to_closed(uuid, reason, now_sec_i64()).await?;

    let channel_details = db
        .get_channel_from_db(uuid)
        .await?
        .ok_or_else(|| MmError::new(SaveChannelClosingError::ChannelNotFound(uuid)))?;

    let closing_tx_hash = platform.get_channel_closing_tx(channel_details).await?;

    db.add_closing_tx_to_db(uuid, closing_tx_hash).await?;

    Ok(())
}

async fn add_claiming_tx_to_db_loop(
    db: SqliteLightningDB,
    closing_txid: String,
    claiming_txid: String,
    claimed_balance: f64,
) {
    while let Err(e) = db
        .add_claiming_tx_to_db(closing_txid.clone(), claiming_txid.clone(), claimed_balance)
        .await
    {
        error!("error {}", e);
        Timer::sleep(TRY_LOOP_INTERVAL).await;
    }
}

impl LightningEventHandler {
    pub fn new(
        platform: Arc<Platform>,
        channel_manager: Arc<ChannelManager>,
        keys_manager: Arc<KeysManager>,
        db: SqliteLightningDB,
        trusted_nodes: TrustedNodesShared,
    ) -> Self {
        LightningEventHandler {
            platform,
            channel_manager,
            keys_manager,
            db,
            trusted_nodes,
        }
    }

    fn handle_funding_generation_ready(
        &self,
        temporary_channel_id: [u8; 32],
        channel_value_satoshis: u64,
        output_script: Script,
        user_channel_id: u128,
        counterparty_node_id: PublicKey,
    ) {
        let uuid = Uuid::from_u128(user_channel_id);
        info!(
            "Handling FundingGenerationReady event for channel with uuid: {} with: {}",
            uuid, counterparty_node_id
        );

        let channel_manager = self.channel_manager.clone();
        let platform = self.platform.clone();
        let db = self.db.clone();

        let fut = async move {
            let funding_tx = match sign_funding_transaction(uuid, &output_script, platform.clone()).await {
                Ok(tx) => tx,
                Err(e) => {
                    error!(
                        "Error generating funding transaction for channel with uuid {}: {}",
                        uuid, e
                    );
                    return;
                },
            };
            let funding_txid = funding_tx.txid();
            // Give the funding transaction back to LDK for opening the channel.
            if let Err(e) =
                channel_manager.funding_transaction_generated(&temporary_channel_id, &counterparty_node_id, funding_tx)
            {
                error!("{:?}", e);
                return;
            }

            let best_block_height = platform.best_block_height();
            db.add_funding_tx_to_db(
                uuid,
                funding_txid.to_string(),
                channel_value_satoshis as i64,
                best_block_height as i64,
            )
            .await
            .error_log();
        };

        let settings = AbortSettings::default().critical_timout_s(CRITICAL_FUTURE_TIMEOUT);
        self.platform.spawner().spawn_with_settings(fut, settings);
    }

    fn handle_payment_claimable(&self, payment_hash: PaymentHash, claimable_amount: u64, purpose: PaymentPurpose) {
        info!(
            "{} for payment_hash: {} with amount {}",
            PAYMENT_CLAIMABLE_LOG,
            hex::encode(payment_hash.0),
            claimable_amount
        );
        let db = self.db.clone();
        let payment_preimage = match purpose {
            PaymentPurpose::InvoicePayment { payment_preimage, .. } => match payment_preimage {
                Some(preimage) => {
                    let fut = async move {
                        db.update_payment_to_claimable_in_db(payment_hash, preimage)
                            .await
                            .error_log_with_msg("Unable to update claimable payment info in DB!");
                    };
                    let settings = AbortSettings::default().critical_timout_s(CRITICAL_FUTURE_TIMEOUT);
                    self.platform.spawner().spawn_with_settings(fut, settings);
                    preimage
                },
                // This is a swap related payment since we don't have the preimage yet
                None => {
                    let amt_msat = Some(
                        claimable_amount
                            .try_into()
                            .expect("claimable_amount shouldn't exceed i64::MAX"),
                    );
                    let payment_info = PaymentInfo::new(
                        payment_hash,
                        PaymentType::InboundPayment,
                        "Swap Payment".into(),
                        amt_msat,
                    )
                    .with_status(HTLCStatus::Claimable);
                    let fut = async move {
                        db.add_payment_to_db(&payment_info)
                            .await
                            .error_log_with_msg("Unable to add payment information to DB!");
                    };

                    let settings = AbortSettings::default().critical_timout_s(CRITICAL_FUTURE_TIMEOUT);
                    self.platform.spawner().spawn_with_settings(fut, settings);

                    return;
                },
            },
            PaymentPurpose::SpontaneousPayment(preimage) => {
                let amt_msat = Some(
                    claimable_amount
                        .try_into()
                        .expect("claimable_amount shouldn't exceed i64::MAX"),
                );
                let payment_info =
                    PaymentInfo::new(payment_hash, PaymentType::InboundPayment, "keysend".into(), amt_msat)
                        .with_preimage(preimage)
                        .with_status(HTLCStatus::Claimable);
                let fut = async move {
                    db.add_payment_to_db(&payment_info)
                        .await
                        .error_log_with_msg("Unable to add payment information to DB!");
                };

                let settings = AbortSettings::default().critical_timout_s(CRITICAL_FUTURE_TIMEOUT);
                self.platform.spawner().spawn_with_settings(fut, settings);
                preimage
            },
        };
        self.channel_manager.claim_funds(payment_preimage);
    }

    fn handle_payment_claimed(&self, payment_hash: PaymentHash, amount_msat: u64) {
        info!(
            "Claimed an amount of {} millisatoshis for payment hash {}",
            amount_msat,
            hex::encode(payment_hash.0)
        );
        let db = self.db.clone();
        let fut = async move {
            match db
                .update_payment_status_in_db(payment_hash, &HTLCStatus::Succeeded)
                .await
            {
                Ok(_) => info!(
                    "{} of {} millisatoshis with payment hash {}",
                    SUCCESSFUL_CLAIM_LOG,
                    amount_msat,
                    hex::encode(payment_hash.0),
                ),
                Err(e) => error!("Unable to update payment status in DB error: {}", e),
            }
        };
        let settings = AbortSettings::default().critical_timout_s(CRITICAL_FUTURE_TIMEOUT);
        self.platform.spawner().spawn_with_settings(fut, settings);
    }

    fn handle_payment_sent(
        &self,
        payment_preimage: PaymentPreimage,
        payment_hash: PaymentHash,
        fee_paid_msat: Option<u64>,
    ) {
        info!(
            "Handling PaymentSent event for payment_hash: {}",
            hex::encode(payment_hash.0)
        );
        let db = self.db.clone();
        let fut = async move {
            match db
                .update_payment_to_sent_in_db(payment_hash, payment_preimage, fee_paid_msat)
                .await
            {
                Ok(_) => info!(
                    "{} with payment hash {}",
                    SUCCESSFUL_SEND_LOG,
                    hex::encode(payment_hash.0)
                ),
                Err(e) => error!("Unable to update sent payment info in DB error: {}", e),
            }
        };
        let settings = AbortSettings::default().critical_timout_s(CRITICAL_FUTURE_TIMEOUT);
        self.platform.spawner().spawn_with_settings(fut, settings);
    }

    fn handle_channel_closed(&self, channel_id: [u8; 32], user_channel_id: u128, reason: String) {
        info!(
            "Channel: {} closed for the following reason: {}",
            hex::encode(channel_id),
            reason
        );
        let uuid = Uuid::from_u128(user_channel_id);
        let db = self.db.clone();
        let platform = self.platform.clone();

        let fut = async move {
            if let Err(e) = save_channel_closing_details(db, platform, uuid, reason).await {
                // This is the case when a channel is closed before funding is broadcasted due to the counterparty disconnecting or other incompatibility issue.
                if e != SaveChannelClosingError::FundingTxNull.into() {
                    error!("Unable to update channel {} closing details in DB: {}", uuid, e);
                }
            }
        };

        let settings = AbortSettings::default().critical_timout_s(CRITICAL_FUTURE_TIMEOUT);
        self.platform.spawner().spawn_with_settings(fut, settings);
    }

    fn handle_payment_failed(&self, payment_hash: PaymentHash) {
        info!(
            "Handling PaymentFailed event for payment_hash: {}",
            hex::encode(payment_hash.0)
        );
        let db = self.db.clone();
        let fut = async move {
            db.update_payment_status_in_db(payment_hash, &HTLCStatus::Failed)
                .await
                .error_log_with_msg("Unable to update payment status in DB!");
        };
        let settings = AbortSettings::default().critical_timout_s(CRITICAL_FUTURE_TIMEOUT);
        self.platform.spawner().spawn_with_settings(fut, settings);
    }

    fn handle_pending_htlcs_forwards(&self, time_forwardable: Duration) {
        info!("Handling PendingHTLCsForwardable event!");
        let min_wait_time = time_forwardable.as_millis() as u64;
        let channel_manager = self.channel_manager.clone();
        self.platform.spawner().spawn(async move {
            let millis_to_sleep = rand::thread_rng().gen_range(min_wait_time, min_wait_time * 5);
            Timer::sleep_ms(millis_to_sleep).await;
            channel_manager.process_pending_htlc_forwards();
        });
    }

    fn handle_spendable_outputs(&self, outputs: Vec<SpendableOutputDescriptor>) {
        info!("Handling SpendableOutputs event!");
        if outputs.is_empty() {
            error!("Received SpendableOutputs event with no outputs!");
            return;
        }

        let platform = self.platform.clone();
        let db = self.db.clone();
        let keys_manager = self.keys_manager.clone();

        let fut = async move {
            // Todo: add support for HD and Hardware wallets for funding transactions and spending spendable outputs (channel closing transactions)
            let my_address = match platform.coin.as_ref().derivation_method.single_addr_or_err().await {
                Ok(addr) => addr.clone(),
                Err(e) => {
                    error!("{}", e);
                    return;
                },
            };
            let change_destination_script = match Builder::build_p2wpkh(my_address.hash()) {
                Ok(script) => script.to_bytes().take().into(),
                Err(err) => {
                    error!(
                        "Could not create witness script for change output {}: {}",
                        my_address, err
                    );
                    return;
                },
            };
            let feerate_sat_per_1000_weight = platform.get_est_sat_per_1000_weight(ConfirmationTarget::Normal);
            let output_descriptors = outputs.iter().collect::<Vec<_>>();
            let claiming_tx = match keys_manager.spend_spendable_outputs(
                &output_descriptors,
                Vec::new(),
                change_destination_script,
                feerate_sat_per_1000_weight,
                &Secp256k1::new(),
            ) {
                Ok(tx) => tx,
                Err(_) => {
                    error!("Error spending spendable outputs");
                    return;
                },
            };

            let claiming_txid = claiming_tx.txid();
            let tx_hex = serialize_hex(&claiming_tx);

            if let Err(e) = platform.coin.send_raw_tx(&tx_hex).compat().await {
                // TODO: broadcast transaction through p2p network in this case, we have to check that the transactions is confirmed on-chain after this.
                error!(
                    "Broadcasting of the claiming transaction {} failed: {}",
                    claiming_txid, e
                );
                return;
            }

            let claiming_tx_inputs_value = outputs.iter().fold(0, |sum, output| match output {
                SpendableOutputDescriptor::StaticOutput { output, .. } => sum + output.value,
                SpendableOutputDescriptor::DelayedPaymentOutput(descriptor) => sum + descriptor.output.value,
                SpendableOutputDescriptor::StaticPaymentOutput(descriptor) => sum + descriptor.output.value,
            });
            let claiming_tx_outputs_value = claiming_tx.output.iter().fold(0, |sum, txout| sum + txout.value);
            if claiming_tx_inputs_value < claiming_tx_outputs_value {
                error!(
                    "Claiming transaction input value {} can't be less than outputs value {}!",
                    claiming_tx_inputs_value, claiming_tx_outputs_value
                );
                return;
            }
            let claiming_tx_fee = claiming_tx_inputs_value - claiming_tx_outputs_value;
            let claiming_tx_fee_per_channel = (claiming_tx_fee as f64) / (outputs.len() as f64);

            for output in outputs {
                let (closing_txid, claimed_balance) = match output {
                    SpendableOutputDescriptor::StaticOutput { outpoint, output } => {
                        (outpoint.txid.to_string(), output.value)
                    },
                    SpendableOutputDescriptor::DelayedPaymentOutput(descriptor) => {
                        (descriptor.outpoint.txid.to_string(), descriptor.output.value)
                    },
                    SpendableOutputDescriptor::StaticPaymentOutput(descriptor) => {
                        (descriptor.outpoint.txid.to_string(), descriptor.output.value)
                    },
                };

                // This doesn't need to be respawned on restart unlike add_closing_tx_to_db since Event::SpendableOutputs will be re-fired on restart
                // if the spending_tx is not broadcasted.
                add_claiming_tx_to_db_loop(
                    db.clone(),
                    closing_txid,
                    claiming_txid.to_string(),
                    (claimed_balance as f64) - claiming_tx_fee_per_channel,
                )
                .await;
            }
        };

        let settings = AbortSettings::default().critical_timout_s(CRITICAL_FUTURE_TIMEOUT);
        self.platform.spawner().spawn_with_settings(fut, settings);
    }

    fn handle_open_channel_request(
        &self,
        temporary_channel_id: [u8; 32],
        counterparty_node_id: PublicKey,
        funding_satoshis: u64,
        push_msat: u64,
    ) {
        info!(
            "Handling OpenChannelRequest from node: {} with funding value: {} and starting balance: {}",
            counterparty_node_id, funding_satoshis, push_msat,
        );

        let db = self.db.clone();
        let trusted_nodes = self.trusted_nodes.clone();
        let channel_manager = self.channel_manager.clone();
        let platform = self.platform.clone();
        let fut = async move {
            let uuid = new_uuid();
            let uuid_u128 = uuid.as_u128();
            let trusted_nodes = trusted_nodes.lock().clone();
            let accepted_inbound_channel_with_0conf = trusted_nodes.contains(&counterparty_node_id)
                && channel_manager
                    .accept_inbound_channel_from_trusted_peer_0conf(
                        &temporary_channel_id,
                        &counterparty_node_id,
                        uuid_u128,
                    )
                    .is_ok();

            if accepted_inbound_channel_with_0conf
                || channel_manager
                    .accept_inbound_channel(&temporary_channel_id, &counterparty_node_id, uuid_u128)
                    .is_ok()
            {
                let is_public = match channel_manager
                    .list_channels()
                    .into_iter()
                    .find(|chan| chan.user_channel_id == uuid_u128)
                {
                    Some(details) => details.is_public,
                    None => {
                        error!("Inbound channel {} details should be found by list_channels!", uuid);
                        return;
                    },
                };

                let pending_channel_details =
                    DBChannelDetails::new(uuid, temporary_channel_id, counterparty_node_id, false, is_public);
                if let Err(e) = db.add_channel_to_db(&pending_channel_details).await {
                    error!("Unable to add new inbound channel {} to db: {}", uuid, e);
                }

                while let Some(details) = channel_manager
                    .list_channels()
                    .into_iter()
                    .find(|chan| chan.user_channel_id == uuid_u128)
                {
                    if let Some(funding_tx) = details.funding_txo {
                        let best_block_height = platform.best_block_height();
                        db.add_funding_tx_to_db(
                            uuid,
                            funding_tx.txid.to_string(),
                            funding_satoshis as i64,
                            best_block_height as i64,
                        )
                        .await
                        .error_log();
                        break;
                    }

                    Timer::sleep(TRY_LOOP_INTERVAL).await;
                }
            }
        };

        self.platform.spawner().spawn(fut);
    }
}
