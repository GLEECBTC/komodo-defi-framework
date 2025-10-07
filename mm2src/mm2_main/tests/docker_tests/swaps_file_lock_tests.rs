use crate::generate_utxo_coin_with_random_privkey;
use crate::integration_tests_common::enable_native;
use bitcrypto::ChecksumType;
use common::block_on;
use crypto::Secp256k1Secret;
use keys::{KeyPair, Private};
use mm2_io::file_lock::FileLock;
use mm2_test_helpers::for_tests::{mm_dump, new_mm2_temp_folder_path, MarketMakerIt};
use serde_json::Value as Json;
use std::thread;
use std::time::Duration;

const UNFINISHED_MAKER_SWAP: &str = r#"{"type":"Maker","uuid":"5acb0e63-8b26-469e-81df-7dd9e4a9ad15","events":[{"timestamp":1588244036250,"event":{"type":"Started","data":{"taker_coin":"MYCOIN1","maker_coin":"MYCOIN","taker":"859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521","secret":"dedf3c8dcfff9ee2787b4bf211f960fd044fdab7fa8e922ef613a0115848a498","secret_hash":"9304bce3196f344b2a22dc99db406e95ab6f3107","my_persistent_pub":"02987d5f82205a55d789616f470ae9df48537f050ee050d501aa316651642a0a4d","lock_duration":7800,"maker_amount":"999.99999","taker_amount":"999.99999","maker_payment_confirmations":1,"maker_payment_requires_nota":false,"taker_payment_confirmations":1,"taker_payment_requires_nota":false,"maker_payment_lock":1588259636,"uuid":"5acb0e63-8b26-469e-81df-7dd9e4a9ad15","started_at":1588244036,"maker_coin_start_block":12,"taker_coin_start_block":11}}},{"timestamp":1588244038035,"event":{"type":"Negotiated","data":{"taker_payment_locktime":1588251836,"taker_pubkey":"02859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521"}}}],"maker_amount":"999.99999","maker_coin":"MYCOIN","taker_amount":"999.99999","taker_coin":"MYCOIN1","gui":"nogui","mm_version":"UNKNOWN","success_events":["Started","Negotiated","TakerFeeValidated","MakerPaymentSent","TakerPaymentReceived","TakerPaymentWaitConfirmStarted","TakerPaymentValidatedAndConfirmed","TakerPaymentSpent","Finished"],"error_events":["StartFailed","NegotiateFailed","TakerFeeValidateFailed","MakerPaymentTransactionFailed","MakerPaymentDataSendFailed","MakerPaymentWaitConfirmFailed","TakerPaymentValidateFailed","TakerPaymentWaitConfirmFailed","TakerPaymentSpendFailed","MakerPaymentWaitRefundStarted","MakerPaymentRefunded","MakerPaymentRefundFailed"]}"#;
const FINISHED_MAKER_SWAP: &str = r#"{"type":"Maker","uuid":"5acb0e63-8b26-469e-81df-7dd9e4a9ad15","events":[{"timestamp":1588244036250,"event":{"type":"Started","data":{"taker_coin":"MYCOIN1","maker_coin":"MYCOIN","taker":"859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521","secret":"dedf3c8dcfff9ee2787b4bf211f960fd044fdab7fa8e922ef613a0115848a498","secret_hash":"9304bce3196f344b2a22dc99db406e95ab6f3107","my_persistent_pub":"02987d5f82205a55d789616f470ae9df48537f050ee050d501aa316651642a0a4d","lock_duration":7800,"maker_amount":"999.99999","taker_amount":"999.99999","maker_payment_confirmations":1,"maker_payment_requires_nota":false,"taker_payment_confirmations":1,"taker_payment_requires_nota":false,"maker_payment_lock":1588259636,"uuid":"5acb0e63-8b26-469e-81df-7dd9e4a9ad15","started_at":1588244036,"maker_coin_start_block":12,"taker_coin_start_block":11}}},{"timestamp":1588244038035,"event":{"type":"Negotiated","data":{"taker_payment_locktime":1588251836,"taker_pubkey":"02859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521"}}},{"timestamp":1588244038463,"event":{"type":"TakerFeeValidated","data":{"tx_hex":"0400008085202f8901bdde9bca02870787441f6068e4c2a869a3aac3d1d0925f6a6e27874343544d0a010000006a47304402206694a794693b55fbe8205cb1cbb992d92fa2a9a851763ad1bf1628c16deaf73e02203cfff465504bdd6c51e8fbd45dd2e1187142fdc29e82f36f577616d9d6097d7a012102859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521ffffffff02dfceab07000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac39fd41892e0000001976a914d24e799df360da3ca3158d63b89ffaff27722c1588ac46aeaa5e000000000000000000000000000000","tx_hash":"7118d7484d1cbfdd6126673a848a228856f8748d946f6a9a440c90f0d62e27c6","from":["RUTBzLtJNTn89Wkb6oZocbesKrjBDTRMrC"],"to":["RThtXup6Zo7LZAi8kRWgjAyi1s4u6U9Cpf","RUTBzLtJNTn89Wkb6oZocbesKrjBDTRMrC"],"total_amount":"2000","spent_by_me":"0","received_by_me":"0","my_balance_change":"0","block_height":13,"timestamp":1588244038,"fee_details":{"amount":"0.00001"},"coin":"MYCOIN1","internal_id":"7118d7484d1cbfdd6126673a848a228856f8748d946f6a9a440c90f0d62e27c6"}}},{"timestamp":1588244038505,"event":{"type":"MakerPaymentSent","data":{"tx_hex":"0400008085202f890163cc0aceb3b432f84c1407991a0389b74b7842f030aa261203aa0b4b9a9a15fd010000006b483045022100f3239794b7b0e1c75aae084a65535270f154231ce86a4739976ff69eeff1ebd402206c0aeb28b7b4b2e77e98b3c3efd9a20d85c2cd0627acc3f45d577c0e88c34fb4012102987d5f82205a55d789616f470ae9df48537f050ee050d501aa316651642a0a4dffffffff0118e476481700000017a914dc4ed4686174503b85374bfb0aefe07a9fb37bcf8746aeaa5e000000000000000000000000000000","tx_hash":"9a4c0b3f85ed0bb24dc9575ce7c2fd6bc50ad9d37c91c478946f9c33d15abfdf","from":["RAzSdYQhjCFdyhjrBz1AZQDVg3Hu8DrzYc"],"to":["bYp9ncp3V7FYsymipriVbdd3QL72hK9hio"],"total_amount":"1000","spent_by_me":"1000","received_by_me":"0","my_balance_change":"-1000","block_height":0,"timestamp":0,"fee_details":{"amount":"0.00001"},"coin":"MYCOIN","internal_id":"9a4c0b3f85ed0bb24dc9575ce7c2fd6bc50ad9d37c91c478946f9c33d15abfdf"}}},{"timestamp":1588244053938,"event":{"type":"TakerPaymentReceived","data":{"tx_hex":"0400008085202f8901c6272ed6f0900c449a6a6f948d74f85688228a843a672661ddbf1c4d48d71871010000006b483045022100c37627385c66b7bdf466b4dd4e7095b7551e6f5ea35f9bcc6344eb629f2edcb202203280eaba64b4d72010500166fab62cf34a687a516b2fe83d4eceaf8572cb37a7012102859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521ffffffff0218e476481700000017a91431ff75ed72cd135a7ce50d121e71efc37066b9f9873915cb40170000001976a914d24e799df360da3ca3158d63b89ffaff27722c1588ac55aeaa5e000000000000000000000000000000","tx_hash":"b4718ce94aa43f9073ab0b70c18ab4ea4b587338fb7110f20ff7d1bb452df08f","from":["RUTBzLtJNTn89Wkb6oZocbesKrjBDTRMrC"],"to":["RUTBzLtJNTn89Wkb6oZocbesKrjBDTRMrC","bHHdqM8XWDHee2oyzydwUJKEE2BjofEtZH"],"total_amount":"1998.71298873","spent_by_me":"0","received_by_me":"0","my_balance_change":"0","block_height":0,"timestamp":0,"fee_details":{"amount":"0.00001"},"coin":"MYCOIN1","internal_id":"b4718ce94aa43f9073ab0b70c18ab4ea4b587338fb7110f20ff7d1bb452df08f"}}},{"timestamp":1588244053939,"event":{"type":"TakerPaymentWaitConfirmStarted"}},{"timestamp":1588244068951,"event":{"type":"TakerPaymentValidatedAndConfirmed"}},{"timestamp":1588244068959,"event":{"type":"TakerPaymentSpent","data":{"tx_hex":"0400008085202f89018ff02d45bbd1f70ff21071fb3873584beab48ac1700bab73903fa44ae98c71b400000000d747304402204f0d641a3916e54d6788744c3229110a431eff18634c66fbd1741f9ca7dba99d02202315ee1d9317cc4d5d75d01f066f2c8a59876f790106d310144cdc03c25f985e0120dedf3c8dcfff9ee2787b4bf211f960fd044fdab7fa8e922ef613a0115848a498004c6b6304bcccaa5eb1752102859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521ac6782012088a9149304bce3196f344b2a22dc99db406e95ab6f3107882102987d5f82205a55d789616f470ae9df48537f050ee050d501aa316651642a0a4dac68ffffffff0130e07648170000001976a91412c553e8469363f2d30268c475af1e9186cc90af88ac54a0aa5e000000000000000000000000000000","tx_hash":"e7aed7a77e47b44dc9d12166589bbade70faea10b64888f73ed4be04bcc9f9a9","from":["bHHdqM8XWDHee2oyzydwUJKEE2BjofEtZH"],"to":["RAzSdYQhjCFdyhjrBz1AZQDVg3Hu8DrzYc"],"total_amount":"999.99999","spent_by_me":"0","received_by_me":"999.99998","my_balance_change":"999.99998","block_height":0,"timestamp":0,"fee_details":{"amount":"0.00001"},"coin":"MYCOIN1","internal_id":"e7aed7a77e47b44dc9d12166589bbade70faea10b64888f73ed4be04bcc9f9a9"}}},{"timestamp":1588244068960,"event":{"type":"Finished"}}],"maker_amount":"999.99999","maker_coin":"MYCOIN","taker_amount":"999.99999","taker_coin":"MYCOIN1","gui":"nogui","mm_version":"UNKNOWN","success_events":["Started","Negotiated","TakerFeeValidated","MakerPaymentSent","TakerPaymentReceived","TakerPaymentWaitConfirmStarted","TakerPaymentValidatedAndConfirmed","TakerPaymentSpent","Finished"],"error_events":["StartFailed","NegotiateFailed","TakerFeeValidateFailed","MakerPaymentTransactionFailed","MakerPaymentDataSendFailed","MakerPaymentWaitConfirmFailed","TakerPaymentValidateFailed","TakerPaymentWaitConfirmFailed","TakerPaymentSpendFailed","MakerPaymentWaitRefundStarted","MakerPaymentRefunded","MakerPaymentRefundFailed"]}"#;

