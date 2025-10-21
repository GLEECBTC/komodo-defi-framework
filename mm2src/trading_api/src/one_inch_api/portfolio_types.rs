//! Structs to call 1inch portfolio api

use super::client::QueryParams;
use super::errors::ApiClientError;
use common::{def_with_opt_param, push_if_some};
use mm2_err_handle::mm_error::MmResult;
use mm2_number::BigDecimal;
use serde::Deserialize;
use std::fmt;

#[derive(Default)]
pub enum DataGranularity {
    Month,
    Week,
    Day,
    FourHour,
    Hour,
    FifteenMin,
    #[default]
    FiveMin,
}

impl fmt::Display for DataGranularity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataGranularity::Month => write!(f, "month"),
            DataGranularity::Week => write!(f, "week"),
            DataGranularity::Day => write!(f, "day"),
            DataGranularity::FourHour => write!(f, "4hour"),
            DataGranularity::Hour => write!(f, "hour"),
            DataGranularity::FifteenMin => write!(f, "15min"),
            DataGranularity::FiveMin => write!(f, "5min"),
        }
    }
}

/// API params builder to get OHLC price history for token pair
/// See 1inch docs: https://portal.1inch.dev/documentation/apis/portfolio/swagger?method=get&path=%2Fintegrations%2Fprices%2Fv1%2Ftime_range%2Fcross_prices
#[derive(Default)]
pub struct CrossPriceParams {
    chain_id: u64,
    /// Base token address
    token0_address: String,
    /// Quote token address
    token1_address: String,
    /// Returned time series intervals
    granularity: Option<DataGranularity>,
    /// max number of time series
    limit: Option<u32>,
}

impl CrossPriceParams {
    pub fn new(chain_id: u64, token0_address: String, token1_address: String) -> Self {
        Self {
            chain_id,
            token0_address,
            token1_address,
            ..Default::default()
        }
    }

    def_with_opt_param!(granularity, DataGranularity);
    def_with_opt_param!(limit, u32);

    #[allow(clippy::result_large_err)]
    pub fn build_query_params(&self) -> MmResult<QueryParams, ApiClientError> {
        let mut params = vec![
            ("chain_id", self.chain_id.to_string()),
            ("token0_address", self.token0_address.clone()),
            ("token1_address", self.token1_address.clone()),
        ];

        push_if_some!(params, "granularity", &self.granularity);
        push_if_some!(params, "limit", &self.limit);

        Ok(params)
    }
}

/// Element of token_0/token_1 price series returned from the 1inch cross_prices call.
/// Contains OHLC (Open, High, Low, Close) prices for the granularity period.
/// TODO: check cross_prices v2
#[derive(Clone, Deserialize, Debug)]
pub struct CrossPricesData {
    /// Time of the granularity period
    pub timestamp: u64,
    /// Price at the period opening
    pub open: BigDecimal,
    /// Lowest price within the period
    pub low: BigDecimal,
    /// Average price within the period
    pub avg: BigDecimal,
    /// Highest price within the period
    pub high: BigDecimal,
    /// Price at the period closing
    pub close: BigDecimal,
}

pub type CrossPricesSeries = Vec<CrossPricesData>;
