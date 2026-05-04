use super::authorization::GasfreeSignedAuthorization;
use super::config::ResolvedTronGaslessProvider;
use crate::eth::tron::TronAddress;
use crate::hd_wallet::HDAddressSelector;
use serde::{Deserialize, Serialize};

pub const TRON_GASFREE_RELAY_TYPE: &str = "tron_gasfree";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TronGasfreeRelayPayload {
    pub relay_type: String,
    pub chain_id: String,
    pub coin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<HDAddressSelector>,
    pub from_address: String,
    pub gasfree_address: String,
    pub verifying_contract: String,
    pub signed_authorization: GasfreeSignedAuthorization,
    pub created_at: String,
}

pub(crate) struct SignedWithdrawRelayPayload<'a> {
    pub provider: &'a ResolvedTronGaslessProvider,
    pub coin: String,
    pub from: Option<HDAddressSelector>,
    pub from_address: String,
    pub gasfree_address: TronAddress,
    pub signed_authorization: GasfreeSignedAuthorization,
    pub created_at: String,
}

impl From<SignedWithdrawRelayPayload<'_>> for TronGasfreeRelayPayload {
    fn from(input: SignedWithdrawRelayPayload<'_>) -> Self {
        TronGasfreeRelayPayload {
            relay_type: TRON_GASFREE_RELAY_TYPE.to_string(),
            chain_id: input.provider.network().chain_id().to_string(),
            coin: input.coin,
            from: input.from,
            from_address: input.from_address,
            gasfree_address: input.gasfree_address.to_base58(),
            verifying_contract: input.provider.verifying_contract().to_base58(),
            signed_authorization: input.signed_authorization,
            created_at: input.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::tron::gasfree::test_helpers::{test_provider, DEFAULT_SERVICE_PROVIDER};
    use crate::eth::tron::{Network, TronAddress, TronSignature};
    use ethereum_types::U256;
    use serde_json::json;

    fn sample_payload() -> TronGasfreeRelayPayload {
        TronGasfreeRelayPayload {
            relay_type: TRON_GASFREE_RELAY_TYPE.to_string(),
            chain_id: "3448148188".to_string(),
            coin: "USDT-TRC20-NILE".to_string(),
            from: Some(HDAddressSelector::AddressId(
                crate::hd_wallet::HDPathAccountToAddressId {
                    account_id: 0,
                    chain: crypto::Bip44Chain::External,
                    address_id: 2,
                },
            )),
            from_address: "TMVQGm1qAQYVdetCeGRRkTWYYrLXuHK2HC".to_string(),
            gasfree_address: "TCtSt8fCkZcVdrGpaVHUr6P8EmdjysswMF".to_string(),
            verifying_contract: "THQGuFzL87ZqhxkgqYEryRAd7gqFqL5rdc".to_string(),
            signed_authorization: GasfreeSignedAuthorization {
                token: "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf".parse::<TronAddress>().unwrap(),
                service_provider: "TKtWbdzEq5ss9vTS9kwRhBp5mXmBfBns3E".parse::<TronAddress>().unwrap(),
                user: "TMVQGm1qAQYVdetCeGRRkTWYYrLXuHK2HC".parse::<TronAddress>().unwrap(),
                receiver: "TJM1BE5wq1VdHh3gwjUeyaVkvZp9DVYCfC".parse::<TronAddress>().unwrap(),
                value: U256::from(5_000_000u64),
                max_fee: U256::from(2_000u64),
                deadline: U256::from(1_742_000_000u64),
                version: U256::from(1u64),
                nonce: U256::from(9u64),
                sig: TronSignature::from_hex_str(&"11".repeat(65)).unwrap(),
            },
            created_at: "2026-04-23T20:47:34Z".to_string(),
        }
    }

    #[test]
    fn relay_payload_serializes_expected_shape() {
        let payload = serde_json::to_value(sample_payload()).unwrap();
        assert_eq!(payload["relay_type"], json!(TRON_GASFREE_RELAY_TYPE));
        assert_eq!(payload["chain_id"], json!("3448148188"));
        assert_eq!(payload["from"]["account_id"], json!(0));
        assert_eq!(payload["from_address"], json!("TMVQGm1qAQYVdetCeGRRkTWYYrLXuHK2HC"));
        assert_eq!(payload["signed_authorization"]["value"], json!("5000000"));
        assert_eq!(payload["signed_authorization"]["max_fee"], json!("2000"));
    }

    #[test]
    fn signed_withdraw_payload_conversion_uses_supplied_created_at() {
        let provider = test_provider(Network::Nile, DEFAULT_SERVICE_PROVIDER);
        let created_at = "2026-04-23T20:47:34Z".to_string();
        let signed_authorization = sample_payload().signed_authorization;

        let payload = TronGasfreeRelayPayload::from(SignedWithdrawRelayPayload {
            provider: &provider,
            coin: "USDT-TRC20-NILE".to_string(),
            from: None,
            from_address: "TMVQGm1qAQYVdetCeGRRkTWYYrLXuHK2HC".to_string(),
            gasfree_address: "TCtSt8fCkZcVdrGpaVHUr6P8EmdjysswMF".parse().unwrap(),
            signed_authorization,
            created_at: created_at.clone(),
        });

        assert_eq!(payload.created_at, created_at);
    }
}