const UNFINISHED_TAKER_SWAP: &str = r#"{"type":"Taker","uuid":"5acb0e63-8b26-469e-81df-7dd9e4a9ad15","events":[{"timestamp":1588244036253,"event":{"type":"Started","data":{"taker_coin":"MYCOIN1","maker_coin":"MYCOIN","maker":"987d5f82205a55d789616f470ae9df48537f050ee050d501aa316651642a0a4d","my_persistent_pub":"02859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521","lock_duration":7800,"maker_amount":"999.99999","taker_amount":"999.99999","maker_payment_confirmations":1,"maker_payment_requires_nota":false,"taker_payment_confirmations":1,"taker_payment_requires_nota":false,"taker_payment_lock":1588251836,"uuid":"5acb0e63-8b26-469e-81df-7dd9e4a9ad15","started_at":1588244036,"maker_payment_wait":1588247156,"maker_coin_start_block":12,"taker_coin_start_block":11}}},{"timestamp":1588244038239,"event":{"type":"Negotiated","data":{"maker_payment_locktime":1588259636,"maker_pubkey":"02987d5f82205a55d789616f470ae9df48537f050ee050d501aa316651642a0a4d","secret_hash":"9304bce3196f344b2a22dc99db406e95ab6f3107"}}},{"timestamp":1588244038271,"event":{"type":"TakerFeeSent","data":{"tx_hex":"0400008085202f8901bdde9bca02870787441f6068e4c2a869a3aac3d1d0925f6a6e27874343544d0a010000006a47304402206694a794693b55fbe8205cb1cbb992d92fa2a9a851763ad1bf1628c16deaf73e02203cfff465504bdd6c51e8fbd45dd2e1187142fdc29e82f36f577616d9d6097d7a012102859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521ffffffff02dfceab07000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac39fd41892e0000001976a914d24e799df360da3ca3158d63b89ffaff27722c1588ac46aeaa5e000000000000000000000000000000","tx_hash":"7118d7484d1cbfdd6126673a848a228856f8748d946f6a9a440c90f0d62e27c6","from":["RUTBzLtJNTn89Wkb6oZocbesKrjBDTRMrC"],"to":["RThtXup6Zo7LZAi8kRWgjAyi1s4u6U9Cpf","RUTBzLtJNTn89Wkb6oZocbesKrjBDTRMrC"],"total_amount":"2000","spent_by_me":"2000","received_by_me":"1998.71298873","my_balance_change":"-1.28701127","block_height":0,"timestamp":0,"fee_details":{"amount":"0.00001"},"coin":"MYCOIN1","internal_id":"7118d7484d1cbfdd6126673a848a228856f8748d946f6a9a440c90f0d62e27c6"}}}],"maker_amount":"999.99999","maker_coin":"MYCOIN","taker_amount":"999.99999","taker_coin":"MYCOIN1","gui":"nogui","mm_version":"UNKNOWN","success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","MakerPaymentWaitConfirmFailed","TakerPaymentTransactionFailed","TakerPaymentWaitConfirmFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","TakerPaymentWaitRefundStarted","TakerPaymentRefunded","TakerPaymentRefundFailed"]}"#;
const FINISHED_TAKER_SWAP: &str = r#"{"type":"Taker","uuid":"5acb0e63-8b26-469e-81df-7dd9e4a9ad15","events":[{"timestamp":1588244036253,"event":{"type":"Started","data":{"taker_coin":"MYCOIN1","maker_coin":"MYCOIN","maker":"987d5f82205a55d789616f470ae9df48537f050ee050d501aa316651642a0a4d","my_persistent_pub":"02859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521","lock_duration":7800,"maker_amount":"999.99999","taker_amount":"999.99999","maker_payment_confirmations":1,"maker_payment_requires_nota":false,"taker_payment_confirmations":1,"taker_payment_requires_nota":false,"taker_payment_lock":1588251836,"uuid":"5acb0e63-8b26-469e-81df-7dd9e4a9ad15","started_at":1588244036,"maker_payment_wait":1588247156,"maker_coin_start_block":12,"taker_coin_start_block":11}}},{"timestamp":1588244038239,"event":{"type":"Negotiated","data":{"maker_payment_locktime":1588259636,"maker_pubkey":"02987d5f82205a55d789616f470ae9df48537f050ee050d501aa316651642a0a4d","secret_hash":"9304bce3196f344b2a22dc99db406e95ab6f3107"}}},{"timestamp":1588244038271,"event":{"type":"TakerFeeSent","data":{"tx_hex":"0400008085202f8901bdde9bca02870787441f6068e4c2a869a3aac3d1d0925f6a6e27874343544d0a010000006a47304402206694a794693b55fbe8205cb1cbb992d92fa2a9a851763ad1bf1628c16deaf73e02203cfff465504bdd6c51e8fbd45dd2e1187142fdc29e82f36f577616d9d6097d7a012102859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521ffffffff02dfceab07000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac39fd41892e0000001976a914d24e799df360da3ca3158d63b89ffaff27722c1588ac46aeaa5e000000000000000000000000000000","tx_hash":"7118d7484d1cbfdd6126673a848a228856f8748d946f6a9a440c90f0d62e27c6","from":["RUTBzLtJNTn89Wkb6oZocbesKrjBDTRMrC"],"to":["RThtXup6Zo7LZAi8kRWgjAyi1s4u6U9Cpf","RUTBzLtJNTn89Wkb6oZocbesKrjBDTRMrC"],"total_amount":"2000","spent_by_me":"2000","received_by_me":"1998.71298873","my_balance_change":"-1.28701127","block_height":0,"timestamp":0,"fee_details":{"amount":"0.00001"},"coin":"MYCOIN1","internal_id":"7118d7484d1cbfdd6126673a848a228856f8748d946f6a9a440c90f0d62e27c6"}}},{"timestamp":1588244038698,"event":{"type":"MakerPaymentReceived","data":{"tx_hex":"0400008085202f890163cc0aceb3b432f84c1407991a0389b74b7842f030aa261203aa0b4b9a9a15fd010000006b483045022100f3239794b7b0e1c75aae084a65535270f154231ce86a4739976ff69eeff1ebd402206c0aeb28b7b4b2e77e98b3c3efd9a20d85c2cd0627acc3f45d577c0e88c34fb4012102987d5f82205a55d789616f470ae9df48537f050ee050d501aa316651642a0a4dffffffff0118e476481700000017a914dc4ed4686174503b85374bfb0aefe07a9fb37bcf8746aeaa5e000000000000000000000000000000","tx_hash":"9a4c0b3f85ed0bb24dc9575ce7c2fd6bc50ad9d37c91c478946f9c33d15abfdf","from":["RAzSdYQhjCFdyhjrBz1AZQDVg3Hu8DrzYc"],"to":["bYp9ncp3V7FYsymipriVbdd3QL72hK9hio"],"total_amount":"1000","spent_by_me":"0","received_by_me":"0","my_balance_change":"0","block_height":0,"timestamp":0,"fee_details":{"amount":"0.00001"},"coin":"MYCOIN","internal_id":"9a4c0b3f85ed0bb24dc9575ce7c2fd6bc50ad9d37c91c478946f9c33d15abfdf"}}},{"timestamp":1588244038699,"event":{"type":"MakerPaymentWaitConfirmStarted"}},{"timestamp":1588244053712,"event":{"type":"MakerPaymentValidatedAndConfirmed"}},{"timestamp":1588244053745,"event":{"type":"TakerPaymentSent","data":{"tx_hex":"0400008085202f8901c6272ed6f0900c449a6a6f948d74f85688228a843a672661ddbf1c4d48d71871010000006b483045022100c37627385c66b7bdf466b4dd4e7095b7551e6f5ea35f9bcc6344eb629f2edcb202203280eaba64b4d72010500166fab62cf34a687a516b2fe83d4eceaf8572cb37a7012102859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521ffffffff0218e476481700000017a91431ff75ed72cd135a7ce50d121e71efc37066b9f9873915cb40170000001976a914d24e799df360da3ca3158d63b89ffaff27722c1588ac55aeaa5e000000000000000000000000000000","tx_hash":"b4718ce94aa43f9073ab0b70c18ab4ea4b587338fb7110f20ff7d1bb452df08f","from":["RUTBzLtJNTn89Wkb6oZocbesKrjBDTRMrC"],"to":["RUTBzLtJNTn89Wkb6oZocbesKrjBDTRMrC","bHHdqM8XWDHee2oyzydwUJKEE2BjofEtZH"],"total_amount":"1998.71298873","spent_by_me":"1998.71298873","received_by_me":"998.71298873","my_balance_change":"-1000","block_height":0,"timestamp":0,"fee_details":{"amount":"0.00001"},"coin":"MYCOIN1","internal_id":"b4718ce94aa43f9073ab0b70c18ab4ea4b587338fb7110f20ff7d1bb452df08f"}}},{"timestamp":1588244078860,"event":{"type":"TakerPaymentSpent","data":{"transaction":{"tx_hex":"0400008085202f89018ff02d45bbd1f70ff21071fb3873584beab48ac1700bab73903fa44ae98c71b400000000d747304402204f0d641a3916e54d6788744c3229110a431eff18634c66fbd1741f9ca7dba99d02202315ee1d9317cc4d5d75d01f066f2c8a59876f790106d310144cdc03c25f985e0120dedf3c8dcfff9ee2787b4bf211f960fd044fdab7fa8e922ef613a0115848a498004c6b6304bcccaa5eb1752102859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521ac6782012088a9149304bce3196f344b2a22dc99db406e95ab6f3107882102987d5f82205a55d789616f470ae9df48537f050ee050d501aa316651642a0a4dac68ffffffff0130e07648170000001976a91412c553e8469363f2d30268c475af1e9186cc90af88ac54a0aa5e000000000000000000000000000000","tx_hash":"e7aed7a77e47b44dc9d12166589bbade70faea10b64888f73ed4be04bcc9f9a9","from":["bHHdqM8XWDHee2oyzydwUJKEE2BjofEtZH"],"to":["RAzSdYQhjCFdyhjrBz1AZQDVg3Hu8DrzYc"],"total_amount":"999.99999","spent_by_me":"0","received_by_me":"0","my_balance_change":"0","block_height":29,"timestamp":1588244070,"fee_details":{"amount":"0.00001"},"coin":"MYCOIN1","internal_id":"e7aed7a77e47b44dc9d12166589bbade70faea10b64888f73ed4be04bcc9f9a9"},"secret":"dedf3c8dcfff9ee2787b4bf211f960fd044fdab7fa8e922ef613a0115848a498"}}},{"timestamp":1588244078870,"event":{"type":"MakerPaymentSpent","data":{"tx_hex":"0400008085202f8901dfbf5ad1339c6f9478c4917cd3d90ac56bfdc2e75c57c94db20bed853f0b4c9a00000000d848304502210092535c081325ba5261699d7cfd4c503fb6125dde86389b83f40f3e2c006039bb022063cfd72aa15558dee874cac08b22dbcf11d3f06c8e48b0ddaf75b86887d604410120dedf3c8dcfff9ee2787b4bf211f960fd044fdab7fa8e922ef613a0115848a498004c6b630434ebaa5eb1752102987d5f82205a55d789616f470ae9df48537f050ee050d501aa316651642a0a4dac6782012088a9149304bce3196f344b2a22dc99db406e95ab6f3107882102859a80b83941e4e8ff2f511080e3ea5021db4ba95caec30eb37864e71ae73521ac68ffffffff0130e07648170000001976a914d24e799df360da3ca3158d63b89ffaff27722c1588ac5ea0aa5e000000000000000000000000000000","tx_hash":"caea128b1c85a88abd5924e512780ee18952dadc217b0c06f4b2820eb71d03bc","from":["bYp9ncp3V7FYsymipriVbdd3QL72hK9hio"],"to":["RUTBzLtJNTn89Wkb6oZocbesKrjBDTRMrC"],"total_amount":"999.99999","spent_by_me":"0","received_by_me":"999.99998","my_balance_change":"999.99998","block_height":0,"timestamp":0,"fee_details":{"amount":"0.00001"},"coin":"MYCOIN","internal_id":"caea128b1c85a88abd5924e512780ee18952dadc217b0c06f4b2820eb71d03bc"}}},{"timestamp":1588244078871,"event":{"type":"MakerPaymentSpendConfirmed"}},{"timestamp":1588244078872,"event":{"type":"Finished"}}],"maker_amount":"999.99999","maker_coin":"MYCOIN","taker_amount":"999.99999","taker_coin":"MYCOIN1","gui":"nogui","mm_version":"UNKNOWN","success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","MakerPaymentWaitConfirmFailed","TakerPaymentTransactionFailed","TakerPaymentWaitConfirmFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","TakerPaymentWaitRefundStarted","TakerPaymentRefunded","TakerPaymentRefundFailed"]}"#;

