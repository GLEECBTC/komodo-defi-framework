use libp2p::{identity::Keypair, PeerId};
use mm2_core::mm_ctx::MmArc;
use parking_lot::Mutex;
use std::sync::Arc;

use crate::AdexCmdTx;

pub struct P2PContext {
    /// Using Mutex helps to prevent cloning which can actually result to channel being unbounded in case of using 1 tx clone per 1 message.
    pub cmd_tx: Mutex<AdexCmdTx>,
    /// Host's keypair used for address derivation of peer and message signing.
    keypair: Keypair,
}

impl P2PContext {
    pub fn new(cmd_tx: AdexCmdTx, keypair: Keypair) -> Self {
        P2PContext {
            cmd_tx: Mutex::new(cmd_tx),
            keypair,
        }
    }

    #[inline(always)]
    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }

    #[inline(always)]
    pub fn peer_id(&self) -> PeerId {
        self.keypair.public().to_peer_id()
    }

    pub fn store_to_mm_arc(self, ctx: &MmArc) {
        *ctx.p2p_ctx.lock().unwrap() = Some(Arc::new(self))
    }

    pub fn fetch_from_mm_arc(ctx: &MmArc) -> Arc<Self> {
        ctx.p2p_ctx
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .clone()
            .downcast()
            .unwrap()
    }
}
