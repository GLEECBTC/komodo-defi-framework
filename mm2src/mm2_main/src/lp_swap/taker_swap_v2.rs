use super::swap_events::{SwapStatusEvent, SwapStatusStreamer};
use super::swap_v2_common::*;
use super::{
    LockedAmount, LockedAmountInfo, SavedTradeFee, SwapsContext, TakerSwapPreparedParams, NEGOTIATE_SEND_INTERVAL,
    NEGOTIATION_TIMEOUT_SEC,
};
use crate::lp_swap::swap_lock::SwapLock;
use crate::lp_swap::swap_v2_pb::*;
use crate::lp_swap::{
    broadcast_swap_v2_msg_every, check_balance_for_taker_swap, recv_swap_v2_msg, swap_v2_topic,
    SwapConfirmationsSettings, TransactionIdentifier, MAX_STARTED_AT_DIFF, TAKER_SWAP_V2_TYPE,
};
use async_trait::async_trait;
use bitcrypto::{dhash160, sha256};
use coins::hd_wallet::AddrToString;
use coins::{
    CanRefundHtlc, ConfirmPaymentInput, DexFee, FeeApproxStage, GenTakerFundingSpendArgs, GenTakerPaymentSpendArgs,
    MakerCoinSwapOpsV2, MmCoin, ParseCoinAssocTypes, RefundFundingSecretArgs, RefundTakerPaymentArgs,
    SendTakerFundingArgs, SpendMakerPaymentArgs, SwapTxTypeWithSecretHash, TakerCoinSwapOpsV2, ToBytes, TradeFee,
    TradePreimageValue, Transaction, TxPreimageWithSig, ValidateMakerPaymentArgs,
};
use common::executor::abortable_queue::AbortableQueue;
use common::executor::{AbortableSystem, Timer};
use common::log::{debug, error, info, warn};
use common::Future01CompatExt;
use crypto::privkey::SerializableSecp256k1Keypair;
use crypto::secret_hash_algo::SecretHashAlgo;
use derive_more::Display;
use keys::KeyPair;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_event_stream::DeriveStreamerId;
use mm2_libp2p::Secp256k1PubkeySerialize;
use mm2_number::MmNumber;
use mm2_state_machine::prelude::*;
use mm2_state_machine::storable_state_machine::*;
use primitives::hash::H256;
use rpc::v1::types::{Bytes as BytesJson, H256 as H256Json};
use secp256k1::PublicKey;
use std::convert::TryInto;
use std::marker::PhantomData;
use uuid::Uuid;

cfg_native!(
    use crate::database::my_swaps::{insert_new_swap_v2, SELECT_MY_SWAP_V2_BY_UUID};
    use common::async_blocking;
    use db_common::sqlite::rusqlite::{named_params, Error as SqlError, Result as SqlResult, Row};
    use db_common::sqlite::rusqlite::types::Type as SqlType;
);

cfg_wasm32!(
    use crate::lp_swap::swap_wasm_db::{MySwapsFiltersTable, SavedSwapTable};
    use crate::swap_versioning::legacy_swap_version;
);

// This is needed to have Debug on messages
#[allow(unused_imports)]
use prost::Message;

/// Negotiation data representation to be stored in DB.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredNegotiationData {
    maker_payment_locktime: u64,
    maker_secret_hash: BytesJson,
    taker_coin_maker_address: String,
    maker_coin_htlc_pub_from_maker: BytesJson,
    taker_coin_htlc_pub_from_maker: BytesJson,
    maker_coin_swap_contract: Option<BytesJson>,
    taker_coin_swap_contract: Option<BytesJson>,
}

/// Represents events produced by taker swap states.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "event_type", content = "event_data")]
pub enum TakerSwapEvent {
    /// Swap has been successfully initialized.
    Initialized {
        maker_coin_start_block: u64,
        taker_coin_start_block: u64,
        taker_payment_fee: SavedTradeFee,
        maker_payment_spend_fee: SavedTradeFee,
    },
    /// Negotiated swap data with maker.
    Negotiated {
        maker_coin_start_block: u64,
        taker_coin_start_block: u64,
        negotiation_data: StoredNegotiationData,
        taker_payment_fee: SavedTradeFee,
        maker_payment_spend_fee: SavedTradeFee,
    },
    /// Sent taker funding tx.
    TakerFundingSent {
        maker_coin_start_block: u64,
        taker_coin_start_block: u64,
        negotiation_data: StoredNegotiationData,
        taker_funding: TransactionIdentifier,
    },
    /// Taker funding tx refund is required.
    TakerFundingRefundRequired {
        maker_coin_start_block: u64,
        taker_coin_start_block: u64,
        negotiation_data: StoredNegotiationData,
        taker_funding: TransactionIdentifier,
        reason: TakerFundingRefundReason,
    },
    /// Received maker payment and taker funding spend preimage
    MakerPaymentAndFundingSpendPreimgReceived {
        maker_coin_start_block: u64,
        taker_coin_start_block: u64,
        negotiation_data: StoredNegotiationData,
        taker_funding: TransactionIdentifier,
        funding_spend_preimage: StoredTxPreimage,
        maker_payment: TransactionIdentifier,
    },
    /// Sent taker payment.
    TakerPaymentSent {
        maker_coin_start_block: u64,
        taker_coin_start_block: u64,
        taker_payment: TransactionIdentifier,
        maker_payment: TransactionIdentifier,
        negotiation_data: StoredNegotiationData,
    },
    /// 'Taker payment`' was sent and preimage of 'taker payment spend' was skipped.
    TakerPaymentSentAndPreimageSendingSkipped {
        maker_coin_start_block: u64,
        taker_coin_start_block: u64,
        taker_payment: TransactionIdentifier,
        maker_payment: TransactionIdentifier,
        negotiation_data: StoredNegotiationData,
    },
    /// Something went wrong, so taker payment refund is required.
    TakerPaymentRefundRequired {
        taker_payment: TransactionIdentifier,
        negotiation_data: StoredNegotiationData,
        reason: TakerPaymentRefundReason,
    },
    /// Maker payment is confirmed on-chain
    MakerPaymentConfirmed {
        maker_coin_start_block: u64,
        taker_coin_start_block: u64,
        maker_payment: TransactionIdentifier,
        taker_funding: TransactionIdentifier,
        funding_spend_preimage: StoredTxPreimage,
        negotiation_data: StoredNegotiationData,
    },
    /// Maker spent taker's payment and taker discovered the tx on-chain.
    TakerPaymentSpent {
        maker_coin_start_block: u64,
        taker_coin_start_block: u64,
        maker_payment: TransactionIdentifier,
        taker_payment: TransactionIdentifier,
        taker_payment_spend: TransactionIdentifier,
        negotiation_data: StoredNegotiationData,
    },
    /// Taker spent maker's payment.
    MakerPaymentSpent {
        maker_coin_start_block: u64,
        taker_coin_start_block: u64,
        maker_payment: TransactionIdentifier,
        taker_payment: TransactionIdentifier,
        taker_payment_spend: TransactionIdentifier,
        maker_payment_spend: TransactionIdentifier,
        negotiation_data: StoredNegotiationData,
    },
    /// Swap has been finished with taker funding tx refund
    TakerFundingRefunded {
        funding_tx: TransactionIdentifier,
        funding_tx_refund: TransactionIdentifier,
        reason: TakerFundingRefundReason,
    },
    /// Swap has been finished with taker payment tx refund
    TakerPaymentRefunded {
        taker_payment: TransactionIdentifier,
        taker_payment_refund: TransactionIdentifier,
        reason: TakerPaymentRefundReason,
    },
    /// Swap has been aborted before taker payment was sent.
    Aborted { reason: AbortReason },
    /// Swap completed successfully.
    Completed,
}

/// Storage for taker swaps.
#[derive(Clone)]
pub struct TakerSwapStorage {
    ctx: MmArc,
}

impl TakerSwapStorage {
    pub fn new(ctx: MmArc) -> Self {
        TakerSwapStorage { ctx }
    }
}

#[async_trait]
impl StateMachineStorage for TakerSwapStorage {
    type MachineId = Uuid;
    type DbRepr = TakerSwapDbRepr;
    type Error = MmError<SwapStateMachineError>;

    #[cfg(not(target_arch = "wasm32"))]
    async fn store_repr(&mut self, _id: Self::MachineId, repr: Self::DbRepr) -> Result<(), Self::Error> {
        let ctx = self.ctx.clone();

        async_blocking(move || {
            let sql_params = named_params! {
                ":my_coin": repr.taker_coin,
                ":other_coin": repr.maker_coin,
                ":uuid": repr.uuid.to_string(),
                ":started_at": repr.started_at,
                ":swap_type": TAKER_SWAP_V2_TYPE,
                ":maker_volume": repr.maker_volume.to_fraction_string(),
                ":taker_volume": repr.taker_volume.to_fraction_string(),
                ":premium": repr.taker_premium.to_fraction_string(),
                ":dex_fee": repr.dex_fee_amount.to_fraction_string(),
                ":dex_fee_burn": repr.dex_fee_burn.to_fraction_string(),
                ":secret": repr.taker_secret.0,
                ":secret_hash": repr.taker_secret_hash.0,
                ":secret_hash_algo": repr.secret_hash_algo as u8,
                ":p2p_privkey": repr.p2p_keypair.map(|k| k.priv_key()).unwrap_or_default(),
                ":lock_duration": repr.lock_duration,
                ":maker_coin_confs": repr.conf_settings.maker_coin_confs,
                ":maker_coin_nota": repr.conf_settings.maker_coin_nota,
                ":taker_coin_confs": repr.conf_settings.taker_coin_confs,
                ":taker_coin_nota": repr.conf_settings.taker_coin_nota,
                ":other_p2p_pub": repr.maker_p2p_pub.to_bytes(),
                ":swap_version": repr.swap_version,
            };
            insert_new_swap_v2(&ctx, sql_params)?;
            Ok(())
        })
        .await
    }

