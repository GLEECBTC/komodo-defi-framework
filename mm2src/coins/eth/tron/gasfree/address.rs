//! TRON GasFree CREATE2 address derivation.
//!
//! Computes the deterministic GasFree wallet address for a given user EOA
//! using TRON's CREATE2 formula (0x41 prefix instead of Ethereum's 0xff).

use crate::eth::tron::{Network, TronAddress};
use bitcrypto::keccak256;
use ethabi::Token;
use ethereum_types::H160;
use lazy_static::lazy_static;

// Network-specific creation code bytecode constants.
// Source: https://github.com/gasfreeio/gasfree-sdk-js/blob/main/src/constant/common.ts
lazy_static! {
    static ref MAINNET_CREATION_CODE: Vec<u8> = hex::decode("60a06040908082526103e5803803809161001982856101d6565b833981019082818303126101d2576100308161020d565b91602091828101519060018060401b0382116101d2570181601f820112156101d25780519061005e8261022a565b9261006b875194856101d6565b8284528483830101116101d25783905f5b8381106101be5750505f9183010152823b1561017a5780516100b3575b50506080525161013c90816102a982396080518160180152f35b8351635c60da1b60e01b81529082826004816001600160a01b0388165afa918215610170575f9261012d575b50905f80838561011c9695519101845af4903d15610124573d6101018161022a565b9061010e885192836101d6565b81525f81943d92013e610245565b505f80610099565b60609250610245565b90918382813d8311610169575b61014481836101d6565b810103126101665750905f8061015d61011c959461020d565b939450506100df565b80fd5b503d61013a565b85513d5f823e3d90fd5b835162461bcd60e51b815260048101839052601b60248201527f626561636f6e2073686f756c64206265206120636f6e747261637400000000006044820152606490fd5b81810183015185820184015285920161007c565b5f80fd5b601f909101601f19168101906001600160401b038211908210176101f957604052565b634e487b7160e01b5f52604160045260245ffd5b516001600160a81b03811681036101d2576001600160a01b031690565b6001600160401b0381116101f957601f01601f191660200190565b9061026c575080511561025a57805190602001fd5b604051630a12f52160e11b8152600490fd5b8151158061029f575b61027d575090565b604051639996b31560e01b81526001600160a01b039091166004820152602490fd5b50803b1561027556fe60806040819052635c60da1b60e01b81526020816004817f00000000000000000000000000000000000000000000000000000000000000006001600160a01b03165afa9081156100ae575f91610056575b506100e8565b6020903d82116100a6575b601f8201601f1916810167ffffffffffffffff8111828210176100925761008c9350604052016100b9565b5f610050565b634e487b7160e01b84526041600452602484fd5b3d9150610061565b6040513d5f823e3d90fd5b602090607f1901126100e4576080516001600160a81b03811681036100e4576001600160a01b031690565b5f80fd5b5f808092368280378136915af43d82803e15610102573d90f35b3d90fdfea26474726f6e58221220309a2919b7a1b203f1a7a1c544a7d671bb94b0adf8a39e4c9b6eeb6d03939ffe64736f6c63430008140033").expect("invalid mainnet creation code hex");

    static ref NILE_CREATION_CODE: Vec<u8> = hex::decode("60a06040908082526103e5803803809161001982856101d6565b833981019082818303126101d2576100308161020d565b91602091828101519060018060401b0382116101d2570181601f820112156101d25780519061005e8261022a565b9261006b875194856101d6565b8284528483830101116101d25783905f5b8381106101be5750505f9183010152823b1561017a5780516100b3575b50506080525161013c90816102a982396080518160180152f35b8351635c60da1b60e01b81529082826004816001600160a01b0388165afa918215610170575f9261012d575b50905f80838561011c9695519101845af4903d15610124573d6101018161022a565b9061010e885192836101d6565b81525f81943d92013e610245565b505f80610099565b60609250610245565b90918382813d8311610169575b61014481836101d6565b810103126101665750905f8061015d61011c959461020d565b939450506100df565b80fd5b503d61013a565b85513d5f823e3d90fd5b835162461bcd60e51b815260048101839052601b60248201527f626561636f6e2073686f756c64206265206120636f6e747261637400000000006044820152606490fd5b81810183015185820184015285920161007c565b5f80fd5b601f909101601f19168101906001600160401b038211908210176101f957604052565b634e487b7160e01b5f52604160045260245ffd5b516001600160a81b03811681036101d2576001600160a01b031690565b6001600160401b0381116101f957601f01601f191660200190565b9061026c575080511561025a57805190602001fd5b604051630a12f52160e11b8152600490fd5b8151158061029f575b61027d575090565b604051639996b31560e01b81526001600160a01b039091166004820152602490fd5b50803b1561027556fe60806040819052635c60da1b60e01b81526020816004817f00000000000000000000000000000000000000000000000000000000000000006001600160a01b03165afa9081156100ae575f91610056575b506100e8565b6020903d82116100a6575b601f8201601f1916810167ffffffffffffffff8111828210176100925761008c9350604052016100b9565b5f610050565b634e487b7160e01b84526041600452602484fd5b3d9150610061565b6040513d5f823e3d90fd5b602090607f1901126100e4576080516001600160a81b03811681036100e4576001600160a01b031690565b5f80fd5b5f808092368280378136915af43d82803e15610102573d90f35b3d90fdfea26474726f6e5822122019fba3a984dfef08920adc4d0e531dbd369df1dec237bfb02ce668f5d8e2704064736f6c63430008140033").expect("invalid nile creation code hex");

    static ref SHASTA_CREATION_CODE: Vec<u8> = hex::decode("60a06040908082526104b8803803809161001982856102a9565b833981019082818303126102a557610030816102e0565b91602091828101519060018060401b0382116102a5570181601f820112156102a55780519061005e826102fd565b9261006b875194856102a9565b8284528483830101116102a55783905f5b8381106102915750505f9183010152823b15610271577fa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d5080546001600160a01b0319166001600160a01b038581169182179092558551635c60da1b60e01b8082529193928582600481885afa918215610267575f92610230575b50813b156102175750508551837f1cf3b03a6cf19fa2baba4df148e9dcabedea7f8a5c07840e207e5c089be95d3e5f80a28251156101f857508390600487518095819382525afa9182156101ee575f926101ab575b50905f8083856101889695519101845af4903d156101a2573d61016d816102fd565b9061017a885192836102a9565b81525f81943d92013e610318565b505b6080525161013c908161037c82396080518160180152f35b60609250610318565b90918382813d83116101e7575b6101c281836102a9565b810103126101e45750905f806101db61018895946102e0565b9394505061014b565b80fd5b503d6101b8565b85513d5f823e3d90fd5b935050505034610208575061018a565b63b398979f60e01b8152600490fd5b8751634c9c8ce360e01b81529116600482015260249150fd5b90918682813d8311610260575b61024781836102a9565b810103126101e45750610259906102e0565b905f6100f6565b503d61023d565b88513d5f823e3d90fd5b8351631933b43b60e21b81526001600160a01b0384166004820152602490fd5b81810183015185820184015285920161007c565b5f80fd5b601f909101601f19168101906001600160401b038211908210176102cc57604052565b634e487b7160e01b5f52604160045260245ffd5b516001600160a81b03811681036102a5576001600160a01b031690565b6001600160401b0381116102cc57601f01601f191660200190565b9061033f575080511561032d57805190602001fd5b604051630a12f52160e11b8152600490fd5b81511580610372575b610350575090565b604051639996b31560e01b81526001600160a01b039091166004820152602490fd5b50803b1561034856fe60806040819052635c60da1b60e01b81526020816004817f00000000000000000000000000000000000000000000000000000000000000006001600160a01b03165afa9081156100ae575f91610056575b506100e8565b6020903d82116100a6575b601f8201601f1916810167ffffffffffffffff8111828210176100925761008c9350604052016100b9565b5f610050565b634e487b7160e01b84526041600452602484fd5b3d9150610061565b6040513d5f823e3d90fd5b602090607f1901126100e4576080516001600160a81b03811681036100e4576001600160a01b031690565b5f80fd5b5f808092368280378136915af43d82803e15610102573d90f35b3d90fdfea26474726f6e58221220b3a0a0f4043f8fe355d62319dafed2ba5d611d7bb6dfe21d6d935af1510ce27964736f6c63430008140033").expect("invalid shasta creation code hex");
}

