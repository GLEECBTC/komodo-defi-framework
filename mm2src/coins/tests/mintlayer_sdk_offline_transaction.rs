use coins::mintlayer::{select_spendable_coin_utxos, MintlayerFeeRate, MintlayerUtxo};
use mintlayer_sdk::crypto::types::*;
use mintlayer_sdk::crypto::{self, Amount, Network, SigHashType, SourceId, TxAdditionalInfo};
use serde_json::json;

const PUBLIC_TEST_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const INPUT_ATOMS: u128 = 100_000_000_000;
const OUTPUT_ATOMS: u128 = 99_999_990_000;
const INCLUSION_HEIGHT: u64 = 700_000;

#[test]
fn build_sign_serialize_and_decode_mainnet_transaction_offline() {
    let network = Network::Mainnet;
    let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();
    let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
    let public_key = crypto::public_key_from_private_key(&spend_key);
    let address = crypto::pubkey_to_pubkeyhash_address(&public_key, network);
    assert!(address.starts_with("mtc1"));

    let challenge = b"Pinco Labs Mintlayer offline SDK probe";
    let challenge_signature = crypto::sign_challenge(&spend_key, challenge).unwrap();
    assert!(crypto::verify_challenge(&address, network, &challenge_signature, challenge).unwrap());

    let fake_hash = H256::from_slice(&[0x11; 32]);
    let source_id = crypto::encode_outpoint_source_id(fake_hash, SourceId::Transaction);
    let input = crypto::encode_input_for_utxo(source_id, 0);
    let previous_destination = crypto::encode_destination(&address, network).unwrap();
    let previous_output = TxOutput::Transfer(OutputValue::Coin(Amount::from_atoms(INPUT_ATOMS)), previous_destination);
    let output = crypto::encode_output_transfer(Amount::from_atoms(OUTPUT_ATOMS), &address, network).unwrap();

    let inputs = vec![input];
    let outputs = vec![output];
    let input_destinations = [address.as_str()];
    let estimated_size = crypto::estimate_transaction_size(&inputs, &input_destinations, &outputs, network).unwrap();
    assert!(estimated_size > 0);

    let transaction = crypto::encode_transaction(inputs, outputs, 0).unwrap();
    let transaction_id = crypto::transaction_id(&transaction);
    assert_eq!(transaction_id.len(), 64);

    let input_utxos = vec![Some(previous_output)];
    let witness = crypto::encode_witness(
        SigHashType::all(),
        &spend_key,
        &address,
        &transaction,
        &input_utxos,
        0,
        &TxAdditionalInfo::new(),
        INCLUSION_HEIGHT,
        network,
    )
    .unwrap();

    let signed = crypto::encode_signed_transaction(transaction, vec![witness]).unwrap();
    let signed_bytes = signed.encode();
    assert!(!signed_bytes.is_empty());

    let decoded = crypto::decode_signed_transaction_to_json(&signed_bytes, network).unwrap();
    assert!(decoded.to_string().contains(&address));

    println!("network:          mainnet");
    println!("address:          {address}");
    println!("transaction id:   {transaction_id}");
    println!("estimated bytes:  {estimated_size}");
    println!("serialized bytes: {}", signed_bytes.len());
    println!("implicit fee:     {} atoms", INPUT_ATOMS - OUTPUT_ATOMS);
    println!("challenge verify: OK");
    println!("submission:       disabled by construction");
}

