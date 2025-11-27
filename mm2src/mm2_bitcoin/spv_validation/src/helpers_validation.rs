use crate::conf::{SPVBlockHeader, SPVConf};
use crate::storage::{BlockHeaderStorageError, BlockHeaderStorageOps};
use crate::work::{next_block_bits, NextBlockBitsError};
use chain::{BlockHeader, RawHeaderError};
use derive_more::Display;
use primitives::hash::H256;
use serialization::parse_compact_int;
use sha2::digest::Digest;
use sha2::Sha256;

#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum SPVError {
    #[display(fmt = "Error validating initial spv parameters: {_0}")]
    InitialValidationError(String),
    #[display(fmt = "Overran a checked read on a slice")]
    ReadOverrun,
    #[display(fmt = "Attempted to parse a CompactInt without enough bytes")]
    BadCompactInt,
    #[display(fmt = "`extract_hash` could not identify the output type")]
    MalformattedOutput,
    #[display(fmt = "Unable to get block header from network or storage: {_0}")]
    UnableToGetHeader(String),
    #[display(fmt = "Header not exactly 80 bytes")]
    WrongLengthHeader,
    #[display(fmt = "Header chain changed difficulties unexpectedly")]
    UnexpectedDifficultyChange,
    #[display(fmt = "Header does not meet its own difficulty target")]
    InsufficientWork,
    #[display(fmt = "Couldn't calculate the required difficulty for the block: {_0}")]
    DifficultyCalculationError(NextBlockBitsError),
    #[display(fmt = "When validating a `BitcoinHeader`, the `hash` field is not the digest of the raw header")]
    WrongDigest,
    #[display(
        fmt = "When validating a `BitcoinHeader`, the `merkle_root` field does not match the root found in the raw header"
    )]
    WrongMerkleRoot,
    #[display(
        fmt = "When validating a `BitcoinHeader`, the `prevhash` field does not match the parent hash found in the raw header"
    )]
    WrongPrevHash,
    #[display(fmt = "A `vin` (transaction input vector) is malformatted")]
    InvalidVin,
    #[display(fmt = "A `vout` (transaction output vector) is malformatted or empty")]
    InvalidVout,
    #[display(fmt = "merkle proof connecting the `tx_id_le` to the `confirming_header`")]
    BadMerkleProof,
    #[display(fmt = "Unable to get merkle tree from network or storage for {coin}: {err}")]
    UnableToGetMerkle { coin: String, err: String },
    #[display(fmt = "Unable to retrieve block height / block height is zero: {_0}")]
    InvalidHeight(String),
    #[display(fmt = "Raises during validation loop")]
    Timeout,
    #[display(fmt = "Block headers storage error: {_0}")]
    HeaderStorageError(BlockHeaderStorageError),
    #[display(
        fmt = "Wrong retarget block header height in config for: {coin} expected block header height :
        {expected_height}."
    )]
    WrongRetargetHeight { coin: String, expected_height: u64 },
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
    #[display(fmt = "Parent Hash Mismatch - coin:{coin} - mismatched_block_height:{mismatched_block_height}")]
    ParentHashMismatch { coin: String, mismatched_block_height: u64 },
}

impl From<RawHeaderError> for SPVError {
    fn from(e: RawHeaderError) -> Self {
        match e {
            RawHeaderError::WrongLengthHeader { .. } => SPVError::WrongLengthHeader,
        }
    }
}

impl From<NextBlockBitsError> for SPVError {
    fn from(e: NextBlockBitsError) -> Self {
        SPVError::DifficultyCalculationError(e)
    }
}

impl From<BlockHeaderStorageError> for SPVError {
    fn from(e: BlockHeaderStorageError) -> Self {
        SPVError::HeaderStorageError(e)
    }
}

/// A slice of `H256`s for use in a merkle array
#[derive(Debug, Clone, PartialEq, Eq)]
struct MerkleArray<'a>(&'a [u8]);

impl<'a> MerkleArray<'a> {
    /// Return a new merkle array from a slice
    pub fn new(slice: &'a [u8]) -> Result<MerkleArray<'a>, SPVError> {
        if slice.len().is_multiple_of(32) {
            Ok(Self(slice))
        } else {
            Err(SPVError::BadMerkleProof)
        }
    }
}

impl MerkleArray<'_> {
    /// The length of the underlying slice
    fn len(&self) -> usize {
        self.0.len() / 32
    }

