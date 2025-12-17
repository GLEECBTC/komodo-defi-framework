//! Variable-length integer commonly used in the Bitcoin [P2P protocol](https://bitcoin.org/en/developer-reference#compactsize-unsigned-integers)

use std::{fmt, io};
use {Deserializable, Error as ReaderError, Reader, Serializable, Stream};

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum CompactIntegerError {
    ParseError(String),
}

/// A type of variable-length integer commonly used in the Bitcoin P2P protocol and Bitcoin serialized data structures.
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct CompactInteger(u64);

/// Parse a CompactInteger into its data length and the number it represents
/// Useful for Parsing Vins and Vouts. Returns `ParseError` if insufficient bytes.
///
/// # Arguments
///
/// * `buf` - A byte-string starting with a CompactInteger
///
/// # Returns
///
/// * (length, number) - the length of the data in bytes, and the number it represents
pub fn parse_compact_int<T: AsRef<[u8]> + ?Sized>(buf: &T) -> Result<CompactInteger, CompactIntegerError> {
    let buf = buf.as_ref();
    if buf.is_empty() {
        return Err(CompactIntegerError::ParseError("Empty buffer!".into()));
    }
    let length = CompactInteger::data_length(buf[0]) as usize;

    if length == 0 {
        return Ok(buf[0].into());
    }
    if buf.len() < 1 + length {
        return Err(CompactIntegerError::ParseError("Insufficient bytes!".into()));
    }

    let mut num_bytes = [0u8; 8];
    num_bytes[..length].copy_from_slice(&buf[1..=length]);

    Ok(u64::from_le_bytes(num_bytes).into())
}

impl CompactInteger {
    /// The underlying number as a usize
    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }

    /// Determine the length of the compact integer when serialized
    pub fn serialized_length(&self) -> usize {
        match self.0 {
            0..=0xfc => 1,
            0xfd..=0xffff => 3,
            0x10000..=0xffff_ffff => 5,
            _ => 9,
        }
    }

    /// Determines the length of a CompactInteger in bytes.
    /// A CompactInteger of > 1 byte is prefixed with a flag indicating its length.
    ///
    /// # Arguments
    ///
    /// * `flag` - The first byte of a compact_integer
    pub fn data_length(flag: u8) -> u8 {
        let length: u8 = match flag {
            0xfd => 2,
            0xfe => 4,
            0xff => 8,
            _ => 0,
        };
        length
    }
}

impl fmt::Display for CompactInteger {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<CompactInteger> for usize {
    fn from(i: CompactInteger) -> Self {
        i.0 as usize
    }
}

impl From<CompactInteger> for u64 {
    fn from(i: CompactInteger) -> Self {
        i.0
    }
}

impl From<u8> for CompactInteger {
    fn from(i: u8) -> Self {
        CompactInteger(i as u64)
    }
}

impl From<u16> for CompactInteger {
    fn from(i: u16) -> Self {
        CompactInteger(i as u64)
    }
}

impl From<u32> for CompactInteger {
    fn from(i: u32) -> Self {
        CompactInteger(i as u64)
    }
}

impl From<usize> for CompactInteger {
    fn from(i: usize) -> Self {
        CompactInteger(i as u64)
    }
}

impl From<u64> for CompactInteger {
    fn from(i: u64) -> Self {
        CompactInteger(i)
    }
}

impl AsRef<u64> for CompactInteger {
    fn as_ref(&self) -> &u64 {
        &self.0
    }
}

impl Serializable for CompactInteger {
    fn serialize(&self, stream: &mut Stream) {
        match self.0 {
            0..=0xfc => {
                stream.append(&(self.0 as u8));
            },
            0xfd..=0xffff => {
                stream.append(&0xfdu8).append(&(self.0 as u16));
            },
            0x10000..=0xffff_ffff => {
                stream.append(&0xfeu8).append(&(self.0 as u32));
            },
            _ => {
                stream.append(&0xffu8).append(&self.0);
            },
        }
    }

    fn serialized_size(&self) -> usize {
        match self.0 {
            0..=0xfc => 1,
            0xfd..=0xffff => 3,
            0x10000..=0xffff_ffff => 5,
            _ => 9,
        }
    }
}

impl Deserializable for CompactInteger {
    fn deserialize<T>(reader: &mut Reader<T>) -> Result<Self, ReaderError>
    where
        T: io::Read,
    {
        let result = match reader.read::<u8>()? {
            i @ 0..=0xfc => i.into(),
            0xfd => reader.read::<u16>()?.into(),
            0xfe => reader.read::<u32>()?.into(),
            _ => reader.read::<u64>()?.into(),
        };

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_compact_int, CompactInteger, CompactIntegerError};
    use test_helpers::hex::force_deserialize_hex;
    use {Error as ReaderError, Reader, Stream};