#[test]
fn select_api_utxo_build_sign_and_decode_with_change_offline() {
    const INPUT_ATOMS: u128 = 100_000;
    const SEND_ATOMS: u128 = 80_000;
    const FEE_ATOMS: u128 = 10_000;
    const EXPECTED_CHANGE_ATOMS: u128 = 10_000;

    let network = Network::Mainnet;
    let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();
    let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
    let sender_public_key = crypto::public_key_from_private_key(&spend_key);
    let sender_address = crypto::pubkey_to_pubkeyhash_address(&sender_public_key, network);

    let recipient_key = crypto::make_receiving_address(&account_key, 1).unwrap();
    let recipient_public_key = crypto::public_key_from_private_key(&recipient_key);
    let recipient_address = crypto::pubkey_to_pubkeyhash_address(&recipient_public_key, network);
    drop(recipient_key);

    let source_id = "22".repeat(32);
    let api_utxo: MintlayerUtxo = serde_json::from_value(json!({
        "outpoint": {
            "index": 3,
            "source_id": source_id,
            "source_type": "Transaction"
        },
        "utxo": {
            "destination": sender_address.clone(),
            "type": "Transfer",
            "value": {
                "amount": {
                    "atoms": INPUT_ATOMS.to_string(),
                    "decimal": "0.000001"
                },
                "type": "Coin"
            }
        }
    }))
    .unwrap();

    let selection =
        select_spendable_coin_utxos(&[api_utxo], &sender_address, SEND_ATOMS.checked_add(FEE_ATOMS).unwrap()).unwrap();
    assert_eq!(selection.selected.len(), 1);
    assert_eq!(selection.total_atoms, INPUT_ATOMS);

    let change_atoms = selection
        .total_atoms
        .checked_sub(SEND_ATOMS)
        .and_then(|remaining| remaining.checked_sub(FEE_ATOMS))
        .unwrap();
    assert_eq!(change_atoms, EXPECTED_CHANGE_ATOMS);

    let selected = &selection.selected[0];
    let source_id_bytes = decode_32_byte_hex(&selected.outpoint.source_id);
    let hash = H256::from_slice(&source_id_bytes);
    let encoded_source_id = crypto::encode_outpoint_source_id(hash, SourceId::Transaction);
    let inputs = vec![crypto::encode_input_for_utxo(
        encoded_source_id,
        selected.outpoint.index,
    )];

    let previous_destination = crypto::encode_destination(&sender_address, network).unwrap();
    let input_utxos = vec![Some(TxOutput::Transfer(
        OutputValue::Coin(Amount::from_atoms(selected.atoms)),
        previous_destination,
    ))];

    let outputs = vec![
        crypto::encode_output_transfer(Amount::from_atoms(SEND_ATOMS), &recipient_address, network).unwrap(),
        crypto::encode_output_transfer(Amount::from_atoms(change_atoms), &sender_address, network).unwrap(),
    ];
    let transaction = crypto::encode_transaction(inputs, outputs, 0).unwrap();
    let transaction_id = crypto::transaction_id(&transaction);

    let witness = crypto::encode_witness(
        SigHashType::all(),
        &spend_key,
        &sender_address,
        &transaction,
        &input_utxos,
        0,
        &TxAdditionalInfo::new(),
        INCLUSION_HEIGHT,
        network,
    )
    .unwrap();
    let signed = crypto::encode_signed_transaction(transaction, vec![witness]).unwrap();
    let signed_bytes = signed.encode();
    let decoded = crypto::decode_signed_transaction_to_json(&signed_bytes, network).unwrap();
    let decoded_json = decoded.to_string();

    assert_eq!(transaction_id.len(), 64);
    assert!(decoded_json.contains(&recipient_address));
    assert!(decoded_json.contains(&sender_address));
    assert!(!signed_bytes.is_empty());

    println!("selected inputs:   {}", selection.selected.len());
    println!("selected atoms:    {}", selection.total_atoms);
    println!("send atoms:        {SEND_ATOMS}");
    println!("change atoms:      {change_atoms}");
    println!("fee atoms:         {FEE_ATOMS}");
    println!("transaction id:    {transaction_id}");
    println!("submission:        disabled by construction");
}

