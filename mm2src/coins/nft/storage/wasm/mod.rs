use crate::nft::storage::NftStorageError;
use derive_more::Display;
use mm2_db::indexed_db::{DbTransactionError, InitDbError};
use mm2_err_handle::prelude::*;

pub(crate) mod nft_idb;
pub(crate) mod wasm_storage;

pub type WasmNftCacheResult<T> = MmResult<T, WasmNftCacheError>;

impl NftStorageError for WasmNftCacheError {}

#[derive(Debug, Display)]
pub enum WasmNftCacheError {
    ErrorSerializing(String),
    ErrorDeserializing(String),
    ErrorSaving(String),
    ErrorLoading(String),
    ErrorClearing(String),
    NotSupported(String),
    InternalError(String),
    GetLastNftBlockError(String),
    GetItemError(String),
    CursorBuilderError(String),
    OpenCursorError(String),
}

impl From<InitDbError> for WasmNftCacheError {
    fn from(e: InitDbError) -> Self {
        match &e {
            InitDbError::NotSupported(_) => WasmNftCacheError::NotSupported(e.to_string()),
            InitDbError::EmptyTableList
            | InitDbError::DbIsOpenAlready { .. }
            | InitDbError::InvalidVersion(_)
            | InitDbError::OpeningError(_)
            | InitDbError::TypeMismatch { .. }
            | InitDbError::UnexpectedState(_)
            | InitDbError::UpgradingError { .. } => WasmNftCacheError::InternalError(e.to_string()),
        }
    }
}

impl From<DbTransactionError> for WasmNftCacheError {
    fn from(e: DbTransactionError) -> Self {
        match e {
            DbTransactionError::ErrorSerializingItem(_) => WasmNftCacheError::ErrorSerializing(e.to_string()),
            DbTransactionError::ErrorDeserializingItem(_) => WasmNftCacheError::ErrorDeserializing(e.to_string()),
            DbTransactionError::ErrorUploadingItem(_) => WasmNftCacheError::ErrorSaving(e.to_string()),
            DbTransactionError::ErrorGettingItems(_) | DbTransactionError::ErrorCountingItems(_) => {
                WasmNftCacheError::ErrorLoading(e.to_string())
            },
            DbTransactionError::ErrorDeletingItems(_) => WasmNftCacheError::ErrorClearing(e.to_string()),
            DbTransactionError::NoSuchTable { .. }
            | DbTransactionError::ErrorCreatingTransaction(_)
            | DbTransactionError::ErrorOpeningTable { .. }
            | DbTransactionError::ErrorSerializingIndex { .. }
            | DbTransactionError::UnexpectedState(_)
            | DbTransactionError::TransactionAborted
            | DbTransactionError::MultipleItemsByUniqueIndex { .. }
            | DbTransactionError::NoSuchIndex { .. }
            | DbTransactionError::InvalidIndex { .. } => WasmNftCacheError::InternalError(e.to_string()),
        }
    }
}
