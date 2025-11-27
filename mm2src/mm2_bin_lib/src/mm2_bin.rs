#[cfg(not(target_arch = "wasm32"))]
use mm2_main::mm2_main;

#[cfg(not(target_arch = "wasm32"))]
const KDF_VERSION: &str = env!("KDF_VERSION");

#[cfg(not(target_arch = "wasm32"))]
const KDF_DATETIME: &str = env!("KDF_DATETIME");

#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        mm2_main(KDF_VERSION.into(), KDF_DATETIME.into())
    }
}
