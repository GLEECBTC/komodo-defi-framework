use super::history::TransferHistoryBuilder;
use super::*;
use crate::eth::{decode_contract_call, PaymentState};
use bitcrypto::ripemd160;
use common::now_sec;
use script_pubkey::{extract_contract_addr_from_script, extract_contract_call_from_script, is_contract_call};

/// `erc20Payment` call details consist of values obtained from [`TransactionOutput::script_pubkey`] and [`TxReceipt::logs`].
#[derive(Debug, Eq, PartialEq)]
pub struct Erc20PaymentDetails {
    pub output_index: u64,
    pub swap_id: Vec<u8>,
    pub value: U256,
    pub token_address: H160,
    pub swap_contract_address: H160,
    pub sender: H160,
    pub receiver: H160,
    pub secret_hash: Vec<u8>,
    pub timelock: U256,
    /// Contract call bytes extracted from [`TransactionOutput::script_pubkey`] using `extract_contract_call_from_script`.
    pub contract_call_bytes: Vec<u8>,
}

/// `receiverSpend` call details consist of values obtained from [`TransactionOutput::script_pubkey`].
#[expect(dead_code)]
#[derive(Debug)]
pub struct ReceiverSpendDetails {
    pub swap_id: Vec<u8>,
    pub value: U256,
    pub secret: [u8; 32],
    pub token_address: H160,
    pub sender: H160,
}

impl Qrc20Coin {
    pub async fn send_hash_time_locked_payment(
        &self,
        id: Vec<u8>,
        value: U256,
        time_lock: u32,
        secret_hash: Vec<u8>,
        receiver_addr: H160,
        swap_contract_address: H160,
    ) -> Result<TransactionEnum, TransactionErr> {
        let secret_hash = if secret_hash.len() == 32 {
            ripemd160(&secret_hash).to_vec()
        } else {
            secret_hash
        };

        let balance = try_tx_s!(self.my_spendable_balance().compat().await);
        let balance = try_tx_s!(u256_from_big_decimal(&balance, self.utxo.decimals));

        // Check the balance to avoid unnecessary burning of gas
        if balance < value {
            return TX_PLAIN_ERR!("Balance {} is less than value {}", balance, value);
        }

        let outputs = try_tx_s!(
            self.generate_swap_payment_outputs(
                balance,
                id,
                value,
                time_lock,
                secret_hash,
                receiver_addr,
                swap_contract_address,
            )
            .await
        );

        self.send_contract_calls(outputs).await
    }

    pub async fn spend_hash_time_locked_payment(
        &self,
        payment_tx: UtxoTx,
        swap_contract_address: H160,
        secret: Vec<u8>,
    ) -> Result<TransactionEnum, TransactionErr> {
        let Erc20PaymentDetails {
            swap_id, value, sender, ..
        } = try_tx_s!(self.erc20_payment_details_from_tx(&payment_tx).await);

        let status = try_tx_s!(self.payment_status(&swap_contract_address, swap_id.clone()).await);
        if status != U256::from(PaymentState::Sent as u8) {
            return TX_PLAIN_ERR!("Payment state is not PAYMENT_STATE_SENT, got {}", status);
        }

        let spend_output =
            try_tx_s!(self.receiver_spend_output(&swap_contract_address, swap_id, value, secret, sender));
        self.send_contract_calls(vec![spend_output]).await
    }

    pub async fn refund_hash_time_locked_payment(
        &self,
        swap_contract_address: H160,
        payment_tx: UtxoTx,
    ) -> Result<TransactionEnum, TransactionErr> {
        let Erc20PaymentDetails {
            swap_id,
            value,
            receiver,
            secret_hash,
            ..
        } = try_tx_s!(self.erc20_payment_details_from_tx(&payment_tx).await);

        let status = try_tx_s!(self.payment_status(&swap_contract_address, swap_id.clone()).await);
        if status != U256::from(PaymentState::Sent as u8) {
            return TX_PLAIN_ERR!("Payment state is not PAYMENT_STATE_SENT, got {}", status);
        }

        let refund_output =
            try_tx_s!(self.sender_refund_output(&swap_contract_address, swap_id, value, secret_hash, receiver));
        self.send_contract_calls(vec![refund_output]).await
    }

