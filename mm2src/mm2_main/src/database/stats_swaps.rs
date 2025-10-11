#![allow(deprecated)] // TODO: remove this once rusqlite is >= 0.29

use crate::lp_swap::{MakerSavedSwap, SavedSwap, SavedSwapIo, TPUSwapStatusForStats, TakerSavedSwap};
use common::log::{debug, error};
use db_common::{
    owned_named_params,
    sqlite::{
        rusqlite::{params_from_iter, Connection, OptionalExtension},
        AsSqlNamedParams, OwnedSqlNamedParams,
    },
};
use mm2_core::mm_ctx::MmArc;
use std::collections::HashSet;

const CREATE_STATS_SWAPS_TABLE: &str = "CREATE TABLE IF NOT EXISTS stats_swaps (
    id INTEGER NOT NULL PRIMARY KEY,
    maker_coin VARCHAR(255) NOT NULL,
    taker_coin VARCHAR(255) NOT NULL,
    uuid VARCHAR(255) NOT NULL UNIQUE,
    started_at INTEGER NOT NULL,
    finished_at INTEGER NOT NULL,
    maker_amount DECIMAL NOT NULL,
    taker_amount DECIMAL NOT NULL,
    is_success INTEGER NOT NULL
);";

const INSERT_STATS_SWAP_ON_INIT: &str = "INSERT INTO stats_swaps (
    maker_coin,
    taker_coin,
    uuid,
    started_at,
    finished_at,
    maker_amount,
    taker_amount,
    is_success
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

const INSERT_STATS_SWAP: &str = "INSERT INTO stats_swaps (
    maker_coin,
    maker_coin_ticker,
    maker_coin_platform,
    taker_coin,
    taker_coin_ticker,
    taker_coin_platform,
    uuid,
    started_at,
    finished_at,
    maker_amount,
    taker_amount,
    is_success,
    maker_coin_usd_price,
    taker_coin_usd_price,
    maker_pubkey,
    taker_pubkey
) VALUES (:maker_coin, :maker_coin_ticker, :maker_coin_platform, :taker_coin, :taker_coin_ticker, 
:taker_coin_platform, :uuid, :started_at, :finished_at, :maker_amount, :taker_amount, :is_success, 
:maker_coin_usd_price, :taker_coin_usd_price, :maker_pubkey, :taker_pubkey)";

pub const ADD_STARTED_AT_INDEX: &str = "CREATE INDEX timestamp_index ON stats_swaps (started_at);";

pub const ADD_SPLIT_TICKERS: &[&str] = &[
    "ALTER TABLE stats_swaps ADD COLUMN maker_coin_ticker VARCHAR(255) NOT NULL DEFAULT '';",
    "ALTER TABLE stats_swaps ADD COLUMN maker_coin_platform VARCHAR(255) NOT NULL DEFAULT '';",
    "ALTER TABLE stats_swaps ADD COLUMN taker_coin_ticker VARCHAR(255) NOT NULL DEFAULT '';",
    "ALTER TABLE stats_swaps ADD COLUMN taker_coin_platform VARCHAR(255) NOT NULL DEFAULT '';",
    "UPDATE stats_swaps SET maker_coin_ticker = CASE instr(maker_coin, '-') \
        WHEN 0 THEN maker_coin \
        ELSE substr(maker_coin, 0, instr(maker_coin, '-')) \
        END;",
    "UPDATE stats_swaps SET maker_coin_platform = CASE instr(maker_coin, '-') \
        WHEN 0 THEN '' \
        ELSE substr(maker_coin, instr(maker_coin, '-') + 1) \
        END;",
    "UPDATE stats_swaps SET taker_coin_ticker = CASE instr(taker_coin, '-') \
        WHEN 0 THEN taker_coin \
        ELSE substr(taker_coin, 0, instr(taker_coin, '-')) \
        END;",
    "UPDATE stats_swaps SET taker_coin_platform = CASE instr(taker_coin, '-') \
        WHEN 0 THEN '' \
        ELSE substr(taker_coin, instr(taker_coin, '-') + 1) \
        END;",
];

pub const ADD_COINS_PRICE_INFOMATION: &[&str] = &[
    "ALTER TABLE stats_swaps ADD COLUMN maker_coin_usd_price DECIMAL;",
    "ALTER TABLE stats_swaps ADD COLUMN taker_coin_usd_price DECIMAL;",
];

