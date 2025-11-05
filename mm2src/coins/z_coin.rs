#![expect(clippy::result_large_err)]

pub mod storage;
pub mod tx_history_events;
#[cfg_attr(not(target_arch = "wasm32"), cfg(test))]
mod tx_streaming_tests;
pub mod z_balance_streaming;
mod z_coin_errors;
mod z_htlc;
mod z_rpc;
mod z_tx_history;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod z_unit_tests;

use crate::coin_errors::{AddressFromPubkeyError, MyAddressError, ValidatePaymentResult};
use crate::hd_wallet::{HDAddressSelector, HDPathAccountToAddressId};
use crate::my_tx_history_v2::{MyTxHistoryErrorV2, MyTxHistoryRequestV2, MyTxHistoryResponseV2};
use crate::rpc_command::init_withdraw::{InitWithdrawCoin, WithdrawInProgressStatus, WithdrawTaskHandleShared};
use crate::utxo::rpc_clients::{
    ElectrumConnectionSettings, UnspentInfo, UtxoRpcClientEnum, UtxoRpcError, UtxoRpcFut, UtxoRpcResult,
};
use crate::utxo::utxo_builder::{UtxoCoinBuildError, UtxoCoinBuilder, UtxoCoinBuilderCommonOps};
use crate::utxo::utxo_common::{
    addresses_from_script, big_decimal_from_sat, big_decimal_from_sat_unsigned, payment_script,
};
use crate::utxo::{
    sat_from_big_decimal, utxo_common, ActualFeeRate, AdditionalTxData, AddrFromStrError, Address, BroadcastTxErr,
    FeePolicy, GetUtxoListOps, HistoryUtxoTx, HistoryUtxoTxMap, MatureUnspentList, RecentlySpentOutPointsGuard,
    UnsupportedAddr, UtxoActivationParams, UtxoAddressFormat, UtxoArc, UtxoCoinFields, UtxoCommonOps, UtxoFeeDetails,
    UtxoRpcMode, UtxoTxBroadcastOps, UtxoTxGenerationOps, VerboseTransactionFrom,
};
use crate::z_coin::storage::{BlockDbImpl, LockedNotesStorage, WalletDbShared};
use crate::z_coin::z_tx_history::{fetch_tx_history_from_db, ZCoinTxHistoryItem};
use crate::{
    BalanceError, BalanceFut, CheckIfMyPaymentSentArgs, CoinBalance, ConfirmPaymentInput, DexFee, FeeApproxStage,
    FoundSwapTxSpend, HistorySyncState, MarketCoinOps, MmCoin, NegotiateSwapContractAddrErr, NumConversError,
    PrivKeyActivationPolicy, PrivKeyBuildPolicy, PrivKeyPolicyNotAllowed, RawTransactionFut, RawTransactionRequest,
    RawTransactionResult, RefundPaymentArgs, SearchForSwapTxSpendInput, SendPaymentArgs, SignRawTransactionRequest,
    SignatureError, SignatureResult, SpendPaymentArgs, SwapOps, TradeFee, TradePreimageFut, TradePreimageResult,
    TradePreimageValue, Transaction, TransactionData, TransactionDetails, TransactionEnum, TransactionResult,
    TxFeeDetails, TxMarshalingErr, UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs,
    ValidateOtherPubKeyErr, ValidatePaymentError, ValidatePaymentInput, VerificationError, VerificationResult,
    WaitForHTLCTxSpendArgs, WatcherOps, WeakSpawner, WithdrawError, WithdrawFut, WithdrawRequest,
};

use crate::z_coin::storage::z_locked_notes::LockedNote;
use async_trait::async_trait;
use bitcrypto::dhash256;
use chain::constants::SEQUENCE_FINAL;
use chain::{Transaction as UtxoTx, TransactionOutput};
use common::executor::{AbortableSystem, AbortedError, SpawnFuture};
use common::log::info;
use common::{calc_total_pages, log};
use crypto::privkey::{key_pair_from_secret, secp_privkey_from_hash};
use crypto::HDPathToCoin;
use crypto::{Bip32DerPathOps, GlobalHDAccountArc};
use futures::channel::oneshot;
use futures::compat::Future01CompatExt;
use futures::lock::Mutex as AsyncMutex;
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use keys::hash::H256;
use keys::{KeyPair, Message, Public};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
#[cfg(test)]
use mocktopus::macros::*;
use rpc::v1::types::{Bytes as BytesJson, Transaction as RpcTransaction, H256 as H256Json, H264 as H264Json};
use script::{Builder as ScriptBuilder, Opcode, Script, TransactionInputSigner};
use serde_json::Value as Json;
use serialization::CoinVariant;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::iter;
use std::num::NonZeroU32;
use std::num::TryFromIntError;
use std::sync::Arc;
pub use z_coin_errors::*;
pub use z_htlc::z_send_dex_fee;
use z_htlc::{z_p2sh_spend, z_send_htlc};
use z_rpc::init_light_client;
pub use z_rpc::{FirstSyncBlock, SyncStatus};
use z_rpc::{SaplingSyncConnector, SaplingSyncGuard};
use zcash_client_backend::encoding::{decode_payment_address, encode_extended_spending_key, encode_payment_address};
use zcash_client_backend::wallet::{AccountId, SpendableNote};
use zcash_extras::WalletRead;
use zcash_primitives::consensus::{BlockHeight, BranchId, NetworkUpgrade, Parameters, H0};
use zcash_primitives::memo::MemoBytes;
use zcash_primitives::sapling::keys::prf_expand;
use zcash_primitives::sapling::keys::OutgoingViewingKey;
use zcash_primitives::sapling::note_encryption::try_sapling_output_recovery;
use zcash_primitives::sapling::Rseed;
use zcash_primitives::transaction::builder::Builder as ZTxBuilder;
use zcash_primitives::transaction::components::{Amount, OutputDescription, TxOut};
use zcash_primitives::transaction::Transaction as ZTransaction;
use zcash_primitives::zip32::ChildIndex as Zip32Child;
use zcash_primitives::{
    constants::mainnet as z_mainnet_constants, sapling::PaymentAddress, zip32::ExtendedFullViewingKey,
    zip32::ExtendedSpendingKey,
};
use zcash_proofs::prover::LocalTxProver;

cfg_native!(
    use common::{async_blocking, sha256_digest};
    use std::path::PathBuf;
    use zcash_proofs::default_params_folder;
    use z_rpc::init_native_client;
);

cfg_wasm32!(
    use crate::z_coin::storage::ZcashParamsWasmImpl;
    use common::executor::AbortOnDropHandle;
    use rand::rngs::OsRng;
    use zcash_primitives::transaction::builder::TransactionMetadata;
    pub use z_coin_errors::ZCoinBalanceError;
);

/// `ZP2SHSpendError` compatible `TransactionErr` handling macro.
macro_rules! try_ztx_s {
    ($e: expr) => {
        match $e {
            Ok(ok) => ok,
            Err(err) => {
                if let Some(tx) = err.get_inner().get_tx() {
                    return Err(crate::TransactionErr::TxRecoverable(
                        tx,
                        format!("{}:{}] {:?}", file!(), line!(), err),
                    ));
                }

                return Err(crate::TransactionErr::Plain(ERRL!("{:?}", err)));
            },
        }
    };
}

const DEX_FEE_OVK: OutgoingViewingKey = OutgoingViewingKey([7; 32]);
const DEX_FEE_Z_ADDR: &str = "zs1rp6426e9r6jkq2nsanl66tkd34enewrmr0uvj0zelhkcwmsy0uvxz2fhm9eu9rl3ukxvgzy2v9f";
const DEX_BURN_Z_ADDR: &str = "zs1ntx28kyurgvsc7rxgkdhasz8p6wzv63nqpcayvnh7c4r6cs4wfkz8ztkwazjzdsxkgaq6erscyl";
cfg_native!(
    #[cfg(test)]
    const DOWNLOAD_URL: &str = "https://komodoplatform.com/downloads";
    const SAPLING_OUTPUT_NAME: &str = "sapling-output.params";
    const SAPLING_SPEND_NAME: &str = "sapling-spend.params";
    const BLOCKS_TABLE: &str = "blocks";
    const SAPLING_SPEND_EXPECTED_HASH: &str = "8e48ffd23abb3a5fd9c5589204f32d9c31285a04b78096ba40a79b75677efc13";
    const SAPLING_OUTPUT_EXPECTED_HASH: &str = "2f0ebbcbb9bb0bcffe95a397e7eba89c29eb4dde6191c339db88570e3f3fb0e4";
);

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ZcoinConsensusParams {
    // we don't support coins without overwinter and sapling active so these are mandatory
    overwinter_activation_height: u32,
    sapling_activation_height: u32,
    // optional upgrades that we will possibly support in the future
    blossom_activation_height: Option<u32>,
    heartwood_activation_height: Option<u32>,
    canopy_activation_height: Option<u32>,
    coin_type: u32,
    hrp_sapling_extended_spending_key: String,
    hrp_sapling_extended_full_viewing_key: String,
    hrp_sapling_payment_address: String,
    b58_pubkey_address_prefix: [u8; 2],
    b58_script_address_prefix: [u8; 2],
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CheckPointBlockInfo {
    height: u32,
    hash: H256Json,
    time: u32,
    sapling_tree: BytesJson,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ZcoinProtocolInfo {
    consensus_params: ZcoinConsensusParams,
    check_point_block: Option<CheckPointBlockInfo>,
    // `z_derivation_path` can be the same or different from [`UtxoCoinFields::derivation_path`].
    z_derivation_path: Option<HDPathToCoin>,
}

impl Parameters for ZcoinConsensusParams {
    fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
        match nu {
            NetworkUpgrade::Overwinter => Some(BlockHeight::from(self.overwinter_activation_height)),
            NetworkUpgrade::Sapling => Some(BlockHeight::from(self.sapling_activation_height)),
            NetworkUpgrade::Blossom => self.blossom_activation_height.map(BlockHeight::from),
            NetworkUpgrade::Heartwood => self.heartwood_activation_height.map(BlockHeight::from),
            NetworkUpgrade::Canopy => self.canopy_activation_height.map(BlockHeight::from),
        }
    }

    fn coin_type(&self) -> u32 {
        self.coin_type
    }

    fn hrp_sapling_extended_spending_key(&self) -> &str {
        &self.hrp_sapling_extended_spending_key
    }

    fn hrp_sapling_extended_full_viewing_key(&self) -> &str {
        &self.hrp_sapling_extended_full_viewing_key
    }

    fn hrp_sapling_payment_address(&self) -> &str {
        &self.hrp_sapling_payment_address
    }

    fn b58_pubkey_address_prefix(&self) -> [u8; 2] {
        self.b58_pubkey_address_prefix
    }

    fn b58_script_address_prefix(&self) -> [u8; 2] {
        self.b58_script_address_prefix
    }
}

#[allow(unused)]
pub struct ZCoinFields {
    dex_fee_addr: PaymentAddress,
    dex_burn_addr: PaymentAddress,
    my_z_addr: PaymentAddress,
    my_z_addr_encoded: String,
    z_spending_key: ExtendedSpendingKey,
    evk: ExtendedFullViewingKey,
    z_tx_prover: Arc<LocalTxProver>,
    light_wallet_db: WalletDbShared,
    consensus_params: ZcoinConsensusParams,
    sync_state_connector: AsyncMutex<SaplingSyncConnector>,
    locked_notes_db: LockedNotesStorage,
}

impl Transaction for ZTransaction {
    fn tx_hex(&self) -> Vec<u8> {
        let mut hex = Vec::with_capacity(1024);
        self.write(&mut hex).expect("Writing should not fail");
        hex
    }

    fn tx_hash_as_bytes(&self) -> BytesJson {
        let mut bytes = self.txid().0.to_vec();
        bytes.reverse();
        bytes.into()
    }
}

#[derive(Clone)]
pub struct ZCoin {
    utxo_arc: UtxoArc,
    z_fields: Arc<ZCoinFields>,
}

pub struct ZOutput {
    pub to_addr: PaymentAddress,
    pub amount: Amount,
    pub viewing_key: Option<OutgoingViewingKey>,
    pub memo: Option<MemoBytes>,
}

#[derive(Serialize)]
pub struct ZcoinTxDetails {
    /// Transaction hash in hexadecimal format
    tx_hash: String,
    /// Coins are sent from these addresses
    from: HashSet<String>,
    /// Coins are sent to these addresses
    to: HashSet<String>,
    /// The amount spent from "my" address
    spent_by_me: BigDecimal,
    /// The amount received by "my" address
    received_by_me: BigDecimal,
    /// Resulting "my" balance change
    my_balance_change: BigDecimal,
    /// Block height
    block_height: i64,
    confirmations: i64,
    /// Transaction timestamp
    timestamp: i64,
    transaction_fee: BigDecimal,
    /// The coin transaction belongs to
    coin: String,
    /// Internal MM2 id used for internal transaction identification, for some coins it might be equal to transaction hash
    internal_id: i64,
}

struct GenTxData<'a> {
    tx: ZTransaction,
    data: AdditionalTxData,
    sync_guard: SaplingSyncGuard<'a>,
    rseeds: Vec<String>,
}