    #[cfg(target_arch = "wasm32")]
    async fn store_repr(&mut self, uuid: Self::MachineId, repr: Self::DbRepr) -> Result<(), Self::Error> {
        let swaps_ctx = SwapsContext::from_ctx(&self.ctx).expect("SwapsContext::from_ctx should not fail");
        let db = swaps_ctx.swap_db().await.map_mm_err()?;
        let transaction = db.transaction().await.map_mm_err()?;

        let filters_table = transaction.table::<MySwapsFiltersTable>().await.map_mm_err()?;

        let item = MySwapsFiltersTable {
            uuid,
            my_coin: repr.taker_coin.clone(),
            other_coin: repr.maker_coin.clone(),
            started_at: repr.started_at as u32,
            is_finished: false.into(),
            swap_type: TAKER_SWAP_V2_TYPE,
        };
        filters_table.add_item(&item).await.map_mm_err()?;

        let table = transaction.table::<SavedSwapTable>().await.map_mm_err()?;
        let item = SavedSwapTable {
            uuid,
            saved_swap: serde_json::to_value(repr)?,
        };
        table.add_item(&item).await.map_mm_err()?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn get_repr(&self, id: Self::MachineId) -> Result<Self::DbRepr, Self::Error> {
        let ctx = self.ctx.clone();
        let id_str = id.to_string();

        async_blocking(move || {
            Ok(ctx.sqlite_connection().query_row(
                SELECT_MY_SWAP_V2_BY_UUID,
                &[(":uuid", &id_str)],
                TakerSwapDbRepr::from_sql_row,
            )?)
        })
        .await
    }

    #[cfg(target_arch = "wasm32")]
    async fn get_repr(&self, id: Self::MachineId) -> Result<Self::DbRepr, Self::Error> {
        get_swap_repr(&self.ctx, id).await
    }

    async fn has_record_for(&mut self, id: &Self::MachineId) -> Result<bool, Self::Error> {
        has_db_record_for(self.ctx.clone(), id).await
    }

    async fn store_event(&mut self, id: Self::MachineId, event: TakerSwapEvent) -> Result<(), Self::Error> {
        store_swap_event::<TakerSwapDbRepr>(self.ctx.clone(), id, event).await
    }

    async fn get_unfinished(&self) -> Result<Vec<Self::MachineId>, Self::Error> {
        get_unfinished_swaps_uuids(self.ctx.clone(), TAKER_SWAP_V2_TYPE).await
    }

    async fn mark_finished(&mut self, id: Self::MachineId) -> Result<(), Self::Error> {
        mark_swap_as_finished(self.ctx.clone(), id).await
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TakerSwapDbRepr {
    /// Maker coin
    pub maker_coin: String,
    /// The amount swapped by maker.
    pub maker_volume: MmNumber,
    /// The secret used in taker funding immediate refund path.
    pub taker_secret: H256Json,
    /// The hash of taker's secret.
    pub taker_secret_hash: BytesJson,
    /// Algorithm used to hash the swap secret.
    pub secret_hash_algo: SecretHashAlgo,
    /// The timestamp when the swap was started.
    pub started_at: u64,
    /// The duration of HTLC timelock in seconds.
    pub lock_duration: u64,
    /// Taker coin
    pub taker_coin: String,
    /// The amount swapped by taker.
    pub taker_volume: MmNumber,
    /// Premium amount, which might be paid to maker as an additional reward.
    pub taker_premium: MmNumber,
    /// DEX fee amount
    pub dex_fee_amount: MmNumber,
    /// DEX fee burn amount
    pub dex_fee_burn: MmNumber,
    /// Swap transactions' confirmations settings
    pub conf_settings: SwapConfirmationsSettings,
    /// UUID of the swap
    pub uuid: Uuid,
    /// If Some, used to sign P2P messages of this swap.
    pub p2p_keypair: Option<SerializableSecp256k1Keypair>,
    /// Swap events
    pub events: Vec<TakerSwapEvent>,
    /// Maker's P2P pubkey
    pub maker_p2p_pub: Secp256k1PubkeySerialize,
    /// Swap protocol version
    #[cfg_attr(target_arch = "wasm32", serde(default = "legacy_swap_version"))]
    pub swap_version: u8,
}

#[cfg(not(target_arch = "wasm32"))]
impl TakerSwapDbRepr {
    fn from_sql_row(row: &Row) -> SqlResult<Self> {
        Ok(TakerSwapDbRepr {
            taker_coin: row.get(0)?,
            maker_coin: row.get(1)?,
            uuid: row
                .get::<_, String>(2)?
                .parse()
                .map_err(|e| SqlError::FromSqlConversionFailure(2, SqlType::Text, Box::new(e)))?,
            started_at: row.get(3)?,
            taker_secret: row.get::<_, [u8; 32]>(4)?.into(),
            taker_secret_hash: row.get::<_, Vec<u8>>(5)?.into(),
            secret_hash_algo: row
                .get::<_, u8>(6)?
                .try_into()
                .map_err(|e| SqlError::FromSqlConversionFailure(6, SqlType::Integer, Box::new(e)))?,
            events: serde_json::from_str(&row.get::<_, String>(7)?)
                .map_err(|e| SqlError::FromSqlConversionFailure(7, SqlType::Text, Box::new(e)))?,
            maker_volume: MmNumber::from_fraction_string(&row.get::<_, String>(8)?)
                .map_err(|e| SqlError::FromSqlConversionFailure(8, SqlType::Text, Box::new(e)))?,
            taker_volume: MmNumber::from_fraction_string(&row.get::<_, String>(9)?)
                .map_err(|e| SqlError::FromSqlConversionFailure(9, SqlType::Text, Box::new(e)))?,
            taker_premium: MmNumber::from_fraction_string(&row.get::<_, String>(10)?)
                .map_err(|e| SqlError::FromSqlConversionFailure(10, SqlType::Text, Box::new(e)))?,
            dex_fee_amount: MmNumber::from_fraction_string(&row.get::<_, String>(11)?)
                .map_err(|e| SqlError::FromSqlConversionFailure(11, SqlType::Text, Box::new(e)))?,
            dex_fee_burn: MmNumber::from_fraction_string(&row.get::<_, String>(12)?)
                .map_err(|e| SqlError::FromSqlConversionFailure(12, SqlType::Text, Box::new(e)))?,
            lock_duration: row.get(13)?,
            conf_settings: SwapConfirmationsSettings {
                maker_coin_confs: row.get(14)?,
                maker_coin_nota: row.get(15)?,
                taker_coin_confs: row.get(16)?,
                taker_coin_nota: row.get(17)?,
            },
            p2p_keypair: row.get::<_, [u8; 32]>(18).and_then(|maybe_key| {
                if maybe_key == [0; 32] {
                    Ok(None)
                } else {
                    Ok(Some(SerializableSecp256k1Keypair::new(maybe_key).map_err(|e| {
                        SqlError::FromSqlConversionFailure(18, SqlType::Blob, Box::new(e))
                    })?))
                }
            })?,
            maker_p2p_pub: row
                .get::<_, Vec<u8>>(19)
                .and_then(|maybe_public| {
                    PublicKey::from_slice(&maybe_public)
                        .map_err(|e| SqlError::FromSqlConversionFailure(19, SqlType::Blob, Box::new(e)))
                })?
                .into(),
            swap_version: row.get(20)?,
        })
    }
}

impl StateMachineDbRepr for TakerSwapDbRepr {
    type Event = TakerSwapEvent;

    fn add_event(&mut self, event: Self::Event) {
        self.events.push(event)
    }
}

impl GetSwapCoins for TakerSwapDbRepr {
    fn maker_coin(&self) -> &str {
        &self.maker_coin
    }

    fn taker_coin(&self) -> &str {
        &self.taker_coin
    }
}

/// Represents the state machine for taker's side of the Trading Protocol Upgrade swap (v2).
pub struct TakerSwapStateMachine<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> {
    /// MM2 context.
    pub ctx: MmArc,
    /// Storage.
    pub storage: TakerSwapStorage,
    /// The timestamp when the swap was started.
    pub started_at: u64,
    /// The duration of HTLC timelock in seconds.
    pub lock_duration: u64,
    /// The coin type the Maker uses, but owned by the Taker in the trade.
    /// This coin is required by the Taker to complete the swap.
    pub maker_coin: MakerCoin,
    /// The amount swapped by maker.
    pub maker_volume: MmNumber,
    /// The coin type the Taker uses in the trade.
    /// This is the coin the Taker offers and manages in the state machine.
    pub taker_coin: TakerCoin,
    /// The amount swapped by taker.
    pub taker_volume: MmNumber,
    /// Premium amount, which might be paid to maker as additional reward.
    pub taker_premium: MmNumber,
    /// Algorithm used to hash swap secrets.
    pub secret_hash_algo: SecretHashAlgo,
    /// Swap transactions' confirmations settings.
    pub conf_settings: SwapConfirmationsSettings,
    /// UUID of the swap.
    pub uuid: Uuid,
    /// The gossipsub topic used for peer-to-peer communication in swap process.
    pub p2p_topic: String,
    /// If Some, used to sign P2P messages of this swap.
    pub p2p_keypair: Option<KeyPair>,
    /// The secret used for immediate taker funding tx reclaim if maker back-outs
    pub taker_secret: H256,
    /// Abortable queue used to spawn related activities
    pub abortable_system: AbortableQueue,
    /// Maker's P2P pubkey
    pub maker_p2p_pubkey: PublicKey,
    /// Whether to require maker payment confirmation before transferring funding tx to payment
    pub require_maker_payment_confirm_before_funding_spend: bool,
    /// Determines if the maker payment spend transaction must be confirmed before marking swap as Completed.
    pub require_maker_payment_spend_confirm: bool,
    /// Swap protocol version
    pub swap_version: u8,
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2>
    TakerSwapStateMachine<MakerCoin, TakerCoin>
{
    #[inline]
    fn maker_payment_conf_timeout(&self) -> u64 {
        self.started_at + self.lock_duration / 3
    }

    #[inline]
    fn taker_funding_locktime(&self) -> u64 {
        self.started_at + self.lock_duration * 3
    }

    #[inline]
    fn taker_payment_locktime(&self) -> u64 {
        self.started_at + self.lock_duration
    }

    fn unique_data(&self) -> Vec<u8> {
        self.uuid.as_bytes().to_vec()
    }

    /// Returns secret hash generated using selected [SecretHashAlgo].
    fn taker_secret_hash(&self) -> Vec<u8> {
        match self.secret_hash_algo {
            SecretHashAlgo::DHASH160 => dhash160(self.taker_secret.as_slice()).take().into(),
            SecretHashAlgo::SHA256 => sha256(self.taker_secret.as_slice()).take().into(),
        }
    }

    fn dex_fee(&self) -> DexFee {
        if self.taker_coin.ticker() == "KMD" || self.maker_coin.ticker() == "KMD" {
            // Set DexFee::NoFee for swaps with KMD coin.
            return DexFee::NoFee;
        }

        if let Some(taker_pub) = self.taker_coin.taker_pubkey_bytes() {
            // for dex fee calculation we need only permanent (non-derived for HTLC) taker pubkey here
            DexFee::new_with_taker_pubkey(
                &self.taker_coin,
                self.maker_coin.ticker(),
                &self.taker_volume,
                taker_pub.as_slice(),
            )
        } else {
            // Return max dex fee (if taker_pub is not known yet)
            DexFee::new_from_taker_coin(&self.taker_coin, self.maker_coin.ticker(), &self.taker_volume)
        }
    }
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableStateMachine
    for TakerSwapStateMachine<MakerCoin, TakerCoin>
{
    type Storage = TakerSwapStorage;
    type Result = ();
    type Error = MmError<SwapStateMachineError>;
    type ReentrancyLock = SwapLock;
    type RecreateCtx = SwapRecreateCtx<MakerCoin, TakerCoin>;
    type RecreateError = MmError<SwapRecreateError>;

    fn to_db_repr(&self) -> TakerSwapDbRepr {
        TakerSwapDbRepr {
            maker_coin: self.maker_coin.ticker().into(),
            maker_volume: self.maker_volume.clone(),
            taker_secret: self.taker_secret.into(),
            taker_secret_hash: self.taker_secret_hash().into(),
            secret_hash_algo: self.secret_hash_algo,
            started_at: self.started_at,
            lock_duration: self.lock_duration,
            taker_coin: self.taker_coin.ticker().into(),
            taker_volume: self.taker_volume.clone(),
            taker_premium: self.taker_premium.clone(),
            dex_fee_amount: self.dex_fee().fee_amount(),
            conf_settings: self.conf_settings,
            uuid: self.uuid,
            p2p_keypair: self.p2p_keypair.map(Into::into),
            events: Vec::new(),
            maker_p2p_pub: self.maker_p2p_pubkey.into(),
            dex_fee_burn: self.dex_fee().burn_amount().unwrap_or_default(),
            swap_version: self.swap_version,
        }
    }

    fn storage(&mut self) -> &mut Self::Storage {
        &mut self.storage
    }

    fn id(&self) -> <Self::Storage as StateMachineStorage>::MachineId {
        self.uuid
    }

    async fn recreate_machine(
        uuid: Uuid,
        storage: TakerSwapStorage,
        mut repr: TakerSwapDbRepr,
        recreate_ctx: Self::RecreateCtx,
    ) -> Result<(RestoredMachine<Self>, Box<dyn RestoredState<StateMachine = Self>>), Self::RecreateError> {
        if repr.events.is_empty() {
            return MmError::err(SwapRecreateError::ReprEventsEmpty);
        }

        let current_state: Box<dyn RestoredState<StateMachine = Self>> = match repr.events.remove(repr.events.len() - 1)
        {
            TakerSwapEvent::Initialized {
                maker_coin_start_block,
                taker_coin_start_block,
                taker_payment_fee,
                maker_payment_spend_fee,
            } => Box::new(Initialized {
                maker_coin: Default::default(),
                taker_coin: Default::default(),
                maker_coin_start_block,
                taker_coin_start_block,
                taker_payment_fee,
                maker_payment_spend_fee,
            }),
            TakerSwapEvent::Negotiated {
                maker_coin_start_block,
                taker_coin_start_block,
                negotiation_data,
                taker_payment_fee,
                maker_payment_spend_fee,
            } => Box::new(Negotiated {
                maker_coin_start_block,
                taker_coin_start_block,
                negotiation_data: NegotiationData::from_stored_data(
                    negotiation_data,
                    &recreate_ctx.maker_coin,
                    &recreate_ctx.taker_coin,
                )?,
                taker_payment_fee,
                maker_payment_spend_fee,
            }),
            TakerSwapEvent::TakerFundingSent {
                maker_coin_start_block,
                taker_coin_start_block,
                negotiation_data,
                taker_funding,
            } => Box::new(TakerFundingSent {
                maker_coin_start_block,
                taker_coin_start_block,
                taker_funding: recreate_ctx
                    .taker_coin
                    .parse_tx(&taker_funding.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                negotiation_data: NegotiationData::from_stored_data(
                    negotiation_data,
                    &recreate_ctx.maker_coin,
                    &recreate_ctx.taker_coin,
                )?,
            }),
            TakerSwapEvent::TakerFundingRefundRequired {
                maker_coin_start_block,
                taker_coin_start_block,
                negotiation_data,
                taker_funding,
                reason,
            } => Box::new(TakerFundingRefundRequired {
                maker_coin_start_block,
                taker_coin_start_block,
                taker_funding: recreate_ctx
                    .taker_coin
                    .parse_tx(&taker_funding.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                negotiation_data: NegotiationData::from_stored_data(
                    negotiation_data,
                    &recreate_ctx.maker_coin,
                    &recreate_ctx.taker_coin,
                )?,
                reason,
            }),
            TakerSwapEvent::MakerPaymentAndFundingSpendPreimgReceived {
                maker_coin_start_block,
                taker_coin_start_block,
                negotiation_data,
                taker_funding,
                maker_payment,
                funding_spend_preimage,
            } => Box::new(MakerPaymentAndFundingSpendPreimgReceived {
                maker_coin_start_block,
                taker_coin_start_block,
                negotiation_data: NegotiationData::from_stored_data(
                    negotiation_data,
                    &recreate_ctx.maker_coin,
                    &recreate_ctx.taker_coin,
                )?,
                taker_funding: recreate_ctx
                    .taker_coin
                    .parse_tx(&taker_funding.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                funding_spend_preimage: TxPreimageWithSig {
                    preimage: recreate_ctx
                        .taker_coin
                        .parse_preimage(&funding_spend_preimage.preimage.0)
                        .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                    signature: recreate_ctx
                        .taker_coin
                        .parse_signature(&funding_spend_preimage.signature.0)
                        .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                },
                maker_payment: recreate_ctx
                    .maker_coin
                    .parse_tx(&maker_payment.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
            }),
            TakerSwapEvent::TakerPaymentSent {
                maker_coin_start_block,
                taker_coin_start_block,
                taker_payment,
                maker_payment,
                negotiation_data,
            } => Box::new(TakerPaymentSent {
                maker_coin_start_block,
                taker_coin_start_block,
                taker_payment: recreate_ctx
                    .taker_coin
                    .parse_tx(&taker_payment.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                maker_payment: recreate_ctx
                    .maker_coin
                    .parse_tx(&maker_payment.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                negotiation_data: NegotiationData::from_stored_data(
                    negotiation_data,
                    &recreate_ctx.maker_coin,
                    &recreate_ctx.taker_coin,
                )?,
            }),
            TakerSwapEvent::TakerPaymentSentAndPreimageSendingSkipped {
                maker_coin_start_block,
                taker_coin_start_block,
                taker_payment,
                maker_payment,
                negotiation_data,
            } => Box::new(TakerPaymentSentAndPreimageSendingSkipped {
                maker_coin_start_block,
                taker_coin_start_block,
                taker_payment: recreate_ctx
                    .taker_coin
                    .parse_tx(&taker_payment.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                maker_payment: recreate_ctx
                    .maker_coin
                    .parse_tx(&maker_payment.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                negotiation_data: NegotiationData::from_stored_data(
                    negotiation_data,
                    &recreate_ctx.maker_coin,
                    &recreate_ctx.taker_coin,
                )?,
            }),
            TakerSwapEvent::TakerPaymentRefundRequired {
                taker_payment,
                negotiation_data,
                reason,
            } => Box::new(TakerPaymentRefundRequired {
                taker_payment: recreate_ctx
                    .taker_coin
                    .parse_tx(&taker_payment.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                negotiation_data: NegotiationData::from_stored_data(
                    negotiation_data,
                    &recreate_ctx.maker_coin,
                    &recreate_ctx.taker_coin,
                )?,
                reason,
            }),
            TakerSwapEvent::MakerPaymentConfirmed {
                maker_coin_start_block,
                taker_coin_start_block,
                maker_payment,
                taker_funding,
                funding_spend_preimage,
                negotiation_data,
            } => Box::new(MakerPaymentConfirmed {
                maker_coin_start_block,
                taker_coin_start_block,
                maker_payment: recreate_ctx
                    .maker_coin
                    .parse_tx(&maker_payment.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                taker_funding: recreate_ctx
                    .taker_coin
                    .parse_tx(&taker_funding.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                funding_spend_preimage: TxPreimageWithSig {
                    preimage: recreate_ctx
                        .taker_coin
                        .parse_preimage(&funding_spend_preimage.preimage.0)
                        .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                    signature: recreate_ctx
                        .taker_coin
                        .parse_signature(&funding_spend_preimage.signature.0)
                        .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                },
                negotiation_data: NegotiationData::from_stored_data(
                    negotiation_data,
                    &recreate_ctx.maker_coin,
                    &recreate_ctx.taker_coin,
                )?,
            }),
            TakerSwapEvent::TakerPaymentSpent {
                maker_coin_start_block,
                taker_coin_start_block,
                maker_payment,
                taker_payment,
                taker_payment_spend,
                negotiation_data,
            } => Box::new(TakerPaymentSpent {
                maker_coin_start_block,
                taker_coin_start_block,
                maker_payment: recreate_ctx
                    .maker_coin
                    .parse_tx(&maker_payment.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                taker_payment: recreate_ctx
                    .taker_coin
                    .parse_tx(&taker_payment.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                taker_payment_spend: recreate_ctx
                    .taker_coin
                    .parse_tx(&taker_payment_spend.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                negotiation_data: NegotiationData::from_stored_data(
                    negotiation_data,
                    &recreate_ctx.maker_coin,
                    &recreate_ctx.taker_coin,
                )?,
            }),
            TakerSwapEvent::MakerPaymentSpent {
                maker_coin_start_block,
                taker_coin_start_block,
                maker_payment,
                taker_payment,
                taker_payment_spend,
                maker_payment_spend,
                negotiation_data,
            } => Box::new(MakerPaymentSpent {
                maker_coin_start_block,
                taker_coin_start_block,
                maker_payment: recreate_ctx
                    .maker_coin
                    .parse_tx(&maker_payment.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                taker_payment: recreate_ctx
                    .taker_coin
                    .parse_tx(&taker_payment.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                taker_payment_spend: recreate_ctx
                    .taker_coin
                    .parse_tx(&taker_payment_spend.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                maker_payment_spend: recreate_ctx
                    .maker_coin
                    .parse_tx(&maker_payment_spend.tx_hex.0)
                    .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
                negotiation_data: NegotiationData::from_stored_data(
                    negotiation_data,
                    &recreate_ctx.maker_coin,
                    &recreate_ctx.taker_coin,
                )?,
            }),
            TakerSwapEvent::Aborted { .. } => return MmError::err(SwapRecreateError::SwapAborted),
            TakerSwapEvent::Completed => return MmError::err(SwapRecreateError::SwapCompleted),
            TakerSwapEvent::TakerFundingRefunded { .. } => {
                return MmError::err(SwapRecreateError::SwapFinishedWithRefund)
            },
            TakerSwapEvent::TakerPaymentRefunded { .. } => {
                return MmError::err(SwapRecreateError::SwapFinishedWithRefund)
            },
        };

        let machine = TakerSwapStateMachine {
            ctx: storage.ctx.clone(),
            abortable_system: storage
                .ctx
                .abortable_system
                .create_subsystem()
                .expect("create_subsystem should not fail"),
            storage,
            started_at: repr.started_at,
            lock_duration: repr.lock_duration,
            maker_coin: recreate_ctx.maker_coin,
            maker_volume: repr.maker_volume,
            taker_coin: recreate_ctx.taker_coin,
            taker_volume: repr.taker_volume,
            taker_premium: repr.taker_premium,
            secret_hash_algo: repr.secret_hash_algo,
            conf_settings: repr.conf_settings,
            p2p_topic: swap_v2_topic(&uuid),
            uuid,
            p2p_keypair: repr.p2p_keypair.map(|k| k.into_inner()),
            taker_secret: repr.taker_secret.into(),
            maker_p2p_pubkey: repr.maker_p2p_pub.into(),
            require_maker_payment_confirm_before_funding_spend: true,
            require_maker_payment_spend_confirm: true,
            swap_version: repr.swap_version,
        };
        Ok((RestoredMachine::new(machine), current_state))
    }

    async fn acquire_reentrancy_lock(&self) -> Result<Self::ReentrancyLock, Self::Error> {
        acquire_reentrancy_lock_impl(&self.ctx, self.uuid).await
    }

    fn spawn_reentrancy_lock_renew(&mut self, guard: Self::ReentrancyLock) {
        spawn_reentrancy_lock_renew_impl(&self.abortable_system, self.uuid, guard)
    }

    fn init_additional_context(&mut self) {
        let swap_info = ActiveSwapV2Info {
            uuid: self.uuid,
            maker_coin: self.maker_coin.ticker().into(),
            taker_coin: self.taker_coin.ticker().into(),
            swap_type: TAKER_SWAP_V2_TYPE,
        };
        init_additional_context_impl(&self.ctx, swap_info, self.maker_p2p_pubkey);
    }

    fn clean_up_context(&mut self) {
        clean_up_context_impl(
            &self.ctx,
            &self.uuid,
            self.maker_coin.ticker(),
            self.taker_coin.ticker(),
        )
    }

    fn on_event(&mut self, event: &TakerSwapEvent) {
        match event {
            TakerSwapEvent::Initialized {
                taker_payment_fee,
                maker_payment_spend_fee: _,
                ..
            } => {
                let swaps_ctx = SwapsContext::from_ctx(&self.ctx).expect("from_ctx should not fail at this point");
                let taker_coin_ticker: String = self.taker_coin.ticker().into();
                let new_locked = LockedAmountInfo {
                    swap_uuid: self.uuid,
                    locked_amount: LockedAmount {
                        coin: taker_coin_ticker.clone(),
                        amount: &(&self.taker_volume + &self.dex_fee().total_spend_amount()) + &self.taker_premium,
                        trade_fee: Some(taker_payment_fee.clone().into()),
                    },
                };
                swaps_ctx
                    .locked_amounts
                    .lock()
                    .unwrap()
                    .entry(taker_coin_ticker)
                    .or_default()
                    .push(new_locked);
            },
            TakerSwapEvent::TakerFundingSent { .. } => {
                let swaps_ctx = SwapsContext::from_ctx(&self.ctx).expect("from_ctx should not fail at this point");
                let ticker = self.taker_coin.ticker();
                if let Some(taker_coin_locked) = swaps_ctx.locked_amounts.lock().unwrap().get_mut(ticker) {
                    taker_coin_locked.retain(|locked| locked.swap_uuid != self.uuid);
                };
            },
            TakerSwapEvent::Negotiated { .. }
            | TakerSwapEvent::TakerFundingRefundRequired { .. }
            | TakerSwapEvent::MakerPaymentAndFundingSpendPreimgReceived { .. }
            | TakerSwapEvent::TakerPaymentSent { .. }
            | TakerSwapEvent::TakerPaymentSentAndPreimageSendingSkipped { .. }
            | TakerSwapEvent::TakerPaymentRefundRequired { .. }
            | TakerSwapEvent::MakerPaymentConfirmed { .. }
            | TakerSwapEvent::TakerPaymentSpent { .. }
            | TakerSwapEvent::MakerPaymentSpent { .. }
            | TakerSwapEvent::TakerFundingRefunded { .. }
            | TakerSwapEvent::TakerPaymentRefunded { .. }
            | TakerSwapEvent::Aborted { .. }
            | TakerSwapEvent::Completed => (),
        }
        // Send a notification to the swap status streamer about a new event.
        self.ctx
            .event_stream_manager
            .send_fn(&SwapStatusStreamer::derive_streamer_id(()), || {
                SwapStatusEvent::TakerV2 {
                    uuid: self.uuid,
                    event: event.clone(),
                }
            })
            .ok();
    }

    fn on_kickstart_event(&mut self, event: TakerSwapEvent) {
        match event {
            TakerSwapEvent::Initialized { taker_payment_fee, .. }
            | TakerSwapEvent::Negotiated { taker_payment_fee, .. } => {
                let swaps_ctx = SwapsContext::from_ctx(&self.ctx).expect("from_ctx should not fail at this point");
                let taker_coin_ticker: String = self.taker_coin.ticker().into();
                let new_locked = LockedAmountInfo {
                    swap_uuid: self.uuid,
                    locked_amount: LockedAmount {
                        coin: taker_coin_ticker.clone(),
                        amount: &(&self.taker_volume + &self.dex_fee().total_spend_amount()) + &self.taker_premium,
                        trade_fee: Some(taker_payment_fee.into()),
                    },
                };
                swaps_ctx
                    .locked_amounts
                    .lock()
                    .unwrap()
                    .entry(taker_coin_ticker)
                    .or_default()
                    .push(new_locked);
            },
            TakerSwapEvent::TakerFundingSent { .. }
            | TakerSwapEvent::TakerFundingRefundRequired { .. }
            | TakerSwapEvent::MakerPaymentAndFundingSpendPreimgReceived { .. }
            | TakerSwapEvent::TakerPaymentSent { .. }
            | TakerSwapEvent::TakerPaymentSentAndPreimageSendingSkipped { .. }
            | TakerSwapEvent::TakerPaymentRefundRequired { .. }
            | TakerSwapEvent::MakerPaymentConfirmed { .. }
            | TakerSwapEvent::TakerPaymentSpent { .. }
            | TakerSwapEvent::MakerPaymentSpent { .. }
            | TakerSwapEvent::TakerFundingRefunded { .. }
            | TakerSwapEvent::TakerPaymentRefunded { .. }
            | TakerSwapEvent::Aborted { .. }
            | TakerSwapEvent::Completed => (),
        }
    }
}

/// Represents a state used to start a new taker swap.
pub struct Initialize<MakerCoin, TakerCoin> {
    maker_coin: PhantomData<MakerCoin>,
    taker_coin: PhantomData<TakerCoin>,
}

impl<MakerCoin, TakerCoin> Default for Initialize<MakerCoin, TakerCoin> {
    fn default() -> Self {
        Initialize {
            maker_coin: Default::default(),
            taker_coin: Default::default(),
        }
    }
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> InitialState
    for Initialize<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for Initialize<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        let maker_coin_start_block = match state_machine.maker_coin.current_block().compat().await {
            Ok(b) => b,
            Err(e) => {
                let reason = AbortReason::FailedToGetMakerCoinBlock(e);
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        let taker_coin_start_block = match state_machine.taker_coin.current_block().compat().await {
            Ok(b) => b,
            Err(e) => {
                let reason = AbortReason::FailedToGetTakerCoinBlock(e);
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        let total_payment_value = &(&state_machine.taker_volume + &state_machine.dex_fee().total_spend_amount())
            + &state_machine.taker_premium;
        let preimage_value = TradePreimageValue::Exact(total_payment_value.to_decimal());
        let stage = FeeApproxStage::StartSwap;

        let taker_payment_fee = match state_machine
            .taker_coin
            .get_sender_trade_fee(preimage_value, stage)
            .await
        {
            Ok(fee) => fee,
            Err(e) => {
                let reason = AbortReason::FailedToGetTakerPaymentFee(e.to_string());
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        let maker_payment_spend_fee = match state_machine.maker_coin.get_receiver_trade_fee(stage).compat().await {
            Ok(fee) => fee,
            Err(e) => {
                let reason = AbortReason::FailedToGetMakerPaymentSpendFee(e.to_string());
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        let prepared_params = TakerSwapPreparedParams {
            dex_fee: state_machine.dex_fee().total_spend_amount(),
            fee_to_send_dex_fee: TradeFee {
                coin: state_machine.taker_coin.ticker().into(),
                amount: Default::default(),
                paid_from_trading_vol: false,
            },
            taker_payment_trade_fee: taker_payment_fee.clone(),
            maker_payment_spend_trade_fee: maker_payment_spend_fee.clone(),
        };

        if let Err(e) = check_balance_for_taker_swap(
            &state_machine.ctx,
            &state_machine.taker_coin,
            &state_machine.maker_coin,
            total_payment_value,
            Some(&state_machine.uuid),
            Some(prepared_params),
            FeeApproxStage::StartSwap,
        )
        .await
        {
            let reason = AbortReason::BalanceCheckFailure(e.to_string());
            return Self::change_state(Aborted::new(reason), state_machine).await;
        }

        info!("Taker swap {} has successfully started", state_machine.uuid);
        let next_state = Initialized {
            maker_coin: Default::default(),
            taker_coin: Default::default(),
            maker_coin_start_block,
            taker_coin_start_block,
            taker_payment_fee: taker_payment_fee.into(),
            maker_payment_spend_fee: maker_payment_spend_fee.into(),
        };
        Self::change_state(next_state, state_machine).await
    }
}

struct Initialized<MakerCoin, TakerCoin> {
    maker_coin: PhantomData<MakerCoin>,
    taker_coin: PhantomData<TakerCoin>,
    maker_coin_start_block: u64,
    taker_coin_start_block: u64,
    taker_payment_fee: SavedTradeFee,
    maker_payment_spend_fee: SavedTradeFee,
}

impl<MakerCoin, TakerCoin> TransitionFrom<Initialize<MakerCoin, TakerCoin>> for Initialized<MakerCoin, TakerCoin> {}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for Initialized<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::Initialized {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            taker_payment_fee: self.taker_payment_fee.clone(),
            maker_payment_spend_fee: self.maker_payment_spend_fee.clone(),
        }
    }
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for Initialized<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        let recv_fut = recv_swap_v2_msg(
            state_machine.ctx.clone(),
            |store| store.maker_negotiation.take(),
            &state_machine.uuid,
            NEGOTIATION_TIMEOUT_SEC,
        );

        let maker_negotiation = match recv_fut.await {
            Ok(d) => d,
            Err(e) => {
                let reason = AbortReason::DidNotReceiveMakerNegotiation(e);
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        debug!("Received maker negotiation message {:?}", maker_negotiation);

        let started_at_diff = state_machine.started_at.abs_diff(maker_negotiation.started_at);
        if started_at_diff > MAX_STARTED_AT_DIFF {
            let reason = AbortReason::TooLargeStartedAtDiff(started_at_diff);
            return Self::change_state(Aborted::new(reason), state_machine).await;
        }

        if !(maker_negotiation.secret_hash.len() == 20 || maker_negotiation.secret_hash.len() == 32) {
            let reason = AbortReason::SecretHashUnexpectedLen(maker_negotiation.secret_hash.len());
            return Self::change_state(Aborted::new(reason), state_machine).await;
        }

        let expected_maker_payment_locktime = maker_negotiation.started_at + 2 * state_machine.lock_duration;
        if maker_negotiation.payment_locktime != expected_maker_payment_locktime {
            let reason = AbortReason::MakerProvidedInvalidLocktime(maker_negotiation.payment_locktime);
            return Self::change_state(Aborted::new(reason), state_machine).await;
        }

        let maker_coin_htlc_pub_from_maker = match state_machine
            .maker_coin
            .parse_pubkey(&maker_negotiation.maker_coin_htlc_pub)
        {
            Ok(p) => p,
            Err(e) => {
                let reason = AbortReason::FailedToParsePubkey(e.to_string());
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        let taker_coin_htlc_pub_from_maker = match state_machine
            .taker_coin
            .parse_pubkey(&maker_negotiation.taker_coin_htlc_pub)
        {
            Ok(p) => p,
            Err(e) => {
                let reason = AbortReason::FailedToParsePubkey(e.to_string());
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        let taker_coin_maker_address = match state_machine
            .taker_coin
            .parse_address(&maker_negotiation.taker_coin_address)
        {
            Ok(p) => p,
            Err(e) => {
                let reason = AbortReason::FailedToParseAddress(e.to_string());
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        let unique_data = state_machine.unique_data();
        let taker_negotiation = TakerNegotiation {
            action: Some(taker_negotiation::Action::Continue(TakerNegotiationData {
                started_at: state_machine.started_at,
                funding_locktime: state_machine.taker_funding_locktime(),
                payment_locktime: state_machine.taker_payment_locktime(),
                taker_secret_hash: state_machine.taker_secret_hash(),
                maker_coin_htlc_pub: state_machine.maker_coin.derive_htlc_pubkey_v2_bytes(&unique_data),
                taker_coin_htlc_pub: state_machine.taker_coin.derive_htlc_pubkey_v2_bytes(&unique_data),
                maker_coin_swap_contract: state_machine.maker_coin.swap_contract_address().map(|bytes| bytes.0),
                taker_coin_swap_contract: state_machine.taker_coin.swap_contract_address().map(|bytes| bytes.0),
            })),
        };

        let swap_msg = SwapMessage {
            inner: Some(swap_message::Inner::TakerNegotiation(taker_negotiation)),
            swap_uuid: state_machine.uuid.as_bytes().to_vec(),
        };
        let abort_handle = broadcast_swap_v2_msg_every(
            state_machine.ctx.clone(),
            state_machine.p2p_topic.clone(),
            swap_msg,
            NEGOTIATE_SEND_INTERVAL,
            state_machine.p2p_keypair,
        );

        let recv_fut = recv_swap_v2_msg(
            state_machine.ctx.clone(),
            |store| store.maker_negotiated.take(),
            &state_machine.uuid,
            NEGOTIATION_TIMEOUT_SEC,
        );

        let maker_negotiated = match recv_fut.await {
            Ok(d) => d,
            Err(e) => {
                let reason = AbortReason::DidNotReceiveMakerNegotiated(e);
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };
        drop(abort_handle);

        debug!("Received maker negotiated message {:?}", maker_negotiated);
        if !maker_negotiated.negotiated {
            let reason = AbortReason::MakerDidNotNegotiate(maker_negotiated.reason.unwrap_or_default());
            return Self::change_state(Aborted::new(reason), state_machine).await;
        }

        let next_state = Negotiated {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            negotiation_data: NegotiationData {
                maker_secret_hash: maker_negotiation.secret_hash,
                maker_payment_locktime: expected_maker_payment_locktime,
                maker_coin_htlc_pub_from_maker,
                taker_coin_htlc_pub_from_maker,
                maker_coin_swap_contract: maker_negotiation.maker_coin_swap_contract,
                taker_coin_swap_contract: maker_negotiation.taker_coin_swap_contract,
                taker_coin_maker_address,
            },
            taker_payment_fee: self.taker_payment_fee,
            maker_payment_spend_fee: self.maker_payment_spend_fee,
        };
        Self::change_state(next_state, state_machine).await
    }
}

struct NegotiationData<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_secret_hash: Vec<u8>,
    maker_payment_locktime: u64,
    maker_coin_htlc_pub_from_maker: MakerCoin::Pubkey,
    taker_coin_htlc_pub_from_maker: TakerCoin::Pubkey,
    maker_coin_swap_contract: Option<Vec<u8>>,
    taker_coin_swap_contract: Option<Vec<u8>>,
    taker_coin_maker_address: TakerCoin::Address,
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> NegotiationData<MakerCoin, TakerCoin> {
    fn to_stored_data(&self) -> StoredNegotiationData {
        StoredNegotiationData {
            maker_payment_locktime: self.maker_payment_locktime,
            maker_secret_hash: self.maker_secret_hash.clone().into(),
            taker_coin_maker_address: self.taker_coin_maker_address.addr_to_string(),
            maker_coin_htlc_pub_from_maker: self.maker_coin_htlc_pub_from_maker.to_bytes().into(),
            taker_coin_htlc_pub_from_maker: self.taker_coin_htlc_pub_from_maker.to_bytes().into(),
            maker_coin_swap_contract: self.maker_coin_swap_contract.clone().map(|b| b.into()),
            taker_coin_swap_contract: self.taker_coin_swap_contract.clone().map(|b| b.into()),
        }
    }

    fn from_stored_data(
        stored: StoredNegotiationData,
        maker_coin: &MakerCoin,
        taker_coin: &TakerCoin,
    ) -> Result<Self, MmError<SwapRecreateError>> {
        Ok(NegotiationData {
            maker_secret_hash: stored.maker_secret_hash.into(),
            maker_payment_locktime: stored.maker_payment_locktime,
            maker_coin_htlc_pub_from_maker: maker_coin
                .parse_pubkey(&stored.maker_coin_htlc_pub_from_maker.0)
                .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
            taker_coin_htlc_pub_from_maker: taker_coin
                .parse_pubkey(&stored.taker_coin_htlc_pub_from_maker.0)
                .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
            maker_coin_swap_contract: None,
            taker_coin_swap_contract: None,
            taker_coin_maker_address: taker_coin
                .parse_address(&stored.taker_coin_maker_address)
                .map_err(|e| SwapRecreateError::FailedToParseData(e.to_string()))?,
        })
    }
}

struct Negotiated<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_coin_start_block: u64,
    taker_coin_start_block: u64,
    negotiation_data: NegotiationData<MakerCoin, TakerCoin>,
    taker_payment_fee: SavedTradeFee,
    maker_payment_spend_fee: SavedTradeFee,
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: TakerCoinSwapOpsV2> TransitionFrom<Initialized<MakerCoin, TakerCoin>>
    for Negotiated<MakerCoin, TakerCoin>
{
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for Negotiated<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        let args = SendTakerFundingArgs {
            funding_time_lock: state_machine.taker_funding_locktime(),
            payment_time_lock: state_machine.taker_payment_locktime(),
            taker_secret_hash: &state_machine.taker_secret_hash(),
            maker_secret_hash: &self.negotiation_data.maker_secret_hash,
            maker_pub: &self.negotiation_data.taker_coin_htlc_pub_from_maker.to_bytes(),
            dex_fee: &state_machine.dex_fee(),
            premium_amount: state_machine.taker_premium.to_decimal(),
            trading_amount: state_machine.taker_volume.to_decimal(),
            swap_unique_data: &state_machine.unique_data(),
        };

        let taker_funding = match state_machine.taker_coin.send_taker_funding(args).await {
            Ok(tx) => tx,
            Err(e) => {
                let reason = AbortReason::FailedToSendTakerFunding(format!("{e:?}"));
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        info!(
            "Sent taker funding {} tx {:02x} during swap {}",
            state_machine.taker_coin.ticker(),
            taker_funding.tx_hash_as_bytes(),
            state_machine.uuid
        );

        let next_state = TakerFundingSent {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            taker_funding,
            negotiation_data: self.negotiation_data,
        };
        Self::change_state(next_state, state_machine).await
    }
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for Negotiated<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::Negotiated {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            negotiation_data: self.negotiation_data.to_stored_data(),
            taker_payment_fee: self.taker_payment_fee.clone(),
            maker_payment_spend_fee: self.maker_payment_spend_fee.clone(),
        }
    }
}

struct TakerFundingSent<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_coin_start_block: u64,
    taker_coin_start_block: u64,
    taker_funding: TakerCoin::Tx,
    negotiation_data: NegotiationData<MakerCoin, TakerCoin>,
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for TakerFundingSent<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        let taker_funding_info = TakerFundingInfo {
            tx_bytes: self.taker_funding.tx_hex(),
            next_step_instructions: None,
        };

        let swap_msg = SwapMessage {
            inner: Some(swap_message::Inner::TakerFundingInfo(taker_funding_info)),
            swap_uuid: state_machine.uuid.as_bytes().to_vec(),
        };
        let abort_handle = broadcast_swap_v2_msg_every(
            state_machine.ctx.clone(),
            state_machine.p2p_topic.clone(),
            swap_msg,
            600.,
            state_machine.p2p_keypair,
        );

        let recv_fut = recv_swap_v2_msg(
            state_machine.ctx.clone(),
            |store| store.maker_payment.take(),
            &state_machine.uuid,
            NEGOTIATION_TIMEOUT_SEC,
        );

        let maker_payment_info = match recv_fut.await {
            Ok(p) => p,
            Err(e) => {
                let next_state = TakerFundingRefundRequired {
                    maker_coin_start_block: self.maker_coin_start_block,
                    taker_coin_start_block: self.taker_coin_start_block,
                    taker_funding: self.taker_funding,
                    negotiation_data: self.negotiation_data,
                    reason: TakerFundingRefundReason::DidNotReceiveMakerPayment(e),
                };
                return Self::change_state(next_state, state_machine).await;
            },
        };
        drop(abort_handle);

        debug!("Received maker payment info message {:?}", maker_payment_info);

        let maker_payment = match state_machine.maker_coin.parse_tx(&maker_payment_info.tx_bytes) {
            Ok(tx) => tx,
            Err(e) => {
                let next_state = TakerFundingRefundRequired {
                    maker_coin_start_block: self.maker_coin_start_block,
                    taker_coin_start_block: self.taker_coin_start_block,
                    taker_funding: self.taker_funding,
                    negotiation_data: self.negotiation_data,
                    reason: TakerFundingRefundReason::FailedToParseMakerPayment(e.to_string()),
                };
                return Self::change_state(next_state, state_machine).await;
            },
        };

        let preimage_tx = match state_machine
            .taker_coin
            .parse_preimage(&maker_payment_info.funding_preimage_tx)
        {
            Ok(p) => p,
            Err(e) => {
                let next_state = TakerFundingRefundRequired {
                    maker_coin_start_block: self.maker_coin_start_block,
                    taker_coin_start_block: self.taker_coin_start_block,
                    taker_funding: self.taker_funding,
                    negotiation_data: self.negotiation_data,
                    reason: TakerFundingRefundReason::FailedToParseFundingSpendPreimg(e.to_string()),
                };
                return Self::change_state(next_state, state_machine).await;
            },
        };

        let preimage_sig = match state_machine
            .taker_coin
            .parse_signature(&maker_payment_info.funding_preimage_sig)
        {
            Ok(p) => p,
            Err(e) => {
                let next_state = TakerFundingRefundRequired {
                    maker_coin_start_block: self.maker_coin_start_block,
                    taker_coin_start_block: self.taker_coin_start_block,
                    taker_funding: self.taker_funding,
                    negotiation_data: self.negotiation_data,
                    reason: TakerFundingRefundReason::FailedToParseFundingSpendSig(e.to_string()),
                };
                return Self::change_state(next_state, state_machine).await;
            },
        };

        let next_state = MakerPaymentAndFundingSpendPreimgReceived {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            negotiation_data: self.negotiation_data,
            taker_funding: self.taker_funding,
            funding_spend_preimage: TxPreimageWithSig {
                preimage: preimage_tx,
                signature: preimage_sig,
            },
            maker_payment,
        };
        Self::change_state(next_state, state_machine).await
    }
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> TransitionFrom<Negotiated<MakerCoin, TakerCoin>>
    for TakerFundingSent<MakerCoin, TakerCoin>
{
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for TakerFundingSent<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::TakerFundingSent {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            taker_funding: TransactionIdentifier {
                tx_hex: self.taker_funding.tx_hex().into(),
                tx_hash: self.taker_funding.tx_hash_as_bytes(),
            },
            negotiation_data: self.negotiation_data.to_stored_data(),
        }
    }
}

struct MakerPaymentAndFundingSpendPreimgReceived<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_coin_start_block: u64,
    taker_coin_start_block: u64,
    negotiation_data: NegotiationData<MakerCoin, TakerCoin>,
    taker_funding: TakerCoin::Tx,
    funding_spend_preimage: TxPreimageWithSig<TakerCoin>,
    maker_payment: MakerCoin::Tx,
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<TakerFundingSent<MakerCoin, TakerCoin>>
    for MakerPaymentAndFundingSpendPreimgReceived<MakerCoin, TakerCoin>
{
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for MakerPaymentAndFundingSpendPreimgReceived<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::MakerPaymentAndFundingSpendPreimgReceived {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            negotiation_data: self.negotiation_data.to_stored_data(),
            taker_funding: TransactionIdentifier {
                tx_hex: self.taker_funding.tx_hex().into(),
                tx_hash: self.taker_funding.tx_hash_as_bytes(),
            },
            funding_spend_preimage: StoredTxPreimage {
                preimage: self.funding_spend_preimage.preimage.to_bytes().into(),
                signature: self.funding_spend_preimage.signature.to_bytes().into(),
            },
            maker_payment: TransactionIdentifier {
                tx_hex: self.maker_payment.tx_hex().into(),
                tx_hash: self.maker_payment.tx_hash_as_bytes(),
            },
        }
    }
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for MakerPaymentAndFundingSpendPreimgReceived<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        let unique_data = state_machine.unique_data();
        let my_secret_hash = state_machine.taker_secret_hash();

        let input = ValidateMakerPaymentArgs {
            maker_payment_tx: &self.maker_payment,
            time_lock: self.negotiation_data.maker_payment_locktime,
            taker_secret_hash: &my_secret_hash,
            amount: state_machine.maker_volume.to_decimal(),
            maker_pub: &self.negotiation_data.maker_coin_htlc_pub_from_maker,
            maker_secret_hash: &self.negotiation_data.maker_secret_hash,
            swap_unique_data: &unique_data,
        };
        if let Err(e) = state_machine.maker_coin.validate_maker_payment_v2(input).await {
            let next_state = TakerFundingRefundRequired {
                maker_coin_start_block: self.maker_coin_start_block,
                taker_coin_start_block: self.taker_coin_start_block,
                taker_funding: self.taker_funding,
                negotiation_data: self.negotiation_data,
                reason: TakerFundingRefundReason::MakerPaymentValidationFailed(e.to_string()),
            };
            return Self::change_state(next_state, state_machine).await;
        };

        let args = GenTakerFundingSpendArgs {
            funding_tx: &self.taker_funding,
            maker_pub: &self.negotiation_data.taker_coin_htlc_pub_from_maker,
            taker_pub: &state_machine.taker_coin.derive_htlc_pubkey_v2(&unique_data),
            funding_time_lock: state_machine.taker_funding_locktime(),
            taker_secret_hash: &state_machine.taker_secret_hash(),
            taker_payment_time_lock: state_machine.taker_payment_locktime(),
            maker_secret_hash: &self.negotiation_data.maker_secret_hash,
        };

        if let Err(e) = state_machine
            .taker_coin
            .validate_taker_funding_spend_preimage(&args, &self.funding_spend_preimage)
            .await
        {
            let next_state = TakerFundingRefundRequired {
                maker_coin_start_block: self.maker_coin_start_block,
                taker_coin_start_block: self.taker_coin_start_block,
                taker_funding: self.taker_funding,
                negotiation_data: self.negotiation_data,
                reason: TakerFundingRefundReason::FundingSpendPreimageValidationFailed(format!("{e:?}")),
            };
            return Self::change_state(next_state, state_machine).await;
        }

        if state_machine.require_maker_payment_confirm_before_funding_spend {
            let input = ConfirmPaymentInput {
                payment_tx: self.maker_payment.tx_hex(),
                confirmations: state_machine.conf_settings.maker_coin_confs,
                requires_nota: state_machine.conf_settings.maker_coin_nota,
                wait_until: state_machine.maker_payment_conf_timeout(),
                check_every: 10,
            };

            if let Err(e) = state_machine.maker_coin.wait_for_confirmations(input).compat().await {
                let next_state = TakerFundingRefundRequired {
                    maker_coin_start_block: self.maker_coin_start_block,
                    taker_coin_start_block: self.taker_coin_start_block,
                    taker_funding: self.taker_funding,
                    negotiation_data: self.negotiation_data,
                    reason: TakerFundingRefundReason::MakerPaymentNotConfirmedInTime(e),
                };
                return Self::change_state(next_state, state_machine).await;
            }

            let next_state = MakerPaymentConfirmed {
                maker_coin_start_block: self.maker_coin_start_block,
                taker_coin_start_block: self.taker_coin_start_block,
                maker_payment: self.maker_payment,
                taker_funding: self.taker_funding,
                funding_spend_preimage: self.funding_spend_preimage,
                negotiation_data: self.negotiation_data,
            };
            Self::change_state(next_state, state_machine).await
        } else {
            let unique_data = state_machine.unique_data();

            let args = GenTakerFundingSpendArgs {
                funding_tx: &self.taker_funding,
                maker_pub: &self.negotiation_data.taker_coin_htlc_pub_from_maker,
                taker_pub: &state_machine.taker_coin.derive_htlc_pubkey_v2(&unique_data),
                funding_time_lock: state_machine.taker_funding_locktime(),
                taker_secret_hash: &state_machine.taker_secret_hash(),
                taker_payment_time_lock: state_machine.taker_payment_locktime(),
                maker_secret_hash: &self.negotiation_data.maker_secret_hash,
            };

            let taker_payment = match state_machine
                .taker_coin
                .sign_and_send_taker_funding_spend(&self.funding_spend_preimage, &args, &unique_data)
                .await
            {
                Ok(tx) => tx,
                Err(e) => {
                    let next_state = TakerFundingRefundRequired {
                        maker_coin_start_block: self.maker_coin_start_block,
                        taker_coin_start_block: self.taker_coin_start_block,
                        taker_funding: self.taker_funding,
                        negotiation_data: self.negotiation_data,
                        reason: TakerFundingRefundReason::FailedToSendTakerPayment(format!("{e:?}")),
                    };
                    return Self::change_state(next_state, state_machine).await;
                },
            };

            info!(
                "Sent taker payment {} tx {:02x} during swap {}",
                state_machine.taker_coin.ticker(),
                taker_payment.tx_hash_as_bytes(),
                state_machine.uuid
            );

            if state_machine.taker_coin.skip_taker_payment_spend_preimage() {
                let next_state = TakerPaymentSentAndPreimageSendingSkipped {
                    maker_coin_start_block: self.maker_coin_start_block,
                    taker_coin_start_block: self.taker_coin_start_block,
                    taker_payment,
                    maker_payment: self.maker_payment,
                    negotiation_data: self.negotiation_data,
                };
                Self::change_state(next_state, state_machine).await
            } else {
                let next_state = TakerPaymentSent {
                    maker_coin_start_block: self.maker_coin_start_block,
                    taker_coin_start_block: self.taker_coin_start_block,
                    taker_payment,
                    maker_payment: self.maker_payment,
                    negotiation_data: self.negotiation_data,
                };
                Self::change_state(next_state, state_machine).await
            }
        }
    }
}

struct TakerPaymentSent<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_coin_start_block: u64,
    taker_coin_start_block: u64,
    taker_payment: TakerCoin::Tx,
    maker_payment: MakerCoin::Tx,
    negotiation_data: NegotiationData<MakerCoin, TakerCoin>,
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<MakerPaymentConfirmed<MakerCoin, TakerCoin>> for TakerPaymentSent<MakerCoin, TakerCoin>
{
}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<MakerPaymentAndFundingSpendPreimgReceived<MakerCoin, TakerCoin>>
    for TakerPaymentSent<MakerCoin, TakerCoin>
{
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for TakerPaymentSent<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        if !state_machine.require_maker_payment_confirm_before_funding_spend {
            let input = ConfirmPaymentInput {
                payment_tx: self.maker_payment.tx_hex(),
                confirmations: state_machine.conf_settings.maker_coin_confs,
                requires_nota: state_machine.conf_settings.maker_coin_nota,
                wait_until: state_machine.maker_payment_conf_timeout(),
                check_every: 10,
            };

            if let Err(e) = state_machine.maker_coin.wait_for_confirmations(input).compat().await {
                let next_state = TakerPaymentRefundRequired {
                    taker_payment: self.taker_payment,
                    negotiation_data: self.negotiation_data,
                    reason: TakerPaymentRefundReason::MakerPaymentNotConfirmedInTime(e),
                };
                return Self::change_state(next_state, state_machine).await;
            }
        }

        let unique_data = state_machine.unique_data();

        let args = GenTakerPaymentSpendArgs {
            taker_tx: &self.taker_payment,
            time_lock: state_machine.taker_payment_locktime(),
            maker_secret_hash: &self.negotiation_data.maker_secret_hash,
            maker_pub: &self.negotiation_data.taker_coin_htlc_pub_from_maker,
            maker_address: &self.negotiation_data.taker_coin_maker_address,
            taker_pub: &state_machine.taker_coin.derive_htlc_pubkey_v2(&unique_data),
            dex_fee: &state_machine.dex_fee(),
            premium_amount: Default::default(),
            trading_amount: state_machine.taker_volume.to_decimal(),
        };

        let preimage = match state_machine
            .taker_coin
            .gen_taker_payment_spend_preimage(&args, &unique_data)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                let next_state = TakerPaymentRefundRequired {
                    taker_payment: self.taker_payment,
                    negotiation_data: self.negotiation_data,
                    reason: TakerPaymentRefundReason::FailedToGenerateSpendPreimage(e.to_string()),
                };
                return Self::change_state(next_state, state_machine).await;
            },
        };

        let preimage_msg = TakerPaymentSpendPreimage {
            signature: preimage.signature.to_bytes(),
            tx_preimage: preimage.preimage.to_bytes(),
        };
        let swap_msg = SwapMessage {
            inner: Some(swap_message::Inner::TakerPaymentSpendPreimage(preimage_msg)),
            swap_uuid: state_machine.uuid.as_bytes().to_vec(),
        };

        let _abort_handle = broadcast_swap_v2_msg_every(
            state_machine.ctx.clone(),
            state_machine.p2p_topic.clone(),
            swap_msg,
            600.,
            state_machine.p2p_keypair,
        );

        let taker_payment_spend = match state_machine
            .taker_coin
            .find_taker_payment_spend_tx(
                &self.taker_payment,
                self.taker_coin_start_block,
                state_machine.taker_payment_locktime(),
            )
            .await
        {
            Ok(tx) => tx,
            Err(e) => {
                let next_state = TakerPaymentRefundRequired {
                    taker_payment: self.taker_payment,
                    negotiation_data: self.negotiation_data,
                    reason: TakerPaymentRefundReason::MakerDidNotSpendInTime(format!("{e}")),
                };
                return Self::change_state(next_state, state_machine).await;
            },
        };
        info!(
            "Found taker payment spend {} tx {:02x} during swap {}",
            state_machine.taker_coin.ticker(),
            taker_payment_spend.tx_hash_as_bytes(),
            state_machine.uuid
        );

        let next_state = TakerPaymentSpent {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            maker_payment: self.maker_payment,
            taker_payment: self.taker_payment,
            taker_payment_spend,
            negotiation_data: self.negotiation_data,
        };
        Self::change_state(next_state, state_machine).await
    }
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for TakerPaymentSent<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::TakerPaymentSent {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            taker_payment: TransactionIdentifier {
                tx_hex: self.taker_payment.tx_hex().into(),
                tx_hash: self.taker_payment.tx_hash_as_bytes(),
            },
            maker_payment: TransactionIdentifier {
                tx_hex: self.maker_payment.tx_hex().into(),
                tx_hash: self.maker_payment.tx_hash_as_bytes(),
            },
            negotiation_data: self.negotiation_data.to_stored_data(),
        }
    }
}

struct TakerPaymentSentAndPreimageSendingSkipped<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_coin_start_block: u64,
    taker_coin_start_block: u64,
    taker_payment: TakerCoin::Tx,
    maker_payment: MakerCoin::Tx,
    negotiation_data: NegotiationData<MakerCoin, TakerCoin>,
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<MakerPaymentConfirmed<MakerCoin, TakerCoin>>
    for TakerPaymentSentAndPreimageSendingSkipped<MakerCoin, TakerCoin>
{
}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<MakerPaymentAndFundingSpendPreimgReceived<MakerCoin, TakerCoin>>
    for TakerPaymentSentAndPreimageSendingSkipped<MakerCoin, TakerCoin>
{
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for TakerPaymentSentAndPreimageSendingSkipped<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        info!("Skipped the generation of the taker payment spend preimage and its p2p message broadcast because the taker's coin does not require this preimage for the process.");

        if !state_machine.require_maker_payment_confirm_before_funding_spend {
            let input = ConfirmPaymentInput {
                payment_tx: self.maker_payment.tx_hex(),
                confirmations: state_machine.conf_settings.maker_coin_confs,
                requires_nota: state_machine.conf_settings.maker_coin_nota,
                wait_until: state_machine.maker_payment_conf_timeout(),
                check_every: 10,
            };

            if let Err(e) = state_machine.maker_coin.wait_for_confirmations(input).compat().await {
                let next_state = TakerPaymentRefundRequired {
                    taker_payment: self.taker_payment,
                    negotiation_data: self.negotiation_data,
                    reason: TakerPaymentRefundReason::MakerPaymentNotConfirmedInTime(e),
                };
                return Self::change_state(next_state, state_machine).await;
            }
        }

        let taker_payment_spend = match state_machine
            .taker_coin
            .find_taker_payment_spend_tx(
                &self.taker_payment,
                self.taker_coin_start_block,
                state_machine.taker_payment_locktime(),
            )
            .await
        {
            Ok(tx) => tx,
            Err(e) => {
                let next_state = TakerPaymentRefundRequired {
                    taker_payment: self.taker_payment,
                    negotiation_data: self.negotiation_data,
                    reason: TakerPaymentRefundReason::MakerDidNotSpendInTime(format!("{e}")),
                };
                return Self::change_state(next_state, state_machine).await;
            },
        };
        info!(
            "Found taker payment spend {} tx {:02x} during swap {}",
            state_machine.taker_coin.ticker(),
            taker_payment_spend.tx_hash_as_bytes(),
            state_machine.uuid
        );

        let next_state = TakerPaymentSpent {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            maker_payment: self.maker_payment,
            taker_payment: self.taker_payment,
            taker_payment_spend,
            negotiation_data: self.negotiation_data,
        };
        Self::change_state(next_state, state_machine).await
    }
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for TakerPaymentSentAndPreimageSendingSkipped<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::TakerPaymentSentAndPreimageSendingSkipped {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            taker_payment: TransactionIdentifier {
                tx_hex: self.taker_payment.tx_hex().into(),
                tx_hash: self.taker_payment.tx_hash_as_bytes(),
            },
            maker_payment: TransactionIdentifier {
                tx_hex: self.maker_payment.tx_hex().into(),
                tx_hash: self.maker_payment.tx_hash_as_bytes(),
            },
            negotiation_data: self.negotiation_data.to_stored_data(),
        }
    }
}

/// Represents the reason taker funding refund
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum TakerFundingRefundReason {
    DidNotReceiveMakerPayment(String),
    FailedToParseFundingSpendPreimg(String),
    FailedToParseFundingSpendSig(String),
    FailedToSendTakerPayment(String),
    MakerPaymentValidationFailed(String),
    FundingSpendPreimageValidationFailed(String),
    FailedToParseMakerPayment(String),
    MakerPaymentNotConfirmedInTime(String),
}

struct TakerFundingRefundRequired<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_coin_start_block: u64,
    taker_coin_start_block: u64,
    taker_funding: TakerCoin::Tx,
    negotiation_data: NegotiationData<MakerCoin, TakerCoin>,
    reason: TakerFundingRefundReason,
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<TakerFundingSent<MakerCoin, TakerCoin>> for TakerFundingRefundRequired<MakerCoin, TakerCoin>
{
}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<MakerPaymentAndFundingSpendPreimgReceived<MakerCoin, TakerCoin>>
    for TakerFundingRefundRequired<MakerCoin, TakerCoin>
{
}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<MakerPaymentConfirmed<MakerCoin, TakerCoin>> for TakerFundingRefundRequired<MakerCoin, TakerCoin>
{
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for TakerFundingRefundRequired<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        warn!(
            "Entered TakerFundingRefundRequired state for swap {} with reason {:?}",
            state_machine.uuid, self.reason
        );

        let secret_hash = state_machine.taker_secret_hash();
        let unique_data = state_machine.unique_data();

        let refund_args = RefundFundingSecretArgs {
            funding_tx: &self.taker_funding,
            funding_time_lock: state_machine.taker_funding_locktime(),
            payment_time_lock: state_machine.taker_payment_locktime(),
            maker_pubkey: &self.negotiation_data.taker_coin_htlc_pub_from_maker,
            taker_secret: &state_machine.taker_secret.take(),
            taker_secret_hash: &secret_hash,
            maker_secret_hash: &self.negotiation_data.maker_secret_hash,
            dex_fee: &state_machine.dex_fee(),
            premium_amount: state_machine.taker_premium.to_decimal(),
            trading_amount: state_machine.taker_volume.to_decimal(),
            swap_unique_data: &unique_data,
            watcher_reward: false,
        };

        let funding_refund_tx = match state_machine.taker_coin.refund_taker_funding_secret(refund_args).await {
            Ok(tx) => tx,
            Err(e) => {
                let reason = AbortReason::TakerFundingRefundFailed(e.get_plain_text_format());
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        let next_state = TakerFundingRefunded {
            maker_coin: Default::default(),
            funding_tx: self.taker_funding,
            funding_refund_tx,
            reason: self.reason,
        };
        Self::change_state(next_state, state_machine).await
    }
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for TakerFundingRefundRequired<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::TakerFundingRefundRequired {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            taker_funding: TransactionIdentifier {
                tx_hex: self.taker_funding.tx_hex().into(),
                tx_hash: self.taker_funding.tx_hash_as_bytes(),
            },
            negotiation_data: self.negotiation_data.to_stored_data(),
            reason: self.reason.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum TakerPaymentRefundReason {
    MakerPaymentNotConfirmedInTime(String),
    MakerPaymentSpendNotConfirmedInTime(String),
    FailedToGenerateSpendPreimage(String),
    MakerDidNotSpendInTime(String),
}

struct TakerPaymentRefundRequired<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    taker_payment: TakerCoin::Tx,
    negotiation_data: NegotiationData<MakerCoin, TakerCoin>,
    reason: TakerPaymentRefundReason,
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<TakerPaymentSent<MakerCoin, TakerCoin>> for TakerPaymentRefundRequired<MakerCoin, TakerCoin>
{
}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<TakerPaymentSentAndPreimageSendingSkipped<MakerCoin, TakerCoin>>
    for TakerPaymentRefundRequired<MakerCoin, TakerCoin>
{
}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<MakerPaymentConfirmed<MakerCoin, TakerCoin>> for TakerPaymentRefundRequired<MakerCoin, TakerCoin>
{
}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<MakerPaymentSpent<MakerCoin, TakerCoin>> for TakerPaymentRefundRequired<MakerCoin, TakerCoin>
{
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for TakerPaymentRefundRequired<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        warn!(
            "Entered TakerPaymentRefundRequired state for swap {} with reason {:?}",
            state_machine.uuid, self.reason
        );

        loop {
            match state_machine
                .taker_coin
                .can_refund_htlc(state_machine.taker_payment_locktime())
                .await
            {
                Ok(CanRefundHtlc::CanRefundNow) => break,
                Ok(CanRefundHtlc::HaveToWait(to_sleep)) => Timer::sleep(to_sleep as f64).await,
                Err(e) => {
                    error!("Error {} on can_refund_htlc, retrying in 30 seconds", e);
                    Timer::sleep(30.).await;
                },
            }
        }

        let payment_tx_bytes = self.taker_payment.tx_hex();
        let unique_data = state_machine.unique_data();
        let maker_pub = self.negotiation_data.taker_coin_htlc_pub_from_maker.to_bytes();

        let args = RefundTakerPaymentArgs {
            payment_tx: &payment_tx_bytes,
            time_lock: state_machine.taker_payment_locktime(),
            maker_pub: &maker_pub,
            tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerPaymentV2 {
                maker_secret_hash: &self.negotiation_data.maker_secret_hash,
                taker_secret_hash: &state_machine.taker_secret_hash(),
            },
            swap_unique_data: &unique_data,
            watcher_reward: false,
            dex_fee: &state_machine.dex_fee(),
            premium_amount: state_machine.taker_premium.to_decimal(),
            trading_amount: state_machine.taker_volume.to_decimal(),
        };

        let taker_payment_refund_tx = match state_machine.taker_coin.refund_combined_taker_payment(args).await {
            Ok(tx) => tx,
            Err(e) => {
                let reason = AbortReason::TakerPaymentRefundFailed(e.get_plain_text_format());
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        let next_state = TakerPaymentRefunded {
            maker_coin: Default::default(),
            taker_payment: self.taker_payment,
            taker_payment_refund: TransactionIdentifier {
                tx_hex: taker_payment_refund_tx.tx_hex().into(),
                tx_hash: taker_payment_refund_tx.tx_hash_as_bytes(),
            },
            reason: self.reason,
        };
        Self::change_state(next_state, state_machine).await
    }
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for TakerPaymentRefundRequired<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::TakerPaymentRefundRequired {
            taker_payment: TransactionIdentifier {
                tx_hex: self.taker_payment.tx_hex().into(),
                tx_hash: self.taker_payment.tx_hash_as_bytes(),
            },
            negotiation_data: self.negotiation_data.to_stored_data(),
            reason: self.reason.clone(),
        }
    }
}

struct MakerPaymentConfirmed<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_coin_start_block: u64,
    taker_coin_start_block: u64,
    maker_payment: MakerCoin::Tx,
    taker_funding: TakerCoin::Tx,
    funding_spend_preimage: TxPreimageWithSig<TakerCoin>,
    negotiation_data: NegotiationData<MakerCoin, TakerCoin>,
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<MakerPaymentAndFundingSpendPreimgReceived<MakerCoin, TakerCoin>>
    for MakerPaymentConfirmed<MakerCoin, TakerCoin>
{
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for MakerPaymentConfirmed<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        let unique_data = state_machine.unique_data();

        let args = GenTakerFundingSpendArgs {
            funding_tx: &self.taker_funding,
            maker_pub: &self.negotiation_data.taker_coin_htlc_pub_from_maker,
            taker_pub: &state_machine.taker_coin.derive_htlc_pubkey_v2(&unique_data),
            funding_time_lock: state_machine.taker_funding_locktime(),
            taker_secret_hash: &state_machine.taker_secret_hash(),
            taker_payment_time_lock: state_machine.taker_payment_locktime(),
            maker_secret_hash: &self.negotiation_data.maker_secret_hash,
        };

        let taker_payment = match state_machine
            .taker_coin
            .sign_and_send_taker_funding_spend(&self.funding_spend_preimage, &args, &unique_data)
            .await
        {
            Ok(tx) => tx,
            Err(e) => {
                let next_state = TakerFundingRefundRequired {
                    maker_coin_start_block: self.maker_coin_start_block,
                    taker_coin_start_block: self.taker_coin_start_block,
                    taker_funding: self.taker_funding,
                    negotiation_data: self.negotiation_data,
                    reason: TakerFundingRefundReason::FailedToSendTakerPayment(format!("{e:?}")),
                };
                return Self::change_state(next_state, state_machine).await;
            },
        };

        info!(
            "Sent taker payment {} tx {:02x} during swap {}",
            state_machine.taker_coin.ticker(),
            taker_payment.tx_hash_as_bytes(),
            state_machine.uuid
        );

        if state_machine.taker_coin.skip_taker_payment_spend_preimage() {
            let next_state = TakerPaymentSentAndPreimageSendingSkipped {
                maker_coin_start_block: self.maker_coin_start_block,
                taker_coin_start_block: self.taker_coin_start_block,
                taker_payment,
                maker_payment: self.maker_payment,
                negotiation_data: self.negotiation_data,
            };
            Self::change_state(next_state, state_machine).await
        } else {
            let next_state = TakerPaymentSent {
                maker_coin_start_block: self.maker_coin_start_block,
                taker_coin_start_block: self.taker_coin_start_block,
                taker_payment,
                maker_payment: self.maker_payment,
                negotiation_data: self.negotiation_data,
            };
            Self::change_state(next_state, state_machine).await
        }
    }
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for MakerPaymentConfirmed<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::MakerPaymentConfirmed {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            maker_payment: TransactionIdentifier {
                tx_hex: self.maker_payment.tx_hex().into(),
                tx_hash: self.maker_payment.tx_hash_as_bytes(),
            },
            taker_funding: TransactionIdentifier {
                tx_hex: self.taker_funding.tx_hex().into(),
                tx_hash: self.taker_funding.tx_hash_as_bytes(),
            },
            negotiation_data: self.negotiation_data.to_stored_data(),
            funding_spend_preimage: StoredTxPreimage {
                preimage: self.funding_spend_preimage.preimage.to_bytes().into(),
                signature: self.funding_spend_preimage.signature.to_bytes().into(),
            },
        }
    }
}

struct TakerPaymentSpent<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_coin_start_block: u64,
    taker_coin_start_block: u64,
    maker_payment: MakerCoin::Tx,
    taker_payment: TakerCoin::Tx,
    taker_payment_spend: TakerCoin::Tx,
    negotiation_data: NegotiationData<MakerCoin, TakerCoin>,
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<TakerPaymentSent<MakerCoin, TakerCoin>> for TakerPaymentSpent<MakerCoin, TakerCoin>
{
}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<TakerPaymentSentAndPreimageSendingSkipped<MakerCoin, TakerCoin>>
    for TakerPaymentSpent<MakerCoin, TakerCoin>
{
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for TakerPaymentSpent<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        let unique_data = state_machine.unique_data();

        let secret = match state_machine
            .taker_coin
            .extract_secret_v2(&self.negotiation_data.maker_secret_hash, &self.taker_payment_spend)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let reason = AbortReason::CouldNotExtractSecret(e);
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };

        let args = SpendMakerPaymentArgs {
            maker_payment_tx: &self.maker_payment,
            time_lock: self.negotiation_data.maker_payment_locktime,
            taker_secret_hash: &state_machine.taker_secret_hash(),
            maker_secret_hash: &self.negotiation_data.maker_secret_hash,
            maker_secret: secret,
            maker_pub: &self.negotiation_data.maker_coin_htlc_pub_from_maker,
            swap_unique_data: &unique_data,
            amount: state_machine.maker_volume.to_decimal(),
        };
        let maker_payment_spend = match state_machine.maker_coin.spend_maker_payment_v2(args).await {
            Ok(tx) => tx,
            Err(e) => {
                let reason = AbortReason::FailedToSpendMakerPayment(format!("{e:?}"));
                return Self::change_state(Aborted::new(reason), state_machine).await;
            },
        };
        info!(
            "Spent maker payment {} tx {:02x} during swap {}",
            state_machine.maker_coin.ticker(),
            maker_payment_spend.tx_hash_as_bytes(),
            state_machine.uuid
        );
        let next_state = MakerPaymentSpent {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            maker_payment: self.maker_payment,
            taker_payment: self.taker_payment,
            taker_payment_spend: self.taker_payment_spend,
            maker_payment_spend,
            negotiation_data: self.negotiation_data,
        };
        Self::change_state(next_state, state_machine).await
    }
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for TakerPaymentSpent<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::TakerPaymentSpent {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            maker_payment: TransactionIdentifier {
                tx_hex: self.maker_payment.tx_hex().into(),
                tx_hash: self.maker_payment.tx_hash_as_bytes(),
            },
            taker_payment: TransactionIdentifier {
                tx_hex: self.taker_payment.tx_hex().into(),
                tx_hash: self.taker_payment.tx_hash_as_bytes(),
            },
            taker_payment_spend: TransactionIdentifier {
                tx_hex: self.taker_payment_spend.tx_hex().into(),
                tx_hash: self.taker_payment_spend.tx_hash_as_bytes(),
            },
            negotiation_data: self.negotiation_data.to_stored_data(),
        }
    }
}

struct MakerPaymentSpent<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_coin_start_block: u64,
    taker_coin_start_block: u64,
    maker_payment: MakerCoin::Tx,
    taker_payment: TakerCoin::Tx,
    taker_payment_spend: TakerCoin::Tx,
    maker_payment_spend: MakerCoin::Tx,
    negotiation_data: NegotiationData<MakerCoin, TakerCoin>,
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: TakerCoinSwapOpsV2>
    TransitionFrom<TakerPaymentSpent<MakerCoin, TakerCoin>> for MakerPaymentSpent<MakerCoin, TakerCoin>
{
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for MakerPaymentSpent<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::MakerPaymentSpent {
            maker_coin_start_block: self.maker_coin_start_block,
            taker_coin_start_block: self.taker_coin_start_block,
            maker_payment: TransactionIdentifier {
                tx_hex: self.maker_payment.tx_hex().into(),
                tx_hash: self.maker_payment.tx_hash_as_bytes(),
            },
            taker_payment: TransactionIdentifier {
                tx_hex: self.taker_payment.tx_hex().into(),
                tx_hash: self.taker_payment.tx_hash_as_bytes(),
            },
            taker_payment_spend: TransactionIdentifier {
                tx_hex: self.taker_payment_spend.tx_hex().into(),
                tx_hash: self.taker_payment_spend.tx_hash_as_bytes(),
            },
            maker_payment_spend: TransactionIdentifier {
                tx_hex: self.maker_payment_spend.tx_hex().into(),
                tx_hash: self.maker_payment_spend.tx_hash_as_bytes(),
            },
            negotiation_data: self.negotiation_data.to_stored_data(),
        }
    }
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> State
    for MakerPaymentSpent<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(self: Box<Self>, state_machine: &mut Self::StateMachine) -> StateResult<Self::StateMachine> {
        if state_machine.require_maker_payment_spend_confirm {
            let input = ConfirmPaymentInput {
                payment_tx: self.maker_payment_spend.tx_hex(),
                confirmations: state_machine.conf_settings.maker_coin_confs,
                requires_nota: state_machine.conf_settings.maker_coin_nota,
                wait_until: state_machine.taker_payment_locktime(),
                check_every: 10,
            };

            if let Err(e) = state_machine.maker_coin.wait_for_confirmations(input).compat().await {
                let next_state = TakerPaymentRefundRequired {
                    taker_payment: self.taker_payment,
                    negotiation_data: self.negotiation_data,
                    reason: TakerPaymentRefundReason::MakerPaymentSpendNotConfirmedInTime(e.to_string()),
                };
                return Self::change_state(next_state, state_machine).await;
            }
        }

        Self::change_state(Completed::new(), state_machine).await
    }
}

/// Represents possible reasons of taker swap being aborted
#[derive(Clone, Debug, Deserialize, Display, Serialize)]
pub enum AbortReason {
    FailedToGetMakerCoinBlock(String),
    FailedToGetTakerCoinBlock(String),
    BalanceCheckFailure(String),
    DidNotReceiveMakerNegotiation(String),
    TooLargeStartedAtDiff(u64),
    FailedToParsePubkey(String),
    FailedToParseAddress(String),
    MakerProvidedInvalidLocktime(u64),
    SecretHashUnexpectedLen(usize),
    DidNotReceiveMakerNegotiated(String),
    MakerDidNotNegotiate(String),
    FailedToSendTakerFunding(String),
    CouldNotExtractSecret(String),
    FailedToSpendMakerPayment(String),
    TakerFundingRefundFailed(String),
    TakerPaymentRefundFailed(String),
    FailedToGetTakerPaymentFee(String),
    FailedToGetMakerPaymentSpendFee(String),
}

struct Aborted<MakerCoin, TakerCoin> {
    maker_coin: PhantomData<MakerCoin>,
    taker_coin: PhantomData<TakerCoin>,
    reason: AbortReason,
}

impl<MakerCoin, TakerCoin> Aborted<MakerCoin, TakerCoin> {
    fn new(reason: AbortReason) -> Aborted<MakerCoin, TakerCoin> {
        Aborted {
            maker_coin: Default::default(),
            taker_coin: Default::default(),
            reason,
        }
    }
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> LastState
    for Aborted<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(
        self: Box<Self>,
        state_machine: &mut Self::StateMachine,
    ) -> <Self::StateMachine as StateMachineTrait>::Result {
        warn!("Swap {} was aborted with reason {}", state_machine.uuid, self.reason);
    }
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for Aborted<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::Aborted {
            reason: self.reason.clone(),
        }
    }
}

impl<MakerCoin, TakerCoin> TransitionFrom<Initialize<MakerCoin, TakerCoin>> for Aborted<MakerCoin, TakerCoin> {}
impl<MakerCoin, TakerCoin> TransitionFrom<Initialized<MakerCoin, TakerCoin>> for Aborted<MakerCoin, TakerCoin> {}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: TakerCoinSwapOpsV2> TransitionFrom<Negotiated<MakerCoin, TakerCoin>>
    for Aborted<MakerCoin, TakerCoin>
{
}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: TakerCoinSwapOpsV2>
    TransitionFrom<TakerPaymentSpent<MakerCoin, TakerCoin>> for Aborted<MakerCoin, TakerCoin>
{
}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: TakerCoinSwapOpsV2>
    TransitionFrom<TakerFundingRefundRequired<MakerCoin, TakerCoin>> for Aborted<MakerCoin, TakerCoin>
{
}
impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: TakerCoinSwapOpsV2>
    TransitionFrom<TakerPaymentRefundRequired<MakerCoin, TakerCoin>> for Aborted<MakerCoin, TakerCoin>
{
}

struct Completed<MakerCoin, TakerCoin> {
    maker_coin: PhantomData<MakerCoin>,
    taker_coin: PhantomData<TakerCoin>,
}

impl<MakerCoin, TakerCoin> Completed<MakerCoin, TakerCoin> {
    fn new() -> Completed<MakerCoin, TakerCoin> {
        Completed {
            maker_coin: Default::default(),
            taker_coin: Default::default(),
        }
    }
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for Completed<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::Completed
    }
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> LastState
    for Completed<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(
        self: Box<Self>,
        state_machine: &mut Self::StateMachine,
    ) -> <Self::StateMachine as StateMachineTrait>::Result {
        info!("Swap {} has been completed", state_machine.uuid);
    }
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<MakerPaymentSpent<MakerCoin, TakerCoin>> for Completed<MakerCoin, TakerCoin>
{
}

struct TakerFundingRefunded<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_coin: PhantomData<MakerCoin>,
    funding_tx: TakerCoin::Tx,
    funding_refund_tx: TakerCoin::Tx,
    reason: TakerFundingRefundReason,
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for TakerFundingRefunded<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::TakerFundingRefunded {
            funding_tx: TransactionIdentifier {
                tx_hex: self.funding_tx.tx_hex().into(),
                tx_hash: self.funding_tx.tx_hash_as_bytes(),
            },
            funding_tx_refund: TransactionIdentifier {
                tx_hex: self.funding_refund_tx.tx_hex().into(),
                tx_hash: self.funding_refund_tx.tx_hash_as_bytes(),
            },
            reason: self.reason.clone(),
        }
    }
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> LastState
    for TakerFundingRefunded<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(
        self: Box<Self>,
        state_machine: &mut Self::StateMachine,
    ) -> <Self::StateMachine as StateMachineTrait>::Result {
        info!(
            "Swap {} has been completed with taker funding refund",
            state_machine.uuid
        );
    }
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<TakerFundingRefundRequired<MakerCoin, TakerCoin>> for TakerFundingRefunded<MakerCoin, TakerCoin>
{
}

struct TakerPaymentRefunded<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes> {
    maker_coin: PhantomData<MakerCoin>,
    taker_payment: TakerCoin::Tx,
    taker_payment_refund: TransactionIdentifier,
    reason: TakerPaymentRefundReason,
}

impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> StorableState
    for TakerPaymentRefunded<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    fn get_event(&self) -> TakerSwapEvent {
        TakerSwapEvent::TakerPaymentRefunded {
            taker_payment: TransactionIdentifier {
                tx_hex: self.taker_payment.tx_hex().into(),
                tx_hash: self.taker_payment.tx_hash_as_bytes(),
            },
            taker_payment_refund: self.taker_payment_refund.clone(),
            reason: self.reason.clone(),
        }
    }
}

#[async_trait]
impl<MakerCoin: MmCoin + MakerCoinSwapOpsV2, TakerCoin: MmCoin + TakerCoinSwapOpsV2> LastState
    for TakerPaymentRefunded<MakerCoin, TakerCoin>
{
    type StateMachine = TakerSwapStateMachine<MakerCoin, TakerCoin>;

    async fn on_changed(
        self: Box<Self>,
        state_machine: &mut Self::StateMachine,
    ) -> <Self::StateMachine as StateMachineTrait>::Result {
        info!(
            "Swap {} has been completed with taker payment refund",
            state_machine.uuid
        );
    }
}

impl<MakerCoin: ParseCoinAssocTypes, TakerCoin: ParseCoinAssocTypes>
    TransitionFrom<TakerPaymentRefundRequired<MakerCoin, TakerCoin>> for TakerPaymentRefunded<MakerCoin, TakerCoin>
{
}
