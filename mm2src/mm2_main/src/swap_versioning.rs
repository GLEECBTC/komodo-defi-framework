//! Swap Versioning Module
//!
//! This module provides a dedicated type for handling swap versioning

#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SwapVersion {
    pub version: u8,
}

impl Default for SwapVersion {
    fn default() -> Self {
        Self {
            version: legacy_swap_version(),
        }
    }
}

impl SwapVersion {
    pub(crate) const fn is_legacy(&self) -> bool {
        self.version == legacy_swap_version()
    }
}

impl From<u8> for SwapVersion {
    fn from(version: u8) -> Self {
        Self { version }
    }
}

pub(crate) const fn legacy_swap_version() -> u8 {
    1
}
