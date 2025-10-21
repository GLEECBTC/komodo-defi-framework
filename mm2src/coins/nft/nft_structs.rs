use common::ten;
use derive_more::Display;
use enum_derives::EnumVariantList;
use ethereum_types::Address;
use mm2_core::mm_ctx::{from_ctx, MmArc};
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, BigUint};
use rpc::v1::types::Bytes as BytesJson;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serializer};
use serde_json::Value as Json;
use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::str::FromStr;
use std::sync::Arc;
use url::Url;

use crate::eth::EthTxFeeDetails;
use crate::hd_wallet::AddrToString;
use crate::nft::nft_errors::{LockDBError, ParseChainTypeError, ParseContractTypeError};
#[cfg(not(target_arch = "wasm32"))]
use crate::nft::storage::NftMigrationOps;
use crate::nft::storage::{NftListStorageOps, NftTransferHistoryStorageOps};
use crate::{TransactionType, TxFeeDetails, WithdrawFee};

cfg_native! {
    use db_common::async_sql_conn::AsyncConnection;
    use futures::lock::Mutex as AsyncMutex;
}

cfg_wasm32! {
    use mm2_db::indexed_db::{ConstructibleDb, SharedDb};
    use crate::nft::storage::wasm::WasmNftCacheError;
    use crate::nft::storage::wasm::nft_idb::NftCacheIDB;
}

/// Represents a request to list NFTs owned by the user across specified chains.
///
/// The request provides options such as pagination, limiting the number of results,
/// and applying specific filters to the list.
#[derive(Debug, Deserialize)]
pub struct NftListReq {
    /// List of chains to fetch the NFTs from.
    pub(crate) chains: Vec<Chain>,
    /// Parameter indicating if the maximum number of NFTs should be fetched.
    /// If true, then `limit` will be ignored.
    #[serde(default)]
    pub(crate) max: bool,
    /// Limit to the number of NFTs returned in a single request.
    #[serde(default = "ten")]
    pub(crate) limit: usize,
    /// Page number for pagination.
    pub(crate) page_number: Option<NonZeroUsize>,
    /// Flag indicating if the returned list should be protected from potential spam.
    #[serde(default)]
    pub(crate) protect_from_spam: bool,
    /// Optional filters to apply when listing the NFTs.
    pub(crate) filters: Option<NftListFilters>,
}

/// Filters that can be applied when listing NFTs to exclude potential threats or nuisances.
#[derive(Copy, Clone, Debug, Deserialize)]
pub struct NftListFilters {
    /// Exclude NFTs that are flagged as possible spam.
    #[serde(default)]
    pub(crate) exclude_spam: bool,
    /// Exclude NFTs that are flagged as phishing attempts.
    #[serde(default)]
    pub(crate) exclude_phishing: bool,
}

/// Contains parameters required to fetch metadata for a specified NFT.
#[derive(Debug, Deserialize)]
pub struct NftMetadataReq {
    /// The address of the NFT token.
    pub(crate) token_address: Address,
    /// The ID of the NFT token.
    #[serde(deserialize_with = "deserialize_token_id")]
    pub(crate) token_id: BigUint,
    /// The blockchain where the NFT exists.
    pub(crate) chain: Chain,
    /// Indicates whether to check and redact potential spam. If set to true,
    /// the internal function `protect_from_nft_spam` is utilized.
    #[serde(default)]
    pub(crate) protect_from_spam: bool,
}

/// Contains parameters required to refresh metadata for a specified NFT.
#[derive(Debug, Deserialize)]
pub struct RefreshMetadataReq {
    /// The address of the NFT token whose metadata needs to be refreshed.
    pub(crate) token_address: Address,
    /// The ID of the NFT token.
    #[serde(deserialize_with = "deserialize_token_id")]
    pub(crate) token_id: BigUint,
    /// The blockchain where the NFT exists.
    pub(crate) chain: Chain,
    /// URL to fetch the metadata.
    pub(crate) url: Url,
    /// URL used to validate if the fetched contract addresses are associated
    /// with spam contracts or if domain fields in the fetched metadata match known phishing domains.
    pub(crate) url_antispam: Url,
    #[serde(default)]
    pub(crate) komodo_proxy: bool,
}

/// Represents blockchains which are supported by NFT feature.
/// Currently there are only EVM based chains.
#[derive(Clone, Copy, Debug, EnumVariantList, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Chain {
    Avalanche,
    Bsc,
    Eth,
    Fantom,
    Polygon,
}

