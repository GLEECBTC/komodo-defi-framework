#![allow(dead_code)]
use compatible_time::Instant;
use std::{collections::hash_map::HashMap, num::NonZeroUsize, time::Duration};

const ONE_SECOND: Duration = Duration::from_secs(1);

/// Stores the timestamps of order requests sent to specific peer
pub struct OrderRequestsTracker {
    requested_at: HashMap<String, Vec<Instant>>,
    limit_per_sec: NonZeroUsize,
}

impl Default for OrderRequestsTracker {
    fn default() -> OrderRequestsTracker {
        OrderRequestsTracker::new(NonZeroUsize::new(5).unwrap())
    }
}

impl OrderRequestsTracker {
    /// Create new tracker with `limit` requests per second
    pub fn new(limit_per_sec: NonZeroUsize) -> OrderRequestsTracker {
        OrderRequestsTracker {
            requested_at: HashMap::new(),
            limit_per_sec,
        }
    }

    pub fn peer_requested(&mut self, peer: &str) {
        let now = Instant::now();
        let limit_per_sec = self.limit_per_sec.get();
        let peer_requested_at = self
            .requested_at
            .entry(peer.to_owned())
            .or_insert_with(|| Vec::with_capacity(limit_per_sec));

        if peer_requested_at.len() >= limit_per_sec {
            peer_requested_at.pop();
        }

        peer_requested_at.insert(0, now);
    }

    pub fn limit_reached(&self, peer: &str) -> bool {
        match self.requested_at.get(peer) {
            Some(requested) => {
                if requested.len() < self.limit_per_sec.get() {
                    false
                } else {
                    let min = requested.last().expect("last() can not be None as len > 0");
                    let now = Instant::now();
                    now.duration_since(*min) < ONE_SECOND
                }
            },
            None => false,
        }
    }
}

#[cfg(test)]
mod order_requests_tracker_tests {
    use super::*;
    use std::{thread::sleep, time::Duration};

    // TODO investigate why this fails on MacOS
    #[test]
    #[ignore]
    fn test_limit_reached_true() {
        let limit = NonZeroUsize::new(5).unwrap();
        let mut tracker = OrderRequestsTracker::new(limit);
        let peer = "peer";
        for _ in 0..5 {
            tracker.peer_requested(peer);
            sleep(Duration::from_millis(100));
        }

        assert!(tracker.limit_reached(peer));
    }

    #[test]
    fn test_limit_reached_false() {
        let limit = NonZeroUsize::new(5).unwrap();
        let mut tracker = OrderRequestsTracker::new(limit);
        let peer = "peer";
        for _ in 0..5 {
            tracker.peer_requested(peer);
            sleep(Duration::from_millis(201));
        }

        assert!(!tracker.limit_reached(peer));
    }
}
