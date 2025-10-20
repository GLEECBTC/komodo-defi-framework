pub mod address;
mod block;
mod block_template_request;
mod bytes;
mod get_block_response;
mod get_tx_out_response;
mod get_tx_out_set_info_response;
mod hash;
mod script;
mod transaction;
mod uint;

pub use self::block::RawBlock;
pub use self::block_template_request::{BlockTemplateRequest, BlockTemplateRequestMode};
pub use self::bytes::Bytes;
pub use self::get_block_response::{GetBlockResponse, VerboseBlock, VerboseBlockClient};
pub use self::get_tx_out_response::GetTxOutResponse;
pub use self::get_tx_out_set_info_response::GetTxOutSetInfoResponse;
pub use self::hash::{H160, H256, H264};
pub use self::script::ScriptType;
pub use self::transaction::{
    GetRawTransactionResponse, RawTransaction, SignedTransactionInput, SignedTransactionOutput, Transaction,
    TransactionInput, TransactionInputEnum, TransactionInputScript, TransactionOutputScript,
    TransactionOutputWithAddress, TransactionOutputWithScriptData,
};
pub use self::uint::U256;

pub trait ToTxHash {
    fn to_tx_hash(&self) -> String;
}

impl ToTxHash for Bytes {
    fn to_tx_hash(&self) -> String {
        format!("{self:02x}")
    }
}

impl ToTxHash for Vec<u8> {
    fn to_tx_hash(&self) -> String {
        Bytes::new(self.clone()).to_tx_hash()
    }
}