pub trait ConvertChain {
    fn to_ticker(&self) -> &'static str;
    fn from_ticker(s: &str) -> Result<Chain, ParseChainTypeError>;
    fn to_nft_ticker(&self) -> &'static str;
    fn from_nft_ticker(s: &str) -> Result<Chain, ParseChainTypeError>;
}

impl ConvertChain for Chain {
    #[inline(always)]
    fn to_ticker(&self) -> &'static str {
        match self {
            Chain::Avalanche => "AVAX",
            Chain::Bsc => "BNB",
            Chain::Eth => "ETH",
            Chain::Fantom => "FTM",
            Chain::Polygon => "MATIC",
        }
    }

    /// Converts a coin ticker string to a `Chain` enum.
    #[inline(always)]
    fn from_ticker(s: &str) -> Result<Chain, ParseChainTypeError> {
        match s {
            "AVAX" | "avax" => Ok(Chain::Avalanche),
            "BNB" | "bnb" => Ok(Chain::Bsc),
            "ETH" | "eth" => Ok(Chain::Eth),
            "FTM" | "ftm" => Ok(Chain::Fantom),
            "MATIC" | "matic" => Ok(Chain::Polygon),
            _ => Err(ParseChainTypeError::UnsupportedChainType),
        }
    }

    #[inline(always)]
    fn to_nft_ticker(&self) -> &'static str {
        match self {
            Chain::Avalanche => "NFT_AVAX",
            Chain::Bsc => "NFT_BNB",
            Chain::Eth => "NFT_ETH",
            Chain::Fantom => "NFT_FTM",
            Chain::Polygon => "NFT_MATIC",
        }
    }

    /// Converts a NFT ticker string to a `Chain` enum.
    #[inline(always)]
    fn from_nft_ticker(s: &str) -> Result<Chain, ParseChainTypeError> {
        match s.to_uppercase().as_str() {
            "NFT_AVAX" => Ok(Chain::Avalanche),
            "NFT_BNB" => Ok(Chain::Bsc),
            "NFT_ETH" => Ok(Chain::Eth),
            "NFT_FTM" => Ok(Chain::Fantom),
            "NFT_MATIC" => Ok(Chain::Polygon),
            _ => Err(ParseChainTypeError::UnsupportedChainType),
        }
    }
}

impl fmt::Display for Chain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Chain::Avalanche => write!(f, "AVALANCHE"),
            Chain::Bsc => write!(f, "BSC"),
            Chain::Eth => write!(f, "ETH"),
            Chain::Fantom => write!(f, "FANTOM"),
            Chain::Polygon => write!(f, "POLYGON"),
        }
    }
}

impl FromStr for Chain {
    type Err = ParseChainTypeError;

    /// Converts a string slice to a `Chain` enum.
    /// This implementation is primarily used in the context of deserialization with Serde.
    #[inline(always)]
    fn from_str(s: &str) -> Result<Chain, ParseChainTypeError> {
        match s {
            "AVALANCHE" | "avalanche" => Ok(Chain::Avalanche),
            "BSC" | "bsc" => Ok(Chain::Bsc),
            "ETH" | "eth" => Ok(Chain::Eth),
            "FANTOM" | "fantom" => Ok(Chain::Fantom),
            "POLYGON" | "polygon" => Ok(Chain::Polygon),
            _ => Err(ParseChainTypeError::UnsupportedChainType),
        }
    }
}

/// This implementation will use `FromStr` to deserialize `Chain`.
impl<'de> Deserialize<'de> for Chain {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

/// Represents the type of smart contract used for NFTs.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ContractType {
    /// Represents an ERC-1155 contract, which allows a single contract to manage
    /// multiple types of NFTs. This means ERC-1155 represents both fungible and non-fungible assets within a single contract.
    /// ERC-1155 provides a way for each token ID to represent multiple assets.
    Erc1155,
    /// Represents an ERC-721 contract, a standard for non-fungible tokens on EVM based chains,
    /// where each token is unique and owned individually.
    Erc721,
}

impl FromStr for ContractType {
    type Err = ParseContractTypeError;

    #[inline]
    fn from_str(s: &str) -> Result<ContractType, ParseContractTypeError> {
        match s {
            "ERC1155" => Ok(ContractType::Erc1155),
            "ERC721" => Ok(ContractType::Erc721),
            _ => Err(ParseContractTypeError::UnsupportedContractType),
        }
    }
}

