use crate::StreamerId;
use serde_json::Value as Json;

// Note `Event` shouldn't be `Clone`able, but rather Arc/Rc wrapped and then shared.
// This is only for testing.
/// Multi-purpose/generic event type that can easily be used over the event streaming
#[cfg_attr(any(test, target_arch = "wasm32"), derive(Clone, Debug, PartialEq))]
pub struct Event {
    /// The type of the event (balance, network, swap, etc...).
    streamer_id: StreamerId,
    /// The message to be sent to the client.
    message: Json,
    /// Indicating whether this event is an error event or a normal one.
    error: bool,
}

impl Event {
    /// Creates a new `Event` instance with the specified event type and message.
    #[inline(always)]
    pub fn new(streamer_id: StreamerId, message: Json) -> Self {
        Self {
            streamer_id,
            message,
            error: false,
        }
    }

    /// Create a new error `Event` instance with the specified error event type and message.
    #[inline(always)]
    pub fn err(streamer_id: StreamerId, message: Json) -> Self {
        Self {
            streamer_id,
            message,
            error: true,
        }
    }

    /// Returns whether this event is an error or not
    #[inline(always)]
    pub fn is_error(&self) -> bool {
        self.error
    }

    /// Returns the `streamer_id` (the ID of the streamer firing this event).
    #[inline(always)]
    pub fn origin(&self) -> &StreamerId {
        &self.streamer_id
    }

    /// Returns the event type and message as a pair.
    pub fn get(&self) -> (String, &Json) {
        let prefix = if self.error { "ERROR:" } else { "" };
        (format!("{prefix}{}", self.streamer_id), &self.message)
    }
}
