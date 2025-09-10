//! This source file is for RPCs specific for EVM platform
use coins::eth::erc20::{get_erc20_ticker_by_contract_address, get_erc20_token_info, Erc20TokenInfo};
use coins::eth::valid_addr_from_str;
use coins::eth::{u256_from_big_decimal, u256_to_big_decimal, EthCoin, Web3RpcError};
use coins::{
    lp_coinfind_or_err, CoinFindError, CoinProtocol, MmCoin, MmCoinEnum, NumConversError, Transaction, TransactionErr,
};
use common::HttpStatusCode;
use derive_more::Display;
use enum_derives::EnumFromStringify;
use ethereum_types::Address as EthAddress;
use futures::compat::Future01CompatExt;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::map_mm_error::MmResultExt;
use mm2_err_handle::{map_to_mm::MapToMmResult, mm_error::MmError, prelude::MmResult};
use mm2_number::BigDecimal;

#[derive(Deserialize)]
pub struct TokenInfoRequest {
    protocol: CoinProtocol,
}

#[derive(Serialize)]
#[serde(tag = "type", content = "info")]
pub enum TokenInfo {
    ERC20(Erc20TokenInfo),
}

#[derive(Serialize)]
pub struct TokenInfoResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    config_ticker: Option<String>,
    #[serde(flatten)]
    info: TokenInfo,
}

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum TokenInfoError {
    #[display(fmt = "No such coin {coin}")]
    NoSuchCoin { coin: String },
    #[display(fmt = "Custom tokens are not supported for {protocol} protocol yet!")]
    UnsupportedTokenProtocol { protocol: String },
    #[display(fmt = "Invalid request {_0}")]
    InvalidRequest(String),
    #[display(fmt = "Error retrieving token info {_0}")]
    RetrieveInfoError(String),
}

impl HttpStatusCode for TokenInfoError {
    fn status_code(&self) -> StatusCode {
        match self {
            TokenInfoError::NoSuchCoin { .. } => StatusCode::NOT_FOUND,
            TokenInfoError::UnsupportedTokenProtocol { .. } | TokenInfoError::InvalidRequest(_) => {
                StatusCode::BAD_REQUEST
            },
            TokenInfoError::RetrieveInfoError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CoinFindError> for TokenInfoError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => TokenInfoError::NoSuchCoin { coin },
        }
    }
}

pub async fn get_token_info(ctx: MmArc, req: TokenInfoRequest) -> MmResult<TokenInfoResponse, TokenInfoError> {
    // Check that the protocol is a token protocol
    let platform = req.protocol.platform().ok_or(TokenInfoError::InvalidRequest(format!(
        "Protocol '{:?}' is not a token protocol",
        req.protocol
    )))?;
    // Platform coin should be activated
    let platform_coin = lp_coinfind_or_err(&ctx, platform).await.map_mm_err()?;
    match platform_coin {
        MmCoinEnum::EthCoin(eth_coin) => {
            let contract_address_str =
                req.protocol
                    .contract_address()
                    .ok_or(TokenInfoError::UnsupportedTokenProtocol {
                        protocol: platform.to_string(),
                    })?;
            let contract_address = valid_addr_from_str(contract_address_str).map_to_mm(|e| {
                let error = format!("Invalid contract address: {e}");
                TokenInfoError::InvalidRequest(error)
            })?;

            let config_ticker = get_erc20_ticker_by_contract_address(&ctx, platform, &contract_address);
            let token_info = get_erc20_token_info(&eth_coin, contract_address)
                .await
                .map_to_mm(TokenInfoError::RetrieveInfoError)?;
            Ok(TokenInfoResponse {
                config_ticker,
                info: TokenInfo::ERC20(token_info),
            })
        },
        _ => MmError::err(TokenInfoError::UnsupportedTokenProtocol {
            protocol: platform.to_string(),
        }),
    }
}

#[derive(Debug, Deserialize, Display, EnumFromStringify, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum Erc20CallError {
    #[display(fmt = "No such coin {coin}")]
    NoSuchCoin { coin: String },
    #[display(fmt = "Coin not supported {coin}")]
    CoinNotSupported { coin: String },
    #[from_stringify("NumConversError")]
    #[display(fmt = "Invalid param: {_0}")]
    InvalidParam(String),
    #[from_stringify("TransactionErr")]
    #[display(fmt = "Transaction error {_0}")]
    TransactionError(String),
    #[from_stringify("Web3RpcError")]
    #[display(fmt = "Web3 RPC error {_0}")]
    Web3RpcError(String),
}

impl HttpStatusCode for Erc20CallError {
    fn status_code(&self) -> StatusCode {
        match self {
            Erc20CallError::NoSuchCoin { .. }
            | Erc20CallError::CoinNotSupported { .. }
            | Erc20CallError::InvalidParam(_) => StatusCode::BAD_REQUEST,
            Erc20CallError::TransactionError(_) | Erc20CallError::Web3RpcError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Erc20AllowanceRequest {
    coin: String,
    spender: EthAddress,
}

/// Call allowance method for ERC20 tokens (see https://eips.ethereum.org/EIPS/eip-20#approve).
/// Returns BigDecimal allowance value.
pub async fn get_token_allowance_rpc(ctx: MmArc, req: Erc20AllowanceRequest) -> MmResult<BigDecimal, Erc20CallError> {
    let eth_coin = find_erc20_eth_coin(&ctx, &req.coin).await?;
    let wei = eth_coin.allowance(req.spender).compat().await.map_mm_err()?;
    let amount = u256_to_big_decimal(wei, eth_coin.decimals()).map_mm_err()?;
    Ok(amount)
}

#[derive(Debug, Deserialize)]
pub struct Erc20ApproveRequest {
    coin: String,
    spender: EthAddress,
    amount: BigDecimal,
}

/// Call approve method for ERC20 tokens (see https://eips.ethereum.org/EIPS/eip-20#allowance).
/// Returns approval transaction hash.
pub async fn approve_token_rpc(ctx: MmArc, req: Erc20ApproveRequest) -> MmResult<String, Erc20CallError> {
    let eth_coin = find_erc20_eth_coin(&ctx, &req.coin).await?;
    let amount = u256_from_big_decimal(&req.amount, eth_coin.decimals()).map_mm_err()?;
    let tx = eth_coin.approve(req.spender, amount).compat().await?;
    Ok(format!("0x{:02x}", tx.tx_hash_as_bytes()))
}

async fn find_erc20_eth_coin(ctx: &MmArc, coin: &str) -> Result<EthCoin, MmError<Erc20CallError>> {
    match lp_coinfind_or_err(ctx, coin).await {
        Ok(MmCoinEnum::EthCoin(eth_coin)) => Ok(eth_coin),
        Ok(_) => Err(MmError::new(Erc20CallError::CoinNotSupported {
            coin: coin.to_string(),
        })),
        Err(_) => Err(MmError::new(Erc20CallError::NoSuchCoin { coin: coin.to_string() })),
    }
}