impl fmt::Display for ContractType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ContractType::Erc1155 => write!(f, "ERC1155"),
            ContractType::Erc721 => write!(f, "ERC721"),
        }
    }
}

/// `UriMeta` structure is the object which we create from `token_uri` and `metadata`.
///
/// `token_uri` and `metadata` usually contain either `image` or `image_url` with image url.
/// But most often nft creators use only `image` name for this value (from my observation),
/// less often they use both parameters with the same url.
///
/// I suspect this is because some APIs only look for one of these image url names, so nft creators try to satisfy all sides.
/// In any case, since there is no clear standard, we have to look for both options,
/// when we build `UriMeta` from `token_uri` or `metadata`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct UriMeta {
    #[serde(rename = "image")]
    pub(crate) raw_image_url: Option<String>,
    pub(crate) image_url: Option<String>,
    pub(crate) image_domain: Option<String>,
    #[serde(rename = "name")]
    pub(crate) token_name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) attributes: Option<Json>,
    pub(crate) animation_url: Option<String>,
    pub(crate) animation_domain: Option<String>,
    pub(crate) external_url: Option<String>,
    pub(crate) external_domain: Option<String>,
    pub(crate) image_details: Option<Json>,
}

impl UriMeta {
    /// `try_to_fill_missing_fields_from` function doesnt change `raw_image_url` field.
    /// It tries to update `image_url` field instead, if it is None.
    /// As `image` is the original name of `raw_image_url` field in data from `token_uri` or `metadata`,
    /// try to find **Some()** in this field first.
    pub(crate) fn try_to_fill_missing_fields_from(&mut self, other: UriMeta) {
        if self.image_url.is_none() {
            self.image_url = other.raw_image_url.or(other.image_url);
        }
        if self.token_name.is_none() {
            self.token_name = other.token_name;
        }
        if self.description.is_none() {
            self.description = other.description;
        }
        if self.attributes.is_none() {
            self.attributes = other.attributes;
        }
        if self.animation_url.is_none() {
            self.animation_url = other.animation_url;
        }
        if self.external_url.is_none() {
            self.external_url = other.external_url;
        }
        if self.image_details.is_none() {
            self.image_details = other.image_details;
        }
    }
}

/// [`NftCommon`] structure contains common fields from [`Nft`] and [`NftFromMoralis`]
/// The `possible_spam` field indicates if any potential spam has been detected.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NftCommon {
    pub(crate) token_address: Address,
    pub(crate) amount: BigDecimal,
    pub(crate) owner_of: Address,
    pub(crate) token_hash: Option<String>,
    #[serde(rename = "name")]
    pub(crate) collection_name: Option<String>,
    pub(crate) symbol: Option<String>,
    pub(crate) token_uri: Option<String>,
    pub(crate) token_domain: Option<String>,
    pub(crate) metadata: Option<String>,
    pub(crate) last_token_uri_sync: Option<String>,
    pub(crate) last_metadata_sync: Option<String>,
    pub(crate) minter_address: Option<String>,
    #[serde(default)]
    pub(crate) possible_spam: bool,
}

/// Represents an NFT with specific chain details, contract type, and other relevant attributes.
/// This structure captures detailed information about an NFT. The `possible_phishing`
/// field indicates if any domains associated with the NFT have been marked as phishing.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Nft {
    #[serde(flatten)]
    pub(crate) common: NftCommon,
    pub(crate) chain: Chain,
    #[serde(serialize_with = "serialize_token_id", deserialize_with = "deserialize_token_id")]
    pub(crate) token_id: BigUint,
    pub(crate) block_number_minted: Option<u64>,
    pub(crate) block_number: u64,
    pub(crate) contract_type: ContractType,
    #[serde(default)]
    pub(crate) possible_phishing: bool,
    pub(crate) uri_meta: UriMeta,
}

pub(crate) struct BuildNftFields {
    pub(crate) token_address: Address,
    pub(crate) token_id: BigUint,
    pub(crate) amount: BigDecimal,
    pub(crate) owner_of: Address,
    pub(crate) contract_type: ContractType,
    pub(crate) possible_spam: bool,
    pub(crate) chain: Chain,
    pub(crate) block_number: u64,
}