pub const ADD_MAKER_TAKER_PUBKEYS: &[&str] = &[
    "ALTER TABLE stats_swaps ADD COLUMN maker_pubkey VARCHAR(255);",
    "ALTER TABLE stats_swaps ADD COLUMN taker_pubkey VARCHAR(255);",
];

pub const ADD_MAKER_TAKER_GUI_AND_VERSION: &[&str] = &[
    "ALTER TABLE stats_swaps ADD COLUMN maker_gui VARCHAR(255);",
    "ALTER TABLE stats_swaps ADD COLUMN taker_gui VARCHAR(255);",
    "ALTER TABLE stats_swaps ADD COLUMN maker_version VARCHAR(255);",
    "ALTER TABLE stats_swaps ADD COLUMN taker_version VARCHAR(255);",
];

pub const SELECT_ID_BY_UUID: &str = "SELECT id FROM stats_swaps WHERE uuid = ?1";

pub const ADD_SWAP_VERSION_AND_MARKET_MARGIN: &[&str] = &[
    "ALTER TABLE stats_swaps ADD COLUMN swap_version INTEGER NOT NULL DEFAULT 0;",
    "ALTER TABLE stats_swaps ADD COLUMN market_margin DECIMAL;",
    // NULL = unknown, 0 = not a bot, 1 = bot
    "ALTER TABLE stats_swaps ADD COLUMN is_maker_bot INTEGER;",
];

/// Returns SQL statements to initially fill stats_swaps table using existing DB with JSON files
pub async fn create_and_fill_stats_swaps_from_json_statements(ctx: &MmArc) -> Vec<(&'static str, Vec<String>)> {
    let maker_swaps = SavedSwap::load_all_from_maker_stats_db(ctx).await.unwrap_or_default();
    let taker_swaps = SavedSwap::load_all_from_taker_stats_db(ctx).await.unwrap_or_default();

    let mut result = vec![(CREATE_STATS_SWAPS_TABLE, vec![])];
    let mut inserted_maker_uuids = HashSet::with_capacity(maker_swaps.len());

    for maker_swap in maker_swaps {
        if let Some(sql_with_params) = insert_stats_maker_swap_sql_init(&maker_swap) {
            inserted_maker_uuids.insert(maker_swap.uuid);
            result.push(sql_with_params);
        }
    }
    for taker_swap in taker_swaps {
        if inserted_maker_uuids.contains(&taker_swap.uuid) {
            continue;
        }
        if let Some(sql_with_params) = insert_stats_taker_swap_sql_init(&taker_swap) {
            result.push(sql_with_params);
        }
    }
    result
}

fn split_coin(coin: &str) -> (String, String) {
    let mut split = coin.split('-');
    let ticker = split.next().expect("split returns empty string at least").into();
    let platform = split.next().map_or("".into(), |platform| platform.into());
    (ticker, platform)
}

fn insert_stats_maker_swap_sql(swap: &MakerSavedSwap) -> Option<(&'static str, OwnedSqlNamedParams)> {
    let swap_data = match swap.swap_data() {
        Ok(d) => d,
        Err(e) => {
            error!("Error {} on getting swap {} data", e, swap.uuid);
            return None;
        },
    };

    let finished_at = match swap.finished_at() {
        Ok(t) => t.to_string(),
        Err(e) => {
            error!("Error {} on getting swap {} finished_at", e, swap.uuid);
            return None;
        },
    };

    let pubkeys = match swap.swap_pubkeys() {
        Ok(p) => p,
        Err(e) => {
            error!("Error {} on getting swap {} pubkeys", e, swap.uuid);
            return None;
        },
    };

    let is_success = swap
        .is_success()
        .expect("is_success can return error only when swap is not finished");

    let (maker_coin_ticker, maker_coin_platform) = split_coin(&swap_data.maker_coin);
    let (taker_coin_ticker, taker_coin_platform) = split_coin(&swap_data.taker_coin);

    let params = owned_named_params! {
        ":maker_coin": swap_data.maker_coin.clone(),
        ":maker_coin_ticker": maker_coin_ticker,
        ":maker_coin_platform": maker_coin_platform,
        ":taker_coin": swap_data.taker_coin.clone(),
        ":taker_coin_ticker": taker_coin_ticker,
        ":taker_coin_platform": taker_coin_platform,
        ":uuid": swap.uuid.to_string(),
        ":started_at": swap_data.started_at.to_string(),
        ":finished_at": finished_at,
        ":maker_amount": swap_data.maker_amount.to_string(),
        ":taker_amount": swap_data.taker_amount.to_string(),
        ":is_success": (is_success as u32).to_string(),
        ":maker_coin_usd_price": swap.maker_coin_usd_price.as_ref().map(|p| p.to_string()),
        ":taker_coin_usd_price": swap.taker_coin_usd_price.as_ref().map(|p| p.to_string()),
        ":maker_pubkey": pubkeys.maker,
        ":taker_pubkey": pubkeys.taker,
    };

    Some((INSERT_STATS_SWAP, params))
}

