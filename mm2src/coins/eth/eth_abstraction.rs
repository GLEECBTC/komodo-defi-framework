//? EVM account abstraction support

use super::*;
use bitcoin_hashes::hex::ToHex;
use ethabi::Function;
use mm2_eth::eip712::{Eip712, EIP712_DOMAIN, ObjectType, PropertyType};
use mm2_eth::eip712_encode::hash_typed_data;
// use alloy::rpc::types::eth::erc4337::PackedUserOperation;
//use alloy::sol_types::{eip712_domain, sol};
//use alloy::primitives;
/*use alloy::{
    primitives, //::{address, keccak256, U256},
	rpc::types::eth::erc4337::{PackedUserOperation, SendUserOperation},
    // signers::{local::PrivateKeySigner, Signer},
    sol,
    sol_types::{eip712_domain, SolStruct},
};*/
use lazy_static::lazy_static;

const PACKED_USEROP_PRIMARY_TYPE: &str = "PackedUserOperation";

const EIP7702_MAGIC: [u8; 1] = [0x05];

const EIP7702_INITCODE_MARKER: [u8; 2] = [0x77, 0x02]; 



lazy_static! {

	/// Pimlico paymaster contract v0.7 address
    static ref PIMLICO_ERC20_PAYMASTER_0_7: Address =
        Address::from_str("0x777777777777AeC03fd955926DbF81597e66834C").expect("Address::from_str valid");

    static ref PIMLICO_ERC20_PAYMASTER_0_8: Address =
        Address::from_str("0x888888888888Ec68A58AB8094Cc1AD20Ba3D2402").expect("Address::from_str valid");
		
	static ref SAFE_EIP_7702_PROXY: Address =
		Address::from_str("0xE60EcE6588DCcFb7373538034963B4D20a280DB0").expect("Address::from_str valid"); // NOTE: experimental, not audited contract
	
	static ref SIMPLE_EIP_7702_SMART_ACCOUNT: Address =
		Address::from_str("0xe6Cae83BdE06E4c305530e199D7217f42808555B").expect("Address::from_str valid");
	
	static ref EIP_4337_ENTRY_POINT_7: Address =
		Address::from_str("0x0000000071727De22E5E9d8BAf0edAc6f37da032").expect("Address::from_str valid");

	static ref EIP_4337_ENTRY_POINT_8: Address =
		Address::from_str("0x4337084D9E255Ff0702461CF8895CE9E3b5Ff108").expect("Address::from_str valid");

	static ref EIP712_DOMAIN_TYPES: ObjectType = Eip712Domain::build_types();

	static ref PACKED_USEROP_TYPES: Vec<ObjectType> = PackedUserOperation::build_types();

	//static ref EIP7702_INITCODE_MARKER_PADDED_RIGHT: Vec<u8> = [EIP7702_INITCODE_MARKER.as_slice(), [0u8; 18].as_slice()].concat();
	static ref EIP7702_INITCODE_MARKER_PADDED_RIGHT: Address = 
		Address::from_str("0x7702000000000000000000000000000000000000").expect("Address::from_str valid");

	//static ref EIP7702_INITCODE_MARKER_PADDED_STR: String = "0x".to_owned() + &hex::encode(EIP7702_INITCODE_MARKER.as_slice());
	static ref EIP7702_INITCODE_MARKER_PADDED: Address = 
		Address::from_str("0x0000000000000000000000000000000000007702").expect("Address::from_str valid");

	static ref EIP_4337_GET_NONCE: Function = serde_json::from_value(json!({
		"inputs":[
			{"internalType":"address","name":"sender","type":"address"},
			{"internalType":"uint192","name":"key","type":"uint192"}
		],
		"name":"getNonce",
		"outputs":[
			{"internalType":"uint256","name":"nonce","type":"uint256"}
		],
		"stateMutability":"view",
		"type":"function"
	})).expect("valid EIP-4337 getNonce ABI");
}

/// Struct to build Eip-712 domain separator 
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Eip712Domain {
    name: String,
	version: String,
	#[serde(rename = "chainId")]
	chain_id: U256,
	#[serde(rename = "verifyingContract")]
	verifying_contract: Address,
}