pub(crate) fn build_nft_with_empty_meta(nft_fields: BuildNftFields) -> Nft {
    Nft {
        common: NftCommon {
            token_address: nft_fields.token_address,
            amount: nft_fields.amount,
            owner_of: nft_fields.owner_of,
            token_hash: None,
            collection_name: None,
            symbol: None,
            token_uri: None,
            token_domain: None,
            metadata: None,
            last_token_uri_sync: None,
            last_metadata_sync: None,
            minter_address: None,
            possible_spam: nft_fields.possible_spam,
        },
        chain: nft_fields.chain,
        token_id: nft_fields.token_id,
        block_number_minted: None,
        block_number: nft_fields.block_number,
        contract_type: nft_fields.contract_type,
        possible_phishing: false,
        uri_meta: Default::default(),
    }
}

/// Represents an NFT structure specifically for deserialization from Moralis's JSON response.
///
/// This structure is adapted to the specific format provided by Moralis's API.
#[derive(Debug, Deserialize)]
pub(crate) struct NftFromMoralis {
    #[serde(flatten)]
    pub(crate) common: NftCommon,
    pub(crate) block_number_minted: Option<SerdeStringWrap<u64>>,
    pub(crate) block_number: SerdeStringWrap<u64>,
    pub(crate) contract_type: Option<ContractType>,
    pub(crate) token_id: SerdeStringWrap<BigUint>,
}

#[derive(Debug)]
pub(crate) struct SerdeStringWrap<T>(pub(crate) T);

impl<'de, T> Deserialize<'de> for SerdeStringWrap<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Debug + std::fmt::Display,
{
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value: &str = Deserialize::deserialize(deserializer)?;
        let value: T = match value.parse() {
            Ok(v) => v,
            Err(e) => return Err(<D::Error as serde::de::Error>::custom(e)),
        };
        Ok(SerdeStringWrap(value))
    }
}

impl<T> std::ops::Deref for SerdeStringWrap<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Represents a detailed list of NFTs, including the total number of NFTs and the number of skipped NFTs.
/// It is used as response of `get_nft_list` if it is successful.
#[derive(Debug, Serialize)]
pub struct NftList {
    pub(crate) nfts: Vec<Nft>,
    pub(crate) skipped: usize,
    pub(crate) total: usize,
}

/// Parameters for withdrawing an ERC-1155 token.
#[derive(Clone, Deserialize)]
pub struct WithdrawErc1155 {
    /// The blockchain network to perform the withdrawal on.
    pub(crate) chain: Chain,
    /// The address to send the NFT to.
    pub(crate) to: String,
    /// The address of the ERC-1155 token contract.
    pub(crate) token_address: String,
    /// The unique identifier of the NFT to withdraw.
    #[serde(deserialize_with = "deserialize_token_id")]
    pub(crate) token_id: BigUint,
    /// Optional amount of the token to withdraw. Defaults to 1 if not specified.
    #[serde(deserialize_with = "deserialize_opt_biguint")]
    pub(crate) amount: Option<BigUint>,
    /// If set to `true`, withdraws the maximum amount available. Overrides the `amount` field.
    #[serde(default)]
    pub(crate) max: bool,
    /// Optional details for the withdrawal fee.
    pub(crate) fee: Option<WithdrawFee>,
}

/// Parameters for withdrawing an ERC-721 token.
#[derive(Clone, Deserialize)]
pub struct WithdrawErc721 {
    /// The blockchain network to perform the withdrawal on.
    pub(crate) chain: Chain,
    /// The address to send the NFT to.
    pub(crate) to: String,
    /// The address of the ERC-721 token contract.
    pub(crate) token_address: String,
    /// The unique identifier of the NFT to withdraw.
    #[serde(deserialize_with = "deserialize_token_id")]
    pub(crate) token_id: BigUint,
    /// Optional details for the withdrawal fee.
    pub(crate) fee: Option<WithdrawFee>,
}

/// Represents a request for withdrawing an NFT, supporting different ERC standards.
#[derive(Clone, Deserialize)]
#[serde(tag = "type", content = "withdraw_data")]
#[serde(rename_all = "snake_case")]
pub enum WithdrawNftReq {
    /// Parameters for withdrawing an ERC-1155 token.
    WithdrawErc1155(WithdrawErc1155),
    /// Parameters for withdrawing an ERC-721 token.
    WithdrawErc721(WithdrawErc721),
}

