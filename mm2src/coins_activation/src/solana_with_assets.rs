#![allow(unused_variables)]

use async_trait::async_trait;
use coins::{
    my_tx_history_v2::TxHistoryStorage,
    solana::{SolanaCoin, SolanaInitError, SolanaProtocolInfo},
    CoinProtocol, MmCoinEnum,
};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::MmError;
use mm2_number::BigDecimal;
use rpc_task::RpcTaskHandleShared;
use serde::{Deserialize, Serialize};

use crate::{
    context::CoinsActivationContext,
    platform_coin_with_tokens::{
        EnablePlatformCoinWithTokensError, GetPlatformBalance, InitPlatformCoinWithTokensAwaitingStatus,
        InitPlatformCoinWithTokensInProgressStatus, InitPlatformCoinWithTokensTask,
        InitPlatformCoinWithTokensTaskManagerShared, InitPlatformCoinWithTokensUserAction,
        PlatformCoinWithTokensActivationOps, TokenAsMmCoinInitializer,
    },
    prelude::{ActivationRequestInfo, CurrentBlock, TryFromCoinProtocol, TxHistory},
};

#[derive(Clone, Deserialize)]
pub struct SolanaActivationRequest {}

impl TxHistory for SolanaActivationRequest {
    fn tx_history(&self) -> bool {
        todo!()
    }
}

impl ActivationRequestInfo for SolanaActivationRequest {
    fn is_hw_policy(&self) -> bool {
        todo!()
    }
}

impl TryFromCoinProtocol for SolanaProtocolInfo {
    fn try_from_coin_protocol(proto: CoinProtocol) -> Result<Self, MmError<CoinProtocol>> {
        todo!()
    }
}

#[derive(Clone, Serialize)]
pub struct SolanaActivationResult {}

impl CurrentBlock for SolanaActivationResult {
    fn current_block(&self) -> u64 {
        todo!()
    }
}

impl GetPlatformBalance for SolanaActivationResult {
    fn get_platform_balance(&self) -> Option<BigDecimal> {
        todo!()
    }
}

impl From<SolanaInitError> for EnablePlatformCoinWithTokensError {
    fn from(err: SolanaInitError) -> Self {
        todo!()
    }
}

#[async_trait]
impl PlatformCoinWithTokensActivationOps for SolanaCoin {
    type ActivationRequest = SolanaActivationRequest;
    type PlatformProtocolInfo = SolanaProtocolInfo;
    type ActivationResult = SolanaActivationResult;
    type ActivationError = SolanaInitError;

    type InProgressStatus = InitPlatformCoinWithTokensInProgressStatus;
    type AwaitingStatus = InitPlatformCoinWithTokensAwaitingStatus;
    type UserAction = InitPlatformCoinWithTokensUserAction;

    async fn enable_platform_coin(
        ctx: MmArc,
        ticker: String,
        coin_conf: &serde_json::Value,
        activation_request: Self::ActivationRequest,
        protocol_conf: Self::PlatformProtocolInfo,
    ) -> Result<Self, MmError<Self::ActivationError>> {
        todo!()
    }

    async fn enable_global_nft(
        &self,
        _activation_request: &Self::ActivationRequest,
    ) -> Result<Option<MmCoinEnum>, MmError<Self::ActivationError>> {
        todo!()
    }

    fn try_from_mm_coin(coin: MmCoinEnum) -> Option<Self>
    where
        Self: Sized,
    {
        todo!()
    }

    fn token_initializers(
        &self,
    ) -> Vec<Box<dyn TokenAsMmCoinInitializer<PlatformCoin = Self, ActivationRequest = Self::ActivationRequest>>> {
        todo!()
    }

    async fn get_activation_result(
        &self,
        _task_handle: Option<RpcTaskHandleShared<InitPlatformCoinWithTokensTask<SolanaCoin>>>,
        activation_request: &Self::ActivationRequest,
        _nft_global: &Option<MmCoinEnum>,
    ) -> Result<Self::ActivationResult, MmError<Self::ActivationError>> {
        todo!()
    }

    fn start_history_background_fetching(
        &self,
        ctx: MmArc,
        storage: impl TxHistoryStorage,
        initial_balance: Option<BigDecimal>,
    ) {
        todo!()
    }

    fn rpc_task_manager(
        activation_ctx: &CoinsActivationContext,
    ) -> &InitPlatformCoinWithTokensTaskManagerShared<SolanaCoin> {
        todo!()
    }
}
