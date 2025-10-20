#![allow(deprecated)] // TODO: remove this once rusqlite is >= 0.29

/// This module contains code to work with my_swaps table in MM2 SQLite DB
use crate::lp_swap::{MyRecentSwapsUuids, MySwapsFilter, SavedSwap, SavedSwapIo};
use common::log::debug;
use common::PagingOptions;
use db_common::sqlite::rusqlite::{Connection, Error as SqlError, Result as SqlResult, ToSql};
use db_common::sqlite::sql_builder::SqlBuilder;
use db_common::sqlite::{offset_by_uuid, query_single_row};
use mm2_core::mm_ctx::MmArc;
use std::convert::TryInto;
use uuid::{Error as UuidError, Uuid};

const MY_SWAPS_TABLE: &str = "my_swaps";

// Using a macro because static variable can't be passed to concat!
// https://stackoverflow.com/a/39024422
#[macro_export]
macro_rules! CREATE_MY_SWAPS_TABLE {
    () => {
        "CREATE TABLE IF NOT EXISTS my_swaps (
            id INTEGER NOT NULL PRIMARY KEY,
            my_coin VARCHAR(255) NOT NULL,
            other_coin VARCHAR(255) NOT NULL,
            uuid VARCHAR(255) NOT NULL UNIQUE,
            started_at INTEGER NOT NULL
        );"
    };
}

/// Adds new fields required for trading protocol upgrade implementation (swap v2)
pub const TRADING_PROTO_UPGRADE_MIGRATION: &[&str] = &[
    "ALTER TABLE my_swaps ADD COLUMN is_finished BOOLEAN NOT NULL DEFAULT 0;",
    "ALTER TABLE my_swaps ADD COLUMN events_json TEXT NOT NULL DEFAULT '[]';",
    "ALTER TABLE my_swaps ADD COLUMN swap_type INTEGER NOT NULL DEFAULT 0;",
    // Storing rational numbers as text to maintain precision
    "ALTER TABLE my_swaps ADD COLUMN maker_volume TEXT;",
    // Storing rational numbers as text to maintain precision
    "ALTER TABLE my_swaps ADD COLUMN taker_volume TEXT;",
    // Storing rational numbers as text to maintain precision
    "ALTER TABLE my_swaps ADD COLUMN premium TEXT;",
    // Storing rational numbers as text to maintain precision
    "ALTER TABLE my_swaps ADD COLUMN dex_fee TEXT;",
    "ALTER TABLE my_swaps ADD COLUMN secret BLOB;",
    "ALTER TABLE my_swaps ADD COLUMN secret_hash BLOB;",
    "ALTER TABLE my_swaps ADD COLUMN secret_hash_algo INTEGER;",
    "ALTER TABLE my_swaps ADD COLUMN p2p_privkey BLOB;",
    "ALTER TABLE my_swaps ADD COLUMN lock_duration INTEGER;",
    "ALTER TABLE my_swaps ADD COLUMN maker_coin_confs INTEGER;",
    "ALTER TABLE my_swaps ADD COLUMN maker_coin_nota BOOLEAN;",
    "ALTER TABLE my_swaps ADD COLUMN taker_coin_confs INTEGER;",
    "ALTER TABLE my_swaps ADD COLUMN taker_coin_nota BOOLEAN;",
];

/// Adds Swap Protocol version column to `my_swaps` table
pub const ADD_SWAP_VERSION_FIELD: &str = "ALTER TABLE my_swaps ADD COLUMN swap_version INTEGER;";
/// Sets default value for `swap_version` to `1` for existing rows
pub const SET_LEGACY_SWAP_VERSION: &str = "UPDATE my_swaps SET swap_version = 1 WHERE swap_version IS NULL;";
pub const ADD_OTHER_P2P_PUBKEY_FIELD: &str = "ALTER TABLE my_swaps ADD COLUMN other_p2p_pub BLOB;";
/// Storing rational numbers as text to maintain precision
pub const ADD_DEX_FEE_BURN_FIELD: &str = "ALTER TABLE my_swaps ADD COLUMN dex_fee_burn TEXT;";

