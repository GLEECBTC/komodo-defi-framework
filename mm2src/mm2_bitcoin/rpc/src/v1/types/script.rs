use global_script::ScriptType as GlobalScriptType;
use serde::de::Unexpected;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum ScriptType {
    NonStandard,
    PubKey,
    PubKeyHash,
    ScriptHash,
    Multisig,
    NullData,
    WitnessScript,
    WitnessKey,
    Taproot,
    // Qtum specific
    CallSender,
    CreateSender,
    Call,
    Create,
    LelantusMint,
    SparkMint,
    ColdStaking,
    // Komodo smart chains specific
    CryptoCondition,
}

impl From<GlobalScriptType> for ScriptType {
    fn from(script_type: GlobalScriptType) -> Self {
        match script_type {
            GlobalScriptType::NonStandard => ScriptType::NonStandard,
            GlobalScriptType::PubKey => ScriptType::PubKey,
            GlobalScriptType::PubKeyHash => ScriptType::PubKeyHash,
            GlobalScriptType::ScriptHash => ScriptType::ScriptHash,
            GlobalScriptType::Multisig => ScriptType::Multisig,
            GlobalScriptType::NullData => ScriptType::NullData,
            GlobalScriptType::WitnessScript => ScriptType::WitnessScript,
            GlobalScriptType::WitnessKey => ScriptType::WitnessKey,
            GlobalScriptType::Taproot => ScriptType::Taproot,
            GlobalScriptType::CallSender => ScriptType::CallSender,
            GlobalScriptType::CreateSender => ScriptType::CreateSender,
            GlobalScriptType::Call => ScriptType::Call,
            GlobalScriptType::Create => ScriptType::Create,
            GlobalScriptType::ColdStaking => ScriptType::ColdStaking,
            GlobalScriptType::CryptoCondition => ScriptType::CryptoCondition,
        }
    }
}

impl Serialize for ScriptType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            ScriptType::NonStandard => "nonstandard".serialize(serializer),
            ScriptType::PubKey => "pubkey".serialize(serializer),
            ScriptType::PubKeyHash => "pubkeyhash".serialize(serializer),
            ScriptType::ScriptHash => "scripthash".serialize(serializer),
            ScriptType::Multisig => "multisig".serialize(serializer),
            ScriptType::NullData => "nulldata".serialize(serializer),
            ScriptType::WitnessScript => "witness_v0_scripthash".serialize(serializer),
            ScriptType::WitnessKey => "witness_v0_keyhash".serialize(serializer),
            ScriptType::Taproot => "witness_v1_taproot".serialize(serializer),
            ScriptType::CallSender => "call_sender".serialize(serializer),
            ScriptType::CreateSender => "create_sender".serialize(serializer),
            ScriptType::Call => "call".serialize(serializer),
            ScriptType::Create => "create".serialize(serializer),
            ScriptType::LelantusMint => "lelantusmint".serialize(serializer),
            ScriptType::SparkMint => "sparksmint".serialize(serializer),
            ScriptType::ColdStaking => "cold_staking".serialize(serializer),
            ScriptType::CryptoCondition => "cryptocondition".serialize(serializer),
        }
    }
}

