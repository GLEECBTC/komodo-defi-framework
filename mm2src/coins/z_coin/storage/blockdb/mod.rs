#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod blockdb_sql_storage;

#[cfg(not(target_arch = "wasm32"))]
use db_common::sqlite::rusqlite::Connection;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};

#[cfg(target_arch = "wasm32")]
pub(crate) mod blockdb_idb_storage;
#[cfg(target_arch = "wasm32")]
use blockdb_idb_storage::BlockDbInner;
#[cfg(target_arch = "wasm32")]
use mm2_db::indexed_db::SharedDb;

/// A wrapper for the db connection to the block cache database in native and browser.
#[derive(Clone)]
pub struct BlockDbImpl {
    #[cfg(not(target_arch = "wasm32"))]
    pub db: Arc<Mutex<Connection>>,
    #[cfg(target_arch = "wasm32")]
    pub db: SharedDb<BlockDbInner>,
    ticker: String,
}

#[cfg(any(test, target_arch = "wasm32"))]
mod block_db_storage_tests {
    use crate::z_coin::storage::BlockDbImpl;
    use common::log::info;

    use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;

    const TICKER: &str = "ARRR";
    const HEADERS: &[(u32, &str)] = &[(1900000,
                                          "10E0FB731A2044797F3BB78323A7717007F1E289A3689E0B5B3433385DBD8E6F6A17000000002220735484676853C744A8CA0FEA105081C54A8C50A151E42E31EC7E20040000000028EBACFD9306"), (1900001,
                                                                                                                                                                                                            "10E1FB731A20A261B624D0E42238255A69F96E45EEA341B5E4125A7DD710118D150B00000000222044797F3BB78323A7717007F1E289A3689E0B5B3433385DBD8E6F6A170000000028FEACFD9306"), (1900002,"10E2FB731A208747587DE8DDED766591FA6C9859D77BFC9C293B054F3D38A9BC5E08000000002220A261B624D0E42238255A69F96E45EEA341B5E4125A7DD710118D150B0000000028F7ADFD93063AC002080212201D7165BCACD3245EED7324367EB34199EA2ED502726933484FEFA6A220AA330F22220A208DD3C9362FBCF766BEF2DFA3A3B186BBB43CA456DB9690EFD06978FC822056D22A7A0A20245E73ED6EB4B73805D3929F841CCD7E01523E2B8A0F29D721CD82547A470C711220D6BAF6AF4783FF265451B8A7A5E4271EA72F034890DA234427082F84F08256DD1A34EAABEE115A1FCDED194189F586C6DC2099E8C5F47BD68B210146EDFFCB39649EB55504910EC590E6E9908B6114ED3DDFD5861FDC2A7A0A2079E70D202FEE537011284A30F1531BCF627613CBBAAFABBB24CE56600FE94B6C122041E9FBA0E6197A58532F61BD7617CACEC8C2F10C77AA8B99B2E535EE1D3C36171A341B6A04C5EC9A2AE8CDF0433C9AAD36C647139C9542759E2758FD4A10ED0C78F8087BE5AEE92EA8834E6CE116C8A5737B7607BD523AC002080312202790606A461DA171221480A3FC414CCF9C273FE6F0C2E3CFA6C85D6CDE8EFE5C22220A201767E6E3B390FAB4C79E46131C54ED91A987EEA2286DB80F240D431AC07A750C2A7A0A20E86C11A660EB72F1449BA0CEB57FFB313A4047880C33ADED93945ED9C477581B12201752816751ABAB19398A4A5CFE429724D820588BCFEDC7D88B399D9B24FB4C111A34DB38AE57231FBE768063E08D8EC70E3486FF89A74E0840B6F5D8412F1C7E2C5D884AA08E2F7EDA42836B80B4433C83CDDC8B51DE2A7A0A20E2FEF897A286A8D5AD9E0485F287CE1A73970EADA899DBE3FC77043846E06B1E1220F0A046829B17CC8B5B750281CD20A1E28F983E599AA2A1C8F3BD97BE49C55CEB1A3488DCDA1444CBACE213100507FC83627D83624EF2AD47C25160F5E604595158C98EBC3549C0A07359FB42D8437A70AB472FB64AA13AC002080412201EDD399E68128B97F6F98E31C1965361528AC07665114D09F9D119C089791E9222220A20B9471453950609CF8C2EDF721FE7D0D2D211BBD158283E8D6B80EAAB312968EF2A7A0A201FF6F7D74ABBAC9D4E5A95F63861C19FE3D18083ABE2EACE7B8A70E7E5FCB51812206753F2992061EF3FC0C37FC0D1352A386514B2CC1AEB39AC835A8D9BFBD022D91A34BA41719ECF19520BD7D6EFB08AAF5018282026781D0FE5697811B34E0DEFE4D4691585D4994056E109DC19FFE63CAB29CA4F26682A7A0A200E570E832326625C9D8536DBAC389529A090FC54C3F378E25431405751BBFF391220D27A030843C93522B2D232644E7AC7CF235494B126FDAEA9F5980FA1AECE746E1A34EF8BD98D7DD39659714E7851E47F57A52741F564F0275CE8A82F2665C70EA5887B0CE8501CF509A8265ECB155A00A0629B463C253AC00208051220E1F375AD9EC6A774E444ECC5EB6F07237B1DE9EAA1A9FD7AEF392D6F40BA705822220A20D8298A06C9657E042DC69473B23A74C94E51AF684DA6281CE7F797791F486AD42A7A0A209216A5DBC616291688CDFB075A5E639FA8000ADD006438C4BCE98D000AE0DF3512202C20533A17279C46EC995DBF819673039E5810DCD2DA024DAEF64053CD7B562D1A346928F93BB25B03519AC83B297F77E2F54F62B1E722E6F8D886ADF709455C2C0B930CE429EA24ECD15354085F7FA3F2A4077DE76D2A7A0A203AE3F07AB8AB4C76B246A0D7CA9321F84081144E9B7E3AE0CEC0139B392E443812200791064E9E188BF1D1373BEEFAE7458F12F976B15896CD69970019B4560A5F721A3428ADC7816F15528F65372E585E07D1CD6C0DFB3F3BA7BD263BB4E5A3ADAAFD84CD55FFBDD23787163F52711A22935EB52A30EB37")
    ];