/// The query to insert swap on migration 1, during this migration swap_type column doesn't exist
/// in my_swaps table yet.
const INSERT_MY_SWAP_MIGRATION_1: &str =
    "INSERT INTO my_swaps (my_coin, other_coin, uuid, started_at) VALUES (?1, ?2, ?3, ?4)";
const INSERT_MY_SWAP: &str =
    "INSERT INTO my_swaps (my_coin, other_coin, uuid, started_at, swap_type) VALUES (?1, ?2, ?3, ?4, ?5)";

pub fn insert_new_swap(
    ctx: &MmArc,
    my_coin: &str,
    other_coin: &str,
    uuid: &str,
    started_at: &str,
    swap_type: u8,
) -> SqlResult<()> {
    debug!("Inserting new swap {} to the SQLite database", uuid);
    let conn = ctx.sqlite_connection();
    let params = [my_coin, other_coin, uuid, started_at, &swap_type.to_string()];
    conn.execute(INSERT_MY_SWAP, params).map(|_| ())
}

const INSERT_MY_SWAP_V2: &str = r#"INSERT INTO my_swaps (
    my_coin,
    other_coin,
    uuid,
    started_at,
    swap_type,
    maker_volume,
    taker_volume,
    premium,
    dex_fee,
    dex_fee_burn,
    secret,
    secret_hash,
    secret_hash_algo,
    p2p_privkey,
    lock_duration,
    maker_coin_confs,
    maker_coin_nota,
    taker_coin_confs,
    taker_coin_nota,
    other_p2p_pub,
    swap_version
) VALUES (
    :my_coin,
    :other_coin,
    :uuid,
    :started_at,
    :swap_type,
    :maker_volume,
    :taker_volume,
    :premium,
    :dex_fee,
    :dex_fee_burn,
    :secret,
    :secret_hash,
    :secret_hash_algo,
    :p2p_privkey,
    :lock_duration,
    :maker_coin_confs,
    :maker_coin_nota,
    :taker_coin_confs,
    :taker_coin_nota,
    :other_p2p_pub,
    :swap_version
);"#;

pub fn insert_new_swap_v2(ctx: &MmArc, params: &[(&str, &dyn ToSql)]) -> SqlResult<()> {
    let conn = ctx.sqlite_connection();
    conn.execute(INSERT_MY_SWAP_V2, params).map(|_| ())
}

/// Returns SQL statements to initially fill my_swaps table using existing DB with JSON files
/// Use this only in migration code!
pub async fn fill_my_swaps_from_json_statements(ctx: &MmArc) -> Vec<(&'static str, Vec<String>)> {
    let swaps = SavedSwap::load_all_my_swaps_from_db(ctx).await.unwrap_or_default();
    swaps
        .into_iter()
        .filter_map(insert_saved_swap_sql_migration_1)
        .collect()
}

/// Use this only in migration code!
fn insert_saved_swap_sql_migration_1(swap: SavedSwap) -> Option<(&'static str, Vec<String>)> {
    let swap_info = swap.get_my_info()?;
    let params = vec![
        swap_info.my_coin,
        swap_info.other_coin,
        swap.uuid().to_string(),
        swap_info.started_at.to_string(),
    ];
    Some((INSERT_MY_SWAP_MIGRATION_1, params))
}

#[derive(Debug)]
pub enum SelectSwapsUuidsErr {
    Sql(SqlError),
    Parse(UuidError),
}

impl std::fmt::Display for SelectSwapsUuidsErr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl From<SqlError> for SelectSwapsUuidsErr {
    fn from(err: SqlError) -> Self {
        SelectSwapsUuidsErr::Sql(err)
    }
}

impl From<UuidError> for SelectSwapsUuidsErr {
    fn from(err: UuidError) -> Self {
        SelectSwapsUuidsErr::Parse(err)
    }
}

