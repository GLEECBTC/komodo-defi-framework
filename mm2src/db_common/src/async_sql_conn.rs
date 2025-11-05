use crate::sqlite::rusqlite::Error as SqlError;
use crossbeam_channel::Sender;
use futures::channel::oneshot::{self};
use rusqlite::OpenFlags;
use std::fmt::{self, Debug, Display};
use std::path::Path;
use std::thread;

/// Represents the errors specific for AsyncConnection.
#[derive(Debug)]
pub enum AsyncConnError {
    /// The connection to the SQLite has been closed and cannot be queried anymore.
    ConnectionClosed,
    /// An error occurred while closing the SQLite connection.
    /// This `Error` variant contains the [`AsyncConnection`], which can be used to retry the close operation
    /// and the underlying [`SqlError`] that made it impossible to close the database.
    Close((AsyncConnection, SqlError)),
    /// A `Rusqlite` error occurred.
    Rusqlite(SqlError),
    /// An application-specific error occurred.
    Internal(InternalError),
}

impl Display for AsyncConnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AsyncConnError::ConnectionClosed => write!(f, "ConnectionClosed"),
            AsyncConnError::Close((_, e)) => write!(f, "Close((AsyncConnection, \"{e}\"))"),
            AsyncConnError::Rusqlite(e) => write!(f, "Rusqlite(\"{e}\")"),
            AsyncConnError::Internal(e) => write!(f, "Internal(\"{e}\")"),
        }
    }
}

impl std::error::Error for AsyncConnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AsyncConnError::ConnectionClosed => None,
            AsyncConnError::Close((_, e)) => Some(e),
            AsyncConnError::Rusqlite(e) => Some(e),
            AsyncConnError::Internal(e) => Some(e),
        }
    }
}

impl From<String> for AsyncConnError {
    fn from(err: String) -> Self {
        Self::Internal(InternalError(err))
    }
}

#[derive(Debug)]
pub struct InternalError(pub String);

impl fmt::Display for InternalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for InternalError {}

impl From<SqlError> for AsyncConnError {
    fn from(value: SqlError) -> Self {
        AsyncConnError::Rusqlite(value)
    }
}

/// The result returned on method calls in this crate.
pub type Result<T> = std::result::Result<T, AsyncConnError>;

type CallFn = Box<dyn FnOnce(&mut rusqlite::Connection) + Send + 'static>;

enum Message {
    Execute(CallFn),
    Close(oneshot::Sender<std::result::Result<(), SqlError>>),
}

/// A handle to call functions in background thread.
#[derive(Clone)]
pub struct AsyncConnection {
    sender: Sender<Message>,
}

