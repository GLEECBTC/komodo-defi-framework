use mm2_err_handle::prelude::*;
use secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use secp256k1::{Message as SecpMessage, Secp256k1};
use web3::types::{H256, H520};

pub use ethkey::{Error, Signature};

/// Inspired by `ethkey::recover` with the only one difference:
/// this methods returns the full `H520` pubkey instead of unprefixed `H512`.
pub fn recover_pubkey(message_hash: H256, mut signature: Signature) -> MmResult<H520, Error> {
    if !(0..3).contains(&signature[64]) {
        if signature[64] < 27 {
            return MmError::err(Error::InvalidSignature);
        }
        // https://github.com/ethereum/go-ethereum/blob/55599ee95d4151a2502465e0afc7c47bd1acba77/internal/ethapi/api.go#L459
        signature[64] -= 27;
    }

    let recovery_id = RecoveryId::from_i32(signature[64] as i32)?;
    let sig = RecoverableSignature::from_compact(&signature[0..64], recovery_id)?;
    let secp_message = SecpMessage::from_slice(message_hash.as_ref())?;
    let pubkey = Secp256k1::new().recover_ecdsa(&secp_message, &sig)?;
    let serialized = pubkey.serialize_uncompressed();

    Ok(H520::from_slice(&serialized))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// https://testnet.bscscan.com/tx/0xbff2c07134a32da9f96588c03502f1caf2948c529c2ce9ce067f320f7c990b81
    #[test]
    fn test_recover_pubkey_for_signed_tx() {
        let msg_hash = H256::from_str("ac3a8b78342416f75a9c46252e67eb1aa41b65cd95155273e719cb39d4fd2c62").unwrap();
        let sig = Signature::from_str("e13785abae3c72a168a95c6dfdd7bdcb1dc7db50d831bdad3b746e4a56f1e4e92a3aa057332cb8aa2ef2edb58234f3eb96d423b701f77cfbf6f5aac93b8824ae01").unwrap();

        let actual_pubkey = recover_pubkey(msg_hash, sig).unwrap();
        let expected_pubkey = H520::from_str("04d5f11f3cf90d165af39b17caec89162c607ddfc2d64d4eba2058c2eb8c2347cc422eaf112cb01a662f5f29924e2a6322153ae05d4e73526cb83cc1759c09fc01").unwrap();
        assert_eq!(actual_pubkey, expected_pubkey);
    }

    #[test]
    fn test_recover_pubkey_for_signed_signed_adex_login() {
        let msg_hash = H256::from_str("dbb6305acd7a0e6c91de2c2f3da96a3ff069b40eba6d796031c72d5c15a10308").unwrap();
        let sig = Signature::from_str("c6714ed4f85a75dd13c87a39a1ff4b33f9f42f8de159df261ade6469f316a60c2c098ce75ef91e794a4807caa0185ea1286d53df2bfb0c4a7e612715bf6b50af1b").unwrap();

        let actual_pubkey = recover_pubkey(msg_hash, sig).unwrap();
        let expected_pubkey = H520::from_str("04d5f11f3cf90d165af39b17caec89162c607ddfc2d64d4eba2058c2eb8c2347cc422eaf112cb01a662f5f29924e2a6322153ae05d4e73526cb83cc1759c09fc01").unwrap();
        assert_eq!(actual_pubkey, expected_pubkey);
    }
}