impl Eip712Domain {
	fn build_types() -> ObjectType {
		let mut domain_types = ObjectType::new(EIP712_DOMAIN);
		domain_types.property("name", PropertyType::String);
		domain_types.property("version", PropertyType::String);
		domain_types.property("chainId", PropertyType::Uint256);
		domain_types.property("verifyingContract", PropertyType::Address);
		domain_types
	}
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackedUserOperationTyped {
	sender: Address,
	nonce: U256,
	init_code: Bytes,
	call_data: Bytes,
	account_gas_limits: Bytes,
	pre_verification_gas: U256,
	gas_fees: Bytes,
	paymaster_and_data: Bytes,
	//eip_7702_auth: Option<SignedAuthorization>,
}

/// PackedUserOperation in the spec: Entry Point V0.7
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackedUserOperation {
    /// The account making the operation.
    pub sender: Address,
    /// Prevents message replay attacks and serves as a randomizing element for initial user
    /// registration.
    pub nonce: U256,
    /// Deployer contract address: Required exclusively for deploying new accounts that don't yet
    /// exist on the blockchain.
    pub factory: Option<Bytes>,  // Address
    /// Factory data for the account creation process, applicable only when using a deployer
    /// contract.
    pub factory_data: Option<Bytes>,
    /// The call data.
    pub call_data: Bytes,
    /// The gas limit for the call.
    pub call_gas_limit: U256,
    /// The gas limit for the verification.
    pub verification_gas_limit: U256,
    /// Prepaid gas fee: Covers the bundler's costs for initial transaction validation and data
    /// transmission.
    pub pre_verification_gas: U256,
    /// The maximum fee per gas.
    pub max_fee_per_gas: U256,
    /// The maximum priority fee per gas.
    pub max_priority_fee_per_gas: U256,
    /// Paymaster contract address: Needed if a third party is covering transaction costs; left
    /// blank for self-funded accounts.
    pub paymaster: Option<Address>,
    /// The gas limit for the paymaster verification.
    pub paymaster_verification_gas_limit: Option<U256>,
    /// The gas limit for the paymaster post-operation.
    pub paymaster_post_op_gas_limit: Option<U256>,
    /// The paymaster data.
    pub paymaster_data: Option<Bytes>, // TODO: remove?
    /// The signature of the transaction.
    pub signature: Bytes,
	/// Authorization to delegate to smart contract
	pub eip_7702_auth: Option<SignedAuthorization>,
}

impl PackedUserOperation {
	fn build_types() -> Vec<ObjectType> {
		let mut userop_type = ObjectType::new(PACKED_USEROP_PRIMARY_TYPE);
		userop_type.property("sender", PropertyType::Address);
		userop_type.property("nonce", PropertyType::Uint256);
		userop_type.property("initCode", PropertyType::Bytes);
		userop_type.property("callData", PropertyType::Bytes);
		userop_type.property("accountGasLimits", PropertyType::Bytes32);
		userop_type.property("preVerificationGas", PropertyType::Uint256);
		userop_type.property("gasFees", PropertyType::Bytes32);
		userop_type.property("paymasterAndData", PropertyType::Bytes);
		/*userop_type.property("eip7702Auth", PropertyType::Custom("eip7702Auth".to_string()));
		let mut eip_7702_auth_type = ObjectType::new("eip7702Auth");
		eip_7702_auth_type.property("address", PropertyType::Address);
		eip_7702_auth_type.property("chainId", PropertyType::Uint256);
		eip_7702_auth_type.property("nonce", PropertyType::Uint256);
		eip_7702_auth_type.property("r", PropertyType::Uint256);
		eip_7702_auth_type.property("s", PropertyType::Uint256);
		eip_7702_auth_type.property("v", PropertyType::Uint256);
		eip_7702_auth_type.property("yParity", PropertyType::Uint256);*/
		
		vec![userop_type] //, eip_7702_auth_type]
	}

