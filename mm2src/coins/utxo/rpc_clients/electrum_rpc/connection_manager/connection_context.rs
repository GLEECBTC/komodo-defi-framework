use std::collections::HashSet;
use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::super::connection::ElectrumConnection;
use super::super::constants::FIRST_SUSPEND_TIME;

use common::now_ms;
use keys::Address;

#[derive(Debug)]
struct SuspendTimer {
    /// When was the connection last disconnected.
    disconnected_at: AtomicU64,
    /// How long to suspend the server the next time it disconnects (in milliseconds).
    next_suspend_time: AtomicU64,
}

impl SuspendTimer {
    /// Creates a new suspend timer.
    fn new() -> Self {
        SuspendTimer {
            disconnected_at: AtomicU64::new(0),
            next_suspend_time: AtomicU64::new(FIRST_SUSPEND_TIME),
        }
    }

    /// Resets the suspend time and disconnection time.
    fn reset(&self) {
        self.disconnected_at.store(0, Ordering::SeqCst);
        self.next_suspend_time.store(FIRST_SUSPEND_TIME, Ordering::SeqCst);
    }

    /// Doubles the suspend time and sets the disconnection time to `now`.
    fn double(&self) {
        // The max suspend time, 12h.
        const MAX_SUSPEND_TIME: u64 = 12 * 60 * 60;
        self.disconnected_at.store(now_ms(), Ordering::SeqCst);
        let mut next_suspend_time = self.next_suspend_time.load(Ordering::SeqCst);
        next_suspend_time = (next_suspend_time * 2).min(MAX_SUSPEND_TIME);
        self.next_suspend_time.store(next_suspend_time, Ordering::SeqCst);
    }

    /// Returns the time until when the server should be suspended in milliseconds.
    fn get_suspend_until(&self) -> u64 {
        self.disconnected_at.load(Ordering::SeqCst) + self.next_suspend_time.load(Ordering::SeqCst) * 1000
    }
}

/// A struct that encapsulates an Electrum connection and its information.
#[derive(Debug)]
pub struct ConnectionContext {
    /// The electrum connection.
    pub connection: Arc<ElectrumConnection>,
    /// The list of addresses subscribed to the connection.
    subs: Mutex<HashSet<Address>>,
    /// The timer deciding when the connection is ready to be used again.
    suspend_timer: SuspendTimer,
    /// The ID of this connection which also serves as a priority (lower is better).
    pub id: u32,
}

impl ConnectionContext {
    /// Creates a new connection context.
    pub(super) fn new(connection: ElectrumConnection, id: u32) -> Self {
        ConnectionContext {
            connection: Arc::new(connection),
            subs: Mutex::new(HashSet::new()),
            suspend_timer: SuspendTimer::new(),
            id,
        }
    }

    /// Resets the suspend time.
    pub(super) fn connected(&self) {
        self.suspend_timer.reset();
    }

    /// Inform the connection context that the connection has been disconnected.
    ///
    /// Doubles the suspend time and clears the subs list and returns it.
    pub(super) fn disconnected(&self) -> HashSet<Address> {
        self.suspend_timer.double();
        mem::take(&mut self.subs.lock().unwrap())
    }

    /// Returns the time the server should be suspended until (when to take it up) in milliseconds.
    pub(super) fn suspended_till(&self) -> u64 {
        self.suspend_timer.get_suspend_until()
    }

    /// Adds a subscription to the connection context.
    pub(super) fn add_sub(&self, address: Address) {
        self.subs.lock().unwrap().insert(address);
    }
}
