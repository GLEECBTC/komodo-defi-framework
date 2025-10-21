use crate::hd_wallet::{
    load_hd_accounts_from_storage, HDAccountsMutex, HDWallet, HDWalletCoinStorage, HDWalletStorageError,
    DEFAULT_GAP_LIMIT,
};
use crate::utxo::rpc_clients::{
    ElectrumClient, ElectrumClientSettings, ElectrumConnectionSettings, EstimateFeeMethod, UtxoRpcClientEnum,
};
use crate::utxo::tx_cache::{UtxoVerboseCacheOps, UtxoVerboseCacheShared};
use crate::utxo::utxo_block_header_storage::BlockHeaderStorage;
use crate::utxo::utxo_builder::utxo_conf_builder::{UtxoConfBuilder, UtxoConfError, UtxoFeeConfig};
use crate::utxo::wallet_connect::{get_pubkey_via_wallatconnect_signature, get_walletconnect_address};
use crate::utxo::{
    output_script, ElectrumBuilderArgs, FeeRate, RecentlySpentOutPoints, UtxoCoinConf, UtxoCoinFields, UtxoHDWallet,
    UtxoRpcMode, UtxoSyncStatus, UtxoSyncStatusLoopHandle, UTXO_DUST_AMOUNT,
};
use crate::{
    BlockchainNetwork, CoinProtocol, CoinTransportMetrics, DerivationMethod, HistorySyncState, IguanaPrivKey,
    PrivKeyBuildPolicy, PrivKeyPolicy, PrivKeyPolicyNotAllowed, RpcClientType, SharableRpcTransportEventHandler,
    UtxoActivationParams,
};
use async_trait::async_trait;
use chain::TxHashAlgo;
use common::executor::{abortable_queue::AbortableQueue, AbortableSystem, AbortedError};
use common::now_sec;
use crypto::{
    Bip32DerPathError, CryptoCtx, CryptoCtxError, GlobalHDAccountArc, HDPathToCoin, HwWalletType, StandardHDPath,
    StandardHDPathError,
};
use derive_more::Display;
use futures::channel::mpsc::{channel, Receiver as AsyncReceiver};
use futures::compat::Future01CompatExt;
use futures::lock::Mutex as AsyncMutex;
use kdf_walletconnect::chain::WcChainId;
use kdf_walletconnect::error::WalletConnectError;
use kdf_walletconnect::{WalletConnectCtx, WcTopic};
pub use keys::{Address, AddressBuilder, AddressFormat as UtxoAddressFormat, KeyPair, Private, Public};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use secp256k1::PublicKey;
use serde_json::{self as json, Value as Json};
use spv_validation::conf::SPVConf;
use spv_validation::helpers_validation::SPVError;
use spv_validation::storage::{BlockHeaderStorageError, BlockHeaderStorageOps};
use std::convert::TryFrom;
use std::str::FromStr;
use std::sync::Mutex;

cfg_native! {
    use crate::utxo::coin_daemon_data_dir;
    use crate::utxo::rpc_clients::{ConcurrentRequestMap, NativeClient, NativeClientImpl};
    use std::env::home_dir;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
}

/// Number of seconds in a day (24 hours * 60 * 60)
pub const DAY_IN_SECONDS: u64 = 86400;

pub type UtxoCoinBuildResult<T> = Result<T, MmError<UtxoCoinBuildError>>;

#[derive(Debug, Display)]
pub enum UtxoCoinBuildError {
    ConfError(UtxoConfError),
    #[display(fmt = "Native RPC client is only supported in native mode")]
    NativeRpcNotSupportedInWasm,
    ErrorReadingNativeModeConf(String),
    #[display(fmt = "Rpc port is not set neither in `coins` file nor in native daemon config")]
    RpcPortIsNotSet,
    ErrorDetectingFeeMethod(String),
    ErrorDetectingDecimals(String),
    InvalidBlockchainNetwork(String),
    #[display(fmt = "Can not detect the user home directory")]
    CantDetectUserHome,
    #[display(fmt = "Private key policy is not allowed: {_0}")]
    PrivKeyPolicyNotAllowed(PrivKeyPolicyNotAllowed),
    #[display(fmt = "Hardware Wallet context is not initialized")]
    HwContextNotInitialized,
    HDWalletStorageError(HDWalletStorageError),
    #[display(
        fmt = "Coin doesn't support Trezor hardware wallet. Please consider adding the 'trezor_coin' field to the coins config"
    )]
    CoinDoesntSupportTrezor,
    BlockHeaderStorageError(BlockHeaderStorageError),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
    #[display(fmt = "SPV params verificaiton failed. Error: {_0}")]
    SPVError(SPVError),
    ErrorCalculatingStartingHeight(String),
    #[display(fmt = "Failed spawning balance events. Error: {_0}")]
    FailedSpawningBalanceEvents(String),
    #[display(fmt = "Can not enable balance events for {mode} mode.")]
    UnsupportedModeForBalanceEvents {
        mode: String,
    },
    InvalidPathToAddress(String),
    WalletConnectError(WalletConnectError),
}

