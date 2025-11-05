use super::{rpc::*, AllBalancesResult, TendermintCoin, TendermintCommons, TendermintToken};

use crate::my_tx_history_v2::{CoinWithTxHistoryV2, MyTxHistoryErrorV2, MyTxHistoryTarget, TxHistoryStorage};
use crate::tendermint::htlc::CustomTendermintMsgType;
use crate::tendermint::TendermintFeeDetails;
use crate::tx_history_storage::{GetTxHistoryFilters, WalletId};
use crate::utxo::tx_history_events::TxHistoryEventStreamer;
use crate::utxo::utxo_common::big_decimal_from_sat_unsigned;
use crate::{
    HistorySyncState, MarketCoinOps, MmCoin, TransactionData, TransactionDetails, TransactionType, TxFeeDetails,
};
use async_trait::async_trait;
use base64::Engine;
use bitcrypto::sha256;
use common::executor::Timer;
use common::log;
use cosmrs::tendermint::abci::Event;
use cosmrs::tendermint::abci::{Code as TxCode, EventAttribute};
use cosmrs::tx::Fee;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::MmResult;
use mm2_event_stream::{DeriveStreamerId, StreamingManager};
use mm2_number::BigDecimal;
use mm2_state_machine::prelude::*;
use mm2_state_machine::state_machine::StateMachineTrait;
use primitives::hash::H256;
use rpc::v1::types::Bytes as BytesJson;
use std::cmp;
use std::convert::{Infallible, TryInto};

const TX_PAGE_SIZE: u8 = 50;

const DEFAULT_TRANSFER_EVENT_COUNT: usize = 1;

const TRANSFER_EVENT: &str = "transfer";

const CREATE_HTLC_EVENT: &str = "create_htlc";
const CLAIM_HTLC_EVENT: &str = "claim_htlc";

const IBC_SEND_EVENT: &str = "ibc_transfer";
const IBC_RECEIVE_EVENT: &str = "fungible_token_packet";
const IBC_NFT_RECEIVE_EVENT: &str = "non_fungible_token_packet";

const DELEGATE_EVENT: &str = "delegate";
const UNDELEGATE_EVENT: &str = "unbond";
const WITHDRAW_REWARDS_EVENT: &str = "withdraw_rewards";

const ACCEPTED_EVENTS: &[&str] = &[
    TRANSFER_EVENT,
    CREATE_HTLC_EVENT,
    CLAIM_HTLC_EVENT,
    IBC_SEND_EVENT,
    IBC_RECEIVE_EVENT,
    IBC_NFT_RECEIVE_EVENT,
    DELEGATE_EVENT,
    UNDELEGATE_EVENT,
    WITHDRAW_REWARDS_EVENT,
];

const RECEIVER_TAG_KEY: &str = "receiver";
const RECEIVER_TAG_KEY_BASE64: &str = "cmVjZWl2ZXI=";

const RECIPIENT_TAG_KEY: &str = "recipient";
const RECIPIENT_TAG_KEY_BASE64: &str = "cmVjaXBpZW50";

const SENDER_TAG_KEY: &str = "sender";
const SENDER_TAG_KEY_BASE64: &str = "c2VuZGVy";

const DELEGATOR_TAG_KEY: &str = "delegator";
const DELEGATOR_TAG_KEY_BASE64: &str = "ZGVsZWdhdG9y";

const VALIDATOR_TAG_KEY: &str = "validator";
const VALIDATOR_TAG_KEY_BASE64: &str = "dmFsaWRhdG9y";

const AMOUNT_TAG_KEY: &str = "amount";
const AMOUNT_TAG_KEY_BASE64: &str = "YW1vdW50";

macro_rules! try_or_return_stopped_as_err {
    ($exp:expr, $reason: expr, $fmt:literal) => {
        match $exp {
            Ok(t) => t,
            Err(e) => {
                return Err(Stopped {
                    phantom: Default::default(),
                    stop_reason: $reason(format!("{}: {}", $fmt, e)),
                })
            },
        }
    };
}

macro_rules! try_or_continue {
    ($exp:expr, $fmt:literal) => {
        match $exp {
            Ok(t) => t,
            Err(e) => {
                log::debug!("{}: {}", $fmt, e);
                continue;
            },
        }
    };
}

macro_rules! some_or_continue {
    ($exp:expr) => {
        match $exp {
            Some(t) => t,
            None => {
                continue;
            },
        }
    };
}

macro_rules! some_or_return {
    ($exp:expr) => {
        match $exp {
            Some(t) => t,
            None => {
                return;
            },
        }
    };
}

trait CoinCapabilities: TendermintCommons + CoinWithTxHistoryV2 + MmCoin + MarketCoinOps {}
impl CoinCapabilities for TendermintCoin {}

#[async_trait]
impl CoinWithTxHistoryV2 for TendermintCoin {
    fn history_wallet_id(&self) -> WalletId {
        WalletId::new(self.ticker().into())
    }

    async fn get_tx_history_filters(
        &self,
        _target: MyTxHistoryTarget,
    ) -> MmResult<GetTxHistoryFilters, MyTxHistoryErrorV2> {
        Ok(GetTxHistoryFilters::for_address(self.account_id.to_string()))
    }
}

#[async_trait]
impl CoinWithTxHistoryV2 for TendermintToken {
    fn history_wallet_id(&self) -> WalletId {
        WalletId::new(self.platform_ticker().to_owned())
    }

    async fn get_tx_history_filters(
        &self,
        _target: MyTxHistoryTarget,
    ) -> MmResult<GetTxHistoryFilters, MyTxHistoryErrorV2> {
        let denom_hash = sha256(self.denom.as_ref().to_lowercase().as_bytes());
        let token_id = H256::from(denom_hash.take()).to_string();

        Ok(GetTxHistoryFilters::for_address(self.platform_coin.account_id.to_string()).with_token_id(token_id))
    }
}