    pub async fn validate_payment(
        &self,
        payment_tx: UtxoTx,
        time_lock: u32,
        sender: H160,
        secret_hash: Vec<u8>,
        amount: BigDecimal,
        expected_swap_contract_address: H160,
    ) -> Result<(), MmError<ValidatePaymentError>> {
        let expected_swap_id = qrc20_swap_id(time_lock, &secret_hash);
        let status = self
            .payment_status(&expected_swap_contract_address, expected_swap_id.clone())
            .await
            .map_mm_err()?;
        if status != U256::from(PaymentState::Sent as u8) {
            return MmError::err(ValidatePaymentError::UnexpectedPaymentState(format!(
                "Payment state is not PAYMENT_STATE_SENT, got {status}"
            )));
        }

        let expected_call_bytes = {
            let expected_value = u256_from_big_decimal(&amount, self.utxo.decimals).map_mm_err()?;
            let my_address = self.utxo.derivation_method.single_addr_or_err().await.map_mm_err()?;
            let expected_receiver = qtum::contract_addr_from_utxo_addr(my_address)
                .mm_err(|err| ValidatePaymentError::InternalError(err.to_string()))?;
            self.erc20_payment_call_bytes(
                expected_swap_id,
                expected_value,
                time_lock,
                &secret_hash,
                expected_receiver,
            )
            .map_mm_err()?
        };
        let erc20_payment = self
            .erc20_payment_details_from_tx(&payment_tx)
            .await
            .map_to_mm(ValidatePaymentError::TxDeserializationError)?;
        if erc20_payment.contract_call_bytes != expected_call_bytes {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Unexpected 'erc20Payment' contract call bytes: {:?}",
                erc20_payment.contract_call_bytes
            )));
        }

        if sender != erc20_payment.sender {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Payment tx {:?} was sent from wrong address, expected {:?}",
                erc20_payment.sender, sender
            )));
        }

        if expected_swap_contract_address != erc20_payment.swap_contract_address {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Payment tx receiver arg {:?} is invalid, expected {:?}",
                erc20_payment.swap_contract_address, expected_swap_contract_address,
            )));
        }

        Ok(())
    }

    pub async fn validate_fee_impl(
        &self,
        fee_tx_hash: H256Json,
        fee_addr: H160,
        expected_value: U256,
        min_block_number: u64,
    ) -> Result<(), String> {
        let verbose_tx = try_s!(
            self.utxo
                .rpc_client
                .get_verbose_transaction(&fee_tx_hash)
                .compat()
                .await
        );
        let conf_before_block = utxo_common::is_tx_confirmed_before_block(self, &verbose_tx, min_block_number);
        if try_s!(conf_before_block.await) {
            return ERR!(
                "Fee tx {:?} confirmed before min_block {}",
                verbose_tx,
                min_block_number,
            );
        }
        let qtum_tx: UtxoTx = try_s!(deserialize(verbose_tx.hex.as_slice()).map_err(|e| ERRL!("{:?}", e)));

        // The transaction could not being mined, just check the transfer tokens.
        let output = qtum_tx
            .outputs
            .first()
            .ok_or(ERRL!("Provided dex fee tx {:?} has no outputs", qtum_tx))?;
        let script_pubkey: Script = output.script_pubkey.clone().into();

        let (receiver, value) = match transfer_call_details_from_script_pubkey(&script_pubkey) {
            Ok((rec, val)) => (rec, val),
            Err(e) => return ERR!("Provided dex fee tx {:?} is incorrect: {}", qtum_tx, e),
        };

        if receiver != fee_addr {
            return ERR!(
                "QRC20 Fee tx was sent to wrong address {:?}, expected {:?}",
                receiver,
                fee_addr
            );
        }

        if value < expected_value {
            return ERR!("QRC20 Fee tx value {} is less than expected {}", value, expected_value);
        }

        let token_addr = try_s!(extract_contract_addr_from_script(&script_pubkey));
        if token_addr != self.contract_address {
            return ERR!(
                "QRC20 Fee tx {:?} called wrong smart contract, expected {:?}",
                qtum_tx,
                self.contract_address
            );
        }

        Ok(())
    }

    pub async fn search_for_swap_tx_spend(
        &self,
        time_lock: u32,
        secret_hash: &[u8],
        tx: UtxoTx,
        search_from_block: u64,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        let tx_hash = tx.hash().reversed().into();
        let verbose_tx = try_s!(self.utxo.rpc_client.get_verbose_transaction(&tx_hash).compat().await);
        if verbose_tx.confirmations < 1 {
            return ERR!("'erc20Payment' was not confirmed yet. Please wait for at least one confirmation");
        }

        let Erc20PaymentDetails { swap_id, receiver, .. } = try_s!(self.erc20_payment_details_from_tx(&tx).await);
        let expected_swap_id = qrc20_swap_id(time_lock, secret_hash);
        if expected_swap_id != swap_id {
            return ERR!("Unexpected swap_id {}", hex::encode(swap_id));
        }

        // First try to find a 'receiverSpend' contract call.
        let spend_txs = try_s!(self.receiver_spend_transactions(receiver, search_from_block).await);
        let found = spend_txs
            .into_iter()
            .find(|tx| find_receiver_spend_with_swap_id_and_secret_hash(tx, &expected_swap_id, secret_hash).is_some());
        if let Some(spent_tx) = found {
            return Ok(Some(FoundSwapTxSpend::Spent(TransactionEnum::UtxoTx(spent_tx))));
        }

        // Else try to find a 'senderRefund' contract call.
        let my_address = try_s!(self.utxo.derivation_method.single_addr_or_err().await);
        let sender = try_s!(qtum::contract_addr_from_utxo_addr(my_address));
        let refund_txs = try_s!(self.sender_refund_transactions(sender, search_from_block).await);
        let found = refund_txs.into_iter().find(|tx| {
            find_swap_contract_call_with_swap_id(MutContractCallType::SenderRefund, tx, &expected_swap_id).is_some()
        });
        if let Some(refunded_tx) = found {
            return Ok(Some(FoundSwapTxSpend::Refunded(TransactionEnum::UtxoTx(refunded_tx))));
        }

        Ok(None)
    }

    pub async fn check_if_my_payment_sent_impl(
        &self,
        swap_contract_address: H160,
        swap_id: Vec<u8>,
        search_from_block: u64,
    ) -> Result<Option<TransactionEnum>, String> {
        let status = try_s!(self.payment_status(&swap_contract_address, swap_id.clone()).await);
        if status == U256::from(PaymentState::Uninitialized as u8) {
            return Ok(None);
        };

        let my_address = try_s!(self.utxo.derivation_method.single_addr_or_err().await);
        let sender = try_s!(qtum::contract_addr_from_utxo_addr(my_address));
        let erc20_payment_txs = try_s!(self.erc20_payment_transactions(sender, search_from_block).await);
        let found = erc20_payment_txs
            .into_iter()
            .find(|tx| find_swap_contract_call_with_swap_id(MutContractCallType::Erc20Payment, tx, &swap_id).is_some())
            .map(TransactionEnum::UtxoTx);
        Ok(found)
    }

    pub fn extract_secret_impl(&self, secret_hash: &[u8], spend_tx: &[u8]) -> Result<[u8; 32], String> {
        let secret_hash = if secret_hash.len() == 32 {
            ripemd160(secret_hash)
        } else {
            chain::hash::H160::from_slice(secret_hash)?
        };

        let spend_tx: UtxoTx = try_s!(deserialize(spend_tx).map_err(|e| ERRL!("{:?}", e)));
        let spend_tx_hash: H256Json = spend_tx.hash().reversed().into();
        for output in spend_tx.outputs {
            let script_pubkey: Script = output.script_pubkey.into();
            let ReceiverSpendDetails { secret, .. } =
                match receiver_spend_call_details_from_script_pubkey(&script_pubkey) {
                    Ok(details) => details,
                    Err(e) => {
                        error!("{}", e);
                        // try to obtain the details from the next output
                        continue;
                    },
                };

            let actual_secret_hash = dhash160(&secret);
            if actual_secret_hash != secret_hash {
                warn!(
                    "invalid 'dhash160(secret)' {:?}, expected {:?}",
                    actual_secret_hash, secret_hash,
                );
                continue;
            }

            return Ok(secret);
        }

        ERR!("Couldn't obtain the 'secret' from {:?} tx", spend_tx_hash)
    }

    pub async fn wait_for_tx_spend_impl(
        &self,
        tx: UtxoTx,
        wait_until: u64,
        from_block: u64,
        check_every: f64,
    ) -> Result<TransactionEnum, String> {
        let Erc20PaymentDetails {
            swap_id,
            receiver,
            secret_hash,
            ..
        } = try_s!(
            retry_on_err!(async { self.erc20_payment_details_from_tx(&tx).await })
                .until_ready()
                .repeat_every_secs(check_every)
                .until_s(wait_until)
                .inspect_err({
                    let tx_hash = tx.hash().reversed();
                    move |e| {
                        error!(
                            "Failed to retrieve QRC20 payment details from transaction {} \
                            will retry in {} seconds. Error: {:?}",
                            tx_hash, check_every, e
                        )
                    }
                })
                .await
        );

        loop {
            // Try to find a 'receiverSpend' contract call.
            let spend_txs = try_s!(self.receiver_spend_transactions(receiver, from_block).await);
            let found = spend_txs
                .into_iter()
                .find(|tx| find_receiver_spend_with_swap_id_and_secret_hash(tx, &swap_id, &secret_hash).is_some())
                .map(TransactionEnum::UtxoTx);

            if let Some(spent_tx) = found {
                return Ok(spent_tx);
            }

            if now_sec() > wait_until {
                return ERR!("Waited too long until {} for {:?} to be spent ", wait_until, tx);
            }
            Timer::sleep(check_every).await;
        }
    }

    pub async fn wait_for_confirmations_and_check_result(
        &self,
        qtum_tx: UtxoTx,
        confirmations: u64,
        requires_nota: bool,
        wait_until: u64,
        check_every: u64,
    ) -> Result<(), String> {
        let tx_hash = H256Json::from(qtum_tx.hash().reversed());
        try_s!(
            self.utxo
                .rpc_client
                .wait_for_confirmations(
                    tx_hash,
                    qtum_tx.expiry_height,
                    confirmations as u32,
                    requires_nota,
                    wait_until,
                    check_every
                )
                .compat()
                .await
        );
        let receipts = try_s!(self.utxo.rpc_client.get_transaction_receipts(&tx_hash).compat().await);

        for receipt in receipts {
            let output = try_s!(qtum_tx
                .outputs
                .get(receipt.output_index as usize)
                .ok_or(ERRL!("TxReceipt::output_index out of bounds")));
            let script_pubkey: Script = output.script_pubkey.clone().into();
            if !is_contract_call(&script_pubkey) {
                continue;
            }

            let contract_call_bytes = try_s!(extract_contract_call_from_script(&script_pubkey));

            let call_type = try_s!(MutContractCallType::from_script_pubkey(&contract_call_bytes));
            match call_type {
                Some(MutContractCallType::Erc20Payment)
                | Some(MutContractCallType::ReceiverSpend)
                | Some(MutContractCallType::SenderRefund) => (),
                _ => continue, // skip not etomic swap contract calls
            }

            try_s!(check_if_contract_call_completed(&receipt));
        }

        Ok(())
    }

    /// Generate `ContractCallOutput` outputs required to send a swap payment.
    /// If the wallet allowance is not enough we should set it to the wallet balance.
    #[allow(clippy::too_many_arguments)]
    pub async fn generate_swap_payment_outputs(
        &self,
        my_balance: U256,
        id: Vec<u8>,
        value: U256,
        time_lock: u32,
        secret_hash: Vec<u8>,
        receiver_addr: H160,
        swap_contract_address: H160,
    ) -> UtxoRpcResult<Vec<ContractCallOutput>> {
        let allowance = self.allowance(swap_contract_address).await?;

        let mut outputs = Vec::with_capacity(3);
        // check if we should reset the allowance to 0 and raise this to the max available value (our balance)
        if allowance < value {
            if allowance > U256::zero() {
                // first reset the allowance to the 0
                outputs.push(self.approve_output(swap_contract_address, 0.into()).map_mm_err()?);
            }
            // set the allowance from 0 to `my_balance` after the previous output is executed
            outputs.push(self.approve_output(swap_contract_address, my_balance).map_mm_err()?);
        }

        // when this output is executed, the allowance will be sufficient already
        outputs.push(
            self.erc20_payment_output(
                id,
                value,
                time_lock,
                &secret_hash,
                receiver_addr,
                &swap_contract_address,
            )
            .map_mm_err()?,
        );
        Ok(outputs)
    }

    pub async fn allowance(&self, spender: H160) -> UtxoRpcResult<U256> {
        let my_address = self
            .utxo
            .derivation_method
            .single_addr_or_err()
            .await
            .mm_err(|e| UtxoRpcError::Internal(e.to_string()))?;
        let tokens = self
            .utxo
            .rpc_client
            .rpc_contract_call(
                ViewContractCallType::Allowance,
                &self.contract_address,
                &[
                    Token::Address(
                        qtum::contract_addr_from_utxo_addr(my_address.clone())
                            .mm_err(|e| UtxoRpcError::Internal(e.to_string()))?,
                    ),
                    Token::Address(spender),
                ],
            )
            .compat()
            .await?;

        match tokens.first() {
            Some(Token::Uint(number)) => Ok(*number),
            Some(_) => {
                let error = format!(r#"Expected U256 as "allowance" result but got {tokens:?}"#);
                MmError::err(UtxoRpcError::InvalidResponse(error))
            },
            None => {
                let error = r#"Expected U256 as "allowance" result but got nothing"#.to_owned();
                MmError::err(UtxoRpcError::InvalidResponse(error))
            },
        }
    }

    /// Get payment status by `swap_id`.
    /// Do not use self swap_contract_address, because it could be updated during restart.
    async fn payment_status(&self, swap_contract_address: &H160, swap_id: Vec<u8>) -> MmResult<U256, UtxoRpcError> {
        let decoded = self
            .utxo
            .rpc_client
            .rpc_contract_call(
                ViewContractCallType::Payments,
                swap_contract_address,
                &[Token::FixedBytes(swap_id)],
            )
            .compat()
            .await?;
        if decoded.len() < 3 {
            return MmError::err(UtxoRpcError::InvalidResponse(format!(
                "Expected at least 3 tokens in \"payments\" call, found {}",
                decoded.len()
            )));
        }

        match decoded[2] {
            Token::Uint(state) => Ok(state),
            _ => MmError::err(UtxoRpcError::InvalidResponse(format!(
                "Payment status must be uint, got {:?}",
                decoded[2]
            ))),
        }
    }

    /// Generate a UTXO output with a script_pubkey that calls standard QRC20 `approve` function.
    pub fn approve_output(&self, spender: H160, amount: U256) -> Qrc20AbiResult<ContractCallOutput> {
        let function = eth::ERC20_CONTRACT.function("approve")?;
        let params = function.encode_input(&[Token::Address(spender), Token::Uint(amount)])?;

        let gas_limit = QRC20_GAS_LIMIT_DEFAULT;
        let gas_price = QRC20_GAS_PRICE_DEFAULT;
        let script_pubkey =
            generate_contract_call_script_pubkey(&params, gas_limit, gas_price, self.contract_address.as_bytes())?
                .to_bytes();

        Ok(ContractCallOutput {
            value: OUTPUT_QTUM_AMOUNT,
            script_pubkey,
            gas_limit,
            gas_price,
        })
    }

    /// Generate a UTXO output with a script_pubkey that calls EtomicSwap `erc20Payment` function.
    fn erc20_payment_output(
        &self,
        id: Vec<u8>,
        value: U256,
        time_lock: u32,
        secret_hash: &[u8],
        receiver_addr: H160,
        swap_contract_address: &H160,
    ) -> Qrc20AbiResult<ContractCallOutput> {
        let params = self.erc20_payment_call_bytes(id, value, time_lock, secret_hash, receiver_addr)?;

        let gas_limit = QRC20_PAYMENT_GAS_LIMIT;
        let gas_price = QRC20_GAS_PRICE_DEFAULT;
        let script_pubkey = generate_contract_call_script_pubkey(
            &params, // params of the function
            gas_limit,
            gas_price,
            swap_contract_address.as_bytes(), // address of the contract which function will be called
        )?
        .to_bytes();

        Ok(ContractCallOutput {
            value: OUTPUT_QTUM_AMOUNT,
            script_pubkey,
            gas_limit,
            gas_price,
        })
    }

    fn erc20_payment_call_bytes(
        &self,
        id: Vec<u8>,
        value: U256,
        time_lock: u32,
        secret_hash: &[u8],
        receiver_addr: H160,
    ) -> Qrc20AbiResult<Vec<u8>> {
        let function = eth::SWAP_CONTRACT.function("erc20Payment")?;
        Ok(function.encode_input(&[
            Token::FixedBytes(id),
            Token::Uint(value),
            Token::Address(self.contract_address),
            Token::Address(receiver_addr),
            Token::FixedBytes(secret_hash.to_vec()),
            Token::Uint(U256::from(time_lock)),
        ])?)
    }

    /// Generate a UTXO output with a script_pubkey that calls EtomicSwap `receiverSpend` function.
    pub fn receiver_spend_output(
        &self,
        swap_contract_address: &H160,
        id: Vec<u8>,
        value: U256,
        secret: Vec<u8>,
        sender_addr: H160,
    ) -> Qrc20AbiResult<ContractCallOutput> {
        let function = eth::SWAP_CONTRACT.function("receiverSpend")?;
        let params = function.encode_input(&[
            Token::FixedBytes(id),
            Token::Uint(value),
            Token::FixedBytes(secret),
            Token::Address(self.contract_address),
            Token::Address(sender_addr),
        ])?;

        let gas_limit = QRC20_GAS_LIMIT_DEFAULT;
        let gas_price = QRC20_GAS_PRICE_DEFAULT;
        let script_pubkey = generate_contract_call_script_pubkey(
            &params, // params of the function
            gas_limit,
            gas_price,
            swap_contract_address.as_bytes(), // address of the contract which function will be called
        )?
        .to_bytes();

        Ok(ContractCallOutput {
            value: OUTPUT_QTUM_AMOUNT,
            script_pubkey,
            gas_limit,
            gas_price,
        })
    }

    pub fn sender_refund_output(
        &self,
        swap_contract_address: &H160,
        id: Vec<u8>,
        value: U256,
        secret_hash: Vec<u8>,
        receiver: H160,
    ) -> Qrc20AbiResult<ContractCallOutput> {
        let function = eth::SWAP_CONTRACT.function("senderRefund")?;

        let params = function.encode_input(&[
            Token::FixedBytes(id),
            Token::Uint(value),
            Token::FixedBytes(secret_hash),
            Token::Address(self.contract_address),
            Token::Address(receiver),
        ])?;

        let gas_limit = QRC20_GAS_LIMIT_DEFAULT;
        let gas_price = QRC20_GAS_PRICE_DEFAULT;
        let script_pubkey = generate_contract_call_script_pubkey(
            &params, // params of the function
            gas_limit,
            gas_price,
            swap_contract_address.as_bytes(), // address of the contract which function will be called
        )?
        .to_bytes();

        Ok(ContractCallOutput {
            value: OUTPUT_QTUM_AMOUNT,
            script_pubkey,
            gas_limit,
            gas_price,
        })
    }

    /// Get `erc20Payment` contract call details.
    /// Note returns an error if the contract call was excepted.
    async fn erc20_payment_details_from_tx(&self, qtum_tx: &UtxoTx) -> Result<Erc20PaymentDetails, String> {
        let tx_hash: H256Json = qtum_tx.hash().reversed().into();
        let receipts = try_s!(self.utxo.rpc_client.get_transaction_receipts(&tx_hash).compat().await);

        for receipt in receipts {
            let output = try_s!(qtum_tx
                .outputs
                .get(receipt.output_index as usize)
                .ok_or(ERRL!("TxReceipt::output_index out of bounds")));
            let script_pubkey: Script = output.script_pubkey.clone().into();
            if !is_contract_call(&script_pubkey) {
                continue;
            }

            let contract_call_bytes = try_s!(extract_contract_call_from_script(&script_pubkey));

            let call_type = try_s!(MutContractCallType::from_script_pubkey(&contract_call_bytes));
            match call_type {
                Some(MutContractCallType::Erc20Payment) => (),
                _ => continue, // skip non-erc20Payment contract calls
            }

            try_s!(check_if_contract_call_completed(&receipt));

            let function = try_s!(eth::SWAP_CONTRACT.function("erc20Payment"));
            let decoded = try_s!(decode_contract_call(function, &contract_call_bytes));

            let mut decoded = decoded.into_iter();

            let swap_id = match decoded.next() {
                Some(Token::FixedBytes(id)) => id,
                Some(token) => return ERR!("Payment tx 'swap_id' arg is invalid, found {:?}", token),
                None => return ERR!("Couldn't find 'swap_id' in erc20Payment call"),
            };

            let value = match decoded.next() {
                Some(Token::Uint(value)) => value,
                Some(token) => return ERR!("Payment tx 'value' arg is invalid, found {:?}", token),
                None => return ERR!("Couldn't find 'value' in erc20Payment call"),
            };

            let token_address = match decoded.next() {
                Some(Token::Address(addr)) => addr,
                Some(token) => return ERR!("Payment tx 'token_address' arg is invalid, found {:?}", token),
                None => return ERR!("Couldn't find 'token_address' in erc20Payment call"),
            };

            let receiver = match decoded.next() {
                Some(Token::Address(addr)) => addr,
                Some(token) => return ERR!("Payment tx 'receiver' arg is invalid, found {:?}", token),
                None => return ERR!("Couldn't find 'receiver' in erc20Payment call"),
            };

            let secret_hash = match decoded.next() {
                Some(Token::FixedBytes(hash)) => hash,
                Some(token) => return ERR!("Payment tx 'secret_hash' arg is invalid, found {:?}", token),
                None => return ERR!("Couldn't find 'secret_hash' in erc20Payment call"),
            };

            let timelock = match decoded.next() {
                Some(Token::Uint(t)) => t,
                Some(token) => return ERR!("Payment tx 'timelock' arg is invalid, found {:?}", token),
                None => return ERR!("Couldn't find 'timelock' in erc20Payment call"),
            };

            // check if there is no arguments more
            if let Some(token) = decoded.next() {
                return ERR!("Unexpected additional arg {:?}", token);
            }

            let mut events = try_s!(transfer_events_from_receipt(&receipt)).into_iter();
            let event = match events.next() {
                Some(e) => e,
                None => return ERR!("Couldn't find 'Transfer' event from logs"),
            };
            // check if the erc20Payment emitted only one Transfer event
            if events.next().is_some() {
                return ERR!("'erc20Payment' should emit only one 'Transfer' event");
            }

            if event.contract_address != self.contract_address {
                return ERR!(
                    "Unexpected token address {:#02x} in 'Transfer' event, expected {:#02x}",
                    event.contract_address,
                    self.contract_address
                );
            }
            if event.amount != value {
                return ERR!(
                    "Unexpected amount {} in 'Transfer' event, expected {}",
                    event.amount,
                    value
                );
            }

            let contract_address_from_script = try_s!(extract_contract_addr_from_script(&script_pubkey));
            // `erc20Payment` function should emit a `Transfer` event where the receiver is the swap contract
            if event.receiver != contract_address_from_script {
                return ERR!(
                    "Contract address {:#02x} from script pubkey and receiver {:#02x} in 'Transfer' event are different",
                    contract_address_from_script,
                    event.receiver
                );
            }

            return Ok(Erc20PaymentDetails {
                output_index: receipt.output_index,
                swap_id,
                value,
                token_address,
                swap_contract_address: contract_address_from_script,
                sender: event.sender,
                receiver,
                secret_hash,
                timelock,
                contract_call_bytes,
            });
        }
        ERR!("Couldn't find erc20Payment contract call in {:?} tx", tx_hash)
    }

    /// Gets transactions emitted `ReceiverSpent` events from etomic swap smart contract since `from_block`
    async fn receiver_spend_transactions(&self, receiver: H160, from_block: u64) -> Result<Vec<UtxoTx>, String> {
        self.transactions_emitted_swap_event(QRC20_RECEIVER_SPENT_TOPIC, receiver, from_block)
            .await
    }

    /// Gets transactions emitted `SenderRefunded` events from etomic swap smart contract since `from_block`
    async fn sender_refund_transactions(&self, sender: H160, from_block: u64) -> Result<Vec<UtxoTx>, String> {
        self.transactions_emitted_swap_event(QRC20_SENDER_REFUNDED_TOPIC, sender, from_block)
            .await
    }

    /// Gets transactions emitted `PaymentSent` events from etomic swap smart contract since `from_block`
    async fn erc20_payment_transactions(&self, sender: H160, from_block: u64) -> Result<Vec<UtxoTx>, String> {
        self.transactions_emitted_swap_event(QRC20_PAYMENT_SENT_TOPIC, sender, from_block)
            .await
    }

    /// Gets transactions emitted the specified events from etomic swap smart contract since `from_block`.
    /// `event_topic` is an event first and once topic in logs.
    /// `caller_address` is who called etomic swap smart contract functions that emitted the specified event.
    async fn transactions_emitted_swap_event(
        &self,
        event_topic: &str,
        caller_address: H160,
        from_block: u64,
    ) -> Result<Vec<UtxoTx>, String> {
        let receipts = try_s!(
            TransferHistoryBuilder::new(self.clone())
                .from_block(from_block)
                .address(caller_address)
                .build()
                .await
        );

        let mut txs = Vec::with_capacity(receipts.len());
        for receipt in receipts {
            let swap_event_emitted = receipt.log.iter().any(|log| is_swap_event_log(event_topic, log));
            if !swap_event_emitted {
                continue;
            }

            let verbose_tx = try_s!(
                self.utxo
                    .rpc_client
                    .get_verbose_transaction(&receipt.transaction_hash)
                    .compat()
                    .await
            );
            let tx = try_s!(deserialize(verbose_tx.hex.as_slice()).map_err(|e| ERRL!("{:?}", e)));
            txs.push(tx);
        }
        Ok(txs)
    }
}