impl ZCoin {
    #[inline]
    pub fn utxo_rpc_client(&self) -> &UtxoRpcClientEnum {
        &self.utxo_arc.rpc_client
    }

    #[inline]
    pub fn my_z_address_encoded(&self) -> String {
        self.z_fields.my_z_addr_encoded.clone()
    }

    #[inline]
    pub fn consensus_params(&self) -> ZcoinConsensusParams {
        self.z_fields.consensus_params.clone()
    }

    #[inline]
    pub fn consensus_params_ref(&self) -> &ZcoinConsensusParams {
        &self.z_fields.consensus_params
    }

    /// Asynchronously checks the synchronization status and returns `true` if
    /// the Sapling state has finished synchronizing, meaning that the block number is available.
    /// Otherwise, it returns `false`.
    #[cfg(any(test, feature = "run-docker-tests"))]
    #[inline]
    pub async fn is_sapling_state_synced(&self) -> bool {
        use futures::StreamExt;

        let mut watcher = self.z_fields.sync_state_connector.lock().await;
        while let Some(sync) = watcher.sync_watcher.next().await {
            if matches!(sync, SyncStatus::Finished { .. }) {
                return true;
            }
        }

        false
    }

    #[inline]
    pub async fn sync_status(&self) -> Result<SyncStatus, MmError<BlockchainScanStopped>> {
        self.z_fields
            .sync_state_connector
            .lock()
            .await
            .current_sync_status()
            .await
    }

    #[inline]
    pub async fn first_sync_block(&self) -> Result<FirstSyncBlock, MmError<BlockchainScanStopped>> {
        self.z_fields.sync_state_connector.lock().await.first_sync_block().await
    }

    #[inline]
    fn secp_keypair(&self) -> &KeyPair {
        self.utxo_arc
            .priv_key_policy
            .activated_key()
            .expect("Zcoin doesn't support HW wallets")
    }

