use crate::sql_condition::SqlCondition;
use crate::sql_value::{SqlValue, SqlValueToString};
use crate::sqlite::{
    query_single_row, validate_ident, validate_table_name, OwnedSqlParam, OwnedSqlParams, SqlParamsBuilder,
    StringError, ToValidSqlIdent, ToValidSqlTable,
};
use log::debug;
use rusqlite::{params_from_iter, Connection, Error as SqlError, Result as SqlResult, Row};
use sql_builder::SqlBuilder;

/// A `SELECT` SQL query builder.
#[derive(Clone)]
pub struct SqlQuery<'a> {
    conn: &'a Connection,
    sql_builder: SqlBuilder,
    params: SqlParamsBuilder,
    ordering: Vec<SqlOrdering>,
}

impl<'a> SqlQuery<'a> {
    /// Create SELECT query.
    /// Please note the function validates the given `table` name.
    pub fn select_from(conn: &'a Connection, table: &str) -> SqlResult<Self> {
        validate_table_name(table)?;
        Ok(SqlQuery {
            conn,
            sql_builder: SqlBuilder::select_from(table),
            params: SqlParamsBuilder::default(),
            ordering: Vec::default(),
        })
    }

    /// Create SELECT query.
    /// The method takes the `alias` of the `table`.
    ///
    /// Please note the function validates the given `table` and `alias` names.
    pub fn select_from_alias(conn: &'a Connection, table: &str, alias: &'static str) -> SqlResult<Self> {
        validate_table_name(table)?;
        validate_table_name(alias)?;
        Ok(SqlQuery {
            conn,
            sql_builder: SqlBuilder::select_from(format!("{table} AS {alias}")),
            params: SqlParamsBuilder::default(),
            ordering: Vec::default(),
        })
    }

    /// Create SELECT query.
    /// Please note that [`SqlQuery::ordering`] is not inherited by the external query.
    pub fn select_from_subquery(subquery: SqlSubquery<'a>) -> SqlResult<Self> {
        let subquery_sql = subquery
            .0
            .sql_builder
            .subquery()
            .map_err(|e| SqlError::ToSqlConversionFailure(e.into()))?;
        Ok(SqlQuery {
            conn: subquery.0.conn,
            sql_builder: SqlBuilder::select_from(subquery_sql),
            params: subquery.0.params,
            ordering: Vec::default(),
        })
    }

    /// Add COUNT(field).
    /// For more details see [`SqlBuilder::count`].
    ///
    /// Please note the function validates the given `field`.
    #[inline]
    pub fn count<S: ToValidSqlIdent>(&mut self, field: S) -> SqlResult<&mut Self> {
        self.sql_builder.count(field.to_valid_sql_ident()?);
        Ok(self)
    }

    /// Add COUNT(DISTINCT field).
    /// For more details see [`SqlBuilder::count`].
    ///
    /// Please note the function validates the given `field`.
    #[inline]
    pub fn count_distinct<S: ToValidSqlIdent>(&mut self, field: S) -> SqlResult<&mut Self> {
        let field = field.to_valid_sql_ident()?;
        self.sql_builder.count(format!("DISTINCT {field}"));
        Ok(self)
    }

    /// Add field.
    /// For more details see [`SqlBuilder::field`].
    ///
    /// Please note the function validates the given `field`.
    #[inline]
    pub fn field<S: ToValidSqlIdent>(&mut self, field: S) -> SqlResult<&mut Self> {
        self.sql_builder.field(field.to_valid_sql_ident()?);
        Ok(self)
    }

    /// Add field and gives it an alias.
    /// For more details see [`SqlBuilder::field`].
    ///
    /// Please note the function validates the given `field` and `alias` names.
    #[inline]
    pub fn field_alias<S: ToValidSqlIdent>(&mut self, field: S, alias: &'static str) -> SqlResult<&mut Self> {
        validate_ident(alias)?;
        self.sql_builder
            .field(format!("{} AS {}", field.to_valid_sql_ident()?, alias));
        Ok(self)
    }