/// Adds where clauses determined by MySwapsFilter
fn apply_my_swaps_filter(builder: &mut SqlBuilder, params: &mut Vec<(&str, String)>, filter: &MySwapsFilter) {
    if let Some(my_coin) = &filter.my_coin {
        builder.and_where("my_coin = :my_coin");
        params.push((":my_coin", my_coin.clone()));
    }

    if let Some(other_coin) = &filter.other_coin {
        builder.and_where("other_coin = :other_coin");
        params.push((":other_coin", other_coin.clone()));
    }

    if let Some(from_timestamp) = &filter.from_timestamp {
        builder.and_where("started_at >= :from_timestamp");
        params.push((":from_timestamp", from_timestamp.to_string()));
    }

    if let Some(to_timestamp) = &filter.to_timestamp {
        builder.and_where("started_at < :to_timestamp");
        params.push((":to_timestamp", to_timestamp.to_string()));
    }
}

pub fn select_uuids_by_my_swaps_filter(
    conn: &Connection,
    filter: &MySwapsFilter,
    paging_options: Option<&PagingOptions>,
) -> SqlResult<MyRecentSwapsUuids, SelectSwapsUuidsErr> {
    let mut query_builder = SqlBuilder::select_from(MY_SWAPS_TABLE);
    let mut params = vec![];
    apply_my_swaps_filter(&mut query_builder, &mut params, filter);

    // count total records matching the filter
    let mut count_builder = query_builder.clone();
    count_builder.count("id");

    let count_query = count_builder.sql().expect("SQL query builder should never fail here");
    debug!("Trying to execute SQL query {} with params {:?}", count_query, params);

    let params_as_trait: Vec<_> = params.iter().map(|(key, value)| (*key, value as &dyn ToSql)).collect();
    let total_count: isize = conn.query_row_named(&count_query, params_as_trait.as_slice(), |row| row.get(0))?;
    let total_count = total_count.try_into().expect("COUNT should always be >= 0");
    if total_count == 0 {
        return Ok(MyRecentSwapsUuids::default());
    }

    // query the uuids and types finally
    query_builder.field("uuid");
    query_builder.field("swap_type");
    query_builder.order_desc("started_at");

    let skipped = match paging_options {
        Some(paging) => {
            // calculate offset, page_number is ignored if from_uuid is set
            let offset = match paging.from_uuid {
                Some(uuid) => offset_by_uuid(conn, &query_builder, &params, &uuid)?,
                None => (paging.page_number.get() - 1) * paging.limit,
            };
            query_builder.limit(paging.limit);
            query_builder.offset(offset);
            offset
        },
        None => 0,
    };

    let uuids_query = query_builder.sql().expect("SQL query builder should never fail here");
    debug!("Trying to execute SQL query {} with params {:?}", uuids_query, params);
    let mut stmt = conn.prepare(&uuids_query)?;
    let uuids_and_types = stmt
        .query_map_named(params_as_trait.as_slice(), |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<SqlResult<Vec<(String, u8)>>>()?;
    let uuids_and_types: SqlResult<Vec<(Uuid, u8)>, UuidError> = uuids_and_types
        .into_iter()
        .map(|(uuid, swap_type)| Ok((uuid.parse()?, swap_type)))
        .collect();
    let uuids_and_types = uuids_and_types?;

    Ok(MyRecentSwapsUuids {
        uuids_and_types,
        total_count,
        skipped,
    })
}

/// Returns whether a swap with specified uuid exists in DB
pub fn does_swap_exist(conn: &Connection, uuid: &str) -> SqlResult<bool> {
    const SELECT_SWAP_ID_BY_UUID: &str = "SELECT id FROM my_swaps WHERE uuid = :uuid;";
    let res: Option<i64> = query_single_row(conn, SELECT_SWAP_ID_BY_UUID, &[(":uuid", uuid)], |row| row.get(0))?;
    Ok(res.is_some())
}

/// Queries swap events by uuid
pub fn get_swap_events(conn: &Connection, uuid: &str) -> SqlResult<String> {
    const SELECT_SWAP_EVENTS_BY_UUID: &str = "SELECT events_json FROM my_swaps WHERE uuid = :uuid;";
    let mut stmt = conn.prepare(SELECT_SWAP_EVENTS_BY_UUID)?;
    let swap_type = stmt.query_row(&[(":uuid", uuid)], |row| row.get(0))?;
    Ok(swap_type)
}

/// Updates swap events by uuid
pub fn update_swap_events(conn: &Connection, uuid: &str, events_json: &str) -> SqlResult<()> {
    const UPDATE_SWAP_EVENTS_BY_UUID: &str = "UPDATE my_swaps SET events_json = :events_json WHERE uuid = :uuid;";
    let mut stmt = conn.prepare(UPDATE_SWAP_EVENTS_BY_UUID)?;
    stmt.execute(&[(":uuid", uuid), (":events_json", events_json)])
        .map(|_| ())
}

const UPDATE_SWAP_IS_FINISHED_BY_UUID: &str = "UPDATE my_swaps SET is_finished = 1 WHERE uuid = :uuid;";
pub fn set_swap_is_finished(conn: &Connection, uuid: &str) -> SqlResult<()> {
    let mut stmt = conn.prepare(UPDATE_SWAP_IS_FINISHED_BY_UUID)?;
    stmt.execute(&[(":uuid", uuid)]).map(|_| ())
}

pub fn select_unfinished_swaps_uuids(conn: &Connection, swap_type: u8) -> SqlResult<Vec<Uuid>, SelectSwapsUuidsErr> {
    const SELECT_UNFINISHED_SWAPS_UUIDS_BY_TYPE: &str =
        "SELECT uuid FROM my_swaps WHERE is_finished = 0 AND swap_type = :type;";
    let mut stmt = conn.prepare(SELECT_UNFINISHED_SWAPS_UUIDS_BY_TYPE)?;
    let uuids = stmt
        .query_map_named(&[(":type", &swap_type)], |row| row.get(0))?
        .collect::<SqlResult<Vec<String>>>()?;
    let uuids: SqlResult<Vec<_>, _> = uuids.into_iter().map(|uuid| uuid.parse()).collect();
    Ok(uuids?)
}

/// The SQL query selecting upgraded swap data and send it to user through RPC API
/// It omits sensitive data (swap secret, p2p privkey, etc) for security reasons
/// TODO: should we add burn amount for rpc?
pub const SELECT_MY_SWAP_V2_FOR_RPC_BY_UUID: &str = r#"SELECT
    my_coin,
    other_coin,
    uuid,
    started_at,
    is_finished,
    events_json,
    maker_volume,
    taker_volume,
    premium,
    dex_fee,
    lock_duration,
    maker_coin_confs,
    maker_coin_nota,
    taker_coin_confs,
    taker_coin_nota,
    swap_version
FROM my_swaps
WHERE uuid = :uuid;
"#;

/// The SQL query selecting upgraded swap data required to re-initialize the swap e.g., on restart.
/// NOTE: for maker v2 swap the dex_fee is stored as default (the real one could be no fee if taker is the dex pubkey)
pub const SELECT_MY_SWAP_V2_BY_UUID: &str = r#"SELECT
    my_coin,
    other_coin,
    uuid,
    started_at,
    secret,
    secret_hash,
    secret_hash_algo,
    events_json,
    maker_volume,
    taker_volume,
    premium,
    dex_fee,
    dex_fee_burn,
    lock_duration,
    maker_coin_confs,
    maker_coin_nota,
    taker_coin_confs,
    taker_coin_nota,
    p2p_privkey,
    other_p2p_pub,
    swap_version
FROM my_swaps
WHERE uuid = :uuid;
"#;

/// Returns SQL statements to set is_finished to 1 for completed legacy swaps
pub async fn set_is_finished_for_legacy_swaps_statements(ctx: &MmArc) -> Vec<(&'static str, Vec<String>)> {
    let swaps = SavedSwap::load_all_my_swaps_from_db(ctx).await.unwrap_or_default();
    swaps
        .into_iter()
        .filter_map(|swap| {
            if swap.is_finished() {
                Some((UPDATE_SWAP_IS_FINISHED_BY_UUID, vec![swap.uuid().to_string()]))
            } else {
                None
            }
        })
        .collect()
}
