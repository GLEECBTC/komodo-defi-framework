// Generated protocol messages; many types are not instantiated in our build.
#![allow(dead_code)]
///*
/// Request: Ask device for Ethereum address corresponding to address_n path
/// @start
/// @next EthereumAddress
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumGetAddress {
    /// BIP-32 path to derive the key from master node
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
    /// optionally show on display before sending the result
    #[prost(bool, optional, tag = "2")]
    pub show_display: ::std::option::Option<bool>,
    /// encoded Ethereum network, see ethereum-definitions.md for details
    #[prost(bytes = "vec", optional, tag = "3")]
    pub encoded_network: ::std::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// display the address in chunks of 4 characters
    #[prost(bool, optional, tag = "4")]
    pub chunkify: ::std::option::Option<bool>,
}

///*
/// Response: Contains an Ethereum address derived from device private seed
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumAddress {
    /// trezor <1.8.0, <2.1.0 - raw bytes of Ethereum address
    #[prost(bytes = "vec", optional, tag = "1")]
    pub encoded_network: ::std::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// Ethereum address as hex-encoded string
    #[prost(string, optional, tag = "2")]
    pub address: ::core::option::Option<::prost::alloc::string::String>,
}

///*
/// Request: Ask device for public key corresponding to address_n path
/// @start
/// @next EthereumPublicKey
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumGetPublicKey {
    // BIP-32 path to derive the key from master node
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub address_n: ::prost::alloc::vec::Vec<u32>, // repeated uint32 address_n = 1;
    // optionally show on display before sending the result
    #[prost(bool, optional, tag = "2")]
    pub show_display: ::std::option::Option<bool>, // optional bool show_display = 2;
}

///*
/// Response: Contains public key derived from device private seed
/// @end
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumPublicKey {
    // BIP32 public node
    #[prost(message, required, tag = "1")]
    pub node: super::messages_common::HdNodeType, // required hw.trezor.messages.common.HDNodeType node = 1;
    // serialized form of public node
    #[prost(string, required, tag = "2")]
    pub xpub: ::prost::alloc::string::String, // required string xpub = 2;
}

///*
/// Request: Ask device to sign transaction
/// gas_price, gas_limit and chain_id must be provided and non-zero.
/// All other fields are optional and default to value `0` if missing.
/// Note: the first at most 1024 bytes of data MUST be transmitted as part of this message.
/// @start
/// @next EthereumTxRequest
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumSignTx {
    /// BIP-32 path to derive the key from master node
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
    /// <=256 bit unsigned big endian
    #[prost(bytes = "vec", optional, tag = "2", default = "b\"\"")]
    pub nonce: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// <=256 bit unsigned big endian (in wei)
    #[prost(bytes = "vec", required, tag = "3")]
    pub gas_price: ::prost::alloc::vec::Vec<u8>,
    /// <=256 bit unsigned big endian
    #[prost(bytes = "vec", required, tag = "4")]
    pub gas_limit: ::prost::alloc::vec::Vec<u8>,
    /// recipient address
    #[prost(string, optional, tag = "11", default = "")]
    pub to: ::core::option::Option<::prost::alloc::string::String>,
    /// <=256 bit unsigned big endian (in wei)
    #[prost(bytes = "vec", optional, tag = "6", default = "b\"\"")]
    pub value: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// The initial data chunk (<= 1024 bytes)
    #[prost(bytes = "vec", optional, tag = "7", default = "b\"\"")]
    pub data_initial_chunk: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// Length of transaction payload
    #[prost(uint32, optional, tag = "8", default = 0)]
    pub data_length: ::core::option::Option<u32>,
    /// Chain Id for EIP 155
    #[prost(uint64, required, tag = "9")]
    pub chain_id: u64,
    /// Used for Wanchain
    #[prost(uint32, optional, tag = "10")]
    pub tx_type: ::core::option::Option<u32>,
    /// network and/or token definitions for tx
    #[prost(message, optional, tag = "12")]
    pub definitions: ::core::option::Option<super::messages_ethereum_definitions::EthereumDefinitions>,
    /// display the address in chunks of 4 characters
    #[prost(bool, optional, tag = "13")]
    pub chunkify: ::std::option::Option<bool>,
}