/// Per-network GasFree contract artifacts (controller, beacon, creation bytecode).
/// These are protocol constants from the official GasFree SDKs.
struct NetworkArtifacts {
    controller: TronAddress,
    beacon: TronAddress,
    creation_code: &'static [u8],
}

/// Returns the compiled GasFree artifacts for a TRON network.
///
/// The creation_code bytecode is hardcoded from the official GasFree JS SDK
/// (github.com/gasfreeio/gasfree-sdk-js) rather than config-driven, because
/// this bytecode determines user-visible receive addresses — a bad value would
/// silently produce wrong addresses.
fn network_artifacts(network: &Network) -> NetworkArtifacts {
    match network {
        Network::Mainnet => NetworkArtifacts {
            controller: TronAddress::from_base58("TFFAMQLZybALaLb4uxHA9RBE7pxhUAjF3U")
                .expect("hardcoded mainnet controller"),
            beacon: TronAddress::from_base58("TSP9UW6FQhT76XD2jWA6ipGMx3yGbjDffP").expect("hardcoded mainnet beacon"),
            creation_code: &MAINNET_CREATION_CODE,
        },
        Network::Nile => NetworkArtifacts {
            controller: TronAddress::from_base58("THQGuFzL87ZqhxkgqYEryRAd7gqFqL5rdc")
                .expect("hardcoded nile controller"),
            beacon: TronAddress::from_base58("TLtCGmaxH3PbuaF6kbybwteZcHptEdgQGC").expect("hardcoded nile beacon"),
            creation_code: &NILE_CREATION_CODE,
        },
        Network::Shasta => NetworkArtifacts {
            controller: TronAddress::from_base58("TQghdCeVDA6CnuNVTUhfaAyPfTetqZWNpm")
                .expect("hardcoded shasta controller"),
            beacon: TronAddress::from_base58("TQ1jvA3nLDMDNbJoMPLzTPoqAg8NvZ5CCW").expect("hardcoded shasta beacon"),
            creation_code: &SHASTA_CREATION_CODE,
        },
    }
}