impl AsyncConnection {
    /// Open a new connection to a SQLite database.
    ///
    /// `AsyncConnection::open(path)` is equivalent to
    /// `AsyncConnection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE |
    /// OpenFlags::SQLITE_OPEN_CREATE)`.
    ///
    /// # Failure
    ///
    /// Will return `Err` if `path` cannot be converted to a C-compatible
    /// string or if the underlying SQLite open call fails.
    pub async fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_owned();
        start(move || rusqlite::Connection::open(path)).await
    }

    /// Open a new AsyncConnection to an in-memory SQLite database.
    ///
    /// # Failure
    ///
    /// Will return `Err` if the underlying SQLite open call fails.
    pub async fn open_in_memory() -> Result<Self> {
        start(rusqlite::Connection::open_in_memory).await
    }

    /// Open a new AsyncConnection to a SQLite database.
    ///
    /// [Database Connection](http://www.sqlite.org/c3ref/open.html) for a
    /// description of valid flag combinations.
    ///
    /// # Failure
    ///
    /// Will return `Err` if `path` cannot be converted to a C-compatible
    /// string or if the underlying SQLite open call fails.
    pub async fn open_with_flags<P: AsRef<Path>>(path: P, flags: OpenFlags) -> Result<Self> {
        let path = path.as_ref().to_owned();
        start(move || rusqlite::Connection::open_with_flags(path, flags)).await
    }

    /// Open a new AsyncConnection to a SQLite database using the specific flags
    /// and vfs name.
    ///
    /// [Database Connection](http://www.sqlite.org/c3ref/open.html) for a
    /// description of valid flag combinations.
    ///
    /// # Failure
    ///
    /// Will return `Err` if either `path` or `vfs` cannot be converted to a
    /// C-compatible string or if the underlying SQLite open call fails.
    pub async fn open_with_flags_and_vfs<P: AsRef<Path>>(path: P, flags: OpenFlags, vfs: &str) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let vfs = vfs.to_owned();
        start(move || rusqlite::Connection::open_with_flags_and_vfs(path, flags, &vfs)).await
    }

    /// Open a new AsyncConnection to an in-memory SQLite database.
    ///
    /// [Database Connection](http://www.sqlite.org/c3ref/open.html) for a
    /// description of valid flag combinations.
    ///
    /// # Failure
    ///
    /// Will return `Err` if the underlying SQLite open call fails.
    pub async fn open_in_memory_with_flags(flags: OpenFlags) -> Result<Self> {
        start(move || rusqlite::Connection::open_in_memory_with_flags(flags)).await
    }

    /// Open a new connection to an in-memory SQLite database using the
    /// specific flags and vfs name.
    ///
    /// [Database Connection](http://www.sqlite.org/c3ref/open.html) for a
    /// description of valid flag combinations.
    ///
    /// # Failure
    ///
    /// Will return `Err` if `vfs` cannot be converted to a C-compatible
    /// string or if the underlying SQLite open call fails.
    pub async fn open_in_memory_with_flags_and_vfs(flags: OpenFlags, vfs: &str) -> Result<Self> {
        let vfs = vfs.to_owned();
        start(move || rusqlite::Connection::open_in_memory_with_flags_and_vfs(flags, &vfs)).await
    }

    /// Call a function in background thread and get the result asynchronously.
    ///
    /// # Failure
    ///
    /// Will return `Err` if the database connection has been closed.
    pub async fn call<F, R>(&self, function: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<R> + 'static + Send,
        R: Send + 'static,
    {
        let (sender, receiver) = oneshot::channel::<Result<R>>();

        self.sender
            .send(Message::Execute(Box::new(move |conn| {
                let value = function(conn);
                let _ = sender.send(value);
            })))
            .map_err(|_| AsyncConnError::ConnectionClosed)?;

        receiver.await.map_err(|_| AsyncConnError::ConnectionClosed)?
    }

    /// Call a function in background thread and get the result asynchronously.
    ///
    /// This method can cause a `panic` if the underlying database connection is closed.
    /// it is a more user-friendly alternative to the [`AsyncConnection::call`] method.
    /// It should be safe if the connection is never explicitly closed (using the [`AsyncConnection::close`] call).
    ///
    /// Calling this on a closed connection will cause a `panic`.
    pub async fn call_unwrap<F, R>(&self, function: F) -> R
    where
        F: FnOnce(&mut rusqlite::Connection) -> R + Send + 'static,
        R: Send + 'static,
    {
        let (sender, receiver) = oneshot::channel::<R>();

        self.sender
            .send(Message::Execute(Box::new(move |conn| {
                let value = function(conn);
                let _ = sender.send(value);
            })))
            .expect("database connection should be open");

        receiver.await.expect("Bug occurred, please report")
    }

    /// Close the database AsyncConnection.
    ///
    /// This is functionally equivalent to the `Drop` implementation for
    /// `AsyncConnection`. It consumes the `AsyncConnection`, but on error returns it
    /// to the caller for retry purposes.
    ///
    /// If successful, any following `close` operations performed
    /// on `AsyncConnection` copies will succeed immediately.
    ///
    /// On the other hand, any calls to [`AsyncConnection::call`] will return a [`AsyncConnError::ConnectionClosed`],
    /// and any calls to [`AsyncConnection::call_unwrap`] will cause a `panic`.
    ///
    /// # Failure
    ///
    /// Will return `Err` if the underlying SQLite close call fails.
    pub async fn close(&mut self) -> Result<()> {
        let (sender, receiver) = oneshot::channel::<std::result::Result<(), SqlError>>();

        if let Err(crossbeam_channel::SendError(_)) = self.sender.send(Message::Close(sender)) {
            // If the channel is closed on the other side, it means the connection closed successfully
            // This is a safeguard against calling close on a `Copy` of the connection
            return Ok(());
        }

        let result = receiver.await;

        if result.is_err() {
            // If we get a RecvError at this point, it also means the channel closed in the meantime
            // we can assume the connection is closed
            return Ok(());
        }

        result.unwrap().map_err(|e| AsyncConnError::Close((self.clone(), e)))
    }
}

impl Debug for AsyncConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsyncConnection").finish()
    }
}

async fn start<F>(open: F) -> Result<AsyncConnection>
where
    F: FnOnce() -> rusqlite::Result<rusqlite::Connection> + Send + 'static,
{
    let (sender, receiver) = crossbeam_channel::unbounded::<Message>();
    let (result_sender, result_receiver) = oneshot::channel();

    thread::spawn(move || {
        let mut conn = match open() {
            Ok(c) => c,
            Err(e) => {
                let _ = result_sender.send(Err(e));
                return;
            },
        };

        if let Err(_e) = result_sender.send(Ok(())) {
            return;
        }

        while let Ok(message) = receiver.recv() {
            match message {
                Message::Execute(f) => f(&mut conn),
                Message::Close(s) => {
                    let result = conn.close();

                    match result {
                        Ok(v) => {
                            if s.send(Ok(v)).is_err() {
                                // terminate the thread
                                return;
                            }
                            break;
                        },
                        Err((c, e)) => {
                            conn = c;
                            if s.send(Err(e)).is_err() {
                                // terminate the thread
                                return;
                            }
                        },
                    }
                },
            }
        }
    });

    result_receiver
        .await
        .map_err(|e| AsyncConnError::Internal(InternalError(e.to_string())))
        .map(|_| AsyncConnection { sender })
}