#[test]
fn converges_canonical_fee_from_serialized_transaction_size_offline() {
    const FEE_RATE_ATOMS_PER_KB: u128 = 100_000_000_000;
    const SEND_ATOMS: u128 = 50_000_000_000;
    const FIRST_UTXO_ATOMS: u128 = 60_000_000_000;
    const SECOND_UTXO_ATOMS: u128 = 40_000_000_000;
    const MAX_ITERATIONS: usize = 8;

    let network = Network::Mainnet;
    let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();
    let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
    let sender_public_key = crypto::public_key_from_private_key(&spend_key);
    let sender_address = crypto::pubkey_to_pubkeyhash_address(&sender_public_key, network);

    let recipient_key = crypto::make_receiving_address(&account_key, 1).unwrap();
    let recipient_public_key = crypto::public_key_from_private_key(&recipient_key);
    let recipient_address = crypto::pubkey_to_pubkeyhash_address(&recipient_public_key, network);
    drop(recipient_key);

    let api_utxos = vec![
        api_coin_utxo("11".repeat(32), 0, FIRST_UTXO_ATOMS, "0.6", &sender_address),
        api_coin_utxo("22".repeat(32), 1, SECOND_UTXO_ATOMS, "0.4", &sender_address),
    ];
    let fee_rate = MintlayerFeeRate::from_atoms_per_kb(FEE_RATE_ATOMS_PER_KB);
    let mut fee_atoms = 0_u128;
    let mut previous_non_final_state = None;
    let mut final_plan = None;

    for iteration in 1..=MAX_ITERATIONS {
        let required_atoms = SEND_ATOMS.checked_add(fee_atoms).unwrap();
        let selection = select_spendable_coin_utxos(&api_utxos, &sender_address, required_atoms).unwrap();
        let change_atoms = selection
            .total_atoms
            .checked_sub(SEND_ATOMS)
            .and_then(|remaining| remaining.checked_sub(fee_atoms))
            .unwrap();

        let inputs = selection
            .selected
            .iter()
            .map(|selected| {
                let source_id_bytes = decode_32_byte_hex(&selected.outpoint.source_id);
                let hash = H256::from_slice(&source_id_bytes);
                let encoded_source_id = crypto::encode_outpoint_source_id(hash, SourceId::Transaction);
                crypto::encode_input_for_utxo(encoded_source_id, selected.outpoint.index)
            })
            .collect::<Vec<_>>();

        let previous_destination = crypto::encode_destination(&sender_address, network).unwrap();
        let input_utxos = selection
            .selected
            .iter()
            .map(|selected| {
                Some(TxOutput::Transfer(
                    OutputValue::Coin(Amount::from_atoms(selected.atoms)),
                    previous_destination.clone(),
                ))
            })
            .collect::<Vec<_>>();

        let mut outputs =
            vec![crypto::encode_output_transfer(Amount::from_atoms(SEND_ATOMS), &recipient_address, network).unwrap()];
        if change_atoms > 0 {
            outputs.push(
                crypto::encode_output_transfer(Amount::from_atoms(change_atoms), &sender_address, network).unwrap(),
            );
        }

        let transaction = crypto::encode_transaction(inputs, outputs, 0).unwrap();
        let transaction_id = crypto::transaction_id(&transaction);
        let mut witnesses = Vec::with_capacity(input_utxos.len());
        for input_index in 0..input_utxos.len() {
            witnesses.push(
                crypto::encode_witness(
                    SigHashType::all(),
                    &spend_key,
                    &sender_address,
                    &transaction,
                    &input_utxos,
                    input_index,
                    &TxAdditionalInfo::new(),
                    INCLUSION_HEIGHT,
                    network,
                )
                .unwrap(),
            );
        }

        let signed = crypto::encode_signed_transaction(transaction, witnesses).unwrap();
        let signed_bytes = signed.encode();
        let decoded_json = crypto::decode_signed_transaction_to_json(&signed_bytes, network)
            .unwrap()
            .to_string();
        let canonical_fee_atoms = fee_rate.compute_fee_atoms(signed_bytes.len()).unwrap();
        let state = (
            selection.selected.len(),
            change_atoms > 0,
            signed_bytes.len(),
            canonical_fee_atoms,
        );

        if canonical_fee_atoms == fee_atoms {
            final_plan = Some((
                iteration,
                selection.selected.len(),
                selection.total_atoms,
                change_atoms,
                signed_bytes.len(),
                canonical_fee_atoms,
                transaction_id,
                decoded_json,
            ));
            break;
        }

        assert_ne!(previous_non_final_state, Some(state), "fee convergence entered a cycle");
        previous_non_final_state = Some(state);
        fee_atoms = canonical_fee_atoms;
    }

    let (
        iterations,
        selected_inputs,
        selected_atoms,
        change_atoms,
        serialized_bytes,
        final_fee_atoms,
        transaction_id,
        decoded_json,
    ) = final_plan.expect("canonical fee must converge within the iteration limit");

    assert!(iterations >= 2);
    assert_eq!(selected_inputs, 2);
    assert_eq!(selected_atoms, FIRST_UTXO_ATOMS + SECOND_UTXO_ATOMS);
    assert_eq!(selected_atoms - SEND_ATOMS - change_atoms, final_fee_atoms);
    assert_eq!(fee_rate.compute_fee_atoms(serialized_bytes).unwrap(), final_fee_atoms);
    assert!(change_atoms > 0);
    assert_eq!(transaction_id.len(), 64);
    assert!(decoded_json.contains(&recipient_address));
    assert!(decoded_json.contains(&sender_address));

    println!("iterations:        {iterations}");
    println!("selected inputs:   {selected_inputs}");
    println!("selected atoms:    {selected_atoms}");
    println!("send atoms:        {SEND_ATOMS}");
    println!("change atoms:      {change_atoms}");
    println!("serialized bytes:  {serialized_bytes}");
    println!("canonical fee:     {final_fee_atoms} atoms");
    println!("transaction id:    {transaction_id}");
    println!("submission:        disabled by construction");
}

