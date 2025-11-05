use super::*;

/// Generate a script_pubkey contains a `function_call` from the specified `contract_address`.
/// The `contract_address` can be either Token address (QRC20) or Swap contract address (EtomicSwap).
pub fn generate_contract_call_script_pubkey(
    function_call: &[u8],
    gas_limit: u64,
    gas_price: u64,
    contract_address: &[u8],
) -> Qrc20AbiResult<Script> {
    if gas_limit == 0 || gas_price == 0 {
        // this is because the `encode_contract_number` will return an empty bytes
        return MmError::err(Qrc20AbiError::InvalidParams(
            "gas_limit and gas_price cannot be zero".to_owned(),
        ));
    }

    if contract_address.is_empty() {
        // this is because the `push_bytes` will panic
        return MmError::err(Qrc20AbiError::InvalidParams("token_addr cannot be empty".to_owned()));
    }

    let gas_limit = encode_contract_number(gas_limit as i64);
    let gas_price = encode_contract_number(gas_price as i64);

    Ok(ScriptBuilder::default()
        .push_opcode(Opcode::OP_4)
        .push_data(&gas_limit)
        .push_data(&gas_price)
        .push_data(function_call)
        .push_data(contract_address)
        .push_opcode(Opcode::OP_CALL)
        .into_script())
}

/// Check if a given script contains a contract call.
/// First opcode should be a version (OP_1..OP_16) to be a contract call.
pub fn is_contract_call(script: &Script) -> bool {
    const VERSION_IDX: usize = 0;
    match script.get_instruction(VERSION_IDX) {
        Some(Ok(instr)) => {
            let opcode = instr.opcode as usize;
            let version_range = (Opcode::OP_1 as usize)..(Opcode::OP_16 as usize);
            version_range.contains(&opcode)
        },
        _ => false,
    }
}

/// The `extract_gas_from_script_pubkey` helper.
#[derive(Clone, Copy, Debug)]
pub enum ExtractGasEnum {
    GasLimit = 1,
    GasPrice = 2,
}

pub fn extract_gas_from_script(script: &Script, extract: ExtractGasEnum) -> Result<u64, String> {
    let instruction = script
        .get_instruction(extract as usize)
        .ok_or(ERRL!("Couldn't extract {:?} from script pubkey", extract as usize))?
        .map_err(|e| ERRL!("Error on extract {:?} from pubkey: {}", extract, e))?;

    let opcode = instruction.opcode as usize;
    if !(1..75).contains(&opcode) {
        return ERR!("Opcode::OP_PUSHBYTES_[X] expected, found {:?}", instruction.opcode);
    }

    let number = match instruction.data {
        Some(d) => try_s!(decode_contract_number(d)),
        _ => return ERR!("Non-empty instruction data expected"),
    };

    Ok(number as u64)
}

pub fn extract_contract_call_from_script(script: &Script) -> Result<Vec<u8>, String> {
    const CONTRACT_CALL_IDX: usize = 3;
    let instruction = script
        .get_instruction(CONTRACT_CALL_IDX)
        .ok_or(ERRL!("Couldn't extract 'contract_params' from script pubkey"))?
        .map_err(|e| ERRL!("Error on extract 'contract_params' from pubkey: {}", e))?;

    match instruction.opcode {
        Opcode::OP_PUSHDATA1 | Opcode::OP_PUSHDATA2 | Opcode::OP_PUSHDATA4 => (),
        opcode if (1..75).contains(&(opcode as usize)) => (),
        _ => return ERR!("Unexpected instruction's opcode {}", instruction.opcode),
    }

    instruction
        .data
        .ok_or(ERRL!("An empty contract call data"))
        .map(Vec::from)
}

pub fn extract_contract_addr_from_script(script: &Script) -> Result<H160, String> {
    const CONTRACT_ADDRESS_IDX: usize = 4;
    let instruction = script
        .get_instruction(CONTRACT_ADDRESS_IDX)
        .ok_or(ERRL!("Couldn't extract 'token_address' from script pubkey"))?
        .map_err(|e| ERRL!("Error on extract 'token_address' from pubkey: {}", e))?;

    match instruction.opcode {
        opcode if (1..75).contains(&(opcode as usize)) => (),
        _ => return ERR!("Unexpected instruction's opcode {}", instruction.opcode),
    }

    instruction.data.try_to_address()
}