fn swap_file_lock_prevents_double_swap_start_on_kick_start(swap_json: &str) {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let addr_hash = addr_hash_for_privkey(bob_priv_key);
    let db_folder = new_mm2_temp_folder_path(None).join("DB");
    let swaps_db_folder = db_folder.join(addr_hash).join("SWAPS").join("MY");
    std::fs::create_dir_all(&swaps_db_folder).unwrap();
    let swap_path = swaps_db_folder.join("5acb0e63-8b26-469e-81df-7dd9e4a9ad15.json");
    let swap_lock_path = swaps_db_folder.join("5acb0e63-8b26-469e-81df-7dd9e4a9ad15.lock");
    let lock = FileLock::lock(swap_lock_path, 10.).unwrap().unwrap();
    std::fs::write(swap_path, swap_json).unwrap();
    thread::spawn(move || loop {
        // touch the lock file to simulate that other process is running the swap already
        lock.touch().unwrap();
        thread::sleep(Duration::from_secs(10));
    });

    let coins = json! ([
        {"coin":"MYCOIN","asset":"MYCOIN","txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
        {"coin":"MYCOIN1","asset":"MYCOIN1","txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
    ]);
    let bob_conf = json! ({
        "gui": "nogui",
        "netid": 9000,
        "dht": "on",  // Enable DHT without delay.
        "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
        "coins": coins,
        "rpc_password": "pass",
        "i_am_seed": true,
        "dbdir": db_folder.to_str().unwrap(),
        "is_bootstrap_node": true
    });
    let mut mm_bob = MarketMakerIt::start(bob_conf, "pass".to_string(), None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains("Kick starting the swap 5acb0e63-8b26-469e-81df-7dd9e4a9ad15")
    }))
    .unwrap();
    block_on(mm_bob.wait_for_log(50., |log| {
        log.contains(
            "Swap 5acb0e63-8b26-469e-81df-7dd9e4a9ad15 file lock is acquired by another process/thread, aborting",
        )
    }))
    .unwrap();
}

