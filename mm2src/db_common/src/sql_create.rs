use crate::sql_constraint::SqlConstraint;
use crate::sql_value::{FromQuoted, SqlValue};
use crate::sqlite::StringError;
use common::{
    write_safe,
    write_safe::fmt::{WriteSafe, WriteSafeJoin},
};
use rusqlite::{Connection, Error as SqlError, Result as SqlResult};
use std::fmt;

pub enum SqlType {
    Varchar(usize),
    Integer,
    Text,
    Real,
    Blob,
}

impl fmt::Display for SqlType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlType::Varchar(len) => write!(f, "VARCHAR({len})"),
            SqlType::Integer => write!(f, "INTEGER"),
            SqlType::Text => write!(f, "TEXT"),
            SqlType::Real => write!(f, "REAL"),
            SqlType::Blob => write!(f, "BLOB"),
        }
    }
}

pub enum TableKey {
    Primary,
}

pub struct SqlColumn {
    name: &'static str,
    column_type: SqlType,
    unique: bool,
    not_null: bool,
    default: Option<SqlValue>,
    key: Option<TableKey>,
}

impl SqlColumn {
    const COLUMN_CAPACITY: usize = 100;

    pub fn new(name: &'static str, column_type: SqlType) -> SqlColumn {
        SqlColumn {
            name,
            column_type,
            unique: false,
            not_null: false,
            default: None,
            key: None,
        }
    }

    pub fn unique(mut self) -> SqlColumn {
        self.unique = true;
        self
    }

    pub fn not_null(mut self) -> SqlColumn {
        self.not_null = true;
        self
    }

    pub fn default<T>(mut self, default: T) -> SqlColumn
    where
        SqlValue: From<T>,
    {
        self.default = Some(SqlValue::from(default));
        self
    }

    pub fn default_quoted<T>(mut self, default: T) -> SqlColumn
    where
        SqlValue: FromQuoted<T>,
    {
        self.default = Some(SqlValue::from_quoted(default));
        self
    }

    pub fn primary(mut self) -> SqlColumn {
        self.key = Some(TableKey::Primary);
        self
    }
}

impl fmt::Display for SqlColumn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.name, self.column_type)?;
        if let Some(ref default) = self.default {
            write!(f, " DEFAULT {default}")?;
        }
        if self.not_null {
            write!(f, " NOT NULL")?;
        }
        if self.unique {
            write!(f, " UNIQUE")?;
        }
        if let Some(TableKey::Primary) = self.key {
            write!(f, " PRIMARY KEY")?;
        }
        Ok(())
    }
}

pub struct SqlCreateTable<'a> {
    conn: &'a Connection,
    table_name: &'static str,
    columns: Vec<SqlColumn>,
    constraint: Vec<SqlConstraint>,
    if_not_exist: bool,
}

impl<'a> SqlCreateTable<'a> {
    const CREATE_CAPACITY: usize = 200;

    pub fn new(conn: &'a Connection, table_name: &'static str) -> Self {
        SqlCreateTable {
            conn,
            table_name,
            columns: Vec::new(),
            constraint: Vec::new(),
            if_not_exist: false,
        }
    }

    pub fn if_not_exist(&mut self) -> &mut Self {
        self.if_not_exist = true;
        self
    }

    pub fn column(&mut self, column: SqlColumn) -> &mut Self {
        self.columns.push(column);
        self
    }

    pub fn constraint<C>(&mut self, constraint: C) -> &mut Self
    where
        SqlConstraint: From<C>,
    {
        self.constraint.push(SqlConstraint::from(constraint));
        self
    }

    pub fn create(self) -> SqlResult<()> {
        self.conn.execute(&self.sql()?, [])?;
        Ok(())
    }

