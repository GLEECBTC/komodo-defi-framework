// Generated protocol messages; many types are not instantiated in our build.
#![allow(dead_code)]
///*
/// Request: Reset device to default state and ask for device details
/// @start
/// @next Features
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Initialize {
    /// assumed device session id; Trezor clears caches if it is different or empty
    #[prost(bytes = "vec", optional, tag = "1")]
    pub session_id: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
}
///*
/// Request: Ask for device details (no device reset)
/// @start
/// @next Features
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetFeatures {}
///*
/// Response: Reports various information about the device
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Features {
    /// name of the manufacturer, e.g. "trezor.io"
    #[prost(string, optional, tag = "1")]
    pub vendor: ::core::option::Option<::prost::alloc::string::String>,
    /// major version of the firmware/bootloader, e.g. 1
    #[prost(uint32, required, tag = "2")]
    pub major_version: u32,
    /// minor version of the firmware/bootloader, e.g. 0
    #[prost(uint32, required, tag = "3")]
    pub minor_version: u32,
    /// patch version of the firmware/bootloader, e.g. 0
    #[prost(uint32, required, tag = "4")]
    pub patch_version: u32,
    /// is device in bootloader mode?
    #[prost(bool, optional, tag = "5")]
    pub bootloader_mode: ::core::option::Option<bool>,
    /// device's unique identifier
    #[prost(string, optional, tag = "6")]
    pub device_id: ::core::option::Option<::prost::alloc::string::String>,
    /// is device protected by PIN?
    #[prost(bool, optional, tag = "7")]
    pub pin_protection: ::core::option::Option<bool>,
    /// is node/mnemonic encrypted using passphrase?
    #[prost(bool, optional, tag = "8")]
    pub passphrase_protection: ::core::option::Option<bool>,
    /// device language
    #[prost(string, optional, tag = "9")]
    pub language: ::core::option::Option<::prost::alloc::string::String>,
    /// device description label
    #[prost(string, optional, tag = "10")]
    pub label: ::core::option::Option<::prost::alloc::string::String>,
    /// does device contain seed?
    #[prost(bool, optional, tag = "12")]
    pub initialized: ::core::option::Option<bool>,
    /// SCM revision of firmware
    #[prost(bytes = "vec", optional, tag = "13")]
    pub revision: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// hash of the bootloader
    #[prost(bytes = "vec", optional, tag = "14")]
    pub bootloader_hash: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// was storage imported from an external source?
    #[prost(bool, optional, tag = "15")]
    pub imported: ::core::option::Option<bool>,
    /// is the device unlocked? called "pin_cached" previously
    #[prost(bool, optional, tag = "16")]
    pub unlocked: ::core::option::Option<bool>,
    ///    optional bool passphrase_cached = 17;       // is passphrase already cached in session?  DEPRECATED
    ///
    /// is valid firmware loaded?
    #[prost(bool, optional, tag = "18")]
    pub firmware_present: ::core::option::Option<bool>,
    /// does storage need backup? (equals to Storage.needs_backup)
    #[prost(bool, optional, tag = "19")]
    pub needs_backup: ::core::option::Option<bool>,
    /// device flags (equals to Storage.flags)
    #[prost(uint32, optional, tag = "20")]
    pub flags: ::core::option::Option<u32>,
    /// device hardware model
    #[prost(string, optional, tag = "21")]
    pub model: ::core::option::Option<::prost::alloc::string::String>,
    /// reported firmware version if in bootloader mode
    #[prost(uint32, optional, tag = "22")]
    pub fw_major: ::core::option::Option<u32>,
    /// reported firmware version if in bootloader mode
    #[prost(uint32, optional, tag = "23")]
    pub fw_minor: ::core::option::Option<u32>,
    /// reported firmware version if in bootloader mode
    #[prost(uint32, optional, tag = "24")]
    pub fw_patch: ::core::option::Option<u32>,
    /// reported firmware vendor if in bootloader mode
    #[prost(string, optional, tag = "25")]
    pub fw_vendor: ::core::option::Option<::prost::alloc::string::String>,
    /// reported firmware vendor keys (their hash)
    #[prost(bytes = "vec", optional, tag = "26")]
    pub fw_vendor_keys: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// report unfinished backup (equals to Storage.unfinished_backup)
    #[prost(bool, optional, tag = "27")]
    pub unfinished_backup: ::core::option::Option<bool>,
    /// report no backup (equals to Storage.no_backup)
    #[prost(bool, optional, tag = "28")]
    pub no_backup: ::core::option::Option<bool>,
    /// is recovery mode in progress
    #[prost(bool, optional, tag = "29")]
    pub recovery_mode: ::core::option::Option<bool>,
    /// list of supported capabilities
    #[prost(enumeration = "features::Capability", repeated, packed = "false", tag = "30")]
    pub capabilities: ::prost::alloc::vec::Vec<i32>,
    /// type of device backup (BIP-39 / SLIP-39 basic / SLIP-39 advanced)
    #[prost(enumeration = "BackupType", optional, tag = "31")]
    pub backup_type: ::core::option::Option<i32>,
    /// is SD card present
    #[prost(bool, optional, tag = "32")]
    pub sd_card_present: ::core::option::Option<bool>,
    /// is SD Protect enabled
    #[prost(bool, optional, tag = "33")]
    pub sd_protection: ::core::option::Option<bool>,
    /// is wipe code protection enabled
    #[prost(bool, optional, tag = "34")]
    pub wipe_code_protection: ::core::option::Option<bool>,
    #[prost(bytes = "vec", optional, tag = "35")]
    pub session_id: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// device enforces passphrase entry on Trezor
    #[prost(bool, optional, tag = "36")]
    pub passphrase_always_on_device: ::core::option::Option<bool>,
    /// safety check level, set to Prompt to limit path namespace enforcement
    #[prost(enumeration = "SafetyCheckLevel", optional, tag = "37")]
    pub safety_checks: ::core::option::Option<i32>,
    /// number of milliseconds after which the device locks itself
    #[prost(uint32, optional, tag = "38")]
    pub auto_lock_delay_ms: ::core::option::Option<u32>,
    /// in degrees from North
    #[prost(uint32, optional, tag = "39")]
    pub display_rotation: ::core::option::Option<u32>,
    /// are experimental message types enabled?
    #[prost(bool, optional, tag = "40")]
    pub experimental_features: ::core::option::Option<bool>,
}
/// Nested message and enum types in `Features`.
pub mod features {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum Capability {
        Bitcoin = 1,
        /// Altcoins based on the Bitcoin source code
        BitcoinLike = 2,
        Binance = 3,
        Cardano = 4,
        /// generic crypto operations for GPG, SSH, etc.
        Crypto = 5,
        Eos = 6,
        Ethereum = 7,
        Lisk = 8,
        Monero = 9,
        Nem = 10,
        Ripple = 11,
        Stellar = 12,
        Tezos = 13,
        U2f = 14,
        Shamir = 15,
        ShamirGroups = 16,
        /// the device is capable of passphrase entry directly on the device
        PassphraseEntry = 17,
    }
}
///*
/// Request: soft-lock the device. Following actions will require PIN. Passphrases remain cached.
/// @start
/// @next Success
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct LockDevice {}
///*
/// Request: end the current sesson. Following actions must call Initialize again.
/// Cache for the current session is discarded, other sessions remain intact.
/// Device is not PIN-locked.
/// @start
/// @next Success
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EndSession {}
///*
/// Request: change language and/or label of the device
/// @start
/// @next Success
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ApplySettings {
    #[prost(string, optional, tag = "1")]
    pub language: ::core::option::Option<::prost::alloc::string::String>,
    #[prost(string, optional, tag = "2")]
    pub label: ::core::option::Option<::prost::alloc::string::String>,
    #[prost(bool, optional, tag = "3")]
    pub use_passphrase: ::core::option::Option<bool>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub homescreen: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    ///    optional PassphraseSourceType passphrase_source = 5;  DEPRECATED
    #[prost(uint32, optional, tag = "6")]
    pub auto_lock_delay_ms: ::core::option::Option<u32>,
    /// in degrees from North
    #[prost(uint32, optional, tag = "7")]
    pub display_rotation: ::core::option::Option<u32>,
    /// do not prompt for passphrase, enforce device entry
    #[prost(bool, optional, tag = "8")]
    pub passphrase_always_on_device: ::core::option::Option<bool>,
    /// Safety check level, set to Prompt to limit path namespace enforcement
    #[prost(enumeration = "SafetyCheckLevel", optional, tag = "9")]
    pub safety_checks: ::core::option::Option<i32>,
    /// enable experimental message types
    #[prost(bool, optional, tag = "10")]
    pub experimental_features: ::core::option::Option<bool>,
}
///*
/// Request: set flags of the device
/// @start
/// @next Success
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ApplyFlags {
    /// bitmask, can only set bits, not unset
    #[prost(uint32, required, tag = "1")]
    pub flags: u32,
}
///*
/// Request: Starts workflow for setting/changing/removing the PIN
/// @start
/// @next Success
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ChangePin {
    /// is PIN removal requested?
    #[prost(bool, optional, tag = "1")]
    pub remove: ::core::option::Option<bool>,
}
///*
/// Request: Starts workflow for setting/removing the wipe code
/// @start
/// @next Success
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ChangeWipeCode {
    /// is wipe code removal requested?
    #[prost(bool, optional, tag = "1")]
    pub remove: ::core::option::Option<bool>,
}
///*
/// Request: Starts workflow for enabling/regenerating/disabling SD card protection
/// @start
/// @next Success
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SdProtect {
    #[prost(enumeration = "sd_protect::SdProtectOperationType", required, tag = "1")]
    pub operation: i32,
}
/// Nested message and enum types in `SdProtect`.
pub mod sd_protect {
    ///*
    /// Structure representing SD card protection operation
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum SdProtectOperationType {
        Disable = 0,
        Enable = 1,
        Refresh = 2,
    }
}
///*
/// Request: Test if the device is alive, device sends back the message in Success response
/// @start
/// @next Success
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Ping {
    /// message to send back in Success message
    #[prost(string, optional, tag = "1", default = "")]
    pub message: ::core::option::Option<::prost::alloc::string::String>,
    /// ask for button press
    #[prost(bool, optional, tag = "2")]
    pub button_protection: ::core::option::Option<bool>,
}
///*
/// Request: Abort last operation that required user interaction
/// @start
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Cancel {}
///*
/// Request: Request a sample of random data generated by hardware RNG. May be used for testing.
/// @start
/// @next Entropy
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetEntropy {
    /// size of requested entropy
    #[prost(uint32, required, tag = "1")]
    pub size: u32,
}
///*
/// Response: Reply with random data generated by internal RNG
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Entropy {
    /// chunk of random generated bytes
    #[prost(bytes = "vec", required, tag = "1")]
    pub entropy: ::prost::alloc::vec::Vec<u8>,
}
///*
/// Request: Request device to wipe all sensitive data and settings
/// @start
/// @next Success
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WipeDevice {}
///*
/// Request: Load seed and related internal settings from the computer
/// @start
/// @next Success
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct LoadDevice {
    /// seed encoded as mnemonic (12, 18 or 24 words for BIP39, 20 or 33 for SLIP39)
    #[prost(string, repeated, tag = "1")]
    pub mnemonics: ::prost::alloc::vec::Vec<::prost::alloc::string::String>,
    /// set PIN protection
    #[prost(string, optional, tag = "3")]
    pub pin: ::core::option::Option<::prost::alloc::string::String>,
    /// enable master node encryption using passphrase
    #[prost(bool, optional, tag = "4")]
    pub passphrase_protection: ::core::option::Option<bool>,
    /// device language (IETF BCP 47 language tag)
    #[prost(string, optional, tag = "5", default = "en-US")]
    pub language: ::core::option::Option<::prost::alloc::string::String>,
    /// device label
    #[prost(string, optional, tag = "6")]
    pub label: ::core::option::Option<::prost::alloc::string::String>,
    /// do not test mnemonic for valid BIP-39 checksum
    #[prost(bool, optional, tag = "7")]
    pub skip_checksum: ::core::option::Option<bool>,
    /// U2F counter
    #[prost(uint32, optional, tag = "8")]
    pub u2f_counter: ::core::option::Option<u32>,
    /// set "needs backup" flag
    #[prost(bool, optional, tag = "9")]
    pub needs_backup: ::core::option::Option<bool>,
    /// indicate that no backup is going to be made
    #[prost(bool, optional, tag = "10")]
    pub no_backup: ::core::option::Option<bool>,
}
///*
/// Request: Ask device to do initialization involving user interaction
/// @start
/// @next EntropyRequest
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ResetDevice {
    /// display entropy generated by the device before asking for additional entropy
    #[prost(bool, optional, tag = "1")]
    pub display_random: ::core::option::Option<bool>,
    /// strength of seed in bits
    #[prost(uint32, optional, tag = "2", default = "256")]
    pub strength: ::core::option::Option<u32>,
    /// enable master node encryption using passphrase
    #[prost(bool, optional, tag = "3")]
    pub passphrase_protection: ::core::option::Option<bool>,
    /// enable PIN protection
    #[prost(bool, optional, tag = "4")]
    pub pin_protection: ::core::option::Option<bool>,
    /// device language (IETF BCP 47 language tag)
    #[prost(string, optional, tag = "5", default = "en-US")]
    pub language: ::core::option::Option<::prost::alloc::string::String>,
    /// device label
    #[prost(string, optional, tag = "6")]
    pub label: ::core::option::Option<::prost::alloc::string::String>,
    /// U2F counter
    #[prost(uint32, optional, tag = "7")]
    pub u2f_counter: ::core::option::Option<u32>,
    /// postpone seed backup to BackupDevice workflow
    #[prost(bool, optional, tag = "8")]
    pub skip_backup: ::core::option::Option<bool>,
    /// indicate that no backup is going to be made
    #[prost(bool, optional, tag = "9")]
    pub no_backup: ::core::option::Option<bool>,
    /// type of the mnemonic backup
    #[prost(enumeration = "BackupType", optional, tag = "10", default = "Bip39")]
    pub backup_type: ::core::option::Option<i32>,
}
///*
/// Request: Perform backup of the device seed if not backed up using ResetDevice
/// @start
/// @next Success
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct BackupDevice {}
///*
/// Response: Ask for additional entropy from host computer
/// @next EntropyAck
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EntropyRequest {}
///*
/// Request: Provide additional entropy for seed generation function
/// @next Success
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EntropyAck {
    /// 256 bits (32 bytes) of random data
    #[prost(bytes = "vec", required, tag = "1")]
    pub entropy: ::prost::alloc::vec::Vec<u8>,
}
///*
/// Request: Start recovery workflow asking user for specific words of mnemonic
/// Used to recovery device safely even on untrusted computer.
/// @start
/// @next WordRequest
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RecoveryDevice {
    /// number of words in BIP-39 mnemonic
    #[prost(uint32, optional, tag = "1")]
    pub word_count: ::core::option::Option<u32>,
    /// enable master node encryption using passphrase
    #[prost(bool, optional, tag = "2")]
    pub passphrase_protection: ::core::option::Option<bool>,
    /// enable PIN protection
    #[prost(bool, optional, tag = "3")]
    pub pin_protection: ::core::option::Option<bool>,
    /// device language (IETF BCP 47 language tag)
    #[prost(string, optional, tag = "4")]
    pub language: ::core::option::Option<::prost::alloc::string::String>,
    /// device label
    #[prost(string, optional, tag = "5")]
    pub label: ::core::option::Option<::prost::alloc::string::String>,
    /// enforce BIP-39 wordlist during the process
    #[prost(bool, optional, tag = "6")]
    pub enforce_wordlist: ::core::option::Option<bool>,
    /// 7 reserved for unused recovery method
    ///
    /// supported recovery type
    #[prost(enumeration = "recovery_device::RecoveryDeviceType", optional, tag = "8")]
    pub r#type: ::core::option::Option<i32>,
    /// U2F counter
    #[prost(uint32, optional, tag = "9")]
    pub u2f_counter: ::core::option::Option<u32>,
    /// perform dry-run recovery workflow (for safe mnemonic validation)
    #[prost(bool, optional, tag = "10")]
    pub dry_run: ::core::option::Option<bool>,
}
/// Nested message and enum types in `RecoveryDevice`.
pub mod recovery_device {
    ///*
    /// Type of recovery procedure. These should be used as bitmask, e.g.,
    /// `RecoveryDeviceType_ScrambledWords | RecoveryDeviceType_Matrix`
    /// listing every method supported by the host computer.
    ///
    /// Note that ScrambledWords must be supported by every implementation
    /// for backward compatibility; there is no way to not support it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum RecoveryDeviceType {
        /// use powers of two when extending this field
        ///
        /// words in scrambled order
        ScrambledWords = 0,
        /// matrix recovery type
        Matrix = 1,
    }
}
///*
/// Response: Device is waiting for user to enter word of the mnemonic
/// Its position is shown only on device's internal display.
/// @next WordAck
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WordRequest {
    #[prost(enumeration = "word_request::WordRequestType", required, tag = "1")]
    pub r#type: i32,
}
/// Nested message and enum types in `WordRequest`.
pub mod word_request {
    ///*
    /// Type of Recovery Word request
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum WordRequestType {
        Plain = 0,
        Matrix9 = 1,
        Matrix6 = 2,
    }
}
///*
/// Request: Computer replies with word from the mnemonic
/// @next WordRequest
/// @next Success
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct WordAck {
    /// one word of mnemonic on asked position
    #[prost(string, required, tag = "1")]
    pub word: ::prost::alloc::string::String,
}
///*
/// Request: Set U2F counter
/// @start
/// @next Success
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SetU2fCounter {
    #[prost(uint32, required, tag = "1")]
    pub u2f_counter: u32,
}
///*
/// Request: Set U2F counter
/// @start
/// @next NextU2FCounter
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetNextU2fCounter {}
///*
/// Request: Set U2F counter
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct NextU2fCounter {
    #[prost(uint32, required, tag = "1")]
    pub u2f_counter: u32,
}
///*
/// Request: Ask device to prepare for a preauthorized operation.
/// @start
/// @next PreauthorizedRequest
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DoPreauthorized {}
///*
/// Request: Device awaits a preauthorized operation.
/// @start
/// @next SignTx
/// @next GetOwnershipProof
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PreauthorizedRequest {}
///*
/// Request: Cancel any outstanding authorization in the current session.
/// @start
/// @next Success
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct CancelAuthorization {}
///*
/// Request: Reboot firmware to bootloader
/// @start
/// @next Success
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RebootToBootloader {}
///*
/// Type of the mnemonic backup given/received by the device during reset/recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum BackupType {
    /// also called "Single Backup", see BIP-0039
    Bip39 = 0,
    /// also called "Shamir Backup", see SLIP-0039
    Slip39Basic = 1,
    /// also called "Super Shamir" or "Shamir with Groups", see SLIP-0039#two-level-scheme
    Slip39Advanced = 2,
}
///*
/// Level of safety checks for unsafe actions like spending from invalid path namespace or setting high transaction fee.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum SafetyCheckLevel {
    /// disallow unsafe actions, this is the default
    Strict = 0,
    /// ask user before unsafe action
    PromptAlways = 1,
    /// like PromptAlways but reverts to Strict after reboot
    PromptTemporarily = 2,
}
