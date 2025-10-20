mod indexeddb;
pub(crate) use indexeddb::ZcashParamsWasmImpl;

use blake2b_simd::State;
use common::log::info;
use common::log::wasm_log::register_wasm_log;
use mm2_err_handle::prelude::MmResult;
use mm2_err_handle::prelude::*;
use mm2_net::wasm::http::FetchRequest;
use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;
use wasm_bindgen_test::*;

const DOWNLOAD_URL: &str = "https://komodoplatform.com/downloads";
const SAPLING_SPEND_NAME: &str = "sapling-spend.params";
const SAPLING_OUTPUT_NAME: &str = "sapling-output.params";
const SAPLING_SPEND_HASH: &str = "8270785a1a0d0bc77196f000ee6d221c9c9894f55307bd9357c3f0105d31ca63991ab91324160d8f53e2bbd3c2633a6eb8bdf5205d822e7f3f73edac51b2b70c";
const SAPLING_OUTPUT_HASH: &str = "657e3d38dbb5cb5e7dd2970e8b03d69b4787dd907285b5a7f0790dcc8072f60bf593b32cc2d1c030e00ff5ae64bf84c5c3beb84ddc841d48264b4a171744d028";

#[derive(Debug, derive_more::Display)]
pub(crate) enum ZcashParamsError {
    Transport(String),
    ValidationError(String),
}

/// Download, validate and return z_params from given `DOWNLOAD_URL`
async fn fetch_params(name: &str, expected_hash: &str) -> MmResult<Vec<u8>, ZcashParamsError> {
    let (status, file) = FetchRequest::get(&format!("{DOWNLOAD_URL}/{name}"))
        .cors()
        .request_array()
        .await
        .mm_err(|err| ZcashParamsError::Transport(err.to_string()))?;

    if status != 200 {
        return MmError::err(ZcashParamsError::Transport(format!(
            "Expected status 200, got {status} for {name}"
        )));
    }

    let hash = State::new().update(&file).finalize().to_hex();
    // Verify parameter file hash.
    if &hash != expected_hash {
        return Err(ZcashParamsError::ValidationError(format!(
            "{name} failed validation (expected: {expected_hash}, actual: {hash}, fetched {} bytes)",
            file.len()
        ))
        .into());
    }

    Ok(file)
}

pub(crate) async fn download_parameters() -> MmResult<(Vec<u8>, Vec<u8>), ZcashParamsError> {
    Ok((
        fetch_params(SAPLING_SPEND_NAME, SAPLING_SPEND_HASH).await?,
        fetch_params(SAPLING_OUTPUT_NAME, SAPLING_OUTPUT_HASH).await?,
    ))
}

#[wasm_bindgen_test]
async fn test_download_save_and_get_params() {
    register_wasm_log();
    info!("Testing download, save and get params");
    let ctx = mm_ctx_with_custom_db();
    let db = ZcashParamsWasmImpl::new(&ctx).await.unwrap();
    // save params
    let (sapling_spend, sapling_output) = db.download_and_save_params().await.unwrap();
    // get params
    let (sapling_spend_db, sapling_output_db) = db.get_params().await.unwrap();
    assert_eq!(sapling_spend, sapling_spend_db);
    assert_eq!(sapling_output, sapling_output_db);
    info!("Testing download, save and get params successful");
}

#[wasm_bindgen_test]
async fn test_check_for_no_params() {
    register_wasm_log();
    let ctx = mm_ctx_with_custom_db();
    let db = ZcashParamsWasmImpl::new(&ctx).await.unwrap();
    // check for no params
    let check_params = db.check_params().await.unwrap();
    assert!(!check_params)
}