	fn build_struct(&self) -> PackedUserOperationTyped {
		/*let mut init_code: Vec<u8> = self.factory.map(|addr| addr.to_bytes()).unwrap_or_default();
		if let Some(factory_data) = &self.factory_data {
			init_code.extend_from_slice(&factory_data.0)
		};*/
		PackedUserOperationTyped {
			sender: self.sender,
			nonce: self.nonce,
			init_code: SIMPLE_EIP_7702_SMART_ACCOUNT.as_bytes().into(),  // SIMPLE_EIP_7702_SMART_ACCOUNT.as_bytes().into(), //init_code.into(),
			call_data: self.call_data.clone(),
			account_gas_limits: make_bytes32_from_two(self.verification_gas_limit, self.call_gas_limit),
			pre_verification_gas: self.pre_verification_gas,
			gas_fees: make_bytes32_from_two(self.max_priority_fee_per_gas, self.max_fee_per_gas),
			paymaster_and_data: Default::default(), //PIMLICO_ERC20_PAYMASTER_0_7.as_bytes().into(),
			//eip_7702_auth: self.eip_7702_auth.clone(),
		}
	}
}

/// Eip-7702 autorisation
#[derive(Debug)]
struct Eip7702Authorization {
	address: Address,
    chain_id: U256,
    nonce: u64,
}

impl rlp::Encodable for Eip7702Authorization {
    fn rlp_append(&self, s: &mut RlpStream) { 
        s.begin_list(3);
		s.append(&self.chain_id);
		s.append(&self.address);
        s.append(&self.nonce);
	}
}

impl Eip7702Authorization {
	fn to_rlp(&self) -> Vec<u8> {
		let mut stream = RlpStream::new();
        self.rlp_append(&mut stream);
        if stream.is_finished() {
            Vec::from(stream.out())
        } else {
            warn!("RlpStream was not finished; returning an empty Vec as a fail-safe.");
            vec![]
        }
	}