impl From<UtxoConfError> for UtxoCoinBuildError {
    fn from(e: UtxoConfError) -> Self {
        UtxoCoinBuildError::ConfError(e)
    }
}

impl From<CryptoCtxError> for UtxoCoinBuildError {
    /// `CryptoCtx` is expected to be initialized already.
    fn from(crypto_err: CryptoCtxError) -> Self {
        UtxoCoinBuildError::Internal(crypto_err.to_string())
    }
}

impl From<Bip32DerPathError> for UtxoCoinBuildError {
    fn from(e: Bip32DerPathError) -> Self {
        UtxoCoinBuildError::Internal(StandardHDPathError::from(e).to_string())
    }
}

impl From<HDWalletStorageError> for UtxoCoinBuildError {
    fn from(e: HDWalletStorageError) -> Self {
        UtxoCoinBuildError::HDWalletStorageError(e)
    }
}

impl From<BlockHeaderStorageError> for UtxoCoinBuildError {
    fn from(e: BlockHeaderStorageError) -> Self {
        UtxoCoinBuildError::BlockHeaderStorageError(e)
    }
}

impl From<AbortedError> for UtxoCoinBuildError {
    fn from(e: AbortedError) -> Self {
        UtxoCoinBuildError::Internal(e.to_string())
    }
}

impl From<PrivKeyPolicyNotAllowed> for UtxoCoinBuildError {
    fn from(e: PrivKeyPolicyNotAllowed) -> Self {
        UtxoCoinBuildError::PrivKeyPolicyNotAllowed(e)
    }
}

impl From<keys::Error> for UtxoCoinBuildError {
    fn from(e: keys::Error) -> Self {
        UtxoCoinBuildError::Internal(e.to_string())
    }
}

impl From<WalletConnectError> for UtxoCoinBuildError {
    fn from(e: WalletConnectError) -> Self {
        UtxoCoinBuildError::WalletConnectError(e)
    }
}

#[async_trait]
pub trait UtxoCoinBuilder: UtxoCoinBuilderCommonOps {
    type ResultCoin;
    type Error: NotMmError;

    fn priv_key_policy(&self) -> PrivKeyBuildPolicy;

    async fn build(self) -> MmResult<Self::ResultCoin, Self::Error>;

    async fn build_utxo_fields(&self) -> UtxoCoinBuildResult<UtxoCoinFields> {
        match self.priv_key_policy() {
            PrivKeyBuildPolicy::IguanaPrivKey(priv_key) => build_utxo_fields_with_iguana_priv_key(self, priv_key).await,
            PrivKeyBuildPolicy::GlobalHDAccount(global_hd_ctx) => {
                build_utxo_fields_with_global_hd(self, global_hd_ctx).await
            },
            PrivKeyBuildPolicy::Trezor => build_utxo_fields_with_trezor(self).await,
            PrivKeyBuildPolicy::WalletConnect { session_topic } => {
                build_utxo_fields_with_walletconnect(self, &session_topic).await
            },
        }
    }
}

pub async fn build_utxo_fields_with_iguana_priv_key<Builder>(
    builder: &Builder,
    priv_key: IguanaPrivKey,
) -> UtxoCoinBuildResult<UtxoCoinFields>
where
    Builder: UtxoCoinBuilderCommonOps + Sync + ?Sized,
{
    let conf = UtxoConfBuilder::new(builder.conf(), builder.activation_params(), builder.ticker())
        .build()
        .map_mm_err()?;

    let private = Private {
        prefix: conf.wif_prefix,
        secret: priv_key,
        compressed: true,
        checksum_type: conf.checksum_type,
    };

    let key_pair = KeyPair::from_private(private).map_to_mm(|e| UtxoCoinBuildError::Internal(e.to_string()))?;
    let priv_key_policy = PrivKeyPolicy::Iguana(key_pair);

    let my_address = AddressBuilder::new(
        builder.address_format()?,
        conf.checksum_type,
        conf.address_prefixes.clone(),
        conf.bech32_hrp.clone(),
    )
    .as_pkh_from_pk(*key_pair.public())
    .build()
    .map_to_mm(UtxoCoinBuildError::Internal)?;

    let derivation_method = DerivationMethod::SingleAddress(my_address);
    build_utxo_coin_fields_with_conf_and_policy(builder, conf, priv_key_policy, derivation_method).await
}

