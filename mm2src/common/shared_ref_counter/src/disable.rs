use std::ops::Deref;
use std::panic::Location;
use std::sync::{Arc, Weak};

pub struct SharedRc<T>(Arc<T>);

unsafe impl<T> Send for SharedRc<T> where Arc<T>: Send {}
unsafe impl<T> Sync for SharedRc<T> {}

impl<T> Deref for SharedRc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> Clone for SharedRc<T> {
    fn clone(&self) -> Self {
        SharedRc(self.0.clone())
    }
}

impl<T> SharedRc<T> {
    pub fn new(inner: T) -> Self {
        SharedRc(Arc::new(inner))
    }

    pub fn existing_pointers(&self) -> Vec<&'static Location<'static>> {
        Vec::new()
    }

    /// Generates a weak pointer, to track the allocated data without prolonging its life.
    pub fn downgrade(&self) -> WeakRc<T> {
        WeakRc(Arc::downgrade(&self.0))
    }
}

pub struct WeakRc<T>(Weak<T>);

unsafe impl<T> Send for WeakRc<T> where Weak<T>: Send {}
unsafe impl<T> Sync for WeakRc<T> {}

impl<T> Clone for WeakRc<T> {
    fn clone(&self) -> Self {
        WeakRc(self.0.clone())
    }
}

impl<T> Default for WeakRc<T> {
    fn default() -> Self {
        WeakRc(Weak::default())
    }
}

impl<T> WeakRc<T> {
    pub fn upgrade(&self) -> Option<SharedRc<T>> {
        self.0.upgrade().map(SharedRc)
    }

    pub fn strong_count(&self) -> usize {
        self.0.strong_count()
    }
}