/// Serialize the `number` similar to BigEndian but in QRC20 specific format.
fn encode_contract_number(number: i64) -> Vec<u8> {
    // | encoded number (0 - 8 bytes) |
    // therefore the max result vector length is 8
    let capacity = 8;
    let mut encoded = Vec::with_capacity(capacity);

    if number == 0 {
        return Vec::new();
    }

    let is_negative = number.is_negative();
    let mut absnum = (number as i128).abs();

    let mut lowest_byte = 0;
    while absnum != 0 {
        // absnum & 0xFF is first lowest byte
        lowest_byte = (absnum & 0xFF) as u8;
        encoded.push(lowest_byte);
        absnum >>= 8;
    }

    if (lowest_byte & 0x80) != 0 {
        encoded.push({
            if is_negative {
                0x80
            } else {
                0
            }
        });
    } else if is_negative {
        *encoded.last_mut().unwrap() |= 0x80;
    }

    encoded
}

fn decode_contract_number(source: &[u8]) -> Result<i64, String> {
    macro_rules! try_opt {
        ($e: expr) => {
            match $e {
                Some(x) => x,
                _ => return ERR!("Couldn't decode the input {:?}", source),
            }
        };
    }

    if source.is_empty() {
        return Ok(0);
    }

    let mut data = source.to_vec();

    // let last_byte = data.pop().unwrap();
    let mut decoded = 0i128;

    // first pop the data last byte
    let (is_negative, last_byte) = match data.pop().unwrap() {
        // this last byte is the sign byte, pop the real last byte
        0x80 => (true, try_opt!(data.pop())),
        // this last byte is the sign byte, pop the real last byte
        0 => (false, try_opt!(data.pop())),
        // this last byte is real, do XOR on it because it's greater than 0x80
        last_byte if 0x80 < last_byte => (true, last_byte ^ 0x80),
        // this last byte is real, returns it
        last_byte => (false, last_byte),
    };

    // push the last_byte back to the data array
    data.push(last_byte);

    for byte in data.iter().rev() {
        decoded <<= 8;
        decoded |= *byte as i128;
    }

    if is_negative {
        let decoded = decoded.neg();
        Ok(decoded as i64)
    } else {
        Ok(decoded as i64)
    }
}

#[cfg(test)]
mod tests {
    use keys::prefixes::QRC20_PREFIXES;

    use super::*;

    #[test]
    fn test_encode_decode_contract_number() {
        let numbers = vec![
            // left is source number, right is expected encoded array
            (0i64, vec![]),
            (1, vec![1]),
            (-1, vec![129]),
            (40, vec![40]),
            (-40, vec![168]),
            (-127, vec![255]),
            (127, vec![127]),
            (-128, vec![128, 128]),
            (128, vec![128, 0]),
            (255, vec![255, 0]),
            (-255, vec![255, 128]),
            (256, vec![0, 1]),
            (-256, vec![0, 129]),
            (2500000, vec![160, 37, 38]),
            (-2500000, vec![160, 37, 166]),
            (i64::MAX, vec![255, 255, 255, 255, 255, 255, 255, 127]),
            (i64::MIN, vec![0, 0, 0, 0, 0, 0, 0, 128, 128]),
            (Opcode::OP_4 as i64, vec![84]),
            (Opcode::OP_CALL as i64, vec![194, 0]),
        ];

        for (source, encoded) in numbers {
            println!("{source}");
            let actual_encoded = encode_contract_number(source);
            assert_eq!(actual_encoded, encoded);
            let actual_decoded = decode_contract_number(&encoded).unwrap();
            assert_eq!(actual_decoded, source);
        }
    }

    #[test]
    fn test_extract_gas_limit_gas_price() {
        let script: Script = "5403a02526012844a9059cbb0000000000000000000000000240b898276ad2cc0d2fe6f527e8e31104e7fde3000000000000000000000000000000000000000000000000000000003b9aca0014d362e096e873eb7907e205fadc6175c6fec7bc44c2".into();

        let expected_gas_limit = 2_500_000;
        let actual = extract_gas_from_script(&script, ExtractGasEnum::GasLimit).unwrap();
        assert_eq!(actual, expected_gas_limit);

        let expected_gas_price = 40;
        let actual = extract_gas_from_script(&script, ExtractGasEnum::GasPrice).unwrap();
        assert_eq!(actual, expected_gas_price);
    }

