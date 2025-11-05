use crate::docker_tests::docker_tests_common::*;
use crate::integration_tests_common::enable_native;
use bitcrypto::dhash160;
use coins::qrc20::rpc_clients::for_tests::Qrc20NativeWalletOps;
use coins::utxo::qtum::{qtum_coin_with_priv_key, QtumCoin};
use coins::utxo::rpc_clients::UtxoRpcClientEnum;
use coins::utxo::utxo_common::big_decimal_from_sat;
use coins::utxo::{UtxoActivationParams, UtxoCommonOps};
use coins::{
    CheckIfMyPaymentSentArgs, ConfirmPaymentInput, DexFee, DexFeeBurnDestination, FeeApproxStage, FoundSwapTxSpend,
    MarketCoinOps, MmCoin, RefundPaymentArgs, SearchForSwapTxSpendInput, SendPaymentArgs, SpendPaymentArgs, SwapOps,
    SwapTxTypeWithSecretHash, TradePreimageValue, TransactionEnum, ValidateFeeArgs, ValidatePaymentInput,
    WaitForHTLCTxSpendArgs,
};
use common::{block_on_f01, temp_dir, DEX_FEE_ADDR_RAW_PUBKEY};
use crypto::Secp256k1Secret;
use ethereum_types::H160;
use http::StatusCode;
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_main::lp_swap::max_taker_vol_from_available;
use mm2_number::BigDecimal;
use mm2_rpc::data::legacy::{CoinInitResponse, OrderbookResponse};
use mm2_test_helpers::structs::{trade_preimage_error, RpcErrorResponse, RpcSuccessResponse, TransactionDetails};
use rand6::Rng;
use serde_json::{self as json, Value as Json};
use std::convert::TryFrom;
use std::process::Command;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Duration;
use testcontainers::clients::Cli;
use testcontainers::core::WaitFor;
use testcontainers::{GenericImage, RunnableImage};

pub const QTUM_REGTEST_DOCKER_IMAGE: &str = "docker.io/sergeyboyko/qtumregtest";
pub const QTUM_REGTEST_DOCKER_IMAGE_WITH_TAG: &str = "docker.io/sergeyboyko/qtumregtest:latest";

const QRC20_TOKEN_BYTES: &str = "6080604052600860ff16600a0a633b9aca000260005534801561002157600080fd5b50600054600160003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002081905550610c69806100776000396000f3006080604052600436106100a4576000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff16806306fdde03146100a9578063095ea7b31461013957806318160ddd1461019e57806323b872dd146101c9578063313ce5671461024e5780635a3b7e421461027f57806370a082311461030f57806395d89b4114610366578063a9059cbb146103f6578063dd62ed3e1461045b575b600080fd5b3480156100b557600080fd5b506100be6104d2565b6040518080602001828103825283818151815260200191508051906020019080838360005b838110156100fe5780820151818401526020810190506100e3565b50505050905090810190601f16801561012b5780820380516001836020036101000a031916815260200191505b509250505060405180910390f35b34801561014557600080fd5b50610184600480360381019080803573ffffffffffffffffffffffffffffffffffffffff1690602001909291908035906020019092919050505061050b565b604051808215151515815260200191505060405180910390f35b3480156101aa57600080fd5b506101b36106bb565b6040518082815260200191505060405180910390f35b3480156101d557600080fd5b50610234600480360381019080803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803590602001909291905050506106c1565b604051808215151515815260200191505060405180910390f35b34801561025a57600080fd5b506102636109a1565b604051808260ff1660ff16815260200191505060405180910390f35b34801561028b57600080fd5b506102946109a6565b6040518080602001828103825283818151815260200191508051906020019080838360005b838110156102d45780820151818401526020810190506102b9565b50505050905090810190601f1680156103015780820380516001836020036101000a031916815260200191505b509250505060405180910390f35b34801561031b57600080fd5b50610350600480360381019080803573ffffffffffffffffffffffffffffffffffffffff1690602001909291905050506109df565b6040518082815260200191505060405180910390f35b34801561037257600080fd5b5061037b6109f7565b6040518080602001828103825283818151815260200191508051906020019080838360005b838110156103bb5780820151818401526020810190506103a0565b50505050905090810190601f1680156103e85780820380516001836020036101000a031916815260200191505b509250505060405180910390f35b34801561040257600080fd5b50610441600480360381019080803573ffffffffffffffffffffffffffffffffffffffff16906020019092919080359060200190929190505050610a30565b604051808215151515815260200191505060405180910390f35b34801561046757600080fd5b506104bc600480360381019080803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190505050610be1565b6040518082815260200191505060405180910390f35b6040805190810160405280600881526020017f515243205445535400000000000000000000000000000000000000000000000081525081565b60008260008173ffffffffffffffffffffffffffffffffffffffff161415151561053457600080fd5b60008314806105bf57506000600260003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054145b15156105ca57600080fd5b82600260003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055508373ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff167f8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925856040518082815260200191505060405180910390a3600191505092915050565b60005481565b60008360008173ffffffffffffffffffffffffffffffffffffffff16141515156106ea57600080fd5b8360008173ffffffffffffffffffffffffffffffffffffffff161415151561071157600080fd5b610797600260008873ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205485610c06565b600260008873ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002081905550610860600160008873ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205485610c06565b600160008873ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055506108ec600160008773ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205485610c1f565b600160008773ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055508473ffffffffffffffffffffffffffffffffffffffff168673ffffffffffffffffffffffffffffffffffffffff167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef866040518082815260200191505060405180910390a36001925050509392505050565b600881565b6040805190810160405280600981526020017f546f6b656e20302e31000000000000000000000000000000000000000000000081525081565b60016020528060005260406000206000915090505481565b6040805190810160405280600381526020017f515443000000000000000000000000000000000000000000000000000000000081525081565b60008260008173ffffffffffffffffffffffffffffffffffffffff1614151515610a5957600080fd5b610aa2600160003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205484610c06565b600160003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002081905550610b2e600160008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205484610c1f565b600160008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055508373ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef856040518082815260200191505060405180910390a3600191505092915050565b6002602052816000526040600020602052806000526040600020600091509150505481565b6000818310151515610c1457fe5b818303905092915050565b6000808284019050838110151515610c3357fe5b80915050929150505600a165627a7a723058207f2e5248b61b80365ea08a0f6d11ac0b47374c4dfd538de76bc2f19591bbbba40029";
const QRC20_SWAP_CONTRACT_BYTES: &str = "608060405234801561001057600080fd5b50611437806100206000396000f3fe60806040526004361061004a5760003560e01c806302ed292b1461004f5780630716326d146100de578063152cf3af1461017b57806346fc0294146101f65780639b415b2a14610294575b600080fd5b34801561005b57600080fd5b506100dc600480360360a081101561007257600080fd5b81019080803590602001909291908035906020019092919080359060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190505050610339565b005b3480156100ea57600080fd5b506101176004803603602081101561010157600080fd5b8101908080359060200190929190505050610867565b60405180846bffffffffffffffffffffffff19166bffffffffffffffffffffffff191681526020018367ffffffffffffffff1667ffffffffffffffff16815260200182600381111561016557fe5b60ff168152602001935050505060405180910390f35b6101f46004803603608081101561019157600080fd5b8101908080359060200190929190803573ffffffffffffffffffffffffffffffffffffffff16906020019092919080356bffffffffffffffffffffffff19169060200190929190803567ffffffffffffffff1690602001909291905050506108bf565b005b34801561020257600080fd5b50610292600480360360a081101561021957600080fd5b81019080803590602001909291908035906020019092919080356bffffffffffffffffffffffff19169060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190505050610bd9565b005b610337600480360360c08110156102aa57600080fd5b810190808035906020019092919080359060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803573ffffffffffffffffffffffffffffffffffffffff16906020019092919080356bffffffffffffffffffffffff19169060200190929190803567ffffffffffffffff169060200190929190505050610fe2565b005b6001600381111561034657fe5b600080878152602001908152602001600020600001601c9054906101000a900460ff16600381111561037457fe5b1461037e57600080fd5b6000600333836003600288604051602001808281526020019150506040516020818303038152906040526040518082805190602001908083835b602083106103db57805182526020820191506020810190506020830392506103b8565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa15801561041d573d6000803e3d6000fd5b5050506040513d602081101561043257600080fd5b8101908080519060200190929190505050604051602001808281526020019150506040516020818303038152906040526040518082805190602001908083835b602083106104955780518252602082019150602081019050602083039250610472565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa1580156104d7573d6000803e3d6000fd5b5050506040515160601b8689604051602001808673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b81526014018573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401846bffffffffffffffffffffffff19166bffffffffffffffffffffffff191681526014018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401828152602001955050505050506040516020818303038152906040526040518082805190602001908083835b602083106105fc57805182526020820191506020810190506020830392506105d9565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa15801561063e573d6000803e3d6000fd5b5050506040515160601b905060008087815260200190815260200160002060000160009054906101000a900460601b6bffffffffffffffffffffffff1916816bffffffffffffffffffffffff19161461069657600080fd5b6002600080888152602001908152602001600020600001601c6101000a81548160ff021916908360038111156106c857fe5b0217905550600073ffffffffffffffffffffffffffffffffffffffff168373ffffffffffffffffffffffffffffffffffffffff16141561074e573373ffffffffffffffffffffffffffffffffffffffff166108fc869081150290604051600060405180830381858888f19350505050158015610748573d6000803e3d6000fd5b50610820565b60008390508073ffffffffffffffffffffffffffffffffffffffff1663a9059cbb33886040518363ffffffff1660e01b8152600401808373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200182815260200192505050602060405180830381600087803b1580156107da57600080fd5b505af11580156107ee573d6000803e3d6000fd5b505050506040513d602081101561080457600080fd5b810190808051906020019092919050505061081e57600080fd5b505b7f36c177bcb01c6d568244f05261e2946c8c977fa50822f3fa098c470770ee1f3e8685604051808381526020018281526020019250505060405180910390a1505050505050565b60006020528060005260406000206000915090508060000160009054906101000a900460601b908060000160149054906101000a900467ffffffffffffffff169080600001601c9054906101000a900460ff16905083565b600073ffffffffffffffffffffffffffffffffffffffff168373ffffffffffffffffffffffffffffffffffffffff16141580156108fc5750600034115b801561094057506000600381111561091057fe5b600080868152602001908152602001600020600001601c9054906101000a900460ff16600381111561093e57fe5b145b61094957600080fd5b60006003843385600034604051602001808673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b81526014018573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401846bffffffffffffffffffffffff19166bffffffffffffffffffffffff191681526014018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401828152602001955050505050506040516020818303038152906040526040518082805190602001908083835b60208310610a6c5780518252602082019150602081019050602083039250610a49565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa158015610aae573d6000803e3d6000fd5b5050506040515160601b90506040518060600160405280826bffffffffffffffffffffffff191681526020018367ffffffffffffffff16815260200160016003811115610af757fe5b81525060008087815260200190815260200160002060008201518160000160006101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908360601c021790555060208201518160000160146101000a81548167ffffffffffffffff021916908367ffffffffffffffff160217905550604082015181600001601c6101000a81548160ff02191690836003811115610b9357fe5b02179055509050507fccc9c05183599bd3135da606eaaf535daffe256e9de33c048014cffcccd4ad57856040518082815260200191505060405180910390a15050505050565b60016003811115610be657fe5b600080878152602001908152602001600020600001601c9054906101000a900460ff166003811115610c1457fe5b14610c1e57600080fd5b600060038233868689604051602001808673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b81526014018573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401846bffffffffffffffffffffffff19166bffffffffffffffffffffffff191681526014018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401828152602001955050505050506040516020818303038152906040526040518082805190602001908083835b60208310610d405780518252602082019150602081019050602083039250610d1d565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa158015610d82573d6000803e3d6000fd5b5050506040515160601b905060008087815260200190815260200160002060000160009054906101000a900460601b6bffffffffffffffffffffffff1916816bffffffffffffffffffffffff1916148015610e10575060008087815260200190815260200160002060000160149054906101000a900467ffffffffffffffff1667ffffffffffffffff164210155b610e1957600080fd5b6003600080888152602001908152602001600020600001601c6101000a81548160ff02191690836003811115610e4b57fe5b0217905550600073ffffffffffffffffffffffffffffffffffffffff168373ffffffffffffffffffffffffffffffffffffffff161415610ed1573373ffffffffffffffffffffffffffffffffffffffff166108fc869081150290604051600060405180830381858888f19350505050158015610ecb573d6000803e3d6000fd5b50610fa3565b60008390508073ffffffffffffffffffffffffffffffffffffffff1663a9059cbb33886040518363ffffffff1660e01b8152600401808373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200182815260200192505050602060405180830381600087803b158015610f5d57600080fd5b505af1158015610f71573d6000803e3d6000fd5b505050506040513d6020811015610f8757600080fd5b8101908080519060200190929190505050610fa157600080fd5b505b7f1797d500133f8e427eb9da9523aa4a25cb40f50ebc7dbda3c7c81778973f35ba866040518082815260200191505060405180910390a1505050505050565b600073ffffffffffffffffffffffffffffffffffffffff168373ffffffffffffffffffffffffffffffffffffffff161415801561101f5750600085115b801561106357506000600381111561103357fe5b600080888152602001908152602001600020600001601c9054906101000a900460ff16600381111561106157fe5b145b61106c57600080fd5b60006003843385888a604051602001808673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b81526014018573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401846bffffffffffffffffffffffff19166bffffffffffffffffffffffff191681526014018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401828152602001955050505050506040516020818303038152906040526040518082805190602001908083835b6020831061118e578051825260208201915060208101905060208303925061116b565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa1580156111d0573d6000803e3d6000fd5b5050506040515160601b90506040518060600160405280826bffffffffffffffffffffffff191681526020018367ffffffffffffffff1681526020016001600381111561121957fe5b81525060008089815260200190815260200160002060008201518160000160006101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908360601c021790555060208201518160000160146101000a81548167ffffffffffffffff021916908367ffffffffffffffff160217905550604082015181600001601c6101000a81548160ff021916908360038111156112b557fe5b021790555090505060008590508073ffffffffffffffffffffffffffffffffffffffff166323b872dd33308a6040518463ffffffff1660e01b8152600401808473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018281526020019350505050602060405180830381600087803b15801561137d57600080fd5b505af1158015611391573d6000803e3d6000fd5b505050506040513d60208110156113a757600080fd5b81019080805190602001909291905050506113c157600080fd5b7fccc9c05183599bd3135da606eaaf535daffe256e9de33c048014cffcccd4ad57886040518082815260200191505060405180910390a1505050505050505056fea265627a7a723158208c83db436905afce0b7be1012be64818c49323c12d451fe2ab6bce76ff6421c964736f6c63430005110032";
const TAKER_PAYMENT_SPEND_SEARCH_INTERVAL: f64 = 1.;

