#![feature(negative_impls)]
#![feature(auto_traits)]
#![feature(allocator_api)]

pub mod common_errors;
pub mod discard_mm_trace;
pub mod map_mm_error;
pub mod map_to_mm;
pub mod map_to_mm_fut;
pub mod mm_error;
pub mod mm_json_error;
pub mod or_mm_error;
pub mod split_mm;

pub mod prelude {
    pub use crate::common_errors::{WithInternal, WithTimeout};
    pub use crate::discard_mm_trace::DiscardMmTrace;
    pub use crate::map_mm_error::{MapMmError, MmResultExt};
    pub use crate::map_to_mm::MapToMmResult;
    pub use crate::map_to_mm_fut::MapToMmFutureExt;
    pub use crate::mm_error::{MmError, MmResult, NotMmError, SerMmErrorType};
    pub use crate::mm_json_error::MmJsonError;
    pub use crate::or_mm_error::OrMmError;
    pub use crate::split_mm::SplitMmResult;
    pub use ser_error::SerializeErrorType;
}
