pub(crate) use crate::eth::eth_swap_v2::PrepareTxDataError;
use derive_more::Display;
use enum_derives::EnumFromStringify;

#[derive(Debug, Display)]
pub(crate) enum Erc721FunctionError {
    #[display(fmt = "ABI error: {_0}")]
    ABIError(String),
    FunctionNotFound(String),
}

#[derive(Debug, Display, EnumFromStringify)]
pub(crate) enum HtlcParamsError {
    WrongPaymentTx(String),
    #[from_stringify("ethabi::Error")]
    #[display(fmt = "ABI error: {_0}")]
    ABIError(String),
    InvalidData(String),
}

impl From<Erc721FunctionError> for PrepareTxDataError {
    fn from(e: Erc721FunctionError) -> Self {
        match e {
            Erc721FunctionError::ABIError(e) => PrepareTxDataError::ABIError(e),
            Erc721FunctionError::FunctionNotFound(e) => PrepareTxDataError::Internal(e),
        }
    }
}