fn update_stats_maker_gui_version(swap: &MakerSavedSwap) -> (&'static str, OwnedSqlNamedParams) {
    const UPDATE_STATS_SWAP_GUI_AND_VERSION_MAKER: &str =
        "UPDATE stats_swaps set maker_gui = :maker_gui, maker_version = :maker_version WHERE uuid = :uuid;";
    let params = owned_named_params! {
        ":maker_gui": swap.gui.clone(),
        ":maker_version": swap.mm_version.clone(),
        ":uuid": swap.uuid.to_string(),
    };

    (UPDATE_STATS_SWAP_GUI_AND_VERSION_MAKER, params)
}

fn insert_stats_maker_swap_sql_init(swap: &MakerSavedSwap) -> Option<(&'static str, Vec<String>)> {
    let swap_data = match swap.swap_data() {
        Ok(d) => d,
        Err(e) => {
            error!("Error {} on getting swap {} data", e, swap.uuid);
            return None;
        },
    };
    let finished_at = match swap.finished_at() {
        Ok(t) => t.to_string(),
        Err(e) => {
            error!("Error {} on getting swap {} finished_at", e, swap.uuid);
            return None;
        },
    };
    let is_success = swap
        .is_success()
        .expect("is_success can return error only when swap is not finished");

    let params = vec![
        swap_data.maker_coin.clone(),
        swap_data.taker_coin.clone(),
        swap.uuid.to_string(),
        swap_data.started_at.to_string(),
        finished_at,
        swap_data.maker_amount.to_string(),
        swap_data.taker_amount.to_string(),
        (is_success as u32).to_string(),
    ];
    Some((INSERT_STATS_SWAP_ON_INIT, params))
}

fn insert_stats_taker_swap_sql(swap: &TakerSavedSwap) -> Option<(&'static str, OwnedSqlNamedParams)> {
    let swap_data = match swap.swap_data() {
        Ok(d) => d,
        Err(e) => {
            error!("Error {} on getting swap {} data", e, swap.uuid);
            return None;
        },
    };
    let finished_at = match swap.finished_at() {
        Ok(t) => t.to_string(),
        Err(e) => {
            error!("Error {} on getting swap {} finished_at", e, swap.uuid);
            return None;
        },
    };

    let pubkeys = match swap.swap_pubkeys() {
        Ok(p) => p,
        Err(e) => {
            error!("Error {} on getting swap {} pubkeys", e, swap.uuid);
            return None;
        },
    };

    let is_success = swap
        .is_success()
        .expect("is_success can return error only when swap is not finished");

    let (maker_coin_ticker, maker_coin_platform) = split_coin(&swap_data.maker_coin);
    let (taker_coin_ticker, taker_coin_platform) = split_coin(&swap_data.taker_coin);

    let params = owned_named_params! {
        ":maker_coin": swap_data.maker_coin.clone(),
        ":maker_coin_ticker": maker_coin_ticker,
        ":maker_coin_platform": maker_coin_platform,
        ":taker_coin": swap_data.taker_coin.clone(),
        ":taker_coin_ticker": taker_coin_ticker,
        ":taker_coin_platform": taker_coin_platform,
        ":uuid": swap.uuid.to_string(),
        ":started_at": swap_data.started_at.to_string(),
        ":finished_at": finished_at,
        ":maker_amount": swap_data.maker_amount.to_string(),
        ":taker_amount": swap_data.taker_amount.to_string(),
        ":is_success": (is_success as u32).to_string(),
        ":maker_coin_usd_price": swap.maker_coin_usd_price.as_ref().map(|p| p.to_string()),
        ":taker_coin_usd_price": swap.taker_coin_usd_price.as_ref().map(|p| p.to_string()),
        ":maker_pubkey": pubkeys.maker,
        ":taker_pubkey": pubkeys.taker,
    };
    Some((INSERT_STATS_SWAP, params))
}

