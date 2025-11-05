use lazy_static::lazy_static;
use mm2_metamask::{Eip712, ObjectType, PropertyType};

const ADEX_LOGIN_TYPE: &str = "AtomicDEXLogin";

lazy_static! {
    static ref ADEX_TYPES: [ObjectType; 2] = adex_login_types();
}

pub(crate) fn adex_eip712_request(
    domain: AtomicDEXDomain,
    req: AtomicDEXLoginRequest,
) -> Eip712<AtomicDEXDomain, AtomicDEXLoginRequest> {
    let types = ADEX_TYPES
        .iter()
        .map(|object_type| (object_type.name.clone(), object_type.properties.clone()))
        .collect();
    Eip712 {
        types,
        domain,
        primary_type: ADEX_LOGIN_TYPE.to_string(),
        message: req,
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct AtomicDEXDomain {
    name: String,
}

impl AtomicDEXDomain {
    pub fn new(name: String) -> AtomicDEXDomain {
        AtomicDEXDomain { name }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct AtomicDEXLoginRequest {
    message: String,
}

impl AtomicDEXLoginRequest {
    pub fn new(project_name: String) -> AtomicDEXLoginRequest {
        AtomicDEXLoginRequest {
            message: format!("Login to {project_name}"),
        }
    }
}

fn adex_login_types() -> [ObjectType; 2] {
    let mut domain = ObjectType::domain();
    domain.property("name", PropertyType::String);

    let mut login_request = ObjectType::new(ADEX_LOGIN_TYPE);
    login_request.property("message", PropertyType::String);

    [domain, login_request]
}

mod tests {
    use super::*;
    use mm2_metamask::hash_typed_data;
    use std::str::FromStr;
    use wasm_bindgen_test::*;
    use web3::types::H256;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    async fn test_hash_adex_login_request() {
        const PROJECT: &str = "AtomicDEX";

        let domain = AtomicDEXDomain {
            name: PROJECT.to_string(),
        };
        let request = AtomicDEXLoginRequest::new(PROJECT.to_string());
        let adex_req = adex_eip712_request(domain, request);

        let actual = hash_typed_data(adex_req).unwrap();
        let expected = H256::from_str("0xad90ea2902042ebef413d66f56cfb2b37d313342ec6622ee589eda314fd782d5").unwrap();
        assert_eq!(actual, expected);
    }
}
