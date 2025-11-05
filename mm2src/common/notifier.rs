//! A simple notification system based on mpsc channels.
//!
//! Since this is based on mpsc, multiple notifiers (senders) are allowed while only a single
//! notifiee (receiver) listens for notifications.
//!
//! NOTE: This implementation memory leaks (in contrast to tokio's, but not used here to avoid tokio dependency on wasm).
//! This is because with each `clone()` of the sender we have a new slot in the channel (this is how `futures-rs` does mpsc).
//! These are removed when the receiver calls `wait()`, which calls `clear()`. But if the receiver never `wait()`s for any reason,
//! and there is a thread that doesn't stop `notify()`ing, the channel will keep growing unbounded.
//!
//! So one must make sure that either `wait()` is called after some time or the receiver is dropped when it's no longer needed.
use futures::{channel::mpsc, StreamExt};

#[derive(Clone, Debug)]
pub struct Notifier(mpsc::Sender<()>);

#[derive(Debug)]
pub struct Notifiee(mpsc::Receiver<()>);

impl Notifier {
    /// Create a new notifier and notifiee pair.
    pub fn new() -> (Notifier, Notifiee) {
        let (sender, receiver) = mpsc::channel(0);
        (Notifier(sender), Notifiee(receiver))
    }

    /// Notify the receiver.
    ///
    /// This will error if the receiver has been dropped (disconnected).
    pub fn notify(&self) -> Result<(), &'static str> {
        if let Err(e) = self.0.clone().try_send(()) {
            if e.is_disconnected() {
                return Err("Notification receiver has been dropped.");
            }
        }
        Ok(())
    }
}

impl Notifiee {
    /// Wait for a notification from any notifier.
    ///
    /// This will error if all notifiers have been dropped (disconnected).
    pub async fn wait(&mut self) -> Result<(), &'static str> {
        let result = self.0.next().await.ok_or("All notifiers have been dropped.");
        // Clear pending notifications if there are any, since we have already been notified.
        self.clear();
        result
    }

    /// Clears the pending notifications if there are any.
    fn clear(&mut self) {
        while let Ok(Some(_)) = self.0.try_next() {}
    }
}
