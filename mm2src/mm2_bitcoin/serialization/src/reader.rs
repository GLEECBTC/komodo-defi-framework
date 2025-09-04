use compact_integer::CompactInteger;
use derive_more::Display;
use std::{io, marker};

const BTC_FORKS: &[&str] = &["BTC", "BCH", "NAV", "RIC"];
const PIVX_FORKS: &[&str] = &["PIVX", "DOGEC"];
const QTUM_FORKS: &[&str] = &["QTUM", "RUNES"];
const RVN_FORKS: &[&str] = &["RVN", "AIPG", "XNA", "EVR", "MEWC", "AVN"];

pub fn deserialize<R, T>(buffer: R) -> Result<T, Error>
where
    R: io::Read,
    T: Deserializable,
{
    let mut reader = Reader::from_read(buffer);
    let result = reader.read()?;

    if reader.is_finished() {
        Ok(result)
    } else {
        Err(Error::UnreadData)
    }
}

pub fn deserialize_iterator<R, T>(buffer: R) -> ReadIterator<R, T>
where
    R: io::Read,
    T: Deserializable,
{
    ReadIterator {
        reader: Reader::from_read(buffer),
        iter_type: marker::PhantomData,
    }
}

#[derive(Debug, Display, PartialEq)]
pub enum Error {
    MalformedData,
    UnexpectedEnd,
    UnreadData,
    Custom(String),
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(_: io::Error) -> Self {
        Error::UnexpectedEnd
    }
}

pub trait Deserializable {
    fn deserialize<T>(reader: &mut Reader<T>) -> Result<Self, Error>
    where
        Self: Sized,
        T: io::Read;
}

#[derive(Debug)]
pub enum CoinVariant {
    // Todo: https://github.com/KomodoPlatform/atomicDEX-API/issues/1345
    BTC,
    Qtum,
    LBC,
    Standard,
    PPC,
    /// Needed to deserialize `RICK` block headers correctly as it's genesis header is `version 1` while all other
    /// block headers are `version 4`.
    RICK,
    /// Same reason as RICK.
    MORTY,
    RVN,
    PIVX,
}

impl CoinVariant {
    pub fn is_btc(&self) -> bool {
        matches!(self, CoinVariant::BTC)
    }
    pub fn is_qtum(&self) -> bool {
        matches!(self, CoinVariant::Qtum)
    }
    pub fn is_lbc(&self) -> bool {
        matches!(self, CoinVariant::LBC)
    }
    pub fn is_ppc(&self) -> bool {
        matches!(self, CoinVariant::PPC)
    }
    pub fn is_kmd_assetchain(&self) -> bool {
        matches!(self, CoinVariant::RICK | CoinVariant::MORTY)
    }
    pub fn is_rvn(&self) -> bool {
        matches!(self, CoinVariant::RVN)
    }

    pub fn is_pivx(&self) -> bool {
        matches!(self, CoinVariant::PIVX)
    }
}

fn ticker_matches(ticker: &str, with: &str) -> bool {
    ticker == with || ticker.contains(&format!("{with}-")) || ticker.contains(&format!("{with}_"))
}

impl From<&str> for CoinVariant {
    fn from(ticker: &str) -> Self {
        match ticker {
            // "BTC", "BTC-segwit", "tBTC", "tBTC-segwit", "BCH", "tBCH", etc..
            t if BTC_FORKS.iter().any(|ticker| ticker_matches(t, ticker)) => CoinVariant::BTC,
            // "QTUM", "QTUM-segwit", "tQTUM", "tQTUM-segwit", etc..
            t if QTUM_FORKS.iter().any(|ticker| ticker_matches(t, ticker)) => CoinVariant::Qtum,
            // "LBC", "LBC-segwit", etc..
            t if ticker_matches(t, "LBC") => CoinVariant::LBC,
            // "PPC", "PPC-segwit", etc..
            t if ticker_matches(t, "PPC") => CoinVariant::PPC,
            // "RICK"
            t if ticker_matches(t, "RICK") => CoinVariant::RICK,
            // "MORTY"
            t if ticker_matches(t, "MORTY") => CoinVariant::MORTY,
            // `RVN`, `AIPG`, etc..
            t if RVN_FORKS.iter().any(|ticker| ticker_matches(t, ticker)) => CoinVariant::RVN,
            // PIVX family ("PIVX", "DOGEC", …)
            t if PIVX_FORKS.iter().any(|ticker| ticker_matches(t, ticker)) => CoinVariant::PIVX,
            _ => CoinVariant::Standard,
        }
    }
}