struct TendermintTxHistoryStateMachine<Coin: CoinCapabilities, Storage: TxHistoryStorage> {
    coin: Coin,
    storage: Storage,
    streaming_manager: StreamingManager,
    balances: AllBalancesResult,
    last_received_page: u32,
    last_spent_page: u32,
}

impl<Coin: CoinCapabilities, Storage: TxHistoryStorage> StateMachineTrait
    for TendermintTxHistoryStateMachine<Coin, Storage>
{
    type Result = ();
    type Error = Infallible;
}

impl<Coin: CoinCapabilities, Storage: TxHistoryStorage> StandardStateMachine
    for TendermintTxHistoryStateMachine<Coin, Storage>
{
}

struct TendermintInit<Coin, Storage> {
    phantom: std::marker::PhantomData<(Coin, Storage)>,
}

impl<Coin, Storage> TendermintInit<Coin, Storage> {
    fn new() -> Self {
        TendermintInit {
            phantom: Default::default(),
        }
    }
}

#[derive(Debug)]
enum StopReason {
    #[expect(dead_code)]
    StorageError(String),
    RpcClient(String),
}

struct Stopped<Coin, Storage> {
    phantom: std::marker::PhantomData<(Coin, Storage)>,
    stop_reason: StopReason,
}

impl<Coin, Storage> Stopped<Coin, Storage> {
    fn storage_error<E>(e: E) -> Self
    where
        E: std::fmt::Debug,
    {
        Stopped {
            phantom: Default::default(),
            stop_reason: StopReason::StorageError(format!("{e:?}")),
        }
    }
}

struct WaitForHistoryUpdateTrigger<Coin, Storage> {
    address: String,
    last_height_state: u64,
    phantom: std::marker::PhantomData<(Coin, Storage)>,
}

impl<Coin, Storage> WaitForHistoryUpdateTrigger<Coin, Storage> {
    fn new(address: String, last_height_state: u64) -> Self {
        WaitForHistoryUpdateTrigger {
            address,
            last_height_state,
            phantom: Default::default(),
        }
    }
}

struct OnIoErrorCooldown<Coin, Storage> {
    address: String,
    last_block_height: u64,
    phantom: std::marker::PhantomData<(Coin, Storage)>,
}

impl<Coin, Storage> OnIoErrorCooldown<Coin, Storage> {
    fn new(address: String, last_block_height: u64) -> Self {
        OnIoErrorCooldown {
            address,
            last_block_height,
            phantom: Default::default(),
        }
    }
}

impl<Coin, Storage> TransitionFrom<FetchingTransactionsData<Coin, Storage>> for OnIoErrorCooldown<Coin, Storage> {}

#[async_trait]
impl<Coin, Storage> State for OnIoErrorCooldown<Coin, Storage>
where
    Coin: CoinCapabilities,
    Storage: TxHistoryStorage,
{
    type StateMachine = TendermintTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(
        mut self: Box<Self>,
        _ctx: &mut TendermintTxHistoryStateMachine<Coin, Storage>,
    ) -> StateResult<TendermintTxHistoryStateMachine<Coin, Storage>> {
        Timer::sleep(30.).await;

        // retry history fetching process from last saved block
        return Self::change_state(FetchingTransactionsData::new(self.address, self.last_block_height));
    }
}

struct FetchingTransactionsData<Coin, Storage> {
    /// The list of addresses for those we have requested [`UpdatingUnconfirmedTxes::all_tx_ids_with_height`] TX hashes
    /// at the `FetchingTxHashes` state.
    address: String,
    from_block_height: u64,
    phantom: std::marker::PhantomData<(Coin, Storage)>,
}

impl<Coin, Storage> FetchingTransactionsData<Coin, Storage> {
    fn new(address: String, from_block_height: u64) -> Self {
        FetchingTransactionsData {
            address,
            phantom: Default::default(),
            from_block_height,
        }
    }
}

impl<Coin, Storage> TransitionFrom<TendermintInit<Coin, Storage>> for Stopped<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<TendermintInit<Coin, Storage>> for FetchingTransactionsData<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<OnIoErrorCooldown<Coin, Storage>> for FetchingTransactionsData<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<WaitForHistoryUpdateTrigger<Coin, Storage>> for OnIoErrorCooldown<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<WaitForHistoryUpdateTrigger<Coin, Storage>> for Stopped<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<FetchingTransactionsData<Coin, Storage>> for Stopped<Coin, Storage> {}

impl<Coin, Storage> TransitionFrom<WaitForHistoryUpdateTrigger<Coin, Storage>>
    for FetchingTransactionsData<Coin, Storage>
{
}

impl<Coin, Storage> TransitionFrom<FetchingTransactionsData<Coin, Storage>>
    for WaitForHistoryUpdateTrigger<Coin, Storage>
{
}

#[async_trait]
impl<Coin, Storage> State for WaitForHistoryUpdateTrigger<Coin, Storage>
where
    Coin: CoinCapabilities,
    Storage: TxHistoryStorage,
{
    type StateMachine = TendermintTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(
        self: Box<Self>,
        ctx: &mut TendermintTxHistoryStateMachine<Coin, Storage>,
    ) -> StateResult<TendermintTxHistoryStateMachine<Coin, Storage>> {
        loop {
            Timer::sleep(30.).await;

            let ctx_balances = ctx.balances.clone();

            let balances = match ctx.coin.get_all_balances().await {
                Ok(balances) => balances,
                Err(_) => {
                    return Self::change_state(OnIoErrorCooldown::new(self.address.clone(), self.last_height_state));
                },
            };

            if balances != ctx_balances {
                // Update balances
                ctx.balances = balances;

                return Self::change_state(FetchingTransactionsData::new(
                    self.address.clone(),
                    self.last_height_state,
                ));
            }
        }
    }
}