impl<'a> Deserialize<'a> for ScriptType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'a>,
    {
        use serde::de::Visitor;

        struct ScriptTypeVisitor;

        impl Visitor<'_> for ScriptTypeVisitor {
            type Value = ScriptType;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("script type")
            }

            fn visit_str<E>(self, value: &str) -> Result<ScriptType, E>
            where
                E: ::serde::de::Error,
            {
                match value {
                    "nonstandard" => Ok(ScriptType::NonStandard),
                    "pubkey" => Ok(ScriptType::PubKey),
                    "pubkeyhash" => Ok(ScriptType::PubKeyHash),
                    "scripthash" => Ok(ScriptType::ScriptHash),
                    "multisig" => Ok(ScriptType::Multisig),
                    "nulldata" => Ok(ScriptType::NullData),
                    "witness_v0_scripthash" => Ok(ScriptType::WitnessScript),
                    "witness_v0_keyhash" => Ok(ScriptType::WitnessKey),
                    "witness_v1_taproot" => Ok(ScriptType::Taproot),
                    "call_sender" => Ok(ScriptType::CallSender),
                    "create_sender" => Ok(ScriptType::CreateSender),
                    "call" => Ok(ScriptType::Call),
                    "create" => Ok(ScriptType::Create),
                    "lelantusmint" => Ok(ScriptType::LelantusMint),
                    "sparksmint" => Ok(ScriptType::SparkMint),
                    "cold_staking" => Ok(ScriptType::ColdStaking),
                    "cryptocondition" => Ok(ScriptType::CryptoCondition),
                    _ => Err(E::invalid_value(Unexpected::Str(value), &self)),
                }
            }
        }

        deserializer.deserialize_identifier(ScriptTypeVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::ScriptType;
    use serde_json;

    #[test]
    fn script_type_serialize() {
        assert_eq!(
            serde_json::to_string(&ScriptType::NonStandard).unwrap(),
            r#""nonstandard""#
        );
        assert_eq!(serde_json::to_string(&ScriptType::PubKey).unwrap(), r#""pubkey""#);
        assert_eq!(
            serde_json::to_string(&ScriptType::PubKeyHash).unwrap(),
            r#""pubkeyhash""#
        );
        assert_eq!(
            serde_json::to_string(&ScriptType::ScriptHash).unwrap(),
            r#""scripthash""#
        );
        assert_eq!(serde_json::to_string(&ScriptType::Multisig).unwrap(), r#""multisig""#);
        assert_eq!(serde_json::to_string(&ScriptType::NullData).unwrap(), r#""nulldata""#);
        assert_eq!(
            serde_json::to_string(&ScriptType::WitnessScript).unwrap(),
            r#""witness_v0_scripthash""#
        );
        assert_eq!(
            serde_json::to_string(&ScriptType::WitnessKey).unwrap(),
            r#""witness_v0_keyhash""#
        );
        assert_eq!(
            serde_json::to_string(&ScriptType::CallSender).unwrap(),
            r#""call_sender""#
        );
        assert_eq!(
            serde_json::to_string(&ScriptType::CreateSender).unwrap(),
            r#""create_sender""#
        );
        assert_eq!(serde_json::to_string(&ScriptType::Call).unwrap(), r#""call""#);
        assert_eq!(serde_json::to_string(&ScriptType::Create).unwrap(), r#""create""#);
        assert_eq!(
            serde_json::to_string(&ScriptType::ColdStaking).unwrap(),
            r#""cold_staking""#
        );
        assert_eq!(
            serde_json::to_string(&ScriptType::CryptoCondition).unwrap(),
            r#""cryptocondition""#
        );
    }

    #[test]
    fn script_type_deserialize() {
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""nonstandard""#).unwrap(),
            ScriptType::NonStandard
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""pubkey""#).unwrap(),
            ScriptType::PubKey
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""pubkeyhash""#).unwrap(),
            ScriptType::PubKeyHash
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""scripthash""#).unwrap(),
            ScriptType::ScriptHash
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""multisig""#).unwrap(),
            ScriptType::Multisig
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""nulldata""#).unwrap(),
            ScriptType::NullData
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""witness_v0_scripthash""#).unwrap(),
            ScriptType::WitnessScript
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""witness_v0_keyhash""#).unwrap(),
            ScriptType::WitnessKey
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""call_sender""#).unwrap(),
            ScriptType::CallSender
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""create_sender""#).unwrap(),
            ScriptType::CreateSender
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""call""#).unwrap(),
            ScriptType::Call
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""create""#).unwrap(),
            ScriptType::Create
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""cold_staking""#).unwrap(),
            ScriptType::ColdStaking
        );
        assert_eq!(
            serde_json::from_str::<ScriptType>(r#""cryptocondition""#).unwrap(),
            ScriptType::CryptoCondition
        );
    }
}