pub struct QtumDockerOps {
    #[allow(dead_code)]
    ctx: MmArc,
    coin: QtumCoin,
}

impl CoinDockerOps for QtumDockerOps {
    fn rpc_client(&self) -> &UtxoRpcClientEnum {
        &self.coin.as_ref().rpc_client
    }
}

impl QtumDockerOps {
    pub fn new() -> QtumDockerOps {
        let ctx = MmCtxBuilder::new().into_mm_arc();
        let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
        let conf = json!({"coin":"QTUM","decimals":8,"network":"regtest","confpath":confpath});
        let req = json!({
            "method": "enable",
        });
        let priv_key = Secp256k1Secret::from("809465b17d0a4ddb3e4c69e8f23c2cabad868f51f8bed5c765ad1d6516c3306f");
        let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
        let coin = block_on(qtum_coin_with_priv_key(&ctx, "QTUM", &conf, &params, priv_key)).unwrap();
        QtumDockerOps { ctx, coin }
    }

    pub fn initialize_contracts(&self) {
        let sender = get_address_by_label(&self.coin, QTUM_ADDRESS_LABEL);
        unsafe {
            QICK_TOKEN_ADDRESS = Some(self.create_contract(&sender, QRC20_TOKEN_BYTES));
            QORTY_TOKEN_ADDRESS = Some(self.create_contract(&sender, QRC20_TOKEN_BYTES));
            QRC20_SWAP_CONTRACT_ADDRESS = Some(self.create_contract(&sender, QRC20_SWAP_CONTRACT_BYTES));
        }
    }

    fn create_contract(&self, sender: &str, hexbytes: &str) -> H160 {
        let bytecode = hex::decode(hexbytes).expect("Hex encoded bytes expected");
        let gas_limit = 2_500_000u64;
        let gas_price = BigDecimal::from_str("0.0000004").unwrap();

        match self.coin.as_ref().rpc_client {
            UtxoRpcClientEnum::Native(ref native) => {
                let result = block_on_f01(native.create_contract(&bytecode.into(), gas_limit, gas_price, sender))
                    .expect("!createcontract");
                result.address.0.into()
            },
            UtxoRpcClientEnum::Electrum(_) => panic!("Native client expected"),
        }
    }
}