/// Details of NFT transaction.
///
/// Includes the raw transaction hex for broadcasting, along with additional information about the transaction.
#[derive(Debug, Deserialize, Serialize)]
pub struct TransactionNftDetails {
    /// Raw bytes of signed transaction, this should be sent as is to `send_raw_transaction` RPC to broadcast the transaction
    pub(crate) tx_hex: BytesJson,
    pub(crate) tx_hash: String,
    /// NFTs are sent from these addresses
    pub(crate) from: Vec<String>,
    /// NFTs are sent to these addresses
    pub(crate) to: Vec<String>,
    pub(crate) contract_type: ContractType,
    pub(crate) token_address: String,
    #[serde(serialize_with = "serialize_token_id")]
    pub(crate) token_id: BigUint,
    pub(crate) amount: BigUint,
    pub(crate) fee_details: Option<TxFeeDetails>,
    /// The coin transaction belongs to
    pub(crate) coin: String,
    /// Block height
    pub(crate) block_height: u64,
    /// Transaction timestamp
    pub(crate) timestamp: u64,
    /// Internal MM2 id used for internal transaction identification, for some coins it might be equal to transaction hash
    pub(crate) internal_id: i64,
    /// Type of transactions, default is StandardTransfer
    #[serde(default)]
    pub(crate) transaction_type: TransactionType,
}

/// Represents a request to fetch the transfer history of NFTs owned by the user across specified chains.
///
/// The request provides options such as pagination, limiting the number of results,
/// and applying specific filters to the history.
#[derive(Debug, Deserialize)]
pub struct NftTransfersReq {
    /// List of chains to fetch the NFT transfer history from.
    pub(crate) chains: Vec<Chain>,
    /// Optional filters to apply when fetching the NFT transfer history.
    pub(crate) filters: Option<NftTransferHistoryFilters>,
    /// Parameter indicating if the maximum number of transfer records should be fetched.
    /// If true, then `limit` will be ignored.
    #[serde(default)]
    pub(crate) max: bool,
    /// Limit to the number of transfer records returned in a single request.
    #[serde(default = "ten")]
    pub(crate) limit: usize,
    /// Page number for pagination.
    pub(crate) page_number: Option<NonZeroUsize>,
    /// Flag indicating if the returned transfer history should be protected from potential spam.
    #[serde(default)]
    pub(crate) protect_from_spam: bool,
}

#[derive(Debug, Display)]
pub(crate) enum ParseTransferStatusError {
    UnsupportedTransferStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) enum TransferStatus {
    Receive,
    Send,
}

impl FromStr for TransferStatus {
    type Err = ParseTransferStatusError;

    #[inline]
    fn from_str(s: &str) -> Result<TransferStatus, ParseTransferStatusError> {
        match s {
            "Receive" => Ok(TransferStatus::Receive),
            "Send" => Ok(TransferStatus::Send),
            _ => Err(ParseTransferStatusError::UnsupportedTransferStatus),
        }
    }
}

impl fmt::Display for TransferStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            TransferStatus::Receive => write!(f, "Receive"),
            TransferStatus::Send => write!(f, "Send"),
        }
    }
}

/// [`NftTransferCommon`] structure contains common fields from [`NftTransferHistory`] and [`NftTransferHistoryFromMoralis`]
/// The `possible_spam` field indicates if any potential spam has been detected.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NftTransferCommon {
    pub(crate) block_hash: Option<String>,
    /// Transaction hash in hexadecimal format
    pub(crate) transaction_hash: String,
    pub(crate) transaction_index: Option<u32>,
    pub(crate) log_index: u32,
    pub(crate) value: Option<BigDecimal>,
    pub(crate) transaction_type: Option<String>,
    pub(crate) token_address: Address,
    pub(crate) from_address: Address,
    pub(crate) to_address: Address,
    pub(crate) amount: BigDecimal,
    pub(crate) verified: Option<u32>,
    pub(crate) operator: Option<String>,
    #[serde(default)]
    pub(crate) possible_spam: bool,
}

