use std::fmt;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, Hash)]
pub struct ChannelId(u16);

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "channel-{}", self.0)
    }
}

impl ChannelId {
    pub fn new(id: u16) -> Self {
        Self(id)
    }
}
