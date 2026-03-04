use super::{ERC20_PROTOCOL_TYPE, ETH_PROTOCOL_TYPE};
use crate::eth::web3_transport::Web3Transport;
use crate::eth::{EthCoin, Web3RpcError, ERC20_CONTRACT};
use crate::{CoinsContext, MarketCoinOps, MmCoinEnum, Ticker};
use ethabi::Token;
use ethereum_types::Address;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use std::str::FromStr;
use web3::types::{BlockId, BlockNumber, CallRequest};
use web3::{Transport, Web3};

async fn call_erc20_function<T: Transport>(
    web3: &Web3<T>,
    token_addr: Address,
    function_name: &str,
) -> MmResult<Vec<Token>, Web3RpcError> {
    let function = ERC20_CONTRACT
        .function(function_name)
        .map_to_mm(|e| Web3RpcError::Internal(e.to_string()))?;
    let data = function
        .encode_input(&[])
        .map_to_mm(|e| Web3RpcError::Internal(e.to_string()))?;
    let request = CallRequest {
        from: Some(Address::default()),
        to: Some(token_addr),
        gas: None,
        gas_price: None,
        value: Some(0.into()),
        data: Some(data.into()),
        ..CallRequest::default()
    };

    let res = web3
        .eth()
        .call(request, Some(BlockId::Number(BlockNumber::Latest)))
        .await
        .map_to_mm(|e| Web3RpcError::Transport(e.to_string()))?;
    function
        .decode_output(&res.0)
        .map_to_mm(|e| Web3RpcError::InvalidResponse(e.to_string()))
}

pub(crate) async fn get_token_decimals(web3: &Web3<Web3Transport>, token_addr: Address) -> MmResult<u8, Web3RpcError> {
    let tokens = call_erc20_function(web3, token_addr, "decimals").await?;
    let Some(token) = tokens.into_iter().next() else {
        return MmError::err(Web3RpcError::InvalidResponse(
            "No value returned from decimals() call".to_string(),
        ));
    };
    let Token::Uint(dec) = token else {
        return MmError::err(Web3RpcError::InvalidResponse(format!(
            "Expected Uint token for decimals, got {:?}",
            token
        )));
    };
    Ok(dec.as_u64() as u8)
}

async fn get_token_symbol(coin: &EthCoin, token_addr: Address) -> Result<String, String> {
    let web3 = try_s!(coin.web3().await);
    let tokens = call_erc20_function(&web3, token_addr, "symbol")
        .await
        .map_err(|e| e.to_string())?;
    let Some(token) = tokens.into_iter().next() else {
        return ERR!("No value returned from symbol() call");
    };
    let Token::String(symbol) = token else {
        return ERR!("Expected String token for symbol, got {:?}", token);
    };
    Ok(symbol)
}

#[derive(Serialize)]
pub struct Erc20TokenInfo {
    pub symbol: String,
    pub decimals: u8,
}

pub async fn get_erc20_token_info(coin: &EthCoin, token_addr: Address) -> Result<Erc20TokenInfo, String> {
    let symbol = get_token_symbol(coin, token_addr).await?;
    let web3 = try_s!(coin.web3().await);
    let decimals = get_token_decimals(&web3, token_addr).await.map_err(|e| e.to_string())?;
    Ok(Erc20TokenInfo { symbol, decimals })
}

/// Finds eth platfrom coin in coins config by chain_id and returns its ticker.
pub fn get_platform_ticker(ctx: &MmArc, chain_id: u64) -> Option<Ticker> {
    ctx.conf["coins"].as_array()?.iter().find_map(|coin| {
        let protocol = coin.get("protocol")?;
        let protocol_type = protocol.get("type")?.as_str()?;
        if protocol_type != ETH_PROTOCOL_TYPE {
            return None;
        }
        let coin_chain_id = protocol.get("protocol_data")?.get("chain_id")?.as_u64()?;
        if coin_chain_id == chain_id {
            coin.get("coin")?.as_str().map(|s| s.to_string())
        } else {
            None
        }
    })
}

/// Finds if an ERC20 token is in coins config by its contract address and returns its ticker.
pub fn get_erc20_ticker_by_contract_address(
    ctx: &MmArc,
    platform_ticker: &str,
    contract_address: &Address,
) -> Option<String> {
    ctx.conf["coins"].as_array()?.iter().find_map(|coin| {
        let protocol = coin.get("protocol")?;
        let protocol_type = protocol.get("type")?.as_str()?;
        if protocol_type != ERC20_PROTOCOL_TYPE {
            return None;
        }
        let protocol_data = protocol.get("protocol_data")?;
        let coin_platform_ticker = protocol_data.get("platform")?.as_str()?;
        let coin_contract_address = Address::from_str(protocol_data.get("contract_address")?.as_str()?).ok()?;

        if coin_platform_ticker == platform_ticker && &coin_contract_address == contract_address {
            coin.get("coin")?.as_str().map(|s| s.to_string())
        } else {
            None
        }
    })
}

/// Finds an enabled ERC20 token by contract address and platform coin ticker and returns it as `MmCoinEnum`.
pub async fn get_enabled_erc20_by_platform_and_contract(
    ctx: &MmArc,
    platform: &str,
    contract_address: &Address,
) -> MmResult<Option<MmCoinEnum>, String> {
    let cctx = CoinsContext::from_ctx(ctx)?;
    let coins = cctx.coins.lock().await;

    Ok(coins.values().find_map(|coin| match &coin.inner {
        MmCoinEnum::EthCoinVariant(eth_coin)
            if eth_coin.platform_ticker() == platform
                && eth_coin.erc20_token_address().as_ref() == Some(contract_address) =>
        {
            Some(coin.inner.clone())
        },
        _ => None,
    }))
}
