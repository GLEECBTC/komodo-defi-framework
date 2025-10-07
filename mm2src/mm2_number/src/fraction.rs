use crate::big_int_str::BigIntStr;
use crate::from_dec_to_ratio;
use bigdecimal::BigDecimal;
use num_bigint::BigInt;
use num_rational::BigRational;
use serde::Serialize;
use serde::{de, Deserialize, Deserializer};

/// Rational number representation de/serializable in human readable form
/// Should simplify the visual perception and parsing in code
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Fraction {
    /// Numerator
    pub(crate) numer: BigIntStr,
    /// Denominator
    pub(crate) denom: BigIntStr,
}

impl Fraction {
    /// Numerator
    pub fn numer(&self) -> &BigInt {
        self.numer.inner()
    }

    /// Denominator
    pub fn denom(&self) -> &BigInt {
        self.denom.inner()
    }
}

impl From<BigRational> for Fraction {
    fn from(ratio: BigRational) -> Fraction {
        let (numer, denom) = ratio.into();
        Fraction {
            numer: numer.into(),
            denom: denom.into(),
        }
    }
}

impl From<Fraction> for BigRational {
    fn from(fraction: Fraction) -> Self {
        BigRational::new(fraction.numer.into(), fraction.denom.into())
    }
}

impl From<BigDecimal> for Fraction {
    fn from(dec: BigDecimal) -> Fraction {
        from_dec_to_ratio(&dec).into()
    }
}

impl<'de> Deserialize<'de> for Fraction {
    fn deserialize<D>(deserializer: D) -> Result<Self, <D as Deserializer<'de>>::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct FractionHelper {
            numer: BigIntStr,
            denom: BigIntStr,
        }

        let maybe_fraction: FractionHelper = Deserialize::deserialize(deserializer)?;
        if maybe_fraction.denom.inner() == &0.into() {
            return Err(de::Error::custom("denom can not be 0"));
        }

        Ok(Fraction {
            numer: maybe_fraction.numer,
            denom: maybe_fraction.denom,
        })
    }
}