    #[test]
    fn test_extract_contract_call() {
        let script: Script = "5403a02526012844a9059cbb0000000000000000000000000240b898276ad2cc0d2fe6f527e8e31104e7fde3000000000000000000000000000000000000000000000000000000003b9aca0014d362e096e873eb7907e205fadc6175c6fec7bc44c2".into();

        let to_addr: UtxoAddress =
            UtxoAddress::from_legacyaddress("qHmJ3KA6ZAjR9wGjpFASn4gtUSeFAqdZgs", &QRC20_PREFIXES).unwrap();
        let to_addr = qtum::contract_addr_from_utxo_addr(to_addr).unwrap();
        let amount: U256 = 1000000000.into();
        let function = eth::ERC20_CONTRACT.function("transfer").unwrap();
        let expected = function
            .encode_input(&[Token::Address(to_addr), Token::Uint(amount)])
            .unwrap();

        let actual = extract_contract_call_from_script(&script).unwrap();
        assert_eq!(actual, expected);

        // TX b11a262380657310abf01f8abe117da2c2adf788ab1fa0fa29da4ab505fc00c0
        let tx = hex::decode("01000000029ba0865fc62aac1f5f1a4aac3c9f54ff3d74211030bf6eb41e870b30297bd3fc010000006a47304402201808cbc98036ea63d32e858f776c722897d3f4b670744594deba25b69128d0ba02207b3f86f0ab6b6fa0ff581dc7be33af034c6004f3537a2f96c4ddf3ed0130defc012103693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9ffffffff63574ffa1e8edd8af8b08f3c1d8e5f33170772c38631a50ddc29c16d74c762f6020000006b483045022100db6cf963f6be56f7c6004ede74d452b2c5932eb6b12094fe67fa4ff0b6f4406e02207acff9163588a0c58fa009f5e876f90817b4006c303095d9b6425aae2922a485012103693bff1b39e8b5a306810023c29b95397eb395530b106b1820ea235fd81d9ce9ffffffff040000000000000000625403a08601012844095ea7b3000000000000000000000000ba8b71f3544b93e2f681f996da519a98ace0107a000000000000000000000000000000000000000000000000000000000000000014d362e096e873eb7907e205fadc6175c6fec7bc44c20000000000000000625403a08601012844095ea7b3000000000000000000000000ba8b71f3544b93e2f681f996da519a98ace0107a0000000000000000000000000000000000000000000000000000000001312d0014d362e096e873eb7907e205fadc6175c6fec7bc44c20000000000000000e35403a0860101284cc49b415b2a65e285b98480fd7de696e9fb5bcb68ec9468dd906c683e38cabb8f39905675fa0000000000000000000000000000000000000000000000000000000001312d00000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc440000000000000000000000000240b898276ad2cc0d2fe6f527e8e31104e7fde30101010101010101010101010101010101010101000000000000000000000000000000000000000000000000000000000000000000000000000000005f6d80b814ba8b71f3544b93e2f681f996da519a98ace0107ac2e52fdd05000000001976a9149e032d4b0090a11dc40fe6c47601499a35d55fbb88ac82816d5f")
            .unwrap();
        let utxo_tx: UtxoTx = deserialize(tx.as_slice()).unwrap();

        // first output in "b11a262380657310abf01f8abe117da2c2adf788ab1fa0fa29da4ab505fc00c0"
        // `approve` to 0 contract call
        let expected = hex::decode("095ea7b3000000000000000000000000ba8b71f3544b93e2f681f996da519a98ace0107a0000000000000000000000000000000000000000000000000000000000000000")
            .unwrap();
        let script = utxo_tx.outputs[0].script_pubkey.clone().into();

        let actual = extract_contract_call_from_script(&script).unwrap();
        assert_eq!(actual, expected);

        // second output in "b11a262380657310abf01f8abe117da2c2adf788ab1fa0fa29da4ab505fc00c0"
        // `approve` to 20000000 contract call
        let expected = hex::decode("095ea7b3000000000000000000000000ba8b71f3544b93e2f681f996da519a98ace0107a0000000000000000000000000000000000000000000000000000000001312d00")
            .unwrap();
        let script = utxo_tx.outputs[1].script_pubkey.clone().into();

        let actual = extract_contract_call_from_script(&script).unwrap();
        assert_eq!(actual, expected);

        // third output in "b11a262380657310abf01f8abe117da2c2adf788ab1fa0fa29da4ab505fc00c0"
        // `erc20Payment` 20000000 amount contract call
        let expected = hex::decode("9b415b2a65e285b98480fd7de696e9fb5bcb68ec9468dd906c683e38cabb8f39905675fa0000000000000000000000000000000000000000000000000000000001312d00000000000000000000000000d362e096e873eb7907e205fadc6175c6fec7bc440000000000000000000000000240b898276ad2cc0d2fe6f527e8e31104e7fde30101010101010101010101010101010101010101000000000000000000000000000000000000000000000000000000000000000000000000000000005f6d80b8")
            .unwrap();
        let script = utxo_tx.outputs[2].script_pubkey.clone().into();

        let actual = extract_contract_call_from_script(&script).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_extract_contract_addr_from_script() {
        let script: Script = "5403a02526012844a9059cbb0000000000000000000000000240b898276ad2cc0d2fe6f527e8e31104e7fde3000000000000000000000000000000000000000000000000000000003b9aca0014d362e096e873eb7907e205fadc6175c6fec7bc44c2".into();
        let expected = qtum::contract_addr_from_str("0xd362e096e873eb7907e205fadc6175c6fec7bc44").unwrap();

        let actual = extract_contract_addr_from_script(&script).unwrap();
        assert_eq!(actual, expected);
    }
}
