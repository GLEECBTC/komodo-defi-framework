//! This module contains code to store swaps with LR data in the my_swaps table in MM2 SQLite DB.

#![allow(deprecated)] // TODO: remove this once rusqlite is >= 0.29

use db_common::sqlite::rusqlite::{Result as SqlResult, ToSql};
use mm2_core::mm_ctx::MmArc;

/// Adds new fields required for aggregated swaps with liquidity routing
pub const LR_SWAP_MIGRATION: &[&str] = &[
    "ALTER TABLE my_swaps ADD COLUMN lr_swap_0 TEXT;",
    "ALTER TABLE my_swaps ADD COLUMN sell_buy_req TEXT;",
    "ALTER TABLE my_swaps ADD COLUMN lr_swap_1 TEXT;",
];

const INSERT_LR_SWAP: &str = r#"INSERT INTO my_swaps (
    my_coin,
    other_coin,
    uuid,
    started_at,
    swap_type,
    maker_volume,
    taker_volume,
    swap_version,
    lr_swap_0,
    sell_buy_req,
    lr_swap_1
) VALUES (
    :my_coin,
    :other_coin,
    :uuid,
    :started_at,
    :swap_type,
    :maker_volume,
    :taker_volume,
    :swap_version,
    :lr_swap_0,
    :sell_buy_req,
    :lr_swap_1
);"#;

pub(crate) fn insert_new_lr_swap(ctx: &MmArc, params: &[(&str, &dyn ToSql)]) -> SqlResult<()> {
    let conn = ctx.sqlite_connection();
    conn.execute(INSERT_LR_SWAP, params).map(|_| ())
}

/// The SQL query selecting swap with LR required to re-initialize the swap e.g., on restart.
/// NOTE: the 'taker_volume' db field is used as source_volume and the 'maker_volume' db field is used as destination_volume
pub(crate) const SELECT_LR_SWAP_BY_UUID: &str = r#"SELECT
    my_coin,
    other_coin,
    uuid,
    started_at,
    is_finished,
    events_json,
    swap_version,
    taker_volume,
    maker_volume,
    lr_swap_0,
    sell_buy_req,
    lr_swap_1
FROM my_swaps
WHERE uuid = :uuid;
"#;