    async fn wait_for_gen_tx_blockchain_sync(&self) -> Result<SaplingSyncGuard<'_>, MmError<BlockchainScanStopped>> {
        let mut connector_guard = self.z_fields.sync_state_connector.lock().await;
        let sync_respawn_guard = connector_guard.wait_for_gen_tx_blockchain_sync().await?;
        Ok(SaplingSyncGuard {
            _connector_guard: connector_guard,
            respawn_guard: sync_respawn_guard,
        })
    }

    async fn get_wallet_notes(&self) -> Result<Vec<SpendableNote>, MmError<SpendableNotesError>> {
        let wallet_db = self.z_fields.light_wallet_db.clone();
        let db_guard = wallet_db.db;
        let latest_db_block = match db_guard
            .block_height_extrema()
            .await
            .map_err(|err| SpendableNotesError::DBClientError(err.to_string()))?
        {
            Some((_, latest)) => latest,
            None => return Ok(Vec::new()),
        };

        db_guard
            .get_spendable_notes(AccountId::default(), latest_db_block)
            .await
            .map_err(|err| MmError::new(SpendableNotesError::DBClientError(err.to_string())))
    }

    /// Returns spendable notes
    async fn wallet_notes_ordered(&self) -> Result<Vec<SpendableNote>, MmError<SpendableNotesError>> {
        let mut unspents = self.get_wallet_notes().await?;

        unspents.sort_unstable_by(|a, b| a.note_value.cmp(&b.note_value));
        Ok(unspents)
    }

    async fn get_one_kbyte_tx_fee(&self) -> UtxoRpcResult<BigDecimal> {
        let fee = self.get_fee_rate().await?;
        match fee {
            ActualFeeRate::Dynamic(fee) | ActualFeeRate::FixedPerKb(fee) | ActualFeeRate::FixedPerKbDingo(fee) => {
                Ok(big_decimal_from_sat_unsigned(fee, self.decimals()))
            },
        }
    }

    /// Generates a tx sending outputs from our address
    async fn gen_tx(
        &self,
        t_outputs: Vec<TxOut>,
        z_outputs: Vec<ZOutput>,
    ) -> Result<GenTxData<'_>, MmError<GenTxError>> {
        // Wait for chain to sync before selecting spendable notes or waiting for locked_notes to become
        // available.
        let sync_guard = self.wait_for_gen_tx_blockchain_sync().await.map_mm_err()?;
        drop(sync_guard);
        let tx_fee = self.get_one_kbyte_tx_fee().await.map_mm_err()?;
        let t_output_sat: u64 = t_outputs.iter().fold(0, |cur, out| cur + u64::from(out.value));
        let z_output_sat: u64 = z_outputs.iter().fold(0, |cur, out| cur + u64::from(out.amount));
        let total_output_sat = t_output_sat + z_output_sat;
        let total_output = big_decimal_from_sat_unsigned(total_output_sat, self.utxo_arc.decimals);
        let total_required = &total_output + &tx_fee;
        let spendable_notes = wait_for_spendable_balance_spawner(self, &total_required).await?;

        // Recreate sync_guard
        let sync_guard = self.wait_for_gen_tx_blockchain_sync().await.map_mm_err()?;

        let mut total_input_amount = BigDecimal::from(0);
        let mut change = BigDecimal::from(0);
        let mut received_by_me = 0u64;
        let mut tx_builder = ZTxBuilder::new(self.consensus_params(), sync_guard.respawn_guard.current_block());

        let mut rseeds: Vec<String> = vec![];
        for spendable_note in spendable_notes {
            total_input_amount += big_decimal_from_sat_unsigned(spendable_note.note_value.into(), self.decimals());

            let note = self
                .z_fields
                .my_z_addr
                .create_note(spendable_note.note_value.into(), spendable_note.rseed)
                .or_mm_err(|| GenTxError::FailedToCreateNote)?;
            tx_builder.add_sapling_spend(
                self.z_fields.z_spending_key.clone(),
                *self.z_fields.my_z_addr.diversifier(),
                note,
                spendable_note
                    .witness
                    .path()
                    .or_mm_err(|| GenTxError::FailedToGetMerklePath)?,
            )?;

            rseeds.push(rseed_to_string(&spendable_note.rseed));

            if total_input_amount >= total_required {
                change = &total_input_amount - &total_required;
                break;
            }
        }

        if total_input_amount < total_required {
            return MmError::err(GenTxError::InsufficientBalance {
                coin: self.ticker().into(),
                available: total_input_amount,
                required: total_required,
            });
        }

        for z_out in z_outputs {
            if z_out.to_addr == self.z_fields.my_z_addr {
                received_by_me += u64::from(z_out.amount);
            }

            tx_builder.add_sapling_output(z_out.viewing_key, z_out.to_addr, z_out.amount, z_out.memo)?;
        }

        // add change to tx output
        let change_sat = sat_from_big_decimal(&change, self.utxo_arc.decimals).map_mm_err()?;
        if change > BigDecimal::from(0u8) {
            received_by_me += change_sat;
            let change_amount = Amount::from_u64(change_sat).map_to_mm(|_| {
                GenTxError::NumConversion(NumConversError(format!("Failed to get ZCash amount from {change_sat}")))
            })?;

            tx_builder.add_sapling_output(
                Some(self.z_fields.evk.fvk.ovk),
                self.z_fields.my_z_addr.clone(),
                change_amount,
                None,
            )?;
        }

        for output in t_outputs {
            tx_builder.add_tx_out(output);
        }

        #[cfg(not(target_arch = "wasm32"))]
        let (tx, _) = async_blocking({
            let prover = self.z_fields.z_tx_prover.clone();
            move || tx_builder.build(BranchId::Sapling, prover.as_ref())
        })
        .await?;

        #[cfg(target_arch = "wasm32")]
        let (tx, _) =
            TxBuilderSpawner::request_tx_result(tx_builder, BranchId::Sapling, self.z_fields.z_tx_prover.clone())
                .await
                .map_mm_err()?
                .tx_result
                .map_mm_err()?;

        let data = AdditionalTxData {
            received_by_me,
            spent_by_me: sat_from_big_decimal(&total_input_amount, self.decimals()).map_mm_err()?,
            fee_amount: sat_from_big_decimal(&tx_fee, self.decimals()).map_mm_err()?,
            kmd_rewards: None,
        };

        Ok(GenTxData {
            tx,
            data,
            sync_guard,
            rseeds,
        })
    }

    pub async fn send_outputs(
        &self,
        t_outputs: Vec<TxOut>,
        z_outputs: Vec<ZOutput>,
    ) -> Result<ZTransaction, MmError<SendOutputsErr>> {
        let GenTxData {
            tx,
            data,
            rseeds,
            mut sync_guard,
        } = self.gen_tx(t_outputs, z_outputs).await.map_mm_err()?;
        let mut tx_bytes = Vec::with_capacity(1024);
        tx.write(&mut tx_bytes).expect("Write should not fail");

        self.utxo_rpc_client()
            .send_raw_transaction(tx_bytes.into())
            .compat()
            .await
            .map_mm_err()?;

        // TODO: Execute updates to `locked_notes_db` and `wallet_db` in a single transaction.
        // This will be possible with a newer librustzcash that supports both spent notes and unconfirmed change tracking.
        // See: https://github.com/KomodoPlatform/komodo-defi-framework/pull/2331#pullrequestreview-2883773336
        for rseed in rseeds {
            self.z_fields
                .locked_notes_db
                .insert_spent_note(tx.txid().to_string(), rseed)
                .await
                .mm_err(|err| SendOutputsErr::InternalError(err.to_string()))?;
        }

        if data.received_by_me > 0 {
            self.z_fields
                .locked_notes_db
                .insert_change_note(tx.txid().to_string(), data.received_by_me)
                .await
                .mm_err(|err| SendOutputsErr::InternalError(err.to_string()))?;
        }

        sync_guard.respawn_guard.watch_for_tx(tx.txid());
        Ok(tx)
    }

    async fn z_transactions_from_cache_or_rpc(
        &self,
        hashes: HashSet<H256Json>,
    ) -> UtxoRpcResult<HashMap<H256Json, ZTransaction>> {
        self.get_verbose_transactions_from_cache_or_rpc(hashes)
            .compat()
            .await?
            .into_iter()
            .map(|(hash, tx)| -> Result<_, std::io::Error> {
                Ok((hash, ZTransaction::read(tx.into_inner().hex.as_slice())?))
            })
            .collect::<Result<_, _>>()
            .map_to_mm(|e| UtxoRpcError::InvalidResponse(e.to_string()))
    }

    fn tx_details_from_db_item(
        &self,
        tx_item: ZCoinTxHistoryItem,
        transactions: &HashMap<H256Json, ZTransaction>,
        prev_transactions: &HashMap<H256Json, ZTransaction>,
        current_block: u64,
    ) -> Result<ZcoinTxDetails, MmError<NoInfoAboutTx>> {
        let mut from = HashSet::new();

        let mut confirmations = current_block as i64 - tx_item.height + 1;
        if confirmations < 0 {
            confirmations = 0;
        }

        let mut transparent_input_amount = Amount::zero();
        let hash = H256Json::from(tx_item.tx_hash);
        let z_tx = transactions.get(&hash).or_mm_err(|| NoInfoAboutTx(hash))?;
        for input in z_tx.vin.iter() {
            let mut hash = H256Json::from(*input.prevout.hash());
            hash.0.reverse();
            let prev_tx = prev_transactions.get(&hash).or_mm_err(|| NoInfoAboutTx(hash))?;

            if let Some(spent_output) = prev_tx.vout.get(input.prevout.n() as usize) {
                transparent_input_amount += spent_output.value;
                if let Ok(addresses) = addresses_from_script(self, &spent_output.script_pubkey.0.clone().into()) {
                    from.extend(addresses.into_iter().map(|a| a.to_string()));
                }
            }
        }

        let transparent_output_amount = z_tx
            .vout
            .iter()
            .fold(Amount::zero(), |current, out| current + out.value);

        let mut to = HashSet::new();
        for out in z_tx.vout.iter() {
            if let Ok(addresses) = addresses_from_script(self, &out.script_pubkey.0.clone().into()) {
                to.extend(addresses.into_iter().map(|a| a.to_string()));
            }
        }

        let fee_amount = z_tx.value_balance + transparent_input_amount - transparent_output_amount;
        if tx_item.spent_amount > 0 {
            from.insert(self.my_z_address_encoded());
        }

        if tx_item.received_amount > 0 {
            to.insert(self.my_z_address_encoded());
        }

        for z_out in z_tx.shielded_outputs.iter() {
            if let Some((_, address, _)) = try_sapling_output_recovery(
                self.consensus_params_ref(),
                BlockHeight::from_u32(current_block as u32),
                &self.z_fields.evk.fvk.ovk,
                z_out,
            ) {
                to.insert(encode_payment_address(
                    self.consensus_params_ref().hrp_sapling_payment_address(),
                    &address,
                ));
            }

            if let Some((_, address, _)) = try_sapling_output_recovery(
                self.consensus_params_ref(),
                BlockHeight::from_u32(current_block as u32),
                &DEX_FEE_OVK,
                z_out,
            ) {
                to.insert(encode_payment_address(
                    self.consensus_params_ref().hrp_sapling_payment_address(),
                    &address,
                ));
            }
        }

        let spent_by_me = big_decimal_from_sat(tx_item.spent_amount, self.decimals());
        let received_by_me = big_decimal_from_sat(tx_item.received_amount, self.decimals());
        Ok(ZcoinTxDetails {
            tx_hash: hex::encode(tx_item.tx_hash),
            from,
            to,
            my_balance_change: &received_by_me - &spent_by_me,
            spent_by_me,
            received_by_me,
            block_height: tx_item.height,
            confirmations,
            timestamp: tx_item.timestamp,
            transaction_fee: big_decimal_from_sat(fee_amount.into(), self.decimals()),
            coin: self.ticker().into(),
            internal_id: tx_item.internal_id,
        })
    }

    pub async fn tx_history(
        &self,
        request: MyTxHistoryRequestV2<i64>,
    ) -> Result<MyTxHistoryResponseV2<ZcoinTxDetails, i64>, MmError<MyTxHistoryErrorV2>> {
        let current_block = self.utxo_rpc_client().get_block_count().compat().await.map_mm_err()?;
        let req_result = fetch_tx_history_from_db(self, request.limit, request.paging_options.clone())
            .await
            .map_mm_err()?;

        let hashes_for_verbose = req_result
            .transactions
            .iter()
            .map(|item| H256Json::from(item.tx_hash))
            .collect();
        let transactions = self
            .z_transactions_from_cache_or_rpc(hashes_for_verbose)
            .await
            .map_mm_err()?;

        let prev_tx_hashes: HashSet<_> = transactions
            .iter()
            .flat_map(|(_, tx)| {
                tx.vin.iter().map(|vin| {
                    let mut hash = *vin.prevout.hash();
                    hash.reverse();
                    H256Json::from(hash)
                })
            })
            .collect();
        let prev_transactions = self
            .z_transactions_from_cache_or_rpc(prev_tx_hashes)
            .await
            .map_mm_err()?;

        let transactions = req_result
            .transactions
            .into_iter()
            .map(|sql_item| self.tx_details_from_db_item(sql_item, &transactions, &prev_transactions, current_block))
            .collect::<Result<_, _>>()
            .map_mm_err()?;

        Ok(MyTxHistoryResponseV2 {
            coin: self.ticker().into(),
            target: request.target,
            current_block,
            transactions,
            // Zcoin is activated only after the state is synced
            sync_status: HistorySyncState::Finished,
            limit: request.limit,
            skipped: req_result.skipped,
            total: req_result.total_tx_count as usize,
            total_pages: calc_total_pages(req_result.total_tx_count as usize, request.limit),
            paging_options: request.paging_options,
        })
    }

    /// Validates dex fee output or burn output
    /// Returns true if the output valid or error if not valid. Returns false if could not decrypt output (some other output)
    fn validate_dex_fee_output(
        &self,
        shielded_out: &OutputDescription,
        ovk: &OutgoingViewingKey,
        expected_address: &PaymentAddress,
        block_height: BlockHeight,
        amount_sat: u64,
        expected_memo: &MemoBytes,
    ) -> Result<bool, String> {
        let Some((note, address, memo)) =
            try_sapling_output_recovery(self.consensus_params_ref(), block_height, ovk, shielded_out)
        else {
            return Ok(false);
        };
        if &address != expected_address {
            return Ok(false);
        }
        if note.value != amount_sat {
            return Err(format!("invalid amount {}, expected {}", note.value, amount_sat));
        }
        if &memo != expected_memo {
            return Err(format!("invalid memo {memo:?}, expected {expected_memo:?}"));
        }
        Ok(true)
    }
}

impl AsRef<UtxoCoinFields> for ZCoin {
    fn as_ref(&self) -> &UtxoCoinFields {
        &self.utxo_arc
    }
}

#[cfg(target_arch = "wasm32")]
type TxResult = MmResult<(zcash_primitives::transaction::Transaction, TransactionMetadata), GenTxError>;

#[cfg(target_arch = "wasm32")]
/// Spawns an asynchronous task to build a transaction and sends the result through a oneshot channel.
pub(crate) struct TxBuilderSpawner {
    pub(crate) tx_result: TxResult,
    _abort_handle: AbortOnDropHandle,
}

#[cfg(target_arch = "wasm32")]
impl TxBuilderSpawner {
    fn spawn_build_tx(
        builder: ZTxBuilder<'static, ZcoinConsensusParams, OsRng>,
        branch_id: BranchId,
        prover: Arc<LocalTxProver>,
        sender: oneshot::Sender<TxResult>,
    ) -> AbortOnDropHandle {
        let fut = async move {
            sender
                .send(
                    builder
                        .build(branch_id, prover.as_ref())
                        .map_to_mm(GenTxError::TxBuilderError),
                )
                .ok();
        };

        common::executor::spawn_local_abortable(fut)
    }

    /// Requests a transaction asynchronously using the provided builder, branch ID, and prover.
    pub(crate) async fn request_tx_result(
        builder: ZTxBuilder<'static, ZcoinConsensusParams, OsRng>,
        branch_id: BranchId,
        prover: Arc<LocalTxProver>,
    ) -> MmResult<Self, GenTxError> {
        // Create a oneshot channel for communication between the spawned task and this function
        let (tx, rx) = oneshot::channel();
        let abort_handle = Self::spawn_build_tx(builder, branch_id, prover, tx);

        Ok(Self {
            tx_result: rx
                .await
                .map_to_mm(|_| GenTxError::Internal("Spawned future has been canceled".to_owned()))?,
            _abort_handle: abort_handle,
        })
    }
}