pub async fn build_utxo_fields_with_global_hd<Builder>(
    builder: &Builder,
    global_hd_ctx: GlobalHDAccountArc,
) -> UtxoCoinBuildResult<UtxoCoinFields>
where
    Builder: UtxoCoinBuilderCommonOps + Sync + ?Sized,
{
    let conf = UtxoConfBuilder::new(builder.conf(), builder.activation_params(), builder.ticker())
        .build()
        .map_mm_err()?;

    let path_to_address = builder.activation_params().path_to_address;

    let path_to_coin = conf
        .derivation_path
        .as_ref()
        .ok_or(UtxoConfError::DerivationPathIsNotSet)?;

    let derivation_path = path_to_address
        .to_derivation_path(path_to_coin)
        .mm_err(|e| UtxoCoinBuildError::InvalidPathToAddress(e.to_string()))?;

    let secret = global_hd_ctx
        .derive_secp256k1_secret(&derivation_path)
        .mm_err(|e| UtxoCoinBuildError::Internal(e.to_string()))?;

    let private = Private {
        prefix: conf.wif_prefix,
        secret,
        compressed: true,
        checksum_type: conf.checksum_type,
    };

    let activated_key_pair =
        KeyPair::from_private(private).map_to_mm(|e| UtxoCoinBuildError::Internal(e.to_string()))?;
    let priv_key_policy = PrivKeyPolicy::HDWallet {
        path_to_coin: path_to_coin.clone(),
        activated_key: activated_key_pair,
        bip39_secp_priv_key: global_hd_ctx.root_priv_key().clone(),
    };

    let address_format = builder.address_format()?;
    let hd_wallet_rmd160 = *builder.ctx().rmd160();
    let hd_wallet_storage =
        HDWalletCoinStorage::init_with_rmd160(builder.ctx(), builder.ticker().to_owned(), hd_wallet_rmd160)
            .await
            .map_mm_err()?;

    let accounts = load_hd_accounts_from_storage(&hd_wallet_storage, path_to_coin)
        .await
        .mm_err(UtxoCoinBuildError::from)?;

    let gap_limit = builder.activation_params().gap_limit.unwrap_or(DEFAULT_GAP_LIMIT);

    let hd_wallet = UtxoHDWallet {
        inner: HDWallet {
            hd_wallet_rmd160,
            hd_wallet_storage,
            derivation_path: path_to_coin.clone(),
            accounts: HDAccountsMutex::new(accounts),
            enabled_address: path_to_address,
            gap_limit,
        },
        address_format,
    };

    let derivation_method = DerivationMethod::HDWallet(hd_wallet);
    build_utxo_coin_fields_with_conf_and_policy(builder, conf, priv_key_policy, derivation_method).await
}

async fn build_utxo_fields_with_walletconnect<Builder>(
    builder: &Builder,
    session_topic: &WcTopic,
) -> UtxoCoinBuildResult<UtxoCoinFields>
where
    Builder: UtxoCoinBuilderCommonOps + Sync + ?Sized,
{
    let conf = UtxoConfBuilder::new(builder.conf(), builder.activation_params(), builder.ticker())
        .build()
        .map_mm_err()?;

    let chain_id = builder.wallet_connect_chain_id()?;
    let full_derivation_path = builder.full_derivation_path()?;

    let wc_ctx = WalletConnectCtx::from_ctx(builder.ctx()).map_mm_err()?;
    let (address, pubkey) = get_walletconnect_address(&wc_ctx, session_topic, &chain_id, &full_derivation_path)
        .await
        .map_mm_err()?;

    let pubkey = match pubkey {
        Some(pubkey) => pubkey,
        // If getAccountAddresses didn't return the public key, we will try to recover it from a signature instead.
        None => {
            let sign_message_prefix = conf.sign_message_prefix.as_ref().ok_or_else(|| {
                UtxoCoinBuildError::Internal("sign_message_prefix is not set in coins config".to_string())
            })?;
            get_pubkey_via_wallatconnect_signature(&wc_ctx, session_topic, &chain_id, &address, sign_message_prefix)
                .await
                .map_mm_err()?
        },
    };

    // Construct the PrivKeyPolicy (of WalletConnect type).
    let pubkey = PublicKey::from_str(&pubkey).map_err(|e| {
        WalletConnectError::ClientError(format!("Received a bad pubkey={pubkey} from WalletConnect: {e}"))
    })?;
    let public_key = pubkey.serialize().into();
    let public_key_uncompressed = pubkey.serialize_uncompressed().into();
    let priv_key_policy = PrivKeyPolicy::WalletConnect {
        public_key,
        public_key_uncompressed,
        session_topic: session_topic.to_owned(),
    };

    // Construct the derivation method (of SingleAddress type).
    let my_address = AddressBuilder::new(
        builder.address_format()?,
        conf.checksum_type,
        conf.address_prefixes.clone(),
        conf.bech32_hrp.clone(),
    )
    .as_pkh_from_pk(Public::Compressed(pubkey.serialize().into()))
    .build()
    .map_to_mm(UtxoCoinBuildError::Internal)?;
    let derivation_method = DerivationMethod::SingleAddress(my_address.clone());

    // Validate that the address received from WalletConnect matches the one derived from the public key.
    // This is a sanity check to ensure that the WalletConnect session is valid and the address
    // corresponds to the public key we have. Otherwise, the wallet (or our address builder) is messed up.
    let my_address_serialized = my_address
        .display_address()
        .map_err(|e| UtxoCoinBuildError::Internal(format!("Failed to serialize address: {e}")))?;
    if my_address_serialized != address {
        return MmError::err(
            WalletConnectError::ClientError(format!(
            "Received address={my_address_serialized} from WalletConnect doesn't match the expected address={address} derived via the public key"
        ))
            .into(),
        );
    }

    build_utxo_coin_fields_with_conf_and_policy(builder, conf, priv_key_policy, derivation_method).await
}