/// Returns the hardcoded GasFreeController address for a TRON network.
///
/// Used by `resolve_tron_gasless_provider` to derive the `verifying_contract`
/// stored on `ResolvedTronGaslessProvider`, and internally by CREATE2 derivation.
pub fn controller_for_network(network: &Network) -> TronAddress {
    network_artifacts(network).controller
}

/// Returns the GasFree API path segment for a TRON network.
///
/// Mainnet (`tron`) and Nile (`nile`) come from the public protocol spec's base URLs.
/// Shasta is not listed there, so we mirror the official SDK's network naming and use
/// `shasta` as the prefix to keep host-only config derivation deterministic.
pub fn api_path_segment_for_network(network: &Network) -> &'static str {
    match network {
        Network::Mainnet => "tron",
        Network::Nile => "nile",
        Network::Shasta => "shasta",
    }
}

/// Compute the GasFree address for a user on a specific TRON network.
///
/// Infallible: the controller and beacon are hardcoded per network, and
/// `user_address` is a pre-validated `TronAddress`.
pub fn compute_gasfree_address_for_network(network: &Network, user_address: &TronAddress) -> TronAddress {
    let artifacts = network_artifacts(network);
    compute_gasfree_address(
        &artifacts.controller,
        &artifacts.beacon,
        user_address,
        artifacts.creation_code,
    )
}

