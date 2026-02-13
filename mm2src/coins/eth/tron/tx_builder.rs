//! TRON unsigned transaction builder.
//!
//! Constructs `TransactionRaw` protobuf messages for TRX transfers and TRC20
//! token transfers.  Signing is handled separately (see `sign` module).

use super::proto::{
    Any, ContractType, TransactionContract, TransactionRaw, TransferContract, TriggerSmartContract,
    TYPE_URL_TRANSFER_CONTRACT, TYPE_URL_TRIGGER_SMART_CONTRACT,
};
use super::{TaposBlockData, TronAddress};
use crate::eth::ERC20_CONTRACT;
use common::now_ms;
use ethabi::Token;
use ethereum_types::U256;
use prost::Message;

/// Extract TAPOS reference fields from a recent block.
///
/// Returns `(ref_block_bytes, ref_block_hash)` for `TransactionRaw`:
/// - `ref_block_bytes`: last 2 bytes of block number (big-endian)
/// - `ref_block_hash`: bytes 8..16 of blockID
fn tapos_from_block(block_number: u64, block_id: &[u8; 32]) -> (Vec<u8>, Vec<u8>) {
    let n = block_number.to_be_bytes();
    let ref_block_bytes = n[6..8].to_vec();
    let ref_block_hash = block_id[8..16].to_vec();
    (ref_block_bytes, ref_block_hash)
}

/// Convert a `TronAddress` to raw 21-byte protobuf format (`0x41` prefix + 20 bytes).
pub fn tron_addr_bytes(addr: &TronAddress) -> Vec<u8> {
    addr.as_bytes().to_vec()
}

/// Build an unsigned TRX (native) transfer transaction.
pub fn build_trx_transfer(
    from: &TronAddress,
    to: &TronAddress,
    amount_sun: i64,
    block_data: &TaposBlockData,
) -> TransactionRaw {
    let (ref_block_bytes, ref_block_hash) = tapos_from_block(block_data.number, &block_data.block_id);

    let transfer = TransferContract {
        owner_address: tron_addr_bytes(from),
        to_address: tron_addr_bytes(to),
        amount: amount_sun,
    };
    let any = Any {
        type_url: TYPE_URL_TRANSFER_CONTRACT.to_string(),
        value: transfer.encode_to_vec(),
    };
    let contract = TransactionContract {
        r#type: ContractType::TransferContract as i32,
        parameter: Some(any),
        permission_id: 0,
    };

    TransactionRaw {
        ref_block_bytes,
        ref_block_hash,
        expiration: block_data.timestamp + 60_000,
        data: Vec::new(),
        contract: vec![contract],
        timestamp: now_ms() as i64,
        fee_limit: 0,
    }
}

/// Build an unsigned TRC20 `transfer(address,uint256)` transaction.
pub fn build_trc20_transfer(
    from: &TronAddress,
    contract_addr: &TronAddress,
    recipient: &TronAddress,
    amount: U256,
    fee_limit: i64,
    block_data: &TaposBlockData,
) -> Result<TransactionRaw, ethabi::Error> {
    let (ref_block_bytes, ref_block_hash) = tapos_from_block(block_data.number, &block_data.block_id);

    let trigger = TriggerSmartContract {
        owner_address: tron_addr_bytes(from),
        contract_address: tron_addr_bytes(contract_addr),
        call_value: 0,
        data: abi_encode_trc20_transfer(recipient, amount)?,
        call_token_value: 0,
        token_id: 0,
    };
    let any = Any {
        type_url: TYPE_URL_TRIGGER_SMART_CONTRACT.to_string(),
        value: trigger.encode_to_vec(),
    };
    let contract = TransactionContract {
        r#type: ContractType::TriggerSmartContract as i32,
        parameter: Some(any),
        permission_id: 0,
    };

    Ok(TransactionRaw {
        ref_block_bytes,
        ref_block_hash,
        expiration: block_data.timestamp + 60_000,
        data: Vec::new(),
        contract: vec![contract],
        timestamp: now_ms() as i64,
        fee_limit,
    })
}