async fn build_utxo_coin_fields_with_conf_and_policy<Builder>(
    builder: &Builder,
    conf: UtxoCoinConf,
    priv_key_policy: PrivKeyPolicy<KeyPair>,
    derivation_method: DerivationMethod<Address, UtxoHDWallet>,
) -> UtxoCoinBuildResult<UtxoCoinFields>
where
    Builder: UtxoCoinBuilderCommonOps + Sync + ?Sized,
{
    let pubkey = {
        match priv_key_policy {
            PrivKeyPolicy::WalletConnect { public_key, .. } => Public::Compressed(public_key.0.into()),
            _ => *priv_key_policy.activated_key_or_err().map_mm_err()?.public(),
        }
    };
    let addr_format = builder.address_format()?;
    let my_address = AddressBuilder::new(
        addr_format,
        conf.checksum_type,
        conf.address_prefixes.clone(),
        conf.bech32_hrp.clone(),
    )
    .as_pkh_from_pk(pubkey)
    .build()
    .map_to_mm(UtxoCoinBuildError::Internal)?;

    let my_script_pubkey = output_script(&my_address).map(|script| script.to_bytes())?;

    // Create an abortable system linked to the `MmCtx` so if the context is stopped via `MmArc::stop`,
    // all spawned futures related to this `UTXO` coin will be aborted as well.
    let abortable_system: AbortableQueue = builder.ctx().abortable_system.create_subsystem()?;

    let rpc_client = builder.rpc_client(abortable_system.create_subsystem()?).await?;
    let tx_fee = builder.tx_fee(&rpc_client).await?;
    let decimals = builder.decimals(&rpc_client).await?;
    let dust_amount = builder.dust_amount();

    let initial_history_state = builder.initial_history_state();
    let tx_hash_algo = builder.tx_hash_algo();
    let check_utxo_maturity = builder.check_utxo_maturity();
    let tx_cache = builder.tx_cache();
    let (block_headers_status_notifier, block_headers_status_watcher) =
        builder.block_header_status_channel(&conf.spv_conf);

    let coin = UtxoCoinFields {
        conf,
        decimals,
        dust_amount,
        rpc_client,
        priv_key_policy,
        derivation_method,
        history_sync_state: Mutex::new(initial_history_state),
        tx_cache,
        recently_spent_outpoints: AsyncMutex::new(RecentlySpentOutPoints::new(my_script_pubkey)),
        tx_fee,
        tx_hash_algo,
        check_utxo_maturity,
        block_headers_status_notifier,
        block_headers_status_watcher,
        ctx: builder.ctx().clone().weak(),
        abortable_system,
    };

    Ok(coin)
}

