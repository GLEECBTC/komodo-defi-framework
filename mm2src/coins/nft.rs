use http::Uri;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::{MmError, MmResult, MmResultExt};
use mm2_p2p::p2p_ctx::P2PContext;
use proxy_signature::{ProxySign, RawMessage};
use url::Url;

pub(crate) mod nft_errors;
pub mod nft_structs;
pub(crate) mod storage;

#[cfg(any(test, target_arch = "wasm32"))]
mod nft_tests;

use crate::hd_wallet::AddrToString;
use crate::{
    lp_coinfind_or_err, CoinWithDerivationMethod, CoinsContext, MarketCoinOps, MmCoinEnum, MmCoinStruct, WithdrawError,
};
use nft_errors::{GetNftInfoError, UpdateNftError};
use nft_structs::{
    Chain, ContractType, ConvertChain, Nft, NftFromMoralis, NftList, NftListReq, NftMetadataReq, NftTransferHistory,
    NftTransferHistoryFromMoralis, NftTransfersReq, NftsTransferHistoryList, TransactionNftDetails, UpdateNftReq,
    WithdrawNftReq,
};

use crate::eth::{withdraw_erc1155, withdraw_erc721, EthCoin, EthCoinType, EthTxFeeDetails, PayForGasOption};
use crate::nft::nft_errors::{
    ClearNftDbError, MetaFromUrlError, ProtectFromSpamError, TransferConfirmationsError, UpdateSpamPhishingError,
};
use crate::nft::nft_structs::{
    build_nft_with_empty_meta, BuildNftFields, ClearNftDbReq, NftCommon, NftCtx, NftInfo, NftTransferCommon,
    PhishingDomainReq, PhishingDomainRes, RefreshMetadataReq, SpamContractReq, SpamContractRes, TransferMeta,
    TransferStatus, UriMeta,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::nft::storage::NftMigrationOps;
use crate::nft::storage::{NftListStorageOps, NftTransferHistoryStorageOps};
use common::log::error;
use common::parse_rfc3339_to_timestamp;
use ethereum_types::{Address, H256};
use futures::compat::Future01CompatExt;
use futures::future::try_join_all;
use mm2_err_handle::map_to_mm::MapToMmResult;
use mm2_net::transport::send_post_request_to_uri;
use mm2_number::BigUint;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value as Json;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use web3::types::TransactionId;

#[cfg(not(target_arch = "wasm32"))]
use mm2_net::native_http::send_request_to_uri;

#[cfg(target_arch = "wasm32")]
use mm2_net::wasm::http::send_request_to_uri;

const MORALIS_API: &str = "api";
const MORALIS_ENDPOINT_V: &str = "v2";
/// query parameters for moralis request: The format of the token ID
const MORALIS_FORMAT_QUERY_NAME: &str = "format";
const MORALIS_FORMAT_QUERY_VALUE: &str = "decimal";
/// The minimum block number from which to get the transfers
const MORALIS_FROM_BLOCK_QUERY_NAME: &str = "from_block";

const BLOCKLIST_ENDPOINT: &str = "api/blocklist";
const BLOCKLIST_CONTRACT: &str = "contract";
const BLOCKLIST_DOMAIN: &str = "domain";
const BLOCKLIST_SCAN: &str = "scan";

/// `WithdrawNftResult` type represents the result of an NFT withdrawal operation. On success, it provides the details
/// of the generated transaction meant for transferring the NFT. On failure, it details the encountered error.
pub type WithdrawNftResult = Result<TransactionNftDetails, MmError<WithdrawError>>;

/// Fetches a list of user-owned NFTs across specified chains.
///
/// The function aggregates NFTs based on provided chains, supports pagination, and
/// allows for result limits and filters. If the `protect_from_spam` flag is true,
/// NFTs are checked and redacted for potential spam.
///
/// # Parameters
///
/// * `ctx`: Shared context with configurations/resources.
/// * `req`: Request specifying chains, pagination, and filters.
///
/// # Returns
///
/// On success, returns a detailed `NftList` containing NFTs, total count, and skipped count.
/// # Errors
///
/// Returns `GetNftInfoError` variants for issues like invalid requests, transport failures,
/// database errors, and spam protection errors.
pub async fn get_nft_list(ctx: MmArc, req: NftListReq) -> MmResult<NftList, GetNftInfoError> {
    let nft_ctx = NftCtx::from_ctx(&ctx).map_to_mm(GetNftInfoError::Internal)?;

    let storage = nft_ctx.lock_db().await.map_mm_err()?;
    for chain in req.chains.iter() {
        if !NftListStorageOps::is_initialized(&storage, chain).await.map_mm_err()? {
            NftListStorageOps::init(&storage, chain).await.map_mm_err()?;
        }
    }
    let mut nft_list = storage
        .get_nft_list(req.chains, req.max, req.limit, req.page_number, req.filters)
        .await
        .map_mm_err()?;
    if req.protect_from_spam {
        for nft in &mut nft_list.nfts {
            protect_from_nft_spam_links(nft, true).map_mm_err()?;
        }
    }
    Ok(nft_list)
}

/// Retrieves detailed metadata for a specified NFT.
///
/// The function accesses the stored NFT data, based on provided token address,
/// token ID, and chain, and returns comprehensive information about the NFT.
/// It also checks and redacts potential spam if `protect_from_spam` in the request is set to true.
pub async fn get_nft_metadata(ctx: MmArc, req: NftMetadataReq) -> MmResult<Nft, GetNftInfoError> {
    let nft_ctx = NftCtx::from_ctx(&ctx).map_to_mm(GetNftInfoError::Internal)?;

    let storage = nft_ctx.lock_db().await.map_mm_err()?;
    if !NftListStorageOps::is_initialized(&storage, &req.chain)
        .await
        .map_mm_err()?
    {
        NftListStorageOps::init(&storage, &req.chain).await.map_mm_err()?;
    }
    let mut nft = storage
        .get_nft(&req.chain, format!("{:#02x}", req.token_address), req.token_id.clone())
        .await
        .map_mm_err()?
        .ok_or_else(|| GetNftInfoError::TokenNotFoundInWallet {
            token_address: format!("{:#02x}", req.token_address),
            token_id: req.token_id.to_string(),
        })?;
    if req.protect_from_spam {
        protect_from_nft_spam_links(&mut nft, true).map_mm_err()?;
    }
    Ok(nft)
}

/// Fetches the transfer history of user-owned NFTs across specified chains.
///
/// The function aggregates NFT transfers based on provided chains, offers pagination,
/// allows for result limits, and filters. If the `protect_from_spam` flag is true,
/// the returned transfers are checked and redacted for potential spam.
///
/// # Parameters
///
/// * `ctx`: Shared context with configurations/resources.
/// * `req`: Request detailing chains, pagination, and filters for the transfer history.
///
/// # Returns
///
/// On success, returns an `NftsTransferHistoryList` containing NFT transfer details,
/// the total count, and skipped count.
///
/// # Errors
///
/// Returns `GetNftInfoError` variants for issues like invalid requests, transport failures,
/// database errors, and spam protection errors.
pub async fn get_nft_transfers(ctx: MmArc, req: NftTransfersReq) -> MmResult<NftsTransferHistoryList, GetNftInfoError> {
    let nft_ctx = NftCtx::from_ctx(&ctx).map_to_mm(GetNftInfoError::Internal)?;

    let storage = nft_ctx.lock_db().await.map_mm_err()?;
    for chain in req.chains.iter() {
        if !NftTransferHistoryStorageOps::is_initialized(&storage, chain)
            .await
            .map_mm_err()?
        {
            NftTransferHistoryStorageOps::init(&storage, chain).await.map_mm_err()?;
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            NftMigrationOps::migrate_tx_history_if_needed(&storage, chain)
                .await
                .map_mm_err()?;
        }
    }
    let mut transfer_history_list = storage
        .get_transfer_history(req.chains.clone(), req.max, req.limit, req.page_number, req.filters)
        .await
        .map_mm_err()?;
    if req.protect_from_spam {
        for transfer in &mut transfer_history_list.transfer_history {
            protect_from_history_spam_links(transfer, true).map_mm_err()?;
        }
    }
    process_transfers_confirmations(&ctx, req.chains, &mut transfer_history_list)
        .await
        .map_mm_err()?;
    Ok(transfer_history_list)
}

async fn process_transfers_confirmations(
    ctx: &MmArc,
    chains: Vec<Chain>,
    history_list: &mut NftsTransferHistoryList,
) -> MmResult<(), TransferConfirmationsError> {
    async fn current_block_impl<Coin: MarketCoinOps>(coin: Coin) -> MmResult<u64, TransferConfirmationsError> {
        coin.current_block()
            .compat()
            .await
            .map_to_mm(TransferConfirmationsError::GetCurrentBlockErr)
    }

    let futures = chains.into_iter().map(|chain| async move {
        let ticker = chain.to_ticker();
        let coin_enum = lp_coinfind_or_err(ctx, ticker).await.map_mm_err()?;
        match coin_enum {
            MmCoinEnum::EthCoin(eth_coin) => {
                let current_block = current_block_impl(eth_coin).await?;
                Ok((ticker, current_block))
            },
            _ => MmError::err(TransferConfirmationsError::CoinDoesntSupportNft {
                coin: coin_enum.ticker().to_owned(),
            }),
        }
    });
    let blocks_map = try_join_all(futures).await?.into_iter().collect::<HashMap<_, _>>();

    for transfer in history_list.transfer_history.iter_mut() {
        let current_block = match blocks_map.get(transfer.chain.to_ticker()) {
            Some(block) => *block,
            None => 0,
        };
        transfer.confirmations = if transfer.block_number > current_block {
            0
        } else {
            current_block + 1 - transfer.block_number
        };
    }
    Ok(())
}

/// Updates NFT transfer history and NFT list in the DB.
///
/// This function refreshes the NFT transfer history and NFT list cache based on new
/// data fetched from the provided `url`. The function ensures the local cache is in
/// sync with the latest data from the source, validates against spam contract addresses and phishing domains.
pub async fn update_nft(ctx: MmArc, req: UpdateNftReq) -> MmResult<(), UpdateNftError> {
    let nft_ctx = NftCtx::from_ctx(&ctx)
        .map_to_mm(GetNftInfoError::Internal)
        .map_mm_err()?;
    let p2p_ctx = P2PContext::fetch_from_mm_arc(&ctx);

    let storage = nft_ctx.lock_db().await.map_mm_err()?;
    for chain in req.chains.iter() {
        let transfer_history_initialized = NftTransferHistoryStorageOps::is_initialized(&storage, chain)
            .await
            .map_mm_err()?;

        let from_block = if transfer_history_initialized {
            #[cfg(not(target_arch = "wasm32"))]
            NftMigrationOps::migrate_tx_history_if_needed(&storage, chain)
                .await
                .map_mm_err()?;
            let last_transfer_block = NftTransferHistoryStorageOps::get_last_block_number(&storage, chain)
                .await
                .map_mm_err()?;
            last_transfer_block.map(|b| b + 1)
        } else {
            NftTransferHistoryStorageOps::init(&storage, chain).await.map_mm_err()?;
            None
        };
        let coin_enum = lp_coinfind_or_err(&ctx, chain.to_nft_ticker()).await.map_mm_err()?;
        let global_nft = match coin_enum {
            MmCoinEnum::EthCoin(eth_coin) => eth_coin,
            _ => {
                return MmError::err(UpdateNftError::CoinDoesntSupportNft {
                    coin: coin_enum.ticker().to_owned(),
                })
            },
        };
        let my_address = global_nft.derivation_method().single_addr_or_err().await.map_mm_err()?;
        let my_address_str = my_address.addr_to_string();
        let proxy_sign = if req.komodo_proxy {
            let uri = Uri::from_str(req.url.as_ref()).map_err(|e| UpdateNftError::Internal(e.to_string()))?;
            let proxy_sign = RawMessage::sign(p2p_ctx.keypair(), &uri, 0, common::PROXY_REQUEST_EXPIRATION_SEC)
                .map_err(|e| UpdateNftError::Internal(e.to_string()))?;
            Some(proxy_sign)
        } else {
            None
        };

        let wrapper = UrlSignWrapper {
            chain,
            orig_url: &req.url,
            url_antispam: &req.url_antispam,
            proxy_sign,
        };

        let nft_transfers = get_moralis_nft_transfers(from_block, global_nft, &my_address_str, &wrapper)
            .await
            .map_mm_err()?;
        storage
            .add_transfers_to_history(*chain, nft_transfers)
            .await
            .map_mm_err()?;

        let nft_block = match NftListStorageOps::get_last_block_number(&storage, chain).await {
            Ok(Some(block)) => block,
            Ok(None) => {
                // if there are no rows in NFT LIST table we can try to get nft list from moralis.
                let nft_list = cache_nfts_from_moralis(&my_address_str, &storage, &wrapper).await?;
                update_meta_in_transfers(&storage, chain, nft_list).await?;
                update_transfers_with_empty_meta(&storage, &wrapper).await?;
                update_spam(&storage, *chain, &req.url_antispam).await.map_mm_err()?;
                update_phishing(&storage, chain, &req.url_antispam).await.map_mm_err()?;
                continue;
            },
            Err(_) => {
                // if there is an error, then NFT LIST table doesn't exist, so we need to cache nft list from moralis.
                NftListStorageOps::init(&storage, chain).await.map_mm_err()?;
                let nft_list = cache_nfts_from_moralis(&my_address_str, &storage, &wrapper).await?;
                update_meta_in_transfers(&storage, chain, nft_list).await?;
                update_transfers_with_empty_meta(&storage, &wrapper).await?;
                update_spam(&storage, *chain, &req.url_antispam).await.map_mm_err()?;
                update_phishing(&storage, chain, &req.url_antispam).await.map_mm_err()?;
                continue;
            },
        };
        let scanned_block = storage
            .get_last_scanned_block(chain)
            .await
            .map_mm_err()?
            .ok_or_else(|| UpdateNftError::LastScannedBlockNotFound {
                last_nft_block: nft_block.to_string(),
            })?;
        // if both block numbers exist, last scanned block should be equal
        // or higher than last block number from NFT LIST table.
        if scanned_block < nft_block {
            return MmError::err(UpdateNftError::InvalidBlockOrder {
                last_scanned_block: scanned_block.to_string(),
                last_nft_block: nft_block.to_string(),
            });
        }
        update_nft_list(&storage, scanned_block + 1, &my_address_str, &wrapper).await?;
        update_nft_global_in_coins_ctx(&ctx, &storage, *chain).await?;
        update_transfers_with_empty_meta(&storage, &wrapper).await?;
        update_spam(&storage, *chain, &req.url_antispam).await.map_mm_err()?;
        update_phishing(&storage, chain, &req.url_antispam).await.map_mm_err()?;
    }
    Ok(())
}

/// Updates the global NFT information in the coins context.
///
/// This function uses the up-to-date NFT list for a given chain and updates the
/// corresponding global NFT information in the coins context.
async fn update_nft_global_in_coins_ctx<T>(ctx: &MmArc, storage: &T, chain: Chain) -> MmResult<(), UpdateNftError>
where
    T: NftListStorageOps + NftTransferHistoryStorageOps,
{
    let coins_ctx = CoinsContext::from_ctx(ctx).map_to_mm(UpdateNftError::Internal)?;
    let mut coins = coins_ctx.lock_coins().await;
    let ticker = chain.to_nft_ticker();

    if let Some(MmCoinStruct {
        inner: MmCoinEnum::EthCoin(nft_global),
        ..
    }) = coins.get_mut(ticker)
    {
        let nft_list = storage
            .get_nft_list(vec![chain], true, 1, None, None)
            .await
            .map_mm_err()?;
        update_nft_infos(nft_global, nft_list.nfts).await;
        return Ok(());
    }
    // if global NFT is None in CoinsContext, then it's just not activated
    Ok(())
}

/// Updates the global NFT information with the latest NFT list.
///
/// This function replaces the existing NFT information (`nfts_infos`) in the global NFT with the new data provided by `nft_list`.
/// The `nft_list` must be current, accurately reflecting the NFTs presently owned by the user.
/// This includes accounting for any changes such as NFTs that have been transferred away, so user is not owner anymore,
/// or changes in the amounts of ERC1155 tokens.
/// Ensuring the data's accuracy is vital for maintaining a correct representation of ownership in the global NFT.
///
/// # Warning
/// Using an outdated `nft_list` for this operation may result in incorrect NFT information persisting in the global NFT,
/// potentially leading to inconsistencies with the actual state of NFT ownership.
async fn update_nft_infos(nft_global: &mut EthCoin, nft_list: Vec<Nft>) {
    let new_nft_infos: HashMap<String, NftInfo> = nft_list
        .into_iter()
        .map(|nft| {
            let key = format!("{},{}", nft.common.token_address, nft.token_id);
            let nft_info = NftInfo {
                token_address: nft.common.token_address,
                token_id: nft.token_id,
                chain: nft.chain,
                contract_type: nft.contract_type,
                amount: nft.common.amount,
            };
            (key, nft_info)
        })
        .collect();

    let mut global_nft_infos = nft_global.nfts_infos.lock().await;
    // we can do this as some `global_nft_infos` keys may not present in `new_nft_infos`, so we will have to remove them anyway
    *global_nft_infos = new_nft_infos;
}

/// `update_spam` function updates spam contracts info in NFT list and NFT transfers.
async fn update_spam<T>(storage: &T, chain: Chain, url_antispam: &Url) -> MmResult<(), UpdateSpamPhishingError>
where
    T: NftListStorageOps + NftTransferHistoryStorageOps,
{
    let token_addresses = storage.get_token_addresses(chain).await.map_mm_err()?;
    if !token_addresses.is_empty() {
        let addresses = token_addresses
            .iter()
            .map(Address::addr_to_string)
            .collect::<Vec<_>>()
            .join(",");
        let spam_res = send_spam_request(&chain, url_antispam, addresses).await?;
        for (address, is_spam) in spam_res.result.into_iter() {
            if is_spam {
                let address_hex = address.addr_to_string();
                storage
                    .update_nft_spam_by_token_address(&chain, address_hex.clone(), is_spam)
                    .await
                    .map_mm_err()?;
                storage
                    .update_transfer_spam_by_token_address(&chain, address_hex, is_spam)
                    .await
                    .map_mm_err()?;
            }
        }
    }
    Ok(())
}

async fn update_phishing<T>(storage: &T, chain: &Chain, url_antispam: &Url) -> MmResult<(), UpdateSpamPhishingError>
where
    T: NftListStorageOps + NftTransferHistoryStorageOps,
{
    let transfer_domains = storage.get_domains(chain).await.map_mm_err()?;
    let nft_domains = storage.get_animation_external_domains(chain).await.map_mm_err()?;
    let domains: HashSet<String> = transfer_domains.union(&nft_domains).cloned().collect();
    if !domains.is_empty() {
        let domains = domains.into_iter().collect::<Vec<_>>().join(",");
        let domain_res = send_phishing_request(url_antispam, domains).await?;
        for (domain, is_phishing) in domain_res.result.into_iter() {
            if is_phishing {
                storage
                    .update_nft_phishing_by_domain(chain, domain.clone(), is_phishing)
                    .await
                    .map_mm_err()?;
                storage
                    .update_transfer_phishing_by_domain(chain, domain, is_phishing)
                    .await
                    .map_mm_err()?;
            }
        }
    }
    Ok(())
}

/// `send_spam_request` function sends request to antispam api to scan contract addresses for spam.
async fn send_spam_request(
    chain: &Chain,
    url_antispam: &Url,
    addresses: String,
) -> MmResult<SpamContractRes, UpdateSpamPhishingError> {
    let scan_contract_uri = prepare_uri_for_blocklist_endpoint(url_antispam, BLOCKLIST_CONTRACT, BLOCKLIST_SCAN)?;
    let req_spam = SpamContractReq {
        network: *chain,
        addresses,
    };
    let req_spam_json = serde_json::to_string(&req_spam)?;
    let scan_contract_res = send_post_request_to_uri(scan_contract_uri.as_str(), req_spam_json)
        .await
        .map_mm_err()?;
    let spam_res: SpamContractRes = serde_json::from_slice(&scan_contract_res)?;
    Ok(spam_res)
}

/// `send_spam_request` function sends request to antispam api to scan domains for phishing.
async fn send_phishing_request(
    url_antispam: &Url,
    domains: String,
) -> MmResult<PhishingDomainRes, UpdateSpamPhishingError> {
    let scan_contract_uri = prepare_uri_for_blocklist_endpoint(url_antispam, BLOCKLIST_DOMAIN, BLOCKLIST_SCAN)?;
    let req_phishing = PhishingDomainReq { domains };
    let req_phishing_json = serde_json::to_string(&req_phishing)?;
    let scan_domains_res = send_post_request_to_uri(scan_contract_uri.as_str(), req_phishing_json)
        .await
        .map_mm_err()?;
    let phishing_res: PhishingDomainRes = serde_json::from_slice(&scan_domains_res)?;
    Ok(phishing_res)
}

/// `prepare_uri_for_blocklist_endpoint` function constructs the URI required for the antispam API request.
/// It appends the required path segments to the given base URL and returns the completed URI.
fn prepare_uri_for_blocklist_endpoint(
    url_antispam: &Url,
    blocklist_type: &str,
    blocklist_action_or_network: &str,
) -> MmResult<Url, UpdateSpamPhishingError> {
    let mut uri = url_antispam.clone();
    uri.set_path(BLOCKLIST_ENDPOINT);
    uri.path_segments_mut()
        .map_to_mm(|_| UpdateSpamPhishingError::Internal("Invalid URI".to_string()))?
        .push(blocklist_type)
        .push(blocklist_action_or_network);
    Ok(uri)
}

/// Refreshes and updates metadata associated with a specific NFT.
///
/// The function obtains updated metadata for an NFT using its token address and token id.
/// It fetches the metadata from the provided `url` and validates it against possible spam and
/// phishing domains using the provided `url_antispam`. If the fetched metadata or its domain
/// is identified as spam or matches with any phishing domains, the NFT's `possible_spam` and/or
/// `possible_phishing` flags are set to true.
pub async fn refresh_nft_metadata(ctx: MmArc, req: RefreshMetadataReq) -> MmResult<(), UpdateNftError> {
    let nft_ctx = NftCtx::from_ctx(&ctx)
        .map_to_mm(GetNftInfoError::Internal)
        .map_mm_err()?;
    let p2p_ctx = P2PContext::fetch_from_mm_arc(&ctx);

    let storage = nft_ctx.lock_db().await.map_mm_err()?;

    let proxy_sign = if req.komodo_proxy {
        let uri = Uri::from_str(req.url.as_ref()).map_err(|e| UpdateNftError::Internal(e.to_string()))?;
        let proxy_sign = RawMessage::sign(p2p_ctx.keypair(), &uri, 0, common::PROXY_REQUEST_EXPIRATION_SEC)
            .map_err(|e| UpdateNftError::Internal(e.to_string()))?;
        Some(proxy_sign)
    } else {
        None
    };
    let wrapper = UrlSignWrapper {
        chain: &req.chain,
        orig_url: &req.url,
        url_antispam: &req.url_antispam,
        proxy_sign,
    };

    let token_address_str = req.token_address.addr_to_string();
    let mut moralis_meta = match get_moralis_metadata(token_address_str.clone(), req.token_id.clone(), &wrapper).await {
        Ok(moralis_meta) => moralis_meta,
        Err(_) => {
            storage
                .update_nft_spam_by_token_address(&req.chain, token_address_str.clone(), true)
                .await
                .map_mm_err()?;
            storage
                .update_transfer_spam_by_token_address(&req.chain, token_address_str.clone(), true)
                .await
                .map_mm_err()?;
            return Ok(());
        },
    };
    let mut nft_db = storage
        .get_nft(&req.chain, token_address_str.clone(), req.token_id.clone())
        .await
        .map_mm_err()?
        .ok_or_else(|| GetNftInfoError::TokenNotFoundInWallet {
            token_address: token_address_str,
            token_id: req.token_id.to_string(),
        })?;
    let token_uri = check_moralis_ipfs_bafy(moralis_meta.common.token_uri.as_deref());
    let token_domain = get_domain_from_url(token_uri.as_deref());
    check_token_uri(&mut moralis_meta.common.possible_spam, token_uri.as_deref()).map_mm_err()?;
    drop_mutability!(moralis_meta);
    let uri_meta = get_uri_meta(
        token_uri.as_deref(),
        moralis_meta.common.metadata.as_deref(),
        &req.url_antispam,
        moralis_meta.common.possible_spam,
        nft_db.possible_phishing,
    )
    .await;
    // Gather domains for phishing checks
    let domains = gather_domains(&token_domain, &uri_meta);
    nft_db.common.collection_name = moralis_meta.common.collection_name;
    nft_db.common.symbol = moralis_meta.common.symbol;
    nft_db.common.token_uri = token_uri;
    nft_db.common.token_domain = token_domain;
    nft_db.common.metadata = moralis_meta.common.metadata;
    nft_db.common.last_token_uri_sync = moralis_meta.common.last_token_uri_sync;
    nft_db.common.last_metadata_sync = moralis_meta.common.last_metadata_sync;
    nft_db.common.possible_spam = moralis_meta.common.possible_spam;
    nft_db.uri_meta = uri_meta;
    if !nft_db.common.possible_spam {
        refresh_possible_spam(&storage, &req.chain, &mut nft_db, &req.url_antispam).await?;
    };
    if !nft_db.possible_phishing {
        refresh_possible_phishing(&storage, &req.chain, domains, &mut nft_db, &req.url_antispam).await?;
    };
    storage
        .refresh_nft_metadata(&moralis_meta.chain, nft_db.clone())
        .await
        .map_mm_err()?;
    update_transfer_meta_using_nft(&storage, &req.chain, &mut nft_db).await?;
    Ok(())
}

/// The `update_transfer_meta_using_nft` function updates the transfer metadata associated with the given NFT.
/// If metadata info contains potential spam links, function sets `possible_spam` true.
async fn update_transfer_meta_using_nft<T>(storage: &T, chain: &Chain, nft: &mut Nft) -> MmResult<(), UpdateNftError>
where
    T: NftListStorageOps + NftTransferHistoryStorageOps,
{
    let transfer_meta = TransferMeta::from(nft.clone());
    storage
        .update_transfers_meta_by_token_addr_id(chain, transfer_meta, nft.common.possible_spam)
        .await
        .map_mm_err()?;
    Ok(())
}

/// Extracts domains from uri_meta and token_domain.
fn gather_domains(token_domain: &Option<String>, uri_meta: &UriMeta) -> HashSet<String> {
    let mut domains = HashSet::new();
    if let Some(domain) = token_domain {
        domains.insert(domain.clone());
    }
    if let Some(domain) = &uri_meta.image_domain {
        domains.insert(domain.clone());
    }
    if let Some(domain) = &uri_meta.animation_domain {
        domains.insert(domain.clone());
    }
    if let Some(domain) = &uri_meta.external_domain {
        domains.insert(domain.clone());
    }
    domains
}

/// Refreshes the `possible_spam` flag based on spam results.
async fn refresh_possible_spam<T>(
    storage: &T,
    chain: &Chain,
    nft_db: &mut Nft,
    url_antispam: &Url,
) -> MmResult<(), UpdateNftError>
where
    T: NftListStorageOps + NftTransferHistoryStorageOps,
{
    let address_hex = nft_db.common.token_address.addr_to_string();
    let spam_res = send_spam_request(chain, url_antispam, address_hex.clone())
        .await
        .map_mm_err()?;
    if let Some(true) = spam_res.result.get(&nft_db.common.token_address) {
        nft_db.common.possible_spam = true;
        storage
            .update_nft_spam_by_token_address(chain, address_hex.clone(), true)
            .await
            .map_mm_err()?;
        storage
            .update_transfer_spam_by_token_address(chain, address_hex, true)
            .await
            .map_mm_err()?;
    }
    Ok(())
}

/// Refreshes the `possible_phishing` flag based on phishing results.
async fn refresh_possible_phishing<T>(
    storage: &T,
    chain: &Chain,
    domains: HashSet<String>,
    nft_db: &mut Nft,
    url_antispam: &Url,
) -> MmResult<(), UpdateNftError>
where
    T: NftListStorageOps + NftTransferHistoryStorageOps,
{
    if !domains.is_empty() {
        let domain_list = domains.into_iter().collect::<Vec<_>>().join(",");
        let domain_res = send_phishing_request(url_antispam, domain_list).await.map_mm_err()?;
        for (domain, is_phishing) in domain_res.result.into_iter() {
            if is_phishing {
                nft_db.possible_phishing = true;
                storage
                    .update_transfer_phishing_by_domain(chain, domain.clone(), is_phishing)
                    .await
                    .map_mm_err()?;
                storage
                    .update_nft_phishing_by_domain(chain, domain, is_phishing)
                    .await
                    .map_mm_err()?;
            }
        }
    }
    Ok(())
}

async fn get_moralis_nft_list(
    wallet_address: &str,
    wrapper: &UrlSignWrapper<'_>,
) -> MmResult<Vec<Nft>, GetNftInfoError> {
    let mut res_list = Vec::new();
    let chain = wrapper.chain;
    let uri_without_cursor = construct_moralis_uri_for_nft(wrapper.orig_url, wallet_address, chain)?;

    // The cursor returned in the previous response (used for getting the next page).
    let mut cursor = String::new();
    loop {
        // Create a new URL instance from uri_without_cursor and modify its query to include the cursor if present
        let uri = format!("{uri_without_cursor}{cursor}");
        let response = build_and_send_request(uri.as_str(), &wrapper.proxy_sign).await?;
        if let Some(nfts_list) = response["result"].as_array() {
            for nft_json in nfts_list {
                let nft_moralis = NftFromMoralis::deserialize(nft_json)?;
                let contract_type = match nft_moralis.contract_type {
                    Some(contract_type) => contract_type,
                    None => continue,
                };
                let mut nft = build_nft_from_moralis(*chain, nft_moralis, contract_type, wrapper.url_antispam).await;
                protect_from_nft_spam_links(&mut nft, false).map_mm_err()?;
                // collect NFTs from the page
                res_list.push(nft);
            }
            // if cursor is not null, there are other NFTs on next page,
            // and we need to send new request with cursor to get info from the next page.
            if let Some(cursor_res) = response["cursor"].as_str() {
                cursor = format!("&cursor={cursor_res}");
                continue;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    Ok(res_list)
}

pub(crate) async fn get_nfts_for_activation(
    chain: &Chain,
    my_address: &Address,
    orig_url: &Url,
    proxy_sign: Option<ProxySign>,
) -> MmResult<HashMap<String, NftInfo>, GetNftInfoError> {
    let mut nfts_map = HashMap::new();
    let uri_without_cursor = construct_moralis_uri_for_nft(orig_url, &my_address.addr_to_string(), chain)?;

    // The cursor returned in the previous response (used for getting the next page).
    let mut cursor = String::new();
    loop {
        // Create a new URL instance from uri_without_cursor and modify its query to include the cursor if present
        let uri = format!("{uri_without_cursor}{cursor}");
        let response = build_and_send_request(uri.as_str(), &proxy_sign).await?;
        if let Some(nfts_list) = response["result"].as_array() {
            process_nft_list_for_activation(nfts_list, chain, &mut nfts_map)?;
            // if cursor is not null, there are other NFTs on next page,
            // and we need to send new request with cursor to get info from the next page.
            if let Some(cursor_res) = response["cursor"].as_str() {
                cursor = format!("&cursor={cursor_res}");
                continue;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    Ok(nfts_map)
}

fn process_nft_list_for_activation(
    nfts_list: &[Json],
    chain: &Chain,
    nfts_map: &mut HashMap<String, NftInfo>,
) -> MmResult<(), GetNftInfoError> {
    for nft_json in nfts_list {
        let nft_moralis = NftFromMoralis::deserialize(nft_json)?;
        let contract_type = match nft_moralis.contract_type {
            Some(contract_type) => contract_type,
            None => continue,
        };
        let token_address_str = nft_moralis.common.token_address.addr_to_string();
        let nft_info = NftInfo {
            token_address: nft_moralis.common.token_address,
            token_id: nft_moralis.token_id.0.clone(),
            chain: *chain,
            contract_type,
            amount: nft_moralis.common.amount,
        };
        let key = format!("{},{}", token_address_str, nft_moralis.token_id.0);
        nfts_map.insert(key, nft_info);
    }
    Ok(())
}

async fn get_moralis_nft_transfers(
    from_block: Option<u64>,
    global_nft: EthCoin,
    wallet_address: &str,
    wrapper: &UrlSignWrapper<'_>,
) -> MmResult<Vec<NftTransferHistory>, GetNftInfoError> {
    let chain = wrapper.chain;
    let mut res_list = Vec::new();

    let mut uri_without_cursor = wrapper.orig_url.clone();
    uri_without_cursor
        .path_segments_mut()
        .map_to_mm(|_| GetNftInfoError::Internal("Invalid URI".to_string()))?
        .push(MORALIS_API)
        .push(MORALIS_ENDPOINT_V)
        .push(wallet_address)
        .push("nft")
        .push("transfers");
    let from_block = match from_block {
        Some(block) => block.to_string(),
        None => "1".into(),
    };
    uri_without_cursor
        .query_pairs_mut()
        .append_pair("chain", &chain.to_string())
        .append_pair(MORALIS_FORMAT_QUERY_NAME, MORALIS_FORMAT_QUERY_VALUE)
        .append_pair(MORALIS_FROM_BLOCK_QUERY_NAME, &from_block);
    drop_mutability!(uri_without_cursor);

    // The cursor returned in the previous response (used for getting the next page).
    let mut cursor = String::new();
    loop {
        // Create a new URL instance from uri_without_cursor and modify its query to include the cursor if present
        let uri = format!("{uri_without_cursor}{cursor}");
        let response = build_and_send_request(uri.as_str(), &wrapper.proxy_sign).await?;
        if let Some(transfer_list) = response["result"].as_array() {
            process_transfer_list(transfer_list, chain, wallet_address, &global_nft, &mut res_list).await?;
            // if the cursor is not null, there are other NFTs transfers on next page,
            // and we need to send new request with cursor to get info from the next page.
            if let Some(cursor_res) = response["cursor"].as_str() {
                cursor = format!("&cursor={cursor_res}");
                continue;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    Ok(res_list)
}

async fn process_transfer_list(
    transfer_list: &[Json],
    chain: &Chain,
    wallet_address: &str,
    global_nft: &EthCoin,
    res_list: &mut Vec<NftTransferHistory>,
) -> MmResult<(), GetNftInfoError> {
    for transfer in transfer_list {
        let transfer_moralis = NftTransferHistoryFromMoralis::deserialize(transfer)?;
        let contract_type = match transfer_moralis.contract_type {
            Some(contract_type) => contract_type,
            None => continue,
        };
        let status = get_transfer_status(wallet_address, &transfer_moralis.common.to_address.addr_to_string());
        let block_timestamp = parse_rfc3339_to_timestamp(&transfer_moralis.block_timestamp)?;
        let fee_details = get_fee_details(global_nft, &transfer_moralis.common.transaction_hash).await;
        let transfer_history = NftTransferHistory {
            common: NftTransferCommon {
                block_hash: transfer_moralis.common.block_hash,
                transaction_hash: transfer_moralis.common.transaction_hash,
                transaction_index: transfer_moralis.common.transaction_index,
                log_index: transfer_moralis.common.log_index,
                value: transfer_moralis.common.value,
                transaction_type: transfer_moralis.common.transaction_type,
                token_address: transfer_moralis.common.token_address,
                from_address: transfer_moralis.common.from_address,
                to_address: transfer_moralis.common.to_address,
                amount: transfer_moralis.common.amount,
                verified: transfer_moralis.common.verified,
                operator: transfer_moralis.common.operator,
                possible_spam: transfer_moralis.common.possible_spam,
            },
            chain: *chain,
            token_id: transfer_moralis.token_id.0,
            block_number: *transfer_moralis.block_number,
            block_timestamp,
            contract_type,
            token_uri: None,
            token_domain: None,
            collection_name: None,
            image_url: None,
            image_domain: None,
            token_name: None,
            status,
            possible_phishing: false,
            fee_details,
            confirmations: 0,
        };
        // collect NFTs transfers from the page
        res_list.push(transfer_history);
    }
    Ok(())
}

async fn get_fee_details(eth_coin: &EthCoin, transaction_hash: &str) -> Option<EthTxFeeDetails> {
    let hash = H256::from_str(transaction_hash).ok()?;
    let receipt = eth_coin.web3().await.ok()?.eth().transaction_receipt(hash).await.ok()?;
    let fee_coin = match eth_coin.coin_type {
        EthCoinType::Eth => eth_coin.ticker(),
        EthCoinType::Erc20 { .. } | EthCoinType::Nft { .. } => return None,
    };

    match receipt {
        Some(r) => {
            let gas_used = r.gas_used.unwrap_or_default();
            match r.effective_gas_price {
                Some(gas_price) => EthTxFeeDetails::new(
                    gas_used,
                    // TODO: is this always legacy?
                    PayForGasOption::Legacy { gas_price },
                    fee_coin,
                )
                .ok(),
                None => {
                    let web3_tx = eth_coin
                        .web3()
                        .await
                        .ok()?
                        .eth()
                        .transaction(TransactionId::Hash(hash))
                        .await
                        .ok()??;
                    let gas_price = web3_tx.gas_price.unwrap_or_default();
                    // TODO: is this always legacy?
                    EthTxFeeDetails::new(gas_used, PayForGasOption::Legacy { gas_price }, fee_coin).ok()
                },
            }
        },
        None => None,
    }
}

/// Implements request to the Moralis "Get NFT metadata" endpoint.
///
/// [Moralis Documentation Link](https://docs.moralis.io/web3-data-api/evm/reference/get-nft-metadata)
///
/// **Caution:**
///
/// ERC-1155 token can have a total supply more than 1, which means there could be several owners
/// of the same token. `get_nft_metadata` returns NFTs info with the most recent owner.
/// **Don't** use this function to get specific info about owner address, amount etc, you will get info not related to my_address.
async fn get_moralis_metadata(
    token_address: String,
    token_id: BigUint,
    wrapper: &UrlSignWrapper<'_>,
) -> MmResult<Nft, GetNftInfoError> {
    let mut uri = wrapper.orig_url.clone();
    let chain = wrapper.chain;
    uri.path_segments_mut()
        .map_to_mm(|_| GetNftInfoError::Internal("Invalid URI".to_string()))?
        .push(MORALIS_API)
        .push(MORALIS_ENDPOINT_V)
        .push("nft")
        .push(&token_address)
        .push(&token_id.to_string());
    uri.query_pairs_mut()
        .append_pair("chain", &chain.to_string())
        .append_pair(MORALIS_FORMAT_QUERY_NAME, MORALIS_FORMAT_QUERY_VALUE);
    drop_mutability!(uri);

    let response = build_and_send_request(uri.as_str(), &wrapper.proxy_sign).await?;
    let nft_moralis: NftFromMoralis = serde_json::from_str(&response.to_string())?;
    let contract_type = match nft_moralis.contract_type {
        Some(contract_type) => contract_type,
        None => return MmError::err(GetNftInfoError::ContractTypeIsNull),
    };
    let mut nft_metadata = build_nft_from_moralis(*chain, nft_moralis, contract_type, wrapper.url_antispam).await;
    protect_from_nft_spam_links(&mut nft_metadata, false).map_mm_err()?;
    Ok(nft_metadata)
}

/// `withdraw_nft` function generates, signs and returns a transaction that transfers NFT
/// from my address to recipient's address.
/// This method generates a raw transaction which should then be broadcast using `send_raw_transaction`.
pub async fn withdraw_nft(ctx: MmArc, req: WithdrawNftReq) -> WithdrawNftResult {
    match req {
        WithdrawNftReq::WithdrawErc1155(erc1155_withdraw) => withdraw_erc1155(ctx, erc1155_withdraw).await,
        WithdrawNftReq::WithdrawErc721(erc721_withdraw) => withdraw_erc721(ctx, erc721_withdraw).await,
    }
}

/// `check_moralis_ipfs_bafy` inspects a given token URI and modifies it if certain conditions are met.
///
/// It checks if the URI points to the Moralis IPFS domain `"ipfs.moralis.io"` and starts with a specific path prefix `"/ipfs/bafy"`.
/// If these conditions are satisfied, it modifies the URI to point to the `"ipfs.io"` domain.
/// This is due to certain "bafy"-prefixed hashes being banned on Moralis IPFS gateway due to abuse.
///
/// If the URI does not meet these conditions or cannot be parsed, it is returned unchanged.
fn check_moralis_ipfs_bafy(token_uri: Option<&str>) -> Option<String> {
    token_uri.map(|uri| {
        if let Ok(parsed_url) = Url::parse(uri) {
            if parsed_url.host_str() == Some("ipfs.moralis.io") && parsed_url.path().starts_with("/ipfs/bafy") {
                let parts: Vec<&str> = parsed_url.path().splitn(2, "/ipfs/").collect();
                format!("https://ipfs.io/ipfs/{}", parts[1])
            } else {
                uri.to_string()
            }
        } else {
            uri.to_string()
        }
    })
}

async fn get_uri_meta(
    token_uri: Option<&str>,
    metadata: Option<&str>,
    url_antispam: &Url,
    possible_spam: bool,
    possible_phishing: bool,
) -> UriMeta {
    let mut uri_meta = UriMeta::default();
    if !possible_spam && !possible_phishing {
        // Fetching data from the URL if token_uri is provided
        if let Some(token_uri) = token_uri {
            if let Some(url) = construct_camo_url_with_token(token_uri, url_antispam) {
                uri_meta = fetch_meta_from_url(url).await.unwrap_or_default();
            }
        }
    }

    // Filling fields from metadata if provided
    if let Some(metadata) = metadata {
        if let Ok(meta_from_meta) = serde_json::from_str::<UriMeta>(metadata) {
            uri_meta.try_to_fill_missing_fields_from(meta_from_meta);
        }
    }
    update_uri_moralis_ipfs_fields(&mut uri_meta);
    uri_meta
}

fn construct_camo_url_with_token(token_uri: &str, url_antispam: &Url) -> Option<Url> {
    let mut url = url_antispam.clone();
    url.set_path("url/decode");
    url.path_segments_mut().ok()?.push(hex::encode(token_uri).as_str());
    Some(url)
}

async fn fetch_meta_from_url(url: Url) -> MmResult<UriMeta, MetaFromUrlError> {
    let response_meta = send_request_to_uri(url.as_str(), None).await.map_mm_err()?;
    serde_json::from_value(response_meta).map_err(|e| e.into())
}

fn update_uri_moralis_ipfs_fields(uri_meta: &mut UriMeta) {
    uri_meta.image_url = check_moralis_ipfs_bafy(uri_meta.image_url.as_deref());
    uri_meta.image_domain = get_domain_from_url(uri_meta.image_url.as_deref());
    uri_meta.animation_url = check_moralis_ipfs_bafy(uri_meta.animation_url.as_deref());
    uri_meta.animation_domain = get_domain_from_url(uri_meta.animation_url.as_deref());
    uri_meta.external_url = check_moralis_ipfs_bafy(uri_meta.external_url.as_deref());
    uri_meta.external_domain = get_domain_from_url(uri_meta.external_url.as_deref());
}

fn get_transfer_status(my_wallet: &str, to_address: &str) -> TransferStatus {
    // if my_wallet == from_address && my_wallet == to_address it is incoming transfer, so we can check just to_address.
    if my_wallet.to_lowercase() == to_address.to_lowercase() {
        TransferStatus::Receive
    } else {
        TransferStatus::Send
    }
}

/// `update_nft_list` function gets nft transfers from NFT HISTORY table, iterates through them
/// and updates NFT LIST table info.
async fn update_nft_list<T: NftListStorageOps + NftTransferHistoryStorageOps>(
    storage: &T,
    scan_from_block: u64,
    wallet_address: &str,
    wrapper: &UrlSignWrapper<'_>,
) -> MmResult<(), UpdateNftError> {
    let chain = wrapper.chain;
    let transfers = storage
        .get_transfers_from_block(*chain, scan_from_block)
        .await
        .map_mm_err()?;
    for transfer in transfers.into_iter() {
        handle_nft_transfer(storage, wrapper, transfer, wallet_address).await?;
    }
    Ok(())
}

async fn handle_nft_transfer<T: NftListStorageOps + NftTransferHistoryStorageOps>(
    storage: &T,
    wrapper: &UrlSignWrapper<'_>,
    transfer: NftTransferHistory,
    my_address: &str,
) -> MmResult<(), UpdateNftError> {
    let chain = wrapper.chain;
    match (transfer.status, transfer.contract_type) {
        (TransferStatus::Send, ContractType::Erc721) => handle_send_erc721(storage, chain, transfer).await,
        (TransferStatus::Receive, ContractType::Erc721) => {
            handle_receive_erc721(storage, transfer, wrapper, my_address).await
        },
        (TransferStatus::Send, ContractType::Erc1155) => handle_send_erc1155(storage, chain, transfer).await,
        (TransferStatus::Receive, ContractType::Erc1155) => {
            handle_receive_erc1155(storage, transfer, wrapper, my_address).await
        },
    }
}

async fn handle_send_erc721<T: NftListStorageOps + NftTransferHistoryStorageOps>(
    storage: &T,
    chain: &Chain,
    transfer: NftTransferHistory,
) -> MmResult<(), UpdateNftError> {
    storage
        .get_nft(
            chain,
            transfer.common.token_address.addr_to_string(),
            transfer.token_id.clone(),
        )
        .await
        .map_mm_err()?
        .ok_or_else(|| UpdateNftError::TokenNotFoundInWallet {
            token_address: transfer.common.token_address.addr_to_string(),
            token_id: transfer.token_id.to_string(),
        })?;
    storage
        .remove_nft_from_list(
            chain,
            transfer.common.token_address.addr_to_string(),
            transfer.token_id,
            transfer.block_number,
        )
        .await
        .map_mm_err()?;
    Ok(())
}

async fn handle_receive_erc721<T: NftListStorageOps + NftTransferHistoryStorageOps>(
    storage: &T,
    transfer: NftTransferHistory,
    wrapper: &UrlSignWrapper<'_>,
    my_address: &str,
) -> MmResult<(), UpdateNftError> {
    let chain = wrapper.chain;
    let token_address_str = transfer.common.token_address.addr_to_string();
    match storage
        .get_nft(chain, token_address_str.clone(), transfer.token_id.clone())
        .await
        .map_mm_err()?
    {
        Some(mut nft_db) => {
            // An error is raised if user tries to receive an identical ERC-721 token they already own
            // and if owner address != from address
            if my_address != transfer.common.from_address.addr_to_string() {
                return MmError::err(UpdateNftError::AttemptToReceiveAlreadyOwnedErc721 {
                    tx_hash: transfer.common.transaction_hash,
                });
            }
            nft_db.block_number = transfer.block_number;
            storage
                .update_nft_amount_and_block_number(chain, nft_db.clone())
                .await
                .map_mm_err()?;
            update_transfer_meta_using_nft(storage, chain, &mut nft_db).await?;
        },
        None => {
            let mut nft = match get_moralis_metadata(token_address_str.clone(), transfer.token_id.clone(), wrapper)
                .await
            {
                Ok(mut moralis_meta) => {
                    // sometimes moralis updates Get All NFTs (which also affects Get Metadata) later
                    // than History by Wallet update
                    moralis_meta.common.owner_of =
                        Address::from_str(my_address).map_to_mm(|e| UpdateNftError::InvalidHexString(e.to_string()))?;
                    moralis_meta.block_number = transfer.block_number;
                    moralis_meta
                },
                Err(_) => {
                    mark_as_spam_and_build_empty_meta(storage, chain, token_address_str, &transfer, my_address).await?
                },
            };
            storage
                .add_nfts_to_list(*chain, vec![nft.clone()], transfer.block_number)
                .await
                .map_mm_err()?;
            update_transfer_meta_using_nft(storage, chain, &mut nft).await?;
        },
    }
    Ok(())
}

async fn handle_send_erc1155<T: NftListStorageOps + NftTransferHistoryStorageOps>(
    storage: &T,
    chain: &Chain,
    transfer: NftTransferHistory,
) -> MmResult<(), UpdateNftError> {
    let token_address_str = transfer.common.token_address.addr_to_string();
    let mut nft_db = storage
        .get_nft(chain, token_address_str.clone(), transfer.token_id.clone())
        .await
        .map_mm_err()?
        .ok_or_else(|| UpdateNftError::TokenNotFoundInWallet {
            token_address: token_address_str.clone(),
            token_id: transfer.token_id.to_string(),
        })?;
    match nft_db.common.amount.cmp(&transfer.common.amount) {
        Ordering::Equal => {
            storage
                .remove_nft_from_list(chain, token_address_str, transfer.token_id, transfer.block_number)
                .await
                .map_mm_err()?;
        },
        Ordering::Greater => {
            nft_db.common.amount -= transfer.common.amount;
            storage
                .update_nft_amount(chain, nft_db.clone(), transfer.block_number)
                .await
                .map_mm_err()?;
        },
        Ordering::Less => {
            return MmError::err(UpdateNftError::InsufficientAmountInCache {
                amount_list: nft_db.common.amount.to_string(),
                amount_history: transfer.common.amount.to_string(),
            });
        },
    }
    Ok(())
}

async fn handle_receive_erc1155<T: NftListStorageOps + NftTransferHistoryStorageOps>(
    storage: &T,
    transfer: NftTransferHistory,
    wrapper: &UrlSignWrapper<'_>,
    my_address: &str,
) -> MmResult<(), UpdateNftError> {
    let chain = wrapper.chain;
    let token_address_str = transfer.common.token_address.addr_to_string();
    let mut nft = match storage
        .get_nft(chain, token_address_str.clone(), transfer.token_id.clone())
        .await
        .map_mm_err()?
    {
        Some(mut nft_db) => {
            // if owner address == from address, then owner sent tokens to themself,
            // which means that the amount will not change.
            if my_address != transfer.common.from_address.addr_to_string() {
                nft_db.common.amount += transfer.common.amount;
            }
            nft_db.block_number = transfer.block_number;
            drop_mutability!(nft_db);
            storage
                .update_nft_amount_and_block_number(chain, nft_db.clone())
                .await
                .map_mm_err()?;
            nft_db
        },
        // If token isn't in NFT LIST table then add nft to the table.
        None => {
            let nft = match get_moralis_metadata(token_address_str.clone(), transfer.token_id.clone(), wrapper).await {
                Ok(moralis_meta) => {
                    create_nft_from_moralis_metadata(moralis_meta, &transfer, my_address, chain, wrapper.url_antispam)
                        .await?
                },
                Err(_) => {
                    mark_as_spam_and_build_empty_meta(storage, chain, token_address_str, &transfer, my_address).await?
                },
            };
            storage
                .add_nfts_to_list(*chain, [nft.clone()], transfer.block_number)
                .await
                .map_mm_err()?;
            nft
        },
    };
    update_transfer_meta_using_nft(storage, chain, &mut nft).await?;
    Ok(())
}

// as there is no warranty that if link matches `is_malicious` it is a phishing, so mark it as spam
fn check_token_uri(possible_spam: &mut bool, token_uri: Option<&str>) -> MmResult<(), regex::Error> {
    if let Some(uri) = token_uri {
        if is_malicious(uri)? {
            *possible_spam = true;
        }
    }
    Ok(())
}

async fn create_nft_from_moralis_metadata(
    mut moralis_meta: Nft,
    transfer: &NftTransferHistory,
    my_address: &str,
    chain: &Chain,
    url_antispam: &Url,
) -> MmResult<Nft, UpdateNftError> {
    let token_uri = check_moralis_ipfs_bafy(moralis_meta.common.token_uri.as_deref());
    let token_domain = get_domain_from_url(token_uri.as_deref());
    check_token_uri(&mut moralis_meta.common.possible_spam, token_uri.as_deref()).map_mm_err()?;
    let uri_meta = get_uri_meta(
        token_uri.as_deref(),
        moralis_meta.common.metadata.as_deref(),
        url_antispam,
        moralis_meta.common.possible_spam,
        moralis_meta.possible_phishing,
    )
    .await;
    let nft = Nft {
        common: NftCommon {
            token_address: moralis_meta.common.token_address,
            amount: transfer.common.amount.clone(),
            owner_of: Address::from_str(my_address).map_to_mm(|e| UpdateNftError::InvalidHexString(e.to_string()))?,
            token_hash: moralis_meta.common.token_hash,
            collection_name: moralis_meta.common.collection_name,
            symbol: moralis_meta.common.symbol,
            token_uri,
            token_domain,
            metadata: moralis_meta.common.metadata,
            last_token_uri_sync: moralis_meta.common.last_token_uri_sync,
            last_metadata_sync: moralis_meta.common.last_metadata_sync,
            minter_address: moralis_meta.common.minter_address,
            possible_spam: moralis_meta.common.possible_spam,
        },
        chain: *chain,
        token_id: moralis_meta.token_id,
        block_number_minted: moralis_meta.block_number_minted,
        block_number: transfer.block_number,
        contract_type: moralis_meta.contract_type,
        possible_phishing: false,
        uri_meta,
    };
    Ok(nft)
}

async fn mark_as_spam_and_build_empty_meta<T: NftListStorageOps + NftTransferHistoryStorageOps>(
    storage: &T,
    chain: &Chain,
    token_address_str: String,
    transfer: &NftTransferHistory,
    my_address: &str,
) -> MmResult<Nft, UpdateNftError> {
    storage
        .update_nft_spam_by_token_address(chain, token_address_str.clone(), true)
        .await
        .map_mm_err()?;
    storage
        .update_transfer_spam_by_token_address(chain, token_address_str, true)
        .await
        .map_mm_err()?;

    Ok(build_nft_with_empty_meta(BuildNftFields {
        token_address: transfer.common.token_address,
        token_id: transfer.token_id.clone(),
        amount: transfer.common.amount.clone(),
        owner_of: Address::from_str(my_address).map_to_mm(|e| UpdateNftError::InvalidHexString(e.to_string()))?,
        contract_type: transfer.contract_type,
        possible_spam: true,
        chain: transfer.chain,
        block_number: transfer.block_number,
    }))
}

async fn cache_nfts_from_moralis<T: NftListStorageOps + NftTransferHistoryStorageOps>(
    wallet_address: &str,
    storage: &T,
    wrapper: &UrlSignWrapper<'_>,
) -> MmResult<Vec<Nft>, UpdateNftError> {
    let nft_list = get_moralis_nft_list(wallet_address, wrapper).await.map_mm_err()?;
    let last_scanned_block = NftTransferHistoryStorageOps::get_last_block_number(storage, wrapper.chain)
        .await
        .map_mm_err()?
        .unwrap_or(0);
    storage
        .add_nfts_to_list(*wrapper.chain, nft_list.clone(), last_scanned_block)
        .await
        .map_mm_err()?;
    Ok(nft_list)
}

/// `update_meta_in_transfers` function updates only transfers related to current nfts in wallet.
async fn update_meta_in_transfers<T>(storage: &T, chain: &Chain, nfts: Vec<Nft>) -> MmResult<(), UpdateNftError>
where
    T: NftListStorageOps + NftTransferHistoryStorageOps,
{
    for mut nft in nfts.into_iter() {
        update_transfer_meta_using_nft(storage, chain, &mut nft).await?;
    }
    Ok(())
}

/// `update_transfers_with_empty_meta` function updates empty metadata in transfers.
async fn update_transfers_with_empty_meta<T>(storage: &T, wrapper: &UrlSignWrapper<'_>) -> MmResult<(), UpdateNftError>
where
    T: NftListStorageOps + NftTransferHistoryStorageOps,
{
    let chain = wrapper.chain;
    let token_addr_id = storage.get_transfers_with_empty_meta(*chain).await.map_mm_err()?;
    for addr_id_pair in token_addr_id.into_iter() {
        let mut nft_meta =
            match get_moralis_metadata(addr_id_pair.token_address.clone(), addr_id_pair.token_id, wrapper).await {
                Ok(nft_meta) => nft_meta,
                Err(_) => {
                    storage
                        .update_nft_spam_by_token_address(chain, addr_id_pair.token_address.clone(), true)
                        .await
                        .map_mm_err()?;
                    storage
                        .update_transfer_spam_by_token_address(chain, addr_id_pair.token_address, true)
                        .await
                        .map_mm_err()?;
                    continue;
                },
            };
        update_transfer_meta_using_nft(storage, chain, &mut nft_meta).await?;
    }
    Ok(())
}

/// Checks if the given URL is potentially malicious based on certain patterns.
fn is_malicious(token_uri: &str) -> MmResult<bool, regex::Error> {
    let patterns = vec![r"\.(xyz|gq|top)(/|$)", r"\.(json|xml|jpg|png)[%?]"];
    for pattern in patterns {
        let regex = Regex::new(pattern)?;
        if regex.is_match(token_uri) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `contains_disallowed_scheme` function checks if the text contains some link.
fn contains_disallowed_url(text: &str) -> Result<bool, regex::Error> {
    let url_regex = Regex::new(
        r"(?:(?:https?|ftp|file|[^:\s]+:)/?|[^:\s]+:/|\b(?:[a-z\d]+\.))(?:(?:[^\s()<>]+|\((?:[^\s()<>]+|(?:\([^\s()<>]+\)))?\))+(?:\((?:[^\s()<>]+|(?:\(?:[^\s()<>]+\)))?\)|[^\s`!()\[\]{};:'.,<>?«»“”‘’]))?",
    )?;
    Ok(url_regex.is_match(text))
}

/// `process_text_for_spam_link` checks if the text contains any links and optionally redacts it.
/// It doesn't matter if the link is valid or not, as this is a spam check.
/// If text contains some link, then function returns `true`.
fn process_text_for_spam_link(text: &mut Option<String>, redact: bool) -> Result<bool, regex::Error> {
    match text {
        Some(s) if contains_disallowed_url(s)? => {
            if redact {
                *text = Some("URL redacted for user protection".to_string());
            }
            Ok(true)
        },
        _ => Ok(false),
    }
}

/// `protect_from_history_spam_links` function checks and redact spam in `NftTransferHistory`.
///
/// `collection_name` and `token_name` in `NftTransferHistory` shouldn't contain any links,
/// they must be just an arbitrary text, which represents NFT names.
fn protect_from_history_spam_links(
    transfer: &mut NftTransferHistory,
    redact: bool,
) -> MmResult<(), ProtectFromSpamError> {
    let collection_name_spam = process_text_for_spam_link(&mut transfer.collection_name, redact)?;
    let token_name_spam = process_text_for_spam_link(&mut transfer.token_name, redact)?;

    if collection_name_spam || token_name_spam {
        transfer.common.possible_spam = true;
    }
    Ok(())
}

/// `protect_from_nft_spam_links` function checks and optionally redacts spam links in `Nft`.
///
/// `collection_name` and `token_name` in `Nft` shouldn't contain any links,
/// they must be just an arbitrary text, which represents NFT names.
/// `symbol` also must be a text or sign that represents a symbol.
/// This function also checks `metadata` field for spam.
fn protect_from_nft_spam_links(nft: &mut Nft, redact: bool) -> MmResult<(), ProtectFromSpamError> {
    let collection_name_spam = process_text_for_spam_link(&mut nft.common.collection_name, redact)?;
    let symbol_spam = process_text_for_spam_link(&mut nft.common.symbol, redact)?;
    let token_name_spam = process_text_for_spam_link(&mut nft.uri_meta.token_name, redact)?;
    let meta_spam = process_metadata_for_spam_link(nft, redact)?;

    if collection_name_spam || symbol_spam || token_name_spam || meta_spam {
        nft.common.possible_spam = true;
    }
    Ok(())
}

/// The `process_metadata_for_spam_link` function checks and optionally redacts spam link in the `metadata` field of `Nft`.
///
/// **note:** `token_name` is usually called `name` in `metadata`.
fn process_metadata_for_spam_link(nft: &mut Nft, redact: bool) -> MmResult<bool, ProtectFromSpamError> {
    if let Some(Ok(mut metadata)) = nft
        .common
        .metadata
        .as_ref()
        .map(|t| serde_json::from_str::<serde_json::Map<String, Json>>(t))
    {
        let spam_detected = process_metadata_field(&mut metadata, "name", redact)?;
        if redact && spam_detected {
            nft.common.metadata = Some(serde_json::to_string(&metadata)?);
        }
        return Ok(spam_detected);
    }
    Ok(false)
}

/// The `process_metadata_field` function scans a specified field in a JSON metadata object for potential spam.
///
/// This function checks the provided `metadata` map for a field matching the `field` parameter.
/// If this field is found and its value contains some link, it's considered to contain spam.
/// Depending on the `redact` flag, it will either redact the spam link or leave it as it is.
/// The function returns `true` if it detected a spam link, or `false` otherwise.
fn process_metadata_field(
    metadata: &mut serde_json::Map<String, Json>,
    field: &str,
    redact: bool,
) -> MmResult<bool, ProtectFromSpamError> {
    match metadata.get(field).and_then(|v| v.as_str()) {
        Some(text) if contains_disallowed_url(text)? => {
            if redact {
                metadata.insert(
                    field.to_string(),
                    serde_json::Value::String("URL redacted for user protection".to_string()),
                );
            }
            Ok(true)
        },
        _ => Ok(false),
    }
}

async fn build_nft_from_moralis(
    chain: Chain,
    mut nft_moralis: NftFromMoralis,
    contract_type: ContractType,
    url_antispam: &Url,
) -> Nft {
    let token_uri = check_moralis_ipfs_bafy(nft_moralis.common.token_uri.as_deref());
    if let Err(e) = check_token_uri(&mut nft_moralis.common.possible_spam, token_uri.as_deref()) {
        error!("Error checking token URI: {}", e);
    }
    let uri_meta = get_uri_meta(
        token_uri.as_deref(),
        nft_moralis.common.metadata.as_deref(),
        url_antispam,
        nft_moralis.common.possible_spam,
        false,
    )
    .await;
    let token_domain = get_domain_from_url(token_uri.as_deref());
    Nft {
        common: NftCommon {
            token_address: nft_moralis.common.token_address,
            amount: nft_moralis.common.amount,
            owner_of: nft_moralis.common.owner_of,
            token_hash: nft_moralis.common.token_hash,
            collection_name: nft_moralis.common.collection_name,
            symbol: nft_moralis.common.symbol,
            token_uri,
            token_domain,
            metadata: nft_moralis.common.metadata,
            last_token_uri_sync: nft_moralis.common.last_token_uri_sync,
            last_metadata_sync: nft_moralis.common.last_metadata_sync,
            minter_address: nft_moralis.common.minter_address,
            possible_spam: nft_moralis.common.possible_spam,
        },
        chain,
        token_id: nft_moralis.token_id.0,
        block_number_minted: nft_moralis.block_number_minted.map(|v| v.0),
        block_number: *nft_moralis.block_number,
        contract_type,
        possible_phishing: false,
        uri_meta,
    }
}

#[inline(always)]
pub(crate) fn get_domain_from_url(url: Option<&str>) -> Option<String> {
    url.and_then(|uri| Url::parse(uri).ok())
        .and_then(|url| url.domain().map(String::from))
}

/// Clears NFT data from the database for specified chains.
pub async fn clear_nft_db(ctx: MmArc, req: ClearNftDbReq) -> MmResult<(), ClearNftDbError> {
    if req.clear_all {
        let nft_ctx = NftCtx::from_ctx(&ctx).map_to_mm(ClearNftDbError::Internal)?;
        let storage = nft_ctx.lock_db().await.map_mm_err()?;
        storage.clear_all_nft_data().await.map_mm_err()?;
        storage.clear_all_history_data().await.map_mm_err()?;
        return Ok(());
    }

    if req.chains.is_empty() {
        return MmError::err(ClearNftDbError::InvalidRequest(
            "Nothing to clear was specified".to_string(),
        ));
    }

    let nft_ctx = NftCtx::from_ctx(&ctx).map_to_mm(ClearNftDbError::Internal)?;
    let storage = nft_ctx.lock_db().await.map_mm_err()?;
    let mut errors = Vec::new();
    for chain in req.chains.iter() {
        if let Err(e) = clear_data_for_chain(&storage, chain).await {
            errors.push(e);
        }
    }
    if !errors.is_empty() {
        return MmError::err(ClearNftDbError::DbError(format!("{errors:?}")));
    }

    Ok(())
}

async fn clear_data_for_chain<T>(storage: &T, chain: &Chain) -> MmResult<(), ClearNftDbError>
where
    T: NftListStorageOps + NftTransferHistoryStorageOps,
{
    let (is_nft_list_init, is_history_init) = (
        NftListStorageOps::is_initialized(storage, chain).await.map_mm_err()?,
        NftTransferHistoryStorageOps::is_initialized(storage, chain)
            .await
            .map_mm_err()?,
    );
    if is_nft_list_init {
        storage.clear_nft_data(chain).await.map_mm_err()?;
    }
    if is_history_init {
        storage.clear_history_data(chain).await.map_mm_err()?;
    }
    Ok(())
}

fn construct_moralis_uri_for_nft(orig_url: &Url, address: &str, chain: &Chain) -> MmResult<Url, GetNftInfoError> {
    let mut uri = orig_url.clone();
    uri.path_segments_mut()
        .map_to_mm(|_| GetNftInfoError::Internal("Invalid URI".to_string()))?
        .push(MORALIS_API)
        .push(MORALIS_ENDPOINT_V)
        .push(address)
        .push("nft");
    uri.query_pairs_mut()
        .append_pair("chain", &chain.to_string())
        .append_pair(MORALIS_FORMAT_QUERY_NAME, MORALIS_FORMAT_QUERY_VALUE);
    Ok(uri)
}

/// A wrapper struct for holding the chain identifier, original URL field from RPC, anti-spam URL and signed message.
struct UrlSignWrapper<'a> {
    chain: &'a Chain,
    orig_url: &'a Url,
    url_antispam: &'a Url,
    proxy_sign: Option<ProxySign>,
}

async fn build_and_send_request(uri: &str, proxy_sign: &Option<ProxySign>) -> MmResult<Json, GetNftInfoError> {
    let payload = proxy_sign.as_ref().map(|msg| serde_json::to_string(&msg)).transpose()?;
    let response = send_request_to_uri(uri, payload.as_deref()).await.map_mm_err()?;
    Ok(response)
}
