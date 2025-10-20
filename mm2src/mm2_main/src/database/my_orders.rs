#![allow(deprecated)] // TODO: remove this once rusqlite is >= 0.29

use crate::lp_ordermatch::{FilteringOrder, MakerOrder, MyOrdersFilter, RecentOrdersSelectResult, TakerOrder};
/// This module contains code to work with my_orders table in MM2 SQLite DB
use common::log::debug;
use common::{now_ms, PagingOptions};
use db_common::sqlite::offset_by_uuid;
use db_common::sqlite::rusqlite::{params_from_iter, Connection, Error as SqlError, Result as SqlResult, ToSql};
use db_common::sqlite::sql_builder::SqlBuilder;
use mm2_core::mm_ctx::MmArc;
use mm2_rpc::data::legacy::TakerAction;
use std::convert::TryInto;
use std::string::ParseError;
use uuid::Uuid;

const MY_ORDERS_TABLE: &str = "my_orders";

pub const CREATE_MY_ORDERS_TABLE: &str = "CREATE TABLE IF NOT EXISTS my_orders (
    id INTEGER NOT NULL PRIMARY KEY,
    uuid VARCHAR(255) NOT NULL UNIQUE,
    type VARCHAR(255) NOT NULL,
    initial_action VARCHAR(255) NOT NULL,
    base VARCHAR(255) NOT NULL,
    rel VARCHAR(255) NOT NULL,
    price DECIMAL NOT NULL,
    volume DECIMAL NOT NULL,
    created_at INTEGER NOT NULL,    
    last_updated INTEGER NOT NULL,  
    was_taker INTEGER NOT NULL,
    status VARCHAR(255) NOT NULL
);";

const INSERT_MY_ORDER: &str = "INSERT INTO my_orders (uuid, type, initial_action, base, rel, price, volume, created_at, last_updated, was_taker, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";

const UPDATE_MY_ORDER: &str =
    "UPDATE my_orders SET price = ?2, volume = ?3, last_updated = ?4, status = ?5 WHERE uuid = ?1";

const UPDATE_WAS_TAKER: &str = "UPDATE my_orders SET type = ?2, last_updated = ?3, was_taker = ?4 WHERE uuid = ?1";

const UPDATE_ORDER_STATUS: &str = "UPDATE my_orders SET last_updated = ?2, status = ?3 WHERE uuid = ?1";

const SELECT_STATUS_BY_UUID: &str = "SELECT status FROM my_orders WHERE uuid = ?1";

pub fn insert_maker_order(ctx: &MmArc, uuid: Uuid, order: &MakerOrder) -> SqlResult<()> {
    debug!("Inserting new order {} to the SQLite database", uuid);
    let params = vec![
        uuid.to_string(),
        "Maker".to_string(),
        "Sell".to_string(),
        order.base.clone(),
        order.rel.clone(),
        order.price.to_decimal().to_string(),
        order.max_base_vol.to_decimal().to_string(),
        order.created_at.to_string(),
        order.updated_at.unwrap_or(0).to_string(),
        0.to_string(),
        "Created".to_string(),
    ];
    let conn = ctx.sqlite_connection();
    conn.execute(INSERT_MY_ORDER, params_from_iter(params.iter()))
        .map(|_| ())
}

pub fn insert_taker_order(ctx: &MmArc, uuid: Uuid, order: &TakerOrder) -> SqlResult<()> {
    debug!("Inserting new order {} to the SQLite database", uuid);
    let price = order.request.rel_amount.to_decimal() / order.request.base_amount.to_decimal();
    let initial_action = match order.request.action {
        TakerAction::Buy => "Buy".to_string(),
        TakerAction::Sell => "Sell".to_string(),
    };
    let params = vec![
        uuid.to_string(),
        "Taker".to_string(),
        initial_action,
        order.request.base.clone(),
        order.request.rel.clone(),
        price.to_string(),
        order.request.base_amount.to_decimal().to_string(),
        order.created_at.to_string(),
        order.created_at.to_string(),
        0.to_string(),
        "Created".to_string(),
    ];
    let conn = ctx.sqlite_connection();
    conn.execute(INSERT_MY_ORDER, params_from_iter(params.iter()))
        .map(|_| ())
}

pub fn update_maker_order(ctx: &MmArc, uuid: Uuid, order: &MakerOrder) -> SqlResult<()> {
    debug!("Updating order {} in the SQLite database", uuid);
    let params = [
        uuid.to_string(),
        order.price.to_decimal().to_string(),
        order.max_base_vol.to_decimal().to_string(),
        order.updated_at.unwrap_or(0).to_string(),
        "Updated".to_string(),
    ];
    let conn = ctx.sqlite_connection();
    conn.execute(UPDATE_MY_ORDER, params_from_iter(params.iter()))
        .map(|_| ())
}

pub fn update_was_taker(ctx: &MmArc, uuid: Uuid) -> SqlResult<()> {
    debug!("Updating order {} in the SQLite database", uuid);
    let params = [
        uuid.to_string(),
        "Maker".to_string(),
        now_ms().to_string(),
        1.to_string(),
    ];
    let conn = ctx.sqlite_connection();
    conn.execute(UPDATE_WAS_TAKER, params_from_iter(params.iter()))
        .map(|_| ())
}