#[test]
fn test_swap_file_lock_prevents_double_swap_start_on_kick_start_maker() {
    swap_file_lock_prevents_double_swap_start_on_kick_start(UNFINISHED_MAKER_SWAP);
}

#[test]
fn test_swap_file_lock_prevents_double_swap_start_on_kick_start_taker() {
    swap_file_lock_prevents_double_swap_start_on_kick_start(UNFINISHED_TAKER_SWAP);
}

#[test]
fn test_swaps_should_kick_start_if_process_was_killed() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 2000.into());
    let coins = json! ([
        {"coin":"MYCOIN","asset":"MYCOIN","txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
        {"coin":"MYCOIN1","asset":"MYCOIN1","txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
    ]);
    let mut bob_conf = json! ({
        "gui": "nogui",
        "netid": 9000,
        "dht": "on",  // Enable DHT without delay.
        "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
        "coins": coins,
        "rpc_password": "pass",
        "i_am_seed": true,
        "is_bootstrap_node": true
    });
    let mut mm_bob = MarketMakerIt::start(bob_conf.clone(), "pass".to_string(), None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mut alice_conf = json! ({
        "gui": "nogui",
        "netid": 9000,
        "dht": "on",  // Enable DHT without delay.
        "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
        "coins": coins,
        "rpc_password": "pass",
        "seednodes": vec![format!("{}", mm_bob.ip)],
    });
    let mut mm_alice = MarketMakerIt::start(alice_conf.clone(), "pass".to_string(), None).unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": "1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);
    let buy: Json = serde_json::from_str(&rc.1).unwrap();
    let uuid = buy["result"]["uuid"].as_str().unwrap().to_owned();
    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains(&format!(
            "Entering the maker_swap_loop MYCOIN/MYCOIN1 with uuid: {uuid}"
        ))
    }))
    .unwrap();
    block_on(mm_alice.wait_for_log(22., |log| {
        log.contains(&format!(
            "Entering the taker_swap_loop MYCOIN/MYCOIN1 with uuid: {uuid}"
        ))
    }))
    .unwrap();
    log!("Give swaps some time to start and save the first event by sleeping for 4 seconds");
    thread::sleep(Duration::from_secs(4));
    // mm_bob using same DB dir that should kick start the swap
    bob_conf["dbdir"] = mm_bob.folder.join("DB").to_str().unwrap().into();
    bob_conf["log"] = mm_bob.folder.join("mm2_dup.log").to_str().unwrap().into();
    // dropping instead of graceful stop to retain swap file locks
    drop(mm_bob);
    let mut mm_bob_dup = MarketMakerIt::start(bob_conf, "pass".to_string(), None).unwrap();
    let (_bob_dup_dump_log, _bob_dup_dump_dashboard) = mm_dump(&mm_bob_dup.log_path);
    log!("{:?}", block_on(enable_native(&mm_bob_dup, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob_dup, "MYCOIN1", &[], None)));

    block_on(mm_bob_dup.wait_for_log(50., |log| log.contains(&format!("Swap {uuid} kick started.")))).unwrap();

    // mm_alice using same DB dir that should kick start the swap
    alice_conf["dbdir"] = mm_alice.folder.join("DB").to_str().unwrap().into();
    alice_conf["log"] = mm_alice.folder.join("mm2_dup.log").to_str().unwrap().into();
    // dropping instead of graceful stop to retain swap file locks
    drop(mm_alice);

    alice_conf["skip_seednodes_check"] = true.into();
    let mut mm_alice_dup = MarketMakerIt::start(alice_conf, "pass".to_string(), None).unwrap();
    let (_alice_dup_dump_log, _alice_dup_dump_dashboard) = mm_dump(&mm_alice_dup.log_path);
    log!("{:?}", block_on(enable_native(&mm_alice_dup, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice_dup, "MYCOIN1", &[], None)));

    block_on(mm_alice_dup.wait_for_log(50., |log| log.contains(&format!("Swap {uuid} kick started.")))).unwrap();
}

