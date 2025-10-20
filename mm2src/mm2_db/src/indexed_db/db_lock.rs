use super::{DbIdentifier, DbInstance, InitDbResult};
use futures::lock::{MappedMutexGuard as AsyncMappedMutexGuard, Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
use mm2_core::{mm_ctx::MmArc, DbNamespaceId};
use primitives::hash::H160;
use std::sync::{Arc, Weak};

/// The mapped mutex guard.
/// This implements `Deref<Db>`.
pub type DbLocked<'a, Db> = AsyncMappedMutexGuard<'a, Option<Db>, Db>;
pub type SharedDb<Db> = Arc<ConstructibleDb<Db>>;
pub type WeakDb<Db> = Weak<ConstructibleDb<Db>>;

pub struct ConstructibleDb<Db> {
    /// It's better to use something like [`Constructible`], but it doesn't provide a method to get the inner value by the mutable reference.
    mutex: AsyncMutex<Option<Db>>,
    db_namespace: DbNamespaceId,
    wallet_rmd160: Option<H160>,
}

impl<Db: DbInstance> ConstructibleDb<Db> {
    pub fn into_shared(self) -> SharedDb<Db> {
        Arc::new(self)
    }

    /// Creates a new uninitialized `Db` instance from other Iguana and/or HD accounts.
    /// This can be initialized later using [`ConstructibleDb::get_or_initialize`].
    pub fn new(ctx: &MmArc) -> Self {
        ConstructibleDb {
            mutex: AsyncMutex::new(None),
            db_namespace: ctx.db_namespace,
            wallet_rmd160: Some(*ctx.rmd160()),
        }
    }

    /// Creates a new uninitialized `Db` instance shared between Iguana and all HD accounts
    /// derived from the same passphrase.
    /// This can be initialized later using [`ConstructibleDb::get_or_initialize`].
    pub fn new_shared_db(ctx: &MmArc) -> Self {
        ConstructibleDb {
            mutex: AsyncMutex::new(None),
            db_namespace: ctx.db_namespace,
            wallet_rmd160: Some(*ctx.shared_db_id()),
        }
    }

    /// Creates a new uninitialized `Db` instance shared between all wallets/seed.
    /// This can be initialized later using [`ConstructibleDb::get_or_initialize`].
    pub fn new_global_db(ctx: &MmArc) -> Self {
        ConstructibleDb {
            mutex: AsyncMutex::new(None),
            db_namespace: ctx.db_namespace,
            wallet_rmd160: None,
        }
    }

    /// Locks the given mutex and checks if the inner database is initialized already or not,
    /// initializes it if it's required, and returns the locked instance.
    pub async fn get_or_initialize(&self) -> InitDbResult<DbLocked<'_, Db>> {
        let mut locked_db = self.mutex.lock().await;
        // Db is initialized already
        if locked_db.is_some() {
            return Ok(unwrap_db_instance(locked_db));
        }

        let db_id = DbIdentifier::new::<Db>(self.db_namespace, self.wallet_rmd160);

        let db = Db::init(db_id).await?;
        *locked_db = Some(db);
        Ok(unwrap_db_instance(locked_db))
    }
}

/// # Panics
///
/// This function will `panic!()` if the inner value of the `guard` is `None`.
fn unwrap_db_instance<Db>(guard: AsyncMutexGuard<'_, Option<Db>>) -> DbLocked<'_, Db> {
    AsyncMutexGuard::map(guard, |wrapped_db| {
        wrapped_db
            .as_mut()
            .expect("The locked 'Option<Db>' must contain a value")
    })
}
