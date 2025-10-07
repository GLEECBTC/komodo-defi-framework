#[cfg(all(test, not(target_arch = "wasm32")))]
mod async_conn_tests;
#[cfg(not(target_arch = "wasm32"))]
pub mod async_sql_conn;
#[cfg(not(target_arch = "wasm32"))]
mod sql_condition;
#[cfg(not(target_arch = "wasm32"))]
mod sql_constraint;
#[cfg(not(target_arch = "wasm32"))]
mod sql_create;
#[cfg(not(target_arch = "wasm32"))]
mod sql_delete;
#[cfg(not(target_arch = "wasm32"))]
mod sql_insert;
#[cfg(not(target_arch = "wasm32"))]
mod sql_query;
#[cfg(not(target_arch = "wasm32"))]
mod sql_update;
#[cfg(not(target_arch = "wasm32"))]
mod sql_value;
#[cfg(not(target_arch = "wasm32"))]
pub mod sqlite;

#[cfg(not(target_arch = "wasm32"))]
pub mod sql_build {
    pub use crate::sql_condition::SqlCondition;
    pub use crate::sql_constraint::{foreign_key, ForeignKey, PrimaryKey, SqlConstraint, Unique};
    pub use crate::sql_create::{SqlColumn, SqlCreateTable, SqlType, TableKey};
    pub use crate::sql_delete::SqlDelete;
    pub use crate::sql_insert::SqlInsert;
    pub use crate::sql_query::{SqlQuery, SqlSubquery};
    pub use crate::sql_update::SqlUpdate;
    pub use crate::sql_value::{FromQuoted, SqlValue, SqlValueOptional};
}