/// SyncStartPoint represents the starting point for synchronizing a wallet's blocks and transaction history.
/// This can be specified as a date, a block height, or starting from the earliest available data.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncStartPoint {
    /// Synchronize from a specific date (in Unix timestamp format).
    Date(u64),
    /// Synchronize from a specific block height.
    Height(u64),
    /// Synchronize from the earliest available data(sapling_activation_height from coin config).
    Earliest,
}

// ZcoinRpcMode reprs available RPC modes for interacting with the Zcoin network. It includes
/// modes for both native and light client, each with their own configuration options.
#[allow(unused)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "rpc", content = "rpc_data")]
pub enum ZcoinRpcMode {
    #[cfg(not(target_arch = "wasm32"))]
    Native,
    #[serde(alias = "Electrum")]
    Light {
        #[serde(alias = "servers")]
        /// The settings of each electrum server.
        electrum_servers: Vec<ElectrumConnectionSettings>,
        /// The minimum number of connections to electrum servers to keep alive/maintained at all times.
        min_connected: Option<usize>,
        /// The maximum number of connections to electrum servers to not exceed at any time.
        max_connected: Option<usize>,
        light_wallet_d_servers: Vec<String>,
        /// Specifies the parameters for synchronizing the wallet from a specific block. This overrides the
        /// `CheckPointBlockInfo` configuration in the coin settings.
        sync_params: Option<SyncStartPoint>,
        /// Indicates that synchronization parameters will be skipped and continue sync from last synced block.
        /// Will use `sync_params` if no last synced block found.
        skip_sync_params: Option<bool>,
    },
    #[cfg(test)]
    UnitTests,
}

#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ZcoinActivationParams {
    pub mode: ZcoinRpcMode,
    pub required_confirmations: Option<u64>,
    pub requires_notarization: Option<bool>,
    pub zcash_params_path: Option<String>,
    pub scan_blocks_per_iteration: NonZeroU32,
    pub scan_interval_ms: u64,
    pub account: u32,
}

impl Default for ZcoinActivationParams {
    fn default() -> Self {
        Self {
            mode: ZcoinRpcMode::Light {
                electrum_servers: Vec::new(),
                min_connected: None,
                max_connected: None,
                light_wallet_d_servers: Vec::new(),
                sync_params: None,
                skip_sync_params: None,
            },
            required_confirmations: None,
            requires_notarization: None,
            zcash_params_path: None,
            scan_blocks_per_iteration: NonZeroU32::new(1000).expect("1000 is a valid value"),
            scan_interval_ms: Default::default(),
            account: Default::default(),
        }
    }
}

pub async fn z_coin_from_conf_and_params(
    ctx: &MmArc,
    ticker: &str,
    conf: &Json,
    params: &ZcoinActivationParams,
    protocol_info: ZcoinProtocolInfo,
    priv_key_policy: PrivKeyBuildPolicy,
) -> Result<ZCoin, MmError<ZCoinBuildError>> {
    let z_spending_key = None;
    let builder = ZCoinBuilder::new(
        ctx,
        ticker,
        conf,
        params,
        priv_key_policy,
        z_spending_key,
        protocol_info,
    )?;
    builder.build().await
}

#[cfg(not(target_arch = "wasm32"))]
fn verify_checksum_zcash_params(spend_path: &PathBuf, output_path: &PathBuf) -> Result<bool, ZCoinBuildError> {
    let spend_hash = sha256_digest(spend_path)?;
    let out_hash = sha256_digest(output_path)?;
    Ok(spend_hash == SAPLING_SPEND_EXPECTED_HASH && out_hash == SAPLING_OUTPUT_EXPECTED_HASH)
}

#[cfg(not(target_arch = "wasm32"))]
fn get_spend_output_paths(params_dir: PathBuf) -> Result<(PathBuf, PathBuf), ZCoinBuildError> {
    if !params_dir.exists() {
        return Err(ZCoinBuildError::ZCashParamsNotFound);
    };
    let spend_path = params_dir.join(SAPLING_SPEND_NAME);
    let output_path = params_dir.join(SAPLING_OUTPUT_NAME);

    if !(spend_path.exists() && output_path.exists()) {
        return Err(ZCoinBuildError::ZCashParamsNotFound);
    }
    Ok((spend_path, output_path))
}

pub struct ZCoinBuilder<'a> {
    ctx: &'a MmArc,
    ticker: &'a str,
    conf: &'a Json,
    z_coin_params: &'a ZcoinActivationParams,
    utxo_params: UtxoActivationParams,
    priv_key_policy: PrivKeyBuildPolicy,
    z_spending_key: ExtendedSpendingKey,
    my_z_addr: PaymentAddress,
    my_z_addr_encoded: String,
    protocol_info: ZcoinProtocolInfo,
}

impl UtxoCoinBuilderCommonOps for ZCoinBuilder<'_> {
    fn ctx(&self) -> &MmArc {
        self.ctx
    }

    fn conf(&self) -> &Json {
        self.conf
    }

    fn activation_params(&self) -> &UtxoActivationParams {
        &self.utxo_params
    }

    fn ticker(&self) -> &str {
        self.ticker
    }
}

#[async_trait]
impl UtxoCoinBuilder for ZCoinBuilder<'_> {
    type ResultCoin = ZCoin;
    type Error = ZCoinBuildError;

    fn priv_key_policy(&self) -> PrivKeyBuildPolicy {
        self.priv_key_policy.clone()
    }

    async fn build(self) -> MmResult<Self::ResultCoin, Self::Error> {
        let utxo = self.build_utxo_fields().await.map_mm_err()?;
        let utxo_arc = UtxoArc::new(utxo);

        let dex_fee_addr = decode_payment_address(
            self.protocol_info.consensus_params.hrp_sapling_payment_address(),
            DEX_FEE_Z_ADDR,
        )
        .expect("DEX_FEE_Z_ADDR is a valid z-address")
        .expect("DEX_FEE_Z_ADDR is a valid z-address");

        let dex_burn_addr = decode_payment_address(
            self.protocol_info.consensus_params.hrp_sapling_payment_address(),
            DEX_BURN_Z_ADDR,
        )
        .expect("DEX_BURN_Z_ADDR is a valid z-address")
        .expect("DEX_BURN_Z_ADDR is a valid z-address");

        let z_tx_prover = self.z_tx_prover().await?;
        let blocks_db = self.init_blocks_db().await.map_mm_err()?;
        let locked_notes_db = LockedNotesStorage::new(self.ctx, self.my_z_addr_encoded.clone())
            .await
            .map_mm_err()?;

        let (sync_state_connector, light_wallet_db) = match &self.z_coin_params.mode {
            #[cfg(not(target_arch = "wasm32"))]
            ZcoinRpcMode::Native => init_native_client(
                &self,
                self.native_client().map_mm_err()?,
                blocks_db,
                locked_notes_db.clone(),
            )
            .await
            .map_mm_err()?,
            ZcoinRpcMode::Light {
                light_wallet_d_servers,
                sync_params,
                skip_sync_params,
                ..
            } => init_light_client(
                &self,
                light_wallet_d_servers.clone(),
                blocks_db,
                sync_params,
                skip_sync_params.unwrap_or_default(),
                locked_notes_db.clone(),
            )
            .await
            .map_mm_err()?,
            #[cfg(test)]
            ZcoinRpcMode::UnitTests => z_unit_tests::create_test_sync_connector(&self).await,
        };

        let z_fields = Arc::new(ZCoinFields {
            dex_fee_addr,
            dex_burn_addr,
            my_z_addr: self.my_z_addr,
            my_z_addr_encoded: self.my_z_addr_encoded,
            evk: ExtendedFullViewingKey::from(&self.z_spending_key),
            z_spending_key: self.z_spending_key,
            z_tx_prover: Arc::new(z_tx_prover),
            light_wallet_db,
            consensus_params: self.protocol_info.consensus_params,
            sync_state_connector,
            locked_notes_db,
        });

        Ok(ZCoin { utxo_arc, z_fields })
    }
}

impl<'a> ZCoinBuilder<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &'a MmArc,
        ticker: &'a str,
        conf: &'a Json,
        z_coin_params: &'a ZcoinActivationParams,
        priv_key_policy: PrivKeyBuildPolicy,
        z_spending_key: Option<ExtendedSpendingKey>,
        protocol_info: ZcoinProtocolInfo,
    ) -> MmResult<ZCoinBuilder<'a>, ZCoinBuildError> {
        let utxo_mode = match &z_coin_params.mode {
            #[cfg(not(target_arch = "wasm32"))]
            ZcoinRpcMode::Native => UtxoRpcMode::Native,
            ZcoinRpcMode::Light {
                electrum_servers,
                min_connected,
                max_connected,
                ..
            } => UtxoRpcMode::Electrum {
                servers: electrum_servers.clone(),
                min_connected: *min_connected,
                max_connected: *max_connected,
            },
            #[cfg(test)]
            ZcoinRpcMode::UnitTests => UtxoRpcMode::Electrum {
                servers: vec![],
                min_connected: None,
                max_connected: Some(1),
            },
        };
        let utxo_params = UtxoActivationParams {
            mode: utxo_mode,
            utxo_merge_params: None,
            tx_history: false,
            required_confirmations: z_coin_params.required_confirmations,
            requires_notarization: z_coin_params.requires_notarization,
            address_format: None,
            gap_limit: None,
            enable_params: Default::default(),
            priv_key_policy: PrivKeyActivationPolicy::ContextPrivKey,
            check_utxo_maturity: None,
            // This is not used for Zcoin so we just provide a default value
            path_to_address: HDPathAccountToAddressId::default(),
        };

        let z_spending_key = match z_spending_key {
            Some(ref z_spending_key) => z_spending_key.clone(),
            None => extended_spending_key_from_protocol_info_and_policy(
                &protocol_info,
                &priv_key_policy,
                z_coin_params.account,
            )?,
        };

        let (_, my_z_addr) = z_spending_key
            .default_address()
            .map_to_mm(|_| ZCoinBuildError::GetAddressError)?;

        let my_z_addr_encoded =
            encode_payment_address(protocol_info.consensus_params.hrp_sapling_payment_address(), &my_z_addr);

        Ok(ZCoinBuilder {
            ctx,
            ticker,
            conf,
            z_coin_params,
            utxo_params,
            priv_key_policy,
            z_spending_key,
            my_z_addr,
            my_z_addr_encoded,
            protocol_info,
        })
    }

    async fn init_blocks_db(&self) -> Result<BlockDbImpl, MmError<ZcoinClientInitError>> {
        let ctx = &self.ctx;
        let ticker = self.ticker.to_string();

        BlockDbImpl::new(ctx, ticker)
            .await
            .mm_err(|err| ZcoinClientInitError::ZcoinStorageError(err.to_string()))
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn z_tx_prover(&self) -> Result<LocalTxProver, MmError<ZCoinBuildError>> {
        let params_dir = match &self.z_coin_params.zcash_params_path {
            None => default_params_folder().or_mm_err(|| ZCoinBuildError::ZCashParamsNotFound)?,
            Some(file_path) => PathBuf::from(file_path),
        };

        #[cfg(test)]
        z_unit_tests::download_parameters_for_tests(&params_dir).await;

        async_blocking(move || {
            let (spend_path, output_path) = get_spend_output_paths(params_dir)?;
            let verification_successful = verify_checksum_zcash_params(&spend_path, &output_path)?;
            if verification_successful {
                Ok(LocalTxProver::new(&spend_path, &output_path))
            } else {
                MmError::err(ZCoinBuildError::SaplingParamsInvalidChecksum)
            }
        })
        .await
    }

    #[cfg(target_arch = "wasm32")]
    async fn z_tx_prover(&self) -> Result<LocalTxProver, MmError<ZCoinBuildError>> {
        let params_db = ZcashParamsWasmImpl::new(self.ctx)
            .await
            .mm_err(|err| ZCoinBuildError::ZCashParamsError(err.to_string()))?;
        let (sapling_spend, sapling_output) = if !params_db
            .check_params()
            .await
            .mm_err(|err| ZCoinBuildError::ZCashParamsError(err.to_string()))?
        {
            params_db
                .download_and_save_params()
                .await
                .mm_err(|err| ZCoinBuildError::ZCashParamsError(err.to_string()))?
        } else {
            // get params
            params_db
                .get_params()
                .await
                .mm_err(|err| ZCoinBuildError::ZCashParamsError(err.to_string()))?
        };

        Ok(LocalTxProver::from_bytes(&sapling_spend[..], &sapling_output[..]))
    }
}

