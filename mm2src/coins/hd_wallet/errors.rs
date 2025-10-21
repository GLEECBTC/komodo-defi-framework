use super::{HDConfirmAddressError, HDWalletStorageError};
use bip32::Error as Bip32Error;
use crypto::trezor::{TrezorError, TrezorProcessingError};
use crypto::{
    Bip32DerPathError, Bip44Chain, CryptoCtxError, HwError, HwProcessingError, StandardHDPathError, XpubError,
};
use derive_more::Display;
use rpc_task::RpcTaskError;

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum AddressDerivingError {
    #[display(fmt = "Coin doesn't support the given BIP44 chain: {chain:?}")]
    InvalidBip44Chain {
        chain: Bip44Chain,
    },
    #[display(fmt = "BIP32 address deriving error: {_0}")]
    Bip32Error(String),
    Internal(String),
}

impl From<InvalidBip44ChainError> for AddressDerivingError {
    fn from(e: InvalidBip44ChainError) -> Self {
        AddressDerivingError::InvalidBip44Chain { chain: e.chain }
    }
}

impl From<Bip32Error> for AddressDerivingError {
    fn from(e: Bip32Error) -> Self {
        AddressDerivingError::Bip32Error(e.to_string())
    }
}

#[derive(Display)]
pub enum NewAddressDerivingError {
    #[display(fmt = "Addresses limit reached. Max number of addresses: {max_addresses_number}")]
    AddressLimitReached { max_addresses_number: u32 },
    #[display(fmt = "Coin doesn't support the given BIP44 chain: {chain:?}")]
    InvalidBip44Chain { chain: Bip44Chain },
    #[display(fmt = "BIP32 address deriving error: {_0}")]
    Bip32Error(String),
    #[display(fmt = "Wallet storage error: {_0}")]
    WalletStorageError(HDWalletStorageError),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<Bip32Error> for NewAddressDerivingError {
    fn from(e: Bip32Error) -> Self {
        NewAddressDerivingError::Bip32Error(e.to_string())
    }
}

impl From<AddressDerivingError> for NewAddressDerivingError {
    fn from(e: AddressDerivingError) -> Self {
        match e {
            AddressDerivingError::InvalidBip44Chain { chain } => NewAddressDerivingError::InvalidBip44Chain { chain },
            AddressDerivingError::Bip32Error(bip32) => NewAddressDerivingError::Bip32Error(bip32),
            AddressDerivingError::Internal(internal) => NewAddressDerivingError::Internal(internal),
        }
    }
}

impl From<InvalidBip44ChainError> for NewAddressDerivingError {
    fn from(e: InvalidBip44ChainError) -> Self {
        NewAddressDerivingError::InvalidBip44Chain { chain: e.chain }
    }
}

impl From<AccountUpdatingError> for NewAddressDerivingError {
    fn from(e: AccountUpdatingError) -> Self {
        match e {
            AccountUpdatingError::AddressLimitReached { max_addresses_number } => {
                NewAddressDerivingError::AddressLimitReached { max_addresses_number }
            },
            AccountUpdatingError::InvalidBip44Chain(e) => NewAddressDerivingError::from(e),
            AccountUpdatingError::WalletStorageError(storage) => NewAddressDerivingError::WalletStorageError(storage),
        }
    }
}

pub enum NewAddressDeriveConfirmError {
    DeriveError(NewAddressDerivingError),
    ConfirmError(HDConfirmAddressError),
}

impl From<HDConfirmAddressError> for NewAddressDeriveConfirmError {
    fn from(e: HDConfirmAddressError) -> Self {
        NewAddressDeriveConfirmError::ConfirmError(e)
    }
}

impl From<NewAddressDerivingError> for NewAddressDeriveConfirmError {
    fn from(e: NewAddressDerivingError) -> Self {
        NewAddressDeriveConfirmError::DeriveError(e)
    }
}

impl From<AccountUpdatingError> for NewAddressDeriveConfirmError {
    fn from(e: AccountUpdatingError) -> Self {
        NewAddressDeriveConfirmError::DeriveError(NewAddressDerivingError::from(e))
    }
}

impl From<InvalidBip44ChainError> for NewAddressDeriveConfirmError {
    fn from(e: InvalidBip44ChainError) -> Self {
        NewAddressDeriveConfirmError::DeriveError(NewAddressDerivingError::from(e))
    }
}

#[derive(Display)]
pub enum NewAccountCreationError {
    #[display(fmt = "Hardware Wallet context is not initialized")]
    HwContextNotInitialized,
    #[display(fmt = "HD wallet is unavailable")]
    HDWalletUnavailable,
    #[display(
        fmt = "Coin doesn't support Trezor hardware wallet. Please consider adding the 'trezor_coin' field to the coins config"
    )]
    CoinDoesntSupportTrezor,
    RpcTaskError(RpcTaskError),
    HardwareWalletError(HwError),
    #[display(fmt = "Accounts limit reached. Max number of accounts: {max_accounts_number}")]
    AccountLimitReached {
        max_accounts_number: u32,
    },
    #[display(fmt = "Error saving HD account to storage: {_0}")]
    ErrorSavingAccountToStorage(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<Bip32DerPathError> for NewAccountCreationError {
    fn from(e: Bip32DerPathError) -> Self {
        NewAccountCreationError::Internal(StandardHDPathError::from(e).to_string())
    }
}

impl From<HDWalletStorageError> for NewAccountCreationError {
    fn from(e: HDWalletStorageError) -> Self {
        match e {
            HDWalletStorageError::ErrorSaving(e) | HDWalletStorageError::ErrorSerializing(e) => {
                NewAccountCreationError::ErrorSavingAccountToStorage(e)
            },
            HDWalletStorageError::HDWalletUnavailable => NewAccountCreationError::HDWalletUnavailable,
            HDWalletStorageError::Internal(internal) => NewAccountCreationError::Internal(internal),
            other => NewAccountCreationError::Internal(other.to_string()),
        }
    }
}

/// Currently, we suppose that ETH/ERC20/QRC20 don't have [`Bip44Chain::Internal`] addresses.
#[derive(Display)]
#[display(fmt = "Coin doesn't support the given BIP44 chain: {chain:?}")]
pub struct InvalidBip44ChainError {
    pub chain: Bip44Chain,
}

#[derive(Display)]
pub enum AccountUpdatingError {
    AddressLimitReached { max_addresses_number: u32 },
    InvalidBip44Chain(InvalidBip44ChainError),
    WalletStorageError(HDWalletStorageError),
}

impl From<InvalidBip44ChainError> for AccountUpdatingError {
    fn from(e: InvalidBip44ChainError) -> Self {
        AccountUpdatingError::InvalidBip44Chain(e)
    }
}

impl From<HDWalletStorageError> for AccountUpdatingError {
    fn from(e: HDWalletStorageError) -> Self {
        AccountUpdatingError::WalletStorageError(e)
    }
}

#[derive(Display)]
pub enum HDWithdrawError {
    UnexpectedFromAddress(String),
    UnknownAccount { account_id: u32 },
    AddressDerivingError(AddressDerivingError),
    InternalError(String),
}

impl From<AddressDerivingError> for HDWithdrawError {
    fn from(e: AddressDerivingError) -> Self {
        HDWithdrawError::AddressDerivingError(e)
    }
}

#[derive(Clone)]
pub enum HDExtractPubkeyError {
    HwContextNotInitialized,
    CoinDoesntSupportTrezor,
    RpcTaskError(RpcTaskError),
    HardwareWalletError(HwError),
    InvalidXpub(String),
    Internal(String),
}

impl From<CryptoCtxError> for HDExtractPubkeyError {
    fn from(e: CryptoCtxError) -> Self {
        HDExtractPubkeyError::Internal(e.to_string())
    }
}

impl From<TrezorError> for HDExtractPubkeyError {
    fn from(e: TrezorError) -> Self {
        HDExtractPubkeyError::HardwareWalletError(HwError::from(e))
    }
}

impl From<HwError> for HDExtractPubkeyError {
    fn from(e: HwError) -> Self {
        HDExtractPubkeyError::HardwareWalletError(e)
    }
}

impl From<TrezorProcessingError<RpcTaskError>> for HDExtractPubkeyError {
    fn from(e: TrezorProcessingError<RpcTaskError>) -> Self {
        match e {
            TrezorProcessingError::TrezorError(trezor) => HDExtractPubkeyError::from(HwError::from(trezor)),
            TrezorProcessingError::ProcessorError(rpc) => HDExtractPubkeyError::RpcTaskError(rpc),
        }
    }
}

impl From<HwProcessingError<RpcTaskError>> for HDExtractPubkeyError {
    fn from(e: HwProcessingError<RpcTaskError>) -> Self {
        match e {
            HwProcessingError::HwError(hw) => HDExtractPubkeyError::from(hw),
            HwProcessingError::ProcessorError(rpc) => HDExtractPubkeyError::RpcTaskError(rpc),
            HwProcessingError::InternalError(internal) => HDExtractPubkeyError::Internal(internal),
        }
    }
}

impl From<XpubError> for HDExtractPubkeyError {
    fn from(e: XpubError) -> Self {
        HDExtractPubkeyError::InvalidXpub(e.to_string())
    }
}

impl From<HDExtractPubkeyError> for NewAccountCreationError {
    fn from(e: HDExtractPubkeyError) -> Self {
        match e {
            HDExtractPubkeyError::HwContextNotInitialized => NewAccountCreationError::HwContextNotInitialized,
            HDExtractPubkeyError::CoinDoesntSupportTrezor => NewAccountCreationError::CoinDoesntSupportTrezor,
            HDExtractPubkeyError::RpcTaskError(rpc) => NewAccountCreationError::RpcTaskError(rpc),
            HDExtractPubkeyError::HardwareWalletError(hw) => NewAccountCreationError::HardwareWalletError(hw),
            HDExtractPubkeyError::InvalidXpub(xpub) => {
                NewAccountCreationError::HardwareWalletError(HwError::InvalidXpub(xpub))
            },
            HDExtractPubkeyError::Internal(internal) => NewAccountCreationError::Internal(internal),
        }
    }
}

#[derive(Display)]
pub enum TrezorCoinError {
    Internal(String),
}

impl From<TrezorCoinError> for HDExtractPubkeyError {
    fn from(e: TrezorCoinError) -> Self {
        HDExtractPubkeyError::Internal(e.to_string())
    }
}

impl From<TrezorCoinError> for NewAddressDeriveConfirmError {
    fn from(e: TrezorCoinError) -> Self {
        NewAddressDeriveConfirmError::DeriveError(NewAddressDerivingError::Internal(e.to_string()))
    }
}

#[derive(Display)]
pub enum SettingEnabledAddressError {
    Internal(String),
}
