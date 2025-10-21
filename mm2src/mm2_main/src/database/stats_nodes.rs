/// This module contains code to work with nodes table for stats collection in MM2 SQLite DB
use crate::lp_stats::{NodeInfo, NodeVersionStat};
use common::log::debug;
use db_common::sqlite::rusqlite::{params_from_iter, Error as SqlError, Result as SqlResult};
use mm2_core::mm_ctx::MmArc;
use std::collections::hash_map::HashMap;

pub const CREATE_NODES_TABLE: &str = "CREATE TABLE IF NOT EXISTS nodes (
    id INTEGER NOT NULL PRIMARY KEY,
    name VARCHAR(255) NOT NULL UNIQUE,
    address VARCHAR(255) NOT NULL,
    peer_id VARCHAR(255) NOT NULL UNIQUE
);";

pub const CREATE_STATS_NODES_TABLE: &str = "CREATE TABLE IF NOT EXISTS stats_nodes (
    id INTEGER NOT NULL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    version VARCHAR(255),
    timestamp INTEGER NOT NULL,
    error VARCHAR(255)
);";

const INSERT_NODE: &str = "INSERT INTO nodes (name, address, peer_id) VALUES (?1, ?2, ?3)";

const DELETE_NODE: &str = "DELETE FROM nodes WHERE name = ?1";

const SELECT_PEERS_ADDRESSES: &str = "SELECT peer_id, address FROM nodes";

const SELECT_PEERS_NAMES: &str = "SELECT peer_id, name FROM nodes";

const INSERT_STAT: &str = "INSERT INTO stats_nodes (name, version, timestamp, error) VALUES (?1, ?2, ?3, ?4)";

pub fn insert_node_info(ctx: &MmArc, node_info: &NodeInfo) -> SqlResult<()> {
    debug!("Inserting info about node {} to the SQLite database", node_info.name);
    let params = [
        node_info.name.clone(),
        node_info.address.clone(),
        node_info.peer_id.clone(),
    ];
    #[cfg(not(feature = "new-db-arch"))]
    let conn = ctx.sqlite_connection();
    #[cfg(feature = "new-db-arch")]
    let conn = ctx.global_db();
    conn.execute(INSERT_NODE, params_from_iter(params.iter())).map(|_| ())
}

pub fn delete_node_info(ctx: &MmArc, name: String) -> SqlResult<()> {
    debug!("Deleting info about node {} from the SQLite database", name);
    let params = [name];
    #[cfg(not(feature = "new-db-arch"))]
    let conn = ctx.sqlite_connection();
    #[cfg(feature = "new-db-arch")]
    let conn = ctx.global_db();
    conn.execute(DELETE_NODE, params_from_iter(params.iter())).map(|_| ())
}

pub fn select_peers_addresses(ctx: &MmArc) -> SqlResult<Vec<(String, String)>, SqlError> {
    #[cfg(not(feature = "new-db-arch"))]
    let conn = ctx.sqlite_connection();
    #[cfg(feature = "new-db-arch")]
    let conn = ctx.global_db();
    let mut stmt = conn.prepare(SELECT_PEERS_ADDRESSES)?;
    let peers_addresses = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<SqlResult<Vec<(String, String)>>>()?;

    Ok(peers_addresses)
}

pub fn select_peers_names(ctx: &MmArc) -> SqlResult<HashMap<String, String>, SqlError> {
    #[cfg(not(feature = "new-db-arch"))]
    let conn = ctx.sqlite_connection();
    #[cfg(feature = "new-db-arch")]
    let conn = ctx.global_db();
    // TODO: Can't use `conn` in the return statement because it's a mutex borrow, and also clippy complains when assigning the result into a temporary `result`.
    #[allow(clippy::let_and_return)]
    let result = conn
        .prepare(SELECT_PEERS_NAMES)?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<SqlResult<HashMap<String, String>>>();
    result
}

pub fn insert_node_version_stat(ctx: &MmArc, node_version_stat: NodeVersionStat) -> SqlResult<()> {
    debug!(
        "Inserting new version stat for node {} to the SQLite database",
        node_version_stat.name
    );
    let params = [
        node_version_stat.name,
        node_version_stat.version.unwrap_or_default(),
        node_version_stat.timestamp.to_string(),
        node_version_stat.error.unwrap_or_default(),
    ];
    #[cfg(not(feature = "new-db-arch"))]
    let conn = ctx.sqlite_connection();
    #[cfg(feature = "new-db-arch")]
    let conn = ctx.global_db();
    conn.execute(INSERT_STAT, params_from_iter(params.iter())).map(|_| ())
}