async fn build_utxo_fields_with_trezor<Builder>(builder: &Builder) -> UtxoCoinBuildResult<UtxoCoinFields>
where
    Builder: UtxoCoinBuilderCommonOps + Sync + ?Sized,
{
    let ticker = builder.ticker().to_owned();
    let conf = UtxoConfBuilder::new(builder.conf(), builder.activation_params(), &ticker)
        .build()
        .map_mm_err()?;

    // Make sure this coin supports Trezor.
    if conf.trezor_coin.is_none() {
        return MmError::err(UtxoCoinBuildError::CoinDoesntSupportTrezor);
    }

    let hd_wallet_rmd160 = {
        let crypto_ctx = CryptoCtx::from_ctx(builder.ctx()).map_mm_err()?;
        let hw_ctx = crypto_ctx
            .hw_ctx()
            .or_mm_err(|| UtxoCoinBuildError::HwContextNotInitialized)?;
        match hw_ctx.hw_wallet_type() {
            HwWalletType::Trezor => hw_ctx.rmd160(),
        }
    };

    let address_format = builder.address_format()?;
    let path_to_coin = conf
        .derivation_path
        .clone()
        .or_mm_err(|| UtxoConfError::DerivationPathIsNotSet)
        .map_mm_err()?;

    let hd_wallet_storage = HDWalletCoinStorage::init(builder.ctx(), ticker).await.map_mm_err()?;

    let accounts = load_hd_accounts_from_storage(&hd_wallet_storage, &path_to_coin)
        .await
        .mm_err(UtxoCoinBuildError::from)?;
    let gap_limit = builder.activation_params().gap_limit.unwrap_or(DEFAULT_GAP_LIMIT);
    let hd_wallet = UtxoHDWallet {
        inner: HDWallet {
            hd_wallet_rmd160,
            hd_wallet_storage,
            derivation_path: path_to_coin,
            accounts: HDAccountsMutex::new(accounts),
            enabled_address: builder.activation_params().path_to_address,
            gap_limit,
        },
        address_format,
    };

    let recently_spent_outpoints = AsyncMutex::new(RecentlySpentOutPoints::new(Default::default()));

    // Create an abortable system linked to the `MmCtx` so if the context is stopped via `MmArc::stop`,
    // all spawned futures related to this `UTXO` coin will be aborted as well.
    let abortable_system: AbortableQueue = builder.ctx().abortable_system.create_subsystem()?;

    let rpc_client = builder.rpc_client(abortable_system.create_subsystem()?).await?;
    let tx_fee = builder.tx_fee(&rpc_client).await?;
    let decimals = builder.decimals(&rpc_client).await?;
    let dust_amount = builder.dust_amount();

    let initial_history_state = builder.initial_history_state();
    let tx_hash_algo = builder.tx_hash_algo();
    let check_utxo_maturity = builder.check_utxo_maturity();
    let tx_cache = builder.tx_cache();
    let (block_headers_status_notifier, block_headers_status_watcher) =
        builder.block_header_status_channel(&conf.spv_conf);

    let coin = UtxoCoinFields {
        conf,
        decimals,
        dust_amount,
        rpc_client,
        priv_key_policy: PrivKeyPolicy::Trezor,
        derivation_method: DerivationMethod::HDWallet(hd_wallet),
        history_sync_state: Mutex::new(initial_history_state),
        tx_cache,
        recently_spent_outpoints,
        tx_fee,
        tx_hash_algo,
        check_utxo_maturity,
        block_headers_status_notifier,
        block_headers_status_watcher,
        ctx: builder.ctx().clone().weak(),
        abortable_system,
    };
    Ok(coin)
}

#[async_trait]
pub trait UtxoCoinBuilderCommonOps {
    fn ctx(&self) -> &MmArc;

    fn conf(&self) -> &Json;

    fn activation_params(&self) -> &UtxoActivationParams;

    fn ticker(&self) -> &str;

    /// This function basically defines 'my address' format (so whether a coin is segwit or not)
    /// For that it looks for the "address_format" property first in the activation request then in the coins file.
    /// This fn is called to set the address format in the derivaion_method in UtxoCoinFields, which creates my address.
    fn address_format(&self) -> UtxoCoinBuildResult<UtxoAddressFormat> {
        let format_from_req = self.activation_params().address_format.clone();
        let format_from_conf = json::from_value::<Option<UtxoAddressFormat>>(self.conf()["address_format"].clone())
            .map_to_mm(|e| UtxoConfError::InvalidAddressFormat(e.to_string()))
            .map_mm_err()?
            .unwrap_or(UtxoAddressFormat::Standard);

        let mut address_format = match format_from_req {
            Some(from_req) => {
                if from_req.is_segwit() != format_from_conf.is_segwit() {
                    let error = format!(
                        "Both conf {format_from_conf:?} and request {from_req:?} must be either Segwit or Standard/CashAddress"
                    );
                    return MmError::err(UtxoCoinBuildError::from(UtxoConfError::InvalidAddressFormat(error)));
                } else {
                    from_req
                }
            },
            None => format_from_conf,
        };

        if let UtxoAddressFormat::CashAddress {
            network: _,
            ref mut pub_addr_prefix,
            ref mut p2sh_addr_prefix,
        } = address_format
        {
            *pub_addr_prefix = self.pub_addr_prefix();
            *p2sh_addr_prefix = self.p2sh_address_prefix();
        }

        let is_segwit_in_conf = self.conf()["segwit"].as_bool().unwrap_or(false);
        if address_format.is_segwit() && (!is_segwit_in_conf || self.conf()["bech32_hrp"].is_null()) {
            let error =
                "Cannot use Segwit address format for coin without segwit support or bech32_hrp in config".to_owned();
            return MmError::err(UtxoCoinBuildError::from(UtxoConfError::InvalidAddressFormat(error)));
        }
        Ok(address_format)
    }

    fn pub_addr_prefix(&self) -> u8 {
        let pubtype = self.conf()["pubtype"]
            .as_u64()
            .unwrap_or(if self.ticker() == "BTC" { 0 } else { 60 });
        pubtype as u8
    }

    fn p2sh_address_prefix(&self) -> u8 {
        self.conf()["p2shtype"]
            .as_u64()
            .unwrap_or(if self.ticker() == "BTC" { 5 } else { 85 }) as u8
    }

    fn dust_amount(&self) -> u64 {
        json::from_value(self.conf()["dust"].clone()).unwrap_or(UTXO_DUST_AMOUNT)
    }

    fn network(&self) -> UtxoCoinBuildResult<BlockchainNetwork> {
        let conf = self.conf();
        if !conf["network"].is_null() {
            return json::from_value(conf["network"].clone())
                .map_to_mm(|e| UtxoCoinBuildError::InvalidBlockchainNetwork(e.to_string()));
        }
        Ok(BlockchainNetwork::Mainnet)
    }

