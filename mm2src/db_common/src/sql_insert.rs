use crate::sql_value::{FromQuoted, SqlValueOptional, SqlValueToString};
use crate::sqlite::{OwnedSqlParam, OwnedSqlParams, SqlParamsBuilder, ToValidSqlIdent};
use common::write_safe;
use common::write_safe::fmt::{WriteSafe, WriteSafeJoin};
use log::debug;
use rusqlite::{params_from_iter, Connection, Result as SqlResult};
use std::fmt;

enum InsertMode {
    OrReplace,
    OrIgnore,
}

impl fmt::Display for InsertMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InsertMode::OrReplace => write!(f, "OR REPLACE"),
            InsertMode::OrIgnore => write!(f, "OR IGNORE"),
        }
    }
}

/// An `INSERT` SQL request builder.
pub struct SqlInsert<'a> {
    conn: &'a Connection,
    table_name: &'static str,
    columns: Vec<String>,
    values: Vec<String>,
    params: SqlParamsBuilder,
    mode: Option<InsertMode>,
}

impl<'a> SqlInsert<'a> {
    const INSERT_CAPACITY: usize = 200;
    const VALUE_CAPACITY: usize = 80;

    /// Creates a new instance of `SqlInsert` builder.
    #[inline]
    pub fn new(conn: &'a Connection, table_name: &'static str) -> Self {
        SqlInsert {
            conn,
            table_name,
            columns: Vec::new(),
            values: Vec::new(),
            params: SqlParamsBuilder::default(),
            mode: None,
        }
    }

    /// Allows replacing the record if it's present in the table already.
    #[inline]
    pub fn or_replace(&mut self) -> &mut Self {
        self.mode = Some(InsertMode::OrReplace);
        self
    }

    /// Allows ignoring the `INSERT` request if it's present in the table already.
    #[inline]
    pub fn or_ignore(&mut self) -> &mut Self {
        self.mode = Some(InsertMode::OrIgnore);
        self
    }

    /// Adds the `value` of the specified `column` to the `INSERT` request.
    ///
    /// Please note the function validates the given `column` name,
    /// and `value` is considered an valid optional value as it's able to be converted into `SqlValueOptional`.
    #[inline]
    pub fn column<S, T>(&mut self, column: S, value: T) -> SqlResult<&mut Self>
    where
        S: ToValidSqlIdent,
        SqlValueOptional: From<T>,
    {
        self.columns.push(column.to_valid_sql_ident()?);
        self.values.push(SqlValueOptional::from(value).to_string());
        Ok(self)
    }

    /// Adds the quoted `value` of the specified `column` to the `INSERT` request.
    ///
    /// Please note the function validates the given `column` name,
    /// and `value` is considered a valid optional value as it's able to be converted into `SqlValueOptional`.
    #[inline]
    pub fn column_quoted<S, T>(&mut self, column: S, value: T) -> SqlResult<&mut Self>
    where
        S: ToValidSqlIdent,
        SqlValueOptional: FromQuoted<T>,
    {
        self.columns.push(column.to_valid_sql_ident()?);
        self.values.push(SqlValueOptional::quoted_value_to_string(value));
        Ok(self)
    }

    /// Adds the `value` of the specified `column` to the `INSERT` request.
    ///
    /// Please note the function validates the given `column` name.
    #[inline]
    pub fn column_param<S, T>(&mut self, column: S, param: T) -> SqlResult<&mut Self>
    where
        S: ToValidSqlIdent,
        OwnedSqlParam: From<T>,
    {
        self.columns.push(column.to_valid_sql_ident()?);
        self.values.push(self.params.push_param(param));
        Ok(self)
    }

    /// Convenience method to execute an insertion.
    /// Returns a number of inserted records.
    pub fn insert(&self) -> SqlResult<usize> {
        let sql = self.sql()?;

        debug!("Trying to execute SQL query {} with params {:?}", sql, self.params());
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.execute(params_from_iter(self.params().iter()))
    }

    /// Returns the reference to the specified SQL parameters.
    #[inline]
    pub fn params(&self) -> &OwnedSqlParams {
        self.params.params()
    }

    /// Generates a string SQL request.
    pub fn sql(&self) -> SqlResult<String> {
        let mut sql = String::with_capacity(Self::INSERT_CAPACITY + self.columns.len() * Self::VALUE_CAPACITY);

        write_safe!(sql, "INSERT");
        if let Some(ref mode) = self.mode {
            write_safe!(sql, " {}", mode);
        }
        write_safe!(sql, " INTO {}", self.table_name);

        if self.columns.is_empty() {
            write_safe!(sql, " DEFAULT VALUES;");
        } else {
            write_safe!(sql, " (");
            self.columns.iter().write_safe_join(&mut sql, ", ");
            write_safe!(sql, ") VALUES (");
            self.values.iter().write_safe_join(&mut sql, ", ");
            write_safe!(sql, ");");
        }

        Ok(sql)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATE_TX_HISTORY_TABLE: &str = "CREATE TABLE tx_history (
        tx_hash VARCHAR(255) NOT NULL UNIQUE,
        description TEXT,
        tx_hex BLOB,
        height INTEGER,
        total_amount DECIMAL
    );";
    const CREATE_ID_TABLE: &str = "CREATE TABLE id (identifier INTEGER NOT NULL PRIMARY KEY)";
    const SELECT_FROM_TX_HISTORY: &str = "SELECT * FROM tx_history;";
    const SELECT_FROM_ID_TABLE: &str = "SELECT * FROM id;";

