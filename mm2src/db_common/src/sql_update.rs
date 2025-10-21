use crate::sql_condition::SqlCondition;
use crate::sql_value::{FromQuoted, SqlValueOptional, SqlValueToString};
use crate::sqlite::{validate_table_name, OwnedSqlParam, OwnedSqlParams, SqlParamsBuilder, ToValidSqlIdent};
use common::log::debug;
use rusqlite::{params_from_iter, Connection, Error as SqlError, Result as SqlResult};
use sql_builder::SqlBuilder;

/// An `UPDATE` SQL request builder.
pub struct SqlUpdate<'a> {
    conn: &'a Connection,
    sql_builder: SqlBuilder,
    params: SqlParamsBuilder,
}

impl<'a> SqlUpdate<'a> {
    /// Create `UPDATE` request builder.
    /// Please note the function validates the given `table` name.
    #[inline]
    pub fn new(conn: &'a Connection, table: &str) -> SqlResult<SqlUpdate<'a>> {
        validate_table_name(table)?;
        Ok(SqlUpdate {
            conn,
            sql_builder: SqlBuilder::update_table(table),
            params: SqlParamsBuilder::default(),
        })
    }

    /// Adds the `value` of the specified `column` to the `UPDATE` request.
    /// For more details see [`SqlBuilder::set`].
    ///
    /// Please note the function validates the given `column` name,
    /// and `value` is considered an valid optional value as it's able to be converted into `SqlValueOptional`.
    #[inline]
    pub fn set<S, V>(&mut self, column: S, value: V) -> SqlResult<&mut Self>
    where
        S: ToValidSqlIdent,
        SqlValueOptional: From<V>,
    {
        let value = SqlValueOptional::from(value).to_string();
        self.sql_builder.set(column.to_valid_sql_ident()?, value);
        Ok(self)
    }

    /// Adds the quoted `value` of the specified `column` to the `UPDATE` request.
    /// For more details see [`SqlBuilder::set`].
    ///
    /// Please note the function validates the given `column` name,
    /// and `value` is considered a valid optional value as it's able to be converted into `SqlValueOptional`.
    #[inline]
    pub fn set_quoted<S, V>(&mut self, column: S, value: V) -> SqlResult<&mut Self>
    where
        S: ToValidSqlIdent,
        SqlValueOptional: FromQuoted<V>,
    {
        let value = SqlValueOptional::quoted_value_to_string(value);
        self.sql_builder.set(column.to_valid_sql_ident()?, value);
        Ok(self)
    }

    /// Adds the `value` of the specified `column` to the `UPDATE` request.
    ///
    /// Please note the function validates the given `column` name.
    #[inline]
    pub fn set_param<S, T>(&mut self, column: S, param: T) -> SqlResult<&mut Self>
    where
        S: ToValidSqlIdent,
        OwnedSqlParam: From<T>,
    {
        let param_id = self.params.push_param(param);
        self.sql_builder.set(column.to_valid_sql_ident()?, param_id);
        Ok(self)
    }

    /// Generates a string SQL request.
    #[inline]
    pub fn sql(&self) -> SqlResult<String> {
        self.sql_builder
            .sql()
            .map_err(|e| SqlError::ToSqlConversionFailure(e.into()))
    }

    /// Returns the reference to the specified SQL parameters.
    #[inline]
    pub fn params(&self) -> &OwnedSqlParams {
        self.params.params()
    }

    /// Convenience method to execute the `UPDATE` request.
    /// Returns a number of updated records.
    /// For more details see [`SqlBuilder::execute`].
    pub fn update(self) -> SqlResult<usize> {
        let sql = self.sql()?;

        let params = self.params();
        debug!("Trying to execute SQL query {} with params {:?}", sql, params);
        self.conn.execute(&sql, params_from_iter(params.iter()))
    }
}

/// `SqlCondition` implements the following methods by default:
/// - [`SqlUpdate::and_where_eq`]
/// - [`SqlUpdate::and_where_eq_param`]
/// - [`SqlUpdate::and_where_in`]
/// - [`SqlUpdate::and_where_in_quoted`]
/// - [`SqlUpdate::and_where_in_params`]
/// - [`SqlUpdate::or_where_eq`]
/// - [`SqlUpdate::or_where_eq_param`]
/// - [`SqlUpdate::or_where_in`]
/// - [`SqlUpdate::or_where_in_quoted`]
/// - [`SqlUpdate::or_where_in_params`]
impl SqlCondition for SqlUpdate<'_> {
    fn sql_builder(&mut self) -> &mut SqlBuilder {
        &mut self.sql_builder
    }

    fn sql_params(&mut self) -> &mut SqlParamsBuilder {
        &mut self.params
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATE_TX_HISTORY_TABLE: &str = "CREATE TABLE tx_history (
        tx_hash VARCHAR(255) NOT NULL UNIQUE,
        total_amount REAL NOT NULL,
        height INTEGER NOT NULL,
        kmd_rewards REAL
    );";

    fn init_table_for_test(conn: &Connection) {
        conn.execute(CREATE_TX_HISTORY_TABLE, []).unwrap();
    }

    #[test]
    fn test_update_all_records() {
        let conn = Connection::open_in_memory().unwrap();
        init_table_for_test(&conn);

        let mut sql_update = SqlUpdate::new(&conn, "tx_history").unwrap();
        sql_update.set("total_amount", 300).unwrap();

        let actual = sql_update.sql().unwrap();
        assert_eq!(actual, "UPDATE tx_history SET total_amount = 300;");

        sql_update.update().unwrap();
    }

    #[test]
    fn test_update_where_eq() {
        const NO_KMD_REWARDS: Option<i64> = None;

        let conn = Connection::open_in_memory().unwrap();
        init_table_for_test(&conn);

        let mut sql_update = SqlUpdate::new(&conn, "tx_history").unwrap();
        sql_update
            .set("total_amount", 300)
            .unwrap()
            .and_where_eq("height", 699545)
            .unwrap()
            .or_where_eq("kmd_rewards", NO_KMD_REWARDS)
            .unwrap();

        let actual = sql_update.sql().unwrap();
        assert_eq!(
            actual,
            "UPDATE tx_history SET total_amount = 300 WHERE height = 699545 OR kmd_rewards IS NULL;"
        );

        sql_update.update().unwrap();
    }
}
