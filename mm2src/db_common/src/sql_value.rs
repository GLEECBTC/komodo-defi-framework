use std::fmt;

pub trait FromQuoted<T> {
    fn from_quoted(t: T) -> Self;
}

/// A valid SQL value that can be passed as an argument to the `SqlQuery`, `SqlCreate`, `SqlInsert` safely.
///
/// Please note that any static string passed into [`SqlValue::from`] will not be quoted.
/// To quote the string use [`SqlValue::from_quoted`].
pub enum SqlValue {
    String(&'static str),
    StringQuoted(&'static str),
    Integer(i64),
    Real(f64),
}

impl fmt::Display for SqlValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlValue::String(string) => write!(f, "{string}"),
            SqlValue::StringQuoted(string) => write!(f, "'{string}'"),
            SqlValue::Integer(decimal) => write!(f, "{decimal}"),
            SqlValue::Real(real) => write!(f, "{real}"),
        }
    }
}

impl From<&'static str> for SqlValue {
    fn from(string: &'static str) -> Self {
        SqlValue::String(string)
    }
}

impl FromQuoted<&'static str> for SqlValue {
    fn from_quoted(string: &'static str) -> Self {
        SqlValue::StringQuoted(string)
    }
}

impl From<i64> for SqlValue {
    fn from(decimal: i64) -> Self {
        SqlValue::Integer(decimal)
    }
}

impl From<f64> for SqlValue {
    fn from(real: f64) -> Self {
        SqlValue::Real(real)
    }
}

/// A valid SQL optional value that can be passed as an argument to the `SqlQuery`, `SqlCreate`, `SqlInsert` safely.
///
/// Please note that any static string passed into [`SqlValueOptional::from`] will not be quoted.
/// To quote the string use [`SqlValueOptional::from_quoted`].
pub enum SqlValueOptional {
    Some(SqlValue),
    Null,
}

impl fmt::Display for SqlValueOptional {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlValueOptional::Some(value) => write!(f, "{value}"),
            SqlValueOptional::Null => write!(f, "NULL"),
        }
    }
}

impl<T> From<T> for SqlValueOptional
where
    SqlValue: From<T>,
{
    fn from(value: T) -> Self {
        SqlValueOptional::Some(SqlValue::from(value))
    }
}

impl<T> From<Option<T>> for SqlValueOptional
where
    SqlValue: From<T>,
{
    fn from(opt: Option<T>) -> Self {
        match opt {
            Some(v) => SqlValueOptional::Some(SqlValue::from(v)),
            None => SqlValueOptional::Null,
        }
    }
}

impl<T> FromQuoted<T> for SqlValueOptional
where
    SqlValue: FromQuoted<T>,
{
    fn from_quoted(value: T) -> Self {
        SqlValueOptional::Some(SqlValue::from_quoted(value))
    }
}

impl<T> FromQuoted<Option<T>> for SqlValueOptional
where
    SqlValue: FromQuoted<T>,
{
    fn from_quoted(opt: Option<T>) -> Self {
        match opt {
            Some(v) => SqlValueOptional::Some(SqlValue::from_quoted(v)),
            None => SqlValueOptional::Null,
        }
    }
}

pub(crate) trait SqlValueToString: fmt::Display + Sized {
    /// Converts the given `value` to string if it implements `Into<SqlValue>`.
    /// The resulting string is considered a safe SQL value.
    fn value_to_string<S>(value: S) -> String
    where
        Self: From<S>,
    {
        Self::from(value).to_string()
    }

    /// Converts the given `value` to string if `Self` implements `FromQuoted<S>`.
    /// The resulting string is considered a safe SQL value.
    fn quoted_value_to_string<S>(value: S) -> String
    where
        Self: FromQuoted<S>,
    {
        Self::from_quoted(value).to_string()
    }
}

impl SqlValueToString for SqlValue {}

impl SqlValueToString for SqlValueOptional {}

pub(crate) trait SqlValuesToStrings: SqlValueToString {
    /// Converts the given `values` to `Vec<String>` if they implement `Into<SqlValue>`.
    /// The resulting strings are considered safe SQL values.
    fn values_to_strings<I, S>(values: I) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        Self: From<S>,
    {
        values.into_iter().map(Self::value_to_string).collect()
    }

    /// Converts the given `values` to `Vec<String>` if `Self` implements `FromQuoted<S>`.
    /// The resulting strings are considered safe SQL values.
    fn quoted_values_to_strings<I, S>(values: I) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        Self: FromQuoted<S>,
    {
        values.into_iter().map(Self::quoted_value_to_string).collect()
    }
}

impl SqlValuesToStrings for SqlValue {}

impl SqlValuesToStrings for SqlValueOptional {}
