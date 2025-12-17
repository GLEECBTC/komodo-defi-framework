use serde::Serialize;

pub const TAG: &str = "error_type";
pub const CONTENT: &str = "error_data";

/// [`SerializeErrorType`] trait ensures that the each type implementing this trait serializes into
/// `error_type: String` and `error_data: Option` fields only.
pub trait SerializeErrorType: Serialize + __private::SerializeErrorTypeImpl {
    fn tag() -> &'static str {
        TAG
    }

    fn content() -> &'static str {
        CONTENT
    }
}

/// Every type that implements [`__private::SerializeErrorTypeImpl`] also implements [`SerializeErrorType`].
impl<T: Serialize + __private::SerializeErrorTypeImpl> SerializeErrorType for T {}

pub mod __private {
    /// This trait must be implemented by deriving `#[derive(SerializeErrorType)]` only.
    pub trait SerializeErrorTypeImpl {}
}
