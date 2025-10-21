use crate::fraction::Fraction;
use crate::from_ratio_to_dec;
use crate::mm_number::MmNumber;
use bigdecimal::BigDecimal;
use num_rational::BigRational;
use serde::{Deserialize, Serialize};

/// MmNumber representation in all available forms.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MmNumberMultiRepr {
    pub decimal: BigDecimal,
    pub rational: BigRational,
    pub fraction: Fraction,
}

impl From<MmNumber> for MmNumberMultiRepr {
    fn from(num: MmNumber) -> Self {
        MmNumberMultiRepr {
            decimal: num.to_decimal(),
            fraction: num.to_fraction(),
            rational: num.0,
        }
    }
}

impl From<BigRational> for MmNumberMultiRepr {
    fn from(rational: BigRational) -> Self {
        MmNumberMultiRepr {
            decimal: from_ratio_to_dec(&rational),
            fraction: rational.clone().into(),
            rational,
        }
    }
}