	fn as_msg(&self) -> [u8; 32] {
		let mut msg = EIP7702_MAGIC.to_vec();
		msg.append(&mut self.to_rlp());
		keccak256(&msg).take()
	}
}


/// Eip7702 signed autorisation
/// NOTE: fields must be serialized as hex prefixed with "0x" and be in camelCase, according to the pimlico API docs
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedAuthorization {
	address: Address,
    chain_id: U256,
    nonce: U256,
    r: U256,
    s: U256,
	//v: U256,
    y_parity: U256,
}

impl SignedAuthorization {
	fn new(auth: Eip7702Authorization, sig: Signature) -> Self {
		Self {
			address: auth.address,
			chain_id: auth.chain_id,
			nonce: auth.nonce.into(),
			r: if sig.r().len() == 32 { U256::from(sig.r()) } else { U256::zero() },
			s: if sig.s().len() == 32 { U256::from(sig.s()) } else { U256::zero() },
			//v: (sig.v() + 27).into(),
			y_parity: sig.v().into(),
		}
	}
}

#[derive(Debug, Deserialize)]
pub struct SendUserOperationResponse {
    /// The hash of the user operation.
    pub user_op_hash: Bytes,
}

impl EthCoin {
	/// Build, sign and submit a PackedUserOperation that delegates `sender` (this coin's account)
	/// to the provided `safe_contract` by calling the Safe contract's delegate/enable method.
	///
	/// Notes:
	/// - This implementation uses the `PackedUserOperation` type to serialize the userOp.
	/// - It attempts to estimate gas for the call via `estimate_gas_for_contract_call_if_conf` and
	///   falls back to configured gas limits from this coin.
	/// - The method signs the user operation using the local `KeyPair` when available. For external
	///   signers (WalletConnect / MetaMask) the call will currently return an Err indicating the
	///   external path should be implemented if needed.
	async fn delegate_to_safe_account(
		&self,
		safe_contract: Address,
		delegate_call_data: Bytes,
	) -> Result<H256, String> {

		let paymaster = *PIMLICO_ERC20_PAYMASTER_0_7;
		let eip4337_rpc = self.eip4337_rpc.as_ref().ok_or("AA RPC not initialized".to_string())?;
		// Determine sender address (this coin's account address)
		let my_address_str = self.my_address()
			.map_err(|e| format!("Failed to get my_address: {e}"))?;
		let my_address = Address::from_str(&my_address_str)
			.map_err(|e| format!("Failed to parse my_address: {e}"))?;
		let address_lock = self.get_address_lock(my_address).await;
		let _nonce_lock = address_lock.lock().await;
		let (nonce, _) = self
			.clone()
			.get_addr_nonce(my_address)
			.compat()
			.await
			.map_err(|e| format!("Failed to get nonce: {e}"))?;
		drop(_nonce_lock);

		/*let eip7702_address_lock = self.get_address_lock(*SAFE_EIP_7702_PROXY).await;
		let eip7702_nonce_lock = address_lock.lock().await;
		let (eip7702_nonce, _) = self
			.clone()
			.get_addr_nonce(*SAFE_EIP_7702_PROXY)
			.compat()
			.await
			.map_err(|e| format!("Failed to get nonce: {e}"))?;
		drop(eip7702_nonce_lock);*/

		let get_nonce_data = EIP_4337_GET_NONCE.encode_input(&[Token::Address(my_address), Token::Uint(U256::from(0))])
			.map_err(|err| err.to_string())?;
		let res = self
			.call_request(my_address, *EIP_4337_ENTRY_POINT_8, None, Some(get_nonce_data.into()), BlockNumber::Latest)
			.await
			.map_err(|err| err.to_string())?;
		let outputs = EIP_4337_GET_NONCE.decode_output(&res.0).map_err(|err| err.to_string())?;
		let eip4337_nonce = match outputs.get(0) {
			Some(Token::Uint(val)) => *val,
			_ => return Err("could not decode getNonce response".to_string()),
		};

		// Estimate gas for the contract call if allowed by config
		/*let call_req = web3::types::CallRequest {
			from: Some(sender),
			to: Some(safe_contract),
			gas: None,
			gas_price: None,
			value: Some(U256::zero()),
			data: Some(delegate_call_data.clone()),
			transaction_type: None,
			access_list: None,
			max_priority_fee_per_gas: None,
			max_fee_per_gas: None,
		};*/

		let estimated_gas_opt = U256::from(250_000u64);
		/*let estimated_gas_opt = None;
		let estimated_gas_opt = match self.estimate_gas_for_contract_call_if_conf(safe_contract, delegate_call_data.0.clone()).await {
			Ok(v) => v,
			Err(e) => {
				// Log and continue with None so we fall back to gas_limit
				debug!("estimate_gas_for_contract_call_if_conf failed: {e}");
				None
			}
		};*/

		
		let verification_gas_limit = U256::from(100_000u64); // conservative default
		// Choose callGasLimit: network estimate or configured fallback
		let call_gas_limit = U256::zero(); // estimated_gas_opt;
		let (max_fee_per_gas, max_priority_fee_per_gas) = (U256::from(100_000_000_000u64), U256::from(2_000_000_000u64));
		//let (max_fee_per_gas, max_priority_fee_per_gas) = (U256::zero(), U256::zero());

		let chain_id = self.chain_id().ok_or("No chain id".to_string())?.into();
		// Delegate to Safe Proxy experimental smart contract
		let eip7702_delegate_auth = Eip7702Authorization {
			address: *SIMPLE_EIP_7702_SMART_ACCOUNT, //*SAFE_EIP_7702_PROXY,
			chain_id,
			nonce: nonce.as_u64(), // TODO: may panic
		};

		// Build EIP-712 domain and hash userOp.
		let domain = Eip712Domain {
			name: "ERC4337".to_owned(),
			version: "1".to_owned(),
			chain_id,
			verifying_contract: *EIP_4337_ENTRY_POINT_8, //paymaster,
			//salt: keccak256(b"test").as_fixed_bytes().into(),
		};
		
		// Prepare basic PackedUserOperation fields. 
		// We fill common fields and leave paymasterAndData empty.
		let mut user_op_packed = PackedUserOperation {
			sender: Address::from_str(&my_address_str).map_err(|e| format!("Failed to parse my_address: {e}"))?,
			nonce: eip4337_nonce,
			factory: Some([0x77, 0x02].to_vec().into()), // Some(*EIP7702_INITCODE_MARKER_PADDED_RIGHT),
			factory_data: None,
			call_data: delegate_call_data,
			call_gas_limit,
			verification_gas_limit,
			pre_verification_gas: U256::from(210_000u64),
			max_fee_per_gas,
			max_priority_fee_per_gas,
			paymaster: None, // Some(paymaster),
			paymaster_verification_gas_limit: None, //Some(U256::from(21_000u64)),
			paymaster_post_op_gas_limit: None, //Some(U256::from(21_000u64)),
			paymaster_data: Some(Bytes::default()),
			signature: Bytes::default(),
    		eip_7702_auth: None,
		};

		// Sign the PackedUserOperation according to EIP-712
        match self.priv_key_policy {
            EthPrivKeyPolicy::Iguana(ref key_pair)
            | EthPrivKeyPolicy::HDWallet {
                activated_key: ref key_pair,
                ..
            } => {


				/*let packed_to_sign = PackedUserOperationToSign {
					sender: packed.sender,
					nonce: packed.nonce,
					initCode: Bytes::default(),
					callData: packed.call_data.clone(),
					accountGasLimits: make_byte32_from_two(packed.verification_gas_limit, packed.call_gas_limit),
					preVerificationGas: packed.pre_verification_gas,
					gasFees: make_byte32_from_two(packed.max_priority_fee_per_gas, packed.max_fee_per_gas),
					paymasterAndData: PIMLICO_ERC20_PAYMASTER_0_7.as_bytes().into(),
				};*/

				// NOTE: alloy helpers may expose a method like `PackedUserOperation::hash_eip712`.
				// If not, the crate compile will point to the correct API and can be adjusted.
				//let userop_hash = packed_to_sign.eip712_signing_hash(&domain);
					//.map_err(|e| format!("Failed to compute userop EIP712 hash: {e}"))?;

				let auth_sig = sign(key_pair.secret(), &eip7702_delegate_auth.as_msg().into()).map_err(|e| format!("Signing failed: {e}"))?;
				let signed_auth = SignedAuthorization::new(eip7702_delegate_auth, auth_sig);
				user_op_packed.eip_7702_auth = Some(signed_auth);
				let user_op_typed_data = user_op_hash_eip712_request(domain, user_op_packed.build_struct());
				let user_op_typed_hash = hash_typed_data(user_op_typed_data)
					.map_err(|e| format!("Typed hash failed: {e}"))?;

				println!("user_op_typed_hash={}", user_op_typed_hash.to_hex());
				// Sign the hash with KeyPair
				let sig = sign(key_pair.secret(), &user_op_typed_hash).map_err(|e| format!("Signing failed: {e}"))?.into_electrum();
				//println!("signature: r={:?} s={:?} v={} is_low_s={}", sig.r(), sig.s(), sig.v(), sig.is_low_s());
				user_op_packed.signature = sig.to_vec().into();
			},
            EthPrivKeyPolicy::Trezor 
			| EthPrivKeyPolicy::WalletConnect { .. } => return Err("External signers (WalletConnect/Trezor) are not implemented for userOps yet".into()),
            #[cfg(target_arch = "wasm32")]
            EthPrivKeyPolicy::Metamask(_) => return Err("MetaMask is not implemented for userOps yet".into()),
        }


		
		// Submit via eth_sendUserOperation RPC. Many bundlers accept params: [userOp, entryPoint]
		// We try to call `eth_sendUserOperation` with the packed userop serialized to JSON.
		let userop_j = serde_json::to_value(&user_op_packed).map_err(|e| format!("Failed to serialize userop: {e}"))?;

		// Use configured entry point if present, otherwise omit (many bundlers accept two-arg form)
		//let entry_point = helpers::serialize(&self.entry_point_address);

		// Call the RPC and return the result. Wrap web3::Error into String for simplicity.
		//let rpc_params = vec![userop_json, serde_json::Value::String(self.entry_point_address.to_string())];

		let resp_json = Eip4337Rpc::send_user_operation(eip4337_rpc, userop_j, *EIP_4337_ENTRY_POINT_8)
			.await
			.map_err(|err| format!("RPC error sending userOp: {err}"))?;
		/*let user_op_resp = serde_json::from_value::<SendUserOperationResponse>(resp_j)
			.map_err(|err| format!("Error parsing userOp RPC response: {err}"))?;
		let user_op_hash = H256::from_str(&hex::encode(user_op_resp.user_op_hash.0))
			.map_err(|err| format!("Error parsing userOp hash in RPC response: {err}"))?;*/
		let user_op_resp = resp_json.as_str()
			.ok_or(format!("Error parsing userOpHash response: expected string"))?;
		let user_op_hash = H256::from_str(user_op_resp)
			.map_err(|err| format!("Error parsing userOpHash response: {err}"))?;
		Ok(user_op_hash)
	}
}

pub(crate) fn user_op_hash_eip712_request(
    domain: Eip712Domain,
    req: PackedUserOperationTyped,
) -> Eip712<Eip712Domain, PackedUserOperationTyped> {
    let mut v_types = vec![EIP712_DOMAIN_TYPES.deref()];
	v_types.extend(PACKED_USEROP_TYPES.deref());
	let types = v_types
        .iter()
        .map(|&object_type| (object_type.name.clone(), object_type.properties.clone()))
        .collect();
    Eip712 {
        types,
        domain,
        primary_type: PACKED_USEROP_PRIMARY_TYPE.to_string(),
        message: req,
    }
}

fn make_init_code(_delegate_address: Address) -> Bytes {
	todo!()
}

fn make_bytes32_from_two(u1: U256, u2: U256) -> Bytes {
	let mut u1_bytes = [0_u8; 32];
	let mut u2_bytes = [0_u8; 32];
	let mut bytes_32 = [0_u8; 32];
	u1.to_big_endian(&mut u1_bytes);
	u2.to_big_endian(&mut u2_bytes);
	bytes_32[0..16].copy_from_slice(&u1_bytes[16..32]);
	bytes_32[16..32].copy_from_slice(&u2_bytes[16..32]);
	bytes_32.into()
}


mod tests {
	use super::*;
	use crate::lp_coininit;
	use common::block_on;
	use ethabi::Address;
	use mm2_core::mm_ctx::MmCtxBuilder;
	use mm2_test_helpers::for_tests::{ETH_SEPOLIA_CHAIN_ID, ETH_SEPOLIA_NODES};

