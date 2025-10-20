#![allow(deprecated)] // TODO: remove this once rusqlite is >= 0.29

pub use rusqlite;
pub use sql_builder;

use log::debug;
use rusqlite::types::{FromSql, Type as SqlType, Value};
use rusqlite::{Connection, Error as SqlError, Result as SqlResult, Row, ToSql};
use sql_builder::SqlBuilder;
use std::error::Error as StdError;
use std::fmt;
use std::sync::{Arc, Mutex, Weak};
use uuid::Uuid;

pub const CHECK_TABLE_EXISTS_SQL: &str = "SELECT name FROM sqlite_master WHERE type='table' AND name=?1;";

/// The macro returns `OwnedSqlNamedParams`.
#[macro_export]
macro_rules! owned_named_params {
    () => {
        Vec::new()
    };
    ($($param_name:literal: $param_val:expr),+ $(,)?) => {
        vec![$(($param_name, $crate::sqlite::rusqlite::types::Value::from($param_val))),+]
    };
}

pub type SqliteConnShared = Arc<Mutex<Connection>>;
pub type SqliteConnWeak = Weak<Mutex<Connection>>;

pub(crate) type ParamId = String;

pub(crate) type OwnedSqlParam = Value;
pub(crate) type OwnedSqlParams = Vec<OwnedSqlParam>;

type SqlNamedParam<'a> = (&'a str, &'a dyn ToSql);
pub type SqlNamedParams<'a> = Vec<SqlNamedParam<'a>>;
type OwnedSqlNamedParam = (&'static str, Value);
pub type OwnedSqlNamedParams = Vec<OwnedSqlNamedParam>;

pub trait AsSqlNamedParams {
    fn as_sql_named_params(&self) -> SqlNamedParams<'_>;
}

impl AsSqlNamedParams for OwnedSqlNamedParams {
    fn as_sql_named_params(&self) -> SqlNamedParams<'_> {
        self.iter().map(|(name, param)| (*name, param as &dyn ToSql)).collect()
    }
}

pub fn string_from_row(row: &Row<'_>) -> Result<String, SqlError> {
    row.get(0)
}

pub fn query_single_row<T, P, F>(conn: &Connection, query: &str, params: P, map_fn: F) -> Result<Option<T>, SqlError>
where
    P: rusqlite::Params,
    F: FnOnce(&Row<'_>) -> Result<T, SqlError>,
{
    let maybe_result = conn.query_row(query, params, map_fn);
    if let Err(SqlError::QueryReturnedNoRows) = maybe_result {
        return Ok(None);
    }

    let result = maybe_result?;
    Ok(Some(result))
}

pub fn query_single_row_with_named_params<T, F>(
    conn: &Connection,
    query: &str,
    params: &SqlNamedParams<'_>,
    map_fn: F,
) -> Result<Option<T>, SqlError>
where
    F: FnOnce(&Row<'_>) -> Result<T, SqlError>,
{
    let maybe_result = conn.query_row_named(query, params, map_fn);
    if let Err(SqlError::QueryReturnedNoRows) = maybe_result {
        return Ok(None);
    }

    let result = maybe_result?;
    Ok(Some(result))
}

pub fn validate_ident(ident: &str) -> SqlResult<()> {
    validate_ident_impl(ident, |c| c.is_alphanumeric() || c == '_' || c == '.')
}

/// Validates a table name against SQL injection risks.
///
/// This function checks if the provided `table_name` is safe for use in SQL queries.
/// It disallows any characters in the table name that may lead to SQL injection, only
/// allowing alphanumeric characters and underscores.
pub fn validate_table_name(table_name: &str) -> SqlResult<()> {
    let table_name = table_name.trim();

    const RESERVED_KEYWORDS: &[&str] = &[
        "SELECT",
        "INSERT",
        "UPDATE",
        "DELETE",
        "FROM",
        "WHERE",
        "JOIN",
        "INNER",
        "OUTER",
        "LEFT",
        "RIGHT",
        "ON",
        "CREATE",
        "ALTER",
        "DROP",
        "TABLE",
        "INDEX",
        "VIEW",
        "TRIGGER",
        "PROCEDURE",
        "FUNCTION",
        "DATABASE",
        "AND",
        "OR",
        "NOT",
        "NULL",
        "IS",
        "IN",
        "EXISTS",
        "BETWEEN",
        "LIKE",
        "UNION",
        "ALL",
        "ANY",
        "AS",
        "DISTINCT",
        "GROUP",
        "BY",
        "ORDER",
        "HAVING",
        "LIMIT",
        "OFFSET",
        "VALUES",
        "INTO",
        "PRIMARY",
        "FOREIGN",
        "KEY",
        "REFERENCES",
    ];

    let validation_error = || {
        SqlError::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::ApiMisuse,
                extended_code: rusqlite::ffi::SQLITE_MISUSE,
            },
            None,
        )
    };

    if table_name.is_empty() {
        log::error!("Table name can not be empty.");
        return Err(validation_error());
    }

    if RESERVED_KEYWORDS.contains(&table_name.to_uppercase().as_str()) {
        log::error!("{table_name} is a reserved SQLite keyword and can not be used as a table name.");
        return Err(validation_error());
    }

    if table_name.len() > u8::MAX as usize {
        log::error!("{table_name} length can not be greater than {}.", u8::MAX);
        return Err(validation_error());
    }

    // As per https://stackoverflow.com/a/3247553, tables can't be the target of parameter substitution.
    // So we have to use a plain concatenation disallowing any characters in the table name that may lead to SQL injection.
    validate_ident_impl(table_name, |c| c.is_alphanumeric() || c == '_')
}

