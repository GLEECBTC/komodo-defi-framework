use futures::future::AbortHandle;

/// The AbortHandle that aborts on drop
pub struct AbortOnDropHandle(AbortHandle);

impl From<AbortHandle> for AbortOnDropHandle {
    fn from(handle: AbortHandle) -> Self {
        AbortOnDropHandle(handle)
    }
}

impl Drop for AbortOnDropHandle {
    #[inline(always)]
    fn drop(&mut self) {
        self.0.abort();
    }
}