///*
/// Request: Ask device to sign EIP1559 transaction
/// Note: the first at most 1024 bytes of data MUST be transmitted as part of this message.
/// @start
/// @next EthereumTxRequest
/// @next Failure
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumSignTxEIP1559 {
    /// BIP-32 path to derive the key from master node
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub address_n: ::prost::alloc::vec::Vec<u32>,
    /// <=256 bit unsigned big endian
    #[prost(bytes = "vec", required, tag = "2")]
    pub nonce: ::prost::alloc::vec::Vec<u8>,
    /// <=256 bit unsigned big endian (in wei)
    #[prost(bytes = "vec", required, tag = "3")]
    pub max_gas_fee: ::prost::alloc::vec::Vec<u8>,
    /// <=256 bit unsigned big endian (in wei)
    #[prost(bytes = "vec", required, tag = "4")]
    pub max_priority_fee: ::prost::alloc::vec::Vec<u8>,
    /// <=256 bit unsigned big endian
    #[prost(bytes = "vec", required, tag = "5")]
    pub gas_limit: ::prost::alloc::vec::Vec<u8>,
    /// recipient address
    #[prost(string, optional, tag = "6", default = "")]
    pub to: ::core::option::Option<::prost::alloc::string::String>,
    /// <=256 bit unsigned big endian (in wei)
    #[prost(bytes = "vec", required, tag = "7")]
    pub value: ::prost::alloc::vec::Vec<u8>,
    /// The initial data chunk (<= 1024 bytes)
    #[prost(bytes = "vec", optional, tag = "8", default = "b\"\"")]
    pub data_initial_chunk: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// Length of transaction payload
    #[prost(uint32, required, tag = "9", default = 0)]
    pub data_length: u32,
    /// Chain Id for EIP 155
    #[prost(uint64, required, tag = "10")]
    pub chain_id: u64,
    /// Access list
    #[prost(message, repeated, tag = "11")]
    pub access_list: ::std::vec::Vec<EthereumAccessList>,
    /// network and/or token definitions for tx
    #[prost(message, optional, tag = "12")]
    pub definitions: ::core::option::Option<super::messages_ethereum_definitions::EthereumDefinitions>,
    /// display the address in chunks of 4 characters
    #[prost(bool, optional, tag = "13")]
    pub chunkify: ::std::option::Option<bool>,
}

#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumAccessList {
    #[prost(string, required, tag = "1")]
    pub address: ::prost::alloc::string::String,
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub storage_keys: ::prost::alloc::vec::Vec<::prost::alloc::vec::Vec<u8>>,
}

///*
/// Response: Device asks for more data from transaction payload, or returns the signature.
/// If data_length is set, device awaits that many more bytes of payload.
/// Otherwise, the signature_* fields contain the computed transaction signature. All three fields will be present.
/// @end
/// @next EthereumTxAck
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumTxRequest {
    /// Number of bytes being requested (<= 1024)
    #[prost(uint32, optional, tag = "1")]
    pub data_length: ::std::option::Option<u32>,
    /// Computed signature (recovery parameter, limited to 27 or 28)
    #[prost(uint32, optional, tag = "2")]
    pub signature_v: ::std::option::Option<u32>,
    /// Computed signature R component (256 bit)
    #[prost(bytes = "vec", optional, tag = "3")]
    pub signature_r: ::std::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// Computed signature S component (256 bit)
    #[prost(bytes = "vec", optional, tag = "4")]
    pub signature_s: ::std::option::Option<::prost::alloc::vec::Vec<u8>>,
}

///*
/// Request: Transaction payload data.
/// @next EthereumTxRequest
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumTxAck {
    /// Bytes from transaction payload (<= 1024 bytes)
    #[prost(bytes = "vec", required, tag = "1")]
    pub data_chunk: ::prost::alloc::vec::Vec<u8>,
}