fn addr_hash_for_privkey(priv_key: Secp256k1Secret) -> String {
    let private = Private {
        prefix: 1,
        secret: priv_key,
        compressed: true,
        checksum_type: ChecksumType::DSHA256,
    };
    let key_pair = KeyPair::from_private(private).unwrap();
    hex::encode(key_pair.public().address_hash())
}

fn swap_should_not_kick_start_if_finished_during_waiting_for_file_lock(
    unfinished_swap_json: &str,
    finished_swap_json: &str,
) {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let addr_hash = addr_hash_for_privkey(bob_priv_key);
    let db_folder = new_mm2_temp_folder_path(None).join("DB");
    let swaps_db_folder = db_folder.join(addr_hash).join("SWAPS").join("MY");
    std::fs::create_dir_all(&swaps_db_folder).unwrap();
    let swap_path = swaps_db_folder.join("5acb0e63-8b26-469e-81df-7dd9e4a9ad15.json");
    let swap_lock_path = swaps_db_folder.join("5acb0e63-8b26-469e-81df-7dd9e4a9ad15.lock");
    let _lock = FileLock::lock(swap_lock_path, 10.).unwrap().unwrap();
    std::fs::write(&swap_path, unfinished_swap_json).unwrap();

    let coins = json! ([
        {"coin":"MYCOIN","asset":"MYCOIN","txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
        {"coin":"MYCOIN1","asset":"MYCOIN1","txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
    ]);
    let bob_conf = json! ({
        "gui": "nogui",
        "netid": 9000,
        "dht": "on",  // Enable DHT without delay.
        "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
        "coins": coins,
        "rpc_password": "pass",
        "i_am_seed": true,
        "dbdir": db_folder.to_str().unwrap(),
        "is_bootstrap_node": true
    });
    let mut mm_bob = MarketMakerIt::start(bob_conf, "pass".to_string(), None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains("Kick starting the swap 5acb0e63-8b26-469e-81df-7dd9e4a9ad15")
    }))
    .unwrap();
    thread::sleep(Duration::from_secs(10));
    std::fs::write(swap_path, finished_swap_json).unwrap();
    block_on(mm_bob.wait_for_log(50., |log| {
        log.contains("Swap 5acb0e63-8b26-469e-81df-7dd9e4a9ad15 has been finished already, aborting.")
    }))
    .unwrap();
}

#[test]
fn maker_swap_should_not_kick_start_if_finished_during_waiting_for_file_lock() {
    swap_should_not_kick_start_if_finished_during_waiting_for_file_lock(UNFINISHED_MAKER_SWAP, FINISHED_MAKER_SWAP);
}

#[test]
fn taker_swap_should_not_kick_start_if_finished_during_waiting_for_file_lock() {
    swap_should_not_kick_start_if_finished_during_waiting_for_file_lock(UNFINISHED_TAKER_SWAP, FINISHED_TAKER_SWAP);
}