/// Initialize `ZCoin` with a forced `z_spending_key` for dockerized tests.
/// db_dir_path is where ZOMBIE_wallet.db located
/// Note that ZOMBIE_cache.db (db where blocks are downloaded to create ZOMBIE_wallet.db) is created in-memory (see BlockDbImpl::new fn)
#[cfg(any(test, feature = "run-docker-tests"))]
#[allow(clippy::too_many_arguments)]
pub async fn z_coin_from_conf_and_params_with_docker(
    ctx: &MmArc,
    ticker: &str,
    conf: &Json,
    params: &ZcoinActivationParams,
    priv_key_policy: PrivKeyBuildPolicy,
    protocol_info: ZcoinProtocolInfo,
    spending_key: &str,
) -> Result<ZCoin, MmError<ZCoinBuildError>> {
    use zcash_client_backend::encoding::decode_extended_spending_key;
    let z_spending_key =
        decode_extended_spending_key(z_mainnet_constants::HRP_SAPLING_EXTENDED_SPENDING_KEY, spending_key)
            .unwrap()
            .unwrap();

    let builder = ZCoinBuilder::new(
        ctx,
        ticker,
        conf,
        params,
        priv_key_policy,
        Some(z_spending_key),
        protocol_info,
    )?;

    println!("ZOMBIE_wallet.db will be synch'ed with the chain, this may take a while for the first time.");
    println!("You may also run prepare_zombie_sapling_cache test to update ZOMBIE_wallet.db before running tests.");
    builder.build().await
}

#[async_trait]
impl MarketCoinOps for ZCoin {
    fn ticker(&self) -> &str {
        &self.utxo_arc.conf.ticker
    }

    fn my_address(&self) -> MmResult<String, MyAddressError> {
        Ok(self.z_fields.my_z_addr_encoded.clone())
    }

