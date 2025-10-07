use std::ops::Deref;

use async_trait::async_trait;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::MmResult;
use mm2_err_handle::prelude::*;
use relay_rpc::domain::Topic;

use crate::{error::WalletConnectError, session::Session};

#[cfg(target_arch = "wasm32")]
pub(crate) mod indexed_db;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod sqlite;

#[async_trait]
pub(crate) trait WalletConnectStorageOps {
    type Error: std::fmt::Debug + NotMmError + Send;

    async fn init(&self) -> MmResult<(), Self::Error>;
    #[expect(dead_code)]
    async fn is_initialized(&self) -> MmResult<bool, Self::Error>;
    async fn save_session(&self, session: &Session) -> MmResult<(), Self::Error>;
    #[allow(dead_code)]
    async fn get_session(&self, topic: &Topic) -> MmResult<Option<Session>, Self::Error>;
    async fn get_all_sessions(&self) -> MmResult<Vec<Session>, Self::Error>;
    async fn delete_session(&self, topic: &Topic) -> MmResult<(), Self::Error>;
    async fn update_session(&self, session: &Session) -> MmResult<(), Self::Error>;
}

#[cfg(target_arch = "wasm32")]
type DB = indexed_db::IDBSessionStorage;
#[cfg(not(target_arch = "wasm32"))]
type DB = sqlite::SqliteSessionStorage;

pub(crate) struct SessionStorageDb(DB);

impl Deref for SessionStorageDb {
    type Target = DB;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl SessionStorageDb {
    pub(crate) fn new(ctx: &MmArc) -> MmResult<Self, WalletConnectError> {
        let db = DB::new(ctx).mm_err(|err| WalletConnectError::StorageError(err.to_string()))?;

        Ok(SessionStorageDb(db))
    }
}

#[cfg(test)]
pub(crate) mod session_storage_tests {
    common::cfg_wasm32! {
        use wasm_bindgen_test::*;
        wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
    }
    use common::cross_test;
    use mm2_test_helpers::for_tests::mm_ctx_with_custom_async_db;
    use relay_rpc::{domain::SubscriptionId, rpc::params::Metadata};

    use crate::{
        session::key::SessionKey,
        session::{Session, SessionType},
        WalletConnectCtx,
    };

    use super::WalletConnectStorageOps;

    fn sample_test_session(wc_ctx: &WalletConnectCtx) -> Session {
        let session_key = SessionKey {
            sym_key: [
                115, 159, 247, 31, 199, 84, 88, 59, 158, 252, 98, 225, 51, 125, 201, 239, 142, 34, 9, 201, 128, 114,
                144, 166, 102, 131, 87, 191, 33, 24, 153, 7,
            ],
            public_key: [
                115, 159, 247, 31, 199, 84, 88, 59, 158, 252, 98, 225, 51, 125, 201, 239, 142, 34, 9, 201, 128, 114,
                144, 166, 102, 131, 87, 191, 33, 24, 153, 7,
            ],
        };

        Session::new(
            wc_ctx,
            "bb89e3bae8cb89e5549f4d9bcc5a1ac2aae6dd90ef37eb2f59d80c5773f36343".into(),
            SubscriptionId::generate(),
            session_key,
            "5af44bdf8d6b11f4635c964a15e9e2d50942534824791757b2c26528e8feef39".into(),
            Metadata::default(),
            SessionType::Controller,
        )
    }

    cross_test!(save_and_get_session_test, {
        let mm_ctx = mm_ctx_with_custom_async_db().await;
        let wc_ctx = WalletConnectCtx::try_init(&mm_ctx).unwrap();
        wc_ctx.session_manager.storage().init().await.unwrap();

        let sample_session = sample_test_session(&wc_ctx);

        // try save session
        wc_ctx
            .session_manager
            .storage()
            .save_session(&sample_session)
            .await
            .unwrap();

        // try get session
        let db_session = wc_ctx
            .session_manager
            .storage()
            .get_session(&sample_session.topic)
            .await
            .unwrap();
        assert_eq!(sample_session, db_session.unwrap());
    });

    cross_test!(delete_session_test, {
        let mm_ctx = mm_ctx_with_custom_async_db().await;
        let wc_ctx = WalletConnectCtx::try_init(&mm_ctx).unwrap();
        wc_ctx.session_manager.storage().init().await.unwrap();

        let sample_session = sample_test_session(&wc_ctx);

        // try save session
        wc_ctx
            .session_manager
            .storage()
            .save_session(&sample_session)
            .await
            .unwrap();

        // try get session
        let db_session = wc_ctx
            .session_manager
            .storage()
            .get_session(&sample_session.topic)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sample_session, db_session);

        // try delete session
        wc_ctx
            .session_manager
            .storage()
            .delete_session(&db_session.topic)
            .await
            .unwrap();

        // try get_session deleted again
        let db_session = wc_ctx.session_manager.storage().get_session(&db_session.topic).await;
        assert!(db_session.is_err());
    });

    cross_test!(update_session_test, {
        let mm_ctx = mm_ctx_with_custom_async_db().await;
        let wc_ctx = WalletConnectCtx::try_init(&mm_ctx).unwrap();
        wc_ctx.session_manager.storage().init().await.unwrap();

        let sample_session = sample_test_session(&wc_ctx);

        // try save session
        wc_ctx
            .session_manager
            .storage()
            .save_session(&sample_session)
            .await
            .unwrap();

        // try get session
        let db_session = wc_ctx
            .session_manager
            .storage()
            .get_session(&sample_session.topic)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sample_session, db_session);

        // modify sample_session
        let mut modified_sample_session = sample_session.clone();
        modified_sample_session.expiry = 100;

        // assert that original session expiry isn't the same as our new expiry.
        assert_ne!(sample_session.expiry, modified_sample_session.expiry);

        // try update session
        wc_ctx
            .session_manager
            .storage()
            .update_session(&modified_sample_session)
            .await
            .unwrap();

        // try get_session again with new updated expiry
        let db_session = wc_ctx
            .session_manager
            .storage()
            .get_session(&sample_session.topic)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(sample_session.expiry, db_session.expiry);

        assert_eq!(modified_sample_session, db_session);
        assert_eq!(100, db_session.expiry);
    });
}