pub fn qtum_docker_node(docker: &Cli, port: u16) -> DockerNode<'_> {
    let image = GenericImage::new(QTUM_REGTEST_DOCKER_IMAGE, "latest")
        .with_env_var("CLIENTS", "2")
        .with_env_var("COIN_RPC_PORT", port.to_string())
        .with_env_var("ADDRESS_LABEL", QTUM_ADDRESS_LABEL)
        .with_env_var("FILL_MEMPOOL", "true")
        .with_wait_for(WaitFor::message_on_stdout("config is ready"));
    let image = RunnableImage::from(image).with_mapped_port((port, port));
    let container = docker.run(image);

    let name = "qtum";
    let mut conf_path = temp_dir().join("qtum-regtest");
    std::fs::create_dir_all(&conf_path).unwrap();
    conf_path.push(format!("{name}.conf"));
    Command::new("docker")
        .arg("cp")
        .arg(format!("{}:/data/node_0/{}.conf", container.id(), name))
        .arg(&conf_path)
        .status()
        .expect("Failed to execute docker command");
    let timeout = wait_until_ms(3000);
    loop {
        if conf_path.exists() {
            break;
        };
        assert!(now_ms() < timeout, "Test timed out");
    }

    unsafe { QTUM_CONF_PATH = Some(conf_path) };
    DockerNode {
        container,
        ticker: name.to_owned(),
        port,
    }
}

fn withdraw_and_send(mm: &MarketMakerIt, coin: &str, to: &str, amount: f64) {
    let withdraw = block_on(mm.rpc(&json! ({
        "mmrpc": "2.0",
        "userpass": mm.userpass,
        "method": "withdraw",
        "params": {
            "coin": coin,
            "to": to,
            "amount": amount,
        },
        "id": 0,
    })))
    .unwrap();

    assert!(withdraw.0.is_success(), "!withdraw: {}", withdraw.1);
    let res: RpcSuccessResponse<TransactionDetails> =
        json::from_str(&withdraw.1).expect("Expected 'RpcSuccessResponse<TransactionDetails>'");
    let tx_details = res.result;

    log!("Balance Change: {:?}", tx_details.my_balance_change);

    assert_eq!(tx_details.to, vec![to.to_owned()]);
    assert!(BigDecimal::try_from(amount).unwrap() + tx_details.my_balance_change < 0.into());

    let send = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "send_raw_transaction",
        "coin": coin,
        "tx_hex": tx_details.tx_hex,
    })))
    .unwrap();
    assert!(send.0.is_success(), "!{} send: {}", coin, send.1);
    let send_json: Json = json::from_str(&send.1).unwrap();
    assert_eq!(tx_details.tx_hash, send_json["tx_hash"]);
}

