use coins::mintlayer::sdk_private_key_from_kdf_key_pair;
use crypto::privkey::key_pair_from_secret;
use mintlayer_sdk::crypto::types::Encode;
use mintlayer_sdk::crypto::{
    pubkey_to_pubkeyhash_address, public_key_from_private_key, sign_challenge, verify_challenge, Network,
};

#[test]
fn kdf_key_adapter_preserves_public_key_address_and_signatures() {
    let kdf_key_pair = key_pair_from_secret(&[7_u8; 32]).expect("valid deterministic KDF key");
    let sdk_private_key =
        sdk_private_key_from_kdf_key_pair(&kdf_key_pair).expect("KDF key converts to SDK private key");
    let sdk_public_key = public_key_from_private_key(&sdk_private_key);
    let sdk_public_key_scale = sdk_public_key.encode();

    assert_eq!(sdk_public_key_scale[0], 0);
    assert_eq!(&sdk_public_key_scale[1..], kdf_key_pair.public_slice());

    let address = pubkey_to_pubkeyhash_address(&sdk_public_key, Network::Mainnet);
    let challenge = b"Pinco Labs KDF-Mintlayer signing adapter offline test";
    let signature = sign_challenge(&sdk_private_key, challenge).expect("challenge signing succeeds");

    assert!(
        verify_challenge(&address, Network::Mainnet, &signature, challenge).expect("challenge verification succeeds")
    );

    println!("address:    {address}");
    println!("public key: KDF and SDK identical");
    println!("signature:  verified offline");
}
