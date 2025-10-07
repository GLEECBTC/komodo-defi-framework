use crate::qrc20::rpc_clients::Qrc20ElectrumOps;
use crate::qrc20::script_pubkey::generate_contract_call_script_pubkey;
use crate::qrc20::{
    contract_addr_into_rpc_format, ContractCallOutput, GenerateQrc20TxResult, Qrc20AbiError, Qrc20FeeDetails,
    OUTPUT_QTUM_AMOUNT, QRC20_DUST, QRC20_GAS_LIMIT_DEFAULT, QRC20_GAS_PRICE_DEFAULT,
};
use crate::utxo::qtum::{QtumBasedCoin, QtumCoin, QtumDelegationOps, QtumDelegationRequest, QtumStakingInfosDetails};
use crate::utxo::rpc_clients::UtxoRpcClientEnum;
use crate::utxo::utxo_common::{big_decimal_from_sat_unsigned, UtxoTxBuilder};
use crate::utxo::{qtum, utxo_common, Address, GetUtxoListOps, UtxoCommonOps};
use crate::utxo::{PrivKeyPolicyNotAllowed, UTXO_LOCK};
use crate::{
    DelegationError, DelegationFut, DelegationResult, MarketCoinOps, StakingInfoError, StakingInfos, StakingInfosFut,
    TransactionData, TransactionDetails, TransactionType,
};
use bitcrypto::dhash256;
use common::now_sec;
use derive_more::Display;
use ethabi::{Contract, Token};
use ethereum_types::H160;
use futures::compat::Future01CompatExt;
use futures::{FutureExt, TryFutureExt};
use keys::{AddressHashEnum, Signature};
use mm2_err_handle::prelude::*;
use mm2_number::bigdecimal::{BigDecimal, Zero};
use rpc::v1::types::ToTxHash;
use serialization::serialize;
use std::convert::TryInto;
use std::str::FromStr;
use utxo_signer::with_key_pair::sign_tx;

pub const QTUM_DELEGATION_STANDARD_FEE: u64 = 10;
pub const QTUM_LOWER_BOUND_DELEGATION_AMOUNT: f64 = 100.0;
pub const QRC20_GAS_LIMIT_DELEGATION: u64 = 2_250_000;
pub const QTUM_ADD_DELEGATION_TOPIC: &str = "a23803f3b2b56e71f2921c22b23c32ef596a439dbe03f7250e6b58a30eb910b5";
pub const QTUM_REMOVE_DELEGATION_TOPIC: &str = "7fe28d2d0b16cf95b5ea93f4305f89133b3892543e616381a1336fc1e7a01fa0";
const QTUM_DELEGATE_CONTRACT_ABI: &str = r#"[{"anonymous":false,"inputs":[{"indexed":true,"internalType":"address","name":"_staker","type":"address"},{"indexed":true,"internalType":"address","name":"_delegate","type":"address"},{"indexed":false,"internalType":"uint8","name":"fee","type":"uint8"},{"indexed":false,"internalType":"uint256","name":"blockHeight","type":"uint256"},{"indexed":false,"internalType":"bytes","name":"PoD","type":"bytes"}],"name":"AddDelegation","type":"event"},{"anonymous":false,"inputs":[{"indexed":true,"internalType":"address","name":"_staker","type":"address"},{"indexed":true,"internalType":"address","name":"_delegate","type":"address"}],"name":"RemoveDelegation","type":"event"},{"constant":false,"inputs":[{"internalType":"address","name":"_staker","type":"address"},{"internalType":"uint8","name":"_fee","type":"uint8"},{"internalType":"bytes","name":"_PoD","type":"bytes"}],"name":"addDelegation","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"},{"constant":true,"inputs":[{"internalType":"address","name":"","type":"address"}],"name":"delegations","outputs":[{"internalType":"address","name":"staker","type":"address"},{"internalType":"uint8","name":"fee","type":"uint8"},{"internalType":"uint256","name":"blockHeight","type":"uint256"},{"internalType":"bytes","name":"PoD","type":"bytes"}],"payable":false,"stateMutability":"view","type":"function"},{"constant":false,"inputs":[],"name":"removeDelegation","outputs":[],"payable":false,"stateMutability":"nonpayable","type":"function"}]"#;

