use crate::eth_with_token_activation::EthTaskManagerShared;
use crate::init_erc20_token_activation::Erc20TokenTaskManagerShared;
#[cfg(not(target_arch = "wasm32"))]
use crate::lightning_activation::LightningTaskManagerShared;
#[cfg(feature = "enable-sia")]
use crate::sia_coin_activation::SiaCoinTaskManagerShared;
#[cfg(feature = "enable-solana")]
use crate::solana_with_assets::SolanaCoinTaskManagerShared;
use crate::tendermint_with_assets_activation::TendermintCoinTaskManagerShared;
use crate::utxo_activation::{BchTaskManagerShared, QtumTaskManagerShared, UtxoStandardTaskManagerShared};
use crate::z_coin_activation::ZcoinTaskManagerShared;
use mm2_core::mm_ctx::{from_ctx, MmArc};
use rpc_task::RpcTaskManager;
use std::sync::Arc;

pub struct CoinsActivationContext {
    pub(crate) init_utxo_standard_task_manager: UtxoStandardTaskManagerShared,
    pub(crate) init_bch_task_manager: BchTaskManagerShared,
    pub(crate) init_qtum_task_manager: QtumTaskManagerShared,
    #[cfg(feature = "enable-sia")]
    pub(crate) init_sia_task_manager: SiaCoinTaskManagerShared,
    pub(crate) init_z_coin_task_manager: ZcoinTaskManagerShared,
    pub(crate) init_eth_task_manager: EthTaskManagerShared,
    pub(crate) init_erc20_token_task_manager: Erc20TokenTaskManagerShared,
    pub(crate) init_tendermint_coin_task_manager: TendermintCoinTaskManagerShared,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) init_lightning_task_manager: LightningTaskManagerShared,
    #[cfg(feature = "enable-solana")]
    pub(crate) init_solana_coin_task_manager: SolanaCoinTaskManagerShared,
}

impl CoinsActivationContext {
    /// Obtains a reference to this crate context, creating it if necessary.
    pub fn from_ctx(ctx: &MmArc) -> Result<Arc<CoinsActivationContext>, String> {
        from_ctx(&ctx.coins_activation_ctx, move || {
            Ok(CoinsActivationContext {
                #[cfg(feature = "enable-sia")]
                init_sia_task_manager: RpcTaskManager::new_shared(ctx.event_stream_manager.clone()),
                init_utxo_standard_task_manager: RpcTaskManager::new_shared(ctx.event_stream_manager.clone()),
                init_bch_task_manager: RpcTaskManager::new_shared(ctx.event_stream_manager.clone()),
                init_qtum_task_manager: RpcTaskManager::new_shared(ctx.event_stream_manager.clone()),
                init_z_coin_task_manager: RpcTaskManager::new_shared(ctx.event_stream_manager.clone()),
                init_eth_task_manager: RpcTaskManager::new_shared(ctx.event_stream_manager.clone()),
                init_erc20_token_task_manager: RpcTaskManager::new_shared(ctx.event_stream_manager.clone()),
                init_tendermint_coin_task_manager: RpcTaskManager::new_shared(ctx.event_stream_manager.clone()),
                #[cfg(not(target_arch = "wasm32"))]
                init_lightning_task_manager: RpcTaskManager::new_shared(ctx.event_stream_manager.clone()),
                #[cfg(feature = "enable-solana")]
                init_solana_coin_task_manager: RpcTaskManager::new_shared(ctx.event_stream_manager.clone()),
            })
        })
    }
}