/// Get `Transfer` events details from [`TxReceipt::logs`].
fn transfer_events_from_receipt(receipt: &TxReceipt) -> Result<Vec<TransferEventDetails>, String> {
    receipt
        .log
        .iter()
        .filter_map(|log_entry| {
            // Transfer event has at least 3 topics
            if log_entry.topics.len() < 3 {
                return None;
            }

            // the first topic is a type of event
            // https://github.com/qtumproject/qtum-electrum/blob/v4.0.2/electrum/wallet.py#L2101
            if log_entry.topics.first().unwrap() != QRC20_TRANSFER_TOPIC {
                return None;
            }

            Some(transfer_event_from_log(log_entry))
        })
        .collect()
}

/// Get `transfer` contract call details from script pubkey.
/// Result - (receiver, amount).
fn transfer_call_details_from_script_pubkey(script_pubkey: &Script) -> Result<(H160, U256), String> {
    if !is_contract_call(script_pubkey) {
        return ERR!("Expected 'transfer' contract call");
    }

    let contract_call_bytes = try_s!(extract_contract_call_from_script(script_pubkey));
    let call_type = try_s!(MutContractCallType::from_script_pubkey(&contract_call_bytes));
    match call_type {
        Some(MutContractCallType::Transfer) => (),
        _ => return ERR!("Expected 'transfer' contract call"),
    }

    let function = try_s!(eth::ERC20_CONTRACT.function("transfer"));
    let decoded = try_s!(decode_contract_call(function, &contract_call_bytes));
    let mut decoded = decoded.into_iter();

    let receiver = match decoded.next() {
        Some(Token::Address(addr)) => addr,
        Some(token) => return ERR!("Transfer 'receiver' arg is invalid, found {:?}", token),
        None => return ERR!("Couldn't find 'receiver' in 'transfer' call"),
    };

    let value = match decoded.next() {
        Some(Token::Uint(value)) => value,
        Some(token) => return ERR!("Transfer 'value' arg is invalid, found {:?}", token),
        None => return ERR!("Couldn't find 'value' in 'transfer' call"),
    };

    Ok((receiver, value))
}

