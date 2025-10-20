/// The module responsible to work with SQLite database
///
pub mod my_orders;
pub mod my_swaps;
pub mod stats_nodes;
pub mod stats_swaps;

use crate::CREATE_MY_SWAPS_TABLE;
use common::log::{debug, error, info};
use db_common::sqlite::run_optimization_pragmas;
use db_common::sqlite::rusqlite::{params_from_iter, Result as SqlResult};
use mm2_core::mm_ctx::MmArc;

use my_swaps::{fill_my_swaps_from_json_statements, set_is_finished_for_legacy_swaps_statements};
use stats_swaps::create_and_fill_stats_swaps_from_json_statements;

const SELECT_MIGRATION: &str = "SELECT * FROM migration ORDER BY current_migration DESC LIMIT 1;";

fn get_current_migration(ctx: &MmArc) -> SqlResult<i64> {
    let conn = ctx.sqlite_connection();
    conn.query_row(SELECT_MIGRATION, [], |row| row.get(0))
}

pub async fn init_and_migrate_sql_db(ctx: &MmArc) -> SqlResult<()> {
    info!("Checking the current SQLite migration");
    match get_current_migration(ctx) {
        Ok(current_migration) => {
            if current_migration >= 1 {
                info!(
                    "Current migration is {}, skipping the init, trying to migrate",
                    current_migration
                );
                migrate_sqlite_database(ctx, current_migration).await?;
                return Ok(());
            }
        },
        Err(e) => {
            debug!("Error '{}' on getting current migration. The database is either empty or corrupted, trying to clean it first", e);
            clean_db(ctx);
        },
    };

    info!("Trying to initialize the SQLite database");

    init_db(ctx)?;
    migrate_sqlite_database(ctx, 1).await?;
    info!("SQLite database initialization is successful");
    Ok(())
}

fn init_db(ctx: &MmArc) -> SqlResult<()> {
    let conn = ctx.sqlite_connection();
    run_optimization_pragmas(&conn)?;
    let init_batch = concat!(
        "BEGIN;
        CREATE TABLE IF NOT EXISTS migration (current_migration INTEGER NOT_NULL UNIQUE);
        INSERT INTO migration (current_migration) VALUES (1);",
        CREATE_MY_SWAPS_TABLE!(),
        "COMMIT;"
    );
    conn.execute_batch(init_batch)
}

fn clean_db(ctx: &MmArc) {
    let conn = ctx.sqlite_connection();
    if let Err(e) = conn.execute_batch(
        "DROP TABLE migration;
                    DROP TABLE my_swaps;",
    ) {
        error!("Error {} on SQLite database cleanup", e);
    }
}

async fn migration_1(ctx: &MmArc) -> Vec<(&'static str, Vec<String>)> {
    fill_my_swaps_from_json_statements(ctx).await
}

async fn migration_2(ctx: &MmArc) -> Vec<(&'static str, Vec<String>)> {
    create_and_fill_stats_swaps_from_json_statements(ctx).await
}

fn migration_3() -> Vec<(&'static str, Vec<String>)> {
    vec![(stats_swaps::ADD_STARTED_AT_INDEX, vec![])]
}

fn migration_4() -> Vec<(&'static str, Vec<String>)> {
    db_common::sqlite::execute_batch(stats_swaps::ADD_SPLIT_TICKERS)
}

fn migration_5() -> Vec<(&'static str, Vec<String>)> {
    vec![(my_orders::CREATE_MY_ORDERS_TABLE, vec![])]
}

fn migration_6() -> Vec<(&'static str, Vec<String>)> {
    vec![
        (stats_nodes::CREATE_NODES_TABLE, vec![]),
        (stats_nodes::CREATE_STATS_NODES_TABLE, vec![]),
    ]
}

fn migration_7() -> Vec<(&'static str, Vec<String>)> {
    db_common::sqlite::execute_batch(stats_swaps::ADD_COINS_PRICE_INFOMATION)
}

fn migration_8() -> Vec<(&'static str, Vec<String>)> {
    db_common::sqlite::execute_batch(stats_swaps::ADD_MAKER_TAKER_PUBKEYS)
}

fn migration_9() -> Vec<(&'static str, Vec<String>)> {
    db_common::sqlite::execute_batch(my_swaps::TRADING_PROTO_UPGRADE_MIGRATION)
}

async fn migration_10(ctx: &MmArc) -> Vec<(&'static str, Vec<String>)> {
    set_is_finished_for_legacy_swaps_statements(ctx).await
}

fn migration_11() -> Vec<(&'static str, Vec<String>)> {
    db_common::sqlite::execute_batch(stats_swaps::ADD_MAKER_TAKER_GUI_AND_VERSION)
}

fn migration_12() -> Vec<(&'static str, Vec<String>)> {
    vec![
        (my_swaps::ADD_OTHER_P2P_PUBKEY_FIELD, vec![]),
        (my_swaps::ADD_DEX_FEE_BURN_FIELD, vec![]),
    ]
}

fn migration_13() -> Vec<(&'static str, Vec<String>)> {
    vec![
        (my_swaps::ADD_SWAP_VERSION_FIELD, vec![]),  // Step 1: Add new column
        (my_swaps::SET_LEGACY_SWAP_VERSION, vec![]), // Step 2: Update old rows
    ]
}

async fn statements_for_migration(ctx: &MmArc, current_migration: i64) -> Option<Vec<(&'static str, Vec<String>)>> {
    match current_migration {
        1 => Some(migration_1(ctx).await),
        2 => Some(migration_2(ctx).await),
        3 => Some(migration_3()),
        4 => Some(migration_4()),
        5 => Some(migration_5()),
        6 => Some(migration_6()),
        7 => Some(migration_7()),
        8 => Some(migration_8()),
        9 => Some(migration_9()),
        10 => Some(migration_10(ctx).await),
        11 => Some(migration_11()),
        12 => Some(migration_12()),
        13 => Some(migration_13()),
        _ => None,
    }
}

pub async fn migrate_sqlite_database(ctx: &MmArc, mut current_migration: i64) -> SqlResult<()> {
    info!("migrate_sqlite_database, current migration {}", current_migration);
    while let Some(statements_with_params) = statements_for_migration(ctx, current_migration).await {
        // `statements_for_migration` locks the [`MmCtx::sqlite_connection`] mutex,
        // so we can't create a transaction outside of this loop.
        let conn = ctx.sqlite_connection();
        let transaction = conn.unchecked_transaction()?;
        for (statement, params) in statements_with_params {
            debug!("Executing SQL statement {:?} with params {:?}", statement, params);
            transaction.execute(statement, params_from_iter(params.iter()))?;
        }
        current_migration += 1;
        transaction.execute(
            "INSERT INTO migration (current_migration) VALUES (?1);",
            [current_migration],
        )?;
        transaction.commit()?;
    }
    info!("migrate_sqlite_database complete, migrated to {}", current_migration);
    Ok(())
}