pub fn update_order_status(ctx: &MmArc, uuid: Uuid, status: String) -> SqlResult<()> {
    debug!("Updating order {} in the SQLite database", uuid);
    let params = [uuid.to_string(), now_ms().to_string(), status];
    let conn = ctx.sqlite_connection();
    conn.execute(UPDATE_ORDER_STATUS, params_from_iter(params.iter()))
        .map(|_| ())
}

/// Adds where clauses determined by MyOrdersFilter
fn apply_my_orders_filter(builder: &mut SqlBuilder, params: &mut Vec<(&str, String)>, filter: &MyOrdersFilter) {
    if let Some(order_type) = &filter.order_type {
        builder.and_where("type = :order_type");
        params.push((":order_type", order_type.clone()));
    }

    if let Some(initial_action) = &filter.initial_action {
        builder.and_where("initial_action = :initial_action");
        params.push((":initial_action", initial_action.clone()));
    }

    if let Some(base) = &filter.base {
        builder.and_where("base = :base");
        params.push((":base", base.clone()));
    }

    if let Some(rel) = &filter.rel {
        builder.and_where("rel = :rel");
        params.push((":rel", rel.clone()));
    }

    if let Some(from_price) = &filter.from_price {
        builder.and_where("price >= :from_price");
        params.push((":from_price", from_price.to_string()));
    }

    if let Some(to_price) = &filter.to_price {
        builder.and_where("price <= :to_price");
        params.push((":to_price", to_price.to_string()));
    }

    if let Some(from_volume) = &filter.from_volume {
        builder.and_where("volume >= :from_volume");
        params.push((":from_volume", from_volume.to_string()));
    }

    if let Some(to_volume) = &filter.to_volume {
        builder.and_where("volume <= :to_volume");
        params.push((":to_volume", to_volume.to_string()));
    }

    if let Some(from_timestamp) = &filter.from_timestamp {
        builder.and_where("created_at >= :from_timestamp");
        params.push((":from_timestamp", from_timestamp.to_string()));
    }

    if let Some(to_timestamp) = &filter.to_timestamp {
        builder.and_where("created_at <= :to_timestamp");
        params.push((":to_timestamp", to_timestamp.to_string()));
    }

    if let Some(was_taker) = filter.was_taker {
        let was_taker = was_taker as u32;
        builder.and_where("was_taker = :was_taker");
        params.push((":was_taker", was_taker.to_string()));
    }
    if let Some(status) = &filter.status {
        builder.and_where("status = :status");
        params.push((":status", status.clone()));
    }
}

#[derive(Debug)]
pub enum SelectRecentOrdersUuidsErr {
    Sql(SqlError),
    Parse(ParseError),
}

impl std::fmt::Display for SelectRecentOrdersUuidsErr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl From<SqlError> for SelectRecentOrdersUuidsErr {
    fn from(err: SqlError) -> Self {
        SelectRecentOrdersUuidsErr::Sql(err)
    }
}

impl From<ParseError> for SelectRecentOrdersUuidsErr {
    fn from(err: ParseError) -> Self {
        SelectRecentOrdersUuidsErr::Parse(err)
    }
}

pub fn select_orders_by_filter(
    conn: &Connection,
    filter: &MyOrdersFilter,
    paging_options: Option<&PagingOptions>,
) -> SqlResult<RecentOrdersSelectResult, SelectRecentOrdersUuidsErr> {
    let mut query_builder = SqlBuilder::select_from(MY_ORDERS_TABLE);
    let mut params = vec![];
    apply_my_orders_filter(&mut query_builder, &mut params, filter);

    // count total records matching the filter
    let mut count_builder = query_builder.clone();
    count_builder.count("id");

    let count_query = count_builder.sql().expect("SQL query builder should never fail here");
    debug!("Trying to execute SQL query {} with params {:?}", count_query, params);

    let params_as_trait: Vec<_> = params.iter().map(|(key, value)| (*key, value as &dyn ToSql)).collect();
    let total_count: isize = conn.query_row_named(&count_query, params_as_trait.as_slice(), |row| row.get(0))?;
    let total_count = total_count.try_into().expect("COUNT should always be >= 0");
    if total_count == 0 {
        return Ok(RecentOrdersSelectResult::default());
    }

    // query the orders finally
    query_builder
        .field("uuid")
        .field("type")
        .field("initial_action")
        .field("base")
        .field("rel")
        .field("price")
        .field("volume")
        .field("created_at")
        .field("last_updated")
        .field("was_taker")
        .field("status");
    query_builder.order_desc("created_at");

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
    let orders = stmt
        .query_map_named(params_as_trait.as_slice(), |row| {
            Ok(FilteringOrder {
                uuid: row.get(0)?,
                order_type: row.get(1)?,
                initial_action: row.get(2)?,
                base: row.get(3)?,
                rel: row.get(4)?,
                price: row.get(5)?,
                volume: row.get(6)?,
                created_at: row.get(7)?,
                last_updated: row.get(8)?,
                was_taker: row.get(9)?,
                status: row.get(10)?,
            })
        })?
        .collect::<SqlResult<Vec<FilteringOrder>>>()?;

    Ok(RecentOrdersSelectResult {
        orders,
        total_count,
        skipped,
    })
}

pub fn select_status_by_uuid(conn: &Connection, uuid: &Uuid) -> Result<String, SqlError> {
    let params = [uuid.to_string()];
    conn.query_row(SELECT_STATUS_BY_UUID, params_from_iter(params.iter()), |row| {
        row.get::<_, String>(0)
    })
}