/// Bitcoin structures reader.
#[derive(Debug)]
pub struct Reader<T> {
    buffer: T,
    peeked: Option<u8>,
    coin_variant: CoinVariant,
}

impl<'a> Reader<&'a [u8]> {
    /// Convenient way of creating for slice of bytes
    pub fn new(buffer: &'a [u8]) -> Self {
        Reader {
            buffer,
            peeked: None,
            coin_variant: CoinVariant::Standard,
        }
    }

    /// Convenient way of creating for slice of bytes
    pub fn new_with_coin_variant(buffer: &'a [u8], coin_variant: CoinVariant) -> Self {
        Reader {
            buffer,
            peeked: None,
            coin_variant,
        }
    }
}

impl<T> io::Read for Reader<T>
where
    T: io::Read,
{
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        // most of the times, there will be nothing in peeked,
        // so to make it as efficient as possible, check it
        // only once
        match self.peeked.take() {
            None => io::Read::read(&mut self.buffer, buf),
            Some(peeked) if buf.is_empty() => {
                self.peeked = Some(peeked);
                Ok(0)
            },
            Some(peeked) => {
                buf[0] = peeked;
                io::Read::read(&mut self.buffer, &mut buf[1..]).map(|x| x + 1)
            },
        }
    }
}

impl<R> Reader<R>
where
    R: io::Read,
{
    pub fn from_read(read: R) -> Self {
        Reader {
            buffer: read,
            peeked: None,
            coin_variant: CoinVariant::Standard,
        }
    }

    pub fn read<T>(&mut self) -> Result<T, Error>
    where
        T: Deserializable,
    {
        T::deserialize(self)
    }

    pub fn read_with_proxy<T, F>(&mut self, proxy: F) -> Result<T, Error>
    where
        T: Deserializable,
        F: FnMut(&[u8]),
    {
        let mut reader = Reader::from_read(Proxy::new(self, proxy));
        T::deserialize(&mut reader)
    }

    pub fn read_slice(&mut self, bytes: &mut [u8]) -> Result<(), Error> {
        io::Read::read_exact(self, bytes).map_err(|_| Error::UnexpectedEnd)
    }

    pub fn read_list<T>(&mut self) -> Result<Vec<T>, Error>
    where
        T: Deserializable,
    {
        let len: usize = self.read::<CompactInteger>()?.into();
        let mut result = Vec::with_capacity(len);

        for _ in 0..len {
            result.push(self.read()?);
        }

        Ok(result)
    }

    pub fn read_list_max<T>(&mut self, max: usize) -> Result<Vec<T>, Error>
    where
        T: Deserializable,
    {
        let len: usize = self.read::<CompactInteger>()?.into();
        if len > max {
            return Err(Error::MalformedData);
        }

        let mut result = Vec::with_capacity(len);

        for _ in 0..len {
            result.push(self.read()?);
        }

        Ok(result)
    }

    pub fn is_finished(&mut self) -> bool {
        if self.peeked.is_some() {
            return false;
        }

        let peek: &mut [u8] = &mut [0u8];
        match self.read_slice(peek) {
            Ok(_) => {
                self.peeked = Some(peek[0]);
                false
            },
            Err(_) => true,
        }
    }

    pub fn coin_variant(&self) -> &CoinVariant {
        &self.coin_variant
    }
}

/// Should be used to iterate over structures of the same type
pub struct ReadIterator<R, T> {
    reader: Reader<R>,
    iter_type: marker::PhantomData<T>,
}

impl<R, T> Iterator for ReadIterator<R, T>
where
    R: io::Read,
    T: Deserializable,
{
    type Item = Result<T, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.reader.is_finished() {
            None
        } else {
            Some(self.reader.read())
        }
    }
}

struct Proxy<F, T> {
    from: F,
    to: T,
}

impl<F, T> Proxy<F, T> {
    fn new(from: F, to: T) -> Self {
        Proxy { from, to }
    }
}

impl<F, T> io::Read for Proxy<F, T>
where
    F: io::Read,
    T: FnMut(&[u8]),
{
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        let len = io::Read::read(&mut self.from, buf)?;
        let to = &mut self.to;
        to(&buf[..len]);
        Ok(len)
    }
}
