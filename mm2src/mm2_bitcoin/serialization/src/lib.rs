extern crate byteorder;
extern crate derive_more;
extern crate primitives;
extern crate test_helpers;

mod compact_integer;
mod impls;
mod list;
mod reader;
mod stream;

pub use primitives::{bytes, compact, hash};

pub use compact_integer::{parse_compact_int, CompactInteger};
pub use list::List;
pub use reader::{deserialize, deserialize_iterator, CoinVariant, Deserializable, Error, ReadIterator, Reader};
pub use stream::{
    serialize, serialize_list, serialize_with_flags, serialized_list_size, serialized_list_size_with_flags,
    Serializable, Stream, SERIALIZE_TRANSACTION_WITNESS,
};