lazy_static! {
    pub static ref QTUM_DELEGATE_CONTRACT: Contract = Contract::load(QTUM_DELEGATE_CONTRACT_ABI.as_bytes()).unwrap();
    pub static ref QTUM_DELEGATE_CONTRACT_ADDRESS: H160 =
        H160::from_str("0000000000000000000000000000000000000086").unwrap();
}

pub type QtumStakingAbiResult<T> = Result<T, MmError<QtumStakingAbiError>>;
type StakingInfosResult = Result<StakingInfos, MmError<StakingInfoError>>;

#[derive(Debug, Display)]
pub enum QtumStakingAbiError {
    #[display(fmt = "Invalid QRC20 ABI params: {_0}")]
    InvalidParams(String),
    #[display(fmt = "QRC20 ABI error: {_0}")]
    ABIError(String),
    #[display(fmt = "Qtum POD error: {_0}")]
    PodSigningError(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<Qrc20AbiError> for QtumStakingAbiError {
    fn from(e: Qrc20AbiError) -> Self {
        match e {
            Qrc20AbiError::InvalidParams(e) => QtumStakingAbiError::InvalidParams(e),
            Qrc20AbiError::ABIError(e) => QtumStakingAbiError::ABIError(e),
        }
    }
}

impl From<QtumStakingAbiError> for DelegationError {
    fn from(e: QtumStakingAbiError) -> Self {
        DelegationError::CannotInteractWithSmartContract(e.to_string())
    }
}

impl From<ethabi::Error> for QtumStakingAbiError {
    fn from(e: ethabi::Error) -> QtumStakingAbiError {
        QtumStakingAbiError::ABIError(e.to_string())
    }
}

impl From<ethabi::Error> for DelegationError {
    fn from(e: ethabi::Error) -> Self {
        DelegationError::from(QtumStakingAbiError::from(e))
    }
}

impl From<Qrc20AbiError> for DelegationError {
    fn from(e: Qrc20AbiError) -> Self {
        DelegationError::from(QtumStakingAbiError::from(e))
    }
}

impl From<PrivKeyPolicyNotAllowed> for QtumStakingAbiError {
    fn from(e: PrivKeyPolicyNotAllowed) -> Self {
        QtumStakingAbiError::Internal(e.to_string())
    }
}

impl QtumDelegationOps for QtumCoin {
    fn add_delegation(&self, request: QtumDelegationRequest) -> DelegationFut {
        let coin = self.clone();
        let fut = async move { coin.add_delegation_impl(request).await };
        Box::new(fut.boxed().compat())
    }

    fn get_delegation_infos(&self) -> StakingInfosFut {
        let coin = self.clone();
        let fut = async move { coin.get_delegation_infos_impl().await };
        Box::new(fut.boxed().compat())
    }

    fn remove_delegation(&self) -> DelegationFut {
        let coin = self.clone();
        let fut = async move { coin.remove_delegation_impl().await };
        Box::new(fut.boxed().compat())
    }

    fn generate_pod(&self, addr_hash: AddressHashEnum) -> Result<Signature, MmError<DelegationError>> {
        let mut buffer = b"\x15Qtum Signed Message:\n\x28".to_vec();
        buffer.append(&mut addr_hash.to_string().into_bytes());
        let hashed = dhash256(&buffer);
        let key_pair = self.as_ref().priv_key_policy.activated_key_or_err().map_mm_err()?;
        let signature = key_pair
            .private()
            .sign_compact(&hashed)
            .map_to_mm(|e| QtumStakingAbiError::PodSigningError(e.to_string()))
            .map_mm_err()?;
        Ok(signature)
    }
}

impl QtumCoin {
    async fn remove_delegation_impl(&self) -> DelegationResult {
        if self.addr_format().is_segwit() {
            return MmError::err(DelegationError::DelegationOpsNotSupported {
                reason: "Qtum doesn't support delegation for segwit".to_string(),
            });
        }
        let delegation_output = self
            .remove_delegation_output(QRC20_GAS_LIMIT_DEFAULT, QRC20_GAS_PRICE_DEFAULT)
            .map_mm_err()?;
        let outputs = vec![delegation_output];
        let my_address = self.my_address().map_mm_err()?;
        self.generate_delegation_transaction(
            outputs,
            my_address,
            QRC20_GAS_LIMIT_DEFAULT,
            TransactionType::RemoveDelegation,
        )
        .await
    }

    async fn am_i_currently_staking(&self) -> Result<Option<String>, MmError<StakingInfoError>> {
        let utxo = self.as_ref();
        let contract_address = contract_addr_into_rpc_format(&QTUM_DELEGATE_CONTRACT_ADDRESS);
        let client = match &utxo.rpc_client {
            UtxoRpcClientEnum::Native(_) => {
                return MmError::err(StakingInfoError::Internal("Native not supported".to_string()))
            },
            UtxoRpcClientEnum::Electrum(electrum) => electrum,
        };
        let address = self.my_addr_as_contract_addr().await.map_mm_err()?;
        let address_rpc = contract_addr_into_rpc_format(&address);
        let add_delegation_history = client
            .blockchain_contract_event_get_history(&address_rpc, &contract_address, QTUM_ADD_DELEGATION_TOPIC)
            .compat()
            .await
            .map_to_mm(|e| StakingInfoError::Transport(e.to_string()))?;
        let remove_delegation_history = client
            .blockchain_contract_event_get_history(&address_rpc, &contract_address, QTUM_REMOVE_DELEGATION_TOPIC)
            .compat()
            .await
            .map_to_mm(|e| StakingInfoError::Transport(e.to_string()))?;
        let am_i_staking = add_delegation_history.len() > remove_delegation_history.len();
        if am_i_staking {
            let last_tx_add = some_or_return_ok_none!(add_delegation_history.last());
            let res = &client
                .blockchain_transaction_get_receipt(&last_tx_add.tx_hash)
                .compat()
                .await
                .map_to_mm(|e| StakingInfoError::Transport(e.to_string()))?;
            // there is only 3 topics for an add_delegation
            // the first entry is the operation (add_delegation / remove_delegation),
            // the second entry is always the staker as hexadecimal 32 byte padded
            // by trimming the start we retrieve the standard hex hash format
            // https://testnet.qtum.info/tx/c62d707b67267a13a53b5910ffbf393c47f00734cff1c73aae6e05d24258372f
            // topic[0] -> a23803f3b2b56e71f2921c22b23c32ef596a439dbe03f7250e6b58a30eb910b5 -> add_delegation_topic
            // topic[1] -> 000000000000000000000000d4ea77298fdac12c657a18b222adc8b307e18127 -> staker_address
            // topic[2] -> 0000000000000000000000006d9d2b554d768232320587df75c4338ecc8bf37d

            return if let Some(raw) = res
                .iter()
                .find(|receipt| {
                    receipt
                        .log
                        .iter()
                        .any(|e| !e.topics.is_empty() && e.topics[0] == QTUM_ADD_DELEGATION_TOPIC)
                })
                .and_then(|receipt| {
                    receipt
                        .log
                        .first()
                        .and_then(|log_entry| log_entry.topics.get(1))
                        .map(|padded_staker_address_hex| padded_staker_address_hex.trim_start_matches('0'))
                }) {
                let hash = H160::from_str(raw).map_to_mm(|e| StakingInfoError::Internal(e.to_string()))?;
                let address = self.utxo_addr_from_contract_addr(hash);
                Ok(Some(address.to_string()))
            } else {
                Ok(None)
            };
        }
        Ok(None)
    }

    async fn get_delegation_infos_impl(&self) -> StakingInfosResult {
        let coin = self.as_ref();
        let my_address = coin.derivation_method.single_addr_or_err().await.map_mm_err()?;

        let staker = self.am_i_currently_staking().await?;
        let (unspents, _) = self.get_unspent_ordered_list(&my_address).await.map_mm_err()?;
        let lower_bound = QTUM_LOWER_BOUND_DELEGATION_AMOUNT
            .try_into()
            .expect("Conversion should succeed");
        let mut amount = BigDecimal::zero();
        if staker.is_some() {
            amount = unspents
                .iter()
                .map(|unspent| big_decimal_from_sat_unsigned(unspent.value, coin.decimals))
                .filter(|unspent_value| unspent_value >= &lower_bound)
                .fold(BigDecimal::zero(), |total, unspent_value| total + unspent_value);
        }
        let am_i_staking = staker.is_some();
        let infos = StakingInfos {
            staking_infos_details: QtumStakingInfosDetails {
                amount,
                staker,
                am_i_staking,
                is_staking_supported: !my_address.addr_format().is_segwit(),
            }
            .into(),
        };
        Ok(infos)
    }

    async fn add_delegation_impl(&self, request: QtumDelegationRequest) -> DelegationResult {
        if self.addr_format().is_segwit() {
            return MmError::err(DelegationError::DelegationOpsNotSupported {
                reason: "Qtum doesn't support delegation for segwit".to_string(),
            });
        }
        if let Some(staking_addr) = self.am_i_currently_staking().await.map_mm_err()? {
            return MmError::err(DelegationError::AlreadyDelegating(staking_addr));
        }
        let to_addr =
            Address::from_legacyaddress(request.validator_address.as_str(), &self.as_ref().conf.address_prefixes)
                .map_to_mm(DelegationError::AddressError)?;
        let fee = request.fee.unwrap_or(QTUM_DELEGATION_STANDARD_FEE);
        let _utxo_lock = UTXO_LOCK.lock();
        let staker_address_hex = qtum::contract_addr_from_utxo_addr(to_addr.clone()).map_mm_err()?;
        let delegation_output = self.add_delegation_output(
            staker_address_hex,
            to_addr.hash().clone(),
            fee,
            QRC20_GAS_LIMIT_DELEGATION,
            QRC20_GAS_PRICE_DEFAULT,
        )?;

        let outputs = vec![delegation_output];
        let my_address = self.my_address().map_mm_err()?;
        self.generate_delegation_transaction(
            outputs,
            my_address,
            QRC20_GAS_LIMIT_DELEGATION,
            TransactionType::StakingDelegation,
        )
        .await
    }

    async fn generate_delegation_transaction(
        &self,
        contract_outputs: Vec<ContractCallOutput>,
        to_address: String,
        gas_limit: u64,
        transaction_type: TransactionType,
    ) -> DelegationResult {
        let utxo = self.as_ref();

        let key_pair = utxo.priv_key_policy.activated_key_or_err().map_mm_err()?;
        let my_address = utxo.derivation_method.single_addr_or_err().await.map_mm_err()?;

        let (unspents, _) = self.get_unspent_ordered_list(&my_address).await.map_mm_err()?;
        let mut gas_fee = 0;
        let mut outputs = Vec::with_capacity(contract_outputs.len());
        for output in contract_outputs {
            gas_fee += output.gas_limit * output.gas_price;
            outputs.push(output.into());
        }

        let (unsigned, data) = UtxoTxBuilder::new(self)
            .await
            .add_available_inputs(unspents)
            .add_outputs(outputs)
            .with_gas_fee(gas_fee)
            .with_dust(QRC20_DUST)
            .build()
            .await
            .mm_err(|gen_tx_error| {
                DelegationError::from_generate_tx_error(gen_tx_error, self.ticker().to_string(), utxo.decimals)
            })?;

        let signed = sign_tx(unsigned, key_pair, utxo.conf.signature_version, utxo.conf.fork_id).map_mm_err()?;

        let generated_tx = GenerateQrc20TxResult {
            signed,
            miner_fee: data.fee_amount,
            gas_fee,
        };

        let fee_details = Qrc20FeeDetails {
            // QRC20 fees are paid in base platform currency (in particular Qtum)
            coin: self.ticker().to_string(),
            miner_fee: utxo_common::big_decimal_from_sat(generated_tx.miner_fee as i64, utxo.decimals),
            gas_limit,
            gas_price: QRC20_GAS_PRICE_DEFAULT,
            total_gas_fee: utxo_common::big_decimal_from_sat(generated_tx.gas_fee as i64, utxo.decimals),
        };
        let my_address_string = self.my_address().map_mm_err()?;

        let spent_by_me = utxo_common::big_decimal_from_sat(data.spent_by_me as i64, utxo.decimals);
        let qtum_amount = spent_by_me.clone();
        let received_by_me = utxo_common::big_decimal_from_sat(data.received_by_me as i64, utxo.decimals);
        let my_balance_change = &received_by_me - &spent_by_me;

        Ok(TransactionDetails {
            tx: TransactionData::new_signed(
                serialize(&generated_tx.signed).into(),
                generated_tx.signed.hash().reversed().to_vec().to_tx_hash(),
            ),
            from: vec![my_address_string],
            to: vec![to_address],
            total_amount: qtum_amount,
            spent_by_me,
            received_by_me,
            my_balance_change,
            block_height: 0,
            timestamp: now_sec(),
            fee_details: Some(fee_details.into()),
            coin: self.ticker().to_string(),
            internal_id: vec![].into(),
            kmd_rewards: None,
            transaction_type,
            memo: None,
        })
    }

    fn remove_delegation_output(&self, gas_limit: u64, gas_price: u64) -> QtumStakingAbiResult<ContractCallOutput> {
        let function: &ethabi::Function = QTUM_DELEGATE_CONTRACT.function("removeDelegation")?;
        let params = function.encode_input(&[])?;
        let script_pubkey = generate_contract_call_script_pubkey(
            &params,
            gas_limit,
            gas_price,
            QTUM_DELEGATE_CONTRACT_ADDRESS.as_bytes(),
        )
        .map_mm_err()?
        .to_bytes();
        Ok(ContractCallOutput {
            value: OUTPUT_QTUM_AMOUNT,
            script_pubkey,
            gas_limit,
            gas_price,
        })
    }

    #[allow(clippy::result_large_err)]
    fn add_delegation_output(
        &self,
        to_addr: H160,
        addr_hash: AddressHashEnum,
        fee: u64,
        gas_limit: u64,
        gas_price: u64,
    ) -> Result<ContractCallOutput, MmError<DelegationError>> {
        let function: &ethabi::Function = QTUM_DELEGATE_CONTRACT.function("addDelegation")?;
        let pod = self.generate_pod(addr_hash)?;
        let params = function.encode_input(&[
            Token::Address(to_addr),
            Token::Uint(fee.into()),
            Token::Bytes(pod.into()),
        ])?;

        let script_pubkey = generate_contract_call_script_pubkey(
            &params,
            gas_limit,
            gas_price,
            QTUM_DELEGATE_CONTRACT_ADDRESS.as_bytes(),
        )
        .map_mm_err()?
        .to_bytes();
        Ok(ContractCallOutput {
            value: OUTPUT_QTUM_AMOUNT,
            script_pubkey,
            gas_limit,
            gas_price,
        })
    }
}