/// Get `receiverSpend` contract call details from script pubkey.
pub fn receiver_spend_call_details_from_script_pubkey(script_pubkey: &Script) -> Result<ReceiverSpendDetails, String> {
    if !is_contract_call(script_pubkey) {
        return ERR!("Expected 'receiverSpend' contract call");
    }

    let contract_call_bytes = try_s!(extract_contract_call_from_script(script_pubkey));
    let call_type = try_s!(MutContractCallType::from_script_pubkey(&contract_call_bytes));
    match call_type {
        Some(MutContractCallType::ReceiverSpend) => (),
        _ => return ERR!("Expected 'receiverSpend' contract call"),
    }

    let function = try_s!(eth::SWAP_CONTRACT.function("receiverSpend"));
    let decoded = try_s!(decode_contract_call(function, &contract_call_bytes));
    let mut decoded = decoded.into_iter();

    let swap_id = match decoded.next() {
        Some(Token::FixedBytes(id)) => id,
        Some(token) => return ERR!("Payment tx 'swap_id' arg is invalid, found {:?}", token),
        None => return ERR!("Couldn't find 'swap_id' in erc20Payment call"),
    };

    let value = match decoded.next() {
        Some(Token::Uint(value)) => value,
        Some(token) => return ERR!("Payment tx 'value' arg is invalid, found {:?}", token),
        None => return ERR!("Couldn't find 'value' in erc20Payment call"),
    };

    let secret = match decoded.next() {
        Some(Token::FixedBytes(hash)) => try_s!(hash.as_slice().try_into()),
        Some(token) => return ERR!("Payment tx 'secret_hash' arg is invalid, found {:?}", token),
        None => return ERR!("Couldn't find 'secret_hash' in erc20Payment call"),
    };

    let token_address = match decoded.next() {
        Some(Token::Address(addr)) => addr,
        Some(token) => return ERR!("Payment tx 'token_address' arg is invalid, found {:?}", token),
        None => return ERR!("Couldn't find 'token_address' in erc20Payment call"),
    };

    let sender = match decoded.next() {
        Some(Token::Address(addr)) => addr,
        Some(token) => return ERR!("Payment tx 'receiver' arg is invalid, found {:?}", token),
        None => return ERR!("Couldn't find 'receiver' in erc20Payment call"),
    };

    Ok(ReceiverSpendDetails {
        swap_id,
        value,
        secret,
        token_address,
        sender,
    })
}