/// Represents a SQL table name that has been validated for safety.
#[derive(Clone, Debug)]
pub struct SafeTableName(String);

impl SafeTableName {
    /// Creates a new SafeTableName, validating the provided table name.
    pub fn new(table_name: &str) -> SqlResult<Self> {
        validate_table_name(table_name)?;
        Ok(SafeTableName(table_name.to_owned()))
    }

    /// Retrieves the table name.
    #[inline(always)]
    pub fn inner(&self) -> &str {
        &self.0
    }
}

/// Calculates the offset to skip records by uuid.
/// Expects `query_builder` to have where clauses applied *before* calling this fn.
pub fn offset_by_uuid(
    conn: &Connection,
    query_builder: &SqlBuilder,
    params: &[(&str, String)],
    uuid: &Uuid,
) -> SqlResult<usize> {
    // building following query to determine offset by from_uuid
    // select row from (
    //     select uuid, ROW_NUMBER() OVER (ORDER BY started_at DESC) AS row
    //     from my_swaps
    //     where ... filtering options here ...
    // ) where uuid = "from_uuid";
    let subquery = query_builder
        .clone()
        .field("ROW_NUMBER() OVER (ORDER BY started_at DESC) AS row")
        .field("uuid")
        .subquery()
        .expect("SQL query builder should never fail here");

    let external_query = SqlBuilder::select_from(subquery)
        .field("row")
        .and_where("uuid = :uuid")
        .sql()
        .expect("SQL query builder should never fail here");

    let mut params_for_offset = params.to_owned();
    params_for_offset.push((":uuid", uuid.to_string()));
    let params_as_trait: Vec<_> = params_for_offset
        .iter()
        .map(|(key, value)| (*key, value as &dyn ToSql))
        .collect();

    debug!(
        "Trying to execute SQL query {} with params {:?}",
        external_query, params_for_offset
    );

    let mut stmt = conn.prepare(&external_query)?;
    let offset: isize = stmt.query_row_named(params_as_trait.as_slice(), |row| row.get(0))?;
    Ok(offset.try_into().expect("row index should be always above zero"))
}

/// A more universal offset_by_id query that will replace offset_by_uuid at some point
pub fn offset_by_id<P>(
    conn: &Connection,
    query_builder: &SqlBuilder,
    params: P,
    id_field: &str,
    order_by: &str,
    where_id: &str,
) -> SqlResult<Option<usize>>
where
    P: IntoIterator + fmt::Debug + rusqlite::Params,
    P::Item: ToSql,
{
    let row_number = format!("ROW_NUMBER() OVER (ORDER BY {order_by}) AS row");
    let subquery = query_builder
        .clone()
        .field(&row_number)
        .field(id_field)
        .subquery()
        .expect("SQL query builder should never fail here");

    let external_query = SqlBuilder::select_from(subquery)
        .field("row")
        .and_where(where_id)
        .sql()
        .expect("SQL query builder should never fail here");

    debug!(
        "Trying to execute SQL query {} with params {:?}",
        external_query, params,
    );

    let mut stmt = conn.prepare(&external_query)?;
    let maybe_offset = stmt.query_row(params, |row| row.get::<_, isize>(0));
    if let Err(SqlError::QueryReturnedNoRows) = maybe_offset {
        return Ok(None);
    }
    let offset = maybe_offset?;
    Ok(Some(offset.try_into().expect("row index should be always above zero")))
}

pub fn sql_text_conversion_err<E>(field_id: usize, e: E) -> SqlError
where
    E: std::error::Error + Send + Sync + 'static,
{
    SqlError::FromSqlConversionFailure(field_id, SqlType::Text, Box::new(e))
}