/// Represents the historical transfer details of an NFT.
///
/// Contains relevant information about the NFT transfer such as the chain, block details,
/// and contract type. Additionally, fields like `collection_name`, `token_name`, and
/// urls to metadata provide insight into the NFT's identity. The `possible_phishing`
/// field indicates if any domains associated with the NFT have been marked as phishing.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NftTransferHistory {
    #[serde(flatten)]
    pub(crate) common: NftTransferCommon,
    pub(crate) chain: Chain,
    #[serde(serialize_with = "serialize_token_id", deserialize_with = "deserialize_token_id")]
    pub(crate) token_id: BigUint,
    pub(crate) block_number: u64,
    pub(crate) block_timestamp: u64,
    pub(crate) contract_type: ContractType,
    pub(crate) token_uri: Option<String>,
    pub(crate) token_domain: Option<String>,
    pub(crate) collection_name: Option<String>,
    pub(crate) image_url: Option<String>,
    pub(crate) image_domain: Option<String>,
    pub(crate) token_name: Option<String>,
    pub(crate) status: TransferStatus,
    #[serde(default)]
    pub(crate) possible_phishing: bool,
    pub(crate) fee_details: Option<EthTxFeeDetails>,
    pub(crate) confirmations: u64,
}

/// Represents an NFT transfer structure specifically for deserialization from Moralis's JSON response.
///
/// This structure is adapted to the specific format provided by Moralis's API.
#[derive(Debug, Deserialize)]
pub(crate) struct NftTransferHistoryFromMoralis {
    #[serde(flatten)]
    pub(crate) common: NftTransferCommon,
    pub(crate) block_number: SerdeStringWrap<u64>,
    pub(crate) block_timestamp: String,
    pub(crate) contract_type: Option<ContractType>,
    pub(crate) token_id: SerdeStringWrap<BigUint>,
}

/// Represents the detailed transfer history of NFTs, including the total number of transfers
/// and the number of skipped transfers.
/// It is used as a response of `get_nft_transfers` if it is successful.
#[derive(Debug, Serialize)]
pub struct NftsTransferHistoryList {
    pub(crate) transfer_history: Vec<NftTransferHistory>,
    pub(crate) skipped: usize,
    pub(crate) total: usize,
}

/// Filters that can be applied to the NFT transfer history.
///
/// Allows filtering based on transaction type (send/receive), date range,
/// and whether to exclude spam or phishing-related transfers.
#[derive(Copy, Clone, Debug, Deserialize)]
pub struct NftTransferHistoryFilters {
    #[serde(default)]
    pub(crate) receive: bool,
    #[serde(default)]
    pub(crate) send: bool,
    pub(crate) from_date: Option<u64>,
    pub(crate) to_date: Option<u64>,
    #[serde(default)]
    pub(crate) exclude_spam: bool,
    #[serde(default)]
    pub(crate) exclude_phishing: bool,
}

/// Contains parameters required to update NFT transfer history and NFT list.
#[derive(Debug, Deserialize)]
pub struct UpdateNftReq {
    /// A list of blockchains for which the NFTs need to be updated.
    pub(crate) chains: Vec<Chain>,
    /// URL to fetch the NFT data.
    pub(crate) url: Url,
    /// URL used to validate if the fetched contract addresses are associated
    /// with spam contracts or if domain fields in the fetched metadata match known phishing domains.
    pub(crate) url_antispam: Url,
    #[serde(default)]
    pub(crate) komodo_proxy: bool,
}

/// Represents a unique identifier for an NFT, consisting of its token address and token ID.
#[derive(Debug, Deserialize, Eq, Hash, PartialEq)]
pub struct NftTokenAddrId {
    /// The address of the NFT token contract.
    pub(crate) token_address: String,
    /// The unique identifier of the NFT within its contract.
    pub(crate) token_id: BigUint,
}

/// Holds metadata information for an NFT transfer.
#[derive(Debug)]
pub struct TransferMeta {
    /// The address of the NFT token contract.
    pub(crate) token_address: String,
    /// The unique identifier of the NFT.
    pub(crate) token_id: BigUint,
    /// Optional URI for the NFT's metadata.
    pub(crate) token_uri: Option<String>,
    /// Optional domain associated with the NFT's metadata.
    pub(crate) token_domain: Option<String>,
    /// Optional name of the NFT's collection.
    pub(crate) collection_name: Option<String>,
    /// Optional URL for the NFT's image.
    pub(crate) image_url: Option<String>,
    /// Optional domain for the NFT's image.
    pub(crate) image_domain: Option<String>,
    /// Optional name of the NFT.
    pub(crate) token_name: Option<String>,
}

impl From<Nft> for TransferMeta {
    fn from(nft_db: Nft) -> Self {
        TransferMeta {
            token_address: nft_db.common.token_address.addr_to_string(),
            token_id: nft_db.token_id,
            token_uri: nft_db.common.token_uri,
            token_domain: nft_db.common.token_domain,
            collection_name: nft_db.common.collection_name,
            image_url: nft_db.uri_meta.image_url,
            image_domain: nft_db.uri_meta.image_domain,
            token_name: nft_db.uri_meta.token_name,
        }
    }
}