#[async_trait]
impl<Coin, Storage> State for FetchingTransactionsData<Coin, Storage>
where
    Coin: CoinCapabilities,
    Storage: TxHistoryStorage,
{
    type StateMachine = TendermintTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(
        self: Box<Self>,
        ctx: &mut TendermintTxHistoryStateMachine<Coin, Storage>,
    ) -> StateResult<TendermintTxHistoryStateMachine<Coin, Storage>> {
        struct TxAmounts {
            total: BigDecimal,
            spent_by_me: BigDecimal,
            received_by_me: BigDecimal,
        }

        fn get_tx_amounts(
            transfer_details: &TransferDetails,
            is_self_transfer: bool,
            sent_by_me: bool,
            is_sign_claim_htlc: bool,
            fee_details: Option<&TendermintFeeDetails>,
        ) -> TxAmounts {
            let amount = BigDecimal::from(transfer_details.amount);

            let total = if is_sign_claim_htlc && !is_self_transfer {
                BigDecimal::default()
            } else {
                amount.clone()
            };

            let spent_by_me =
                if sent_by_me && !matches!(transfer_details.transfer_event_type, TransferEventType::ClaimHtlc) {
                    amount.clone()
                } else {
                    BigDecimal::default()
                };

            let received_by_me = if !sent_by_me || is_self_transfer {
                amount
            } else {
                BigDecimal::default()
            };

            let mut tx_amounts = TxAmounts {
                total,
                spent_by_me,
                received_by_me,
            };

            if let Some(fee_details) = fee_details {
                tx_amounts.total += BigDecimal::from(fee_details.uamount);
                tx_amounts.spent_by_me += BigDecimal::from(fee_details.uamount);
            }

            tx_amounts
        }

        fn get_fee_details<Coin>(fee: Fee, coin: &Coin) -> Result<TendermintFeeDetails, String>
        where
            Coin: CoinCapabilities,
        {
            let fee_coin = fee
                .amount
                .first()
                .ok_or_else(|| "fee coin can't be empty".to_string())?;
            let fee_uamount: u64 = fee_coin.amount.to_string().parse().map_err(|e| format!("{e:?}"))?;

            Ok(TendermintFeeDetails {
                coin: coin.platform_ticker().to_string(),
                amount: big_decimal_from_sat_unsigned(fee_uamount, coin.decimals()),
                uamount: fee_uamount,
                gas_limit: fee.gas_limit,
            })
        }

        #[derive(Default, Clone)]
        enum TransferEventType {
            #[default]
            Standard,
            CreateHtlc,
            ClaimHtlc,
            IBCSend,
            IBCReceive,
            Delegate,
            Undelegate,
            ClaimRewards,
        }

        #[derive(Clone)]
        struct TransferDetails {
            ticker: String,
            denom: String,
            from: String,
            to: String,
            amount: u64,
            transfer_event_type: TransferEventType,
        }

        /// Reads sender and receiver addresses properly from an IBC event.
        fn read_real_ibc_addresses(transfer_details: &mut TransferDetails, msg_event: &Event) {
            let event_type = match msg_event.kind.as_str() {
                IBC_SEND_EVENT => TransferEventType::IBCSend,
                IBC_RECEIVE_EVENT | IBC_NFT_RECEIVE_EVENT => TransferEventType::IBCReceive,
                _ => unreachable!("`read_real_ibc_addresses` shouldn't be called for non-IBC events."),
            };

            let from = some_or_return!(get_value_from_event_attributes(
                &msg_event.attributes,
                SENDER_TAG_KEY,
                SENDER_TAG_KEY_BASE64
            ));

            let to = some_or_return!(get_value_from_event_attributes(
                &msg_event.attributes,
                RECEIVER_TAG_KEY,
                RECEIVER_TAG_KEY_BASE64,
            ));

            transfer_details.from = from;
            transfer_details.to = to;
            transfer_details.transfer_event_type = event_type;
        }

        /// Reads sender and receiver addresses properly from an HTLC event.
        fn read_real_htlc_addresses(transfer_details: &mut TransferDetails, msg_event: &Event) {
            match msg_event.kind.as_str() {
                CREATE_HTLC_EVENT => {
                    let from = some_or_return!(get_value_from_event_attributes(
                        &msg_event.attributes,
                        SENDER_TAG_KEY,
                        SENDER_TAG_KEY_BASE64
                    ));

                    let to = some_or_return!(get_value_from_event_attributes(
                        &msg_event.attributes,
                        RECEIVER_TAG_KEY,
                        RECEIVER_TAG_KEY_BASE64,
                    ));

                    transfer_details.from = from;
                    transfer_details.to = to;
                    transfer_details.transfer_event_type = TransferEventType::CreateHtlc;
                },
                CLAIM_HTLC_EVENT => {
                    let from = some_or_return!(get_value_from_event_attributes(
                        &msg_event.attributes,
                        SENDER_TAG_KEY,
                        SENDER_TAG_KEY_BASE64
                    ));

                    transfer_details.from = from;
                    transfer_details.transfer_event_type = TransferEventType::ClaimHtlc;
                },
                _ => unreachable!("`read_real_htlc_addresses` shouldn't be called for non-HTLC events."),
            }
        }

        fn parse_transfer_values_from_events<Coin>(coin: &Coin, mut tx_events: Vec<&Event>) -> Vec<TransferDetails>
        where
            Coin: CoinCapabilities,
        {
            let mut transfer_details_list: Vec<TransferDetails> = vec![];

            for i in 0..tx_events.len() {
                // Avoid out-of-bounds exceptions after removing HTLC and IBC elements below.
                if i >= tx_events.len() {
                    break;
                }

                let event = tx_events[i];

                let amount_with_denoms = some_or_continue!(get_value_from_event_attributes(
                    &event.attributes,
                    AMOUNT_TAG_KEY,
                    AMOUNT_TAG_KEY_BASE64
                ));

                let amount_with_denoms = amount_with_denoms.split(',');
                for amount_with_denom in amount_with_denoms {
                    let extracted_amount: String = amount_with_denom.chars().take_while(|c| c.is_numeric()).collect();
                    let denom = amount_with_denom[extracted_amount.len()..].to_owned();
                    let ticker = some_or_continue!(coin.denom_to_ticker(&denom));
                    let amount = some_or_continue!(extracted_amount.parse().ok());

                    match event.kind.as_str() {
                        TRANSFER_EVENT => {
                            let from = some_or_continue!(get_value_from_event_attributes(
                                &event.attributes,
                                SENDER_TAG_KEY,
                                SENDER_TAG_KEY_BASE64
                            ));

                            let to = some_or_continue!(get_value_from_event_attributes(
                                &event.attributes,
                                RECIPIENT_TAG_KEY,
                                RECIPIENT_TAG_KEY_BASE64,
                            ));

                            let mut tx_details = TransferDetails {
                                ticker,
                                denom,
                                from,
                                to,
                                amount,
                                // Default is Standard, can be changed later in read_real_htlc_addresses
                                transfer_event_type: TransferEventType::default(),
                            };

                            // For HTLC transactions, the sender and receiver addresses in the "transfer" event will be incorrect.
                            // Use `read_real_htlc_addresses` to handle them properly.
                            if let Some(htlc_event_index) = tx_events
                                .iter()
                                .position(|e| [CREATE_HTLC_EVENT, CLAIM_HTLC_EVENT].contains(&e.kind.as_str()))
                            {
                                read_real_htlc_addresses(&mut tx_details, tx_events[htlc_event_index]);
                                tx_events.remove(htlc_event_index);
                            }
                            // For IBC transactions, the sender and receiver addresses in the "transfer" event will be incorrect.
                            // Use `read_real_ibc_addresses` to handle them properly.
                            else if let Some(ibc_event_index) = tx_events.iter().position(|e| {
                                [IBC_SEND_EVENT, IBC_RECEIVE_EVENT, IBC_NFT_RECEIVE_EVENT].contains(&e.kind.as_str())
                            }) {
                                read_real_ibc_addresses(&mut tx_details, tx_events[ibc_event_index]);
                                tx_events.remove(ibc_event_index);
                            }

                            handle_new_transfer_event(&mut transfer_details_list, tx_details);
                        },

                        DELEGATE_EVENT => {
                            let from = some_or_continue!(get_value_from_event_attributes(
                                &event.attributes,
                                DELEGATOR_TAG_KEY,
                                DELEGATOR_TAG_KEY_BASE64,
                            ));

                            let to = some_or_continue!(get_value_from_event_attributes(
                                &event.attributes,
                                VALIDATOR_TAG_KEY,
                                VALIDATOR_TAG_KEY_BASE64,
                            ));

                            let tx_details = TransferDetails {
                                ticker,
                                denom,
                                from,
                                to,
                                amount,
                                transfer_event_type: TransferEventType::Delegate,
                            };

                            handle_new_transfer_event(&mut transfer_details_list, tx_details);
                        },

                        UNDELEGATE_EVENT => {
                            let from = some_or_continue!(get_value_from_event_attributes(
                                &event.attributes,
                                DELEGATOR_TAG_KEY,
                                DELEGATOR_TAG_KEY_BASE64,
                            ));

                            let tx_details = TransferDetails {
                                ticker,
                                denom,
                                from,
                                to: String::default(),
                                amount: 0,
                                transfer_event_type: TransferEventType::Undelegate,
                            };

                            handle_new_transfer_event(&mut transfer_details_list, tx_details);
                        },

                        WITHDRAW_REWARDS_EVENT => {
                            let to = some_or_continue!(get_value_from_event_attributes(
                                &event.attributes,
                                DELEGATOR_TAG_KEY,
                                DELEGATOR_TAG_KEY_BASE64,
                            ));

                            let from = some_or_continue!(get_value_from_event_attributes(
                                &event.attributes,
                                VALIDATOR_TAG_KEY,
                                VALIDATOR_TAG_KEY_BASE64,
                            ));

                            let tx_details = TransferDetails {
                                ticker,
                                denom,
                                from,
                                to,
                                amount,
                                transfer_event_type: TransferEventType::ClaimRewards,
                            };

                            handle_new_transfer_event(&mut transfer_details_list, tx_details);
                        },

                        unrecognized => {
                            covered_warn!(
                                "Found an unrecognized event '{unrecognized}' in transaction history processing."
                            );
                        },
                    };
                }
            }

            transfer_details_list
        }

        fn handle_new_transfer_event(transfer_details_list: &mut Vec<TransferDetails>, new_transfer: TransferDetails) {
            let mut existing_transfer = transfer_details_list.iter_mut().find(|details| {
                details.from == new_transfer.from
                    && details.to == new_transfer.to
                    && details.denom == new_transfer.denom
            });

            if let Some(existing_transfer) = &mut existing_transfer {
                // Handle multi-amount transfer events
                existing_transfer.amount += new_transfer.amount;
            } else {
                transfer_details_list.push(new_transfer);
            }
        }

        fn get_transfer_details<Coin>(
            coin: &Coin,
            mut tx_events: Vec<Event>,
            fee_amount_with_denom: String,
        ) -> Vec<TransferDetails>
        where
            Coin: CoinCapabilities,
        {
            tx_events.sort_by(|a, b| a.kind.cmp(&b.kind));
            tx_events.dedup();

            // We are only interested `DELEGATE_EVENT` events for delegation transactions.
            if let Some(delegate_event) = tx_events.iter().find(|e| e.kind == DELEGATE_EVENT) {
                return parse_transfer_values_from_events(coin, vec![delegate_event]);
            };

            // We are only interested `UNDELEGATE_EVENT` events for undelegation transactions.
            if let Some(undelegate_event) = tx_events.iter().find(|e| e.kind == UNDELEGATE_EVENT) {
                return parse_transfer_values_from_events(coin, vec![undelegate_event]);
            };

            // We are only interested `WITHDRAW_REWARDS_EVENT` events for withdraw reward transactions.
            if let Some(withdraw_rewards_event) = tx_events.iter().find(|e| e.kind == WITHDRAW_REWARDS_EVENT) {
                return parse_transfer_values_from_events(coin, vec![withdraw_rewards_event]);
            };

            // Filter out irrelevant events
            let mut events: Vec<&Event> = tx_events
                .iter()
                .filter(|event| ACCEPTED_EVENTS.contains(&event.kind.as_str()))
                .rev()
                .collect();

            if events.len() > DEFAULT_TRANSFER_EVENT_COUNT {
                events.retain(|event| {
                    // Fees are included in `TRANSFER_EVENT` events, but since we handle fees
                    // separately, drop them from this list as we use them to extract the user
                    // amounts.
                    if event.kind == TRANSFER_EVENT {
                        let amount_with_denom =
                            get_value_from_event_attributes(&event.attributes, AMOUNT_TAG_KEY, AMOUNT_TAG_KEY_BASE64);

                        return amount_with_denom.as_deref() != Some(&fee_amount_with_denom);
                    }

                    true
                });
            }

            parse_transfer_values_from_events(coin, events)
        }

        fn get_transaction_type(
            transfer_event_type: &TransferEventType,
            token_id: Option<BytesJson>,
            is_sign_claim_htlc: bool,
        ) -> TransactionType {
            match (transfer_event_type, token_id) {
                (TransferEventType::CreateHtlc, token_id) => TransactionType::CustomTendermintMsg {
                    msg_type: CustomTendermintMsgType::SendHtlcAmount,
                    token_id,
                },
                (TransferEventType::ClaimHtlc, token_id) => TransactionType::CustomTendermintMsg {
                    msg_type: if is_sign_claim_htlc {
                        CustomTendermintMsgType::SignClaimHtlc
                    } else {
                        CustomTendermintMsgType::ClaimHtlcAmount
                    },
                    token_id,
                },
                (TransferEventType::IBCSend, token_id) | (TransferEventType::IBCReceive, token_id) => {
                    TransactionType::TendermintIBCTransfer { token_id }
                },
                (TransferEventType::Delegate, _) => TransactionType::StakingDelegation,
                (TransferEventType::Undelegate, _) => TransactionType::RemoveDelegation,
                (TransferEventType::ClaimRewards, _) => TransactionType::ClaimDelegationRewards,
                (TransferEventType::Standard, Some(token_id)) => TransactionType::TokenTransfer(token_id),
                (TransferEventType::Standard, None) => TransactionType::StandardTransfer,
            }
        }

        fn get_pair_addresses(
            my_address: String,
            tx_sent_by_me: bool,
            transfer_details: &TransferDetails,
        ) -> Option<(Vec<String>, Vec<String>)> {
            match transfer_details.transfer_event_type {
                TransferEventType::CreateHtlc => {
                    if tx_sent_by_me {
                        Some((vec![my_address], vec![]))
                    } else {
                        // This shouldn't happen if rpc node properly executes the tx search query.
                        None
                    }
                },
                TransferEventType::ClaimHtlc => Some((vec![my_address], vec![])),
                TransferEventType::Standard
                | TransferEventType::IBCSend
                | TransferEventType::IBCReceive
                | TransferEventType::Delegate
                | TransferEventType::ClaimRewards => {
                    Some((vec![transfer_details.from.clone()], vec![transfer_details.to.clone()]))
                },
                TransferEventType::Undelegate => Some((vec![my_address], vec![])),
            }
        }

        async fn fetch_and_insert_txs<Coin, Storage>(
            address: String,
            coin: &Coin,
            storage: &Storage,
            streaming_manager: &StreamingManager,
            query: String,
            from_height: u64,
            page: &mut u32,
        ) -> Result<u64, Stopped<Coin, Storage>>
        where
            Coin: CoinCapabilities,
            Storage: TxHistoryStorage,
        {
            let mut highest_height = from_height;

            let client = try_or_return_stopped_as_err!(
                coin.rpc_client().await,
                StopReason::RpcClient,
                "could not get rpc client"
            );

            loop {
                let response = try_or_return_stopped_as_err!(
                    client
                        .perform(TxSearchRequest::new(
                            query.clone(),
                            false,
                            *page,
                            TX_PAGE_SIZE,
                            TendermintResultOrder::Ascending.into(),
                        ))
                        .await,
                    StopReason::RpcClient,
                    "tx search rpc call failed"
                );

                let mut tx_details = vec![];
                let current_page_is_full = response.txs.len() == TX_PAGE_SIZE as usize;
                for tx in response.txs {
                    if tx.tx_result.code != TxCode::Ok {
                        continue;
                    }

                    let timestamp = try_or_return_stopped_as_err!(
                        coin.get_block_timestamp(i64::from(tx.height)).await,
                        StopReason::RpcClient,
                        "could not get block_timestamp over rpc node"
                    );
                    let timestamp = some_or_continue!(timestamp);

                    let tx_hash = tx.hash.to_string();

                    highest_height = cmp::max(highest_height, tx.height.into());

                    let deserialized_tx =
                        try_or_continue!(cosmrs::Tx::from_bytes(&tx.tx), "Could not deserialize transaction");

                    let msg = try_or_continue!(
                        deserialized_tx.body.messages.first().ok_or("Tx body couldn't be read."),
                        "Tx body messages is empty"
                    )
                    .value
                    .as_slice();

                    let fee_data = match deserialized_tx.auth_info.fee.amount.first() {
                        Some(data) => data,
                        None => {
                            log::debug!("Could not read transaction fee for tx '{}', skipping it", &tx_hash);
                            continue;
                        },
                    };

                    let fee_amount_with_denom = format!("{}{}", fee_data.amount, fee_data.denom);

                    let transfer_details_list = get_transfer_details(coin, tx.tx_result.events, fee_amount_with_denom);

                    if transfer_details_list.is_empty() {
                        log::debug!(
                            "Could not find transfer details in events for tx '{}', skipping it",
                            &tx_hash
                        );
                        continue;
                    }

                    let fee_details = try_or_continue!(
                        get_fee_details(deserialized_tx.auth_info.fee, coin),
                        "get_fee_details failed"
                    );

                    let mut fee_added = false;
                    let wallet_id = coin.history_wallet_id();
                    for (index, transfer_details) in transfer_details_list.iter().enumerate() {
                        let mut internal_id_hash = index.to_le_bytes().to_vec();
                        internal_id_hash.extend_from_slice(tx_hash.as_bytes());

                        let len = internal_id_hash.len();

                        // TODO: Remove this block at Q3 2025.
                        {
                            let old_internal_id_hash: [u8; 32] = internal_id_hash
                                .get(..32)
                                .and_then(|slice| slice.try_into().ok())
                                .unwrap_or_default();

                            let old_internal_id: BytesJson =
                                H256::from(old_internal_id_hash).reversed().to_vec().into();
                            let old_internal_id_for_fees: BytesJson = H256::from(old_internal_id_hash).to_vec().into();

                            for id in [old_internal_id, old_internal_id_for_fees] {
                                if let Ok(Some(_)) = storage.get_tx_from_history(&wallet_id, &id).await {
                                    if let Err(e) = storage.remove_tx_from_history(&wallet_id, &id).await {
                                        log::debug!("Failed to remove old transaction history record. {e:?}");
                                    };
                                }
                            }
                        }

                        let internal_id_hash: [u8; 33] = match internal_id_hash
                            .get(..33)
                            .and_then(|slice| slice.try_into().ok())
                        {
                            Some(hash) => hash,
                            None => {
                                log::debug!(
                                    "Invalid internal_id_hash length for tx '{}' at index {}: expected 32 bytes, got {} bytes.",
                                    tx_hash,
                                    index,
                                    len
                                );
                                continue;
                            },
                        };

                        let internal_id = internal_id_hash.iter().rev().copied().collect::<Vec<_>>().into();

                        if let Ok(Some(_)) = storage
                            .get_tx_from_history(&coin.history_wallet_id(), &internal_id)
                            .await
                        {
                            log::debug!("Tx '{}' already exists in tx_history. Skipping it.", &tx_hash);
                            continue;
                        }

                        let tx_sent_by_me = address == transfer_details.from;
                        let is_platform_coin_tx = transfer_details.ticker == *coin.platform_ticker();
                        let is_self_tx = transfer_details.to == transfer_details.from && tx_sent_by_me;
                        let is_sign_claim_htlc = tx_sent_by_me
                            && matches!(transfer_details.transfer_event_type, TransferEventType::ClaimHtlc);

                        let (from, to) =
                            some_or_continue!(get_pair_addresses(address.clone(), tx_sent_by_me, transfer_details));

                        let maybe_add_fees = if !fee_added
                        // if tx is platform coin tx and sent by me
                            && is_platform_coin_tx && tx_sent_by_me
                        {
                            fee_added = true;
                            Some(&fee_details)
                        } else {
                            None
                        };

                        let tx_amounts = get_tx_amounts(
                            transfer_details,
                            is_self_tx,
                            tx_sent_by_me,
                            is_sign_claim_htlc,
                            maybe_add_fees,
                        );

                        let token_id: Option<BytesJson> = match !is_platform_coin_tx {
                            true => {
                                let denom_hash = sha256(transfer_details.denom.to_lowercase().as_bytes());
                                Some(H256::from(denom_hash.take()).to_vec().into())
                            },
                            false => None,
                        };

                        let transaction_type = get_transaction_type(
                            &transfer_details.transfer_event_type,
                            token_id.clone(),
                            is_sign_claim_htlc,
                        );

                        let details = TransactionDetails {
                            from,
                            to,
                            total_amount: tx_amounts.total,
                            spent_by_me: tx_amounts.spent_by_me,
                            received_by_me: tx_amounts.received_by_me,
                            // This can be 0 since it gets remapped in `coins::my_tx_history_v2`
                            my_balance_change: BigDecimal::default(),
                            tx: TransactionData::new_signed(msg.into(), tx_hash.to_string()),
                            fee_details: Some(TxFeeDetails::Tendermint(fee_details.clone())),
                            block_height: tx.height.into(),
                            coin: transfer_details.ticker.clone(),
                            internal_id,
                            timestamp,
                            kmd_rewards: None,
                            transaction_type,
                            memo: Some(deserialized_tx.body.memo.clone()),
                        };
                        tx_details.push(details.clone());

                        // Display fees as extra transactions for asset txs sent by user
                        if tx_sent_by_me && !fee_added && !is_platform_coin_tx {
                            let fee_details = fee_details.clone();
                            let mut fee_tx_details = details;
                            fee_tx_details.to = vec![];
                            fee_tx_details.total_amount = fee_details.amount.clone();
                            fee_tx_details.spent_by_me = fee_details.amount.clone();
                            fee_tx_details.received_by_me = BigDecimal::default();
                            fee_tx_details.my_balance_change = BigDecimal::default() - &fee_details.amount;
                            fee_tx_details.coin = fee_details.coin.clone();
                            // Non-reversed version of original internal id
                            fee_tx_details.internal_id = internal_id_hash.to_vec().into();
                            fee_tx_details.transaction_type = TransactionType::FeeForTokenTx;

                            tx_details.push(fee_tx_details);
                            fee_added = true;
                        }
                    }

                    log::debug!("Tx '{}' successfully parsed.", tx.hash);
                }

                streaming_manager
                    .send_fn(&TxHistoryEventStreamer::derive_streamer_id(coin.ticker()), || {
                        tx_details.clone()
                    })
                    .ok();

                try_or_return_stopped_as_err!(
                    storage
                        .add_transactions_to_history(&coin.history_wallet_id(), tx_details)
                        .await
                        .map_err(|e| format!("{e:?}")),
                    StopReason::StorageError,
                    "add_transactions_to_history failed"
                );

                if (*page * TX_PAGE_SIZE as u32) >= response.total_count {
                    // if last page is full, we can start with next page on next iteration
                    if current_page_is_full {
                        *page += 1;
                    }
                    break Ok(highest_height);
                }
                *page += 1;
            }
        }

        let q = format!("coin_spent.spender = '{}'", self.address);
        let highest_send_tx_height = match fetch_and_insert_txs(
            self.address.clone(),
            &ctx.coin,
            &ctx.storage,
            &ctx.streaming_manager,
            q,
            self.from_block_height,
            &mut ctx.last_spent_page,
        )
        .await
        {
            Ok(block) => block,
            Err(stopped) => {
                if let StopReason::RpcClient(e) = &stopped.stop_reason {
                    log::error!("Sent tx history process turned into cooldown mode due to rpc error: {e}");
                    return Self::change_state(OnIoErrorCooldown::new(self.address.clone(), self.from_block_height));
                }

                return Self::change_state(stopped);
            },
        };

        let q = format!("coin_received.receiver = '{}'", self.address);
        let highest_received_tx_height = match fetch_and_insert_txs(
            self.address.clone(),
            &ctx.coin,
            &ctx.storage,
            &ctx.streaming_manager,
            q,
            self.from_block_height,
            &mut ctx.last_received_page,
        )
        .await
        {
            Ok(block) => block,
            Err(stopped) => {
                if let StopReason::RpcClient(e) = &stopped.stop_reason {
                    log::error!("Received tx history process turned into cooldown mode due to rpc error: {e}");
                    return Self::change_state(OnIoErrorCooldown::new(self.address.clone(), self.from_block_height));
                }

                return Self::change_state(stopped);
            },
        };

        let last_fetched_block = cmp::max(highest_send_tx_height, highest_received_tx_height);

        log::info!(
            "Tx history fetching finished for {}. Last fetched block {}",
            ctx.coin.platform_ticker(),
            last_fetched_block
        );

        ctx.coin.set_history_sync_state(HistorySyncState::Finished);
        Self::change_state(WaitForHistoryUpdateTrigger::new(
            self.address.clone(),
            last_fetched_block,
        ))
    }
}