fn update_stats_taker_gui_version(swap: &TakerSavedSwap) -> (&'static str, OwnedSqlNamedParams) {
    const UPDATE_STATS_SWAP_GUI_AND_VERSION_TAKER: &str =
        "UPDATE stats_swaps set taker_gui = :taker_gui, taker_version = :taker_version WHERE uuid = :uuid;";
    let params = owned_named_params! {
        ":taker_gui": swap.gui.clone(),
        ":taker_version": swap.mm_version.clone(),
        ":uuid": swap.uuid.to_string(),
    };

    (UPDATE_STATS_SWAP_GUI_AND_VERSION_TAKER, params)
}

fn insert_stats_taker_swap_sql_init(swap: &TakerSavedSwap) -> Option<(&'static str, Vec<String>)> {
    let swap_data = match swap.swap_data() {
        Ok(d) => d,
        Err(e) => {
            error!("Error {} on getting swap {} data", e, swap.uuid);
            return None;
        },
    };
    let finished_at = match swap.finished_at() {
        Ok(t) => t.to_string(),
        Err(e) => {
            error!("Error {} on getting swap {} finished_at", e, swap.uuid);
            return None;
        },
    };
    let is_success = swap
        .is_success()
        .expect("is_success can return error only when swap is not finished");

    let params = vec![
        swap_data.maker_coin.clone(),
        swap_data.taker_coin.clone(),
        swap.uuid.to_string(),
        swap_data.started_at.to_string(),
        finished_at,
        swap_data.maker_amount.to_string(),
        swap_data.taker_amount.to_string(),
        (is_success as u32).to_string(),
    ];
    Some((INSERT_STATS_SWAP_ON_INIT, params))
}

fn execute_query_with_params(conn: &Connection, sql: &str, params: OwnedSqlNamedParams) {
    debug!("Executing query {} with params {:?}", sql, params);
    if let Err(e) = conn.execute_named(sql, &params.as_sql_named_params()) {
        error!("Error {} on query {} with params {:?}", e, sql, params);
    };
}

pub fn add_swap_to_index(conn: &Connection, swap: &SavedSwap) {
    let params = vec![swap.uuid().to_string()];
    let query_row = conn.query_row(SELECT_ID_BY_UUID, params_from_iter(params.iter()), |row| {
        row.get::<_, i64>(0)
    });
    match query_row.optional() {
        // swap is not indexed yet, insert it into the DB
        Ok(None) => {
            let sql_with_params = match swap {
                SavedSwap::Maker(maker) => insert_stats_maker_swap_sql(maker),
                SavedSwap::Taker(taker) => insert_stats_taker_swap_sql(taker),
            };

            let (sql, params) = match sql_with_params {
                Some(tuple) => tuple,
                None => return,
            };

            execute_query_with_params(conn, sql, params);
        },
        // swap is already indexed. Only need to update
        Ok(Some(_)) => (),
        Err(e) => {
            error!("Error {} on query {} with params {:?}", e, SELECT_ID_BY_UUID, params);
            return;
        },
    };

    let (sql, params) = match swap {
        SavedSwap::Maker(maker) => update_stats_maker_gui_version(maker),
        SavedSwap::Taker(taker) => update_stats_taker_gui_version(taker),
    };

    execute_query_with_params(conn, sql, params);
}

