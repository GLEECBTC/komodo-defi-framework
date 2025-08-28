mod utxo_arc_builder;
mod utxo_coin_builder;
mod utxo_conf_builder;

pub use utxo_arc_builder::{merge_utxos, MergeConditions, MergeUtxoArcOps, UtxoArcBuilder, UtxoMergeError};
pub use utxo_coin_builder::{
    build_utxo_fields_with_global_hd, build_utxo_fields_with_iguana_priv_key, UtxoCoinBuildError, UtxoCoinBuildResult,
    UtxoCoinBuilder, UtxoCoinBuilderCommonOps, DAY_IN_SECONDS,
};
pub use utxo_conf_builder::{UtxoConfBuilder, UtxoConfError, UtxoConfResult};

#[cfg(test)]
pub(crate) use utxo_arc_builder::{block_header_utxo_loop, BlockHeaderUtxoLoopExtraArgs};
