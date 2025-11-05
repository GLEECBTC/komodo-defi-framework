#[cfg(target_arch = "wasm32")]
use derive_more::Display;
#[cfg(target_arch = "wasm32")]
use rand::{thread_rng, Rng};

pub mod data_asker;
pub mod event_dispatcher;
pub mod mm_ctx;

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Display, PartialEq, Default)]
pub enum DbNamespaceId {
    #[display(fmt = "MAIN")]
    #[default]
    Main,
    #[display(fmt = "TEST_{_0}")]
    Test(u64),
}

#[cfg(target_arch = "wasm32")]
impl DbNamespaceId {
    pub fn for_test() -> DbNamespaceId {
        let mut rng = thread_rng();
        DbNamespaceId::Test(rng.gen())
    }

    #[inline(always)]
    pub fn for_test_with_id(id: u64) -> DbNamespaceId {
        DbNamespaceId::Test(id)
    }
}