fn find_receiver_spend_with_swap_id_and_secret_hash(
    tx: &UtxoTx,
    expected_swap_id: &[u8],
    expected_secret_hash: &[u8],
) -> Option<usize> {
    let expected_secret_hash = if expected_secret_hash.len() == 32 {
        ripemd160(expected_secret_hash)
    } else {
        chain::hash::H160::from_slice(expected_secret_hash).expect("this shouldn't fail")
    };

    for (output_idx, output) in tx.outputs.iter().enumerate() {
        let script_pubkey: Script = output.script_pubkey.clone().into();
        let ReceiverSpendDetails { swap_id, secret, .. } =
            match receiver_spend_call_details_from_script_pubkey(&script_pubkey) {
                Ok(details) => details,
                Err(_) => {
                    // try to obtain the details from the next output
                    continue;
                },
            };

        if swap_id != expected_swap_id {
            continue;
        }

        let secret_hash = dhash160(&secret);
        if secret_hash != expected_secret_hash {
            warn!(
                "invalid 'dhash160(secret)' {:?}, expected {:?}",
                secret_hash, expected_secret_hash
            );
            continue;
        }

        return Some(output_idx);
    }

    None
}

fn find_swap_contract_call_with_swap_id(
    expected_call_type: MutContractCallType,
    tx: &UtxoTx,
    expected_swap_id: &[u8],
) -> Option<usize> {
    let tx_hash: H256Json = tx.hash().reversed().into();

    for (output_idx, output) in tx.outputs.iter().enumerate() {
        let script_pubkey: Script = output.script_pubkey.clone().into();
        if !is_contract_call(&script_pubkey) {
            continue;
        }

        let contract_call_bytes = match extract_contract_call_from_script(&script_pubkey) {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("{}", e);
                continue;
            },
        };

        let call_type = match MutContractCallType::from_script_pubkey(&contract_call_bytes) {
            Ok(Some(t)) => t,
            Ok(None) => continue, // unknown contract call type
            Err(e) => {
                error!("{}", e);
                continue;
            },
        };
        if call_type != expected_call_type {
            // skip the output
            continue;
        }

        let function = call_type.as_function();
        let decoded = match decode_contract_call(function, &contract_call_bytes) {
            Ok(d) => d,
            Err(e) => {
                error!("{}", e);
                continue;
            },
        };

        // swap_id is the first in `erc20Payment` call
        let swap_id = match decoded.into_iter().next() {
            Some(Token::FixedBytes(id)) => id,
            Some(token) => {
                warn!("tx {:?} 'swap_id' arg is invalid, found {:?}", tx_hash, token);
                continue;
            },
            None => {
                warn!("Warning: couldn't find 'swap_id' in {:?}", tx_hash);
                continue;
            },
        };

        if swap_id == expected_swap_id {
            return Some(output_idx);
        }
    }

    None
}

fn check_if_contract_call_completed(receipt: &TxReceipt) -> Result<(), String> {
    match receipt.excepted {
        Some(ref ex) if ex != "None" && ex != "none" => {
            let msg = match receipt.excepted_message {
                Some(ref m) if !m.is_empty() => format!(": {m}"),
                _ => String::default(),
            };
            ERR!("Contract call failed with an error: {}{}", ex, msg)
        },
        _ => Ok(()),
    }
}

fn is_swap_event_log(event_topic: &str, log: &LogEntry) -> bool {
    let mut topics = log.topics.iter();
    match topics.next() {
        // every swap event should have only one topic in log
        Some(first_event) => first_event == event_topic && topics.next().is_none(),
        _ => false,
    }
}
