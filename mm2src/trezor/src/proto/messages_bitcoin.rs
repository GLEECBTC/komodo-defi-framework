// Generated protocol messages; many types are not instantiated in our build.
#![allow(dead_code)]
///*
/// Type of redeem script used in input
/// @embed
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MultisigRedeemScriptType {
    /// pubkeys from multisig address (sorted lexicographically)
    #[prost(message, repeated, tag = "1")]
    pub pubkeys: ::prost::alloc::vec::Vec<multisig_redeem_script_type::HdNodePathType>,
    /// existing signatures for partially signed input
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub signatures: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    /// "m" from n, how many valid signatures is necessary for spending
    #[prost(uint32, required, tag = "3")]
    pub m: u32,
    /// simplified way how to specify pubkeys if they share the same address_n path
    #[prost(message, repeated, tag = "4")]
    pub nodes: ::prost::alloc::vec::Vec<super::common::HdNodeType>,
    /// use only field 1 or fields 4+5, if fields 4+5 are used, field 1 is ignored
    #[prost(uint32, repeated, packed = "false", tag = "5")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
}
/// Nested message and enum types in `MultisigRedeemScriptType`.
pub mod multisig_redeem_script_type {
    ///*
    /// Structure representing HDNode + Path
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct HdNodePathType {
        /// BIP-32 node in deserialized form
        #[prost(message, required, tag = "1")]
        pub node: super::super::common::HdNodeType,
        /// BIP-32 path to derive the key from node
        #[prost(uint32, repeated, packed = "false", tag = "2")]
        pub address_n: ::prost::alloc::vec::Vec<u32>,
    }
}
///*
/// Request: Ask device for public key corresponding to address_n path
/// @start
/// @next PublicKey
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetPublicKey {
    /// BIP-32 path to derive the key from master node
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
    /// ECDSA curve name to use
    #[prost(string, optional, tag = "2")]
    pub ecdsa_curve_name: ::core::option::Option<::prost::alloc::string::String>,
    /// optionally show on display before sending the result
    #[prost(bool, optional, tag = "3")]
    pub show_display: ::core::option::Option<bool>,
    /// coin to use for verifying
    #[prost(string, optional, tag = "4", default = "Bitcoin")]
    pub coin_name: ::core::option::Option<::prost::alloc::string::String>,
    /// used to distinguish between various address formats (non-segwit, segwit, etc.)
    #[prost(enumeration = "InputScriptType", optional, tag = "5", default = "Spendaddress")]
    pub script_type: ::core::option::Option<i32>,
    /// ignore SLIP-0132 XPUB magic, use xpub/tpub prefix for all account types
    #[prost(bool, optional, tag = "6")]
    pub ignore_xpub_magic: ::core::option::Option<bool>,
}
///*
/// Response: Contains public key derived from device private seed
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PublicKey {
    /// BIP32 public node
    #[prost(message, required, tag = "1")]
    pub node: super::common::HdNodeType,
    /// serialized form of public node
    #[prost(string, required, tag = "2")]
    pub xpub: ::prost::alloc::string::String,
    /// master root node fingerprint
    #[prost(uint32, optional, tag = "3")]
    pub root_fingerprint: ::core::option::Option<u32>,
}
///*
/// Request: Ask device for address corresponding to address_n path
/// @start
/// @next Address
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetAddress {
    /// BIP-32 path to derive the key from master node
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
    /// coin to use
    #[prost(string, optional, tag = "2", default = "Bitcoin")]
    pub coin_name: ::core::option::Option<::prost::alloc::string::String>,
    /// optionally show on display before sending the result
    #[prost(bool, optional, tag = "3")]
    pub show_display: ::core::option::Option<bool>,
    /// filled if we are showing a multisig address
    #[prost(message, optional, tag = "4")]
    pub multisig: ::core::option::Option<MultisigRedeemScriptType>,
    /// used to distinguish between various address formats (non-segwit, segwit, etc.)
    #[prost(enumeration = "InputScriptType", optional, tag = "5", default = "Spendaddress")]
    pub script_type: ::core::option::Option<i32>,
    /// ignore SLIP-0132 XPUB magic, use xpub/tpub prefix for all account types
    #[prost(bool, optional, tag = "6")]
    pub ignore_xpub_magic: ::core::option::Option<bool>,
}
///*
/// Response: Contains address derived from device private seed
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Address {
    /// Coin address in Base58 encoding
    #[prost(string, required, tag = "1")]
    pub address: ::prost::alloc::string::String,
}
///*
/// Request: Ask device for ownership identifier corresponding to scriptPubKey for address_n path
/// @start
/// @next OwnershipId
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetOwnershipId {
    /// BIP-32 path to derive the key from master node
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
    /// coin to use
    #[prost(string, optional, tag = "2", default = "Bitcoin")]
    pub coin_name: ::core::option::Option<::prost::alloc::string::String>,
    /// filled if we are dealing with a multisig scriptPubKey
    #[prost(message, optional, tag = "3")]
    pub multisig: ::core::option::Option<MultisigRedeemScriptType>,
    /// used to distinguish between various address formats (non-segwit, segwit, etc.)
    #[prost(enumeration = "InputScriptType", optional, tag = "4", default = "Spendaddress")]
    pub script_type: ::core::option::Option<i32>,
}
///*
/// Response: Contains the ownership identifier for the scriptPubKey and device private seed
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct OwnershipId {
    /// ownership identifier
    #[prost(bytes = "vec", required, tag = "1")]
    pub ownership_id: ::prost::alloc::vec::Vec<u8>,
}
///*
/// Request: Ask device to sign message
/// @start
/// @next MessageSignature
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SignMessage {
    /// BIP-32 path to derive the key from master node
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
    /// message to be signed
    #[prost(bytes = "vec", required, tag = "2")]
    pub message: ::prost::alloc::vec::Vec<u8>,
    /// coin to use for signing
    #[prost(string, optional, tag = "3", default = "Bitcoin")]
    pub coin_name: ::core::option::Option<::prost::alloc::string::String>,
    /// used to distinguish between various address formats (non-segwit, segwit, etc.)
    #[prost(enumeration = "InputScriptType", optional, tag = "4", default = "Spendaddress")]
    pub script_type: ::core::option::Option<i32>,
}
///*
/// Response: Signed message
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct MessageSignature {
    /// address used to sign the message
    #[prost(string, required, tag = "1")]
    pub address: ::prost::alloc::string::String,
    /// signature of the message
    #[prost(bytes = "vec", required, tag = "2")]
    pub signature: ::prost::alloc::vec::Vec<u8>,
}
///*
/// Request: Ask device to verify message
/// @start
/// @next Success
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct VerifyMessage {
    /// address to verify
    #[prost(string, required, tag = "1")]
    pub address: ::prost::alloc::string::String,
    /// signature to verify
    #[prost(bytes = "vec", required, tag = "2")]
    pub signature: ::prost::alloc::vec::Vec<u8>,
    /// message to verify
    #[prost(bytes = "vec", required, tag = "3")]
    pub message: ::prost::alloc::vec::Vec<u8>,
    /// coin to use for verifying
    #[prost(string, optional, tag = "4", default = "Bitcoin")]
    pub coin_name: ::core::option::Option<::prost::alloc::string::String>,
}
///*
/// Request: Ask device to sign transaction
/// @start
/// @next TxRequest
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SignTx {
    /// number of transaction outputs
    #[prost(uint32, required, tag = "1")]
    pub outputs_count: u32,
    /// number of transaction inputs
    #[prost(uint32, required, tag = "2")]
    pub inputs_count: u32,
    /// coin to use
    #[prost(string, optional, tag = "3", default = "Bitcoin")]
    pub coin_name: ::core::option::Option<::prost::alloc::string::String>,
    /// transaction version
    #[prost(uint32, optional, tag = "4", default = "1")]
    pub version: ::core::option::Option<u32>,
    /// transaction lock_time
    #[prost(uint32, optional, tag = "5", default = "0")]
    pub lock_time: ::core::option::Option<u32>,
    /// only for Decred and Zcash
    #[prost(uint32, optional, tag = "6")]
    pub expiry: ::core::option::Option<u32>,
    /// deprecated in 2.3.2, the field is not needed as it can be derived from `version`
    #[deprecated]
    #[prost(bool, optional, tag = "7")]
    pub overwintered: ::core::option::Option<bool>,
    /// only for Zcash, nVersionGroupId
    #[prost(uint32, optional, tag = "8")]
    pub version_group_id: ::core::option::Option<u32>,
    /// only for Peercoin
    #[prost(uint32, optional, tag = "9")]
    pub timestamp: ::core::option::Option<u32>,
    /// only for Zcash, BRANCH_ID
    #[prost(uint32, optional, tag = "10")]
    pub branch_id: ::core::option::Option<u32>,
    /// show amounts in
    #[prost(enumeration = "AmountUnit", optional, tag = "11", default = "Bitcoin")]
    pub amount_unit: ::core::option::Option<i32>,
    /// only for Decred, this is signing a ticket purchase
    #[prost(bool, optional, tag = "12", default = "false")]
    pub decred_staking_ticket: ::core::option::Option<bool>,
}
///*
/// Response: Device asks for information for signing transaction or returns the last result
/// If request_index is set, device awaits TxAck<any> matching the request type.
/// If signature_index is set, 'signature' contains signed input of signature_index's input
/// @end
/// @next TxAckInput
/// @next TxAckOutput
/// @next TxAckPrevMeta
/// @next TxAckPrevInput
/// @next TxAckPrevOutput
/// @next TxAckPrevExtraData
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TxRequest {
    /// what should be filled in TxAck message?
    #[prost(enumeration = "tx_request::RequestType", optional, tag = "1")]
    pub request_type: ::core::option::Option<i32>,
    /// request for tx details
    #[prost(message, optional, tag = "2")]
    pub details: ::core::option::Option<tx_request::TxRequestDetailsType>,
    /// serialized data and request for next
    #[prost(message, optional, tag = "3")]
    pub serialized: ::core::option::Option<tx_request::TxRequestSerializedType>,
}
/// Nested message and enum types in `TxRequest`.
pub mod tx_request {
    ///*
    /// Structure representing request details
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TxRequestDetailsType {
        /// device expects TxAck message from the computer
        #[prost(uint32, optional, tag = "1")]
        pub request_index: ::core::option::Option<u32>,
        /// tx_hash of requested transaction
        #[prost(bytes = "vec", optional, tag = "2")]
        pub tx_hash: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
        /// length of requested extra data (only for Dash, Zcash)
        #[prost(uint32, optional, tag = "3")]
        pub extra_data_len: ::core::option::Option<u32>,
        /// offset of requested extra data (only for Dash, Zcash)
        #[prost(uint32, optional, tag = "4")]
        pub extra_data_offset: ::core::option::Option<u32>,
    }
    ///*
    /// Structure representing serialized data
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TxRequestSerializedType {
        /// 'signature' field contains signed input of this index
        #[prost(uint32, optional, tag = "1")]
        pub signature_index: ::core::option::Option<u32>,
        /// signature of the signature_index input
        #[prost(bytes = "vec", optional, tag = "2")]
        pub signature: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
        /// part of serialized and signed transaction
        #[prost(bytes = "vec", optional, tag = "3")]
        pub serialized_tx: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    }
    ///*
    /// Type of information required by transaction signing process
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
    #[repr(i32)]
    pub enum RequestType {
        Txinput = 0,
        Txoutput = 1,
        Txmeta = 2,
        Txfinished = 3,
        Txextradata = 4,
        Txoriginput = 5,
        Txorigoutput = 6,
    }
}
///*
/// Request: Reported transaction data (legacy)
///
/// This message contains all possible field that can be sent in response to a TxRequest.
/// Depending on the request_type, the host is supposed to fill some of these fields.
///
/// The interface is wire-compatible with the new method of specialized TxAck subtypes,
/// so it can be used in the old way. However, it is now recommended to use more
/// specialized messages, which have better-configured constraints on field values.
///
/// @next TxRequest
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TxAck {
    #[prost(message, optional, tag = "1")]
    pub tx: ::core::option::Option<tx_ack::TransactionType>,
}
/// Nested message and enum types in `TxAck`.
pub mod tx_ack {
    ///*
    /// Structure representing transaction
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TransactionType {
        #[prost(uint32, optional, tag = "1")]
        pub version: ::core::option::Option<u32>,
        #[prost(message, repeated, tag = "2")]
        pub inputs: ::prost::alloc::vec::Vec<transaction_type::TxInputType>,
        #[prost(message, repeated, tag = "3")]
        pub bin_outputs: ::prost::alloc::vec::Vec<transaction_type::TxOutputBinType>,
        #[prost(uint32, optional, tag = "4")]
        pub lock_time: ::core::option::Option<u32>,
        #[prost(message, repeated, tag = "5")]
        pub outputs: ::prost::alloc::vec::Vec<transaction_type::TxOutputType>,
        #[prost(uint32, optional, tag = "6")]
        pub inputs_cnt: ::core::option::Option<u32>,
        #[prost(uint32, optional, tag = "7")]
        pub outputs_cnt: ::core::option::Option<u32>,
        /// only for Dash, Zcash
        #[prost(bytes = "vec", optional, tag = "8")]
        pub extra_data: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
        /// only for Dash, Zcash
        #[prost(uint32, optional, tag = "9")]
        pub extra_data_len: ::core::option::Option<u32>,
        /// only for Decred and Zcash
        #[prost(uint32, optional, tag = "10")]
        pub expiry: ::core::option::Option<u32>,
        /// Zcash only; deprecated in 2.3.2, the field is not needed, it can be derived from `version`
        #[deprecated]
        #[prost(bool, optional, tag = "11")]
        pub overwintered: ::core::option::Option<bool>,
        /// only for Zcash, nVersionGroupId
        #[prost(uint32, optional, tag = "12")]
        pub version_group_id: ::core::option::Option<u32>,
        /// only for Peercoin
        #[prost(uint32, optional, tag = "13")]
        pub timestamp: ::core::option::Option<u32>,
        /// only for Zcash, BRANCH_ID
        #[prost(uint32, optional, tag = "14")]
        pub branch_id: ::core::option::Option<u32>,
    }
    /// Nested message and enum types in `TransactionType`.
    pub mod transaction_type {
        ///*
        /// Structure representing transaction input
        #[derive(Clone, PartialEq, ::prost::Message)]
        pub struct TxInputType {
            /// BIP-32 path to derive the key from master node
            #[prost(uint32, repeated, packed = "false", tag = "1")]
            pub address_n: ::prost::alloc::vec::Vec<u32>,
            /// hash of previous transaction output to spend by this input
            #[prost(bytes = "vec", required, tag = "2")]
            pub prev_hash: ::prost::alloc::vec::Vec<u8>,
            /// index of previous output to spend
            #[prost(uint32, required, tag = "3")]
            pub prev_index: u32,
            /// script signature, unset for tx to sign
            #[prost(bytes = "vec", optional, tag = "4")]
            pub script_sig: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
            /// sequence (default=0xffffffff)
            #[prost(uint32, optional, tag = "5", default = "4294967295")]
            pub sequence: ::core::option::Option<u32>,
            /// defines template of input script
            #[prost(
                enumeration = "super::super::InputScriptType",
                optional,
                tag = "6",
                default = "Spendaddress"
            )]
            pub script_type: ::core::option::Option<i32>,
            /// Filled if input is going to spend multisig tx
            #[prost(message, optional, tag = "7")]
            pub multisig: ::core::option::Option<super::super::MultisigRedeemScriptType>,
            /// amount of previous transaction output (for segwit only)
            #[prost(uint64, optional, tag = "8")]
            pub amount: ::core::option::Option<u64>,
            /// only for Decred, 0 is a normal transaction while 1 is a stake transaction
            #[prost(uint32, optional, tag = "9")]
            pub decred_tree: ::core::option::Option<u32>,
            /// optional uint32 decred_script_version = 10;                         // only for Decred  // deprecated -> only 0 is supported
            /// optional bytes prev_block_hash_bip115 = 11;     // BIP-115 support dropped
            /// optional uint32 prev_block_height_bip115 = 12;  // BIP-115 support dropped
            ///
            /// witness data, only set for EXTERNAL inputs
            #[prost(bytes = "vec", optional, tag = "13")]
            pub witness: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
            /// SLIP-0019 proof of ownership, only set for EXTERNAL inputs
            #[prost(bytes = "vec", optional, tag = "14")]
            pub ownership_proof: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
            /// optional commitment data for the SLIP-0019 proof of ownership
            #[prost(bytes = "vec", optional, tag = "15")]
            pub commitment_data: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
            /// tx_hash of the original transaction where this input was spent (used when creating a replacement transaction)
            #[prost(bytes = "vec", optional, tag = "16")]
            pub orig_hash: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
            /// index of the input in the original transaction (used when creating a replacement transaction)
            #[prost(uint32, optional, tag = "17")]
            pub orig_index: ::core::option::Option<u32>,
            /// if not None this holds the type of stake spend: revocation or stake generation
            #[prost(enumeration = "super::super::DecredStakingSpendType", optional, tag = "18")]
            pub decred_staking_spend: ::core::option::Option<i32>,
        }
        ///*
        /// Structure representing compiled transaction output
        #[derive(Clone, PartialEq, ::prost::Message)]
        pub struct TxOutputBinType {
            #[prost(uint64, required, tag = "1")]
            pub amount: u64,
            #[prost(bytes = "vec", required, tag = "2")]
            pub script_pubkey: ::prost::alloc::vec::Vec<u8>,
            /// only for Decred, currently only 0 is supported
            #[prost(uint32, optional, tag = "3")]
            pub decred_script_version: ::core::option::Option<u32>,
        }
        ///*
        /// Structure representing transaction output
        #[derive(Clone, PartialEq, ::prost::Message)]
        pub struct TxOutputType {
            /// target coin address in Base58 encoding
            #[prost(string, optional, tag = "1")]
            pub address: ::core::option::Option<::prost::alloc::string::String>,
            /// BIP-32 path to derive the key from master node; has higher priority than "address"
            #[prost(uint32, repeated, packed = "false", tag = "2")]
            pub address_n: ::prost::alloc::vec::Vec<u32>,
            /// amount to spend in satoshis
            #[prost(uint64, required, tag = "3")]
            pub amount: u64,
            /// output script type
            #[prost(
                enumeration = "super::super::OutputScriptType",
                optional,
                tag = "4",
                default = "Paytoaddress"
            )]
            pub script_type: ::core::option::Option<i32>,
            /// defines multisig address; script_type must be PAYTOMULTISIG
            #[prost(message, optional, tag = "5")]
            pub multisig: ::core::option::Option<super::super::MultisigRedeemScriptType>,
            /// defines op_return data; script_type must be PAYTOOPRETURN, amount must be 0
            #[prost(bytes = "vec", optional, tag = "6")]
            pub op_return_data: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
            /// optional uint32 decred_script_version = 7;      // only for Decred  // deprecated -> only 0 is supported
            /// optional bytes block_hash_bip115 = 8;        // BIP-115 support dropped
            /// optional uint32 block_height_bip115 = 9;     // BIP-115 support dropped
            ///
            /// tx_hash of the original transaction where this output was present (used when creating a replacement transaction)
            #[prost(bytes = "vec", optional, tag = "10")]
            pub orig_hash: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
            /// index of the output in the original transaction (used when creating a replacement transaction)
            #[prost(uint32, optional, tag = "11")]
            pub orig_index: ::core::option::Option<u32>,
        }
    }
}
///* Data type for transaction input to be signed.
///
/// When adding fields, take care to not conflict with PrevInput
///
/// @embed
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TxInput {
    /// BIP-32 path to derive the key from master node
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
    /// hash of previous transaction output to spend by this input
    #[prost(bytes = "vec", required, tag = "2")]
    pub prev_hash: ::prost::alloc::vec::Vec<u8>,
    /// index of previous output to spend
    #[prost(uint32, required, tag = "3")]
    pub prev_index: u32,
    /// script signature, only set for EXTERNAL inputs
    #[prost(bytes = "vec", optional, tag = "4")]
    pub script_sig: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// sequence
    #[prost(uint32, optional, tag = "5", default = "4294967295")]
    pub sequence: ::core::option::Option<u32>,
    /// defines template of input script
    #[prost(enumeration = "InputScriptType", optional, tag = "6", default = "Spendaddress")]
    pub script_type: ::core::option::Option<i32>,
    /// Filled if input is going to spend multisig tx
    #[prost(message, optional, tag = "7")]
    pub multisig: ::core::option::Option<MultisigRedeemScriptType>,
    /// amount of previous transaction output
    #[prost(uint64, required, tag = "8")]
    pub amount: u64,
    /// only for Decred, 0 is a normal transaction while 1 is a stake transaction
    #[prost(uint32, optional, tag = "9")]
    pub decred_tree: ::core::option::Option<u32>,
    /// witness data, only set for EXTERNAL inputs
    #[prost(bytes = "vec", optional, tag = "13")]
    pub witness: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// SLIP-0019 proof of ownership, only set for EXTERNAL inputs
    #[prost(bytes = "vec", optional, tag = "14")]
    pub ownership_proof: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// optional commitment data for the SLIP-0019 proof of ownership
    #[prost(bytes = "vec", optional, tag = "15")]
    pub commitment_data: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// tx_hash of the original transaction where this input was spent (used when creating a replacement transaction)
    #[prost(bytes = "vec", optional, tag = "16")]
    pub orig_hash: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// index of the input in the original transaction (used when creating a replacement transaction)
    #[prost(uint32, optional, tag = "17")]
    pub orig_index: ::core::option::Option<u32>,
    /// if not None this holds the type of stake spend: revocation or stake generation
    #[prost(enumeration = "DecredStakingSpendType", optional, tag = "18")]
    pub decred_staking_spend: ::core::option::Option<i32>,
}
///* Data type for transaction output to be signed.
///  @embed
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TxOutput {
    /// destination address in Base58 encoding; script_type must be PAYTOADDRESS
    #[prost(string, optional, tag = "1")]
    pub address: ::core::option::Option<::prost::alloc::string::String>,
    /// BIP-32 path to derive the destination (used for change addresses)
    #[prost(uint32, repeated, packed = "false", tag = "2")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
    /// amount to spend in satoshis
    #[prost(uint64, required, tag = "3")]
    pub amount: u64,
    /// output script type
    #[prost(enumeration = "OutputScriptType", optional, tag = "4", default = "Paytoaddress")]
    pub script_type: ::core::option::Option<i32>,
    /// defines multisig address; script_type must be PAYTOMULTISIG
    #[prost(message, optional, tag = "5")]
    pub multisig: ::core::option::Option<MultisigRedeemScriptType>,
    /// defines op_return data; script_type must be PAYTOOPRETURN, amount must be 0
    #[prost(bytes = "vec", optional, tag = "6")]
    pub op_return_data: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// tx_hash of the original transaction where this output was present (used when creating a replacement transaction)
    #[prost(bytes = "vec", optional, tag = "10")]
    pub orig_hash: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// index of the output in the original transaction (used when creating a replacement transaction)
    #[prost(uint32, optional, tag = "11")]
    pub orig_index: ::core::option::Option<u32>,
}
///* Data type for metadata about previous transaction which contains the UTXO being spent.
///  @embed
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PrevTx {
    #[prost(uint32, required, tag = "1")]
    pub version: u32,
    #[prost(uint32, required, tag = "4")]
    pub lock_time: u32,
    #[prost(uint32, required, tag = "6")]
    pub inputs_count: u32,
    #[prost(uint32, required, tag = "7")]
    pub outputs_count: u32,
    /// only for Dash, Zcash
    #[prost(uint32, optional, tag = "9", default = "0")]
    pub extra_data_len: ::core::option::Option<u32>,
    /// only for Decred and Zcash
    #[prost(uint32, optional, tag = "10")]
    pub expiry: ::core::option::Option<u32>,
    /// only for Zcash, nVersionGroupId
    #[prost(uint32, optional, tag = "12")]
    pub version_group_id: ::core::option::Option<u32>,
    /// only for Peercoin
    #[prost(uint32, optional, tag = "13")]
    pub timestamp: ::core::option::Option<u32>,
    /// only for Zcash, BRANCH_ID
    #[prost(uint32, optional, tag = "14")]
    pub branch_id: ::core::option::Option<u32>,
}
///* Data type for inputs of previous transactions.
///
/// When adding fields, take care to not conflict with TxInput
/// @embed
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PrevInput {
    /// hash of previous transaction output to spend by this input
    #[prost(bytes = "vec", required, tag = "2")]
    pub prev_hash: ::prost::alloc::vec::Vec<u8>,
    /// index of previous output to spend
    #[prost(uint32, required, tag = "3")]
    pub prev_index: u32,
    /// script signature
    #[prost(bytes = "vec", required, tag = "4")]
    pub script_sig: ::prost::alloc::vec::Vec<u8>,
    /// sequence
    #[prost(uint32, required, tag = "5")]
    pub sequence: u32,
    /// only for Decred
    #[prost(uint32, optional, tag = "9")]
    pub decred_tree: ::core::option::Option<u32>,
}
///* Data type for outputs of previous transactions.
///  @embed
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct PrevOutput {
    /// amount sent to this output
    #[prost(uint64, required, tag = "1")]
    pub amount: u64,
    /// scriptPubkey of this output
    #[prost(bytes = "vec", required, tag = "2")]
    pub script_pubkey: ::prost::alloc::vec::Vec<u8>,
    /// only for Decred
    #[prost(uint32, optional, tag = "3")]
    pub decred_script_version: ::core::option::Option<u32>,
}
///*
/// Request: Data about input to be signed.
/// Wire-alias of TxAck.
///
/// Do not edit this type without considering compatibility with TxAck.
/// Prefer to modify the inner TxInput type.
///
/// @next TxRequest
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TxAckInput {
    #[prost(message, required, tag = "1")]
    pub tx: tx_ack_input::TxAckInputWrapper,
}
/// Nested message and enum types in `TxAckInput`.
pub mod tx_ack_input {
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TxAckInputWrapper {
        #[prost(message, required, tag = "2")]
        pub input: super::TxInput,
    }
}
///*
/// Request: Data about output to be signed.
/// Wire-alias of TxAck.
///
/// Do not edit this type without considering compatibility with TxAck.
/// Prefer to modify the inner TxOutput type.
///
/// @next TxRequest
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TxAckOutput {
    #[prost(message, required, tag = "1")]
    pub tx: tx_ack_output::TxAckOutputWrapper,
}
/// Nested message and enum types in `TxAckOutput`.
pub mod tx_ack_output {
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TxAckOutputWrapper {
        #[prost(message, required, tag = "5")]
        pub output: super::TxOutput,
    }
}
///*
/// Request: Data about previous transaction metadata
/// Wire-alias of TxAck.
///
/// Do not edit this type without considering compatibility with TxAck.
/// Prefer to modify the inner PrevTx type.
///
/// @next TxRequest
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TxAckPrevMeta {
    #[prost(message, required, tag = "1")]
    pub tx: PrevTx,
}
///*
/// Request: Data about previous transaction input
/// Wire-alias of TxAck.
///
/// Do not edit this type without considering compatibility with TxAck.
/// Prefer to modify the inner PrevInput type.
///
/// @next TxRequest
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TxAckPrevInput {
    #[prost(message, required, tag = "1")]
    pub tx: tx_ack_prev_input::TxAckPrevInputWrapper,
}
/// Nested message and enum types in `TxAckPrevInput`.
pub mod tx_ack_prev_input {
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TxAckPrevInputWrapper {
        #[prost(message, required, tag = "2")]
        pub input: super::PrevInput,
    }
}
///*
/// Request: Data about previous transaction output
/// Wire-alias of TxAck.
///
/// Do not edit this type without considering compatibility with TxAck.
/// Prefer to modify the inner PrevOutput type.
///
/// @next TxRequest
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TxAckPrevOutput {
    #[prost(message, required, tag = "1")]
    pub tx: tx_ack_prev_output::TxAckPrevOutputWrapper,
}
/// Nested message and enum types in `TxAckPrevOutput`.
pub mod tx_ack_prev_output {
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TxAckPrevOutputWrapper {
        #[prost(message, required, tag = "3")]
        pub output: super::PrevOutput,
    }
}
///*
/// Request: Content of the extra data of a previous transaction
/// Wire-alias of TxAck.
///
/// Do not edit this type without considering compatibility with TxAck.
///
/// @next TxRequest
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct TxAckPrevExtraData {
    #[prost(message, required, tag = "1")]
    pub tx: tx_ack_prev_extra_data::TxAckPrevExtraDataWrapper,
}
/// Nested message and enum types in `TxAckPrevExtraData`.
pub mod tx_ack_prev_extra_data {
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct TxAckPrevExtraDataWrapper {
        #[prost(bytes = "vec", required, tag = "8")]
        pub extra_data_chunk: ::prost::alloc::vec::Vec<u8>,
    }
}
///*
/// Request: Ask device for a proof of ownership corresponding to address_n path
/// @start
/// @next OwnershipProof
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GetOwnershipProof {
    /// BIP-32 path to derive the key from master node
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
    /// coin to use
    #[prost(string, optional, tag = "2", default = "Bitcoin")]
    pub coin_name: ::core::option::Option<::prost::alloc::string::String>,
    /// used to distinguish between various scriptPubKey types
    #[prost(enumeration = "InputScriptType", optional, tag = "3", default = "Spendwitness")]
    pub script_type: ::core::option::Option<i32>,
    /// filled if proof is for a multisig address
    #[prost(message, optional, tag = "4")]
    pub multisig: ::core::option::Option<MultisigRedeemScriptType>,
    /// show a confirmation dialog and set the "user confirmation" bit in the proof
    #[prost(bool, optional, tag = "5", default = "false")]
    pub user_confirmation: ::core::option::Option<bool>,
    /// list of ownership identifiers in case of multisig
    #[prost(bytes = "vec", repeated, tag = "6")]
    pub ownership_ids: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
    /// additional data to which the proof should commit
    #[prost(bytes = "vec", optional, tag = "7", default = "b\"\"")]
    pub commitment_data: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
}
///*
/// Response: Contains the proof of ownership
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct OwnershipProof {
    /// SLIP-0019 proof of ownership
    #[prost(bytes = "vec", required, tag = "1")]
    pub ownership_proof: ::prost::alloc::vec::Vec<u8>,
    /// signature of the proof
    #[prost(bytes = "vec", required, tag = "2")]
    pub signature: ::prost::alloc::vec::Vec<u8>,
}
///*
/// Request: Ask device to prompt the user to authorize a CoinJoin transaction
/// @start
/// @next Success
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct AuthorizeCoinJoin {
    /// coordinator identifier to approve as a prefix in commitment data (max. 18 ASCII characters)
    #[prost(string, required, tag = "1")]
    pub coordinator: ::prost::alloc::string::String,
    /// maximum total fees
    #[prost(uint64, required, tag = "2")]
    pub max_total_fee: u64,
    /// fee per anonymity set in units of 10^-9 percent
    #[prost(uint32, optional, tag = "3", default = "0")]
    pub fee_per_anonymity: ::core::option::Option<u32>,
    /// prefix of the BIP-32 path leading to the account (m / purpose' / coin_type' / account')
    #[prost(uint32, repeated, packed = "false", tag = "4")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
    /// coin to use
    #[prost(string, optional, tag = "5", default = "Bitcoin")]
    pub coin_name: ::core::option::Option<::prost::alloc::string::String>,
    /// used to distinguish between various address formats (non-segwit, segwit, etc.)
    #[prost(enumeration = "InputScriptType", optional, tag = "6", default = "Spendaddress")]
    pub script_type: ::core::option::Option<i32>,
    /// show amounts in
    #[prost(enumeration = "AmountUnit", optional, tag = "11", default = "Bitcoin")]
    pub amount_unit: ::core::option::Option<i32>,
}
///*
/// Type of script which will be used for transaction input
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum InputScriptType {
    /// standard P2PKH address
    Spendaddress = 0,
    /// P2SH multisig address
    Spendmultisig = 1,
    /// reserved for external inputs (coinjoin)
    External = 2,
    /// native SegWit
    Spendwitness = 3,
    /// SegWit over P2SH (backward compatible)
    Spendp2shwitness = 4,
}
///*
/// Type of script which will be used for transaction output
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum OutputScriptType {
    /// used for all addresses (bitcoin, p2sh, witness)
    Paytoaddress = 0,
    /// p2sh address (deprecated; use PAYTOADDRESS)
    Paytoscripthash = 1,
    /// only for change output
    Paytomultisig = 2,
    /// op_return
    Paytoopreturn = 3,
    /// only for change output
    Paytowitness = 4,
    /// only for change output
    Paytop2shwitness = 5,
}
///*
/// Type of script which will be used for decred stake transaction input
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum DecredStakingSpendType {
    SsGen = 0,
    Ssrtx = 1,
}
///*
/// Unit to be used when showing amounts on the display
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::prost::Enumeration)]
#[repr(i32)]
pub enum AmountUnit {
    /// BTC
    Bitcoin = 0,
    /// mBTC
    Millibitcoin = 1,
    /// uBTC
    Microbitcoin = 2,
    /// sat
    Satoshi = 3,
}
