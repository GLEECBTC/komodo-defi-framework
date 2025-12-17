// Generated protocol messages; many types are not instantiated in our build.
#![allow(dead_code)]
///*
/// Ethereum network definition. Used to (de)serialize the definition.
/// Must be signed by vendor signatures and could be found on the trezor web site
///
/// Definition types should not be cross-parseable, i.e., it should not be possible to
/// incorrectly parse network info as token info or vice versa.
/// To achieve that, the first field is wire type varint while the second field is wire type
/// length-delimited. Both are a mismatch for the token definition.
///
/// @embed
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumNetworkInfo {
    #[prost(uint64, required, tag = "1")]
    pub chain_id: u64,
    #[prost(string, required, tag = "2")]
    pub symbol: ::prost::alloc::string::String,
    #[prost(uint32, required, tag = "3")]
    pub slip44: u32,
    #[prost(string, required, tag = "4")]
    pub name: ::prost::alloc::string::String,
}

///*
/// Ethereum token definition. Used to (de)serialize the definition.
/// Must be signed by vendor signatures and could be found on the trezor web site
///
/// Definition types should not be cross-parseable, i.e., it should not be possible to
/// incorrectly parse network info as token info or vice versa.
/// To achieve that, the first field is wire type length-delimited while the second field
/// is wire type varint. Both are a mismatch for the network definition.
///
/// @embed
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumTokenInfo {
    #[prost(bytes = "vec", required, tag = "1")]
    pub address: ::prost::alloc::vec::Vec<u8>,
    #[prost(uint64, required, tag = "2")]
    pub chain_id: u64,
    #[prost(string, required, tag = "3")]
    pub symbol: ::prost::alloc::string::String,
    #[prost(uint32, required, tag = "4")]
    pub decimals: u32,
    #[prost(string, required, tag = "5")]
    pub name: ::prost::alloc::string::String,
}

///*
/// Ethereum definitions
/// @embed
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct EthereumDefinitions {
    /// encoded Ethereum network
    #[prost(bytes = "vec", optional, tag = "1")]
    pub encoded_network: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
    /// encoded Ethereum token
    #[prost(bytes = "vec", optional, tag = "2")]
    pub encoded_token: ::core::option::Option<::prost::alloc::vec::Vec<u8>>,
}