	#[test]
	fn sepolia_delegate_to_safe() {
		const ETH: &str = "ETH";
		const ETH_TOKEN: &str = "JST";

		let conf = json!({
			"coins": [{
				"coin": "ETH",
				"name": "ethereum-sepolia",
				"fname": "Ethereum",
				"rpcport": 80,
				"mm2": 1,
				"sign_message_prefix": "Ethereum Signed Message:\n",
				"required_confirmations": 3,
				"avg_blocktime": 15,
				"protocol": {
					"type": "ETH",
					"protocol_data": {
						"chain_id": ETH_SEPOLIA_CHAIN_ID
					}
				},
				"derivation_path": "m/44'/60'"
			},{
				"coin": "JST",
				"name": "Just Token",
				"fname": "jst",
				"rpcport": 80,
				"mm2": 1,
				"avg_blocktime": 15,
				"required_confirmations": 3,
				"decimals": 18,
				"protocol": {
					"type": "ERC20",
					"protocol_data": {
					"platform": "ETH",
					"contract_address": "0x948BF5172383F1Bc0Fdf3aBe0630b855694A5D2c"
					}
				},
				"derivation_path": "m/44'/60'"
			}]
		});

		let ctx = MmCtxBuilder::new().with_conf(conf).into_mm_arc();
		CryptoCtx::init_with_iguana_passphrase(
			ctx.clone(),
			"hen garden proud labor donkey cluster shield jazz worry category pelican immune body letter green badge face more apology smile estate ridge fall armor", // TODO: don't push
		)
		.unwrap();

		let eth_params = json!({
			"urls": ETH_SEPOLIA_NODES,
			"eip4337_url": "https://api.pimlico.io/v2/11155111/rpc?apikey=PIMLICO-KEY-HERE",
			"swap_contract_address": "0x9130b257d37a52e52f21054c4da3450c72f595ce",
		});
		let coin = match block_on(lp_coininit(&ctx, ETH, &eth_params)).unwrap() {
			MmCoinEnum::EthCoin(coin) => coin,
			_ => panic!("incorrect coin type"),
		};

		let res = block_on(coin.delegate_to_safe_account(Address::zero(), Bytes::from(Vec::new())));
		println!("res={:?}", res);
	}
}