#[async_trait]
impl<Coin, Storage> State for TendermintInit<Coin, Storage>
where
    Coin: CoinCapabilities,
    Storage: TxHistoryStorage,
{
    type StateMachine = TendermintTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(
        self: Box<Self>,
        ctx: &mut TendermintTxHistoryStateMachine<Coin, Storage>,
    ) -> StateResult<TendermintTxHistoryStateMachine<Coin, Storage>> {
        const INITIAL_SEARCH_HEIGHT: u64 = 0;

        ctx.coin.set_history_sync_state(HistorySyncState::NotStarted);

        if let Err(e) = ctx.storage.init(&ctx.coin.history_wallet_id()).await {
            return Self::change_state(Stopped::storage_error(e));
        }

        let search_from = match ctx
            .storage
            .get_highest_block_height(&ctx.coin.history_wallet_id())
            .await
        {
            Ok(Some(height)) if height > 0 => height as u64 - 1,
            _ => INITIAL_SEARCH_HEIGHT,
        };

        Self::change_state(FetchingTransactionsData::new(
            ctx.coin.my_address().expect("my_address can't fail"),
            search_from,
        ))
    }
}

#[async_trait]
impl<Coin, Storage> LastState for Stopped<Coin, Storage>
where
    Coin: CoinCapabilities,
    Storage: TxHistoryStorage,
{
    type StateMachine = TendermintTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(self: Box<Self>, ctx: &mut TendermintTxHistoryStateMachine<Coin, Storage>) -> () {
        log::info!(
            "Stopping tx history fetching for {}. Reason: {:?}",
            ctx.coin.ticker(),
            self.stop_reason
        );

        let new_state_json = json!({
            "message": format!("{:?}", self.stop_reason),
        });

        ctx.coin.set_history_sync_state(HistorySyncState::Error(new_state_json));
    }
}