#[test]
fn test_taker_spends_maker_payment() {
    let (_ctx, maker_coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 10.into());
    let (_ctx, taker_coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 1.into());
    let maker_old_balance = block_on_f01(maker_coin.my_spendable_balance()).expect("Error on get maker balance");
    let taker_old_balance = block_on_f01(taker_coin.my_spendable_balance()).expect("Error on get taker balance");
    assert_eq!(maker_old_balance, BigDecimal::from(10));
    assert_eq!(taker_old_balance, BigDecimal::from(1));

    let timelock = now_sec() - 200;
    let maker_pub = maker_coin.my_public_key().unwrap().to_vec();
    let taker_pub = taker_coin.my_public_key().unwrap().to_vec();
    let secret = &[1; 32];
    let secret_hash = dhash160(secret).to_vec();
    let amount = BigDecimal::try_from(0.2).unwrap();
    let maker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock: timelock,
        other_pubkey: &taker_pub,
        secret_hash: &secret_hash,
        amount: amount.clone(),
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let payment = block_on(maker_coin.send_maker_payment(maker_payment_args)).unwrap();
    let payment_tx_hash = payment.tx_hash_as_bytes();
    let payment_tx_hex = payment.tx_hex();
    log!("Maker payment: {:?}", payment_tx_hash);

    let confirmations = 1;
    let requires_nota = false;
    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let check_every = 1;
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: payment_tx_hex.clone(),
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(taker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let input = ValidatePaymentInput {
        payment_tx: payment_tx_hex.clone(),
        time_lock_duration: 0,
        time_lock: timelock,
        other_pub: maker_pub.clone(),
        secret_hash: secret_hash.clone(),
        amount: amount.clone(),
        swap_contract_address: taker_coin.swap_contract_address(),
        try_spv_proof_until: wait_until + 30,
        confirmations,
        unique_swap_data: Vec::new(),
        watcher_reward: None,
    };
    block_on(taker_coin.validate_maker_payment(input)).unwrap();
    let taker_spends_payment_args = SpendPaymentArgs {
        other_payment_tx: &payment_tx_hex,
        time_lock: timelock,
        other_pubkey: &maker_pub,
        secret,
        secret_hash: &secret_hash,
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let spend = block_on(taker_coin.send_taker_spends_maker_payment(taker_spends_payment_args)).unwrap();
    let spend_tx_hash = spend.tx_hash_as_bytes();
    let spend_tx_hex = spend.tx_hex();
    log!("Taker spends tx: {:?}", spend_tx_hash);

    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: spend_tx_hex,
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(taker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let maker_balance = block_on_f01(maker_coin.my_spendable_balance()).expect("Error on get maker balance");
    let taker_balance = block_on_f01(taker_coin.my_spendable_balance()).expect("Error on get taker balance");
    assert_eq!(maker_old_balance - amount.clone(), maker_balance);
    assert_eq!(taker_old_balance + amount, taker_balance);
}

#[test]
fn test_maker_spends_taker_payment() {
    let (_ctx, maker_coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 10.into());
    let (_ctx, taker_coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 10.into());
    let maker_old_balance = block_on_f01(maker_coin.my_spendable_balance()).expect("Error on get maker balance");
    let taker_old_balance = block_on_f01(taker_coin.my_spendable_balance()).expect("Error on get taker balance");
    assert_eq!(maker_old_balance, BigDecimal::from(10));
    assert_eq!(taker_old_balance, BigDecimal::from(10));

    let timelock = now_sec() - 200;
    let maker_pub = maker_coin.my_public_key().unwrap().to_vec();
    let taker_pub = taker_coin.my_public_key().unwrap().to_vec();
    let secret = &[1; 32];
    let secret_hash = dhash160(secret).to_vec();
    let amount = BigDecimal::try_from(0.2).unwrap();
    let taker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock: timelock,
        other_pubkey: &maker_pub,
        secret_hash: &secret_hash,
        amount: amount.clone(),
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let payment = block_on(taker_coin.send_taker_payment(taker_payment_args)).unwrap();
    let payment_tx_hash = payment.tx_hash_as_bytes();
    let payment_tx_hex = payment.tx_hex();
    log!("Taker payment: {:?}", payment_tx_hash);

    let confirmations = 1;
    let requires_nota = false;
    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let check_every = 1;
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: payment_tx_hex.clone(),
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(maker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let input = ValidatePaymentInput {
        payment_tx: payment_tx_hex.clone(),
        time_lock_duration: 0,
        time_lock: timelock,
        other_pub: taker_pub.clone(),
        secret_hash: secret_hash.clone(),
        amount: amount.clone(),
        swap_contract_address: maker_coin.swap_contract_address(),
        try_spv_proof_until: wait_until + 30,
        confirmations,
        unique_swap_data: Vec::new(),
        watcher_reward: None,
    };
    block_on(maker_coin.validate_taker_payment(input)).unwrap();
    let maker_spends_payment_args = SpendPaymentArgs {
        other_payment_tx: &payment_tx_hex,
        time_lock: timelock,
        other_pubkey: &taker_pub,
        secret,
        secret_hash: &secret_hash,
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let spend = block_on(maker_coin.send_maker_spends_taker_payment(maker_spends_payment_args)).unwrap();
    let spend_tx_hash = spend.tx_hash_as_bytes();
    let spend_tx_hex = spend.tx_hex();
    log!("Maker spends tx: {:?}", spend_tx_hash);

    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: spend_tx_hex,
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(maker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let maker_balance = block_on_f01(maker_coin.my_spendable_balance()).expect("Error on get maker balance");
    let taker_balance = block_on_f01(taker_coin.my_spendable_balance()).expect("Error on get taker balance");
    assert_eq!(maker_old_balance + amount.clone(), maker_balance);
    assert_eq!(taker_old_balance - amount, taker_balance);
}

#[test]
fn test_maker_refunds_payment() {
    let (_ctx, coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 10.into());
    let expected_balance = block_on_f01(coin.my_spendable_balance()).unwrap();
    assert_eq!(expected_balance, BigDecimal::from(10));

    let timelock = now_sec() - 200;
    let taker_pub = hex::decode("022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1a").unwrap();
    let secret_hash = &[1; 20];
    let amount = BigDecimal::from_str("0.2").unwrap();
    let maker_payment = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock: timelock,
        other_pubkey: &taker_pub,
        secret_hash,
        amount: amount.clone(),
        swap_contract_address: &coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let payment = block_on(coin.send_maker_payment(maker_payment)).unwrap();
    let payment_tx_hash = payment.tx_hash_as_bytes();
    let payment_tx_hex = payment.tx_hex();
    log!("Maker payment: {:?}", payment_tx_hash);

    let confirmations = 1;
    let requires_nota = false;
    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let check_every = 1;
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: payment_tx_hex.clone(),
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let balance_after_payment = block_on_f01(coin.my_spendable_balance()).unwrap();
    assert_eq!(expected_balance.clone() - amount, balance_after_payment);
    let maker_refunds_payment_args = RefundPaymentArgs {
        payment_tx: &payment_tx_hex,
        time_lock: timelock,
        other_pubkey: &taker_pub,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: secret_hash,
        },
        swap_contract_address: &coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let refund = block_on(coin.send_maker_refunds_payment(maker_refunds_payment_args)).unwrap();
    let refund_tx_hash = refund.tx_hash_as_bytes();
    let refund_tx_hex = refund.tx_hex();
    log!("Maker refunds payment: {:?}", refund_tx_hash);

    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: refund_tx_hex,
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let balance_after_refund = block_on_f01(coin.my_spendable_balance()).unwrap();
    assert_eq!(expected_balance, balance_after_refund);
}

#[test]
fn test_taker_refunds_payment() {
    let (_ctx, coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 10.into());
    let expected_balance = block_on_f01(coin.my_spendable_balance()).unwrap();
    assert_eq!(expected_balance, BigDecimal::from(10));

    let timelock = now_sec() - 200;
    let maker_pub = hex::decode("022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1a").unwrap();
    let secret_hash = &[1; 20];
    let amount = BigDecimal::from_str("0.2").unwrap();
    let taker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock: timelock,
        other_pubkey: &maker_pub,
        secret_hash,
        amount: amount.clone(),
        swap_contract_address: &coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let payment = block_on(coin.send_taker_payment(taker_payment_args)).unwrap();
    let payment_tx_hash = payment.tx_hash_as_bytes();
    let payment_tx_hex = payment.tx_hex();
    log!("Taker payment: {:?}", payment_tx_hash);

    let confirmations = 1;
    let requires_nota = false;
    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let check_every = 1;
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: payment_tx_hex.clone(),
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let balance_after_payment = block_on_f01(coin.my_spendable_balance()).unwrap();
    assert_eq!(expected_balance.clone() - amount, balance_after_payment);
    let taker_refunds_payment_args = RefundPaymentArgs {
        payment_tx: &payment_tx_hex,
        time_lock: timelock,
        other_pubkey: &maker_pub,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: secret_hash,
        },
        swap_contract_address: &coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let refund = block_on(coin.send_taker_refunds_payment(taker_refunds_payment_args)).unwrap();
    let refund_tx_hash = refund.tx_hash_as_bytes();
    let refund_tx_hex = refund.tx_hex();
    log!("Taker refunds payment: {:?}", refund_tx_hash);

    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: refund_tx_hex,
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let balance_after_refund = block_on_f01(coin.my_spendable_balance()).unwrap();
    assert_eq!(expected_balance, balance_after_refund);
}

#[test]
fn test_check_if_my_payment_sent() {
    let (_ctx, coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 10.into());
    let timelock = now_sec() - 200;
    let taker_pub = hex::decode("022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1a").unwrap();
    let secret_hash = &[1; 20];
    let amount = BigDecimal::from_str("0.2").unwrap();
    let maker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock: timelock,
        other_pubkey: &taker_pub,
        secret_hash,
        amount: amount.clone(),
        swap_contract_address: &coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let payment = block_on(coin.send_maker_payment(maker_payment_args)).unwrap();
    let payment_tx_hash = payment.tx_hash_as_bytes();
    let payment_tx_hex = payment.tx_hex();
    log!("Maker payment: {:?}", payment_tx_hash);

    let confirmations = 2;
    let requires_nota = false;
    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let check_every = 1;
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: payment_tx_hex,
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let search_from_block = block_on_f01(coin.current_block()).expect("!current_block") - 10;
    let if_my_payment_sent_args = CheckIfMyPaymentSentArgs {
        time_lock: timelock,
        other_pub: &taker_pub,
        secret_hash,
        search_from_block,
        swap_contract_address: &coin.swap_contract_address(),
        swap_unique_data: &[],
        amount: &amount,
        payment_instructions: &None,
    };
    let found = block_on(coin.check_if_my_payment_sent(if_my_payment_sent_args)).unwrap();
    assert_eq!(found, Some(payment));
}

#[test]
fn test_search_for_swap_tx_spend_taker_spent() {
    let (_ctx, maker_coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 10.into());
    let (_ctx, taker_coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 1.into());
    let search_from_block = block_on_f01(maker_coin.current_block()).expect("!current_block");

    let timelock = now_sec() - 200;
    let maker_pub = maker_coin.my_public_key().unwrap();
    let taker_pub = taker_coin.my_public_key().unwrap();
    let secret = &[1; 32];
    let secret_hash = dhash160(secret);
    let amount = BigDecimal::try_from(0.2).unwrap();
    let maker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock: timelock,
        other_pubkey: taker_pub,
        secret_hash: secret_hash.as_slice(),
        amount,
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let payment = block_on(maker_coin.send_maker_payment(maker_payment_args)).unwrap();
    let payment_tx_hash = payment.tx_hash_as_bytes();
    let payment_tx_hex = payment.tx_hex();
    log!("Maker payment: {:?}", payment_tx_hash);

    let confirmations = 1;
    let requires_nota = false;
    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let check_every = 1;
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: payment_tx_hex.clone(),
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(taker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();
    let taker_spends_payment_args = SpendPaymentArgs {
        other_payment_tx: &payment_tx_hex,
        time_lock: timelock,
        other_pubkey: maker_pub,
        secret,
        secret_hash: secret_hash.as_slice(),
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let spend = block_on(taker_coin.send_taker_spends_maker_payment(taker_spends_payment_args)).unwrap();
    let spend_tx_hash = spend.tx_hash_as_bytes();
    let spend_tx_hex = spend.tx_hex();
    log!("Taker spends tx: {:?}", spend_tx_hash);

    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: spend_tx_hex,
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(taker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock: timelock,
        other_pub: taker_pub,
        secret_hash: secret_hash.as_slice(),
        tx: &payment_tx_hex,
        search_from_block,
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let actual = block_on(maker_coin.search_for_swap_tx_spend_my(search_input));
    let expected = Ok(Some(FoundSwapTxSpend::Spent(spend)));
    assert_eq!(actual, expected);
}

#[test]
fn test_search_for_swap_tx_spend_maker_refunded() {
    let (_ctx, maker_coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 10.into());
    let search_from_block = block_on_f01(maker_coin.current_block()).expect("!current_block");

    let timelock = now_sec() - 200;
    let taker_pub = hex::decode("022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1a").unwrap();
    let secret = &[1; 32];
    let secret_hash = &*dhash160(secret);
    let amount = BigDecimal::try_from(0.2).unwrap();
    let maker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock: timelock,
        other_pubkey: &taker_pub,
        secret_hash,
        amount,
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let payment = block_on(maker_coin.send_maker_payment(maker_payment_args)).unwrap();
    let payment_tx_hash = payment.tx_hash_as_bytes();
    let payment_tx_hex = payment.tx_hex();
    log!("Maker payment: {:?}", payment_tx_hash);

    let confirmations = 1;
    let requires_nota = false;
    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let check_every = 1;
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: payment_tx_hex.clone(),
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(maker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();
    let maker_refunds_payment_args = RefundPaymentArgs {
        payment_tx: &payment_tx_hex,
        time_lock: timelock,
        other_pubkey: &taker_pub,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: secret_hash,
        },
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let refund = block_on(maker_coin.send_maker_refunds_payment(maker_refunds_payment_args)).unwrap();
    let refund_tx_hash = refund.tx_hash_as_bytes();
    let refund_tx_hex = refund.tx_hex();
    log!("Maker refunds tx: {:?}", refund_tx_hash);

    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: refund_tx_hex,
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(maker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock: timelock,
        other_pub: &taker_pub,
        secret_hash,
        tx: &payment_tx_hex,
        search_from_block,
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let actual = block_on(maker_coin.search_for_swap_tx_spend_my(search_input));
    let expected = Ok(Some(FoundSwapTxSpend::Refunded(refund)));
    assert_eq!(actual, expected);
}

#[test]
fn test_search_for_swap_tx_spend_not_spent() {
    let (_ctx, maker_coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 10.into());
    let search_from_block = block_on_f01(maker_coin.current_block()).expect("!current_block");

    let timelock = now_sec() - 200;
    let taker_pub = hex::decode("022b00078841f37b5d30a6a1defb82b3af4d4e2d24dd4204d41f0c9ce1e875de1a").unwrap();
    let secret = &[1; 32];
    let secret_hash = &*dhash160(secret);
    let amount = BigDecimal::try_from(0.2).unwrap();
    let maker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock: timelock,
        other_pubkey: &taker_pub,
        secret_hash,
        amount,
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let payment = block_on(maker_coin.send_maker_payment(maker_payment_args)).unwrap();
    let payment_tx_hash = payment.tx_hash_as_bytes();
    let payment_tx_hex = payment.tx_hex();
    log!("Maker payment: {:?}", payment_tx_hash);

    let confirmations = 1;
    let requires_nota = false;
    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let check_every = 1;
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: payment_tx_hex.clone(),
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(maker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock: timelock,
        other_pub: &taker_pub,
        secret_hash,
        tx: &payment_tx_hex,
        search_from_block,
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let actual = block_on(maker_coin.search_for_swap_tx_spend_my(search_input));
    // maker payment hasn't been spent or refunded yet
    assert_eq!(actual, Ok(None));
}

#[test]
fn test_wait_for_tx_spend() {
    let (_ctx, maker_coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 10.into());
    let (_ctx, taker_coin, _priv_key) = generate_qrc20_coin_with_random_privkey("QICK", 20.into(), 1.into());
    let from_block = block_on_f01(maker_coin.current_block()).expect("!current_block");

    let timelock = now_sec() - 200;
    let maker_pub = maker_coin.my_public_key().unwrap();
    let taker_pub = taker_coin.my_public_key().unwrap();
    let secret = &[1; 32];
    let secret_hash = dhash160(secret);
    let amount = BigDecimal::try_from(0.2).unwrap();
    let maker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock: timelock,
        other_pubkey: taker_pub,
        secret_hash: secret_hash.as_slice(),
        amount,
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let payment = block_on(maker_coin.send_maker_payment(maker_payment_args)).unwrap();
    let payment_tx_hash = payment.tx_hash_as_bytes();
    let payment_tx_hex = payment.tx_hex();
    log!("Maker payment: {:?}", payment_tx_hash);

    let confirmations = 1;
    let requires_nota = false;
    let wait_until = wait_until_sec(40); // timeout if test takes more than 40 seconds to run
    let check_every = 1;
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: payment_tx_hex.clone(),
        confirmations,
        requires_nota,
        wait_until,
        check_every,
    };
    block_on_f01(taker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    // first try to check if the wait_for_htlc_tx_spend() returns an error correctly
    let wait_until = wait_until_sec(5);
    let tx_err = block_on(maker_coin.wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
        tx_bytes: &payment_tx_hex,
        secret_hash: &[],
        wait_until,
        from_block,
        swap_contract_address: &maker_coin.swap_contract_address(),
        check_every: TAKER_PAYMENT_SPEND_SEARCH_INTERVAL,
        watcher_reward: false,
    }))
    .expect_err("Expected 'Waited too long' error");

    let err = tx_err.get_plain_text_format();
    log!("error: {:?}", err);
    assert!(err.contains("Waited too long"));

    /// Also spends the maker payment and try to check if the wait_for_htlc_tx_spend() returns the correct tx
    static SPEND_TX: Mutex<Option<TransactionEnum>> = Mutex::new(None);

    let maker_pub_c = maker_pub.to_vec();
    let payment_hex = payment_tx_hex.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(5));
        let taker_spends_payment_args = SpendPaymentArgs {
            other_payment_tx: &payment_hex,
            time_lock: timelock,
            other_pubkey: &maker_pub_c,
            secret,
            secret_hash: secret_hash.as_slice(),
            swap_contract_address: &taker_coin.swap_contract_address(),
            swap_unique_data: &[],
            watcher_reward: false,
        };
        let spend = block_on(taker_coin.send_taker_spends_maker_payment(taker_spends_payment_args)).unwrap();
        *SPEND_TX.lock().unwrap() = Some(spend);
    });

    let wait_until = wait_until_sec(120);
    let found = block_on(maker_coin.wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
        tx_bytes: &payment_tx_hex,
        secret_hash: &[],
        wait_until,
        from_block,
        swap_contract_address: &maker_coin.swap_contract_address(),
        check_every: TAKER_PAYMENT_SPEND_SEARCH_INTERVAL,
        watcher_reward: false,
    }))
    .unwrap();

    assert_eq!(Some(found), *SPEND_TX.lock().unwrap());
}

#[test]
fn test_check_balance_on_order_post_base_coin_locked() {
    let bob_priv_key = random_secp256k1_secret();
    let alice_priv_key = random_secp256k1_secret();
    let timeout = 30; // timeout if test takes more than 80 seconds to run

    // fill the Bob address by 0.05 Qtum
    let (_ctx, coin) = qrc20_coin_from_privkey("QICK", bob_priv_key);
    let my_address = coin.my_address().expect("!my_address");
    fill_address(&coin, &my_address, BigDecimal::try_from(0.05).unwrap(), timeout);
    // fill the Bob address by 10 MYCOIN
    let (_ctx, coin) = utxo_coin_from_privkey("MYCOIN", bob_priv_key);
    let my_address = coin.my_address().expect("!my_address");
    fill_address(&coin, &my_address, 10.into(), timeout);

    // fill the Alice address by 10 Qtum and 10 QICK
    let (_ctx, coin) = qrc20_coin_from_privkey("QICK", alice_priv_key);
    let my_address = coin.my_address().expect("!my_address");
    fill_address(&coin, &my_address, 10.into(), timeout);
    fill_qrc20_address(&coin, 10.into(), timeout);
    // fill the Alice address by 10 MYCOIN
    let (_ctx, coin) = utxo_coin_from_privkey("MYCOIN", alice_priv_key);
    let my_address = coin.my_address().expect("!my_address");
    fill_address(&coin, &my_address, 10.into(), timeout);

    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
    let qick_contract_address = format!("{:#02x}", unsafe { QICK_TOKEN_ADDRESS.expect("!QICK_TOKEN_ADDRESS") });
    let coins = json!([
        {"coin":"MYCOIN","asset":"MYCOIN","required_confirmations":0,"txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
        {"coin":"QICK","required_confirmations":1,"pubtype": 120,"p2shtype": 50,"wiftype": 128,"mm2": 1,"mature_confirmations": 500,"confpath": confpath,"network":"regtest",
         "protocol":{"type":"QRC20","protocol_data":{"platform":"QTUM","contract_address":qick_contract_address}}},
    ]);

    let mut mm_bob = MarketMakerIt::start(
        json! ({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var ("BOB_TRADE_IP") .ok(),
            "rpcip": env::var ("BOB_TRADE_IP") .ok(),
            "canbind": env::var ("BOB_TRADE_PORT") .ok().map(|s| s.parse::<i64>().unwrap()),
            "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
            "coins": coins,
            "i_am_seed": true,
            "rpc_password": "pass",
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm_dump(&mm_bob.log_path);
    log!("Log path: {}", mm_bob.log_path.display());
    block_on(mm_bob.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();
    block_on(enable_native(&mm_bob, "MYCOIN", &[], None));
    block_on(enable_qrc20_native(&mm_bob, "QICK"));

    // start alice
    let mut mm_alice = MarketMakerIt::start(
        json! ({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var ("BOB_TRADE_IP") .ok(),
            "rpcip": env::var ("BOB_TRADE_IP") .ok(),
            "canbind": env::var ("BOB_TRADE_PORT") .ok().map(|s| s.parse::<i64>().unwrap()),
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "seednodes": [mm_bob.ip.to_string()],
            "rpc_password": "pass",
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm_dump(&mm_alice.log_path);
    log!("Log path: {}", mm_alice.log_path.display());
    block_on(mm_alice.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();
    block_on(enable_native(&mm_alice, "MYCOIN", &[], None));
    block_on(enable_qrc20_native(&mm_alice, "QICK"));

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "setprice",
        "base": "QICK",
        "rel": "MYCOIN",
        "price": 1,
        "volume": 1,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);

    log!("Give Bob 2 seconds to import Alice order");
    thread::sleep(Duration::from_secs(2));

    // Buy QICK and thus lock ~ 0.05 Qtum
    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "QICK",
        "rel": "MYCOIN",
        "price": 1,
        "volume": 1,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    log!("Give swaps some time to start");
    thread::sleep(Duration::from_secs(4));

    // QRC20 balance is sufficient, but most of the balance is locked
    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "sell",
        "base": "MYCOIN",
        "rel": "QICK",
        "price": 1,
        "volume": 1,
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "!sell success but should be error: {}", rc.1);
}

/// Test the following statements:
/// * `max_taker_vol` returns an expected volume. This expected volume is calculated according to the instructions described in the comments to the [`lp_swap::taker_swap::max_taker_vol`] function;
/// * If we issue a `sell` request it never fails;
/// * Our balance is sufficient to send `TakerFee` and `TakerPayment` with the expected volume;
/// * Zero left on QTUM balance.
///
/// Please note this function should be called before the Qtum balance is filled.
fn test_get_max_taker_vol_and_trade_with_dynamic_trade_fee(coin: QtumCoin, priv_key: &[u8]) {
    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
    let coins = json! ([
        {"coin":"MYCOIN","asset":"MYCOIN","txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
        {"coin":"QTUM","decimals":8,"pubtype":120,"p2shtype":110,"wiftype":128,"txfee":0,"txfee_volatility_percent":0.1,
        "mm2":1,"mature_confirmations":500,"network":"regtest","confpath":confpath,"protocol":{"type":"UTXO"}, "dust": 72800},
    ]);
    let mut mm = MarketMakerIt::start(
        json! ({
            "gui": "nogui",
            "netid": 9000u32,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm.log_path);
    block_on(mm.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "QTUM", &[], None)));

    let qtum_balance = block_on_f01(coin.my_spendable_balance()).expect("!my_balance");
    let qtum_min_tx_amount = MmNumber::from("0.000728");

    // - `max_possible = balance - locked_amount`, where `locked_amount = 0`
    // - `max_trade_fee = trade_fee(balance)`
    // Please note if we pass the exact value, the `get_sender_trade_fee` will fail with 'Not sufficient balance: Couldn't collect enough value from utxos'.
    // So we should deduct trade fee from the output.
    let max_trade_fee = block_on(coin.get_sender_trade_fee(
        TradePreimageValue::UpperBound(qtum_balance.clone()),
        FeeApproxStage::TradePreimageMax,
    ))
    .expect("!get_sender_trade_fee");
    let max_trade_fee = max_trade_fee.amount.to_decimal();
    log!("max_trade_fee: {}", max_trade_fee);

    // - `max_possible_2 = balance - locked_amount - max_trade_fee`, where `locked_amount = 0`
    let max_possible_2 = &qtum_balance - &max_trade_fee;
    // - `max_dex_fee = dex_fee(max_possible_2)`
    let max_dex_fee = DexFee::new_from_taker_coin(&coin, "MYCOIN", &MmNumber::from(max_possible_2));
    log!("max_dex_fee: {:?}", max_dex_fee.fee_amount().to_fraction());

    // - `max_fee_to_send_taker_fee = fee_to_send_taker_fee(max_dex_fee)`
    // `taker_fee` is sent using general withdraw, and the fee get be obtained from withdraw result
    let max_fee_to_send_taker_fee =
        block_on(coin.get_fee_to_send_taker_fee(max_dex_fee, FeeApproxStage::TradePreimageMax))
            .expect("!get_fee_to_send_taker_fee");
    let max_fee_to_send_taker_fee = max_fee_to_send_taker_fee.amount.to_decimal();
    log!("max_fee_to_send_taker_fee: {}", max_fee_to_send_taker_fee);

    // and then calculate `min_max_val = balance - locked_amount - max_trade_fee - max_fee_to_send_taker_fee - dex_fee(max_val)` using `max_taker_vol_from_available()`
    // where `available = balance - locked_amount - max_trade_fee - max_fee_to_send_taker_fee`
    let available = &qtum_balance - &max_trade_fee - &max_fee_to_send_taker_fee;
    log!("total_available: {}", available);
    let expected_max_taker_vol =
        max_taker_vol_from_available(MmNumber::from(available), "QTUM", "MYCOIN", &qtum_min_tx_amount)
            .expect("max_taker_vol_from_available");
    let real_dex_fee = DexFee::new_from_taker_coin(&coin, "MYCOIN", &expected_max_taker_vol).fee_amount();
    log!("real_max_dex_fee: {:?}", real_dex_fee.to_fraction());

    // check if the actual max_taker_vol equals to the expected
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "max_taker_vol",
        "coin": "QTUM",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!max_taker_vol: {}", rc.1);
    let json: Json = json::from_str(&rc.1).unwrap();
    assert_eq!(
        json["result"],
        json::to_value(expected_max_taker_vol.to_fraction()).unwrap()
    );

    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "QTUM",
        "rel": "MYCOIN",
        "price": 1u64,
        "volume": expected_max_taker_vol.to_fraction(),
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);

    block_on(mm.stop()).unwrap();

    let timelock = now_sec() - 200;
    let secret_hash = &[0; 20];

    let dex_fee = DexFee::new_from_taker_coin(&coin, "MYCOIN", &expected_max_taker_vol);
    let _taker_fee_tx = block_on(coin.send_taker_fee(dex_fee, &[], timelock)).expect("!send_taker_fee");
    let taker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock: timelock,
        other_pubkey: &DEX_FEE_ADDR_RAW_PUBKEY,
        secret_hash,
        amount: expected_max_taker_vol.to_decimal(),
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };

    let _taker_payment_tx = block_on(coin.send_taker_payment(taker_payment_args)).expect("!send_taker_payment");

    let my_balance = block_on_f01(coin.my_spendable_balance()).expect("!my_balance");
    assert_eq!(
        my_balance,
        BigDecimal::from(0u32),
        "NOT AN ERROR, but it would be better if the balance remained zero"
    );
}

/// Generate the Qtum coin with a random balance and start the `test_get_max_taker_vol_and_trade_with_dynamic_trade_fee` test.
#[test]
fn test_max_taker_vol_dynamic_trade_fee() {
    wait_for_estimate_smart_fee(60).expect("!wait_for_estimate_smart_fee");
    // generate QTUM coin with the dynamic fee and fill the wallet by 2 Qtums
    let (_ctx, coin, priv_key) = generate_qtum_coin_with_random_privkey("QTUM", 2.into(), Some(0));
    let my_address = coin.my_address().expect("!my_address");
    let mut rng = rand6::thread_rng();
    let mut qtum_balance = BigDecimal::from(2);
    let mut qtum_balance_steps = "2".to_owned();
    for _ in 0..4 {
        let amount = rng.gen_range(100000, 10000000);
        let amount = big_decimal_from_sat(amount, 8);
        qtum_balance_steps = format!("{qtum_balance_steps} + {amount}");
        qtum_balance = &qtum_balance + &amount;
        fill_address(&coin, &my_address, amount, 30);
    }
    log!("QTUM balance {} = {}", qtum_balance, qtum_balance_steps);

    test_get_max_taker_vol_and_trade_with_dynamic_trade_fee(coin, &priv_key);
}

/// This is a special of a set of Qtum inputs where the `max_taker_payment` returns a volume such that
/// if the volume is passed into the `sell` request, the request will fail with `Not sufficient balance`.
/// This may be due to the `get_sender_trade_fee(balance)` called from `max_taker_payment` doesn't include the change output,
/// but the `get_sender_trade_fee(max_volume)` called from `sell` includes the change output.
/// To sum up, `get_sender_trade_fee(balance) < get_sender_trade_fee(max_volume)`, where `balance > max_volume`.
/// This test checks if the fee returned from `get_sender_trade_fee` should include the change output anyway.
#[test]
fn test_trade_preimage_fee_includes_change_output_anyway() {
    wait_for_estimate_smart_fee(60).expect("!wait_for_estimate_smart_fee");
    // generate QTUM coin with the dynamic fee and fill the wallet by 2 Qtums
    let (_ctx, coin, priv_key) = generate_qtum_coin_with_random_privkey("QTUM", 2.into(), Some(0));
    let my_address = coin.my_address().expect("!my_address");
    let mut qtum_balance = BigDecimal::from_str("2.2839365").expect("!BigDecimal::from_str");
    let amounts = vec!["0.09968324", "0.06979112", "0.09229586", "0.02216628"];
    for amount in amounts {
        let amount = BigDecimal::from_str(amount).expect("!BigDecimal::from_str");
        qtum_balance = &qtum_balance + &amount;
        fill_address(&coin, &my_address, amount, 30);
    }

    test_get_max_taker_vol_and_trade_with_dynamic_trade_fee(coin, &priv_key);
}
#[test]
fn test_trade_preimage_not_sufficient_base_coin_balance_for_ticker() {
    wait_for_estimate_smart_fee(60).expect("!wait_for_estimate_smart_fee");
    // generate QRC20 coin(QICK) fill the wallet with 10 QICK
    // fill QTUM balance with 0.005 QTUM which is will be than expected transaction fee just to get our desired output for this test.
    let qick_balance = MmNumber::from("10").to_decimal();
    let qtum_balance = MmNumber::from("0.005").to_decimal();
    let (_, _, priv_key) = generate_qrc20_coin_with_random_privkey("QICK", qtum_balance.clone(), qick_balance);

    let qick_contract_address = format!("{:#02x}", unsafe { QICK_TOKEN_ADDRESS.expect("!QICK_TOKEN_ADDRESS") });
    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
    let coins = json! ([
        {"coin":"MYCOIN","asset":"MYCOIN","txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
        {"coin":"QICK","required_confirmations":1,"pubtype": 120,"p2shtype": 50,"wiftype": 128,"mm2": 1,"mature_confirmations": 500,"confpath": confpath,"network":"regtest",
         "protocol":{"type":"QRC20","protocol_data":{"platform":"QTUM","contract_address":qick_contract_address}}},
    ]);
    let mut mm = MarketMakerIt::start(
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm.log_path);
    block_on(mm.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "QICK", &[], None)));

    // txfee > 0, amount = 0.005 => required = txfee + amount > 0.005,
    // but balance = 0.005
    // This RPC call should fail because [`QtumCoin::get_sender_trade_fee`] will try to generate a dummy transaction due to the dynamic tx fee,
    // and this operation must fail with the [`TradePreimageError::NotSufficientBaseCoinBalance`].
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "QICK",
            "rel": "MYCOIN",
            "swap_method": "setprice",
            "price": 10,
            "volume": 1,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    let actual: RpcErrorResponse<trade_preimage_error::NotSufficientBalance> = json::from_str(&rc.1).unwrap();
    assert_eq!(actual.error_type, "NotSufficientBaseCoinBalance");
    let data = actual.error_data.expect("Expected 'error_data'");
    assert_eq!(data.coin, "QTUM");
    assert_eq!(data.available, qtum_balance);
    assert!(data.required > qtum_balance);
}

#[test]
fn test_trade_preimage_dynamic_fee_not_sufficient_balance() {
    wait_for_estimate_smart_fee(60).expect("!wait_for_estimate_smart_fee");
    // generate QTUM coin with the dynamic fee and fill the wallet by 0.5 Qtums
    let qtum_balance = MmNumber::from("0.5").to_decimal();
    let (_ctx, _coin, priv_key) = generate_qtum_coin_with_random_privkey("QTUM", qtum_balance.clone(), Some(0));

    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
    let coins = json! ([
        {"coin":"MYCOIN","asset":"MYCOIN","txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
        {"coin":"QTUM","decimals":8,"pubtype":120,"p2shtype":110,"wiftype":128,"txfee":0,"txfee_volatility_percent":0.1,
        "mm2":1,"mature_confirmations":500,"network":"regtest","confpath":confpath,"protocol":{"type":"UTXO"}},
    ]);
    let mut mm = MarketMakerIt::start(
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm.log_path);
    block_on(mm.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "QTUM", &[], None)));

    // txfee > 0, amount = 0.5 => required = txfee + amount > 0.5,
    // but balance = 0.5
    // This RPC call should fail because [`QtumCoin::get_sender_trade_fee`] will try to generate a dummy transaction due to the dynamic tx fee,
    // and this operation must fail with the [`TradePreimageError::NotSufficientBalance`].
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "QTUM",
            "rel": "MYCOIN",
            "swap_method": "setprice",
            "price": 1,
            "volume": qtum_balance,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    let actual: RpcErrorResponse<trade_preimage_error::NotSufficientBalance> = json::from_str(&rc.1).unwrap();
    assert_eq!(actual.error_type, "NotSufficientBalance");
    let data = actual.error_data.expect("Expected 'error_data'");
    assert_eq!(data.coin, "QTUM");
    assert_eq!(data.available, qtum_balance);
    assert!(data.required > qtum_balance);
}

/// If we try to deduct a transaction fee from `output = 0.00073`, the remaining value less than `dust = 0.000728`,
/// so we have to receive the `NotSufficientBalance` error.
#[test]
fn test_trade_preimage_deduct_fee_from_output_failed() {
    wait_for_estimate_smart_fee(60).expect("!wait_for_estimate_smart_fee");
    // generate QTUM coin with the dynamic fee and fill the wallet by 0.00073 Qtums (that is little greater than dust 0.000728)
    let qtum_balance = MmNumber::from("0.00073").to_decimal();
    let (_ctx, _coin, priv_key) = generate_qtum_coin_with_random_privkey("QTUM", qtum_balance.clone(), Some(0));

    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
    let coins = json! ([
        {"coin":"MYCOIN","asset":"MYCOIN","txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
        {"coin":"QTUM","decimals":8,"pubtype":120,"p2shtype":110,"wiftype":128,"txfee":0,"txfee_volatility_percent":0.1,
        "mm2":1,"mature_confirmations":500,"network":"regtest","confpath":confpath,"protocol":{"type":"UTXO"}},
    ]);
    let mut mm = MarketMakerIt::start(
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm.log_path);
    block_on(mm.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "QTUM", &[], None)));

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "QTUM",
            "rel": "MYCOIN",
            "swap_method": "setprice",
            "price": 1,
            "max": true,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    let actual: RpcErrorResponse<trade_preimage_error::NotSufficientBalance> = json::from_str(&rc.1).unwrap();
    assert_eq!(actual.error_type, "NotSufficientBalance");
    let trade_preimage_error::NotSufficientBalance {
        coin: actual_coin,
        available: actual_available,
        required: actual_required,
        ..
    } = actual.error_data.expect("Expected NotSufficientBalance error data");
    assert_eq!(actual_coin, "QTUM");
    assert_eq!(actual_available, qtum_balance);
    assert!(actual_required > qtum_balance);
}

#[test]
fn test_segwit_native_balance() {
    wait_for_estimate_smart_fee(60).expect("!wait_for_estimate_smart_fee");
    // generate QTUM coin with the dynamic fee and fill the wallet by 0.5 Qtums
    let (_ctx, _coin, priv_key) =
        generate_segwit_qtum_coin_with_random_privkey("QTUM", BigDecimal::try_from(0.5).unwrap(), Some(0));

    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
    let coins = json! ([
        {"coin":"QTUM","decimals":8,"pubtype":120,"p2shtype":110,"wiftype":128,"segwit":true,"txfee":0,"txfee_volatility_percent":0.1,
        "mm2":1,"mature_confirmations":500,"network":"regtest","confpath":confpath,"protocol":{"type":"UTXO"},"bech32_hrp":"qcrt","address_format":{"format":"segwit"}},
    ]);
    let mut mm = MarketMakerIt::start(
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm.log_path);
    block_on(mm.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    let enable_res = block_on(enable_native(&mm, "QTUM", &[], None));
    let expected_balance: BigDecimal = "0.5".parse().unwrap();
    assert_eq!(enable_res.balance, expected_balance);

    let my_balance = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "my_balance",
        "coin": "QTUM",
    })))
    .unwrap();
    let json: Json = json::from_str(&my_balance.1).unwrap();
    let my_balance = json["balance"].as_str().unwrap();
    assert_eq!(my_balance, "0.5");
    let my_unspendable_balance = json["unspendable_balance"].as_str().unwrap();
    assert_eq!(my_unspendable_balance, "0");
}

#[test]
fn test_withdraw_and_send_from_segwit() {
    wait_for_estimate_smart_fee(60).expect("!wait_for_estimate_smart_fee");
    // generate QTUM coin with the dynamic fee and fill the wallet by 0.7 Qtums
    let (_ctx, _coin, priv_key) =
        generate_segwit_qtum_coin_with_random_privkey("QTUM", BigDecimal::try_from(0.7).unwrap(), Some(0));

    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
    let coins = json! ([
        {"coin":"QTUM","decimals":8,"pubtype":120,"p2shtype":110,"wiftype":128,"segwit":true,"txfee":0,"txfee_volatility_percent":0.1,
        "mm2":1,"mature_confirmations":500,"network":"regtest","confpath":confpath,"protocol":{"type":"UTXO"},"bech32_hrp":"qcrt","address_format":{"format":"segwit"}},
    ]);
    let mut mm = MarketMakerIt::start(
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm.log_path);
    block_on(mm.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    log!("{:?}", block_on(enable_native(&mm, "QTUM", &[], None)));

    // Send from Segwit Address to Segwit Address
    withdraw_and_send(&mm, "QTUM", "qcrt1q6pwxl4na4a363mgmrw8tjyppdcwuyfmat836dd", 0.2);

    // Send from Segwit Address to Legacy Address
    withdraw_and_send(&mm, "QTUM", "qVgbLqYPvKN5zH2eEJ6Jh8cjbUVx851yxV", 0.2);

    // Send from Segwit Address to P2WSH Address
    withdraw_and_send(
        &mm,
        "QTUM",
        "qcrt1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q2uwvdw",
        0.2,
    );

    block_on(mm.stop()).unwrap();
}

#[test]
fn test_withdraw_and_send_legacy_to_segwit() {
    wait_for_estimate_smart_fee(60).expect("!wait_for_estimate_smart_fee");
    // generate QTUM coin with the dynamic fee and fill the wallet by 0.7 Qtums
    let (_ctx, _coin, priv_key) =
        generate_qtum_coin_with_random_privkey("QTUM", BigDecimal::try_from(0.7).unwrap(), Some(0));

    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
    let coins = json! ([
        {"coin":"QTUM","decimals":8,"pubtype":120,"p2shtype":110,"wiftype":128,"segwit":true,"txfee":0,"txfee_volatility_percent":0.1,
        "mm2":1,"mature_confirmations":500,"network":"regtest","confpath":confpath,"protocol":{"type":"UTXO"},"bech32_hrp":"qcrt"},
    ]);
    let mut mm = MarketMakerIt::start(
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm.log_path);
    block_on(mm.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    log!("{:?}", block_on(enable_native(&mm, "QTUM", &[], None)));

    // Send from Legacy Address to Segwit Address
    withdraw_and_send(&mm, "QTUM", "qcrt1q6pwxl4na4a363mgmrw8tjyppdcwuyfmat836dd", 0.2);

    // Send from Legacy Address to P2WSH Address
    withdraw_and_send(
        &mm,
        "QTUM",
        "qcrt1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q2uwvdw",
        0.2,
    );

    block_on(mm.stop()).unwrap();
}

#[test]
fn test_search_for_segwit_swap_tx_spend_native_was_refunded_maker() {
    wait_for_estimate_smart_fee(60).expect("!wait_for_estimate_smart_fee");
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run
    let (_ctx, coin, _) = generate_segwit_qtum_coin_with_random_privkey("QTUM", 1000u64.into(), Some(0));
    let my_public_key = coin.my_public_key().unwrap();

    let time_lock = now_sec() - 3600;
    let maker_payment = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock,
        other_pubkey: my_public_key,
        secret_hash: &[0; 20],
        amount: 1u64.into(),
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let tx = block_on(coin.send_maker_payment(maker_payment)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();
    let maker_refunds_payment_args = RefundPaymentArgs {
        payment_tx: &tx.tx_hex(),
        time_lock,
        other_pubkey: my_public_key,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: &[0; 20],
        },
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let refund_tx = block_on(coin.send_maker_refunds_payment(maker_refunds_payment_args)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: refund_tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: coin.my_public_key().unwrap(),
        secret_hash: &[0; 20],
        tx: &tx.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let found = block_on(coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();
    assert_eq!(FoundSwapTxSpend::Refunded(refund_tx), found);
}

#[test]
fn test_search_for_segwit_swap_tx_spend_native_was_refunded_taker() {
    wait_for_estimate_smart_fee(60).expect("!wait_for_estimate_smart_fee");
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run
    let (_ctx, coin, _) = generate_segwit_qtum_coin_with_random_privkey("QTUM", 1000u64.into(), Some(0));
    let my_public_key = coin.my_public_key().unwrap();

    let time_lock = now_sec() - 3600;
    let taker_payment = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock,
        other_pubkey: my_public_key,
        secret_hash: &[0; 20],
        amount: 1u64.into(),
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let tx = block_on(coin.send_taker_payment(taker_payment)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();
    let maker_refunds_payment_args = RefundPaymentArgs {
        payment_tx: &tx.tx_hex(),
        time_lock,
        other_pubkey: my_public_key,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: &[0; 20],
        },
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let refund_tx = block_on(coin.send_maker_refunds_payment(maker_refunds_payment_args)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: refund_tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: coin.my_public_key().unwrap(),
        secret_hash: &[0; 20],
        tx: &tx.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let found = block_on(coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();
    assert_eq!(FoundSwapTxSpend::Refunded(refund_tx), found);
}

pub async fn enable_native_segwit(mm: &MarketMakerIt, coin: &str) -> Json {
    let native = mm
        .rpc(&json! ({
            "userpass": mm.userpass,
            "method": "enable",
            "coin": coin,
            "address_format": {
                "format": "segwit",
            },
            "mm2": 1,
        }))
        .await
        .unwrap();
    assert_eq!(native.0, StatusCode::OK, "'enable' failed: {}", native.1);
    json::from_str(&native.1).unwrap()
}

#[test]
#[ignore]
fn segwit_address_in_the_orderbook() {
    wait_for_estimate_smart_fee(60).expect("!wait_for_estimate_smart_fee");
    // generate QTUM coin with the dynamic fee and fill the wallet by 0.5 Qtums
    let (_ctx, coin, priv_key) =
        generate_qtum_coin_with_random_privkey("QTUM", BigDecimal::try_from(0.5).unwrap(), Some(0));

    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
    let coins = json! ([
        {"coin":"QTUM","decimals":8,"pubtype":120,"p2shtype":110,"wiftype":128,"segwit":true,"txfee":0,"txfee_volatility_percent":0.1,
        "mm2":1,"mature_confirmations":500,"network":"regtest","confpath":confpath,"protocol":{"type":"UTXO"},"bech32_hrp":"qcrt"},
        {"coin":"MYCOIN","asset":"MYCOIN","txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
    ]);
    let mut mm = MarketMakerIt::start(
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm.log_path);
    block_on(mm.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    let enable_qtum_res = block_on(enable_native_segwit(&mm, "QTUM"));
    let enable_qtum_res: CoinInitResponse = json::from_value(enable_qtum_res).unwrap();
    let segwit_addr = enable_qtum_res.address;

    fill_address(&coin, &segwit_addr, 1000.into(), 30);

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));

    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "QTUM",
        "rel": "MYCOIN",
        "price": 1,
        "volume": "1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let orderbook = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "orderbook",
        "base": "QTUM",
        "rel": "MYCOIN",
    })))
    .unwrap();
    assert!(orderbook.0.is_success(), "!orderbook: {}", rc.1);

    let orderbook: OrderbookResponse = json::from_str(&orderbook.1).unwrap();
    assert_eq!(orderbook.asks[0].entry.coin, "QTUM");
    assert_eq!(orderbook.asks[0].entry.address, segwit_addr);
    block_on(mm.stop()).unwrap();
}

#[test]
fn test_trade_qrc20() {
    trade_base_rel(("QICK", "QORTY"));
}

#[test]
fn trade_test_with_maker_segwit() {
    trade_base_rel(("QTUM", "MYCOIN"));
}

#[test]
fn trade_test_with_taker_segwit() {
    trade_base_rel(("MYCOIN", "QTUM"));
}

#[test]
#[ignore]
fn test_trade_qrc20_utxo() {
    trade_base_rel(("QICK", "MYCOIN"));
}

#[test]
#[ignore]
fn test_trade_utxo_qrc20() {
    trade_base_rel(("MYCOIN", "QICK"));
}

#[test]
fn test_send_standard_taker_fee_qtum() {
    // generate QTUM coin with the dynamic fee and fill the wallet by 0.5 Qtums
    let (_ctx, coin, _priv_key) =
        generate_segwit_qtum_coin_with_random_privkey("QTUM", BigDecimal::try_from(0.5).unwrap(), Some(0));

    let amount = BigDecimal::from_str("0.01").unwrap();
    let tx = block_on(coin.send_taker_fee(DexFee::Standard(amount.clone().into()), &[], 0)).expect("!send_taker_fee");
    assert!(matches!(tx, TransactionEnum::UtxoTx(_)), "Expected UtxoTx");

    block_on(coin.validate_fee(ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: coin.my_public_key().unwrap(),
        dex_fee: &DexFee::Standard(amount.into()),
        min_block_number: 0,
        uuid: &[],
    }))
    .expect("!validate_fee");
}

#[test]
fn test_send_taker_fee_with_burn_qtum() {
    // generate QTUM coin with the dynamic fee and fill the wallet by 0.5 Qtums
    let (_ctx, coin, _priv_key) =
        generate_segwit_qtum_coin_with_random_privkey("QTUM", BigDecimal::try_from(0.5).unwrap(), Some(0));

    let fee_amount = BigDecimal::from_str("0.0075").unwrap();
    let burn_amount = BigDecimal::from_str("0.0025").unwrap();
    let tx = block_on(coin.send_taker_fee(
        DexFee::WithBurn {
            fee_amount: fee_amount.clone().into(),
            burn_amount: burn_amount.clone().into(),
            burn_destination: DexFeeBurnDestination::PreBurnAccount,
        },
        &[],
        0,
    ))
    .expect("!send_taker_fee");
    assert!(matches!(tx, TransactionEnum::UtxoTx(_)), "Expected UtxoTx");

    block_on(coin.validate_fee(ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: coin.my_public_key().unwrap(),
        dex_fee: &DexFee::WithBurn {
            fee_amount: fee_amount.into(),
            burn_amount: burn_amount.into(),
            burn_destination: DexFeeBurnDestination::PreBurnAccount,
        },
        min_block_number: 0,
        uuid: &[],
    }))
    .expect("!validate_fee");
}

#[test]
fn test_send_taker_fee_qrc20() {
    let (_ctx, coin, _priv_key) = generate_qrc20_coin_with_random_privkey(
        "QICK",
        BigDecimal::try_from(0.5).unwrap(),
        BigDecimal::try_from(0.5).unwrap(),
    );

    let amount = BigDecimal::from_str("0.01").unwrap();
    let tx = block_on(coin.send_taker_fee(DexFee::Standard(amount.clone().into()), &[], 0)).expect("!send_taker_fee");
    assert!(matches!(tx, TransactionEnum::UtxoTx(_)), "Expected UtxoTx");

    block_on(coin.validate_fee(ValidateFeeArgs {
        fee_tx: &tx,
        expected_sender: coin.my_public_key().unwrap(),
        dex_fee: &DexFee::Standard(amount.into()),
        min_block_number: 0,
        uuid: &[],
    }))
    .expect("!validate_fee");
}