    /// Index into the merkle array
    fn index(&self, index: usize) -> Result<H256, SPVError> {
        let to_index = (index + 1) * 32;
        if self.0.len() < to_index {
            return Err(SPVError::BadMerkleProof);
        }
        let mut digest = H256::default();
        digest.as_mut().copy_from_slice(&self.0[index * 32..to_index]);
        Ok(digest)
    }
}

/// Determines the length of an input from its scriptsig:
/// 36 for outpoint, 1 for scriptsig length, 4 for sequence.
///
/// # Arguments
///
/// * `tx_in` - The input as a u8 array
fn determine_input_length(tx_in: &[u8]) -> Result<usize, SPVError> {
    if tx_in.len() < 37 {
        return Err(SPVError::ReadOverrun);
    }
    let script_sig_len = parse_compact_int(&tx_in[36..]).map_err(|_| SPVError::BadCompactInt)?;
    // 40 = 36 (outpoint) + 4 (sequence)
    Ok(40 + script_sig_len.serialized_length() + script_sig_len.as_usize())
}

//
// Outputs
//

/// Determines the length of an output.
/// 5 types: WPKH, WSH, PKH, SH, and OP_RETURN.
///
/// # Arguments
///
/// * `tx_out` - The output
///
/// # Errors
///
/// * Errors if CompactInt represents a number larger than 253; large CompactInts are not supported.
fn determine_output_length(tx_out: &[u8]) -> Result<usize, SPVError> {
    if tx_out.len() < 9 {
        return Err(SPVError::MalformattedOutput);
    }

    let script_pubkey_len = parse_compact_int(&tx_out[8..]).map_err(|_| SPVError::BadCompactInt)?;

    Ok(8 + script_pubkey_len.serialized_length() + script_pubkey_len.as_usize())
}

//
// Transaction
//

/// Checks that the vin passed up is properly formatted;
/// Consider a vin with a valid vout in its scriptsig.
///
/// # Arguments
///
/// * `vin` - Raw bytes length-prefixed input vector
pub(crate) fn validate_vin(vin: &[u8]) -> bool {
    let n_ins = match parse_compact_int(vin) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let vin_length = vin.len();

    let mut offset = n_ins.serialized_length();
    if n_ins.as_usize() == 0usize {
        return false;
    }

    for _ in 0..n_ins.as_usize() {
        if offset >= vin_length {
            return false;
        }
        match determine_input_length(&vin[offset..]) {
            Ok(v) => offset += v,
            Err(_) => return false,
        };
    }

    offset == vin_length
}

/// Checks that the vout passed up is properly formatted;
/// Consider a vin with a valid vout in its scriptsig.
///
/// # Arguments
///
/// * `vout` - Raw bytes length-prefixed output vector
pub(crate) fn validate_vout(vout: &[u8]) -> bool {
    let n_outs = match parse_compact_int(vout) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let vout_length = vout.len();

    let mut offset = n_outs.serialized_length();
    if n_outs.as_usize() == 0usize {
        return false;
    }

    for _ in 0..n_outs.as_usize() {
        if offset >= vout_length {
            return false;
        }
        match determine_output_length(&vout[offset..]) {
            Ok(v) => offset += v,
            Err(_) => return false,
        };
    }

    offset == vout_length
}

/// Implements bitcoin's hash256 (double sha2).
/// Returns the digest.
///
/// # Arguments
///
/// * `preimage` - The pre-image
fn hash256(preimages: &[&[u8]]) -> H256 {
    let mut sha = Sha256::new();
    for preimage in preimages.iter() {
        sha.update(preimage);
    }
    let digest = sha.finalize();

    let mut second_sha = Sha256::new();
    second_sha.update(digest);
    let buf: [u8; 32] = second_sha.finalize().into();
    buf.into()
}

/// Concatenates and hashes two inputs for merkle proving.
///
/// # Arguments
///
/// * `a` - The first hash
/// * `b` - The second hash
fn hash256_merkle_step(a: &[u8], b: &[u8]) -> H256 {
    hash256(&[a, b])
}