    #[test]
    fn test_compact_integer_data_length() {
        let input: u8 = 1;
        let expected: u8 = 0;
        assert_eq!(CompactInteger::data_length(input), expected);

        let input: u8 = 253;
        let expected: u8 = 2;
        assert_eq!(CompactInteger::data_length(input), expected);

        let input: u8 = 254;
        let expected: u8 = 4;
        assert_eq!(CompactInteger::data_length(input), expected);

        let input: u8 = 255;
        let expected: u8 = 8;
        assert_eq!(CompactInteger::data_length(input), expected);
    }

    #[test]
    fn test_parse_compact_integers() {
        let input = force_deserialize_hex("0x01");
        let expected = [0, 1];
        assert_eq!(parse_compact_int(&input).unwrap().as_usize(), expected[1]);

        let input = force_deserialize_hex("0xff0000000000000000");
        let expected = [8, 0];
        assert_eq!(parse_compact_int(&input).unwrap().as_usize(), expected[1]);

        let input = force_deserialize_hex("0xfe03000000");
        let expected = [4, 3];
        assert_eq!(parse_compact_int(&input).unwrap().as_usize(), expected[1]);

        let input = force_deserialize_hex("0xfd0001");
        let expected = [2, 256];
        assert_eq!(parse_compact_int(&input).unwrap().as_usize(), expected[1]);
    }

    #[test]
    fn test_parse_compact_integer_errors() {
        let input = force_deserialize_hex("0xfd01");
        let error = parse_compact_int(&input).unwrap_err();
        assert_eq!(error, CompactIntegerError::ParseError("Insufficient bytes!".into()));

        let input = force_deserialize_hex("0xfe010000");
        let error = parse_compact_int(&input).unwrap_err();
        assert_eq!(error, CompactIntegerError::ParseError("Insufficient bytes!".into()));

        let input = force_deserialize_hex("0xff01000000000000");
        let error = parse_compact_int(&input).unwrap_err();
        assert_eq!(error, CompactIntegerError::ParseError("Insufficient bytes!".into()));

        let input = force_deserialize_hex("0x");
        let error = parse_compact_int(&input).unwrap_err();
        assert_eq!(error, CompactIntegerError::ParseError("Empty buffer!".into()));
    }

    #[test]
    fn test_compact_integer_stream() {
        let mut stream = Stream::default();

        stream
            .append(&CompactInteger::from(0u64))
            .append(&CompactInteger::from(0xfcu64))
            .append(&CompactInteger::from(0xfdu64))
            .append(&CompactInteger::from(0xffffu64))
            .append(&CompactInteger::from(0x10000u64))
            .append(&CompactInteger::from(0xffff_ffffu64))
            .append(&CompactInteger::from(0x1_0000_0000u64));

        let expected = vec![
            0, 0xfc, 0xfd, 0xfd, 0x00, 0xfd, 0xff, 0xff, 0xfe, 0x00, 0x00, 0x01, 0x00, 0xfe, 0xff, 0xff, 0xff, 0xff,
            0xff, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        ]
        .into();

        assert_eq!(stream.out(), expected);
    }

    #[test]
    fn test_compact_integer_reader() {
        let buffer = vec![
            0, 0xfc, 0xfd, 0xfd, 0x00, 0xfd, 0xff, 0xff, 0xfe, 0x00, 0x00, 0x01, 0x00, 0xfe, 0xff, 0xff, 0xff, 0xff,
            0xff, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        ];

        let mut reader = Reader::new(&buffer);
        assert_eq!(reader.read::<CompactInteger>().unwrap(), 0u64.into());
        assert_eq!(reader.read::<CompactInteger>().unwrap(), 0xfcu64.into());
        assert_eq!(reader.read::<CompactInteger>().unwrap(), 0xfdu64.into());
        assert_eq!(reader.read::<CompactInteger>().unwrap(), 0xffffu64.into());
        assert_eq!(reader.read::<CompactInteger>().unwrap(), 0x10000u64.into());
        assert_eq!(reader.read::<CompactInteger>().unwrap(), 0xffff_ffffu64.into());
        assert_eq!(reader.read::<CompactInteger>().unwrap(), 0x1_0000_0000u64.into());
        assert_eq!(reader.read::<CompactInteger>().unwrap_err(), ReaderError::UnexpectedEnd);
    }
}