    #[derive(Debug, PartialEq)]
    struct TxHistoryItem {
        tx_hash: String,
        description: Option<String>,
        tx_hex: Option<Vec<u8>>,
        height: Option<i64>,
        total_amount: Option<f64>,
    }

    fn select_from_tx_history(conn: &Connection) -> Vec<TxHistoryItem> {
        let mut stmt = conn.prepare(SELECT_FROM_TX_HISTORY).unwrap();
        stmt.query_map([], |row| {
            Ok(TxHistoryItem {
                tx_hash: row.get(0)?,
                description: row.get(1)?,
                tx_hex: row.get(2)?,
                height: row.get(3)?,
                total_amount: row.get(4)?,
            })
        })
        .unwrap()
        .collect::<SqlResult<Vec<_>>>()
        .unwrap()
    }

    fn select_from_id_table(conn: &Connection) -> Vec<i64> {
        let mut stmt = conn.prepare(SELECT_FROM_ID_TABLE).unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .collect::<SqlResult<_>>()
            .unwrap()
    }

    #[test]
    fn test_sql_insert() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(CREATE_TX_HISTORY_TABLE, []).unwrap();

        let mut insert = SqlInsert::new(&conn, "tx_history");
        insert
            .column_quoted("tx_hash", "tx_hash_1")
            .unwrap()
            .column_quoted("description", "any description")
            .unwrap()
            .column_param("tx_hex", vec![1, 2, 3, 4, 5])
            .unwrap()
            .column("height", 102030)
            .unwrap()
            .column("total_amount", 0.3)
            .unwrap();

        let actual_sql = insert.sql().unwrap();
        let expected_sql = "INSERT INTO tx_history (tx_hash, description, tx_hex, height, total_amount) \
            VALUES ('tx_hash_1', 'any description', :1, 102030, 0.3);";
        assert_eq!(actual_sql, expected_sql);

        insert.insert().unwrap();

        let mut insert_or_replace = SqlInsert::new(&conn, "tx_history");
        insert_or_replace
            .or_replace()
            .column_quoted("tx_hash", "tx_hash_1")
            .unwrap()
            .column_quoted("description", "another description")
            .unwrap();

        let actual_or_replace = insert_or_replace.sql().unwrap();
        let expected_or_replace = "INSERT OR REPLACE INTO tx_history (tx_hash, description) \
            VALUES ('tx_hash_1', 'another description');";
        assert_eq!(actual_or_replace, expected_or_replace);

        let actual_items = select_from_tx_history(&conn);
        let expected_items = vec![TxHistoryItem {
            tx_hash: "tx_hash_1".to_string(),
            description: Some("any description".to_string()),
            tx_hex: Some(vec![1, 2, 3, 4, 5]),
            height: Some(102030),
            total_amount: Some(0.3),
        }];
        assert_eq!(actual_items, expected_items);
    }

    #[test]
    fn test_sql_insert_nulls() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(CREATE_TX_HISTORY_TABLE, []).unwrap();

        let description: Option<&'static str> = None;
        let tx_hex: Option<Vec<u8>> = None;
        let height: Option<i64> = None;
        let total_amount: Option<f64> = None;

        let mut insert = SqlInsert::new(&conn, "tx_history");
        insert
            .column_quoted("tx_hash", "tx_hash_1")
            .unwrap()
            .column_quoted("description", description)
            .unwrap()
            .column_param("tx_hex", tx_hex)
            .unwrap()
            .column("height", height)
            .unwrap()
            .column("total_amount", total_amount)
            .unwrap();

        let actual = insert.sql().unwrap();
        let expected = "INSERT INTO tx_history (tx_hash, description, tx_hex, height, total_amount) \
            VALUES ('tx_hash_1', NULL, :1, NULL, NULL);";
        assert_eq!(actual, expected);

        insert.insert().unwrap();

        let actual_items = select_from_tx_history(&conn);
        let expected_items = vec![TxHistoryItem {
            tx_hash: "tx_hash_1".to_string(),
            description: None,
            tx_hex: None,
            height: None,
            total_amount: None,
        }];
        assert_eq!(actual_items, expected_items);
    }

    #[test]
    fn test_sql_insert_one_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(CREATE_TX_HISTORY_TABLE, []).unwrap();

        let mut insert = SqlInsert::new(&conn, "tx_history");
        insert.column_quoted("tx_hash", "tx_hash_1").unwrap();

        let actual = insert.sql().unwrap();
        let expected = "INSERT INTO tx_history (tx_hash) VALUES ('tx_hash_1');";
        assert_eq!(actual, expected);

        insert.insert().unwrap();

        let actual_items = select_from_tx_history(&conn);
        let expected_items = vec![TxHistoryItem {
            tx_hash: "tx_hash_1".to_string(),
            description: None,
            tx_hex: None,
            height: None,
            total_amount: None,
        }];
        assert_eq!(actual_items, expected_items);
    }

    #[test]
    fn test_sql_create_no_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(CREATE_ID_TABLE, []).unwrap();

        let insert = SqlInsert::new(&conn, "id");

        let actual = insert.sql().unwrap();
        let expected = r#"INSERT INTO id DEFAULT VALUES;"#;
        assert_eq!(actual, expected);

        insert.insert().unwrap();

        let actual_items = select_from_id_table(&conn);
        assert_eq!(actual_items, vec![1]);
    }
}
