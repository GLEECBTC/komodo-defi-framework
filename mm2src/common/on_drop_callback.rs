/// Runs some function when this object is dropped.
///
/// We wrap the callback function in an `Option` so that we can exercise the less strict `FnOnce` bound
/// (`FnOnce` is less strict than `Fn`). This way we can take out the function and execute it when dropping.
/// We also implement this with `Box<dyn ...>` instead of generics so not to force users to use generics if
/// this callback handle is stored in some struct.
pub struct OnDropCallback(Option<Box<dyn FnOnce() + Send>>);

impl OnDropCallback {
    pub fn new(f: impl FnOnce() + Send + 'static) -> Self {
        Self(Some(Box::new(f)))
    }
}

impl Drop for OnDropCallback {
    fn drop(&mut self) {
        if let Some(func) = self.0.take() {
            func()
        }
    }
}