    fn address_from_pubkey(&self, _pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError> {
        // NOTE: We can't derive a z-address from pubkey, so we will just return our own z_address.
        Ok(self.z_fields.my_z_addr_encoded.clone())
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        let pubkey = utxo_common::my_public_key(self.as_ref())?;
        Ok(pubkey.to_string())
    }

    fn sign_message_hash(&self, _message: &str) -> Option<[u8; 32]> {
        None
    }

    fn sign_message(&self, _message: &str, _address: Option<HDAddressSelector>) -> SignatureResult<String> {
        MmError::err(SignatureError::InvalidRequest(
            "Message signing is not supported by the given coin type".to_string(),
        ))
    }

    fn verify_message(&self, _signature_base64: &str, _message: &str, _address: &str) -> VerificationResult<bool> {
        MmError::err(VerificationError::InvalidRequest(
            "Message verification is not supported by the given coin type".to_string(),
        ))
    }

    /// Calculates the wallet balance, divided into spendable and unspendable portions.
    /// Unspendable balance consists of notes that are locked in the wallet.
    /// TODO: Track unconfirmed change outputs in a dedicated DB/table (similar to locked_notes_db).
    /// - Include them in the unspendable portion of the balance until confirmed.
    /// - This will improve spendable/unspendable accuracy.
    fn my_balance(&self) -> BalanceFut<CoinBalance> {
        let coin = self.clone();
        let fut = async move {
            let locked_notes = coin
                .z_fields
                .locked_notes_db
                .load_all_notes()
                .await
                .mm_err(|e| BalanceError::WalletStorageError(e.to_string()))?;

            // Locked (unconfirmed) spent notes are not counted as spendable.
            let spent_rseeds: HashSet<_> = locked_notes
                .iter()
                .filter_map(|n| {
                    if let LockedNote::Spent { rseed, .. } = n {
                        Some(rseed.clone())
                    } else {
                        None
                    }
                })
                .collect();

            // Locked (unconfirmed) change notes are counted as unspendable.
            let unspendable_change_sat: u64 = locked_notes
                .iter()
                .filter_map(|n| {
                    if let LockedNote::Change { value, .. } = n {
                        Some(*value)
                    } else {
                        None
                    }
                })
                .sum();

            let wallet_notes = coin
                .get_wallet_notes()
                .await
                .map_err(|err| BalanceError::WalletStorageError(err.to_string()))?;

            let spendable_amount = wallet_notes
                .iter()
                .filter(|n| !spent_rseeds.contains(&rseed_to_string(&n.rseed)))
                .fold(Amount::zero(), |acc, n| acc + n.note_value);

            let spendable_sat = u64::from(spendable_amount);
            let unspendable = big_decimal_from_sat_unsigned(unspendable_change_sat, coin.decimals());
            let spendable = big_decimal_from_sat_unsigned(spendable_sat, coin.decimals());
            Ok(CoinBalance { spendable, unspendable })
        };

        Box::new(fut.boxed().compat())
    }

    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal> {
        utxo_common::platform_coin_balance(self)
    }

    fn platform_ticker(&self) -> &str {
        self.ticker()
    }

    fn send_raw_tx(&self, tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send> {
        let tx_bytes = try_fus!(hex::decode(tx));
        let z_tx = try_fus!(ZTransaction::read(tx_bytes.as_slice()));

        let this = self.clone();
        let tx = tx.to_owned();

        let fut = async move {
            let mut sync_guard = try_s!(this.wait_for_gen_tx_blockchain_sync().await);
            let tx_hash = utxo_common::send_raw_tx(this.as_ref(), &tx).compat().await?;
            sync_guard.respawn_guard.watch_for_tx(z_tx.txid());
            Ok(tx_hash)
        };
        Box::new(fut.boxed().compat())
    }

    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        let z_tx = try_fus!(ZTransaction::read(tx));

        let this = self.clone();
        let tx = tx.to_owned();

        let fut = async move {
            let mut sync_guard = try_s!(this.wait_for_gen_tx_blockchain_sync().await);
            let tx_hash = utxo_common::send_raw_tx_bytes(this.as_ref(), &tx).compat().await?;
            sync_guard.respawn_guard.watch_for_tx(z_tx.txid());
            Ok(tx_hash)
        };
        Box::new(fut.boxed().compat())
    }

    #[inline(always)]
    async fn sign_raw_tx(&self, args: &SignRawTransactionRequest) -> RawTransactionResult {
        utxo_common::sign_raw_tx(self, args).await
    }

    fn wait_for_confirmations(&self, input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        utxo_common::wait_for_confirmations(self.as_ref(), input)
    }

    async fn wait_for_htlc_tx_spend(&self, args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult {
        utxo_common::wait_for_output_spend(
            self.clone(),
            args.tx_bytes,
            utxo_common::DEFAULT_SWAP_VOUT,
            args.from_block,
            args.wait_until,
            args.check_every,
        )
        .await
    }

    fn tx_enum_from_bytes(&self, bytes: &[u8]) -> Result<TransactionEnum, MmError<TxMarshalingErr>> {
        ZTransaction::read(bytes)
            .map(TransactionEnum::from)
            .map_to_mm(|e| TxMarshalingErr::InvalidInput(e.to_string()))
    }

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send> {
        utxo_common::current_block(&self.utxo_arc)
    }

    fn display_priv_key(&self) -> Result<String, String> {
        Ok(encode_extended_spending_key(
            z_mainnet_constants::HRP_SAPLING_EXTENDED_SPENDING_KEY,
            &self.z_fields.z_spending_key,
        ))
    }

    fn min_tx_amount(&self) -> BigDecimal {
        utxo_common::min_tx_amount(self.as_ref())
    }

    fn min_trading_vol(&self) -> MmNumber {
        utxo_common::min_trading_vol(self.as_ref())
    }

    fn is_privacy(&self) -> bool {
        true
    }

    fn should_burn_dex_fee(&self) -> bool {
        false
    } // TODO: enable when burn z_address fixed

    fn is_trezor(&self) -> bool {
        self.as_ref().priv_key_policy.is_trezor()
    }
}

#[async_trait]
impl SwapOps for ZCoin {
    async fn send_taker_fee(&self, dex_fee: DexFee, uuid: &[u8], _expire_at: u64) -> TransactionResult {
        let uuid = uuid.to_owned();
        let tx = try_tx_s!(z_send_dex_fee(self, dex_fee, &uuid).await);
        Ok(tx.into())
    }

    async fn send_maker_payment(&self, maker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        let maker_key_pair = self.derive_htlc_key_pair(maker_payment_args.swap_unique_data);
        let taker_pub = try_tx_s!(Public::from_slice(maker_payment_args.other_pubkey));
        let secret_hash = maker_payment_args.secret_hash.to_vec();
        let time_lock = try_tx_s!(maker_payment_args.time_lock.try_into());
        let amount = maker_payment_args.amount;
        let utxo_tx = try_tx_s!(
            z_send_htlc(
                self,
                time_lock,
                maker_key_pair.public(),
                &taker_pub,
                &secret_hash,
                amount
            )
            .await
        );
        Ok(utxo_tx.into())
    }

    async fn send_taker_payment(&self, taker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        let taker_keypair = self.derive_htlc_key_pair(taker_payment_args.swap_unique_data);
        let maker_pub = try_tx_s!(Public::from_slice(taker_payment_args.other_pubkey));
        let secret_hash = taker_payment_args.secret_hash.to_vec();
        let time_lock = try_tx_s!(taker_payment_args.time_lock.try_into());
        let amount = taker_payment_args.amount;
        let utxo_tx = try_tx_s!(
            z_send_htlc(
                self,
                time_lock,
                taker_keypair.public(),
                &maker_pub,
                &secret_hash,
                amount
            )
            .await
        );
        Ok(utxo_tx.into())
    }

    async fn send_maker_spends_taker_payment(
        &self,
        maker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        let tx = try_tx_s!(ZTransaction::read(maker_spends_payment_args.other_payment_tx));
        let key_pair = self.derive_htlc_key_pair(maker_spends_payment_args.swap_unique_data);
        let time_lock = try_tx_s!(maker_spends_payment_args.time_lock.try_into());
        let redeem_script = payment_script(
            time_lock,
            maker_spends_payment_args.secret_hash,
            &try_tx_s!(Public::from_slice(maker_spends_payment_args.other_pubkey)),
            key_pair.public(),
        );
        let script_data = ScriptBuilder::default()
            .push_data(maker_spends_payment_args.secret)
            .push_opcode(Opcode::OP_0)
            .into_script();
        let tx = try_ztx_s!(
            z_p2sh_spend(
                self,
                tx,
                time_lock,
                SEQUENCE_FINAL,
                redeem_script,
                script_data,
                &key_pair,
            )
            .await
        );
        Ok(tx.into())
    }

    async fn send_taker_spends_maker_payment(
        &self,
        taker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        let tx = try_tx_s!(ZTransaction::read(taker_spends_payment_args.other_payment_tx));
        let key_pair = self.derive_htlc_key_pair(taker_spends_payment_args.swap_unique_data);
        let time_lock = try_tx_s!(taker_spends_payment_args.time_lock.try_into());
        let redeem_script = payment_script(
            time_lock,
            taker_spends_payment_args.secret_hash,
            &try_tx_s!(Public::from_slice(taker_spends_payment_args.other_pubkey)),
            key_pair.public(),
        );
        let script_data = ScriptBuilder::default()
            .push_data(taker_spends_payment_args.secret)
            .push_opcode(Opcode::OP_0)
            .into_script();
        let tx = try_ztx_s!(
            z_p2sh_spend(
                self,
                tx,
                time_lock,
                SEQUENCE_FINAL,
                redeem_script,
                script_data,
                &key_pair,
            )
            .await
        );
        Ok(tx.into())
    }

    async fn send_taker_refunds_payment(&self, taker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        let tx = try_tx_s!(ZTransaction::read(taker_refunds_payment_args.payment_tx));
        let key_pair = self.derive_htlc_key_pair(taker_refunds_payment_args.swap_unique_data);
        let time_lock = try_tx_s!(taker_refunds_payment_args.time_lock.try_into());
        let redeem_script = taker_refunds_payment_args.tx_type_with_secret_hash.redeem_script(
            time_lock,
            key_pair.public(),
            &try_tx_s!(Public::from_slice(taker_refunds_payment_args.other_pubkey)),
        );
        let script_data = ScriptBuilder::default().push_opcode(Opcode::OP_1).into_script();

        let tx_fut = z_p2sh_spend(
            self,
            tx,
            time_lock,
            SEQUENCE_FINAL - 1,
            redeem_script,
            script_data,
            &key_pair,
        );
        let tx = try_ztx_s!(tx_fut.await);
        Ok(tx.into())
    }

    async fn send_maker_refunds_payment(&self, maker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        let tx = try_tx_s!(ZTransaction::read(maker_refunds_payment_args.payment_tx));
        let key_pair = self.derive_htlc_key_pair(maker_refunds_payment_args.swap_unique_data);
        let time_lock = try_tx_s!(maker_refunds_payment_args.time_lock.try_into());
        let redeem_script = maker_refunds_payment_args.tx_type_with_secret_hash.redeem_script(
            time_lock,
            key_pair.public(),
            &try_tx_s!(Public::from_slice(maker_refunds_payment_args.other_pubkey)),
        );
        let script_data = ScriptBuilder::default().push_opcode(Opcode::OP_1).into_script();
        let tx_fut = z_p2sh_spend(
            self,
            tx,
            time_lock,
            SEQUENCE_FINAL - 1,
            redeem_script,
            script_data,
            &key_pair,
        );
        let tx = try_ztx_s!(tx_fut.await);
        Ok(tx.into())
    }

    /// Currently validates both Standard and WithBurn options for DexFee
    /// TODO: when all mm2 nodes upgrade to support the burn account then disable validation of the Standard option
    async fn validate_fee(&self, validate_fee_args: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()> {
        let z_tx = match validate_fee_args.fee_tx {
            TransactionEnum::ZTransaction(t) => t,
            fee_tx => {
                return MmError::err(ValidatePaymentError::InternalError(format!(
                    "Invalid fee tx type. fee tx: {fee_tx:?}"
                )))
            },
        };
        let fee_amount_sat = validate_fee_args
            .dex_fee
            .fee_amount_as_u64(self.utxo_arc.decimals)
            .map_mm_err()?;
        let burn_amount_sat = validate_fee_args
            .dex_fee
            .burn_amount_as_u64(self.utxo_arc.decimals)
            .map_mm_err()?;
        let expected_memo = MemoBytes::from_bytes(validate_fee_args.uuid).expect("Uuid length < 512");

        let tx_hash = H256::from(z_tx.txid().0).reversed();
        let tx_from_rpc = self
            .utxo_rpc_client()
            .get_verbose_transaction(&tx_hash.into())
            .compat()
            .await
            .mm_err(|e| ValidatePaymentError::InvalidRpcResponse(e.to_string()))?;

        let mut encoded = Vec::with_capacity(1024);
        z_tx.write(&mut encoded).expect("Writing should not fail");
        if encoded != tx_from_rpc.hex.0 {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Encoded transaction {encoded:?} does not match the tx {tx_from_rpc:?} from RPC"
            )));
        }

        let block_height = match tx_from_rpc.height {
            Some(h) => {
                if h < validate_fee_args.min_block_number {
                    return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                        "Dex fee tx {:?} confirmed before min block {}",
                        z_tx, validate_fee_args.min_block_number
                    )));
                } else {
                    BlockHeight::from_u32(h as u32)
                }
            },
            None => H0,
        };

        let mut fee_output_valid = false;
        let mut burn_output_valid = false;
        for shielded_out in z_tx.shielded_outputs.iter() {
            if self
                .validate_dex_fee_output(
                    shielded_out,
                    &DEX_FEE_OVK,
                    &self.z_fields.dex_fee_addr,
                    block_height,
                    fee_amount_sat,
                    &expected_memo,
                )
                .map_err(|err| {
                    MmError::new(ValidatePaymentError::WrongPaymentTx(format!(
                        "Bad dex fee output: {err}"
                    )))
                })?
            {
                fee_output_valid = true;
            }
            if let Some(burn_amount_sat) = burn_amount_sat {
                if self
                    .validate_dex_fee_output(
                        shielded_out,
                        &DEX_FEE_OVK,
                        &self.z_fields.dex_burn_addr,
                        block_height,
                        burn_amount_sat,
                        &expected_memo,
                    )
                    .map_err(|err| {
                        MmError::new(ValidatePaymentError::WrongPaymentTx(format!("Bad burn output: {err}")))
                    })?
                {
                    burn_output_valid = true;
                }
            }
        }

        if fee_output_valid && (burn_amount_sat.is_none() || burn_output_valid) {
            return Ok(());
        }

        MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
            "The dex fee tx {z_tx:?} has no shielded outputs or outputs decryption failed"
        )))
    }

    #[inline]
    async fn validate_maker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        utxo_common::validate_maker_payment(self, input).await
    }

    #[inline]
    async fn validate_taker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        utxo_common::validate_taker_payment(self, input).await
    }

    #[inline]
    async fn check_if_my_payment_sent(
        &self,
        if_my_payment_sent_args: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<TransactionEnum>, String> {
        let time_lock = if_my_payment_sent_args
            .time_lock
            .try_into()
            .map_err(|e: TryFromIntError| e.to_string())?;
        utxo_common::check_if_my_payment_sent(
            self.clone(),
            time_lock,
            if_my_payment_sent_args.other_pub,
            if_my_payment_sent_args.secret_hash,
            if_my_payment_sent_args.swap_unique_data,
        )
        .compat()
        .await
    }

    #[inline]
    async fn search_for_swap_tx_spend_my(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        utxo_common::search_for_swap_tx_spend_my(self, input, utxo_common::DEFAULT_SWAP_VOUT).await
    }

    #[inline]
    async fn search_for_swap_tx_spend_other(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        utxo_common::search_for_swap_tx_spend_other(self, input, utxo_common::DEFAULT_SWAP_VOUT).await
    }

    #[inline]
    async fn extract_secret(
        &self,
        secret_hash: &[u8],
        spend_tx: &[u8],
        _watcher_reward: bool,
    ) -> Result<[u8; 32], String> {
        utxo_common::extract_secret(secret_hash, spend_tx)
    }

    #[inline]
    fn negotiate_swap_contract_addr(
        &self,
        _other_side_address: Option<&[u8]>,
    ) -> Result<Option<BytesJson>, MmError<NegotiateSwapContractAddrErr>> {
        Ok(None)
    }

    fn derive_htlc_key_pair(&self, swap_unique_data: &[u8]) -> KeyPair {
        let message = Message::from(dhash256(swap_unique_data).take());
        let signature = self.secp_keypair().private().sign(&message).expect("valid privkey");

        let key = secp_privkey_from_hash(dhash256(&signature));
        key_pair_from_secret(&key.take()).expect("valid privkey")
    }

    #[inline]
    fn derive_htlc_pubkey(&self, swap_unique_data: &[u8]) -> [u8; 33] {
        self.derive_htlc_key_pair(swap_unique_data)
            .public_slice()
            .to_vec()
            .try_into()
            .expect("valid pubkey length")
    }

    #[inline]
    fn validate_other_pubkey(&self, raw_pubkey: &[u8]) -> MmResult<(), ValidateOtherPubKeyErr> {
        utxo_common::validate_other_pubkey(raw_pubkey)
    }
}

#[async_trait]
impl WatcherOps for ZCoin {}

#[async_trait]
impl MmCoin for ZCoin {
    fn is_asset_chain(&self) -> bool {
        self.utxo_arc.conf.asset_chain
    }

    fn spawner(&self) -> WeakSpawner {
        self.as_ref().abortable_system.weak_spawner()
    }