    /// Returns WcChainId for this coin. Parsed from the coin config.
    fn wallet_connect_chain_id(&self) -> UtxoCoinBuildResult<WcChainId> {
        let protocol: CoinProtocol = json::from_value(self.conf()["protocol"].clone()).map_to_mm(|e| {
            UtxoCoinBuildError::ConfError(UtxoConfError::InvalidProtocolData(format!(
                "Couldn't parse protocol info: {e}"
            )))
        })?;

        if let CoinProtocol::UTXO(utxo_info) = protocol {
            let utxo_info = utxo_info.ok_or_else(|| {
                WalletConnectError::InvalidChainId(format!(
                    "coin={} doesn't have chain_id (bip122 standard) set in coin config which is required for WalletConnect",
                    self.ticker()
                ))
            })?;
            let chain_id = WcChainId::try_from_str(&utxo_info.chain_id).map_mm_err()?;
            Ok(chain_id)
        } else {
            MmError::err(UtxoCoinBuildError::ConfError(UtxoConfError::InvalidProtocolData(
                format!("Expected UTXO protocol, got: {protocol:?}"),
            )))
        }
    }

    /// Constructs the full HD derivation path from the coin config and the activation params partial paths.
    fn full_derivation_path(&self) -> UtxoCoinBuildResult<StandardHDPath> {
        let path_purpose_to_coin = self.conf()["derivation_path"].as_str().ok_or_else(|| {
            UtxoCoinBuildError::InvalidPathToAddress("derivation_path is not set in coin config".to_owned())
        })?;
        let path_purpose_to_coin = HDPathToCoin::from_str(path_purpose_to_coin).map_err(|e| {
            UtxoCoinBuildError::InvalidPathToAddress(format!("Failed to parse derivation_path in coins config: {e:?}"))
        })?;
        let path_account_to_address = self.activation_params().path_to_address;
        let full_derivation_path = path_account_to_address
            .to_derivation_path(&path_purpose_to_coin)
            .map_err(|e| {
                UtxoCoinBuildError::InvalidPathToAddress(format!("Failed to construct full derivation path: {e}"))
            })?;
        let full_derivation_path = StandardHDPath::try_from(full_derivation_path).map_err(|e| {
            UtxoCoinBuildError::InvalidPathToAddress(format!("Failed to parse full derivation path: {e:?}"))
        })?;
        Ok(full_derivation_path)
    }

    async fn decimals(&self, _rpc_client: &UtxoRpcClientEnum) -> UtxoCoinBuildResult<u8> {
        Ok(self.conf()["decimals"].as_u64().unwrap_or(8) as u8)
    }

    async fn tx_fee(&self, rpc_client: &UtxoRpcClientEnum) -> UtxoCoinBuildResult<FeeRate> {
        let tx_fee = match UtxoFeeConfig::parse_val(self.conf()) {
            UtxoFeeConfig::NotSet => FeeRate::FixedPerKb(1000),
            UtxoFeeConfig::Dynamic => {
                let fee_method = match &rpc_client {
                    UtxoRpcClientEnum::Electrum(_) => EstimateFeeMethod::Standard,
                    UtxoRpcClientEnum::Native(client) => client
                        .detect_fee_method()
                        .compat()
                        .await
                        .map_to_mm(UtxoCoinBuildError::ErrorDetectingFeeMethod)?,
                };
                FeeRate::Dynamic(fee_method)
            },
            UtxoFeeConfig::FixedPerKb(fee) => FeeRate::FixedPerKb(fee),
            UtxoFeeConfig::FixedPerKbDingo(fee) => FeeRate::FixedPerKbDingo(fee),
        };
        Ok(tx_fee)
    }

    fn initial_history_state(&self) -> HistorySyncState {
        if self.activation_params().tx_history {
            HistorySyncState::NotStarted
        } else {
            HistorySyncState::NotEnabled
        }
    }

    async fn rpc_client(&self, abortable_system: AbortableQueue) -> UtxoCoinBuildResult<UtxoRpcClientEnum> {
        match self.activation_params().mode.clone() {
            UtxoRpcMode::Native => {
                #[cfg(target_arch = "wasm32")]
                {
                    MmError::err(UtxoCoinBuildError::NativeRpcNotSupportedInWasm)
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let native = self.native_client()?;
                    Ok(UtxoRpcClientEnum::Native(native))
                }
            },
            UtxoRpcMode::Electrum {
                servers,
                min_connected,
                max_connected,
            } => {
                let electrum = self
                    .electrum_client(
                        abortable_system,
                        ElectrumBuilderArgs::default(),
                        servers,
                        (min_connected, max_connected),
                    )
                    .await?;
                Ok(UtxoRpcClientEnum::Electrum(electrum))
            },
        }
    }

