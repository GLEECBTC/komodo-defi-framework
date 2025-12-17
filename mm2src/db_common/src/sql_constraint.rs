use crate::sqlite::StringError;
use common::write_safe::fmt::WriteJoin;
use rusqlite::{Error as SqlError, Result as SqlResult};
use std::collections::BTreeMap;
use std::fmt;

pub use foreign_key::ForeignKey;

macro_rules! named_constraint {
    ($constraint_ident:ident, $constraint_str:literal) => {
        #[derive(Debug)]
        pub struct $constraint_ident {
            name: &'static str,
            columns: Vec<&'static str>,
        }

        impl $constraint_ident {
            const CONSTRAINT: &'static str = $constraint_str;

            pub fn new<I>(constraint_name: &'static str, columns: I) -> SqlResult<$constraint_ident>
            where
                I: IntoIterator<Item = &'static str>,
            {
                let columns: Vec<_> = columns.into_iter().collect();
                if columns.is_empty() {
                    return Err(no_columns_error(Self::CONSTRAINT));
                }
                Ok($constraint_ident {
                    name: constraint_name,
                    columns,
                })
            }
        }

        impl From<$constraint_ident> for SqlConstraint {
            fn from(constraint: $constraint_ident) -> Self {
                SqlConstraint::$constraint_ident(constraint)
            }
        }

        impl fmt::Display for $constraint_ident {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                assert!(
                    !self.columns.is_empty(),
                    "{}::new must check if no columns were passed",
                    stringify!($constraint_ident)
                );
                write!(f, "CONSTRAINT {} {} (", self.name, Self::CONSTRAINT)?;

                self.columns.iter().write_join(f, ", ")?;
                write!(f, ")")
            }
        }
    };
}

pub enum SqlConstraint {
    Unique(Unique),
    PrimaryKey(PrimaryKey),
    ForeignKey(ForeignKey),
}

impl fmt::Display for SqlConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlConstraint::Unique(unique) => write!(f, "{unique}"),
            SqlConstraint::PrimaryKey(prim_key) => write!(f, "{prim_key}"),
            SqlConstraint::ForeignKey(foreign_key) => write!(f, "{foreign_key}"),
        }
    }
}

// This type is used to define a UNIQUE constraint on multiple columns.
// https://www.w3schools.com/sql/sql_unique.asp
named_constraint!(Unique, "UNIQUE");

// This type is used to define a PRIMARY KEY constraint on multiple columns.
// https://www.w3schools.com/sql/sql_primarykey.asp
named_constraint!(PrimaryKey, "PRIMARY KEY");

pub mod foreign_key {
    use super::*;
    use std::fmt;

    #[macro_export]
    macro_rules! foreign_columns {
        ($($referenced_col:expr => $parent_col:expr),+ $(,)?) => {
            [
                $((
                    $crate::sql_build::foreign_key::ReferencedColumn($referenced_col),
                    $crate::sql_build::foreign_key::ParentColumn($parent_col),
                )),+
            ]
        };
    }

    /// The parent table name.
    pub struct ParentTable(pub &'static str);

    /// The column name in a referenced table.
    pub struct ReferencedColumn(pub &'static str);

    /// The column name in a parent table.
    pub struct ParentColumn(pub &'static str);

    /// Foreign key `ON DELETE` and `ON UPDATE` clauses are used to configure actions that take place
    /// when deleting rows from the parent table (`ON DELETE`),
    /// or modifying the parent key values of existing rows (`ON UPDATE`).
    /// https://www.sqlite.org/foreignkeys.html
    #[derive(Eq, Ord, PartialEq, PartialOrd)]
    pub enum Event {
        OnUpdate,
        OnDelete,
    }

    impl fmt::Display for Event {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Event::OnUpdate => write!(f, "ON UPDATE"),
                Event::OnDelete => write!(f, "ON DELETE"),
            }
        }
    }

    /// The `ON DELETE` and `ON UPDATE` action associated with each foreign key in an SQLite database
    /// is one of `NO ACTION`, `RESTRICT`, `SET NULL`, `SET DEFAULT` or `CASCADE`.
    /// If an action is not explicitly specified, it defaults to `NO ACTION`.
    pub enum Action {
        /// Configuring `NO ACTION` means just that:
        /// when a parent key is modified or deleted from the database, no special action is taken.
        NoAction,
        /// The `RESTRICT` action means that the application is prohibited from deleting (for `ON DELETE` RESTRICT)
        /// or modifying (for `ON UPDATE RESTRICT`) a parent key when there exists one or more child keys mapped to it.
        Restrict,
        /// If the configured action is `SET NULL`, then when a parent key is deleted (for `ON DELETE SET NULL`)
        /// or modified (for `ON UPDATE SET NULL`), the child key columns of all rows in the child table
        /// that mapped to the parent key are set to contain SQL NULL values.
        SetNull,
        /// The `SET DEFAULT` actions are similar to `SET NULL`,
        /// except that each of the child key columns is set to contain the column's default value instead of NULL.
        SetDefault,
        /// A `CASCADE` action propagates the delete or update operation on the parent key to each dependent child key.
        /// For an `ON DELETE CASCADE` action,
        /// this means that each row in the child table that was associated with the deleted parent row is also deleted.
        /// For an `ON UPDATE CASCADE` action,
        /// it means that the values stored in each dependent child key are modified to match the new parent key values.
        Cascade,
    }

    impl fmt::Display for Action {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Action::NoAction => write!(f, "NO ACTION"),
                Action::Restrict => write!(f, "RESTRICT"),
                Action::SetNull => write!(f, "SET NULL"),
                Action::SetDefault => write!(f, "SET DEFAULT"),
                Action::Cascade => write!(f, "CASCADE"),
            }
        }
    }

    /// This type is used to define a FOREIGN KEY constraint on multiple columns.
    /// https://www.w3schools.com/sql/sql_foreignkey.asp
    pub struct ForeignKey {
        parent_table_name: &'static str,
        columns: Vec<(ReferencedColumn, ParentColumn)>,
        on_events: BTreeMap<Event, Action>,
    }

    impl ForeignKey {
        const CONSTRAINT: &'static str = "FOREIGN KEY";

        pub fn new<I>(parent_table_name: ParentTable, columns: I) -> SqlResult<ForeignKey>
        where
            I: IntoIterator<Item = (ReferencedColumn, ParentColumn)>,
        {
            let columns: Vec<_> = columns.into_iter().collect();
            if columns.is_empty() {
                return Err(no_columns_error(Self::CONSTRAINT));
            }

            Ok(ForeignKey {
                parent_table_name: parent_table_name.0,
                columns,
                on_events: BTreeMap::new(),
            })
        }

        pub fn on_event(mut self, event: Event, action: Action) -> ForeignKey {
            self.on_events.insert(event, action);
            self
        }
    }

    impl From<ForeignKey> for SqlConstraint {
        fn from(foreign: ForeignKey) -> Self {
            SqlConstraint::ForeignKey(foreign)
        }
    }

    impl fmt::Display for ForeignKey {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            assert!(
                !self.columns.is_empty(),
                "'{}::new' must check if no columns were passed",
                Self::CONSTRAINT
            );

            // Write "FOREIGN KEY (".
            write!(f, "{} (", Self::CONSTRAINT)?;
            // Write columns of the child table.
            self.columns
                .iter()
                .map(|(referenced_col, _parent_col)| referenced_col.0)
                .write_join(f, ", ")?;

            write!(f, ") REFERENCES {}(", self.parent_table_name)?;
            // Write columns of the parent table.
            self.columns
                .iter()
                .map(|(_referenced_col, parent_col)| parent_col.0)
                .write_join(f, ", ")?;

            write!(f, ")")?;

            if !self.on_events.is_empty() {
                write!(f, " ")?;
                // Write events and corresponding actions.
                self.on_events
                    .iter()
                    .map(|(event, action)| ActionOnEvent { event, action })
                    .write_join(f, " ")?;
            }
            Ok(())
        }
    }

    /// This helper is used to display `BTreeMap<Event, Action>` pairs.
    struct ActionOnEvent<'a> {
        event: &'a Event,
        action: &'a Action,
    }

    impl fmt::Display for ActionOnEvent<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{} {}", self.event, self.action)
        }
    }
}