/// Find, decode (if needed) and return the event attribute value.
///
/// If the attribute doesn't exist, or decoding fails, `None` will be returned.
fn get_value_from_event_attributes(events: &[EventAttribute], tag: &str, base64_encoded_tag: &str) -> Option<String> {
    let event_attribute = events
        .iter()
        .find(|attribute| attribute.key == tag || attribute.key == base64_encoded_tag)?;

    if event_attribute.key == base64_encoded_tag {
        let decoded_bytes = base64::engine::general_purpose::STANDARD
            .decode(event_attribute.value.clone())
            .ok()?;
        String::from_utf8(decoded_bytes).ok()
    } else {
        Some(event_attribute.value.clone())
    }
}

pub async fn tendermint_history_loop(
    coin: TendermintCoin,
    storage: impl TxHistoryStorage,
    ctx: MmArc,
    _current_balance: Option<BigDecimal>,
) {
    let balances = match coin.get_all_balances().await {
        Ok(balances) => balances,
        Err(e) => {
            log::error!("{}", e);
            return;
        },
    };

    let mut state_machine = TendermintTxHistoryStateMachine {
        coin,
        storage,
        streaming_manager: ctx.event_stream_manager.clone(),
        balances,
        last_received_page: 1,
        last_spent_page: 1,
    };

    state_machine
        .run(Box::new(TendermintInit::new()))
        .await
        .expect("The error of this machine is Infallible");
}

