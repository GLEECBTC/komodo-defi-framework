use lazy_static::lazy_static;
use libp2p::PeerId;
use parking_lot::Mutex;
use std::collections::HashMap;

/// Minimal explicit ban reasons used by the app-level policy.
/// - Connectivity: temporary connectivity-related bans that may be auto-unbanned to restore mesh.
/// - Misbehavior: validation or protocol issues; must NOT be auto-unbanned.
/// - Unknown: default/unspecified classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BanReason {
    Connectivity,
    Misbehavior,
    Unknown,
}

lazy_static! {
    static ref BAN_REASONS: Mutex<HashMap<PeerId, BanReason>> = Mutex::new(HashMap::new());
}

/// Record/update a reason.
pub fn set_ban_reason(peer: PeerId, reason: BanReason) {
    BAN_REASONS.lock().insert(peer, reason);
}

/// Return the recorded ban reason for a peer (if any).
pub fn ban_reason(peer: &PeerId) -> Option<BanReason> {
    BAN_REASONS.lock().get(peer).copied()
}

/// Remove any recorded ban reason for a peer. (call on unban).
pub fn remove_ban_reason(peer: &PeerId) {
    BAN_REASONS.lock().remove(peer);
}