fn no_columns_error(constraint: &str) -> SqlError {
    let error = format!("SQL {constraint} CONSTRAINT must contain columns");
    SqlError::ToSqlConversionFailure(StringError::from(error).into_boxed())
}

#[cfg(test)]
mod tests {
    use super::foreign_key::*;
    use super::*;
    use crate::foreign_columns;

    macro_rules! test_named_constraint {
        ($test_name:ident, $constraint_ident:ident, $constraint_str:literal) => {
            #[test]
            fn $test_name() {
                let constraint = $constraint_ident::new("id_type_constraint", vec!["id", "type"]).unwrap();
                let actual = constraint.to_string();
                let expected = format!("CONSTRAINT id_type_constraint {} (id, type)", $constraint_str);
                assert_eq!(actual, expected);

                let constraint = $constraint_ident::new("id_constraint", vec!["id"]).unwrap();
                let actual = constraint.to_string();
                let expected = format!("CONSTRAINT id_constraint {} (id)", $constraint_str);
                assert_eq!(actual, expected);

                $constraint_ident::new("a_constraint_name", std::iter::empty())
                    .expect_err("Expected an error on creating SQL UNIQUE CONSTRAINT without columns");
            }
        };
    }

    test_named_constraint!(test_unique_constraint, Unique, "UNIQUE");
    test_named_constraint!(test_primary_key, PrimaryKey, "PRIMARY KEY");

    #[test]
    fn test_foreign_key() {
        let constraint = ForeignKey::new(
            ParentTable("user"),
            foreign_columns!["user_id" => "id", "user_login" => "login"],
        )
        .unwrap();
        let actual = constraint.to_string();
        let expected = "FOREIGN KEY (user_id, user_login) REFERENCES user(id, login)";
        assert_eq!(actual, expected);

        let constraint = ForeignKey::new(ParentTable("client"), foreign_columns!["client_id" => "id"]).unwrap();
        let actual = constraint.to_string();
        let expected = "FOREIGN KEY (client_id) REFERENCES client(id)";
        assert_eq!(actual, expected);

        let constraint = ForeignKey::new(ParentTable("client"), foreign_columns!["client_id" => "id"])
            .unwrap()
            .on_event(Event::OnDelete, Action::Cascade);
        let actual = constraint.to_string();
        let expected = "FOREIGN KEY (client_id) REFERENCES client(id) ON DELETE CASCADE";
        assert_eq!(actual, expected);

        let constraint = ForeignKey::new(ParentTable("client"), foreign_columns!["client_id" => "id"])
            .unwrap()
            .on_event(Event::OnDelete, Action::Cascade)
            .on_event(Event::OnUpdate, Action::Restrict);
        let actual = constraint.to_string();
        let expected = "FOREIGN KEY (client_id) REFERENCES client(id) ON UPDATE RESTRICT ON DELETE CASCADE";
        assert_eq!(actual, expected);

        PrimaryKey::new("a_table", std::iter::empty())
            .expect_err("Expected an error on creating SQL FOREIGN KEY CONSTRAINT without columns");
    }
}