#[cfg(any(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use common::cross_test;

    common::cfg_wasm32! {
        use wasm_bindgen_test::*;
        wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
    }

    cross_test!(test_get_value_from_event_attributes, {
        let attributes = vec![
            EventAttribute {
                key: "recipient".to_owned(),
                value: "nuc1erfnkjsmalkwtvj44qnfr2drfzdt4n9ledw63y".to_owned(),
                index: false,
            },
            EventAttribute {
                key: "sender".to_owned(),
                value: "nuc1a7xynj4ceft8kgdjr6kcq0s07y3ccya60rqwwn".to_owned(),
                index: false,
            },
            EventAttribute {
                key: "amount".to_owned(),
                value: "8000ibc/F7F28FF3C09024A0225EDBBDB207E5872D2B4EF2FB874FE47B05EF9C9A7D211C".to_owned(),
                index: false,
            },
        ];

        let value = get_value_from_event_attributes(&attributes, "invalid", "");
        assert_eq!(value, None);
        let value = get_value_from_event_attributes(&attributes, RECIPIENT_TAG_KEY, RECIPIENT_TAG_KEY_BASE64).unwrap();
        assert_eq!(value, "nuc1erfnkjsmalkwtvj44qnfr2drfzdt4n9ledw63y");
        let value = get_value_from_event_attributes(&attributes, SENDER_TAG_KEY, SENDER_TAG_KEY_BASE64).unwrap();
        assert_eq!(value, "nuc1a7xynj4ceft8kgdjr6kcq0s07y3ccya60rqwwn");
        let value = get_value_from_event_attributes(&attributes, AMOUNT_TAG_KEY, AMOUNT_TAG_KEY_BASE64).unwrap();
        assert_eq!(
            value,
            "8000ibc/F7F28FF3C09024A0225EDBBDB207E5872D2B4EF2FB874FE47B05EF9C9A7D211C"
        );

        let encoded_attributes = vec![
            EventAttribute {
                key: "cmVjaXBpZW50".to_owned(),
                value: "bnVjMTd4cGZ2YWttMmFtZzk2MnlsczZmODR6M2tlbGw4YzVsM3B6YTJ5".to_owned(),
                index: true,
            },
            EventAttribute {
                key: "c2VuZGVy".to_owned(),
                value: "bnVjMWE3eHluajRjZWZ0OGtnZGpyNmtjcTBzMDd5M2NjeWE2MHJxd3du".to_owned(),
                index: true,
            },
            EventAttribute {
                key: "YW1vdW50".to_owned(),
                value: "MjcxNjJ1bnVjbA==".to_owned(),
                index: true,
            },
        ];

        let value = get_value_from_event_attributes(&encoded_attributes, "invalid", "");
        assert_eq!(value, None);
        let value =
            get_value_from_event_attributes(&encoded_attributes, RECIPIENT_TAG_KEY, RECIPIENT_TAG_KEY_BASE64).unwrap();
        assert_eq!(value, "nuc17xpfvakm2amg962yls6f84z3kell8c5l3pza2y");
        let value =
            get_value_from_event_attributes(&encoded_attributes, SENDER_TAG_KEY, SENDER_TAG_KEY_BASE64).unwrap();
        assert_eq!(value, "nuc1a7xynj4ceft8kgdjr6kcq0s07y3ccya60rqwwn");
        let value =
            get_value_from_event_attributes(&encoded_attributes, AMOUNT_TAG_KEY, AMOUNT_TAG_KEY_BASE64).unwrap();
        assert_eq!(value, "27162unucl");

        let invalid_attributes = vec![
            EventAttribute {
                key: String::default(),
                value: String::default(),
                index: true,
            },
            EventAttribute {
                key: "invalid-key".to_owned(),
                value: String::default(),
                index: true,
            },
            EventAttribute {
                key: "dummy-key".to_owned(),
                value: String::default(),
                index: true,
            },
        ];

        let value = get_value_from_event_attributes(&invalid_attributes, RECIPIENT_TAG_KEY, RECIPIENT_TAG_KEY_BASE64);
        assert_eq!(value, None);
        let value = get_value_from_event_attributes(&invalid_attributes, SENDER_TAG_KEY, SENDER_TAG_KEY_BASE64);
        assert_eq!(value, None);
        let value = get_value_from_event_attributes(&invalid_attributes, AMOUNT_TAG_KEY, AMOUNT_TAG_KEY_BASE64);
        assert_eq!(value, None);
    });
}
