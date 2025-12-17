use crate::async_sql_conn::{AsyncConnError, AsyncConnection, InternalError, Result as AsyncConnResult};
use rusqlite::{ffi, ErrorCode};
use std::fmt::Display;

#[tokio::test]
async fn open_in_memory_test() -> AsyncConnResult<()> {
    let conn = AsyncConnection::open_in_memory().await;
    assert!(conn.is_ok());
    Ok(())
}

#[tokio::test]
async fn call_success_test() -> AsyncConnResult<()> {
    let conn = AsyncConnection::open_in_memory().await?;

    let result = conn
        .call(|conn| {
            conn.execute(
                "CREATE TABLE person(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL);",
                [],
            )
            .map_err(|e| e.into())
        })
        .await;

    assert_eq!(0, result.unwrap());

    Ok(())
}

#[tokio::test]
async fn call_unwrap_success_test() -> AsyncConnResult<()> {
    let conn = AsyncConnection::open_in_memory().await?;

    let result = conn
        .call_unwrap(|conn| {
            conn.execute(
                "CREATE TABLE person(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL);",
                [],
            )
            .unwrap()
        })
        .await;

    assert_eq!(0, result);

    Ok(())
}

#[tokio::test]
async fn call_failure_test() -> AsyncConnResult<()> {
    let conn = AsyncConnection::open_in_memory().await?;

    let result = conn
        .call(|conn| conn.execute("Invalid sql", []).map_err(|e| e.into()))
        .await;

    assert!(match result.unwrap_err() {
        AsyncConnError::Rusqlite(e) => {
            e == rusqlite::Error::SqlInputError {
                error: ffi::Error {
                    code: ErrorCode::Unknown,
                    extended_code: 1,
                },
                msg: "near \"Invalid\": syntax error".to_string(),
                sql: "Invalid sql".to_string(),
                offset: 0,
            }
        },
        _ => false,
    });

    Ok(())
}

#[tokio::test]
async fn close_success_test() -> AsyncConnResult<()> {
    let mut conn = AsyncConnection::open_in_memory().await?;

    assert!(conn.close().await.is_ok());

    Ok(())
}

#[tokio::test]
async fn double_close_test() -> AsyncConnResult<()> {
    let mut conn = AsyncConnection::open_in_memory().await?;

    let mut conn2 = conn.clone();

    assert!(conn.close().await.is_ok());
    assert!(conn2.close().await.is_ok());

    Ok(())
}

#[tokio::test]
async fn close_call_test() -> AsyncConnResult<()> {
    let mut conn = AsyncConnection::open_in_memory().await?;

    let conn2 = conn.clone();

    assert!(conn.close().await.is_ok());

    let result = conn2
        .call(|conn| conn.execute("SELECT 1;", []).map_err(|e| e.into()))
        .await;

    assert!(matches!(result.unwrap_err(), AsyncConnError::ConnectionClosed));

    Ok(())
}

#[tokio::test]
#[should_panic]
async fn close_call_unwrap_test() {
    let mut conn = AsyncConnection::open_in_memory().await.unwrap();

    let conn2 = conn.clone();

    assert!(conn.close().await.is_ok());

    conn2.call_unwrap(|conn| conn.execute("SELECT 1;", [])).await.unwrap();
}

#[tokio::test]
async fn close_failure_test() -> AsyncConnResult<()> {
    let mut conn = AsyncConnection::open_in_memory().await?;

    conn.call(|conn| {
        conn.execute(
            "CREATE TABLE person(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL);",
            [],
        )
        .map_err(|e| e.into())
    })
    .await?;

    conn.call(|conn| {
        // Leak a prepared statement to make the database uncloseable
        // See https://www.sqlite.org/c3ref/close.html for details regarding this behaviour
        let stmt = Box::new(conn.prepare("INSERT INTO person VALUES (1, ?1);").unwrap());
        Box::leak(stmt);
        Ok(())
    })
    .await?;

    assert!(match conn.close().await.unwrap_err() {
        AsyncConnError::Close((_, e)) => {
            e == rusqlite::Error::SqliteFailure(
                ffi::Error {
                    code: ErrorCode::DatabaseBusy,
                    extended_code: 5,
                },
                Some("unable to close due to unfinalized statements or unfinished backups".to_string()),
            )
        },
        _ => false,
    });

    Ok(())
}

#[tokio::test]
async fn debug_format_test() -> AsyncConnResult<()> {
    let conn = AsyncConnection::open_in_memory().await?;

    assert_eq!("AsyncConnection".to_string(), format!("{conn:?}"));

    Ok(())
}

#[tokio::test]
async fn test_error_display() -> AsyncConnResult<()> {
    let conn = AsyncConnection::open_in_memory().await?;

    let error = AsyncConnError::Close((conn, rusqlite::Error::InvalidQuery));
    assert_eq!(
        "Close((AsyncConnection, \"Query is not read-only\"))",
        format!("{error}")
    );

    let error = AsyncConnError::ConnectionClosed;
    assert_eq!("ConnectionClosed", format!("{error}"));

    let error = AsyncConnError::Rusqlite(rusqlite::Error::InvalidQuery);
    assert_eq!("Rusqlite(\"Query is not read-only\")", format!("{error}"));

    Ok(())
}

#[tokio::test]
async fn test_error_source() -> AsyncConnResult<()> {
    let conn = AsyncConnection::open_in_memory().await?;

    let error = AsyncConnError::Close((conn, rusqlite::Error::InvalidQuery));
    assert_eq!(
        std::error::Error::source(&error)
            .and_then(|e| e.downcast_ref::<rusqlite::Error>())
            .unwrap(),
        &rusqlite::Error::InvalidQuery,
    );

    let error = AsyncConnError::ConnectionClosed;
    assert_eq!(
        std::error::Error::source(&error).and_then(|e| e.downcast_ref::<rusqlite::Error>()),
        None,
    );

    let error = AsyncConnError::Rusqlite(rusqlite::Error::InvalidQuery);
    assert_eq!(
        std::error::Error::source(&error)
            .and_then(|e| e.downcast_ref::<rusqlite::Error>())
            .unwrap(),
        &rusqlite::Error::InvalidQuery,
    );

    Ok(())
}

fn failable_func(_: &rusqlite::Connection) -> std::result::Result<(), MyError> {
    Err(MyError::MySpecificError)
}

#[tokio::test]
async fn test_ergonomic_errors() -> AsyncConnResult<()> {
    let conn = AsyncConnection::open_in_memory().await?;

    let res = conn
        .call(|conn| failable_func(conn).map_err(|e| AsyncConnError::Internal(InternalError(e.to_string()))))
        .await
        .unwrap_err();

    let err = std::error::Error::source(&res)
        .and_then(|e| e.downcast_ref::<InternalError>())
        .unwrap()
        .to_string();

    assert_eq!(err, MyError::MySpecificError.to_string());

    Ok(())
}

#[derive(Debug)]
enum MyError {
    MySpecificError,
}

impl Display for MyError {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

impl std::error::Error for MyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}