#[test]
fn sdk_size_estimator_is_conservative_across_transaction_shapes() {
    for &(input_count, include_change) in &[(1_usize, false), (1, true), (2, false), (2, true)] {
        let (estimated_bytes, serialized_bytes) = measure_signed_transaction_shape(input_count, include_change);
        let output_count = if include_change { 2 } else { 1 };

        assert!(
            estimated_bytes >= serialized_bytes,
            "SDK estimator underestimated {}-input/{}-output transaction",
            input_count,
            output_count
        );
        assert!(
            estimated_bytes - serialized_bytes <= 2,
            "SDK estimator margin unexpectedly exceeded two bytes"
        );

        println!(
            "shape: {input_count} input(s), {output_count} output(s) | estimated: {estimated_bytes} | serialized: {serialized_bytes} | margin: {}",
            estimated_bytes - serialized_bytes
        );
    }

    println!("submission:        disabled by construction");
}

fn measure_signed_transaction_shape(input_count: usize, include_change: bool) -> (usize, usize) {
    const INPUT_ATOMS: u128 = 100_000_000_000;
    const RECIPIENT_ATOMS: u128 = 10_000_000_000;
    const CHANGE_ATOMS: u128 = 20_000_000_000;

    assert!(input_count > 0 && input_count <= 16);

    let network = Network::Mainnet;
    let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();
    let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
    let sender_public_key = crypto::public_key_from_private_key(&spend_key);
    let sender_address = crypto::pubkey_to_pubkeyhash_address(&sender_public_key, network);

    let recipient_key = crypto::make_receiving_address(&account_key, 1).unwrap();
    let recipient_public_key = crypto::public_key_from_private_key(&recipient_key);
    let recipient_address = crypto::pubkey_to_pubkeyhash_address(&recipient_public_key, network);
    drop(recipient_key);

    let previous_destination = crypto::encode_destination(&sender_address, network).unwrap();
    let mut inputs = Vec::with_capacity(input_count);
    let mut input_utxos = Vec::with_capacity(input_count);
    for input_index in 0..input_count {
        let marker = 0x40_u8 + input_index as u8;
        let fake_hash = H256::from_slice(&[marker; 32]);
        let source_id = crypto::encode_outpoint_source_id(fake_hash, SourceId::Transaction);
        inputs.push(crypto::encode_input_for_utxo(source_id, input_index as u32));
        input_utxos.push(Some(TxOutput::Transfer(
            OutputValue::Coin(Amount::from_atoms(INPUT_ATOMS)),
            previous_destination.clone(),
        )));
    }

    let mut outputs =
        vec![crypto::encode_output_transfer(Amount::from_atoms(RECIPIENT_ATOMS), &recipient_address, network).unwrap()];
    if include_change {
        outputs
            .push(crypto::encode_output_transfer(Amount::from_atoms(CHANGE_ATOMS), &sender_address, network).unwrap());
    }

    let input_destinations = vec![sender_address.as_str(); input_count];
    let estimated_bytes = crypto::estimate_transaction_size(&inputs, &input_destinations, &outputs, network).unwrap();
    let transaction = crypto::encode_transaction(inputs, outputs, 0).unwrap();

    let mut witnesses = Vec::with_capacity(input_count);
    for input_index in 0..input_count {
        witnesses.push(
            crypto::encode_witness(
                SigHashType::all(),
                &spend_key,
                &sender_address,
                &transaction,
                &input_utxos,
                input_index,
                &TxAdditionalInfo::new(),
                INCLUSION_HEIGHT,
                network,
            )
            .unwrap(),
        );
    }

    let signed = crypto::encode_signed_transaction(transaction, witnesses).unwrap();
    let serialized_bytes = signed.encode().len();
    (estimated_bytes, serialized_bytes)
}

#[derive(Debug, PartialEq, Eq)]
struct SingleInputChangePlan {
    change_atoms: Option<u128>,
    signed_bytes: usize,
    minimum_fee_atoms: u128,
    actual_fee_atoms: u128,
    iterations: usize,
}

