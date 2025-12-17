use super::connection_manager::ConnectionManager;

use crate::RpcTransportEventHandler;

/// An `RpcTransportEventHandler` that notifies the `ConnectionManager` upon connections and  disconnections.
///
/// When a connection is connected or disconnected, this event handler will notify the `ConnectionManager`
/// to handle the the event.
pub struct ElectrumConnectionManagerNotifier {
    pub connection_manager: ConnectionManager,
}

impl RpcTransportEventHandler for ElectrumConnectionManagerNotifier {
    fn debug_info(&self) -> String {
        "ElectrumConnectionManagerNotifier".into()
    }

    fn on_connected(&self, address: &str) -> Result<(), String> {
        self.connection_manager.on_connected(address);
        Ok(())
    }

    fn on_disconnected(&self, address: &str) -> Result<(), String> {
        self.connection_manager.on_disconnected(address);
        Ok(())
    }

    fn on_incoming_response(&self, _data: &[u8]) {}

    fn on_outgoing_request(&self, _data: &[u8]) {}
}
