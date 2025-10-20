#[cfg(feature = "log")]
use log::{log, Level};
use std::collections::HashMap;
use std::ops::Deref;
use std::panic::Location;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock, Weak};

const LOCKING_ERROR: &str = "Error locking 'SharedRc::existing_pointers'";
const UPGRADING_ERROR: &str = "Some counter fields are dropped unexpectedly though an inner pointer is still alive";

pub struct SharedRc<T> {
    inner: Arc<T>,
    index: usize,
    next_index: Arc<AtomicUsize>,
    existing_pointers: Arc<RwLock<HashMap<usize, &'static Location<'static>>>>,
}

unsafe impl<T> Send for SharedRc<T> {}
unsafe impl<T> Sync for SharedRc<T> {}

impl<T> Deref for SharedRc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Drop for SharedRc<T> {
    fn drop(&mut self) {
        let mut existing_pointers = self.existing_pointers.write().expect(LOCKING_ERROR);
        existing_pointers.remove(&self.index).unwrap();
    }
}

impl<T> Clone for SharedRc<T> {
    /// # Panic
    ///
    /// May panic if another task failed inside with the locked `SharedRc::existing_pointers` mutex.
    /// This behavior is considered acceptable since the `enable` feature is expected to be used for **debug** purposes only.
    #[track_caller]
    fn clone(&self) -> Self {
        let index = self.next_index.fetch_add(1, Ordering::Relaxed);

        let mut existing_pointers = self.existing_pointers.write().expect(LOCKING_ERROR);
        existing_pointers.insert(index, Location::caller());
        drop(existing_pointers);

        Self {
            inner: self.inner.clone(),
            index,
            next_index: self.next_index.clone(),
            existing_pointers: self.existing_pointers.clone(),
        }
    }
}

impl<T> SharedRc<T> {
    #[track_caller]
    pub fn new(inner: T) -> Self {
        let index = 0;
        let existing_pointers = HashMap::from([(index, Location::caller())]);

        Self {
            inner: Arc::new(inner),
            index,
            next_index: Arc::new(AtomicUsize::new(index + 1)),
            existing_pointers: Arc::new(RwLock::new(existing_pointers)),
        }
    }

    /// # Panic
    ///
    /// May panic if another task failed inside with the locked `SharedRc::existing_pointers` mutex.
    /// This behavior is considered acceptable since the `enable` feature is expected to be used for **debug** purposes only.
    #[cfg(feature = "log")]
    pub fn log_existing_pointers(&self, level: Level, ident: &'static str) {
        let existing_pointers = self.existing_pointers.read().expect(LOCKING_ERROR);
        log!(level, "{} exists at:", ident);
        for (_idx, location) in existing_pointers.iter() {
            log!(level, "\t{}", stringify_location(location));
        }
    }

    /// # Panic
    ///
    /// May panic if another task failed inside with the locked `SharedRc::existing_pointers` mutex.
    /// This behavior is considered acceptable since the `enable` feature is expected to be used for **debug** purposes only.
    pub fn existing_pointers(&self) -> Vec<&'static Location<'static>> {
        let existing_pointers = self.existing_pointers.read().expect(LOCKING_ERROR);
        let locations: Vec<_> = existing_pointers.values().copied().collect();
        locations
    }

    /// Generates a weak pointer, to track the allocated data without prolonging its life.
    pub fn downgrade(&self) -> WeakRc<T> {
        WeakRc {
            inner: Arc::downgrade(&self.inner),
            next_index: Arc::downgrade(&self.next_index),
            existing_pointers: Arc::downgrade(&self.existing_pointers),
        }
    }
}

pub struct WeakRc<T> {
    inner: Weak<T>,
    next_index: Weak<AtomicUsize>,
    existing_pointers: Weak<RwLock<HashMap<usize, &'static Location<'static>>>>,
}

unsafe impl<T> Send for WeakRc<T> {}
unsafe impl<T> Sync for WeakRc<T> {}

impl<T> Clone for WeakRc<T> {
    fn clone(&self) -> Self {
        WeakRc {
            inner: self.inner.clone(),
            next_index: self.next_index.clone(),
            existing_pointers: self.existing_pointers.clone(),
        }
    }
}

impl<T> Default for WeakRc<T> {
    fn default() -> Self {
        WeakRc {
            inner: Weak::default(),
            next_index: Weak::default(),
            existing_pointers: Weak::default(),
        }
    }
}

impl<T> WeakRc<T> {
    /// # Panic
    ///
    /// May panic if another task failed inside with the locked `SharedRc::existing_pointers` mutex.
    /// This behavior is considered acceptable since the `enable` feature is expected to be used for **debug** purposes only.
    #[track_caller]
    pub fn upgrade(&self) -> Option<SharedRc<T>> {
        let inner = self.inner.upgrade()?;
        let next_index = self.next_index.upgrade().expect(UPGRADING_ERROR);
        let index = next_index.fetch_add(1, Ordering::Relaxed);

        let existing_pointers = self.existing_pointers.upgrade().expect(UPGRADING_ERROR);
        let mut existing_pointers_lock = existing_pointers.write().expect(LOCKING_ERROR);
        existing_pointers_lock.insert(index, Location::caller());
        drop(existing_pointers_lock);

        Some(SharedRc {
            inner,
            index,
            next_index,
            existing_pointers,
        })
    }

    pub fn strong_count(&self) -> usize {
        self.inner.strong_count()
    }
}

#[cfg(feature = "log")]
fn stringify_location(location: &'static Location<'static>) -> String {
    format!("{}:{}", location.file(), location.line())
}
