use crate::eth::{decode_contract_call, signed_tx_from_web3_tx, EthCoin, EthCoinType, Transaction, TransactionErr};
use crate::hd_wallet::DisplayAddress;
use crate::{FindPaymentSpendError, MarketCoinOps};
use common::executor::Timer;
use common::log::{error, info};
use common::now_sec;
use derive_more::Display;
use enum_derives::EnumFromStringify;
use ethabi::{Contract, Token};
use ethcore_transaction::{Action, SignedTransaction as SignedEthTx};
use ethereum_types::{Address, H256, U256};
use futures::compat::Future01CompatExt;
use mm2_err_handle::prelude::{MmError, MmResult};
use mm2_number::BigDecimal;
use num_traits::Signed;
use web3::types::TransactionId;

pub(crate) mod eth_maker_swap_v2;
pub(crate) mod eth_taker_swap_v2;

/// ZERO_VALUE is used to represent a 0 amount in transactions where the value is encoded in the transaction input data.
/// This is typically used in function calls where the value is not directly transferred with the transaction, such as in
/// `spendTakerPayment` where the [amount](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapTakerV2.sol#L166)
/// is provided as part of the input data rather than as an Ether value
pub(crate) const ZERO_VALUE: u32 = 0;

pub enum EthPaymentType {
    MakerPayments,
    TakerPayments,
}

impl EthPaymentType {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            EthPaymentType::MakerPayments => "makerPayments",
            EthPaymentType::TakerPayments => "takerPayments",
        }
    }
}

pub enum PaymentMethod {
    Send,
    Spend,
    RefundTimelock,
    RefundSecret,
}

#[derive(Debug, Display)]
pub(crate) enum ValidatePaymentV2Err {
    WrongPaymentTx(String),
}

#[derive(Debug, Display, EnumFromStringify)]
pub(crate) enum PrepareTxDataError {
    #[from_stringify("ethabi::Error")]
    #[display(fmt = "ABI error: {_0}")]
    ABIError(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
    #[display(fmt = "Invalid data error: {_0}")]
    InvalidData(String),
}

pub(crate) struct SpendTxSearchParams<'a> {
    pub(crate) swap_contract_address: Address,
    pub(crate) event_name: &'a str,
    pub(crate) abi_contract: &'a Contract,
    pub(crate) swap_id: &'a [u8; 32],
    pub(crate) from_block: u64,
    pub(crate) wait_until: u64,
    pub(crate) check_every: f64,
}

impl EthCoin {
    pub(super) fn get_token_address(&self) -> Result<Address, String> {
        match &self.coin_type {
            EthCoinType::Eth => Ok(Address::default()),
            EthCoinType::Erc20 { token_addr, .. } => Ok(*token_addr),
            EthCoinType::Nft { .. } => Err("NFT protocol is not supported for ETH and ERC20 Swaps".to_string()),
        }
    }

    /// A helper function that scans blocks for a specific event containing the given `swap_id`,
    /// returning transaction hash of spend transaction once found.
    /// **NOTE:** The current function implementation assumes that `swap_id` is the first 32 bytes of the transaction input data.
    pub(crate) async fn find_transaction_hash_by_event(
        &self,
        params: SpendTxSearchParams<'_>,
    ) -> MmResult<H256, FindPaymentSpendError> {
        loop {
            let now = now_sec();
            if now > params.wait_until {
                return MmError::err(FindPaymentSpendError::Timeout {
                    wait_until: params.wait_until,
                    now,
                });
            }

            let current_block = match self.current_block().compat().await {
                Ok(b) => b,
                Err(e) => {
                    error!("Error getting block number: {}", e);
                    Timer::sleep(params.check_every).await;
                    continue;
                },
            };

            let mut next_from_block = params.from_block;
            while next_from_block <= current_block {
                let to_block = std::cmp::min(next_from_block + self.logs_block_range - 1, current_block);

                // Fetch events for the current block range
                let events = match self
                    .events_from_block(
                        params.swap_contract_address,
                        params.event_name,
                        next_from_block,
                        Some(to_block),
                        params.abi_contract,
                    )
                    .await
                {
                    Ok(events) => events,
                    Err(e) => {
                        error!(
                            "Error getting {} events from {} to {} block: {}",
                            params.event_name, next_from_block, to_block, e
                        );
                        Timer::sleep(params.check_every).await;
                        continue;
                    },
                };

                // Check if any event matches the SWAP ID
                if let Some(found_event) = events
                    .into_iter()
                    .find(|event| event.data.0.len() >= 32 && &event.data.0[..32] == params.swap_id)
                {
                    if let Some(hash) = found_event.transaction_hash {
                        return Ok(hash);
                    }
                }

                next_from_block += self.logs_block_range;
            }

            Timer::sleep(params.check_every).await;
        }
    }