    fn withdraw(&self, _req: WithdrawRequest) -> WithdrawFut {
        Box::new(futures01::future::err(MmError::new(WithdrawError::InternalError(
            "Zcoin doesn't support legacy withdraw".into(),
        ))))
    }

    fn get_raw_transaction(&self, req: RawTransactionRequest) -> RawTransactionFut<'_> {
        Box::new(utxo_common::get_raw_transaction(&self.utxo_arc, req).boxed().compat())
    }

    fn get_tx_hex_by_hash(&self, tx_hash: Vec<u8>) -> RawTransactionFut<'_> {
        Box::new(
            utxo_common::get_tx_hex_by_hash(&self.utxo_arc, tx_hash)
                .boxed()
                .compat(),
        )
    }

    fn decimals(&self) -> u8 {
        self.utxo_arc.decimals
    }

    fn convert_to_address(&self, _from: &str, _to_address_format: Json) -> Result<String, String> {
        Err(MmError::new("Address conversion is not available for ZCoin".to_string()).to_string())
    }

    fn validate_address(&self, address: &str) -> ValidateAddressResult {
        match decode_payment_address(z_mainnet_constants::HRP_SAPLING_PAYMENT_ADDRESS, address) {
            Ok(Some(_)) => ValidateAddressResult {
                is_valid: true,
                reason: None,
            },
            Ok(None) => ValidateAddressResult {
                is_valid: false,
                reason: Some("decode_payment_address returned None".to_owned()),
            },
            Err(e) => ValidateAddressResult {
                is_valid: false,
                reason: Some(format!("Error {e} on decode_payment_address")),
            },
        }
    }

    fn process_history_loop(&self, _ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        log::warn!("process_history_loop is not implemented for ZCoin yet!");
        Box::new(futures01::future::err(()))
    }

    fn history_sync_status(&self) -> HistorySyncState {
        HistorySyncState::NotEnabled
    }

    fn get_trade_fee(&self) -> Box<dyn Future<Item = TradeFee, Error = String> + Send> {
        utxo_common::get_trade_fee(self.clone())
    }

    async fn get_sender_trade_fee(
        &self,
        _value: TradePreimageValue,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        Ok(TradeFee {
            coin: self.ticker().to_owned(),
            amount: self.get_one_kbyte_tx_fee().await.map_mm_err()?.into(),
            paid_from_trading_vol: false,
        })
    }

    fn get_receiver_trade_fee(&self, _stage: FeeApproxStage) -> TradePreimageFut<TradeFee> {
        utxo_common::get_receiver_trade_fee(self.clone())
    }

    async fn get_fee_to_send_taker_fee(
        &self,
        _dex_fee_amount: DexFee,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        Ok(TradeFee {
            coin: self.ticker().to_owned(),
            amount: self.get_one_kbyte_tx_fee().await.map_mm_err()?.into(),
            paid_from_trading_vol: false,
        })
    }

    fn required_confirmations(&self) -> u64 {
        utxo_common::required_confirmations(&self.utxo_arc)
    }

    fn requires_notarization(&self) -> bool {
        utxo_common::requires_notarization(&self.utxo_arc)
    }

    fn set_required_confirmations(&self, confirmations: u64) {
        utxo_common::set_required_confirmations(&self.utxo_arc, confirmations)
    }

    fn set_requires_notarization(&self, requires_nota: bool) {
        utxo_common::set_requires_notarization(&self.utxo_arc, requires_nota)
    }

    fn swap_contract_address(&self) -> Option<BytesJson> {
        utxo_common::swap_contract_address()
    }

    fn fallback_swap_contract(&self) -> Option<BytesJson> {
        utxo_common::fallback_swap_contract()
    }

    fn mature_confirmations(&self) -> Option<u32> {
        Some(self.utxo_arc.conf.mature_confirmations)
    }

    fn coin_protocol_info(&self, _amount_to_receive: Option<MmNumber>) -> Vec<u8> {
        utxo_common::coin_protocol_info(self)
    }

    fn is_coin_protocol_supported(
        &self,
        info: &Option<Vec<u8>>,
        _amount_to_send: Option<MmNumber>,
        _locktime: u64,
        _is_maker: bool,
    ) -> bool {
        utxo_common::is_coin_protocol_supported(self, info)
    }

    fn on_disabled(&self) -> Result<(), AbortedError> {
        AbortableSystem::abort_all(&self.as_ref().abortable_system)
    }

    fn on_token_deactivated(&self, _ticker: &str) {}
}

#[async_trait]
impl UtxoTxGenerationOps for ZCoin {
    async fn get_fee_rate(&self) -> UtxoRpcResult<ActualFeeRate> {
        utxo_common::get_fee_rate(&self.utxo_arc).await
    }

    async fn calc_interest_if_required(&self, unsigned: &mut TransactionInputSigner) -> UtxoRpcResult<u64> {
        utxo_common::calc_interest_if_required(self, unsigned).await
    }

    fn supports_interest(&self) -> bool {
        utxo_common::is_kmd(self)
    }
}

#[async_trait]
impl UtxoTxBroadcastOps for ZCoin {
    async fn broadcast_tx(&self, tx: &UtxoTx) -> Result<H256Json, MmError<BroadcastTxErr>> {
        utxo_common::broadcast_tx(self, tx).await
    }
}

/// Please note `ZCoin` is not assumed to work with transparent UTXOs.
/// Remove implementation of the `GetUtxoListOps` trait for `ZCoin`
/// when [`ZCoin::preimage_trade_fee_required_to_send_outputs`] is refactored.
#[async_trait]
#[cfg_attr(test, mockable)]
impl GetUtxoListOps for ZCoin {
    async fn get_unspent_ordered_list(
        &self,
        address: &Address,
    ) -> UtxoRpcResult<(Vec<UnspentInfo>, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_unspent_ordered_list(self, address).await
    }

    async fn get_all_unspent_ordered_list(
        &self,
        address: &Address,
    ) -> UtxoRpcResult<(Vec<UnspentInfo>, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_all_unspent_ordered_list(self, address).await
    }

    async fn get_mature_unspent_ordered_list(
        &self,
        address: &Address,
    ) -> UtxoRpcResult<(MatureUnspentList, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_mature_unspent_ordered_list(self, address).await
    }
}

#[async_trait]
impl UtxoCommonOps for ZCoin {
    async fn get_htlc_spend_fee(&self, tx_size: u64, stage: &FeeApproxStage) -> UtxoRpcResult<u64> {
        utxo_common::get_htlc_spend_fee(self, tx_size, stage).await
    }

    fn addresses_from_script(&self, script: &Script) -> Result<Vec<Address>, String> {
        utxo_common::addresses_from_script(self, script)
    }

    fn denominate_satoshis(&self, satoshi: i64) -> f64 {
        utxo_common::denominate_satoshis(&self.utxo_arc, satoshi)
    }

    fn my_public_key(&self) -> Result<&Public, MmError<UnexpectedDerivationMethod>> {
        utxo_common::my_public_key(self.as_ref())
    }

    fn address_from_str(&self, address: &str) -> MmResult<Address, AddrFromStrError> {
        utxo_common::checked_address_from_str(self, address)
    }

    fn script_for_address(&self, address: &Address) -> MmResult<Script, UnsupportedAddr> {
        utxo_common::output_script_checked(self.as_ref(), address)
    }

    async fn get_current_mtp(&self) -> UtxoRpcResult<u32> {
        utxo_common::get_current_mtp(&self.utxo_arc, CoinVariant::Standard).await
    }

    fn is_unspent_mature(&self, output: &RpcTransaction) -> bool {
        utxo_common::is_unspent_mature(self.utxo_arc.conf.mature_confirmations, output)
    }

    async fn calc_interest_of_tx(
        &self,
        _tx: &UtxoTx,
        _input_transactions: &mut HistoryUtxoTxMap,
    ) -> UtxoRpcResult<u64> {
        MmError::err(UtxoRpcError::Internal(
            "ZCoin doesn't support transaction rewards".to_owned(),
        ))
    }

    async fn get_mut_verbose_transaction_from_map_or_rpc<'a, 'b>(
        &'a self,
        tx_hash: H256Json,
        utxo_tx_map: &'b mut HistoryUtxoTxMap,
    ) -> UtxoRpcResult<&'b mut HistoryUtxoTx> {
        utxo_common::get_mut_verbose_transaction_from_map_or_rpc(self, tx_hash, utxo_tx_map).await
    }

    async fn p2sh_spending_tx(&self, input: utxo_common::P2SHSpendingTxInput<'_>) -> Result<UtxoTx, String> {
        utxo_common::p2sh_spending_tx(self, input).await
    }

    fn get_verbose_transactions_from_cache_or_rpc(
        &self,
        tx_ids: HashSet<H256Json>,
    ) -> UtxoRpcFut<HashMap<H256Json, VerboseTransactionFrom>> {
        let selfi = self.clone();
        let fut = async move { utxo_common::get_verbose_transactions_from_cache_or_rpc(&selfi.utxo_arc, tx_ids).await };
        Box::new(fut.boxed().compat())
    }

    async fn preimage_trade_fee_required_to_send_outputs(
        &self,
        outputs: Vec<TransactionOutput>,
        fee_policy: FeePolicy,
        gas_fee: Option<u64>,
        stage: &FeeApproxStage,
    ) -> TradePreimageResult<BigDecimal> {
        utxo_common::preimage_trade_fee_required_to_send_outputs(
            self,
            self.ticker(),
            outputs,
            fee_policy,
            gas_fee,
            stage,
        )
        .await
    }

    fn increase_dynamic_fee_by_stage(&self, dynamic_fee: u64, stage: &FeeApproxStage) -> u64 {
        utxo_common::increase_dynamic_fee_by_stage(self, dynamic_fee, stage)
    }

    async fn p2sh_tx_locktime(&self, htlc_locktime: u32) -> Result<u32, MmError<UtxoRpcError>> {
        utxo_common::p2sh_tx_locktime(self, self.ticker(), htlc_locktime).await
    }

    fn addr_format(&self) -> &UtxoAddressFormat {
        utxo_common::addr_format(self)
    }

    fn addr_format_for_standard_scripts(&self) -> UtxoAddressFormat {
        utxo_common::addr_format_for_standard_scripts(self)
    }

    fn address_from_pubkey(&self, pubkey: &Public) -> Address {
        let conf = &self.utxo_arc.conf;
        utxo_common::address_from_pubkey(
            pubkey,
            conf.address_prefixes.clone(),
            conf.checksum_type,
            conf.bech32_hrp.clone(),
            self.addr_format().clone(),
        )
    }
}