#[test]
fn plans_single_input_change_boundaries_offline() {
    const FEE_RATE_ATOMS_PER_KB: u128 = 100_000_000_000;
    const SEND_ATOMS: u128 = 50_000_000_000;

    let fee_rate = MintlayerFeeRate::from_atoms_per_kb(FEE_RATE_ATOMS_PER_KB);
    let probe_input_atoms = SEND_ATOMS + FEE_RATE_ATOMS_PER_KB;
    let no_change_bytes = signed_single_input_transaction_size(probe_input_atoms, SEND_ATOMS, None);
    let with_change_bytes = signed_single_input_transaction_size(probe_input_atoms, SEND_ATOMS, Some(1));
    let no_change_fee = fee_rate.compute_fee_atoms(no_change_bytes).unwrap();
    let with_change_fee = fee_rate.compute_fee_atoms(with_change_bytes).unwrap();

    assert!(with_change_fee > no_change_fee);

    let exact = plan_single_input_change_boundary(SEND_ATOMS + no_change_fee, SEND_ATOMS, fee_rate).unwrap();
    assert_eq!(exact.change_atoms, None);
    assert_eq!(exact.signed_bytes, no_change_bytes);
    assert_eq!(exact.minimum_fee_atoms, no_change_fee);
    assert_eq!(exact.actual_fee_atoms, no_change_fee);

    let absorbed_extra = (with_change_fee - no_change_fee) / 2;
    assert!(absorbed_extra > 0);
    let absorbed_available_fee = no_change_fee + absorbed_extra;
    let absorbed =
        plan_single_input_change_boundary(SEND_ATOMS + absorbed_available_fee, SEND_ATOMS, fee_rate).unwrap();
    assert_eq!(absorbed.change_atoms, None);
    assert_eq!(absorbed.signed_bytes, no_change_bytes);
    assert_eq!(absorbed.minimum_fee_atoms, no_change_fee);
    assert_eq!(absorbed.actual_fee_atoms, absorbed_available_fee);
    assert!(absorbed.actual_fee_atoms < with_change_fee);

    let change_reserve_atoms = 10_000_000_000;
    let change_total_atoms = SEND_ATOMS + with_change_fee + change_reserve_atoms;
    let changed = plan_single_input_change_boundary(change_total_atoms, SEND_ATOMS, fee_rate).unwrap();
    let converged_change_atoms = changed.change_atoms.unwrap();
    assert!(converged_change_atoms > 0);
    assert_eq!(changed.actual_fee_atoms, changed.minimum_fee_atoms);
    assert_eq!(
        fee_rate.compute_fee_atoms(changed.signed_bytes).unwrap(),
        changed.minimum_fee_atoms
    );
    assert_eq!(
        SEND_ATOMS + converged_change_atoms + changed.actual_fee_atoms,
        change_total_atoms
    );
    assert!(changed.iterations >= 2);

    let insufficient = plan_single_input_change_boundary(SEND_ATOMS + no_change_fee - 1, SEND_ATOMS, fee_rate);
    assert_eq!(insufficient, Err("insufficient funds after canonical fee"));

    println!("case exact:        no change, {no_change_bytes} byte, {no_change_fee} fee atoms");
    println!(
        "case absorbed:     no change, {} actual fee atoms",
        absorbed.actual_fee_atoms
    );
    println!(
        "case change:       {} change atoms, {} byte, {} fee atoms, {} iterations",
        changed.change_atoms.unwrap(),
        changed.signed_bytes,
        changed.minimum_fee_atoms,
        changed.iterations
    );
    println!("case insufficient: rejected");
    println!("submission:        disabled by construction");
}