/// ABI-encode `transfer(address,uint256)` call data using the shared ERC20 ABI.
///
/// Uses the same `ERC20_CONTRACT` as the EVM path. The recipient is converted
/// to a 20-byte EVM address (0x41 prefix stripped) for standard ABI encoding.
fn abi_encode_trc20_transfer(recipient: &TronAddress, amount: U256) -> Result<Vec<u8>, ethabi::Error> {
    let function = ERC20_CONTRACT.function("transfer")?;
    function.encode_input(&[Token::Address(recipient.to_evm_address()), Token::Uint(amount)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    /// Golden vector: verify builder output matches a real broadcast Nile TRX transfer.
    /// Source: https://nile.tronscan.org/#/transaction/ebd91b4138365e7d8d71d9ce3704f3889614e7316c20ab449011fe4dbc67f0a4
    #[test]
    fn build_trx_transfer_golden_vector() {
        // Real Nile tx: 1000 SUN from 4123b0...08b6 to 418840...d808
        // TAPOS source: block 64687673 (blockID: 0000000003db0e39901ce5715271b601...)
        let block_data = TaposBlockData {
            number: 64_687_673,
            block_id: {
                let bytes = hex::decode("0000000003db0e39901ce5715271b601b1c57055f5d8fa6a9fe3505eee560308").unwrap();
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                arr
            },
            timestamp: 1_770_522_369_000,
        };
        let from = TronAddress::from_hex("4123b00d15c601b30613bf5a3b2f72527c79cc08b6").unwrap();
        let to = TronAddress::from_hex("418840e6c55b9ada326d211d818c34a994aeced808").unwrap();

        let mut raw = build_trx_transfer(&from, &to, 1000, &block_data);
        // Override dynamic fields to match the real tx values.
        raw.timestamp = 1_770_522_424_709;
        raw.expiration = 1_770_522_483_000;

        let expected_hex = "0a020e392208901ce5715271b60140b8b2f4dac3335a66080112620a2d747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e5472616e73666572436f6e747261637412310a154123b00d15c601b30613bf5a3b2f72527c79cc08b61215418840e6c55b9ada326d211d818c34a994aeced80818e8077085ebf0dac333";
        assert_eq!(hex::encode(raw.encode_to_vec()), expected_hex);
    }

    /// Golden vector: verify builder output matches a real broadcast Nile TRC20 transfer.
    /// Source: https://nile.tronscan.org/#/transaction/f0cd35cfdafa93c67c3ee652df3d8995f1eed42814f6a225c6d767e280db3444
    #[test]
    fn build_trc20_transfer_golden_vector() {
        // Real Nile tx: TRC20 transfer of 2,380,000 units
        // TAPOS source: block 64837309 (blockID: 0000000003dd56bde31bf1375e25873d...)
        let block_data = TaposBlockData {
            number: 64_837_309,
            block_id: {
                let bytes = hex::decode("0000000003dd56bde31bf1375e25873dd2d6dea05d81e126be272f42e4c27c26").unwrap();
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                arr
            },
            timestamp: 1_770_972_777_000,
        };
        let from = TronAddress::from_hex("413c5568f418ee30c71f61813a23ef1f92fb1c434c").unwrap();
        let contract_addr = TronAddress::from_hex("41eca9bc828a3005b9a3b909f2cc5c2a54794de05f").unwrap();
        let recipient = TronAddress::from_hex("413ed853b5cddf63533c4e6703b27feb34ff9063b3").unwrap();
        let amount = U256::from(2_380_000u64);
        let fee_limit = 2_172_000i64;

        let mut raw = build_trc20_transfer(&from, &contract_addr, &recipient, amount, fee_limit, &block_data).unwrap();
        // Override dynamic fields to match the real tx values.
        raw.timestamp = 1_770_972_831_784;
        raw.expiration = 1_770_972_891_000;

        let expected_hex = "0a0256bd2208e31bf1375e25873d40f88ed7b1c5335aae01081f12a9010a31747970652e676f6f676c65617069732e636f6d2f70726f746f636f6c2e54726967676572536d617274436f6e747261637412740a15413c5568f418ee30c71f61813a23ef1f92fb1c434c121541eca9bc828a3005b9a3b909f2cc5c2a54794de05f2244a9059cbb0000000000000000000000003ed853b5cddf63533c4e6703b27feb34ff9063b300000000000000000000000000000000000000000000000000000000002450e070a8c0d3b1c5339001e0c88401";
        assert_eq!(hex::encode(raw.encode_to_vec()), expected_hex);
    }
}
