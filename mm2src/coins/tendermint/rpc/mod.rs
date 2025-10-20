#[cfg(not(target_arch = "wasm32"))]
mod tendermint_native_rpc;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use tendermint_native_rpc::*;

#[cfg(target_arch = "wasm32")]
mod tendermint_wasm_rpc;
#[cfg(target_arch = "wasm32")]
pub(crate) use tendermint_wasm_rpc::*;

pub(crate) const TX_SUCCESS_CODE: u32 = 0;

#[repr(u8)]
pub enum TendermintResultOrder {
    Ascending = 1,
    Descending,
}

impl From<TendermintResultOrder> for Order {
    fn from(order: TendermintResultOrder) -> Self {
        match order {
            TendermintResultOrder::Ascending => Self::Ascending,
            TendermintResultOrder::Descending => Self::Descending,
        }
    }
}