/// Verifies a Bitcoin-style merkle tree.
/// Leaves are 0-indexed.
/// Note that `index` is not a reliable indicator of location within a block.
///
/// # Arguments
///
/// * `proof` - The proof. Tightly packed LE sha256 hashes.  The last hash is the root
/// * `index` - The index of the leaf
fn verify_hash256_merkle(
    txid: H256,
    merkle_root: H256,
    intermediate_nodes: &MerkleArray,
    index: u64,
) -> Result<(), SPVError> {
    let mut idx = index;
    let proof_len = intermediate_nodes.len();

    if (proof_len == 0 && txid == merkle_root) || proof_len == 1 {
        return Ok(());
    }

    let mut current = txid;

    for i in 0..proof_len {
        let next = intermediate_nodes.index(i)?;

        if idx % 2 == 1 {
            current = hash256_merkle_step(next.as_slice(), current.as_slice());
        } else {
            current = hash256_merkle_step(current.as_slice(), next.as_slice());
        }
        idx >>= 1;
    }

    if current != merkle_root {
        return Err(SPVError::BadMerkleProof);
    }

    Ok(())
}

/// Evaluates a Bitcoin merkle inclusion proof.
/// Note that `index` is not a reliable indicator of location within a block.
///
/// # Arguments
///
/// * `txid` - The txid (LE)
/// * `merkle_root` - The merkle root (as in the block header) (LE)
/// * `intermediate_nodes` - The proof's intermediate nodes (digests between leaf and root) (LE)
/// * `index` - The leaf's index in the tree (0-indexed)
///
/// # Notes
/// Wrapper around `bitcoin_spv::validatespv::prove`
pub(crate) fn merkle_prove(
    txid: H256,
    merkle_root: H256,
    intermediate_nodes: Vec<H256>,
    index: u64,
) -> Result<(), SPVError> {
    if txid == merkle_root && index == 0 && intermediate_nodes.is_empty() {
        return Ok(());
    }
    let vec: Vec<u8> = intermediate_nodes.into_iter().flat_map(|node| node.take()).collect();
    let nodes = MerkleArray::new(vec.as_slice())?;
    verify_hash256_merkle(txid.take().into(), merkle_root.take().into(), &nodes, index)
}

fn validate_header_prev_hash(actual: &H256, to_compare_with: &H256) -> bool {
    actual == to_compare_with
}