fn plan_single_input_change_boundary(
    input_atoms: u128,
    send_atoms: u128,
    fee_rate: MintlayerFeeRate,
) -> Result<SingleInputChangePlan, &'static str> {
    const MAX_ITERATIONS: usize = 8;

    let available_fee_atoms = input_atoms
        .checked_sub(send_atoms)
        .ok_or("insufficient funds before fee")?;
    let no_change_bytes = signed_single_input_transaction_size(input_atoms, send_atoms, None);
    let no_change_fee_atoms = fee_rate
        .compute_fee_atoms(no_change_bytes)
        .map_err(|_| "fee computation failed")?;

    if available_fee_atoms < no_change_fee_atoms {
        return Err("insufficient funds after canonical fee");
    }

    let mut fee_guess_atoms = no_change_fee_atoms;
    for iteration in 1..=MAX_ITERATIONS {
        if available_fee_atoms <= fee_guess_atoms {
            return Ok(SingleInputChangePlan {
                change_atoms: None,
                signed_bytes: no_change_bytes,
                minimum_fee_atoms: no_change_fee_atoms,
                actual_fee_atoms: available_fee_atoms,
                iterations: iteration,
            });
        }

        let change_atoms = available_fee_atoms - fee_guess_atoms;
        let signed_bytes = signed_single_input_transaction_size(input_atoms, send_atoms, Some(change_atoms));
        let canonical_fee_atoms = fee_rate
            .compute_fee_atoms(signed_bytes)
            .map_err(|_| "fee computation failed")?;

        if canonical_fee_atoms == fee_guess_atoms {
            return Ok(SingleInputChangePlan {
                change_atoms: Some(change_atoms),
                signed_bytes,
                minimum_fee_atoms: canonical_fee_atoms,
                actual_fee_atoms: canonical_fee_atoms,
                iterations: iteration,
            });
        }

        if available_fee_atoms <= canonical_fee_atoms {
            return Ok(SingleInputChangePlan {
                change_atoms: None,
                signed_bytes: no_change_bytes,
                minimum_fee_atoms: no_change_fee_atoms,
                actual_fee_atoms: available_fee_atoms,
                iterations: iteration,
            });
        }

        fee_guess_atoms = canonical_fee_atoms;
    }

    Err("canonical fee did not converge")
}

fn signed_single_input_transaction_size(input_atoms: u128, send_atoms: u128, change_atoms: Option<u128>) -> usize {
    let network = Network::Mainnet;
    let account_key = crypto::make_default_account_privkey(PUBLIC_TEST_MNEMONIC, network, None).unwrap();
    let spend_key = crypto::make_receiving_address(&account_key, 0).unwrap();
    let sender_public_key = crypto::public_key_from_private_key(&spend_key);
    let sender_address = crypto::pubkey_to_pubkeyhash_address(&sender_public_key, network);

    let recipient_key = crypto::make_receiving_address(&account_key, 1).unwrap();
    let recipient_public_key = crypto::public_key_from_private_key(&recipient_key);
    let recipient_address = crypto::pubkey_to_pubkeyhash_address(&recipient_public_key, network);
    drop(recipient_key);

    let fake_hash = H256::from_slice(&[0x66; 32]);
    let source_id = crypto::encode_outpoint_source_id(fake_hash, SourceId::Transaction);
    let input = crypto::encode_input_for_utxo(source_id, 0);
    let previous_destination = crypto::encode_destination(&sender_address, network).unwrap();
    let input_utxos = vec![Some(TxOutput::Transfer(
        OutputValue::Coin(Amount::from_atoms(input_atoms)),
        previous_destination,
    ))];

    let mut outputs =
        vec![crypto::encode_output_transfer(Amount::from_atoms(send_atoms), &recipient_address, network).unwrap()];
    if let Some(change_atoms) = change_atoms {
        assert!(change_atoms > 0);
        outputs
            .push(crypto::encode_output_transfer(Amount::from_atoms(change_atoms), &sender_address, network).unwrap());
    }

    let transaction = crypto::encode_transaction(vec![input], outputs, 0).unwrap();
    let witness = crypto::encode_witness(
        SigHashType::all(),
        &spend_key,
        &sender_address,
        &transaction,
        &input_utxos,
        0,
        &TxAdditionalInfo::new(),
        INCLUSION_HEIGHT,
        network,
    )
    .unwrap();
    let signed = crypto::encode_signed_transaction(transaction, vec![witness]).unwrap();
    signed.encode().len()
}

fn api_coin_utxo(source_id: String, index: u32, atoms: u128, decimal: &str, destination: &str) -> MintlayerUtxo {
    serde_json::from_value(json!({
        "outpoint": {
            "index": index,
            "source_id": source_id,
            "source_type": "Transaction"
        },
        "utxo": {
            "destination": destination,
            "type": "Transfer",
            "value": {
                "amount": {
                    "atoms": atoms.to_string(),
                    "decimal": decimal
                },
                "type": "Coin"
            }
        }
    }))
    .unwrap()
}

fn decode_32_byte_hex(value: &str) -> [u8; 32] {
    assert_eq!(value.len(), 64);
    let mut decoded = [0_u8; 32];
    for (index, byte) in decoded.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&value[offset..offset + 2], 16).unwrap();
    }
    decoded
}