/// Pure CREATE2 GasFree address derivation.
///
/// Algorithm (from official GasFree SDK):
/// 1. Salt = user's 20-byte EVM address, left-padded to 32 bytes
/// 2. init_calldata = selector("initialize(address)") || salt
/// 3. init_code = creation_code || abi.encode(beacon_evm, init_calldata)
/// 4. init_code_hash = keccak256(init_code)
/// 5. preimage = 0x41 || controller_20bytes || salt || init_code_hash
/// 6. result = last 20 bytes of keccak256(preimage) → TronAddress
fn compute_gasfree_address(
    controller: &TronAddress,
    beacon: &TronAddress,
    user_address: &TronAddress,
    creation_code: &[u8],
) -> TronAddress {
    // Step 1: Salt = 20-byte EVM address right-aligned in 32 bytes
    let user_evm = user_address.to_evm_address();
    let mut salt = [0u8; 32];
    salt[12..].copy_from_slice(user_evm.as_bytes());

    // Step 2: initialize(address) selector + salt as calldata
    let init_selector = &keccak256(b"initialize(address)")[..4];
    let mut init_calldata = Vec::with_capacity(4 + 32);
    init_calldata.extend_from_slice(init_selector);
    init_calldata.extend_from_slice(&salt);

    // Step 3: ABI-encode constructor args (beacon address, init calldata)
    let beacon_evm = beacon.to_evm_address();
    let encoded_args = ethabi::encode(&[
        Token::Address(H160::from_slice(beacon_evm.as_bytes())),
        Token::Bytes(init_calldata),
    ]);

    // Step 4: init_code = creation_code || encoded_args, then hash
    let mut init_code = Vec::with_capacity(creation_code.len() + encoded_args.len());
    init_code.extend_from_slice(creation_code);
    init_code.extend_from_slice(&encoded_args);
    let init_code_hash = keccak256(&init_code);

    // Step 5: TRON CREATE2 preimage: 0x41 || controller_20 || salt || init_code_hash
    let controller_evm = controller.to_evm_address();
    let mut preimage = Vec::with_capacity(1 + 20 + 32 + 32);
    preimage.push(0x41); // TRON prefix (Ethereum uses 0xff)
    preimage.extend_from_slice(controller_evm.as_bytes());
    preimage.extend_from_slice(&salt);
    preimage.extend_from_slice(&init_code_hash[..]);

    // Step 6: Take last 20 bytes → TRON address
    let hash = keccak256(&preimage);
    let evm_addr = H160::from_slice(&hash[12..]);
    TronAddress::from(evm_addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    // --- Nile test vectors from official JS SDK ---

    #[test]
    fn test_nile_vector_1() {
        let user = TronAddress::from_str("TMVQGm1qAQYVdetCeGRRkTWYYrLXuHK2HC").unwrap();
        let result = compute_gasfree_address_for_network(&Network::Nile, &user);
        assert_eq!(result.to_base58(), "TUGC4eNuEgbaLotwxzzXEck1fRWru6n8ye");
    }

    #[test]
    fn test_nile_vector_2() {
        let user = TronAddress::from_str("TDvSsdrNM5eeXNL3czpa6AxLDHZA9nwe9K").unwrap();
        let result = compute_gasfree_address_for_network(&Network::Nile, &user);
        assert_eq!(result.to_base58(), "TLvVuqx74fMy8QMjEsMT4dWwmVbuNwYt8X");
    }

    #[test]
    fn test_nile_vector_3() {
        let user = TronAddress::from_str("TKTX96CBxr5kvhjsDHcqoiPWZageGxoTW3").unwrap();
        let result = compute_gasfree_address_for_network(&Network::Nile, &user);
        assert_eq!(result.to_base58(), "TTjqEjsitExzYsoDaR65nd3d2avhsXayfL");
    }

    // --- Mainnet test vectors from official JS SDK ---

    #[test]
    fn test_mainnet_vector_1() {
        let user = TronAddress::from_str("TMVQGm1qAQYVdetCeGRRkTWYYrLXuHK2HC").unwrap();
        let result = compute_gasfree_address_for_network(&Network::Mainnet, &user);
        assert_eq!(result.to_base58(), "TBwmA2PtMXC4HiGvfi8xg2jZd5i3y89DjK");
    }

    #[test]
    fn test_mainnet_vector_2() {
        let user = TronAddress::from_str("TDvSsdrNM5eeXNL3czpa6AxLDHZA9nwe9K").unwrap();
        let result = compute_gasfree_address_for_network(&Network::Mainnet, &user);
        assert_eq!(result.to_base58(), "TTA7pGKZdpkJwiwuookcfbkdq6kZxysn86");
    }

    // --- Shasta test vector ---

    #[test]
    fn test_shasta_vector_1() {
        let user = TronAddress::from_str("TMVQGm1qAQYVdetCeGRRkTWYYrLXuHK2HC").unwrap();
        let result = compute_gasfree_address_for_network(&Network::Shasta, &user);
        assert_eq!(result.to_base58(), "TX5TyYzg9a15HksNfTkKbEqGxa7UEPuJE8");
    }

    // --- Expected-controller lookup (used by activation-time validation) ---

    #[test]
    fn test_expected_controller_per_network() {
        assert_eq!(
            controller_for_network(&Network::Mainnet).to_base58(),
            "TFFAMQLZybALaLb4uxHA9RBE7pxhUAjF3U"
        );
        assert_eq!(
            controller_for_network(&Network::Nile).to_base58(),
            "THQGuFzL87ZqhxkgqYEryRAd7gqFqL5rdc"
        );
        assert_eq!(
            controller_for_network(&Network::Shasta).to_base58(),
            "TQghdCeVDA6CnuNVTUhfaAyPfTetqZWNpm"
        );
    }

    #[test]
    fn test_initialize_selector() {
        let selector = &keccak256(b"initialize(address)")[..4];
        assert_eq!(selector, &[0xc4, 0xd6, 0x6d, 0xe8]);
    }
}