/// Constructs the update query for optional fields of the TPU swap status struct. That is, any field that is wrapped in an Option<T>.
fn update_optional_info(swap: &TPUSwapStatusForStats) -> (String, OwnedSqlNamedParams) {
    let mut extra_args = String::new();
    let mut params = OwnedSqlNamedParams::new();

    if let Some(ref maker_pubkey) = swap.maker_swap_pubkey {
        extra_args.push_str(", maker_pubkey = :maker_pubkey");
        params.push((":maker_pubkey", maker_pubkey.clone().into()));
    }
    if let Some(ref taker_pubkey) = swap.taker_swap_pubkey {
        extra_args.push_str(", taker_pubkey = :taker_pubkey");
        params.push((":taker_pubkey", taker_pubkey.clone().into()));
    }
    if let Some(ref maker_coin_usd_price) = swap.maker_coin_usd_price {
        extra_args.push_str(", maker_coin_usd_price = :maker_coin_usd_price");
        params.push((":maker_coin_usd_price", maker_coin_usd_price.to_string().into()));
    }
    if let Some(ref taker_coin_usd_price) = swap.taker_coin_usd_price {
        extra_args.push_str(", taker_coin_usd_price = :taker_coin_usd_price");
        params.push((":taker_coin_usd_price", taker_coin_usd_price.to_string().into()));
    }
    if let Some(ref market_margin) = swap.market_margin {
        extra_args.push_str(", market_margin = :market_margin");
        params.push((":market_margin", market_margin.to_string().into()));
    }
    if let Some(is_maker_bot) = swap.is_maker_bot {
        extra_args.push_str(", is_maker_bot = :is_maker_bot");
        params.push((":is_maker_bot", (is_maker_bot as u32).into()));
    }
    if let Some(ref maker_gui) = swap.maker_gui {
        extra_args.push_str(", maker_gui = :maker_gui");
        params.push((":maker_gui", maker_gui.clone().into()));
    }
    if let Some(ref maker_version) = swap.maker_version {
        extra_args.push_str(", maker_version = :maker_version");
        params.push((":maker_version", maker_version.clone().into()));
    }
    if let Some(ref taker_gui) = swap.taker_gui {
        extra_args.push_str(", taker_gui = :taker_gui");
        params.push((":taker_gui", taker_gui.clone().into()));
    }
    if let Some(ref taker_version) = swap.taker_version {
        extra_args.push_str(", taker_version = :taker_version");
        params.push((":taker_version", taker_version.clone().into()));
    }

    let update_query = format!(
        "UPDATE stats_swaps set swap_version = :swap_version{} WHERE uuid = :uuid;",
        extra_args
    );

    // Swap version is actually mandatory and will always be set.
    params.push((":uuid", swap.uuid.to_string().into()));
    params.push((":swap_version", (swap.swap_version as u32).into()));

    (update_query, params)
}

/// Adds a TPU swap to stats_swap table if it's not present and updates the swap with the new data if it already exists.
pub fn add_v2swap_to_index(conn: &Connection, swap: &TPUSwapStatusForStats) -> Result<(), String> {
    let params = vec![swap.uuid.to_string()];
    let query_row = conn.query_row(SELECT_ID_BY_UUID, params_from_iter(params.iter()), |row| {
        row.get::<_, i64>(0)
    });
    match query_row.optional() {
        // swap is not indexed yet, insert it into the DB
        Ok(None) => {
            let (maker_coin_ticker, maker_coin_platform) = split_coin(&swap.maker_coin);
            let (taker_coin_ticker, taker_coin_platform) = split_coin(&swap.taker_coin);

            let params = owned_named_params! {
                ":maker_coin": swap.maker_coin.clone(),
                ":maker_coin_ticker": maker_coin_ticker,
                ":maker_coin_platform": maker_coin_platform,
                ":taker_coin": swap.taker_coin.clone(),
                ":taker_coin_ticker": taker_coin_ticker,
                ":taker_coin_platform": taker_coin_platform,
                ":uuid": swap.uuid.to_string(),
                ":started_at": swap.started_at.to_string(),
                ":finished_at": swap.finished_at.to_string(),
                ":maker_amount": swap.maker_amount.to_string(),
                ":taker_amount": swap.taker_amount.to_string(),
                ":is_success": (swap.is_success() as u32).to_string(),
                // We will set those in the optional info update. For now setting them to NULL to conform with `INSERT_STATS_SWAP` statement.
                ":maker_coin_usd_price": None::<String>,
                ":taker_coin_usd_price": None::<String>,
                ":maker_pubkey": None::<String>,
                ":taker_pubkey": None::<String>,
            };

            execute_query_with_params(conn, INSERT_STATS_SWAP, params);
        },
        // swap is already indexed. Only update missing fields.
        Ok(Some(_)) => (),
        Err(e) => {
            let err_msg = format!("Error {} on query {} with params {:?}", e, SELECT_ID_BY_UUID, params);
            error!("{}", err_msg);
            return Err(err_msg);
        },
    };

    let (sql, params) = update_optional_info(swap);

    execute_query_with_params(conn, &sql, params);

    Ok(())
}

#[test]
fn test_split_coin() {
    let input = "";
    let expected = ("".into(), "".into());
    let actual = split_coin(input);
    assert_eq!(expected, actual);

    let input = "RICK";
    let expected = ("RICK".into(), "".into());
    let actual = split_coin(input);
    assert_eq!(expected, actual);

    let input = "RICK-BEP20";
    let expected = ("RICK".into(), "BEP20".into());
    let actual = split_coin(input);
    assert_eq!(expected, actual);

    let input = "RICK-";
    let expected = ("RICK".into(), "".into());
    let actual = split_coin(input);
    assert_eq!(expected, actual);
}
