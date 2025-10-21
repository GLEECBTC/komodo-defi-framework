// historical milestone, first performed RICK/ZOMBIE swap
// dex fee - https://zombie.explorer.lordofthechains.com/tx/40bec29f268c349722a3228743e5c5b461cf16d124cddcfd2fc624fe895a0bdd
// maker payment - https://rick.explorer.dexstats.info/tx/9d36e95e5147450399895f0f248ac2e2de13382401c2986e134cc3d62bda738e
// taker payment - https://zombie.explorer.lordofthechains.com/tx/b248992e064fab579774c0479b04043091cf62f3975cb1664ea7d4f857ebe6f8
// taker payment spend - https://zombie.explorer.lordofthechains.com/tx/af6bb0f99f9a5a070a0c1f53d69e4189b0e9b68f9d66e69f201a6b6d9f93897e
// maker payment spend - https://rick.explorer.dexstats.info/tx/6a2dcc866ad75cebecb780a02320073a88bcf5e57ddccbe2657494e7747d591e

use super::{GenTxError, ZCoin};
use crate::utxo::rpc_clients::{UtxoRpcClientEnum, UtxoRpcError};
use crate::utxo::utxo_common::payment_script;
use crate::utxo::{sat_from_big_decimal, UtxoAddressFormat};
use crate::z_coin::SendOutputsErr;
use crate::z_coin::{ZOutput, DEX_FEE_OVK};
use crate::{DexFee, NumConversError};
use crate::{PrivKeyPolicyNotAllowed, TransactionEnum};
use bitcrypto::dhash160;
use derive_more::Display;
use futures::compat::Future01CompatExt;
use keys::{AddressBuilder, KeyPair, Public};
use mm2_err_handle::prelude::*;
use mm2_number::BigDecimal;
use script::Script;
use script::{Builder as ScriptBuilder, Opcode};
use secp256k1::SecretKey;
use zcash_primitives::consensus;
use zcash_primitives::legacy::Script as ZCashScript;
use zcash_primitives::memo::MemoBytes;
use zcash_primitives::transaction::builder::Builder as ZTxBuilder;
use zcash_primitives::transaction::builder::Error as ZTxBuilderError;
use zcash_primitives::transaction::components::OutPoint as ZCashOutpoint;
use zcash_primitives::transaction::components::{Amount, TxOut};
use zcash_primitives::transaction::Transaction as ZTransaction;

cfg_native!(
    use common::async_blocking;
);

/// Sends HTLC output from the coin's my_z_addr
pub async fn z_send_htlc(
    coin: &ZCoin,
    time_lock: u32,
    my_pub: &Public,
    other_pub: &Public,
    secret_hash: &[u8],
    amount: BigDecimal,
) -> Result<ZTransaction, MmError<SendOutputsErr>> {
    let payment_script = payment_script(time_lock, secret_hash, my_pub, other_pub);
    let script_hash = dhash160(&payment_script);
    let htlc_address = AddressBuilder::new(
        UtxoAddressFormat::Standard,
        coin.utxo_arc.conf.checksum_type,
        coin.utxo_arc.conf.address_prefixes.clone(),
        None,
    )
    .as_sh(script_hash.into())
    .build()
    .map_to_mm(SendOutputsErr::InternalError)?;

    let amount_sat = sat_from_big_decimal(&amount, coin.utxo_arc.decimals).map_mm_err()?;
    let address = htlc_address.to_string();
    if let UtxoRpcClientEnum::Native(native) = coin.utxo_rpc_client() {
        native.import_address(&address, &address, false).compat().await.unwrap();
    }

    let htlc_script = ScriptBuilder::build_p2sh(&script_hash.into()).to_bytes().take();
    let htlc_output = TxOut {
        value: Amount::from_u64(amount_sat).map_err(|_| NumConversError::new("Invalid ZCash amount".into()))?,
        script_pubkey: ZCashScript(htlc_script),
    };

    let opret_script = ScriptBuilder::default()
        .push_opcode(Opcode::OP_RETURN)
        .push_data(&payment_script)
        .into_bytes()
        .take();
    let op_return_out = TxOut {
        value: Amount::zero(),
        script_pubkey: ZCashScript(opret_script),
    };
    let mm_tx = coin.send_outputs(vec![htlc_output, op_return_out], vec![]).await?;

    Ok(mm_tx)
}