    pub(crate) async fn test_insert_block_and_get_latest_block_impl() {
        let ctx = mm_ctx_with_custom_db();
        let db = BlockDbImpl::new(&ctx, TICKER.to_string()).await.unwrap();
        // insert block
        for header in HEADERS.iter() {
            db.insert_block(header.0, hex::decode(header.1).unwrap()).await.unwrap();
        }

        // get last block header
        let last_height = db.get_latest_block().await.unwrap();
        assert_eq!(1900002, last_height)
    }

    pub(crate) async fn test_rewind_to_height_impl() {
        let ctx = mm_ctx_with_custom_db();
        let db = BlockDbImpl::new(&ctx, TICKER.to_string()).await.unwrap();
        // insert block
        for header in HEADERS.iter() {
            db.insert_block(header.0, hex::decode(header.1).unwrap()).await.unwrap();
        }

        // rewind height to 1900000
        let rewind_result = db.rewind_to_height(1900000.into()).await;
        assert!(rewind_result.is_ok());

        // get last height - we expect it to be 1900000
        let last_height = db.get_latest_block().await.unwrap();
        assert_eq!(1900000, last_height);
        info!("Rewinding to height ended!");

        // get last height - we expect it to be 1900000
        let last_height = db.get_latest_block().await.unwrap();
        assert_eq!(1900000, last_height)
    }

    #[allow(unused)]
    pub(crate) async fn test_process_blocks_with_mode_impl() {
        let ctx = mm_ctx_with_custom_db();
        let db = BlockDbImpl::new(&ctx, TICKER.to_string()).await.unwrap();
        // insert block
        for header in HEADERS.iter() {
            let inserted_id = db.insert_block(header.0, hex::decode(header.1).unwrap()).await.unwrap();
            assert_eq!(1, inserted_id);
        }

        // get last height - we expect it to be 1900002
        let block_height = db.get_latest_block().await.unwrap();
        assert_eq!(1900002, block_height);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_tests {
    use crate::z_coin::storage::blockdb::block_db_storage_tests::{
        test_insert_block_and_get_latest_block_impl, test_rewind_to_height_impl,
    };
    use common::block_on;

    #[test]
    fn test_insert_block_and_get_latest_block() {
        block_on(test_insert_block_and_get_latest_block_impl())
    }

    #[test]
    fn test_rewind_to_height() {
        block_on(test_rewind_to_height_impl())
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_tests {
    use crate::z_coin::storage::blockdb::block_db_storage_tests::{
        test_insert_block_and_get_latest_block_impl, test_rewind_to_height_impl,
    };
    // use crate::z_coin::z_rpc::{LightRpcClient, ZRpcOps};
    // use common::log::info;
    // use common::log::wasm_log::register_wasm_log;
    use common::log::warn;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    async fn test_insert_block_and_get_latest_block() {
        test_insert_block_and_get_latest_block_impl().await
    }

    #[wasm_bindgen_test]
    async fn test_rewind_to_height() {
        test_rewind_to_height_impl().await
    }

    #[wasm_bindgen_test]
    async fn test_transport() {
        warn!("Skipping test_transport since it's failing, check https://github.com/KomodoPlatform/komodo-defi-framework/issues/2366");
        // register_wasm_log();
        // let client = LightRpcClient::new(vec!["https://pirate.battlefield.earth:8581".to_string()])
        //     .await
        //     .unwrap();
        // let latest_height = client.get_block_height().await;
        //
        // assert!(latest_height.is_ok());
        // info!("LATEST BLOCK: {latest_height:?}");
    }
}