    /// Set OFFSET.
    /// For more details see [`SqlBuilder::offset`].
    #[inline]
    pub fn offset(&mut self, offset: usize) -> &mut Self {
        self.sql_builder.offset(offset);
        self
    }

    /// Set LIMIT.
    /// For more details see [`SqlBuilder::limit`].
    #[inline]
    pub fn limit(&mut self, limit: usize) -> &mut Self {
        self.sql_builder.limit(limit);
        self
    }

    /// Add GROUP BY part.
    /// For more details see [`SqlBuilder::group_by`].
    ///
    /// Please note the function validates the given `field`.
    #[inline]
    pub fn group_by<S: ToValidSqlIdent>(&mut self, field: S) -> SqlResult<&mut Self> {
        self.sql_builder.group_by(field.to_valid_sql_ident()?);
        Ok(self)
    }

    /// Add ORDER BY ASC.
    /// For more details see [`SqlBuilder::order_asc`].
    ///
    /// Please note the function validates the given `field` name.
    #[inline]
    pub fn order_asc<S: ToValidSqlIdent>(&mut self, field: S) -> SqlResult<&mut Self> {
        self.ordering.push(SqlOrdering::Asc(field.to_valid_sql_ident()?));
        Ok(self)
    }

    /// Add ORDER BY DESC.
    /// For more details see [`SqlBuilder::order_desc`].
    ///
    /// Please note the function validates the given `field` name.
    #[inline]
    pub fn order_desc<S: ToValidSqlIdent>(&mut self, field: S) -> SqlResult<&mut Self> {
        self.ordering.push(SqlOrdering::Desc(field.to_valid_sql_ident()?));
        Ok(self)
    }

    /// Join the given `table` and gives it the `alias`.
    /// For more details see [`SqlBuilder::join`].
    ///
    /// Please note the function validates the given `table` and `alias` names.
    #[inline]
    pub fn join_alias<S: ToValidSqlTable>(&mut self, table: S, alias: &'static str) -> SqlResult<&mut Self> {
        validate_table_name(alias)?;
        self.sql_builder
            .join(format!("{} AS {}", table.to_valid_sql_table()?, alias));
        Ok(self)
    }

    /// Join the given `table`.
    /// For more details see [`SqlBuilder::join`].
    ///
    /// Please note the function validates the given `table` name.
    #[inline]
    pub fn join<S: ToValidSqlTable>(&mut self, table: S) -> SqlResult<&mut Self> {
        self.sql_builder.join(table.to_valid_sql_table()?);
        Ok(self)
    }

    /// Join constraint to the last JOIN part [`SqlQuery::join`].
    /// For more details see [`SqlBuilder::on_eq`].
    ///
    /// Please note the function validates the given `c1` name,
    /// and `c2` is considered a valid value as it's able to be converted into `SqlValue`.
    #[inline]
    pub fn on_join_eq<C1, C2>(&mut self, c1: C1, c2: C2) -> SqlResult<&mut Self>
    where
        C1: ToValidSqlIdent,
        SqlValue: From<C2>,
    {
        self.sql_builder
            .on_eq(c1.to_valid_sql_ident()?, SqlValue::value_to_string(c2));
        Ok(self)
    }

    #[inline]
    pub fn sql(mut self) -> SqlResult<String> {
        self.apply_ordering();
        self.sql_builder
            .sql()
            .map_err(|e| SqlError::ToSqlConversionFailure(e.into()))
    }