pub fn h256_slice_from_row<T>(row: &Row<'_>, column_id: usize) -> Result<[u8; 32], SqlError>
where
    T: AsRef<[u8]> + FromSql,
{
    let mut h256_slice = [0u8; 32];
    hex::decode_to_slice(row.get::<_, T>(column_id)?, &mut h256_slice as &mut [u8])
        .map_err(|e| sql_text_conversion_err(column_id, e))?;
    Ok(h256_slice)
}

pub fn h256_option_slice_from_row<T>(row: &Row<'_>, column_id: usize) -> Result<Option<[u8; 32]>, SqlError>
where
    T: AsRef<[u8]> + FromSql,
{
    let maybe_h256_slice = row.get::<_, Option<T>>(column_id)?;
    let res = match maybe_h256_slice {
        Some(s) => {
            let mut h256_slice = [0u8; 32];
            hex::decode_to_slice(s, &mut h256_slice as &mut [u8]).map_err(|e| sql_text_conversion_err(column_id, e))?;
            Some(h256_slice)
        },
        None => None,
    };
    Ok(res)
}

/// As per https://twitter.com/marcan42/status/1494213862970707969, I've noticed significant SQLite performance
/// difference on Apple Silicon Mac and Linux.
/// But according to https://phiresky.github.io/blog/2020/sqlite-performance-tuning/, these pragmas should
/// be safe to use, while giving great speed boost.
/// With these, Mac and Linux have comparable SQLite performance.
pub fn run_optimization_pragmas(conn: &Connection) -> Result<(), SqlError> {
    conn.query_row("pragma journal_mode = WAL;", [], |row| row.get::<_, String>(0))?;
    conn.execute("pragma synchronous = normal;", [])?;
    conn.execute("pragma temp_store = memory;", [])?;
    conn.execute("pragma foreign_keys = ON;", [])?;
    Ok(())
}

pub fn execute_batch(statement: &'static [&str]) -> Vec<(&'static str, Vec<String>)> {
    statement.iter().map(|sql| (*sql, vec![])).collect()
}

pub fn is_constraint_error(error: &SqlError) -> bool {
    match error {
        SqlError::SqliteFailure(failure, _error) => failure.code == rusqlite::ErrorCode::ConstraintViolation,
        _ => false,
    }
}

pub trait ToValidSqlTable {
    /// Converts `self` to a valid SQL table name or returns an error.
    fn to_valid_sql_table(&self) -> SqlResult<String>;
}

impl<S: ToString> ToValidSqlTable for S {
    fn to_valid_sql_table(&self) -> SqlResult<String> {
        let table = self.to_string();
        validate_table_name(&table)?;
        Ok(table)
    }
}

pub trait ToValidSqlIdent {
    /// Converts `self` to a valid SQL value or returns an error.
    fn to_valid_sql_ident(&self) -> SqlResult<String>;
}

impl<S: ToString> ToValidSqlIdent for S {
    fn to_valid_sql_ident(&self) -> SqlResult<String> {
        let ident = self.to_string();
        validate_ident(&ident)?;
        Ok(ident)
    }
}

/// This structure manages the SQL parameters.
#[derive(Clone, Default)]
pub struct SqlParamsBuilder {
    next_param_id: usize,
    params: OwnedSqlParams,
}

impl SqlParamsBuilder {
    /// Pushes the given `param` and returns its `:<IDX>` identifier.
    pub(crate) fn push_param<P>(&mut self, param: P) -> ParamId
    where
        OwnedSqlParam: From<P>,
    {
        self.push_owned_param(OwnedSqlParam::from(param))
    }

    /// Pushes the given `param` and returns its `:<IDX>` identifier.
    pub(crate) fn push_owned_param(&mut self, param: OwnedSqlParam) -> ParamId {
        self.params.push(param);
        self.next_param_id += 1;
        format!(":{}", self.next_param_id)
    }

    /// Pushes the given `params` and returns their `:<IDX>` identifiers.
    pub(crate) fn push_params<I, P>(&mut self, params: I) -> Vec<ParamId>
    where
        I: IntoIterator<Item = P>,
        OwnedSqlParam: From<P>,
    {
        params.into_iter().map(|param| self.push_param(param)).collect()
    }

    pub(crate) fn params(&self) -> &OwnedSqlParams {
        &self.params
    }
}

/// TODO move it to `mm2_err_handle::common_errors` when it's merged.
#[derive(Debug)]
pub struct StringError(String);

impl fmt::Display for StringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl StdError for StringError {}

impl From<&'static str> for StringError {
    fn from(s: &str) -> Self {
        StringError(s.to_owned())
    }
}

impl From<String> for StringError {
    fn from(s: String) -> Self {
        StringError(s)
    }
}

impl StringError {
    pub fn into_boxed(self) -> Box<StringError> {
        Box::new(self)
    }
}

/// Internal function to validate identifiers such as table names.
///
/// This function is a general-purpose identifier validator. It uses a closure to determine
/// the validity of each character in the provided identifier.
fn validate_ident_impl<F>(ident: &str, is_valid: F) -> SqlResult<()>
where
    F: Fn(char) -> bool,
{
    let ident = ident.trim();

    let validation_error = || {
        SqlError::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::ApiMisuse,
                extended_code: rusqlite::ffi::SQLITE_MISUSE,
            },
            None,
        )
    };

    if ident.is_empty() {
        log::error!("Ident can not be empty.");
        return Err(validation_error());
    }

    if ident.as_bytes()[0].is_ascii_digit() {
        log::error!("{ident} starts with number.");
        return Err(validation_error());
    }

    if ident.chars().all(is_valid) {
        Ok(())
    } else {
        log::error!("{ident} is not valid.");
        Err(validation_error())
    }
}