    /// Waits until the specified transaction is found by its hash or the given timeout is reached
    pub(crate) async fn wait_for_transaction(
        &self,
        tx_hash: H256,
        wait_until: u64,
        check_every: f64,
    ) -> MmResult<SignedEthTx, FindPaymentSpendError> {
        loop {
            let now = now_sec();
            if now > wait_until {
                return MmError::err(FindPaymentSpendError::Timeout { wait_until, now });
            }

            match self.transaction(TransactionId::Hash(tx_hash)).await {
                Ok(Some(t)) => {
                    let transaction = signed_tx_from_web3_tx(t).map_err(FindPaymentSpendError::Internal)?;
                    return Ok(transaction);
                },
                Ok(None) => info!("Transaction {} not found yet", tx_hash),
                Err(e) => error!("Get transaction {} error: {}", tx_hash, e),
            };

            Timer::sleep(check_every).await;
        }
    }
}

pub(crate) fn validate_from_to_addresses(
    signed_tx: &SignedEthTx,
    expected_from: Address,
    expected_to: Address,
) -> Result<(), MmError<ValidatePaymentV2Err>> {
    if signed_tx.sender() != expected_from {
        return MmError::err(ValidatePaymentV2Err::WrongPaymentTx(format!(
            "Payment tx {signed_tx:?} was sent from wrong address, expected {}",
            expected_from.display_address()
        )));
    }

    // (in NFT case) as NFT owner calls "safeTransferFrom" directly, then in Transaction 'to' field we expect token_address
    match signed_tx.unsigned().action() {
        Action::Call(to) => {
            if *to != expected_to {
                return MmError::err(ValidatePaymentV2Err::WrongPaymentTx(format!(
                    "Payment tx was sent to wrong address, expected {}, got {}",
                    expected_to.display_address(),
                    to.display_address()
                )));
            }
        },
        Action::Create => {
            return MmError::err(ValidatePaymentV2Err::WrongPaymentTx(
                "Tx action must be Call, found Create instead".to_string(),
            ));
        },
    }

    Ok(())
}

// TODO validate premium when add its support in swap_v2
fn validate_amount(trading_amount: &BigDecimal) -> Result<(), String> {
    if !trading_amount.is_positive() {
        return Err("trading_amount must be a positive value".to_string());
    }
    Ok(())
}

fn check_decoded_length(decoded: &[Token], expected_len: usize) -> Result<(), PrepareTxDataError> {
    if decoded.len() != expected_len {
        return Err(PrepareTxDataError::Internal(format!(
            "Invalid number of tokens in decoded. Expected {}, found {}",
            expected_len,
            decoded.len()
        )));
    }
    Ok(())
}

impl EthCoin {
    async fn handle_allowance(
        &self,
        swap_contract: Address,
        payment_amount: U256,
        time_lock: u64,
    ) -> Result<(), TransactionErr> {
        let allowed = self
            .allowance(swap_contract)
            .compat()
            .await
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;

        if allowed < payment_amount {
            let approved_tx = self.approve(swap_contract, U256::max_value()).compat().await?;
            self.wait_for_required_allowance(swap_contract, payment_amount, time_lock)
                .compat()
                .await
                .map_err(|e| {
                    TransactionErr::Plain(ERRL!(
                        "Allowed value was not updated in time after sending approve transaction {:02x}: {}",
                        approved_tx.tx_hash_as_bytes(),
                        e
                    ))
                })?;
        }
        Ok(())
    }
}

pub(crate) async fn extract_id_from_tx_data(
    tx_data: &[u8],
    abi_contract: &Contract,
    func_name: &str,
) -> Result<Vec<u8>, FindPaymentSpendError> {
    let func = abi_contract.function(func_name)?;
    let decoded = decode_contract_call(func, tx_data)?;
    match decoded.first() {
        Some(Token::FixedBytes(bytes)) => Ok(bytes.clone()),
        invalid_token => Err(FindPaymentSpendError::InvalidData(format!(
            "Expected Token::FixedBytes, got {invalid_token:?}"
        ))),
    }
}