    /// The method takes `abortable_system` that will be used to spawn Electrum's related futures.
    /// It can be pinned to the coin's abortable system via [`AbortableSystem::create_subsystem`], but not required.
    async fn electrum_client(
        &self,
        abortable_system: AbortableQueue,
        args: ElectrumBuilderArgs,
        servers: Vec<ElectrumConnectionSettings>,
        (min_connected, max_connected): (Option<usize>, Option<usize>),
    ) -> UtxoCoinBuildResult<ElectrumClient> {
        let coin_ticker = self.ticker().to_owned();
        let ctx = self.ctx();
        let mut event_handlers: Vec<Box<SharableRpcTransportEventHandler>> = vec![];
        if args.collect_metrics {
            event_handlers.push(Box::new(CoinTransportMetrics::new(
                ctx.metrics.weak(),
                coin_ticker.clone(),
                RpcClientType::Electrum,
            )));
        }

        let storage_ticker = self.ticker().replace('-', "_");
        let block_headers_storage = BlockHeaderStorage::new_from_ctx(self.ctx().clone(), storage_ticker)
            .map_to_mm(|e| UtxoCoinBuildError::Internal(e.to_string()))?;
        if !block_headers_storage.is_initialized_for().await? {
            block_headers_storage.init().await?;
        }

        let gui = ctx.gui().unwrap_or("UNKNOWN").to_string();
        let mm_version = ctx.mm_version().to_string();
        let (min_connected, max_connected) = (min_connected.unwrap_or(1), max_connected.unwrap_or(servers.len()));
        let client_settings = ElectrumClientSettings {
            client_name: format!("{gui} GUI/MM2 {mm_version}"),
            servers: servers.clone(),
            coin_ticker,
            spawn_ping: args.spawn_ping,
            negotiate_version: args.negotiate_version,
            min_connected,
            max_connected,
        };

        ElectrumClient::try_new(
            client_settings,
            event_handlers,
            block_headers_storage,
            ctx.event_stream_manager.clone(),
            abortable_system,
        )
        .map_to_mm(UtxoCoinBuildError::Internal)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn native_client(&self) -> UtxoCoinBuildResult<NativeClient> {
        use base64::engine::general_purpose::URL_SAFE;
        use base64::Engine;

        let native_conf_path = self.confpath()?;
        let network = self.network()?;
        let (rpc_port, rpc_user, rpc_password) = read_native_mode_conf(&native_conf_path, &network)
            .map_to_mm(UtxoCoinBuildError::ErrorReadingNativeModeConf)?;
        let auth_str = format!("{rpc_user}:{rpc_password}");
        let rpc_port = match rpc_port {
            Some(p) => p,
            None => self.conf()["rpcport"]
                .as_u64()
                .or_mm_err(|| UtxoCoinBuildError::RpcPortIsNotSet)? as u16,
        };

        let ctx = self.ctx();
        let coin_ticker = self.ticker().to_owned();
        let event_handlers =
            vec![
                CoinTransportMetrics::new(ctx.metrics.weak(), coin_ticker.clone(), RpcClientType::Native).into_shared(),
            ];
        let client = Arc::new(NativeClientImpl {
            coin_ticker,
            uri: format!("http://127.0.0.1:{rpc_port}"),
            auth: format!("Basic {}", URL_SAFE.encode(auth_str)),
            event_handlers,
            request_id: 0u64.into(),
            list_unspent_concurrent_map: ConcurrentRequestMap::new(),
        });

        Ok(NativeClient(client))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn confpath(&self) -> UtxoCoinBuildResult<PathBuf> {
        let conf = self.conf();
        // Documented at https://github.com/jl777/coins#bitcoin-protocol-specific-json
        // "USERHOME/" prefix should be replaced with the user's home folder.
        let declared_confpath = match self.conf()["confpath"].as_str() {
            Some(path) if !path.is_empty() => path.trim(),
            _ => {
                let (name, is_asset_chain) = {
                    match conf["asset"].as_str() {
                        Some(a) => (a, true),
                        None => {
                            let name = conf["name"]
                                .as_str()
                                .or_mm_err(|| UtxoConfError::CurrencyNameIsNotSet)
                                .map_mm_err()?;
                            (name, false)
                        },
                    }
                };
                let data_dir = coin_daemon_data_dir(name, is_asset_chain);
                let confname = format!("{name}.conf");

                return Ok(data_dir.join(&confname[..]));
            },
        };

        let (confpath, rel_to_home) = match declared_confpath.strip_prefix("~/") {
            Some(stripped) => (stripped, true),
            None => match declared_confpath.strip_prefix("USERHOME/") {
                Some(stripped) => (stripped, true),
                None => (declared_confpath, false),
            },
        };

        if rel_to_home {
            let home = home_dir().or_mm_err(|| UtxoCoinBuildError::CantDetectUserHome)?;
            Ok(home.join(confpath))
        } else {
            Ok(confpath.into())
        }
    }

    fn tx_hash_algo(&self) -> TxHashAlgo {
        if self.ticker() == "GRS" {
            TxHashAlgo::SHA256
        } else {
            TxHashAlgo::DSHA256
        }
    }

    fn check_utxo_maturity(&self) -> bool {
        // First, check if the flag is set in the activation params.
        if let Some(check_utxo_maturity) = self.activation_params().check_utxo_maturity {
            return check_utxo_maturity;
        }
        self.conf()["check_utxo_maturity"].as_bool().unwrap_or_default()
    }

    #[cfg(target_arch = "wasm32")]
    fn tx_cache(&self) -> UtxoVerboseCacheShared {
        #[allow(clippy::default_constructed_unit_structs)] // This is a false-possitive bug from clippy
        crate::utxo::tx_cache::wasm_tx_cache::WasmVerboseCache::default().into_shared()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn tx_cache(&self) -> UtxoVerboseCacheShared {
        crate::utxo::tx_cache::fs_tx_cache::FsVerboseCache::new(self.ticker().to_owned(), self.tx_cache_path())
            .into_shared()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn tx_cache_path(&self) -> PathBuf {
        self.ctx().global_dir().join("TX_CACHE")
    }

    fn block_header_status_channel(
        &self,
        spv_conf: &Option<SPVConf>,
    ) -> (
        Option<UtxoSyncStatusLoopHandle>,
        Option<AsyncMutex<AsyncReceiver<UtxoSyncStatus>>>,
    ) {
        if spv_conf.is_some() && !self.activation_params().mode.is_native() {
            let (sync_status_notifier, sync_watcher) = channel(1);
            return (
                Some(UtxoSyncStatusLoopHandle::new(sync_status_notifier)),
                Some(AsyncMutex::new(sync_watcher)),
            );
        };

        (None, None)
    }

    /// Calculates the starting block height based on a given date and the current block height.
    ///
    /// # Arguments
    /// * `date`: The date in seconds representing the desired starting date.
    /// * `current_block_height`: The current block height at the time of calculation.
    ///
    fn calculate_starting_height_from_date(
        &self,
        date_s: u64,
        current_block_height: u64,
    ) -> UtxoCoinBuildResult<Option<u64>> {
        let avg_blocktime = self.conf()["avg_blocktime"]
            .as_u64()
            .ok_or_else(|| format!("avg_blocktime not specified in {} coin config", self.ticker()))
            .map_to_mm(UtxoCoinBuildError::ErrorCalculatingStartingHeight)?;
        let blocks_per_day = DAY_IN_SECONDS / avg_blocktime;
        let current_time_sec = now_sec();

        if current_time_sec < date_s {
            return MmError::err(UtxoCoinBuildError::ErrorCalculatingStartingHeight(format!(
                "{} sync date must be earlier then current date",
                self.ticker()
            )));
        };

        let secs_since_date = current_time_sec - date_s;
        let days_since_date = (secs_since_date / DAY_IN_SECONDS).max(1) - 1;
        let blocks_to_sync = (days_since_date * blocks_per_day) + blocks_per_day;

        if current_block_height < blocks_to_sync {
            return Ok(None);
        }

        let block_to_sync_from = current_block_height - blocks_to_sync;

        Ok(Some(block_to_sync_from))
    }
}

/// Attempts to parse native daemon conf file and return rpcport, rpcuser and rpcpassword
#[cfg(not(target_arch = "wasm32"))]
fn read_native_mode_conf(
    filename: &dyn AsRef<Path>,
    network: &BlockchainNetwork,
) -> Result<(Option<u16>, String, String), String> {
    use ini::Ini;

    fn read_property<'a>(conf: &'a ini::Ini, network: &BlockchainNetwork, property: &str) -> Option<&'a String> {
        let subsection = match network {
            BlockchainNetwork::Mainnet => None,
            BlockchainNetwork::Testnet => conf.section(Some("test")),
            BlockchainNetwork::Regtest => conf.section(Some("regtest")),
        };
        subsection
            .and_then(|props| props.get(property))
            .or_else(|| conf.general_section().get(property))
    }

    let conf: Ini = match Ini::load_from_file(filename) {
        Ok(ini) => ini,
        Err(err) => {
            return ERR!(
                "Error parsing the native wallet configuration '{}': {}",
                filename.as_ref().display(),
                err
            )
        },
    };
    let rpc_port = match read_property(&conf, network, "rpcport") {
        Some(port) => port.parse::<u16>().ok(),
        None => None,
    };
    let rpc_user = try_s!(read_property(&conf, network, "rpcuser").ok_or(ERRL!(
        "Conf file {} doesn't have the rpcuser key",
        filename.as_ref().display()
    )));
    let rpc_password = try_s!(read_property(&conf, network, "rpcpassword").ok_or(ERRL!(
        "Conf file {} doesn't have the rpcpassword key",
        filename.as_ref().display()
    )));
    Ok((rpc_port, rpc_user.clone(), rpc_password.clone()))
}
