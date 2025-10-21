// Generated protocol messages; many types are not instantiated in our build.
#![allow(dead_code)]
///*
/// Response: Success of the previous request
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Success {
    /// human readable description of action or request-specific payload
    #[prost(string, optional, tag = "1", default = "")]
    pub message: ::core::option::Option<::prost::alloc::string::String>,
}
///*
/// Response: Failure of the previous request
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Failure {
    /// computer-readable definition of the error state
    #[prost(enumeration = "failure::FailureType", optional, tag = "1")]
    pub code: ::core::option::Option<i32>,
    /// human-readable message of the error state
    #[prost(string, optional, tag = "2")]
    pub message: ::core::option::Option<::prost::alloc::string::String>,
}
/// Nested message and enum types in `Failure`.
pub mod failure {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    #[allow(clippy::enum_variant_names)]
    pub enum FailureType {
        FailureUnexpectedMessage = 1,
        FailureButtonExpected = 2,
        FailureDataError = 3,
        FailureActionCancelled = 4,
        FailurePinExpected = 5,
        FailurePinCancelled = 6,
        FailurePinInvalid = 7,
        FailureInvalidSignature = 8,
        FailureProcessError = 9,
        FailureNotEnoughFunds = 10,
        FailureNotInitialized = 11,
        FailurePinMismatch = 12,
        FailureWipeCodeMismatch = 13,
        FailureInvalidSession = 14,
        FailureFirmwareError = 99,
    }
}
///*
/// Response: Device is waiting for HW button press.
/// @auxstart
/// @next ButtonAck
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ButtonRequest {
    /// enum identifier of the screen
    #[prost(enumeration = "button_request::ButtonRequestType", optional, tag = "1")]
    pub code: ::core::option::Option<i32>,
    /// if the screen is paginated, number of pages
    #[prost(uint32, optional, tag = "2")]
    pub pages: ::core::option::Option<u32>,
}
/// Nested message and enum types in `ButtonRequest`.
pub mod button_request {
    ///*
    /// Type of button request
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum ButtonRequestType {
        ButtonRequestOther = 1,
        ButtonRequestFeeOverThreshold = 2,
        ButtonRequestConfirmOutput = 3,
        ButtonRequestResetDevice = 4,
        ButtonRequestConfirmWord = 5,
        ButtonRequestWipeDevice = 6,
        ButtonRequestProtectCall = 7,
        ButtonRequestSignTx = 8,
        ButtonRequestFirmwareCheck = 9,
        ButtonRequestAddress = 10,
        ButtonRequestPublicKey = 11,
        ButtonRequestMnemonicWordCount = 12,
        ButtonRequestMnemonicInput = 13,
        DeprecatedButtonRequestPassphraseType = 14,
        ButtonRequestUnknownDerivationPath = 15,
        ButtonRequestRecoveryHomepage = 16,
        ButtonRequestSuccess = 17,
        ButtonRequestWarning = 18,
        ButtonRequestPassphraseEntry = 19,
        ButtonRequestPinEntry = 20,
    }
}
///*
/// Request: Computer agrees to wait for HW button press
/// @auxend
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ButtonAck {}
///*
/// Response: Device is asking computer to show PIN matrix and awaits PIN encoded using this matrix scheme
/// @auxstart
/// @next PinMatrixAck
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PinMatrixRequest {
    #[prost(enumeration = "pin_matrix_request::PinMatrixRequestType", optional, tag = "1")]
    pub r#type: ::core::option::Option<i32>,
}
/// Nested message and enum types in `PinMatrixRequest`.
pub mod pin_matrix_request {
    ///*
    /// Type of PIN request
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum PinMatrixRequestType {
        Current = 1,
        NewFirst = 2,
        NewSecond = 3,
        WipeCodeFirst = 4,
        WipeCodeSecond = 5,
    }
}
///*
/// Request: Computer responds with encoded PIN
/// @auxend
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PinMatrixAck {
    /// matrix encoded PIN entered by user
    #[prost(string, required, tag = "1")]
    pub pin: ::prost::alloc::string::String,
}
///*
/// Response: Device awaits encryption passphrase
/// @auxstart
/// @next PassphraseAck
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PassphraseRequest {
    /// <2.3.0
    #[deprecated]
    #[prost(bool, optional, tag = "1")]
    pub on_device: ::core::option::Option<bool>,
}
///*
/// Request: Send passphrase back
/// @auxend
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PassphraseAck {
    #[prost(string, optional, tag = "1")]
    pub passphrase: ::core::option::Option<::prost::alloc::string::String>,
    /// <2.3.0
    #[deprecated]
    #[prost(bytes = "vec", optional, tag = "2")]
    pub state: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// user wants to enter passphrase on the device
    #[prost(bool, optional, tag = "3")]
    pub on_device: ::core::option::Option<bool>,
}
///*
/// Response: Device awaits passphrase state
/// Deprecated in 2.3.0
/// @next Deprecated_PassphraseStateAck
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DeprecatedPassphraseStateRequest {
    /// actual device state
    #[prost(bytes = "vec", optional, tag = "1")]
    pub state: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
}
///*
/// Request: Send passphrase state back
/// Deprecated in 2.3.0
/// @auxend
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DeprecatedPassphraseStateAck {}
///*
/// Structure representing BIP32 (hierarchical deterministic) node
/// Used for imports of private key into the device and exporting public key out of device
/// @embed
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct HdNodeType {
    #[prost(uint32, required, tag = "1")]
    pub depth: u32,
    #[prost(uint32, required, tag = "2")]
    pub fingerprint: u32,
    #[prost(uint32, required, tag = "3")]
    pub child_num: u32,
    #[prost(bytes = "vec", required, tag = "4")]
    pub chain_code: ::prost::alloc::vec::Vec<u8>,
    #[prost(bytes = "vec", optional, tag = "5")]
    pub private_key: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    #[prost(bytes = "vec", required, tag = "6")]
    pub public_key: ::prost::alloc::vec::Vec<u8>,
}