/// Sends HTLC output from the coin's my_z_addr
pub async fn z_send_dex_fee(
    coin: &ZCoin,
    dex_fee: DexFee,
    uuid: &[u8],
) -> Result<ZTransaction, MmError<SendOutputsErr>> {
    if matches!(dex_fee, DexFee::NoFee) {
        return MmError::err(SendOutputsErr::InternalError("unexpected DexFee::NoFee".to_string()));
    }
    let dex_fee_amount_sat =
        sat_from_big_decimal(&dex_fee.fee_amount().to_decimal(), coin.utxo_arc.decimals).map_mm_err()?;
    // add dex fee output
    let dex_fee_out = ZOutput {
        to_addr: coin.z_fields.dex_fee_addr.clone(),
        amount: Amount::from_u64(dex_fee_amount_sat)
            .map_err(|_| NumConversError::new("Invalid ZCash amount".into()))?,
        viewing_key: Some(DEX_FEE_OVK),
        memo: Some(MemoBytes::from_bytes(uuid).expect("uuid length < 512")),
    };
    let mut outputs = vec![dex_fee_out];
    if let Some(dex_burn_amount) = dex_fee.burn_amount() {
        let dex_burn_amount_sat =
            sat_from_big_decimal(&dex_burn_amount.to_decimal(), coin.utxo_arc.decimals).map_mm_err()?;
        // add output to the dex burn address:
        let dex_burn_out = ZOutput {
            to_addr: coin.z_fields.dex_burn_addr.clone(),
            amount: Amount::from_u64(dex_burn_amount_sat)
                .map_err(|_| NumConversError::new("Invalid ZCash amount".into()))?,
            viewing_key: Some(DEX_FEE_OVK),
            memo: Some(MemoBytes::from_bytes(uuid).expect("uuid length < 512")),
        };
        outputs.push(dex_burn_out);
    }

    let tx = coin.send_outputs(vec![], outputs).await?;
    Ok(tx)
}

#[derive(Debug, Display)]
#[allow(clippy::large_enum_variant, clippy::upper_case_acronyms, unused)]
pub enum ZP2SHSpendError {
    ZTxBuilderError(ZTxBuilderError),
    GenTxError(GenTxError),
    PrivKeyPolicyNotAllowed(PrivKeyPolicyNotAllowed),
    Rpc(UtxoRpcError),
    #[display(fmt = "{_0:?} {_1}")]
    TxRecoverable(TransactionEnum, String),
    Io(std::io::Error),
}

impl From<ZTxBuilderError> for ZP2SHSpendError {
    fn from(tx_builder: ZTxBuilderError) -> ZP2SHSpendError {
        ZP2SHSpendError::ZTxBuilderError(tx_builder)
    }
}

impl From<PrivKeyPolicyNotAllowed> for ZP2SHSpendError {
    fn from(err: PrivKeyPolicyNotAllowed) -> Self {
        ZP2SHSpendError::PrivKeyPolicyNotAllowed(err)
    }
}

impl From<UtxoRpcError> for ZP2SHSpendError {
    fn from(rpc: UtxoRpcError) -> ZP2SHSpendError {
        ZP2SHSpendError::Rpc(rpc)
    }
}

impl From<std::io::Error> for ZP2SHSpendError {
    fn from(e: std::io::Error) -> Self {
        ZP2SHSpendError::Io(e)
    }
}

impl ZP2SHSpendError {
    #[inline]
    pub fn get_tx(&self) -> Option<TransactionEnum> {
        match self {
            ZP2SHSpendError::TxRecoverable(ref tx, _) => Some(tx.clone()),
            _ => None,
        }
    }
}

/// Spends P2SH output 0 to the coin's my_z_addr
pub async fn z_p2sh_spend(
    coin: &ZCoin,
    p2sh_tx: ZTransaction,
    tx_locktime: u32,
    input_sequence: u32,
    redeem_script: Script,
    script_data: Script,
    htlc_keypair: &KeyPair,
) -> Result<ZTransaction, MmError<ZP2SHSpendError>> {
    let current_block = coin.utxo_arc.rpc_client.get_block_count().compat().await.map_mm_err()? as u32;
    let mut tx_builder = ZTxBuilder::new(coin.consensus_params(), current_block.into());
    tx_builder.set_lock_time(tx_locktime);

    let secp_secret = SecretKey::from_slice(htlc_keypair.private_ref()).expect("Keypair contains a valid secret key");

    let outpoint = ZCashOutpoint::new(p2sh_tx.txid().0, 0);
    let tx_out = TxOut {
        value: p2sh_tx.vout[0].value,
        script_pubkey: ZCashScript(redeem_script.to_vec()),
    };
    tx_builder.add_transparent_input(
        secp_secret,
        outpoint,
        input_sequence,
        ZCashScript(script_data.to_vec()),
        tx_out,
    )?;
    tx_builder.add_sapling_output(
        None,
        coin.z_fields.my_z_addr.clone(),
        // TODO use fee from coin here. Will do on next iteration, 1000 is default value that works fine
        p2sh_tx.vout[0].value - Amount::from_i64(1000).expect("1000 will always succeed"),
        None,
    )?;

    let prover = coin.z_fields.z_tx_prover.clone();
    #[cfg(not(target_arch = "wasm32"))]
    let (zcash_tx, _) = async_blocking(move || tx_builder.build(consensus::BranchId::Sapling, prover.as_ref())).await?;

    #[cfg(target_arch = "wasm32")]
    let (zcash_tx, _) =
        crate::z_coin::TxBuilderSpawner::request_tx_result(tx_builder, consensus::BranchId::Sapling, prover.clone())
            .await
            .mm_err(ZP2SHSpendError::GenTxError)?
            .tx_result
            .mm_err(ZP2SHSpendError::GenTxError)?;

    let mut tx_buffer = Vec::with_capacity(1024);
    zcash_tx.write(&mut tx_buffer)?;

    coin.utxo_rpc_client()
        .send_raw_transaction(tx_buffer.into())
        .compat()
        .await
        .map(|_| zcash_tx.clone())
        .mm_err(|e| ZP2SHSpendError::TxRecoverable(zcash_tx.into(), e.to_string()))
}