/// The primary context for NFT operations within the MM environment.
///
/// This struct provides an interface for interacting with the underlying data structures
/// required for NFT operations, including guarding against concurrent accesses and
/// dealing with platform-specific storage mechanisms.
pub(crate) struct NftCtx {
    /// Platform-specific database for caching NFT data.
    #[cfg(target_arch = "wasm32")]
    pub(crate) nft_cache_db: SharedDb<NftCacheIDB>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) nft_cache_db: Arc<AsyncMutex<AsyncConnection>>,
}

impl NftCtx {
    /// Create a new `NftCtx` from the given MM context.
    ///
    /// If an `NftCtx` instance doesn't already exist in the MM context, it gets created and cached for subsequent use.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn from_ctx(ctx: &MmArc) -> Result<Arc<NftCtx>, String> {
        from_ctx(&ctx.nft_ctx, move || {
            let async_sqlite_connection = ctx
                .async_sqlite_connection
                .get()
                .ok_or("async_sqlite_connection is not initialized".to_owned())?;
            Ok(NftCtx {
                nft_cache_db: async_sqlite_connection.clone(),
            })
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn from_ctx(ctx: &MmArc) -> Result<Arc<NftCtx>, String> {
        Ok(try_s!(from_ctx(&ctx.nft_ctx, move || {
            Ok(NftCtx {
                nft_cache_db: ConstructibleDb::new(ctx).into_shared(),
            })
        })))
    }

    /// Lock database to guard against concurrent NFT operations, ensuring data consistency.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn lock_db(
        &self,
    ) -> MmResult<impl NftListStorageOps + NftTransferHistoryStorageOps + NftMigrationOps + '_, LockDBError> {
        Ok(self.nft_cache_db.lock().await)
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn lock_db(
        &self,
    ) -> MmResult<impl NftListStorageOps + NftTransferHistoryStorageOps + '_, LockDBError> {
        self.nft_cache_db
            .get_or_initialize()
            .await
            .mm_err(WasmNftCacheError::from)
            .mm_err(LockDBError::from)
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct SpamContractReq {
    pub(crate) network: Chain,
    pub(crate) addresses: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct PhishingDomainReq {
    pub(crate) domains: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SpamContractRes {
    pub(crate) result: HashMap<Address, bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PhishingDomainRes {
    pub(crate) result: HashMap<String, bool>,
}

fn serialize_token_id<S>(token_id: &BigUint, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let token_id_str = token_id.to_string();
    serializer.serialize_str(&token_id_str)
}

fn deserialize_token_id<'de, D>(deserializer: D) -> Result<BigUint, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    BigUint::from_str(&s).map_err(serde::de::Error::custom)
}

/// Custom deserialization function for optional BigUint.
fn deserialize_opt_biguint<'de, D>(deserializer: D) -> Result<Option<BigUint>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    if let Some(s) = opt {
        BigUint::from_str(&s).map(Some).map_err(serde::de::Error::custom)
    } else {
        Ok(None)
    }
}

/// Request parameters for clearing NFT data from the database.
#[derive(Debug, Deserialize)]
pub struct ClearNftDbReq {
    /// Specifies the blockchain networks (e.g., Ethereum, BSC) to clear NFT data.
    #[serde(default)]
    pub(crate) chains: Vec<Chain>,
    /// If `true`, clears NFT data for all chains, ignoring the `chains` field. Defaults to `false`.
    #[serde(default)]
    pub(crate) clear_all: bool,
}

/// Represents detailed information about a Non-Fungible Token (NFT).
/// This struct is used to keep info about NFTs owned by user in global Non-Fungible Token.
#[derive(Clone, Debug, Serialize)]
pub struct NftInfo {
    /// The address of the NFT token.
    pub token_address: Address,
    /// The ID of the NFT token.
    #[serde(serialize_with = "serialize_token_id")]
    pub token_id: BigUint,
    /// The blockchain where the NFT exists.
    pub chain: Chain,
    /// The type of smart contract that governs this NFT.
    pub contract_type: ContractType,
    /// The quantity of this type of NFT owned. Particularly relevant for ERC-1155 tokens,
    /// where a single token ID can represent multiple assets.
    pub amount: BigDecimal,
}