    /// Returns an SQL subquery that can be used in [`SqlQuery::select_from_subquery`].
    #[inline]
    pub fn subquery(self) -> SqlSubquery<'a> {
        SqlSubquery(self)
    }

    /// Returns the reference to the specified SQL parameters.
    #[inline]
    pub fn params(&self) -> &OwnedSqlParams {
        self.params.params()
    }

    /// # Usage
    ///
    /// 1) Create a `SqlQuery` instance;
    /// 2) Join tables, specify `WHERE`, `ORDER`, `GROUP BY` constraints;
    /// 3) Create a copy of the instance;
    /// 4) Call [`SqlQuery::offset_by_id`] method on the copy instance
    ///    to get an offset from which you need to query rows from the storage;
    /// 5) Specify the returned `offset` value by [`SqlQuery::offset`] on the original instance;
    /// 6) Query rows by [`SqlQuery::query`].
    ///
    /// # Note
    ///
    /// 1) It's recommended not to specify fields by [`SqlQuery::field`] before [`SqlQuery::offset_by_id`] is used;
    /// 2) Don't specify any `WHERE` constraint, ordering on the original `SqlQuery` instance
    ///    after [`SqlQuery::offset_by_id`] is called.
    #[inline]
    pub fn query_offset_by_id<S, T>(mut self, id_field: S, where_id_eq_param: T) -> SqlResult<Option<usize>>
    where
        S: ToValidSqlIdent,
        OwnedSqlParam: From<T>,
    {
        /// The alias is needed so that the external query can access the results of the subquery.
        /// Example:
        ///   SUBQUERY: `SELECT ROW_NUMBER() OVER (ORDER BY h.height ASC, h.total_amount DESC) AS __ROW, h.tx_hash as __ID_FIELD FROM tx_history h JOIN tx_address a ON h.tx_hash = a.tx_hash WHERE a.address IN ('address_2', 'address_4') GROUP BY h.tx_hash`
        ///   EXTERNAL_QUERY: `SELECT __ROW FROM (<SUBQUERY>) WHERE __ID_FIELD = :1;`
        /// Here we can't use `id_field = "h.tx_hash"` in the external query because it doesn't know about the `tx_history AS h` table.
        /// So we need to give the `id_field` an alias like `__ID_FIELD`.
        const ID_FIELD_ALIAS: &str = "__ID_FIELD";
        const ROW_NUMBER_ALIAS: &str = "__ROW";

        if self.ordering.is_empty() {
            let error = "SQL ORDERs must be specified before `SqlQuery::query_offset_by_id` is called";
            return Err(SqlError::ToSqlConversionFailure(StringError::from(error).into_boxed()));
        }

        self
            // Query the number of the row with the specified `order_by` ordering.
            .row_number_alias(ROW_NUMBER_ALIAS)?
            // Query `id_field` and give it the `__ID_FIELD` alias.
            .field_alias(id_field.to_valid_sql_ident()?, ID_FIELD_ALIAS)?;

        let mut external_query = SqlQuery::select_from_subquery(self.subquery())?;
        external_query
            .field(ROW_NUMBER_ALIAS)?
            .and_where_eq_param(ID_FIELD_ALIAS, where_id_eq_param)?;
        Ok(external_query
            .query_single_row(|row| row.get::<_, isize>(0))?
            .map(|offset| offset.try_into().expect("row index should be always above zero")))
    }

    /// Convenience method to execute a query that is expected to return mapped rows.
    /// For more details see [`SqlBuilder::query_row`].
    pub fn query<F, B>(mut self, f: F) -> SqlResult<Vec<B>>
    where
        F: FnMut(&Row<'_>) -> SqlResult<B>,
    {
        self.apply_ordering();
        let sql = self
            .sql_builder
            .sql()
            .map_err(|e| SqlError::ToSqlConversionFailure(e.into()))?;

        debug!("Trying to execute SQL query {} with params {:?}", sql, self.params());
        let mut stmt = self.conn.prepare(&sql)?;
        let items = stmt
            .query_map(params_from_iter(self.params().iter()), f)?
            .collect::<SqlResult<Vec<_>>>()?;
        // Otherwise, we'll get the compile error:
        // `stmt` does not live long enough
        Ok(items)
    }

    /// Convenience method to execute a query that is expected to return a single row.
    /// For more details see [`SqlBuilder::query_row`].
    pub fn query_single_row<F, T>(mut self, f: F) -> SqlResult<Option<T>>
    where
        F: FnOnce(&Row<'_>) -> SqlResult<T>,
    {
        self.apply_ordering();
        let sql = self
            .sql_builder
            .sql()
            .map_err(|e| SqlError::ToSqlConversionFailure(e.into()))?;
        debug!("Trying to execute SQL query {} with params {:?}", sql, self.params());
        query_single_row(self.conn, &sql, params_from_iter(self.params().iter()), f)
    }

    /// Applies [`SqlQuery::ordering`] to [`SqlQuery::sql_builder`].
    /// Please note the functions clears [`SqlQuery::ordering`].
    fn apply_ordering(&mut self) {
        for order in self.ordering.drain(..) {
            match order {
                SqlOrdering::Asc(column) => self.sql_builder.order_asc(column),
                SqlOrdering::Desc(column) => self.sql_builder.order_desc(column),
            };
        }
    }

    /// Add `ROW_NUMBER()` field with the specified `order_by` ordering,
    /// and give it the specified `alias`.
    /// For more details see [`SqlBuilder::field`].
    ///
    /// Please note the functions clears [`SqlQuery::ordering`].
    fn row_number_alias(&mut self, alias: &'static str) -> SqlResult<&mut Self> {
        validate_ident(alias)?;
        let order_by = self
            .ordering
            .drain(..)
            .map(|ordering| SqlOrdering::to_sql(&ordering))
            .collect::<Vec<_>>()
            .join(", ");
        // Query the number of the row with the specified `order_by` ordering.
        self.sql_builder
            .field(format!("ROW_NUMBER() OVER (ORDER BY {order_by}) AS {alias}"));
        Ok(self)
    }

    /// Count all rows
    pub fn count_all(&mut self) -> SqlResult<&mut Self> {
        self.sql_builder.count("*");
        Ok(self)
    }

    /// Select from union tables
    pub fn select_from_union_alias(conn: &'a Connection, union_sql: &str, alias: &'static str) -> SqlResult<Self> {
        validate_table_name(alias)?;
        Ok(SqlQuery {
            conn,
            sql_builder: SqlBuilder::select_from(format!("({union_sql}) AS {alias}")),
            params: SqlParamsBuilder::default(),
            ordering: Vec::default(),
        })
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
impl SqlCondition for SqlQuery<'_> {
    fn sql_builder(&mut self) -> &mut SqlBuilder {
        &mut self.sql_builder
    }

    fn sql_params(&mut self) -> &mut SqlParamsBuilder {
        &mut self.params
    }
}

/// An instance of this structure is returned by [`SqlQuery::subquery`].
pub struct SqlSubquery<'a>(SqlQuery<'a>);

#[derive(Clone)]
enum SqlOrdering {
    Asc(String),
    Desc(String),
}

impl SqlOrdering {
    fn to_sql(&self) -> String {
        match self {
            SqlOrdering::Asc(column) => format!("{column} ASC"),
            SqlOrdering::Desc(column) => format!("{column} DESC"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_insert::SqlInsert;

    const CREATE_TX_HISTORY_TABLE: &str = "CREATE TABLE tx_history (
        tx_hash VARCHAR(255) NOT NULL UNIQUE,
        height INTEGER NOT NULL,
        total_amount INTEGER NOT NULL,
        kmd_rewards INTEGER
    );";
    const CREATE_TX_ADDRESS_TABLE: &str = "CREATE TABLE tx_address (
        tx_hash VARCHAR(255) NOT NULL,
        address VARCHAR(255) NOT NULL
    );";

    fn init_table_for_test(conn: &Connection) {
        conn.execute(CREATE_TX_HISTORY_TABLE, []).unwrap();
        conn.execute(CREATE_TX_ADDRESS_TABLE, []).unwrap();

        let history_items = vec![
            ("tx_hash_1", 699545, 23, Some(0.5)),
            ("tx_hash_2", 699547, 10, Some(0.2)),
            ("tx_hash_3", 699547, 11, None),
            ("tx_hash_4", 699530, 100, Some(1.01)),
            ("tx_hash_5", 699532, 19, None),
        ];

        for (tx_hash, height, total_amount, kmd_rewards) in history_items {
            let mut insert_sql = SqlInsert::new(conn, "tx_history");
            insert_sql
                .column_quoted("tx_hash", tx_hash)
                .unwrap()
                .column("height", height)
                .unwrap()
                .column("total_amount", total_amount)
                .unwrap()
                .column("kmd_rewards", kmd_rewards)
                .unwrap();
            insert_sql.insert().unwrap();
        }

        let address_table_items = vec![
            ("tx_hash_1", "address_1"),
            ("tx_hash_1", "address_2"),
            ("tx_hash_2", "address_1"),
            ("tx_hash_2", "address_3"),
            ("tx_hash_2", "address_4"),
            ("tx_hash_3", "address_3"),
            ("tx_hash_4", "address_2"),
            ("tx_hash_4", "address_4"),
            ("tx_hash_5", "address_1"),
        ];
        for (tx_hash, address) in address_table_items {
            let mut insert_sql = SqlInsert::new(conn, "tx_address");
            insert_sql
                .column_quoted("tx_hash", tx_hash)
                .unwrap()
                .column_quoted("address", address)
                .unwrap();
            insert_sql.insert().unwrap();
        }
    }

    #[test]
    fn test_query_join() {
        const SEARCHING_ADDRESSES: [&str; 2] = ["address_2", "address_4"];

        let conn = Connection::open_in_memory().unwrap();
        init_table_for_test(&conn);

        let mut query = SqlQuery::select_from(&conn, "tx_history").unwrap();
        query
            .field("tx_history.tx_hash")
            .unwrap()
            .join("tx_address")
            .unwrap()
            .on_join_eq("tx_history.tx_hash", "tx_address.tx_hash")
            .unwrap()
            .and_where_in_quoted("tx_address.address", SEARCHING_ADDRESSES.to_owned())
            .unwrap()
            .group_by("tx_history.tx_hash")
            .unwrap();
        assert_eq!(
            query.clone().sql().unwrap(),
            "SELECT tx_history.tx_hash FROM tx_history JOIN tx_address ON tx_history.tx_hash = tx_address.tx_hash WHERE tx_address.address IN ('address_2', 'address_4') GROUP BY tx_history.tx_hash;"
        );
        let actual: Vec<String> = query.query(|row| row.get(0)).unwrap();
        let expected = vec!["tx_hash_1".to_owned(), "tx_hash_2".to_owned(), "tx_hash_4".to_owned()];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_query_where_in_params() {
        const SEARCHING_HEIGHTS: [u32; 3] = [699547, 699530, 699533];

        let conn = Connection::open_in_memory().unwrap();
        init_table_for_test(&conn);

        let mut query = SqlQuery::select_from(&conn, "tx_history").unwrap();
        query
            .field("tx_hash")
            .unwrap()
            .and_where_in_params("height", SEARCHING_HEIGHTS)
            .unwrap();
        assert_eq!(
            query.clone().sql().unwrap(),
            "SELECT tx_hash FROM tx_history WHERE height IN (:1, :2, :3);"
        );
        assert_eq!(query.params(), &vec![699547.into(), 699530.into(), 699533.into()]);

        let actual: Vec<String> = query.query(|row| row.get(0)).unwrap();
        let expected = vec!["tx_hash_2".to_owned(), "tx_hash_3".to_owned(), "tx_hash_4".to_owned()];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_query_where_eq_null() {
        const NO_KMD_REWARDS: Option<f64> = None;

        let conn = Connection::open_in_memory().unwrap();
        init_table_for_test(&conn);

        macro_rules! run_test {
            ($fun: ident) => {{
                let mut query = SqlQuery::select_from(&conn, "tx_history").unwrap();
                query
                    .field("tx_hash")
                    .unwrap()
                    .$fun("kmd_rewards", NO_KMD_REWARDS)
                    .unwrap();

                // Check if the `kmd_rewards IS NULL` instead of `kmd_rewards = NULL` or `kmd_rewards = :1`.
                assert_eq!(
                    query.clone().sql().unwrap(),
                    "SELECT tx_hash FROM tx_history WHERE kmd_rewards IS NULL;"
                );

                let actual: Vec<String> = query.query(|row| row.get(0)).unwrap();
                let expected = vec!["tx_hash_3".to_owned(), "tx_hash_5".to_owned()];
                assert_eq!(actual, expected);
            }};
        }

        run_test!(and_where_eq);
        run_test!(or_where_eq);
        run_test!(and_where_eq_param);
        run_test!(or_where_eq_param);
    }

    #[test]
    fn test_query_offset_by_id() {
        let conn = Connection::open_in_memory().unwrap();
        init_table_for_test(&conn);

        let mut query = SqlQuery::select_from(&conn, "tx_history").unwrap();
        query.order_asc("height").unwrap().order_desc("total_amount").unwrap();

        // Resulting rows:
        // 1) tx_hash="tx_hash_4", height=699530, total_amount=100
        // 2) tx_hash="tx_hash_5", height=699532, total_amount=19
        // 3) tx_hash="tx_hash_1", height=699545, total_amount=23
        // 4) tx_hash="tx_hash_3", height=699547, total_amount=11
        // 5) tx_hash="tx_hash_2", height=699547, total_amount=10

        let actual = query
            .clone()
            .query_offset_by_id("tx_hash", "tx_hash_4".to_owned())
            .unwrap();
        assert_eq!(actual, Some(1));

        let actual = query
            .clone()
            .query_offset_by_id("tx_hash", "tx_hash_3".to_owned())
            .unwrap();
        assert_eq!(actual, Some(4));

        let actual = query
            .clone()
            .query_offset_by_id("tx_hash", "tx_hash_2".to_owned())
            .unwrap();
        assert_eq!(actual, Some(5));

        let actual = query.query_offset_by_id("tx_hash", "tx_hash_6".to_owned()).unwrap();
        assert_eq!(actual, None);
    }

    #[test]
    fn test_query_offset_by_id_with_join() {
        const SEARCHING_ADDRESSES: [&str; 2] = ["address_2", "address_4"];

        let conn = Connection::open_in_memory().unwrap();
        init_table_for_test(&conn);

        let mut query = SqlQuery::select_from_alias(&conn, "tx_history", "h").unwrap();
        query
            .order_asc("h.height")
            .unwrap()
            .order_desc("h.total_amount")
            .unwrap()
            .join_alias("tx_address", "a")
            .unwrap()
            .on_join_eq("h.tx_hash", "a.tx_hash")
            .unwrap()
            .and_where_in_quoted("a.address", SEARCHING_ADDRESSES.to_owned())
            .unwrap()
            .group_by("h.tx_hash")
            .unwrap();

        // Resulting rows:
        // 1) tx_hash="tx_hash_4", height=699530, total_amount=100
        // 2) tx_hash="tx_hash_1", height=699545, total_amount=23
        // 3) tx_hash="tx_hash_2", height=699547, total_amount=10

        let actual = query
            .clone()
            .query_offset_by_id("h.tx_hash", "tx_hash_4".to_owned())
            .unwrap();
        assert_eq!(actual, Some(1));

        let actual = query
            .clone()
            .query_offset_by_id("h.tx_hash", "tx_hash_1".to_owned())
            .unwrap();
        assert_eq!(actual, Some(2));

        let actual = query
            .clone()
            .query_offset_by_id("h.tx_hash", "tx_hash_2".to_owned())
            .unwrap();
        assert_eq!(actual, Some(3));

        let actual = query.query_offset_by_id("h.tx_hash", "tx_hash_3".to_owned()).unwrap();
        assert_eq!(actual, None);
    }
}
