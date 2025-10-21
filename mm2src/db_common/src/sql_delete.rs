use crate::sql_condition::SqlCondition;
use crate::sqlite::{validate_table_name, OwnedSqlParams, SqlParamsBuilder};
use common::log::debug;
use rusqlite::{params_from_iter, Connection, Error as SqlError, Result as SqlResult};
use sql_builder::SqlBuilder;

/// A `DELETE` SQL request builder.
pub struct SqlDelete<'a> {
    conn: &'a Connection,
    sql_builder: SqlBuilder,
    params: SqlParamsBuilder,
}

impl<'a> SqlDelete<'a> {
    /// Creates `DELETE` request builder.
    /// Please note the function validates the given `table` name.
    #[inline]
    pub fn new(conn: &'a Connection, table: &str) -> SqlResult<Self> {
        validate_table_name(table)?;
        Ok(SqlDelete {
            conn,
            sql_builder: SqlBuilder::delete_from(table),
            params: SqlParamsBuilder::default(),
        })
    }

    /// Returns a reference to the SQL params of the request.
    #[inline]
    pub fn params(&self) -> &OwnedSqlParams {
        self.params.params()
    }

    /// Convenience method to execute the `DELETE` request.
    /// Returns a number of deleted records.
    /// For more details see [`SqlBuilder::execute`].
    pub fn delete(self) -> SqlResult<usize> {
        let sql = self.sql()?;

        let params = self.params();
        debug!("Trying to execute SQL query {} with params {:?}", sql, params);
        let params = params.clone().into_boxed_slice();
        self.conn.execute(&sql, params_from_iter(params.iter()))
    }

    /// Generates a string SQL request.
    #[inline]
    pub fn sql(&self) -> SqlResult<String> {
        self.sql_builder
            .sql()
            .map_err(|e| SqlError::ToSqlConversionFailure(e.into()))
    }
}

/// `SqlCondition` implements the following methods by default:
/// - [`SqlQuery::and_where_eq`]
/// - [`SqlQuery::and_where_eq_param`]
/// - [`SqlQuery::and_where_in`]
/// - [`SqlQuery::and_where_in_quoted`]
/// - [`SqlQuery::and_where_in_params`]
/// - [`SqlQuery::or_where_eq`]
/// - [`SqlQuery::or_where_eq_param`]
/// - [`SqlQuery::or_where_in`]
/// - [`SqlQuery::or_where_in_quoted`]
/// - [`SqlQuery::or_where_in_params`]
impl SqlCondition for SqlDelete<'_> {
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
        from_address VARCHAR(255) NOT NULL,
        height INTEGER NOT NULL,
        description TEXT
    );";

    fn init_table_for_test(conn: &Connection) {
        conn.execute(CREATE_TX_HISTORY_TABLE, []).unwrap();
    }

    #[test]
    fn test_delete_all_sql() {
        let conn = Connection::open_in_memory().unwrap();
        println!("{CREATE_TX_HISTORY_TABLE}");
        init_table_for_test(&conn);

        let sql_delete = SqlDelete::new(&conn, "tx_history").unwrap();

        let actual = sql_delete.sql().unwrap();
        assert_eq!(actual, "DELETE FROM tx_history;");

        sql_delete.delete().unwrap();
    }

    #[test]
    fn test_delete_where_sql() {
        let conn = Connection::open_in_memory().unwrap();
        init_table_for_test(&conn);

        let mut sql_delete = SqlDelete::new(&conn, "tx_history").unwrap();
        sql_delete
            .and_where_eq("height", 100)
            .unwrap()
            .or_where_eq_param("description", Some("My 100th TX".to_string()))
            .unwrap()
            .and_where_in_quoted("from_address", ["3MqZh9ips9W5ekHbzLaRxs8xZZTbJzTLwd"])
            .unwrap();

        let actual = sql_delete.sql().unwrap();
        assert_eq!(
            actual,
            "DELETE FROM tx_history WHERE (height = 100 OR description = :1) AND (from_address IN ('3MqZh9ips9W5ekHbzLaRxs8xZZTbJzTLwd'));"
        );

        sql_delete.delete().unwrap();
    }
}