    /// Generates a string SQL request.
    pub fn sql(&self) -> SqlResult<String> {
        if self.columns.is_empty() {
            let error = "SQL CREATE TABLE columns must be specified before `SqlQuery::create` is called";
            return Err(SqlError::ToSqlConversionFailure(StringError::from(error).into_boxed()));
        }

        let mut sql = String::with_capacity(Self::CREATE_CAPACITY + self.columns.len() * SqlColumn::COLUMN_CAPACITY);

        write_safe!(sql, "CREATE TABLE");
        if self.if_not_exist {
            write_safe!(sql, " IF NOT EXISTS");
        }
        write_safe!(sql, " {} (", self.table_name);

        self.columns
            .iter()
            .map(|column| column as &dyn fmt::Display)
            .chain(self.constraint.iter().map(|constraint| constraint as &dyn fmt::Display))
            .write_safe_join(&mut sql, ", ");

        write_safe!(sql, ");");
        Ok(sql)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_constraint::Unique;

    #[test]
    fn test_sql_create() {
        let conn = Connection::open_in_memory().unwrap();

        let mut create = SqlCreateTable::new(&conn, "my_swaps");
        create
            .if_not_exist()
            .column(SqlColumn::new("id", SqlType::Integer).not_null().primary())
            .column(SqlColumn::new("my_coin", SqlType::Varchar(255)))
            .column(SqlColumn::new("other_coin", SqlType::Varchar(255)))
            .column(SqlColumn::new("uuid", SqlType::Varchar(255)).not_null().unique())
            .column(
                SqlColumn::new("started_at", SqlType::Integer)
                    .not_null()
                    .default("CURRENT_TIMESTAMP"),
            )
            .column(SqlColumn::new("about", SqlType::Text).default_quoted(""))
            .column(SqlColumn::new("changed_balance", SqlType::Real))
            .column(SqlColumn::new("raw_data", SqlType::Blob));

        let actual = create.sql().unwrap();
        let expected = "CREATE TABLE IF NOT EXISTS my_swaps (\
            id INTEGER NOT NULL PRIMARY KEY, \
            my_coin VARCHAR(255), \
            other_coin VARCHAR(255), \
            uuid VARCHAR(255) NOT NULL UNIQUE, \
            started_at INTEGER DEFAULT CURRENT_TIMESTAMP NOT NULL, \
            about TEXT DEFAULT '', \
            changed_balance REAL, \
            raw_data BLOB\
        );";
        assert_eq!(actual, expected);

        create.create().unwrap();
    }

    #[test]
    fn test_sql_create_one_column() {
        let conn = Connection::open_in_memory().unwrap();

        let mut create = SqlCreateTable::new(&conn, "my_swaps");
        create.column(SqlColumn::new("id", SqlType::Integer).not_null().primary());

        let actual = create.sql().unwrap();
        let expected = "CREATE TABLE my_swaps (id INTEGER NOT NULL PRIMARY KEY);";
        assert_eq!(actual, expected);

        create.create().unwrap();
    }

    #[test]
    fn test_sql_create_no_columns() {
        let conn = Connection::open_in_memory().unwrap();

        SqlCreateTable::new(&conn, "my_swaps")
            .sql()
            .expect_err("SQL CREATE TABLE must contain columns");
    }

    #[test]
    fn test_sql_create_with_constraints() {
        let conn = Connection::open_in_memory().unwrap();

        let mut create = SqlCreateTable::new(&conn, "my_swaps");
        create
            .column(SqlColumn::new("id", SqlType::Integer).not_null().primary())
            .column(SqlColumn::new("uuid", SqlType::Varchar(255)).not_null())
            .constraint(Unique::new("id_uuid_constraint", ["id", "uuid"]).unwrap());
        let actual = create.sql().unwrap();
        let expected = "CREATE TABLE my_swaps (\
            id INTEGER NOT NULL PRIMARY KEY, \
            uuid VARCHAR(255) NOT NULL, \
            CONSTRAINT id_uuid_constraint UNIQUE (id, uuid)\
        );";
        assert_eq!(actual, expected);

        create.create().unwrap();
    }
}