/// Checks validity of header chain.
/// Compares the hash of each header to the prevHash in the next header.
///
/// # Arguments
///
/// * `headers` - Raw byte array of header chain
/// * `difficulty_check`: Rather the difficulty need to check or not, usefull for chain like Qtum (Pos)
///   or KMD/SmartChain (Difficulty change NN)
/// * `constant_difficulty`: If we do not expect difficulty change (BTC difficulty change every 2016 blocks)
///   use this variable to false when you do not have a chance to use a checkpoint
///
/// # Errors
///
/// * Errors if header chain is invalid, insufficient work, unexpected difficulty change or unable to get a target
///
/// # Notes
/// Wrapper inspired by `bitcoin_spv::validatespv::validate_header_chain`
pub async fn validate_headers(
    coin: &str,
    last_validated_height: u64,
    headers_to_validate: &[BlockHeader],
    storage: &dyn BlockHeaderStorageOps,
    conf: &SPVConf,
) -> Result<(), SPVError> {
    let mut last_validated_header = if last_validated_height == conf.starting_block_header.height {
        conf.starting_block_header.clone()
    } else {
        let header = storage.get_block_header(last_validated_height).await?.ok_or(
            BlockHeaderStorageError::GetFromStorageError {
                coin: coin.to_string(),
                reason: format!("Header with height {last_validated_height} is not found in storage"),
            },
        )?;
        SPVBlockHeader::from_block_header_and_height(&header, last_validated_height)
    };
    let mut last_validated_height = last_validated_height;
    let mut last_validated_hash = last_validated_header.hash;
    let mut last_validated_bits = last_validated_header.bits.clone();

    for header_to_validate in headers_to_validate.iter() {
        if !validate_header_prev_hash(&header_to_validate.previous_header_hash, &last_validated_hash) {
            // Detect for chain reorganization and return the last header(previous_height + 1).
            return Err(SPVError::ParentHashMismatch {
                coin: coin.to_string(),
                mismatched_block_height: last_validated_height + 1,
            });
        }

        let block_bits_to_validate = header_to_validate.bits.clone();
        if let Some(params) = &conf.validation_params {
            if params.constant_difficulty && params.difficulty_check && block_bits_to_validate != last_validated_bits {
                return Err(SPVError::UnexpectedDifficultyChange);
            }

            if let Some(algorithm) = &params.difficulty_algorithm {
                let next_block_bits = next_block_bits(
                    coin,
                    header_to_validate.time,
                    last_validated_header.clone(),
                    storage,
                    algorithm,
                )
                .await?;

                if !params.constant_difficulty && params.difficulty_check && block_bits_to_validate != next_block_bits {
                    return Err(SPVError::InsufficientWork);
                }
            }
        }

        last_validated_bits = block_bits_to_validate;
        last_validated_height += 1;
        last_validated_header = SPVBlockHeader::from_block_header_and_height(header_to_validate, last_validated_height);
        last_validated_hash = last_validated_header.hash
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::conf::{BlockHeaderValidationParams, SPVBlockHeader};
    use crate::test_utils::{self};
    use crate::work::tests::TestBlockHeadersStorage;
    use crate::work::DifficultyAlgorithm;
    use chain::BlockHeaderBits;
    use common::block_on;
    use std::{println, vec};
    use test_helpers::hex::force_deserialize_hex;

    #[test]
    fn it_does_bitcoin_hash256() {
        test_utils::run_test(|fixtures| {
            let test_cases = test_utils::get_test_cases("hash256", fixtures);
            for case in test_cases {
                let input = force_deserialize_hex(case.input.as_str().unwrap());
                let mut expected = H256::default();
                let output = force_deserialize_hex(case.output.as_str().unwrap());
                expected.as_mut().copy_from_slice(&output);
                assert_eq!(hash256(&[&input]), expected);
            }
        })
    }

    #[test]
    fn it_computes_hash256_merkle_steps() {
        test_utils::run_test(|fixtures| {
            let test_cases = test_utils::get_test_cases("hash256MerkleStep", fixtures);
            for case in test_cases {
                let inputs = case.input.as_array().unwrap();
                let a = force_deserialize_hex(inputs[0].as_str().unwrap());
                let b = force_deserialize_hex(inputs[1].as_str().unwrap());
                let mut expected = H256::default();
                let output = force_deserialize_hex(case.output.as_str().unwrap());
                expected.as_mut().copy_from_slice(&output);
                assert_eq!(hash256_merkle_step(&a, &b), expected);
            }
        })
    }

    #[test]
    fn it_determines_input_length() {
        test_utils::run_test(|fixtures| {
            let test_cases = test_utils::get_test_cases("determineInputLength", fixtures);
            for case in test_cases {
                let input = force_deserialize_hex(case.input.as_str().unwrap());
                let expected = case.output.as_u64().unwrap() as usize;
                assert_eq!(determine_input_length(&input).unwrap(), expected);
            }
        })
    }

    #[test]
    fn it_determines_output_length() {
        test_utils::run_test(|fixtures| {
            let test_cases = test_utils::get_test_cases("determineOutputLength", fixtures);
            for case in test_cases {
                let input = force_deserialize_hex(case.input.as_str().unwrap());
                let expected = case.output.as_u64().unwrap() as usize;
                assert_eq!(determine_output_length(&input).unwrap(), expected);
            }
        })
    }

    #[test]
    fn it_validates_vin_syntax() {
        test_utils::run_test(|fixtures| {
            let test_cases = test_utils::get_test_cases("validateVin", fixtures);
            for case in test_cases {
                let input = force_deserialize_hex(case.input.as_str().unwrap());
                let expected = case.output.as_bool().unwrap();
                assert_eq!(validate_vin(&input), expected);
            }
        })
    }

    #[test]
    fn it_validates_vout_syntax() {
        test_utils::run_test(|fixtures| {
            let test_cases = test_utils::get_test_cases("validateVout", fixtures);
            for case in test_cases {
                let input = force_deserialize_hex(case.input.as_str().unwrap());
                let expected = case.output.as_bool().unwrap();
                assert_eq!(validate_vout(&input), expected);
            }
        })
    }

    #[test]
    fn it_verifies_hash256_merkles() {
        test_utils::run_test(|fixtures| {
            let test_cases = test_utils::get_test_cases("verifyHash256Merkle", fixtures);
            for case in test_cases {
                let inputs = case.input.as_object().unwrap();
                let extended_proof = force_deserialize_hex(inputs.get("proof").unwrap().as_str().unwrap());
                let proof_len = extended_proof.len();
                if proof_len < 32 {
                    continue;
                }

                let index = inputs.get("index").unwrap().as_u64().unwrap();
                let expected = case
                    .output
                    .as_bool()
                    .unwrap()
                    .then_some(())
                    .ok_or(SPVError::BadMerkleProof);

                // extract root and txid
                let mut root = H256::default();
                let mut txid = H256::default();
                println!("{extended_proof:?}");
                root.as_mut().copy_from_slice(&extended_proof[proof_len - 32..]);
                txid.as_mut().copy_from_slice(&extended_proof[..32]);

                let proof = if proof_len > 64 {
                    extended_proof[32..proof_len - 32].to_vec()
                } else {
                    vec![]
                };

                println!("{:?} {:?} {:?} {:?}", root, txid, proof, proof.len());

                assert_eq!(
                    verify_hash256_merkle(txid, root, &MerkleArray::new(&proof).unwrap(), index),
                    expected
                );
            }
        })
    }

    #[test]
    fn test_merkle_prove_inclusion() {
        // https://rick.explorer.dexstats.info/tx/7e9797a05abafbc1542449766ef9a41838ebbf6d24cd3223d361aa07c51981df
        // merkle intermediate nodes 2 element
        let tx_id: H256 = H256::from_reversed_str("7e9797a05abafbc1542449766ef9a41838ebbf6d24cd3223d361aa07c51981df");
        let merkle_pos = 1;
        let merkle_root: H256 =
            H256::from_reversed_str("41f138275d13690e3c5d735e2f88eb6f1aaade1207eb09fa27a65b40711f3ae0");
        let merkle_nodes: Vec<H256> = vec![
            H256::from_reversed_str("73dfb53e6f49854b09d98500d4899d5c4e703c4fa3a2ddadc2cd7f12b72d4182"),
            H256::from_reversed_str("4274d707b2308d39a04f2940024d382fa80d994152a50d4258f5a7feead2a563"),
        ];
        let result = merkle_prove(tx_id, merkle_root, merkle_nodes, merkle_pos);
        result.unwrap()
    }

    #[test]
    fn test_merkle_prove_inclusion_single_element() {
        // https://www.blockchain.com/btc/tx/c06fbab289f723c6261d3030ddb6be121f7d2508d77862bb1e484f5cd7f92b25
        // merkle intermediate nodes single element
        let tx_id: H256 = H256::from_reversed_str("c06fbab289f723c6261d3030ddb6be121f7d2508d77862bb1e484f5cd7f92b25");
        let merkle_pos = 0;
        let merkle_root: H256 =
            H256::from_reversed_str("8fb300e3fdb6f30a4c67233b997f99fdd518b968b9a3fd65857bfe78b2600719");
        let merkle_nodes: Vec<H256> = vec![H256::from_reversed_str(
            "5a4ebf66822b0b2d56bd9dc64ece0bc38ee7844a23ff1d7320a88c5fdb2ad3e2",
        )];
        let result = merkle_prove(tx_id, merkle_root, merkle_nodes, merkle_pos);
        result.unwrap()
    }

    #[test]
    fn test_merkle_prove_inclusion_complex() {
        // https://www.blockchain.com/btc/tx/b36bced99cc459506ad2b3af6990920b12f6dc84f9c7ed0dd2c3703f94a4b692
        // merkle intermediate nodes complex merkle proof inclusion
        let tx_id: H256 = H256::from_reversed_str("b36bced99cc459506ad2b3af6990920b12f6dc84f9c7ed0dd2c3703f94a4b692");
        let merkle_pos = 680;
        let merkle_root: H256 =
            H256::from_reversed_str("def7a26d91789069dad448cb4b68658b7ba419f9fbd28dce7fe32ed0010e55df");
        let merkle_nodes: Vec<H256> = vec![
            H256::from_reversed_str("39141331f2b7133e72913460384927b421ffdef3e24b88521e7ac54d30019409"),
            H256::from_reversed_str("39aeb77571ee0b0cf9feb7e121938b862f3994ff1254b34559378f6f2ed8b1fb"),
            H256::from_reversed_str("5815f83f4eb2423c708127ea1f47feeabcf005d4aed18701d9692925f152d0b4"),
            H256::from_reversed_str("efbb90aae6875af1b05a17e53fabe79ca1655329d6e107269a190739bf9d9038"),
            H256::from_reversed_str("20eb7431ae5a185e89bd2ad89956fc660392ee9d231df58600ac675734013e82"),
            H256::from_reversed_str("1f1dd980e6196ec4de9037941076a6030debe466dfc177e54447171b64ea99e5"),
            H256::from_reversed_str("bbc4264359bec656298e31443034fc3ff9877752b765b9665b4da1eb8a32d1ff"),
            H256::from_reversed_str("71788bf5224f228f390243a2664d41d96bae97ae1e4cfbc39095448e4cd1addd"),
            H256::from_reversed_str("1b24a907c86e59eb698afeb4303c00fe3ecf8425270134ed3d0e62c6991621f2"),
            H256::from_reversed_str("7776b46bb148c573d5eabe1436a428f3dae484557fea6efef1da901009ca5f8f"),
            H256::from_reversed_str("623a90d6122a233b265aab497b13bb64b5d354d2e2112c3f554e51bfa4e6bbd3"),
            H256::from_reversed_str("3104295d99163e16405b80321238a97d02e2448bb634017e2e027281cc4af9e8"),
        ];
        let result = merkle_prove(tx_id, merkle_root, merkle_nodes, merkle_pos);
        result.unwrap()
    }

    #[test]
    fn test_block_headers_no_difficulty_check() {
        // morty: 1330481, 1330482
        let headers: Vec<BlockHeader> = vec![
            "04000000001f22e1bc88c53b1554f8fdcf261fdb09f4cae6ef5e5032b788515f4a60d30d67d1b35fda68abc05f5af39e5ade224a5312b8dcd1f3629a7ff33355bb7ca93e32d719d14c15e565c05e84ead95a2f101a1b658ee2f36eb7ca65206e27cfca478be6146220bb071f49000b055b22a7a4bbafd6b52efb90f963d5f80126c27e437005fb47720e0000fd4005004d9875d71c540f558813142e263f597243bdd8d8105ff3d1ffd62ae51ccf22729debe510f97ab0631701dbd34b73e570597dc8825be6bd669e693037fb701040c273b44745f4e850c2d8aeca7ccab6ef7f462206a16d75358f2e8fddf9d0dbc6333ff55b1813a37f0ba240bd2d897fbd6cfdb1989ac8f3ec93b15ae4360edf84088ac9a4ea7d3d71290532bb51675e7310be1210aa33c184d693f6f7c15c5be1e89356ae3d663d0c548fceac0974fe4cb6c6559f50643280df9508460fd04f9cde55521b4c6d61c644c6c7b7473f9e39b412e3776f5e47b6c466aaf1dc76ff2114e716eb6b9614d0c93cdc229ec13b07057a7f7446c1aac51ef0950d4361fa2d20f22f29ff490bf6d6a2a267c45d88d3152d9f5291695f2f4fba65ca9763cb4176506c73b8162611b6004af7ec8d1ea55a225cca2576e4ac84ac333b663693a2f19f7786340ad9d2212d576a0b4e7700bd7d60de88940dce1f01481f9c41350eefd7b496218bcf70c4c8922dfd18d666d37d10cb0f14dd38e1225ec179dcab5501a4434674d6f9ff9f23c4df5f445cc2accf43189fc99ac56693df373a4207b0dc991009fae4796fd7e49cea4dd139ee72264dfd47f4e1ad2420d635c7a1f37950d022ffdcccc7651b645db0ba0ce94c18dcc902279b4601806beefe05016f1f85411e6562b584da0854db2e36f602d8c4974d385aee4a01d1132082c8cd7c71443162f7d7487c73d8a46f830f72a0d352d957bef5afc33c4447ef33b2491e28000d1f4687e95ffc2b9532d28ae4c48f8551bf527dbe18c672204495f2bd546566fd5770189e28c2de0974130a492ccd8737a8c6e971d02a23c4f9f27410348d1f666f93385bdc81bad8e9a9d1dbffdfa2609ebae52740b457ecd67a3bf0db02a14f5bdf3e25b35b2d3d303094e46e0e3daef559d9f0e074e512bcaf9fcc9d035083eec16806af8a93d27b4ad46754a425b6a02b1ac22f682e48f214d66b379d7042aa39f2c5f3448d05ca4b6360e162f31f197225f4ad579d69207c666711fb3f6ca814efcf430899360cced1168cd69ec0e809a89cf2cf2015f9f895a3dadd4ced6d94793e98201b1da6a0a5d90be5d06925e3ad60b9227f84b9c3060a6db6e7857d8731f975d4a993abf10d84590da02b114625109d864de070813179b651d528f66036c30a0700ee84fc5e59757a509745b64e76fa3396f3c8b01a7724cd434e6d774dad36be8a73ad29f6859352aa15236e7825947396cb98e26b912b19ddc127590e59200c4334d1d96d7585a0e349b920f2e4e59cdedac911214c42c0894f72c8a7423d7aef3ea5ef9a5b650821f46537c65509ad8dcf6558c16c04f9877c737ff81875d9fbe01d23d37e937444cf257b0b57bc1c2a774f2e2bf5f3b0881be0e2282ba97ef6aad797f8fdb4053da4e478575805c7a93076c09847544a8e89f1cb3838df7870bcf61deb2144c6f6349c966b67545703058f9227965b97835b049538fb428431a8461586b022368626d20e9b6bfdd7232a5cc6a0aa214319cb440c45443a2446d1e17713c0e1049f0fd759d1dbff493302140376cfb153330ed455a043189260cb7d2d90333a37d3584f2d907d0a73dccee299ad14141d60d1409cda688464a13b5dab37476641741717d599a60c0ac84d85869ed449f83933ad30e2591157fd1f07b73ecf26f34e91bc00f1ca86ae34ca8231b372cdc2ed18d463ac42f92859d6f0e2c483dbb23d785f1233db2033458af9d7c1e7029ac5cc33ca7d25b2b49fd71b1ae5f5ce969b6e77333bf5fbb5e6645dd0a4d0c6e82eb534ac264ddbe28513e4b82b3578c1a6cbfaa2522aa50985fe2cce43cf3363eaacca0e09c721fd603d43c3a4fdf8dde0c9ff2c054910b16aeef7c4d86b31".into(),
            "04000000fcead9a1b425124f11aa97e0614120ce87bdddcad655672916f9c4564dc057002bd3df07a4602620282b276359529114ba89b59b16bec235d584c3cf5cc6b2d132d719d14c15e565c05e84ead95a2f101a1b658ee2f36eb7ca65206e27cfca47bfe61462d5b9071f1a001daf299c51afbd74fd75a98ba49a6e40ae8ad92b3afdc1cf215fd6190000fd40050044b5e035b02d138a9704f9513c0865f2733b7c09294ee504c155c283f4895559b6ac39828eac98ad393a642330589e8849040f55ce44f8f2197529d0b0ed57ccdda41f1971e153ec28ac5b4eba968741db374104d65ee234580a83bea1c0cdb67b8bc207057486eb1d90e21ba0cd4f5e9fd834821fafc1517c5d1fceb50ba6f6b102a9b4edac46f2359aec795a4e2458f51114a41289634b3b1cf250e3e38f3689f951278dfa7202a7dfe311cc098fd4a8d02c8f8a74e4a5010b18ee2e60578d5e9f1c094433a73f26e6546e20a574fc261baaa79e9910ab86ed607786a1cc88e7de51ff928d434e26eaef1437f7068c743f26d7c0eea6791e869b101fee8ab41b50af6174c5e6b731a1719f31ee3e6529efef49f31665baedc9382e9665278a84467d479f139fc7a8ef66fef9bd2fd17f7779ee315d458f691a290fa7c2179de8bb91a78458c5290d4aa45b163254006800ba2fce7479511f744fd7de96495c39be93413d8b0b187fe092537e1a7646a66a125b33333f6ecd10085e23ad168b24ee7be69d01ea021a39401e4bd41d818499e7174dd9b85542076c78cb89eeec1c190301b4709dbc963d47926e31bb0235ba6a7029d49458150f6491ac9c973b8a2c893258f907baf4bcb7c39f12b900ba2b2382cd5dd84314ee504ade835ad9a1cb13a7f5928a483ebc9415429810fd99893f2f8f83970b8b47143d617e6f9853e4d86ff378be664218f1c32531143e209f171590dd48216fec879a6b9cbf04432bf4f1a3734b69b6a9f1a358a259a0f9082cfb6c1f3d9d2d9e4522ad651ccce565f06b30c1c0b27252270c2f6608cf4f3288a7e7d4b174e646de05341f7db62b00b5ccb295f058d34b87201148828e9b3f7e08f60e100f810be27eb7f4c471cda7621106fe78bc69ec2bd27acabd55dc094b8626913b7d24d9b60939754700f32574a733a195f8b0220d56f6797de0bcd7b80d561896b816586593409f76e85a7a1035f821dee32a02fdbc26bc4cca375bed418b9d678ac589249a1a5a5b24447ee9b42e33f817066caf3d4e17d0347f6acf0cbf426d4df49413b3d12350edec2681ab9cfecd0825ccfb2649a57391d3f153050dfb4350d60e5e464229ddd6e49ece95557b8ef48c18cbffbe9fc8d7700f611a4b33a2a254afcec638c485e36daf0364da7d4302e488db7b6c41297571048cfea5452e324abb9f9e1043e625fd0853b7e03063d1c3a43aa1ee62d45d890b5e4d10640e775cff6852b6d1acd4a503b3ece3b319cbcf33ff9fdf17b8f852d748db1e05af80507f5d0e1bc44444b155d7da20f7f0b4d6d83368c3bb9e1321b39472a8677ea1d3aca43b453d35edca37b7536d19c26b764958b3c7c30f3211d7b7bb7f6a6d7fd7bf2dda6e7d7b1e533556863549bbe1394a3828596f25029b7e30495e1235f084e5edd133bc29fce4f1e5e514eb1d1cb19fd8dfbb0d130fbec4e288f23dae86311ffd6f4afbaacc2ffe1cc8811a455ba6f5659f82515b56c6ac84277bff5bef98fefc74e002e4a11866a417a429541f8a62df4108e4730d3045f92984bcf1ab2f7d03f8bb1767e91791530cd8eec412919e1f2e341e66a1588a8f485f7aa005787af946b9cb10f6685420b7e1663f66374fddc5e70720507ee2134f3b02df042fcf6db4a5bdd74cc5010793634816fe447cc68e076b225cc1ca872929ef246ce356dc8d8964ff6d7119d071eccb6dc37f75b932c44cdc30723b8357a2761c6de6ab2713e6f6a782538cb731b07950d3f459760a00cc0af406d6848014746b02653636f479d952b46fdeff976e1d159ba46ae7363d5b0042d3905a0bda12aaa6eaae1a5a0d55d4c1930aa1c004cd610866853a247239366aa20f8968ea9ca3d5d6d7321a5d0f2c".into()
        ];
        let params = BlockHeaderValidationParams {
            difficulty_check: false,
            constant_difficulty: false,
            difficulty_algorithm: None,
        };
        let conf = SPVConf {
            starting_block_header: SPVBlockHeader {
                height: 0,
                hash: "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f".into(),
                time: 486604799,
                bits: BlockHeaderBits::Compact(1231006505.into()),
            },
            max_stored_block_headers: None,
            validation_params: Some(params),
        };
        block_on(validate_headers(
            "MORTY",
            1330480,
            &headers,
            &TestBlockHeadersStorage { ticker: "MORTY".into() },
            &conf,
        ))
        .unwrap()
    }

    #[test]
    fn test_block_headers_difficulty_check() {
        // BTC: 724609, 724610, 724611
        let headers: Vec<BlockHeader> = vec![
            "00200020eab6fa183da8f9e4c761b31a67a76fa6a7658eb84c760200000000000000000063cd9585d434ec0db25894ec4b1f03735f10e31709c4395ea67c50c8378f134b972f166278100a17bfd87203".into(),
            "0000402045c698413fbe8b5bf10635658d2a1cec72062798e51200000000000000000000869617420a4c95b1d3d6d012419d2b6c199cff9b68dd9a790892a4da8466fb056033166278100a1743ac4d5b".into(),
            "0400e02019d733c1fd76a1fa5950de7bee9d80f107276b93a67204000000000000000000a0d1dee718f5f732c041800e9aa2c25e92be3f6de28278545388db8a6ae27df64c37166278100a170a970c19".into()
        ];
        // modify hash of header 724610 to hash of header 0.
        let params = BlockHeaderValidationParams {
            difficulty_check: true,
            constant_difficulty: false,
            difficulty_algorithm: Some(DifficultyAlgorithm::BitcoinTestnet),
        };
        let conf = SPVConf {
            starting_block_header: SPVBlockHeader {
                height: 0,
                hash: "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f".into(),
                time: 486604799,
                bits: BlockHeaderBits::Compact(1231006505.into()),
            },
            max_stored_block_headers: None,
            validation_params: Some(params),
        };
        block_on(validate_headers(
            "BTC",
            724608,
            &headers,
            &TestBlockHeadersStorage { ticker: "BTC".into() },
            &conf,
        ))
        .unwrap()
    }
}