#[async_trait]
impl InitWithdrawCoin for ZCoin {
    async fn init_withdraw(
        &self,
        _ctx: MmArc,
        req: WithdrawRequest,
        task_handle: WithdrawTaskHandleShared,
    ) -> Result<TransactionDetails, MmError<WithdrawError>> {
        if req.fee.is_some() {
            return MmError::err(WithdrawError::UnsupportedError(
                "Setting a custom withdraw fee is not supported for ZCoin yet".to_owned(),
            ));
        }

        if req.from.is_some() {
            return MmError::err(WithdrawError::UnsupportedError(
                "Withdraw from a specific address is not supported for ZCoin yet".to_owned(),
            ));
        }

        let to_addr = decode_payment_address(z_mainnet_constants::HRP_SAPLING_PAYMENT_ADDRESS, &req.to)
            .map_to_mm(|e| WithdrawError::InvalidAddress(format!("{e}")))?
            .or_mm_err(|| WithdrawError::InvalidAddress(format!("Address {} decoded to None", req.to)))?;
        let amount = if req.max {
            let fee = self.get_one_kbyte_tx_fee().await.map_mm_err()?;
            let balance = self.my_balance().compat().await.map_mm_err()?;
            balance.spendable - fee
        } else {
            req.amount
        };

        task_handle
            .update_in_progress_status(WithdrawInProgressStatus::GeneratingTransaction)
            .map_mm_err()?;
        let satoshi = sat_from_big_decimal(&amount, self.decimals()).map_mm_err()?;

        let memo = req.memo.as_deref().map(interpret_memo_string).transpose()?;
        let z_output = ZOutput {
            to_addr,
            amount: Amount::from_u64(satoshi)
                .map_to_mm(|_| NumConversError(format!("Failed to get ZCash amount from {amount}")))
                .map_mm_err()?,
            // TODO add optional viewing_key and memo fields to the WithdrawRequest
            viewing_key: Some(self.z_fields.evk.fvk.ovk),
            memo,
        };

        let GenTxData { tx, data, .. } = self.gen_tx(vec![], vec![z_output]).await.map_mm_err()?;
        let mut tx_bytes = Vec::with_capacity(1024);
        tx.write(&mut tx_bytes)
            .map_to_mm(|e| WithdrawError::InternalError(e.to_string()))?;
        let mut tx_hash = tx.txid().0.to_vec();
        tx_hash.reverse();

        let received_by_me = big_decimal_from_sat_unsigned(data.received_by_me, self.decimals());
        let spent_by_me = big_decimal_from_sat_unsigned(data.spent_by_me, self.decimals());
        let tx_hash_hex = hex::encode(&tx_hash);

        Ok(TransactionDetails {
            tx: TransactionData::new_signed(tx_bytes.into(), tx_hash_hex),
            from: vec![self.z_fields.my_z_addr_encoded.clone()],
            to: vec![req.to],
            my_balance_change: &received_by_me - &spent_by_me,
            total_amount: spent_by_me.clone(),
            spent_by_me,
            received_by_me,
            block_height: 0,
            timestamp: 0,
            fee_details: Some(TxFeeDetails::Utxo(UtxoFeeDetails {
                coin: Some(self.ticker().to_owned()),
                amount: big_decimal_from_sat_unsigned(data.fee_amount, self.decimals()),
            })),
            coin: self.ticker().to_owned(),
            internal_id: tx_hash.into(),
            kmd_rewards: None,
            transaction_type: Default::default(),
            memo: req.memo,
        })
    }
}

/// Waits until there are enough _unlocked_ Sapling notes to cover `total_required`.
/// TODO: Consider adding `wait_until` argument.
/// TODO: Integrate this into `light_wallet_db_sync_loop` instead of having a separate function.
/// Can be addressed when migrating to a newer librustzcash which supports spent note tracking.
/// See: https://github.com/KomodoPlatform/komodo-defi-framework/pull/2331#pullrequestreview-2883773336
async fn wait_for_spendable_balance_impl(
    selfi: ZCoin,
    total_required: BigDecimal,
) -> Result<impl Iterator<Item = SpendableNote>, MmError<GenTxError>> {
    const MAX_RETRIES: usize = 40;
    const RETRY_DELAY: f64 = 15.0;

    let mut retries = 0;

    loop {
        let wallet_notes = selfi
            .wallet_notes_ordered()
            .await
            .map_err(|e| GenTxError::SpendableNotesError(e.to_string()))?;
        let wallet_notes_len = wallet_notes.len();

        let locked_notes = selfi.z_fields.locked_notes_db.load_all_notes().await.map_mm_err()?;

        let unlocked_notes: Vec<SpendableNote> = if locked_notes.is_empty() {
            wallet_notes
        } else {
            let unconfirmed_spent_rseeds: HashSet<String> = locked_notes
                .iter()
                .filter_map(|n| {
                    if let LockedNote::Spent { rseed, .. } = n {
                        Some(rseed.clone())
                    } else {
                        None
                    }
                })
                .collect();

            wallet_notes
                .into_iter()
                .filter(|note| !unconfirmed_spent_rseeds.contains(&rseed_to_string(&note.rseed)))
                .collect()
        };
        let unlocked_notes_len = unlocked_notes.len();

        let sum_available = unlocked_notes.iter().map(|n| n.note_value).sum::<Amount>();
        let sum_available = u64::from(sum_available);
        let sum_available = big_decimal_from_sat_unsigned(sum_available, selfi.decimals());

        // Reteurn InsufficientBalance error when all notes are unlocked but amount is insufficient.
        if sum_available < total_required && unlocked_notes_len == wallet_notes_len {
            return MmError::err(GenTxError::InsufficientBalance {
                coin: selfi.ticker().to_string(),
                available: sum_available,
                required: total_required,
            });
        }

        // Returns available notes when either sufficient funds exist or all notes are unlocked.
        // Otherwise, waits for locked notes to become available up to MAX_RETRIES.
        if sum_available >= total_required || unlocked_notes_len == wallet_notes_len {
            return Ok(unlocked_notes.into_iter());
        }

        if retries >= MAX_RETRIES {
            return MmError::err(GenTxError::Internal(format!(
                "Locked notes did not become available after {MAX_RETRIES} retries"
            )));
        }

        info!(
            "Locked notes present; retrying in {}s (attempt {}/{})",
            RETRY_DELAY,
            retries + 1,
            MAX_RETRIES
        );
        common::executor::Timer::sleep(RETRY_DELAY).await;
        retries += 1;
    }
}

async fn wait_for_spendable_balance_spawner(
    selfi: &ZCoin,
    total_required: &BigDecimal,
) -> Result<impl Iterator<Item = SpendableNote>, MmError<GenTxError>> {
    let coin = selfi.clone();
    let required = total_required.clone();
    let (tx, rx) = oneshot::channel();

    selfi.spawner().spawn(async move {
        let result = wait_for_spendable_balance_impl(coin, required).await;
        let _ = tx.send(result);
    });

    match rx.await {
        Ok(res) => res,
        Err(_) => MmError::err(GenTxError::Internal(
            "wait_for_spendable_balance task was cancelled".into(),
        )),
    }
}

/// Interpret a string or hex-encoded memo, and return a Memo object.
/// Inspired by https://github.com/adityapk00/zecwallet-light-cli/blob/v1.7.20/lib/src/lightwallet/utils.rs#L23
#[allow(clippy::result_large_err)]
pub fn interpret_memo_string(memo_str: &str) -> MmResult<MemoBytes, WithdrawError> {
    // If the string starts with an "0x", and contains only hex chars ([a-f0-9]+) then
    // interpret it as a hex.
    let s_bytes = if let Some(memo_hexadecimal) = memo_str.to_lowercase().strip_prefix("0x") {
        hex::decode(memo_hexadecimal).unwrap_or_else(|_| memo_str.as_bytes().to_vec())
    } else {
        memo_str.as_bytes().to_vec()
    };

    MemoBytes::from_bytes(&s_bytes).map_to_mm(|_| {
        let error = format!("Memo '{memo_str:?}' is too long");
        WithdrawError::InvalidMemo(error)
    })
}

fn extended_spending_key_from_protocol_info_and_policy(
    protocol_info: &ZcoinProtocolInfo,
    priv_key_policy: &PrivKeyBuildPolicy,
    account: u32,
) -> MmResult<ExtendedSpendingKey, ZCoinBuildError> {
    match priv_key_policy {
        PrivKeyBuildPolicy::IguanaPrivKey(iguana) => Ok(ExtendedSpendingKey::master(iguana.as_slice())),
        PrivKeyBuildPolicy::GlobalHDAccount(global_hd) => {
            extended_spending_key_from_global_hd_account(protocol_info, global_hd, account)
        },
        PrivKeyBuildPolicy::Trezor => {
            let priv_key_err = PrivKeyPolicyNotAllowed::HardwareWalletNotSupported;
            MmError::err(ZCoinBuildError::UtxoBuilderError(
                UtxoCoinBuildError::PrivKeyPolicyNotAllowed(priv_key_err),
            ))
        },
        PrivKeyBuildPolicy::WalletConnect { .. } => {
            let priv_key_err =
                PrivKeyPolicyNotAllowed::UnsupportedMethod("WalletConnect is not supported for ZCoin".to_string());
            MmError::err(ZCoinBuildError::UtxoBuilderError(
                UtxoCoinBuildError::PrivKeyPolicyNotAllowed(priv_key_err),
            ))
        },
    }
}

fn extended_spending_key_from_global_hd_account(
    protocol_info: &ZcoinProtocolInfo,
    global_hd: &GlobalHDAccountArc,
    account: u32,
) -> MmResult<ExtendedSpendingKey, ZCoinBuildError> {
    let path_to_coin = protocol_info
        .z_derivation_path
        .clone()
        .or_mm_err(|| ZCoinBuildError::ZDerivationPathNotSet)?;
    let path_to_account = path_to_coin
        .to_derivation_path()
        .into_iter()
        // Map `bip32::ChildNumber` to `zip32::Zip32Child`.
        .map(|child| Zip32Child::from_index(child.0))
        // Push the hardened `account` index, so the derivation path looks like:
        // `m/purpose'/coin'/account'`.
        .chain(iter::once(Zip32Child::Hardened(account)));

    let mut spending_key = ExtendedSpendingKey::master(global_hd.root_seed_bytes());
    for zip32_child in path_to_account {
        spending_key = spending_key.derive_child(zip32_child);
    }

    Ok(spending_key)
}

#[inline]
fn rseed_to_string(rseed: &Rseed) -> String {
    const INPUT: [u8; 1] = [0x04];

    match rseed {
        Rseed::BeforeZip212(rcm) => rcm.to_string(),
        Rseed::AfterZip212(rseed) => jubjub::Fr::from_bytes_wide(prf_expand(rseed, &INPUT).as_array()).to_string(),
    }
}
