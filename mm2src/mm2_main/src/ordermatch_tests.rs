use super::*;
use crate::lp_ordermatch::new_protocol::{MakerOrderUpdated, PubkeyKeepAlive};
use coins::{MmCoin, TestCoin};
use common::{block_on, executor::spawn};
use crypto::privkey::key_pair_from_seed;
use db_common::sqlite::rusqlite::Connection;
use futures::{channel::mpsc, StreamExt};
use mm2_core::mm_ctx::{MmArc, MmCtx};
use mm2_libp2p::application::request_response::ordermatch::OrdermatchRequest;
use mm2_libp2p::application::request_response::P2PRequest;
use mm2_libp2p::behaviours::atomicdex::generate_ed25519_keypair;
use mm2_libp2p::p2p_ctx::P2PContext;
use mm2_libp2p::AdexBehaviourCmd;
use mm2_libp2p::{decode_message, PeerId};
use mm2_test_helpers::for_tests::mm_ctx_with_iguana;
use mocktopus::mocking::*;
use rand::{seq::SliceRandom, thread_rng, Rng};
use secp256k1::PublicKey;
use std::collections::HashSet;
use std::iter::{self, FromIterator};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[test]
fn test_match_maker_order_and_taker_request() {
    let maker = MakerOrder {
        base: "BASE".into(),
        rel: "REL".into(),
        created_at: now_ms(),
        updated_at: Some(now_ms()),
        max_base_vol: 10.into(),
        min_base_vol: 0.into(),
        price: 1.into(),
        matches: HashMap::new(),
        started_swaps: Vec::new(),
        uuid: new_uuid(),
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid: new_uuid(),
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 10.into(),
        rel_amount: 20.into(),
        action: TakerAction::Buy,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let actual = maker.match_with_request(&request);
    let expected = OrderMatchResult::Matched((10.into(), 10.into()));
    assert_eq!(expected, actual);

    let maker = MakerOrder {
        base: "BASE".into(),
        rel: "REL".into(),
        created_at: now_ms(),
        updated_at: Some(now_ms()),
        max_base_vol: 10.into(),
        min_base_vol: 0.into(),
        price: "0.5".into(),
        matches: HashMap::new(),
        started_swaps: Vec::new(),
        uuid: new_uuid(),
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid: new_uuid(),
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 10.into(),
        rel_amount: 20.into(),
        action: TakerAction::Buy,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let actual = maker.match_with_request(&request);
    let expected = OrderMatchResult::Matched((10.into(), 5.into()));
    assert_eq!(expected, actual);

    let maker = MakerOrder {
        base: "BASE".into(),
        rel: "REL".into(),
        created_at: now_ms(),
        updated_at: Some(now_ms()),
        max_base_vol: 10.into(),
        min_base_vol: 0.into(),
        price: "0.5".into(),
        matches: HashMap::new(),
        started_swaps: Vec::new(),
        uuid: new_uuid(),
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid: new_uuid(),
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 10.into(),
        rel_amount: 2.into(),
        action: TakerAction::Buy,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let actual = maker.match_with_request(&request);
    let expected = OrderMatchResult::NotMatched;
    assert_eq!(expected, actual);

    let maker = MakerOrder {
        base: "BASE".into(),
        rel: "REL".into(),
        created_at: now_ms(),
        updated_at: Some(now_ms()),
        max_base_vol: 10.into(),
        min_base_vol: 0.into(),
        price: "0.5".into(),
        matches: HashMap::new(),
        started_swaps: Vec::new(),
        uuid: new_uuid(),
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };

    let request = TakerRequest {
        base: "REL".into(),
        rel: "BASE".into(),
        uuid: new_uuid(),
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 5.into(),
        rel_amount: 10.into(),
        action: TakerAction::Sell,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let actual = maker.match_with_request(&request);
    let expected = OrderMatchResult::Matched((10.into(), 5.into()));
    assert_eq!(expected, actual);

    let maker = MakerOrder {
        base: "BASE".into(),
        rel: "REL".into(),
        created_at: now_ms(),
        updated_at: Some(now_ms()),
        max_base_vol: 20.into(),
        min_base_vol: 0.into(),
        price: "0.5".into(),
        matches: HashMap::new(),
        started_swaps: Vec::new(),
        uuid: new_uuid(),
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };

    let request = TakerRequest {
        base: "REL".into(),
        rel: "BASE".into(),
        uuid: new_uuid(),
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 10.into(),
        rel_amount: 10.into(),
        action: TakerAction::Sell,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let actual = maker.match_with_request(&request);
    let expected = OrderMatchResult::Matched((20.into(), 10.into()));
    assert_eq!(expected, actual);

    let maker = MakerOrder {
        base: "BASE".into(),
        rel: "REL".into(),
        created_at: now_ms(),
        updated_at: Some(now_ms()),
        max_base_vol: 1.into(),
        min_base_vol: 0.into(),
        price: "1".into(),
        matches: HashMap::new(),
        started_swaps: Vec::new(),
        uuid: new_uuid(),
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };

    let request = TakerRequest {
        base: "REL".into(),
        rel: "BASE".into(),
        uuid: new_uuid(),
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 1.into(),
        rel_amount: "0.9".into(),
        action: TakerAction::Sell,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let actual = maker.match_with_request(&request);
    let expected = OrderMatchResult::Matched((1.into(), 1.into()));
    assert_eq!(expected, actual);

    // The following Taker request has not to be matched since the resulted base amount it greater than `max_base_vol`.
    // https://github.com/KomodoPlatform/atomicDEX-API/issues/1041#issuecomment-901863864
    let maker = MakerOrder {
        max_base_vol: "0.2928826881884105".into(),
        min_base_vol: 0.into(),
        price: "2643.01935664".into(),
        created_at: now_ms(),
        updated_at: None,
        base: "ETH-BEP20".to_owned(),
        rel: "KMD".to_owned(),
        matches: HashMap::new(),
        started_swaps: vec![],
        uuid: new_uuid(),
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };
    let request = TakerRequest {
        base: "KMD".to_owned(),
        rel: "ETH-BEP20".to_owned(),
        base_amount: "774.205645538427044180416545".into(),
        rel_amount: "0.2928826881884105".into(),
        action: TakerAction::Sell,
        uuid: new_uuid(),
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };
    let actual = maker.match_with_request(&request);
    assert_eq!(actual, OrderMatchResult::NotMatched);

    // Though the Taker's rel amount is less than the Makers' min base volume '2',
    // the Maker's price is chosen to calculate the result amounts, so we have:
    // `base_amount = taker_base_amount/maker_price = 30/10 = 3`
    // `rel_amount = taker_base_amount = 30`.
    // The order should be matched.
    let maker = MakerOrder {
        max_base_vol: "3".into(),
        min_base_vol: "2".into(),
        price: 10.into(),
        created_at: now_ms(),
        updated_at: None,
        base: "BASE".to_owned(),
        rel: "REL".to_owned(),
        matches: HashMap::new(),
        started_swaps: vec![],
        uuid: new_uuid(),
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };
    let request = TakerRequest {
        base: "REL".to_owned(),
        rel: "BASE".to_owned(),
        base_amount: "30".into(),
        rel_amount: "1.5".into(),
        action: TakerAction::Sell,
        uuid: new_uuid(),
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };
    let actual = maker.match_with_request(&request);
    let expected_base_amount = MmNumber::from(3);
    let expected_rel_amount = MmNumber::from(30);
    let expected = OrderMatchResult::Matched((expected_base_amount, expected_rel_amount));
    assert_eq!(actual, expected);
}

// https://github.com/KomodoPlatform/atomicDEX-API/pull/739#discussion_r517275495
#[test]
fn maker_order_match_with_request_zero_volumes() {
    let coin = MmCoinEnum::Test(TestCoin::default());

    let maker_order = MakerOrderBuilder::new(&coin, &coin)
        .with_max_base_vol(1.into())
        .with_price(1.into())
        .build_unchecked();

    // default taker order has empty coins and zero amounts so it should pass to the price calculation stage (division)
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_rel_amount(1.into())
        .build_unchecked();

    let expected = OrderMatchResult::NotMatched;
    let actual = maker_order.match_with_request(&taker_order.request);
    assert_eq!(expected, actual);

    // default taker order has empty coins and zero amounts so it should pass to the price calculation stage (division)
    let taker_request = TakerOrderBuilder::new(&coin, &coin)
        .with_base_amount(1.into())
        .with_action(TakerAction::Sell)
        .build_unchecked();

    let expected = OrderMatchResult::NotMatched;
    let actual = maker_order.match_with_request(&taker_request.request);
    assert_eq!(expected, actual);
}

#[test]
fn test_maker_order_available_amount() {
    let mut maker = MakerOrder {
        base: "BASE".into(),
        rel: "REL".into(),
        created_at: now_ms(),
        updated_at: Some(now_ms()),
        max_base_vol: 10.into(),
        min_base_vol: 0.into(),
        price: 1.into(),
        matches: HashMap::new(),
        started_swaps: Vec::new(),
        uuid: new_uuid(),
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };
    maker.matches.insert(
        new_uuid(),
        MakerMatch {
            request: TakerRequest {
                uuid: new_uuid(),
                base: "BASE".into(),
                rel: "REL".into(),
                base_amount: 5.into(),
                rel_amount: 5.into(),
                sender_pubkey: H256Json::default(),
                dest_pub_key: H256Json::default(),
                action: TakerAction::Buy,
                match_by: MatchBy::Any,
                conf_settings: None,
                base_protocol_info: None,
                rel_protocol_info: None,
                swap_version: SwapVersion::default(),
                #[cfg(feature = "ibc-routing-for-swaps")]
                order_metadata: OrderMetadata::default(),
            },
            reserved: MakerReserved {
                base: "BASE".into(),
                rel: "REL".into(),
                base_amount: 5.into(),
                rel_amount: 5.into(),
                sender_pubkey: H256Json::default(),
                dest_pub_key: H256Json::default(),
                maker_order_uuid: new_uuid(),
                taker_order_uuid: new_uuid(),
                conf_settings: None,
                base_protocol_info: None,
                rel_protocol_info: None,
                swap_version: SwapVersion::default(),
                #[cfg(feature = "ibc-routing-for-swaps")]
                order_metadata: OrderMetadata::default(),
            },
            connect: None,
            connected: None,
            last_updated: now_ms(),
        },
    );
    maker.matches.insert(
        new_uuid(),
        MakerMatch {
            request: TakerRequest {
                uuid: new_uuid(),
                base: "BASE".into(),
                rel: "REL".into(),
                base_amount: 1.into(),
                rel_amount: 1.into(),
                sender_pubkey: H256Json::default(),
                dest_pub_key: H256Json::default(),
                action: TakerAction::Buy,
                match_by: MatchBy::Any,
                conf_settings: None,
                base_protocol_info: None,
                rel_protocol_info: None,
                swap_version: SwapVersion::default(),
                #[cfg(feature = "ibc-routing-for-swaps")]
                order_metadata: OrderMetadata::default(),
            },
            reserved: MakerReserved {
                base: "BASE".into(),
                rel: "REL".into(),
                base_amount: 1.into(),
                rel_amount: 1.into(),
                sender_pubkey: H256Json::default(),
                dest_pub_key: H256Json::default(),
                maker_order_uuid: new_uuid(),
                taker_order_uuid: new_uuid(),
                conf_settings: None,
                base_protocol_info: None,
                rel_protocol_info: None,
                swap_version: SwapVersion::default(),
                #[cfg(feature = "ibc-routing-for-swaps")]
                order_metadata: OrderMetadata::default(),
            },
            connect: None,
            connected: None,
            last_updated: now_ms(),
        },
    );

    let expected = BigRational::from_integer(4.into());
    let actual = maker.available_amount();
    assert_eq!(MmNumber::from(expected), actual);
}

#[test]
fn test_taker_match_reserved() {
    let uuid = new_uuid();

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid,
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 10.into(),
        rel_amount: 10.into(),
        action: TakerAction::Buy,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let order = TakerOrder {
        request,
        matches: HashMap::new(),
        created_at: now_ms(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    let reserved = MakerReserved {
        base: "BASE".into(),
        rel: "REL".into(),
        base_amount: 10.into(),
        rel_amount: 10.into(),
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        maker_order_uuid: new_uuid(),
        taker_order_uuid: uuid,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    assert_eq!(MatchReservedResult::Matched, order.match_reserved(&reserved));

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid,
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 10.into(),
        rel_amount: 10.into(),
        action: TakerAction::Sell,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let order = TakerOrder {
        request,
        matches: HashMap::new(),
        created_at: now_ms(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    let reserved = MakerReserved {
        base: "REL".into(),
        rel: "BASE".into(),
        base_amount: 10.into(),
        rel_amount: 10.into(),
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        maker_order_uuid: new_uuid(),
        taker_order_uuid: uuid,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    assert_eq!(MatchReservedResult::Matched, order.match_reserved(&reserved));

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid,
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 1.into(),
        rel_amount: "0.9".into(),
        action: TakerAction::Sell,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let order = TakerOrder {
        request,
        matches: HashMap::new(),
        created_at: now_ms(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    let reserved = MakerReserved {
        base: "REL".into(),
        rel: "BASE".into(),
        base_amount: 1.into(),
        rel_amount: 1.into(),
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        maker_order_uuid: new_uuid(),
        taker_order_uuid: uuid,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    assert_eq!(MatchReservedResult::Matched, order.match_reserved(&reserved));

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid,
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 1.into(),
        rel_amount: "0.9".into(),
        action: TakerAction::Sell,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let order = TakerOrder {
        request,
        matches: HashMap::new(),
        created_at: now_ms(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    let reserved = MakerReserved {
        base: "REL".into(),
        rel: "BASE".into(),
        base_amount: "0.8".into(),
        rel_amount: 1.into(),
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        maker_order_uuid: new_uuid(),
        taker_order_uuid: uuid,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    assert_eq!(MatchReservedResult::NotMatched, order.match_reserved(&reserved));

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid,
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 1.into(),
        rel_amount: 2.into(),
        action: TakerAction::Buy,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let order = TakerOrder {
        request,
        matches: HashMap::new(),
        created_at: now_ms(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    let reserved = MakerReserved {
        base: "BASE".into(),
        rel: "REL".into(),
        base_amount: 1.into(),
        rel_amount: 1.into(),
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        maker_order_uuid: new_uuid(),
        taker_order_uuid: uuid,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    assert_eq!(MatchReservedResult::Matched, order.match_reserved(&reserved));

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid,
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 1.into(),
        rel_amount: 2.into(),
        action: TakerAction::Buy,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let order = TakerOrder {
        request,
        matches: HashMap::new(),
        created_at: now_ms(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    let reserved = MakerReserved {
        base: "BASE".into(),
        rel: "REL".into(),
        base_amount: 1.into(),
        rel_amount: 1.into(),
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        maker_order_uuid: new_uuid(),
        taker_order_uuid: uuid,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    assert_eq!(MatchReservedResult::Matched, order.match_reserved(&reserved));

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid,
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 1.into(),
        rel_amount: 2.into(),
        action: TakerAction::Buy,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let order = TakerOrder {
        request,
        matches: HashMap::new(),
        created_at: now_ms(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    let reserved = MakerReserved {
        base: "BASE".into(),
        rel: "REL".into(),
        base_amount: 1.into(),
        rel_amount: 1.into(),
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        maker_order_uuid: new_uuid(),
        taker_order_uuid: uuid,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    assert_eq!(MatchReservedResult::Matched, order.match_reserved(&reserved));

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid,
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 1.into(),
        rel_amount: 2.into(),
        action: TakerAction::Buy,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let order = TakerOrder {
        request,
        matches: HashMap::new(),
        created_at: now_ms(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    let reserved = MakerReserved {
        base: "BASE".into(),
        rel: "REL".into(),
        base_amount: 1.into(),
        rel_amount: 3.into(),
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        maker_order_uuid: new_uuid(),
        taker_order_uuid: uuid,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    assert_eq!(MatchReservedResult::NotMatched, order.match_reserved(&reserved));

    let order = TakerOrder {
        created_at: 1568358064115,
        request: TakerRequest {
            base: "RICK".into(),
            rel: "MORTY".into(),
            base_amount:
                "0.3333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333"
                    .into(),
            rel_amount: 1.into(),
            action: TakerAction::Buy,
            uuid,
            sender_pubkey: H256Json::default(),
            dest_pub_key: H256Json::default(),
            match_by: MatchBy::Any,
            conf_settings: None,
            base_protocol_info: None,
            rel_protocol_info: None,
            swap_version: SwapVersion::default(),
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: OrderMetadata::default(),
        },
        matches: HashMap::new(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    let reserved = MakerReserved {
        base: "RICK".into(),
        rel: "MORTY".into(),
        base_amount: "0.3333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333333".into(),
        rel_amount: "0.777777776666666666666666666666666666666666666666666666666666666666666666666666666666666666666666666588888889".into(),
        taker_order_uuid: uuid,
        maker_order_uuid: uuid,
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    assert_eq!(MatchReservedResult::Matched, order.match_reserved(&reserved));
}

#[test]
fn test_taker_order_cancellable() {
    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid: new_uuid(),
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 1.into(),
        rel_amount: 2.into(),
        action: TakerAction::Buy,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let order = TakerOrder {
        request,
        matches: HashMap::new(),
        created_at: now_ms(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    assert!(order.is_cancellable());

    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid: new_uuid(),
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 1.into(),
        rel_amount: 2.into(),
        action: TakerAction::Buy,
        match_by: MatchBy::Any,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let mut order = TakerOrder {
        request,
        matches: HashMap::new(),
        created_at: now_ms(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    order.matches.insert(
        new_uuid(),
        TakerMatch {
            last_updated: now_ms(),
            reserved: MakerReserved {
                base: "BASE".into(),
                rel: "REL".into(),
                base_amount: 1.into(),
                rel_amount: 3.into(),
                sender_pubkey: H256Json::default(),
                dest_pub_key: H256Json::default(),
                maker_order_uuid: new_uuid(),
                taker_order_uuid: new_uuid(),
                conf_settings: None,
                base_protocol_info: None,
                rel_protocol_info: None,
                swap_version: SwapVersion::default(),
                #[cfg(feature = "ibc-routing-for-swaps")]
                order_metadata: OrderMetadata::default(),
            },
            connect: TakerConnect {
                sender_pubkey: H256Json::default(),
                dest_pub_key: H256Json::default(),
                maker_order_uuid: new_uuid(),
                taker_order_uuid: new_uuid(),
            },
            connected: None,
        },
    );

    assert!(!order.is_cancellable());
}

fn prepare_for_cancel_by(ctx: &MmArc) -> mpsc::Receiver<AdexBehaviourCmd> {
    let (tx, rx) = mpsc::channel(10);

    let p2p_key = {
        let crypto_ctx = CryptoCtx::from_ctx(ctx).unwrap();
        let key = bitcrypto::sha256(crypto_ctx.mm2_internal_privkey_slice());
        key.take()
    };

    let p2p_ctx = P2PContext::new(tx, generate_ed25519_keypair(p2p_key));
    p2p_ctx.store_to_mm_arc(ctx);

    let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).unwrap();
    let mut maker_orders_ctx = ordermatch_ctx.maker_orders_ctx.lock();
    let mut taker_orders = block_on(ordermatch_ctx.my_taker_orders.lock());

    maker_orders_ctx.add_order(
        ctx.weak(),
        MakerOrder {
            uuid: Uuid::from_bytes([0; 16]),
            base: "RICK".into(),
            rel: "MORTY".into(),
            created_at: now_ms(),
            updated_at: Some(now_ms()),
            matches: HashMap::new(),
            max_base_vol: 0.into(),
            min_base_vol: 0.into(),
            price: 0.into(),
            started_swaps: vec![],
            conf_settings: None,
            changes_history: None,
            save_in_history: false,
            base_orderbook_ticker: None,
            rel_orderbook_ticker: None,
            p2p_privkey: None,
            swap_version: SwapVersion::default(),
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: OrderMetadata::default(),
            timeout_in_minutes: None,
        },
        None,
    );
    maker_orders_ctx.add_order(
        ctx.weak(),
        MakerOrder {
            uuid: Uuid::from_bytes([1; 16]),
            base: "MORTY".into(),
            rel: "RICK".into(),
            created_at: now_ms(),
            updated_at: Some(now_ms()),
            matches: HashMap::new(),
            max_base_vol: 0.into(),
            min_base_vol: 0.into(),
            price: 0.into(),
            started_swaps: vec![],
            conf_settings: None,
            changes_history: None,
            save_in_history: false,
            base_orderbook_ticker: None,
            rel_orderbook_ticker: None,
            p2p_privkey: None,
            swap_version: SwapVersion::default(),
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: OrderMetadata::default(),
            timeout_in_minutes: None,
        },
        None,
    );
    maker_orders_ctx.add_order(
        ctx.weak(),
        MakerOrder {
            uuid: Uuid::from_bytes([2; 16]),
            base: "MORTY".into(),
            rel: "ETH".into(),
            created_at: now_ms(),
            updated_at: Some(now_ms()),
            matches: HashMap::new(),
            max_base_vol: 0.into(),
            min_base_vol: 0.into(),
            price: 0.into(),
            started_swaps: vec![],
            conf_settings: None,
            changes_history: None,
            save_in_history: false,
            base_orderbook_ticker: None,
            rel_orderbook_ticker: None,
            p2p_privkey: None,
            swap_version: SwapVersion::default(),
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: OrderMetadata::default(),
            timeout_in_minutes: None,
        },
        None,
    );
    taker_orders.insert(
        Uuid::from_bytes([3; 16]),
        TakerOrder {
            matches: HashMap::new(),
            created_at: now_ms(),
            request: TakerRequest {
                base: "RICK".into(),
                rel: "MORTY".into(),
                uuid: Uuid::from_bytes([3; 16]),
                action: TakerAction::Buy,
                base_amount: 0.into(),
                rel_amount: 0.into(),
                dest_pub_key: H256Json::default(),
                sender_pubkey: H256Json::default(),
                match_by: MatchBy::Any,
                conf_settings: None,
                base_protocol_info: None,
                rel_protocol_info: None,
                swap_version: SwapVersion::default(),
                #[cfg(feature = "ibc-routing-for-swaps")]
                order_metadata: OrderMetadata::default(),
            },
            order_type: OrderType::GoodTillCancelled,
            min_volume: 0.into(),
            timeout: 30,
            save_in_history: false,
            base_orderbook_ticker: None,
            rel_orderbook_ticker: None,
            p2p_privkey: None,
        },
    );
    rx
}

#[test]
fn test_cancel_by_single_coin() {
    let ctx = mm_ctx_with_iguana(None);
    let rx = prepare_for_cancel_by(&ctx);

    let connection = Connection::open_in_memory().unwrap();
    let _ = ctx
        .sqlite_connection
        .set(Arc::new(Mutex::new(connection)))
        .map_err(|_| "Already Initialized".to_string());

    delete_my_maker_order.mock_safe(|_, _, _| MockResult::Return(Box::new(futures01::future::ok(()))));
    delete_my_taker_order.mock_safe(|_, _, _| MockResult::Return(Box::new(futures01::future::ok(()))));

    let (cancelled, _) = block_on(cancel_orders_by(&ctx, CancelBy::Coin { ticker: "RICK".into() })).unwrap();
    block_on(rx.take(2).collect::<Vec<_>>());
    assert!(cancelled.contains(&Uuid::from_bytes([0; 16])));
    assert!(cancelled.contains(&Uuid::from_bytes([1; 16])));
    assert!(!cancelled.contains(&Uuid::from_bytes([2; 16])));
    assert!(cancelled.contains(&Uuid::from_bytes([3; 16])));
}

#[test]
fn test_cancel_by_pair() {
    let ctx = mm_ctx_with_iguana(None);
    let rx = prepare_for_cancel_by(&ctx);

    let connection = Connection::open_in_memory().unwrap();
    let _ = ctx
        .sqlite_connection
        .set(Arc::new(Mutex::new(connection)))
        .map_err(|_| "Already Initialized".to_string());

    delete_my_maker_order.mock_safe(|_, _, _| MockResult::Return(Box::new(futures01::future::ok(()))));
    delete_my_taker_order.mock_safe(|_, _, _| MockResult::Return(Box::new(futures01::future::ok(()))));

    let (cancelled, _) = block_on(cancel_orders_by(
        &ctx,
        CancelBy::Pair {
            base: "RICK".into(),
            rel: "MORTY".into(),
        },
    ))
    .unwrap();
    block_on(rx.take(1).collect::<Vec<_>>());
    assert!(cancelled.contains(&Uuid::from_bytes([0; 16])));
    assert!(!cancelled.contains(&Uuid::from_bytes([1; 16])));
    assert!(!cancelled.contains(&Uuid::from_bytes([2; 16])));
    assert!(cancelled.contains(&Uuid::from_bytes([3; 16])));
}

#[test]
fn test_cancel_by_all() {
    let ctx = mm_ctx_with_iguana(None);
    let rx = prepare_for_cancel_by(&ctx);

    let connection = Connection::open_in_memory().unwrap();
    let _ = ctx
        .sqlite_connection
        .set(Arc::new(Mutex::new(connection)))
        .map_err(|_| "Already Initialized".to_string());

    delete_my_maker_order.mock_safe(|_, _, _| MockResult::Return(Box::new(futures01::future::ok(()))));
    delete_my_taker_order.mock_safe(|_, _, _| MockResult::Return(Box::new(futures01::future::ok(()))));

    let (cancelled, _) = block_on(cancel_orders_by(&ctx, CancelBy::All)).unwrap();
    block_on(rx.take(3).collect::<Vec<_>>());
    assert!(cancelled.contains(&Uuid::from_bytes([0; 16])));
    assert!(cancelled.contains(&Uuid::from_bytes([1; 16])));
    assert!(cancelled.contains(&Uuid::from_bytes([2; 16])));
    assert!(cancelled.contains(&Uuid::from_bytes([3; 16])));
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/607
fn test_taker_order_match_by() {
    let uuid = new_uuid();

    let mut not_matching_uuids = HashSet::new();
    not_matching_uuids.insert(new_uuid());
    let request = TakerRequest {
        base: "BASE".into(),
        rel: "REL".into(),
        uuid,
        dest_pub_key: H256Json::default(),
        sender_pubkey: H256Json::default(),
        base_amount: 10.into(),
        rel_amount: 10.into(),
        action: TakerAction::Buy,
        match_by: MatchBy::Orders(not_matching_uuids),
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    let mut order = TakerOrder {
        request,
        matches: HashMap::new(),
        created_at: now_ms(),
        order_type: OrderType::GoodTillCancelled,
        min_volume: 0.into(),
        timeout: 30,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
    };

    let reserved = MakerReserved {
        base: "BASE".into(),
        rel: "REL".into(),
        base_amount: 10.into(),
        rel_amount: 10.into(),
        sender_pubkey: H256Json::default(),
        dest_pub_key: H256Json::default(),
        maker_order_uuid: new_uuid(),
        taker_order_uuid: uuid,
        conf_settings: None,
        base_protocol_info: None,
        rel_protocol_info: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
    };

    assert_eq!(MatchReservedResult::NotMatched, order.match_reserved(&reserved));

    let mut matching_uuids = HashSet::new();
    matching_uuids.insert(reserved.maker_order_uuid);
    order.request.match_by = MatchBy::Orders(matching_uuids);
    assert_eq!(MatchReservedResult::Matched, order.match_reserved(&reserved));

    let mut not_matching_pubkeys = HashSet::new();
    not_matching_pubkeys.insert([1; 32].into());
    order.request.match_by = MatchBy::Pubkeys(not_matching_pubkeys);
    assert_eq!(MatchReservedResult::NotMatched, order.match_reserved(&reserved));

    let mut matching_pubkeys = HashSet::new();
    matching_pubkeys.insert(H256Json::default());
    order.request.match_by = MatchBy::Pubkeys(matching_pubkeys);
    assert_eq!(MatchReservedResult::Matched, order.match_reserved(&reserved));
}

#[test]
fn test_maker_order_was_updated() {
    let created_at = now_ms();
    let mut maker_order = MakerOrder {
        base: "BASE".into(),
        rel: "REL".into(),
        created_at,
        updated_at: Some(created_at),
        max_base_vol: 10.into(),
        min_base_vol: 0.into(),
        price: 1.into(),
        matches: HashMap::new(),
        started_swaps: Vec::new(),
        uuid: new_uuid(),
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };
    let mut update_msg = MakerOrderUpdated::new(maker_order.uuid);
    update_msg.with_new_price(BigRational::from_integer(2.into()));

    std::thread::sleep(Duration::from_secs(1));

    maker_order.apply_updated(&update_msg);
    assert!(maker_order.was_updated());
}

#[test]
fn lp_connect_start_bob_should_not_be_invoked_if_order_match_already_connected() {
    let ctx = mm_ctx_with_iguana(Some(
        "also shoot benefit prefer juice shell elder veteran woman mimic image kidney",
    ));

    let order_json = r#"{"max_base_vol":"1","max_base_vol_rat":[[1,[1]],[1,[1]]],"min_base_vol":"0","min_base_vol_rat":[[0,[]],[1,[1]]],"price":"1","price_rat":[[1,[1]],[1,[1]]],"created_at":1589265312093,"updated_at":1589265312093,"base":"ETH","rel":"JST","matches":{"2f9afe84-7a89-4194-8947-45fba563118f":{"request":{"base":"ETH","rel":"JST","base_amount":"0.1","base_amount_rat":[[1,[1]],[1,[10]]],"rel_amount":"0.2","rel_amount_rat":[[1,[1]],[1,[5]]],"action":"Buy","uuid":"2f9afe84-7a89-4194-8947-45fba563118f","method":"request","sender_pubkey":"031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3","dest_pub_key":"0000000000000000000000000000000000000000000000000000000000000000","match_by":{"type":"Any"}},"reserved":{"base":"ETH","rel":"JST","base_amount":"0.1","base_amount_rat":[[1,[1]],[1,[10]]],"rel_amount":"0.1","rel_amount_rat":[[1,[1]],[1,[10]]],"taker_order_uuid":"2f9afe84-7a89-4194-8947-45fba563118f","maker_order_uuid":"5f6516ea-ccaa-453a-9e37-e1c2c0d527e3","method":"reserved","sender_pubkey":"c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3ed","dest_pub_key":"031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3"},"connect":{"taker_order_uuid":"2f9afe84-7a89-4194-8947-45fba563118f","maker_order_uuid":"5f6516ea-ccaa-453a-9e37-e1c2c0d527e3","method":"connect","sender_pubkey":"031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3","dest_pub_key":"c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3ed"},"connected":{"taker_order_uuid":"2f9afe84-7a89-4194-8947-45fba563118f","maker_order_uuid":"5f6516ea-ccaa-453a-9e37-e1c2c0d527e3","method":"connected","sender_pubkey":"c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3ed","dest_pub_key":"031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3"},"last_updated":1589265314408}},"started_swaps":["2f9afe84-7a89-4194-8947-45fba563118f"],"uuid":"5f6516ea-ccaa-453a-9e37-e1c2c0d527e3"}"#;
    let maker_order: MakerOrder = json::from_str(order_json).unwrap();

    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    ordermatch_ctx
        .maker_orders_ctx
        .lock()
        .add_order(ctx.weak(), maker_order, None);

    static CONNECT_START_CALLED: AtomicBool = AtomicBool::new(false);

    lp_connect_start_bob.mock_safe(|_, _, _, _| {
        CONNECT_START_CALLED.store(true, Ordering::Relaxed);

        MockResult::Return(())
    });

    let connect: TakerConnect = json::from_str(r#"{"taker_order_uuid":"2f9afe84-7a89-4194-8947-45fba563118f","maker_order_uuid":"5f6516ea-ccaa-453a-9e37-e1c2c0d527e3","method":"connect","sender_pubkey":"031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3","dest_pub_key":"c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3ed"}"#).unwrap();
    let mut prefixed_pub = connect.sender_pubkey.0.to_vec();
    prefixed_pub.insert(0, 2);
    block_on(process_taker_connect(
        ctx,
        PublicKey::from_slice(&prefixed_pub).unwrap().into(),
        connect,
    ));

    assert!(!CONNECT_START_CALLED.load(Ordering::Relaxed));
}

#[test]
fn should_process_request_only_once() {
    let ctx = mm_ctx_with_iguana(Some(
        "also shoot benefit prefer juice shell elder veteran woman mimic image kidney",
    ));

    let order_json = r#"{"max_base_vol":"1","max_base_vol_rat":[[1,[1]],[1,[1]]],"min_base_vol":"0","min_base_vol_rat":[[0,[]],[1,[1]]],"price":"1","price_rat":[[1,[1]],[1,[1]]],"created_at":1589265312093,"updated_at":1589265312093,"base":"ETH","rel":"JST","matches":{"2f9afe84-7a89-4194-8947-45fba563118f":{"request":{"base":"ETH","rel":"JST","base_amount":"0.1","base_amount_rat":[[1,[1]],[1,[10]]],"rel_amount":"0.2","rel_amount_rat":[[1,[1]],[1,[5]]],"action":"Buy","uuid":"2f9afe84-7a89-4194-8947-45fba563118f","method":"request","sender_pubkey":"031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3","dest_pub_key":"0000000000000000000000000000000000000000000000000000000000000000","match_by":{"type":"Any"}},"reserved":{"base":"ETH","rel":"JST","base_amount":"0.1","base_amount_rat":[[1,[1]],[1,[10]]],"rel_amount":"0.1","rel_amount_rat":[[1,[1]],[1,[10]]],"taker_order_uuid":"2f9afe84-7a89-4194-8947-45fba563118f","maker_order_uuid":"5f6516ea-ccaa-453a-9e37-e1c2c0d527e3","method":"reserved","sender_pubkey":"c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3ed","dest_pub_key":"031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3"},"connect":{"taker_order_uuid":"2f9afe84-7a89-4194-8947-45fba563118f","maker_order_uuid":"5f6516ea-ccaa-453a-9e37-e1c2c0d527e3","method":"connect","sender_pubkey":"031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3","dest_pub_key":"c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3ed"},"connected":{"taker_order_uuid":"2f9afe84-7a89-4194-8947-45fba563118f","maker_order_uuid":"5f6516ea-ccaa-453a-9e37-e1c2c0d527e3","method":"connected","sender_pubkey":"c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3ed","dest_pub_key":"031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3"},"last_updated":1589265314408}},"started_swaps":["2f9afe84-7a89-4194-8947-45fba563118f"],"uuid":"5f6516ea-ccaa-453a-9e37-e1c2c0d527e3"}"#;
    let maker_order: MakerOrder = json::from_str(order_json).unwrap();
    let uuid = maker_order.uuid;
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    ordermatch_ctx
        .maker_orders_ctx
        .lock()
        .add_order(ctx.weak(), maker_order, None);
    let request: TakerRequest = json::from_str(
        r#"{"base":"ETH","rel":"JST","base_amount":"0.1","base_amount_rat":[[1,[1]],[1,[10]]],"rel_amount":"0.2","rel_amount_rat":[[1,[1]],[1,[5]]],"action":"Buy","uuid":"2f9afe84-7a89-4194-8947-45fba563118f","method":"request","sender_pubkey":"031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3","dest_pub_key":"0000000000000000000000000000000000000000000000000000000000000000","match_by":{"type":"Any"}}"#,
    ).unwrap();
    block_on(process_taker_request(ctx, request));
    let maker_orders = &ordermatch_ctx.maker_orders_ctx.lock().orders;
    let order = block_on(maker_orders.get(&uuid).unwrap().lock());
    // when new request is processed match is replaced with new instance resetting
    // connect and connected to None so by checking is_some we check that request message is ignored
    assert!(order
        .matches
        .get(&"2f9afe84-7a89-4194-8947-45fba563118f".parse().unwrap())
        .unwrap()
        .connect
        .is_some());
    assert!(order
        .matches
        .get(&"2f9afe84-7a89-4194-8947-45fba563118f".parse().unwrap())
        .unwrap()
        .connected
        .is_some());
}

#[test]
fn test_choose_maker_confs_settings() {
    let coin = TestCoin::default().into();
    // no confs set
    let taker_order = TakerOrderBuilder::new(&coin, &coin).build_unchecked();
    TestCoin::requires_notarization.mock_safe(|_| MockResult::Return(true));
    TestCoin::required_confirmations.mock_safe(|_| MockResult::Return(8));
    let settings = choose_maker_confs_and_notas(None, &taker_order.request, &coin, &coin);
    // should pick settings from coin configuration
    assert!(settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 8);
    assert!(settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 8);

    let maker_conf_settings = OrderConfirmationsSettings {
        base_confs: 1,
        base_nota: false,
        rel_confs: 1,
        rel_nota: false,
    };
    // no confs set
    let taker_order = TakerOrderBuilder::new(&coin, &coin).build_unchecked();
    let settings = choose_maker_confs_and_notas(Some(maker_conf_settings), &taker_order.request, &coin, &coin);
    // should pick settings from maker order
    assert!(!settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 1);
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 1);

    let maker_conf_settings = OrderConfirmationsSettings {
        base_confs: 10,
        base_nota: true,
        rel_confs: 1,
        rel_nota: false,
    };
    let taker_conf_settings = OrderConfirmationsSettings {
        base_confs: 5,
        base_nota: false,
        rel_confs: 5,
        rel_nota: false,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_conf_settings(taker_conf_settings)
        .build_unchecked();
    let settings = choose_maker_confs_and_notas(Some(maker_conf_settings), &taker_order.request, &coin, &coin);
    // should pick settings from taker request because taker will wait less time for our
    // payment confirmation
    assert!(!settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 5);
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 1);

    let maker_conf_settings = OrderConfirmationsSettings {
        base_confs: 10,
        base_nota: false,
        rel_confs: 1,
        rel_nota: false,
    };
    let taker_conf_settings = OrderConfirmationsSettings {
        base_confs: 1000,
        base_nota: true,
        rel_confs: 1000,
        rel_nota: true,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_conf_settings(taker_conf_settings)
        .build_unchecked();
    let settings = choose_maker_confs_and_notas(Some(maker_conf_settings), &taker_order.request, &coin, &coin);
    // keep using our settings allowing taker to wait for our payment conf as much as he likes
    assert!(!settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 10);
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 1);

    let maker_conf_settings = OrderConfirmationsSettings {
        base_confs: 10,
        base_nota: false,
        rel_confs: 2,
        rel_nota: true,
    };

    let taker_conf_settings = OrderConfirmationsSettings {
        rel_confs: 1,
        rel_nota: false,
        base_confs: 1,
        base_nota: false,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_conf_settings(taker_conf_settings)
        .build_unchecked();
    let settings = choose_maker_confs_and_notas(Some(maker_conf_settings), &taker_order.request, &coin, &coin);

    // Taker conf settings should not have any effect on maker conf requirements for taker payment
    assert!(settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 2);

    let maker_conf_settings = OrderConfirmationsSettings {
        base_confs: 10,
        base_nota: true,
        rel_confs: 1,
        rel_nota: false,
    };
    // Pair is reversed for TakerAction::Sell
    let taker_conf_settings = OrderConfirmationsSettings {
        rel_confs: 5,
        rel_nota: false,
        base_confs: 5,
        base_nota: false,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_conf_settings(taker_conf_settings)
        .with_action(TakerAction::Sell)
        .build_unchecked();
    let settings = choose_maker_confs_and_notas(Some(maker_conf_settings), &taker_order.request, &coin, &coin);
    // should pick settings from taker request because taker will wait less time for our
    // payment confirmation
    assert!(!settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 5);
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 1);
}

#[test]
fn test_choose_taker_confs_settings_buy_action() {
    let coin = TestCoin::default().into();

    // no confs and notas set
    let taker_order = TakerOrderBuilder::new(&coin, &coin).build_unchecked();
    // no confs and notas set
    let maker_reserved = MakerReserved::default();
    TestCoin::requires_notarization.mock_safe(|_| MockResult::Return(true));
    TestCoin::required_confirmations.mock_safe(|_| MockResult::Return(8));
    let settings = choose_taker_confs_and_notas(&taker_order.request, &maker_reserved.conf_settings, &coin, &coin);
    // should pick settings from coins
    assert!(settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 8);
    assert!(settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 8);

    let taker_conf_settings = OrderConfirmationsSettings {
        base_confs: 5,
        base_nota: true,
        rel_confs: 4,
        rel_nota: false,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_conf_settings(taker_conf_settings)
        .build_unchecked();
    // no confs and notas set
    let maker_reserved = MakerReserved::default();
    let settings = choose_taker_confs_and_notas(&taker_order.request, &maker_reserved.conf_settings, &coin, &coin);
    // should pick settings from taker request
    // as action is buy my_coin is rel and other coin is base
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 4);
    assert!(settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 5);

    let taker_conf_settings = OrderConfirmationsSettings {
        base_confs: 2,
        base_nota: true,
        rel_confs: 2,
        rel_nota: true,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_conf_settings(taker_conf_settings)
        .build_unchecked();
    let mut maker_reserved = MakerReserved::default();
    let maker_conf_settings = OrderConfirmationsSettings {
        rel_confs: 1,
        rel_nota: false,
        base_confs: 2,
        base_nota: true,
    };
    maker_reserved.conf_settings = Some(maker_conf_settings);
    let settings = choose_taker_confs_and_notas(&taker_order.request, &maker_reserved.conf_settings, &coin, &coin);
    // should pick settings from maker reserved if he requires less confs
    // as action is buy my_coin is rel and other coin is base in request
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 1);
    assert!(settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 2);

    let taker_conf_settings = OrderConfirmationsSettings {
        base_confs: 2,
        base_nota: true,
        rel_confs: 1,
        rel_nota: false,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_conf_settings(taker_conf_settings)
        .build_unchecked();
    let mut maker_reserved = MakerReserved::default();
    let maker_conf_settings = OrderConfirmationsSettings {
        rel_confs: 2,
        rel_nota: true,
        base_confs: 2,
        base_nota: true,
    };
    maker_reserved.conf_settings = Some(maker_conf_settings);
    let settings = choose_taker_confs_and_notas(&taker_order.request, &maker_reserved.conf_settings, &coin, &coin);
    // should allow maker to use more confirmations than we require, but it shouldn't affect our settings
    // as action is buy my_coin is rel and other coin is base in request
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 1);
    assert!(settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 2);

    let taker_conf_settings = OrderConfirmationsSettings {
        base_confs: 2,
        base_nota: true,
        rel_confs: 1,
        rel_nota: false,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_conf_settings(taker_conf_settings)
        .build_unchecked();
    let mut maker_reserved = MakerReserved::default();
    let maker_conf_settings = OrderConfirmationsSettings {
        base_confs: 1,
        base_nota: false,
        rel_confs: 2,
        rel_nota: true,
    };
    maker_reserved.conf_settings = Some(maker_conf_settings);
    let settings = choose_taker_confs_and_notas(&taker_order.request, &maker_reserved.conf_settings, &coin, &coin);
    // maker settings should have no effect on other_coin_confs and other_coin_nota
    // as action is buy my_coin is rel and other coin is base in request
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 1);
    assert!(settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 2);
}

#[test]
fn test_choose_taker_confs_settings_sell_action() {
    let coin = TestCoin::default().into();

    // no confs and notas set
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_action(TakerAction::Sell)
        .build_unchecked();
    // no confs and notas set
    let maker_reserved = MakerReserved::default();
    TestCoin::requires_notarization.mock_safe(|_| MockResult::Return(true));
    TestCoin::required_confirmations.mock_safe(|_| MockResult::Return(8));
    let settings = choose_taker_confs_and_notas(&taker_order.request, &maker_reserved.conf_settings, &coin, &coin);
    // should pick settings from coins
    assert!(settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 8);
    assert!(settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 8);

    let taker_conf_settings = OrderConfirmationsSettings {
        base_confs: 4,
        base_nota: false,
        rel_confs: 5,
        rel_nota: true,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_action(TakerAction::Sell)
        .with_conf_settings(taker_conf_settings)
        .build_unchecked();
    // no confs and notas set
    let maker_reserved = MakerReserved::default();
    let settings = choose_taker_confs_and_notas(&taker_order.request, &maker_reserved.conf_settings, &coin, &coin);
    // should pick settings from taker request
    // as action is sell my_coin is base and other coin is rel in request
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 4);
    assert!(settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 5);

    let taker_conf_settings = OrderConfirmationsSettings {
        base_confs: 2,
        base_nota: true,
        rel_confs: 2,
        rel_nota: true,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_action(TakerAction::Sell)
        .with_conf_settings(taker_conf_settings)
        .build_unchecked();
    let mut maker_reserved = MakerReserved::default();
    let maker_conf_settings = OrderConfirmationsSettings {
        base_confs: 2,
        base_nota: true,
        rel_confs: 1,
        rel_nota: false,
    };
    maker_reserved.conf_settings = Some(maker_conf_settings);
    let settings = choose_taker_confs_and_notas(&taker_order.request, &maker_reserved.conf_settings, &coin, &coin);
    // should pick settings from maker reserved if he requires less confs
    // as action is sell my_coin is base and other coin is rel in request
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 1);
    assert!(settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 2);

    let taker_conf_settings = OrderConfirmationsSettings {
        base_confs: 1,
        base_nota: false,
        rel_confs: 2,
        rel_nota: true,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_action(TakerAction::Sell)
        .with_conf_settings(taker_conf_settings)
        .build_unchecked();
    let mut maker_reserved = MakerReserved::default();
    let maker_conf_settings = OrderConfirmationsSettings {
        rel_confs: 2,
        rel_nota: true,
        base_confs: 1,
        base_nota: false,
    };
    maker_reserved.conf_settings = Some(maker_conf_settings);
    let settings = choose_taker_confs_and_notas(&taker_order.request, &maker_reserved.conf_settings, &coin, &coin);
    // should allow maker to use more confirmations than we require, but it shouldn't affect our settings
    // as action is sell my_coin is base and other coin is rel in request
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 1);
    assert!(settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 2);

    let taker_conf_settings = OrderConfirmationsSettings {
        base_confs: 1,
        base_nota: false,
        rel_confs: 2,
        rel_nota: true,
    };
    let taker_order = TakerOrderBuilder::new(&coin, &coin)
        .with_action(TakerAction::Sell)
        .with_conf_settings(taker_conf_settings)
        .build_unchecked();
    let mut maker_reserved = MakerReserved::default();
    let maker_conf_settings = OrderConfirmationsSettings {
        rel_confs: 2,
        rel_nota: true,
        base_confs: 1,
        base_nota: false,
    };
    maker_reserved.conf_settings = Some(maker_conf_settings);
    let settings = choose_taker_confs_and_notas(&taker_order.request, &maker_reserved.conf_settings, &coin, &coin);
    // maker settings should have no effect on other_coin_confs and other_coin_nota
    // as action is sell my_coin is base and other coin is rel in request
    assert!(!settings.taker_coin_nota);
    assert_eq!(settings.taker_coin_confs, 1);
    assert!(settings.maker_coin_nota);
    assert_eq!(settings.maker_coin_confs, 2);
}

fn make_ctx_for_tests() -> (MmArc, String, [u8; 32]) {
    let ctx = MmArc::new(MmCtx::default());
    ctx.init_metrics().unwrap();
    let crypto_ctx = CryptoCtx::init_with_iguana_passphrase(ctx.clone(), "passphrase").unwrap();

    let secret = crypto_ctx.mm2_internal_privkey_secret().take();
    let pubkey = crypto_ctx.mm2_internal_pubkey_hex();
    (ctx, pubkey, secret)
}

pub(super) fn make_random_orders(
    pubkey: String,
    _secret: &[u8; 32],
    base: String,
    rel: String,
    n: usize,
) -> Vec<OrderbookItem> {
    let mut rng = rand::thread_rng();
    let mut orders = Vec::with_capacity(n);
    for _i in 0..n {
        let numer: u64 = rng.gen_range(2000, 10000000);
        let order = new_protocol::MakerOrderCreated {
            uuid: new_uuid().into(),
            base: base.clone(),
            rel: rel.clone(),
            price: BigRational::new(numer.into(), 1000000.into()),
            max_volume: BigRational::from_integer(1.into()),
            min_volume: BigRational::from_integer(0.into()),
            conf_settings: OrderConfirmationsSettings::default(),
            created_at: now_sec(),
            timestamp: now_sec(),
            pair_trie_root: H64::default(),
            base_protocol_info: vec![],
            rel_protocol_info: vec![],
        };

        orders.push((order, pubkey.clone()).into());
    }

    orders
}

fn pubkey_and_secret_for_test(passphrase: &str) -> (String, [u8; 32]) {
    let key_pair = key_pair_from_seed(passphrase).unwrap();
    let pubkey = hex::encode(&**key_pair.public());
    let secret = *key_pair.private().secret;
    (pubkey, secret)
}

fn init_p2p_context(ctx: &MmArc) -> (mpsc::Sender<AdexBehaviourCmd>, mpsc::Receiver<AdexBehaviourCmd>) {
    let (cmd_tx, cmd_rx) = mpsc::channel(10);

    let p2p_key = {
        let crypto_ctx = CryptoCtx::from_ctx(ctx).unwrap();
        let key = bitcrypto::sha256(crypto_ctx.mm2_internal_privkey_slice());
        key.take()
    };

    let p2p_context = P2PContext::new(cmd_tx.clone(), generate_ed25519_keypair(p2p_key));
    p2p_context.store_to_mm_arc(ctx);
    (cmd_tx, cmd_rx)
}

#[test]
fn test_process_get_orderbook_request() {
    const ORDERS_NUMBER: usize = 10;

    let (ctx, _pubkey, _secret) = make_ctx_for_tests();
    let (pubkey1, secret1) = pubkey_and_secret_for_test("passphrase-1");
    let (pubkey2, secret2) = pubkey_and_secret_for_test("passphrase-2");
    let (pubkey3, secret3) = pubkey_and_secret_for_test("passphrase-3");

    let mut pubkey1_orders =
        make_random_orders(pubkey1.clone(), &secret1, "RICK".into(), "MORTY".into(), ORDERS_NUMBER);
    let mut pubkey2_orders =
        make_random_orders(pubkey2.clone(), &secret2, "MORTY".into(), "RICK".into(), ORDERS_NUMBER);
    let mut pubkey3_orders =
        make_random_orders(pubkey3.clone(), &secret3, "RICK".into(), "MORTY".into(), ORDERS_NUMBER);
    pubkey3_orders.extend_from_slice(&make_random_orders(
        pubkey3.clone(),
        &secret3,
        "MORTY".into(),
        "RICK".into(),
        ORDERS_NUMBER,
    ));

    pubkey1_orders.sort_unstable_by(|x, y| x.uuid.cmp(&y.uuid));
    pubkey2_orders.sort_unstable_by(|x, y| x.uuid.cmp(&y.uuid));
    pubkey3_orders.sort_unstable_by(|x, y| x.uuid.cmp(&y.uuid));

    let mut orders_by_pubkeys = HashMap::new();
    orders_by_pubkeys.insert(pubkey1, pubkey1_orders);
    orders_by_pubkeys.insert(pubkey2, pubkey2_orders);
    orders_by_pubkeys.insert(pubkey3, pubkey3_orders);

    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let mut orderbook = ordermatch_ctx.orderbook.lock();

    for order in orders_by_pubkeys.values().flatten() {
        orderbook.insert_or_update_order_update_trie(order.clone());
    }

    // avoid dead lock on orderbook as process_get_orderbook_request also acquires it
    drop(orderbook);

    let encoded = process_get_orderbook_request(ctx, "RICK".into(), "MORTY".into())
        .unwrap()
        .unwrap();

    let orderbook = decode_message::<GetOrderbookRes>(&encoded).unwrap();
    for (pubkey, item) in orderbook.pubkey_orders {
        let expected = orders_by_pubkeys
            .get(&pubkey)
            .unwrap_or_else(|| panic!("!best_orders_by_pubkeys is expected to contain {:?}", pubkey));

        let mut actual: Vec<OrderbookItem> = item
            .orders
            .iter()
            .map(|(_uuid, order)| {
                OrderbookItem::from_p2p_and_info(
                    order.clone(),
                    BaseRelProtocolInfo::default(),
                    Some(OrderConfirmationsSettings::default()),
                )
            })
            .collect();
        actual.sort_unstable_by(|x, y| x.uuid.cmp(&y.uuid));
        log!("{:?}-{:?}", pubkey, actual.len());
        assert_eq!(actual, *expected);
    }
}

#[test]
fn test_process_get_orderbook_request_limit() {
    let (ctx, pubkey, secret) = make_ctx_for_tests();
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let mut orderbook = ordermatch_ctx.orderbook.lock();

    let orders = make_random_orders(
        pubkey,
        &secret,
        "RICK".into(),
        "MORTY".into(),
        MAX_ORDERS_NUMBER_IN_ORDERBOOK_RESPONSE + 1,
    );

    for order in orders {
        orderbook.insert_or_update_order_update_trie(order);
    }

    // avoid dead lock on orderbook as process_get_orderbook_request also acquires it
    drop(orderbook);

    let err = process_get_orderbook_request(ctx, "RICK".into(), "MORTY".into()).expect_err("Expected an error");

    log!("error: {}", err);
    assert!(err.contains("Orderbook too large"));
}

#[test]
fn test_request_and_fill_orderbook() {
    const PUBKEYS_NUMBER: usize = 3;
    const ORDERS_NUMBER: usize = 10;

    let (ctx, _pubkey, _secret) = make_ctx_for_tests();
    let (_, mut cmd_rx) = init_p2p_context(&ctx);

    let other_pubkeys: Vec<(String, [u8; 32])> = (0..PUBKEYS_NUMBER)
        .map(|idx| {
            let passphrase = format!("passphrase-{idx}");
            pubkey_and_secret_for_test(&passphrase)
        })
        .collect();
    let expected_orders: HashMap<String, Vec<(Uuid, OrderbookItem)>> = other_pubkeys
        .iter()
        .map(|(pubkey, secret)| {
            let orders: Vec<_> =
                make_random_orders(pubkey.clone(), secret, "RICK".into(), "MORTY".into(), ORDERS_NUMBER)
                    .into_iter()
                    .map(|order| (order.uuid, order))
                    .collect();
            (pubkey.clone(), orders)
        })
        .collect();

    // insert extra (RICK, MORTY) orders that must be removed from our trie before the orderbook is filled
    {
        let (pubkey, secret) = &other_pubkeys[0];
        for extra_order in make_random_orders(pubkey.clone(), secret, "RICK".into(), "MORTY".into(), 2) {
            insert_or_update_order(&ctx, extra_order);
        }
    }

    let expected_request = P2PRequest::Ordermatch(OrdermatchRequest::GetOrderbook {
        base: "RICK".into(),
        rel: "MORTY".into(),
    });

    let orders = expected_orders.clone();
    spawn(async move {
        let cmd = cmd_rx.next().await.unwrap();
        let (req, response_tx) = if let AdexBehaviourCmd::RequestAnyRelay { req, response_tx } = cmd {
            (req, response_tx)
        } else {
            panic!("Unexpected cmd");
        };

        // check if the received request is expected
        let actual = decode_message::<P2PRequest>(&req).unwrap();
        assert_eq!(actual, expected_request);

        let mut conf_infos = HashMap::new();
        let result = orders
            .into_iter()
            .map(|(pubkey, orders)| {
                let orders = orders
                    .into_iter()
                    .map(|(uuid, order)| {
                        if let Some(ref conf_settings) = order.conf_settings {
                            conf_infos.insert(uuid, conf_settings.clone());
                        }
                        (uuid, order.into())
                    })
                    .collect();
                let item = GetOrderbookPubkeyItem {
                    orders,
                    last_keep_alive: now_sec(),
                    last_signed_pubkey_payload: vec![],
                };
                (pubkey, item)
            })
            .collect();
        let orderbook = GetOrderbookRes {
            pubkey_orders: result,
            protocol_infos: HashMap::new(),
            conf_infos,
        };
        let encoded = encode_message(&orderbook).unwrap();

        // send the response through the response channel
        response_tx.send(Some((PeerId::random(), encoded))).unwrap();
    });

    block_on(request_and_fill_orderbook(&ctx, "RICK", "MORTY")).unwrap();

    // check if the best asks and bids are in the orderbook
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let orderbook = ordermatch_ctx.orderbook.lock();

    let expected = expected_orders.values().flatten().cloned().collect();
    assert_eq!(orderbook.order_set, expected);

    let expected = expected_orders.values().flatten().map(|(uuid, _order)| *uuid).collect();
    let unordered = orderbook
        .unordered
        .get(&("RICK".to_owned(), "MORTY".to_owned()))
        .expect("No (RICK, MORTY) in unordered container");
    assert_eq!(*unordered, expected);

    let expected = expected_orders
        .values()
        .flatten()
        .map(|(uuid, order)| OrderedByPriceOrder {
            uuid: *uuid,
            price: order.price.clone().into(),
        })
        .collect();
    let ordered = orderbook
        .ordered
        .get(&("RICK".to_owned(), "MORTY".to_owned()))
        .expect("No (RICK, MORTY) in unordered container");
    assert_eq!(*ordered, expected);

    let rick_morty_pair = alb_ordered_pair("RICK", "MORTY");
    for (pubkey, orders) in expected_orders {
        let pubkey_state = orderbook
            .pubkeys_state
            .get(&pubkey)
            .unwrap_or_else(|| panic!("!pubkey_state.get() {} pubkey", pubkey));

        let expected = orders
            .iter()
            .map(|(uuid, _order)| (*uuid, rick_morty_pair.clone()))
            .collect();
        assert_eq!(pubkey_state.orders_uuids, expected);

        let root = pubkey_state
            .trie_roots
            .get(&rick_morty_pair)
            .unwrap_or_else(|| panic!("!pubkey_state.trie_roots.get() {}", rick_morty_pair));

        // check if the root contains only expected orders
        let trie = TrieDB::<Layout>::new(&orderbook.memory_db, root).expect("!TrieDB::new()");
        let mut in_trie: Vec<(Uuid, OrderbookItem)> = trie
            .iter()
            .expect("!TrieDB::iter()")
            .map(|key_value| {
                let (key, _) = key_value.expect("Iterator returned an error");
                let key = TryFromBytes::try_from_bytes(key).expect("!try_from_bytes() key");
                let value = orderbook.order_set.get(&key).cloned().unwrap();
                (key, value)
            })
            .collect();

        in_trie.sort_by(|x, y| x.0.cmp(&y.0));
        let mut expected = orders;
        expected.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(in_trie, expected);
    }
}

/*
#[test]
fn test_process_order_keep_alive_requested_from_peer() {
    let ordermatch_ctx = Arc::new(OrdermatchContext::default());
    let ordermatch_ctx_clone = ordermatch_ctx.clone();
    OrdermatchContext::from_ctx.mock_safe(move |_| MockResult::Return(Ok(ordermatch_ctx_clone.clone())));

    let (ctx, pubkey, secret) = make_ctx_for_tests();
    let (_, mut cmd_rx) = init_p2p_context(&ctx);

    let uuid = new_uuid();
    let peer = PeerId::random().to_string();

    let order = new_protocol::MakerOrderCreated {
        uuid: uuid.clone().into(),
        base: "RICK".into(),
        rel: "MORTY".into(),
        price: BigRational::from_integer(1000000.into()),
        max_volume: BigRational::from_integer(2000000.into()),
        min_volume: BigRational::from_integer(2000000.into()),
        conf_settings: OrderConfirmationsSettings::default(),
    };

    // create an initial_message and encode it with the secret
    let initial_order_message = encode_and_sign(
        &new_protocol::OrdermatchMessage::MakerOrderCreated(order.clone()),
        &secret,
    )
    .unwrap();

    let expected_request = P2PRequest::Ordermatch(OrdermatchRequest::GetOrders {
        pairs: vec![("RICK".into(), "MORTY".into())],
        from_pubkey: pubkey.clone(),
    });
    let from_peer = peer.clone();
    let initial_message = initial_order_message.clone();
    spawn(async move {
        let cmd = cmd_rx.next().await.unwrap();
        let (req, response_tx) = if let AdexBehaviourCmd::RequestPeers { req, response_tx, .. } = cmd {
            (req, response_tx)
        } else {
            panic!("Unexpected cmd");
        };

        // check if the received request is expected
        let actual = decode_message::<P2PRequest>(&req).unwrap();
        assert_eq!(actual, expected_request);

        // create a response with the initial_message and random from_peer
        let response = vec![new_protocol::OrderInitialMessage {
            initial_message,
            from_peer: from_peer.clone(),
            update_messages: Vec::new(),
        }];

        let response = AdexResponse::Ok {
            response: encode_message(&response).unwrap(),
        };
        response_tx.send(vec![(PeerId::random(), response)]).unwrap();
    });

    let keep_alive = new_protocol::MakerOrdersKeepAlive {
        timestamp: now_ms(),
        num_orders: HashMap::from_iter(iter::once((("RICK".into(), "MORTY".into()), 1))),
    };

    // process_order_keep_alive() should return true because an order was successfully requested from a peer.
    assert!(block_on(process_orders_keep_alive(
        ctx,
        peer.clone(),
        pubkey.clone(),
        keep_alive
    )));

    let mut orderbook = block_on(ordermatch_ctx.orderbook.lock());
    // try to find the order within OrdermatchContext::orderbook and check if this order equals to the expected
    let actual = orderbook.find_order_by_uuid_and_pubkey(&uuid, &pubkey).unwrap();
    let expected: OrderbookItem = (order, pubkey).into();

    assert_eq!(actual, &expected);
}

#[test]
fn test_process_get_order_request() {
    let (ctx, pubkey, secret) = make_ctx_for_tests();
    let ordermatch_ctx = Arc::new(OrdermatchContext::default());
    let ordermatch_ctx_clone = ordermatch_ctx.clone();
    OrdermatchContext::from_ctx.mock_safe(move |_| MockResult::Return(Ok(ordermatch_ctx_clone.clone())));

    let mut orderbook = block_on(ordermatch_ctx.orderbook.lock());

    let order = new_protocol::MakerOrderCreated {
        uuid: new_uuid().into(),
        base: "RICK".into(),
        rel: "MORTY".into(),
        price: BigRational::from_integer(1000000.into()),
        max_volume: BigRational::from_integer(2000000.into()),
        min_volume: BigRational::from_integer(2000000.into()),
        conf_settings: OrderConfirmationsSettings::default(),
    };
    // create an initial_message and encode it with the secret
    let initial_message = encode_and_sign(
        &new_protocol::OrdermatchMessage::MakerOrderCreated(order.clone()),
        &secret,
    )
    .unwrap();
    let price_ping_request: OrderbookItem = (order, pubkey.clone()).into();
    orderbook.insert_or_update_order(price_ping_request.clone());

    // avoid dead lock on orderbook as process_get_orderbook_request also acquires it
    drop(orderbook);

    let encoded = block_on(process_get_order_request(
        ctx.clone(),
        price_ping_request.uuid,
        pubkey.clone(),
    ))
    .unwrap()
    .unwrap();

    let order = decode_message::<new_protocol::OrderInitialMessage>(&encoded).unwrap();
    let actual_price_ping_request = OrderbookItem::from_initial_msg(order.initial_message, order.from_peer).unwrap();
    assert_eq!(actual_price_ping_request, price_ping_request);
}

#[test]
fn test_subscribe_to_ordermatch_topic_not_subscribed() {
    let (ctx, _pubkey, _secret) = make_ctx_for_tests();
    let (_, mut cmd_rx) = init_p2p_context(&ctx);

    spawn(async move {
        match cmd_rx.next().await.unwrap() {
            AdexBehaviourCmd::Subscribe { .. } => (),
            _ => panic!("AdexBehaviourCmd::Subscribe expected first"),
        }

        let (req, response_tx) = match cmd_rx.next().await.unwrap() {
            AdexBehaviourCmd::RequestRelays { req, response_tx } => (req, response_tx),
            _ => panic!("AdexBehaviourCmd::RequestRelays expected"),
        };

        let request = decode_message::<P2PRequest>(&req).unwrap();
        match request {
            P2PRequest::Ordermatch(OrdermatchRequest::GetOrderbook { .. }) => (),
            _ => panic!(),
        }

        let response = new_protocol::Orderbook {
            asks: Vec::new(),
            bids: Vec::new(),
        };
        let encoded = encode_message(&response).unwrap();
        let response = vec![(PeerId::random(), AdexResponse::Ok { response: encoded })];
        response_tx.send(response).unwrap();
    });

    block_on(subscribe_to_orderbook_topic(&ctx, "RICK", "MORTY", true)).unwrap();

    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let orderbook = block_on(ordermatch_ctx.orderbook.lock());

    let actual = orderbook
        .topics_subscribed_to
        .get(&orderbook_topic("RICK", "MORTY"))
        .cloned();
    let expected = Some(OrderbookRequestingState::Requested);
    assert_eq!(actual, expected);
}

#[test]
fn test_subscribe_to_ordermatch_topic_subscribed_not_filled() {
    let (ctx, _pubkey, _secret) = make_ctx_for_tests();
    let (_, mut cmd_rx) = init_p2p_context(&ctx);

    {
        let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
        let mut orderbook = block_on(ordermatch_ctx.orderbook.lock());
        // not enough time has passed for the orderbook to be filled
        let subscribed_at = now_ms() / 1000 - ORDERBOOK_REQUESTING_TIMEOUT + 1;
        orderbook.topics_subscribed_to.insert(
            orderbook_topic("RICK", "MORTY"),
            OrderbookRequestingState::NotRequested { subscribed_at },
        );
    }

    spawn(async move {
        let (req, response_tx) = match cmd_rx.next().await.unwrap() {
            AdexBehaviourCmd::RequestRelays { req, response_tx } => (req, response_tx),
            _ => panic!("AdexBehaviourCmd::RequestRelays expected"),
        };

        let request = decode_message::<P2PRequest>(&req).unwrap();
        match request {
            P2PRequest::Ordermatch(OrdermatchRequest::GetOrderbook { .. }) => (),
            _ => panic!(),
        }

        let response = new_protocol::Orderbook {
            asks: Vec::new(),
            bids: Vec::new(),
        };
        let encoded = encode_message(&response).unwrap();
        let response = vec![(PeerId::random(), AdexResponse::Ok { response: encoded })];
        response_tx.send(response).unwrap();
    });

    block_on(subscribe_to_orderbook_topic(&ctx, "RICK", "MORTY", true)).unwrap();

    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let orderbook = block_on(ordermatch_ctx.orderbook.lock());

    let actual = orderbook
        .topics_subscribed_to
        .get(&orderbook_topic("RICK", "MORTY"))
        .cloned();
    let expected = Some(OrderbookRequestingState::Requested);
    assert_eq!(actual, expected);

    // orderbook.topics_subscribed_to.insert(orderbook_topic("RICK", "MORTY"), OrderbookSubscriptionState::NotRequested {subscribed_at: now_ms() - 41});
}

#[test]
fn test_subscribe_to_ordermatch_topic_subscribed_filled() {
    let (ctx, _pubkey, _secret) = make_ctx_for_tests();
    let (_, mut cmd_rx) = init_p2p_context(&ctx);

    // enough time has passed for the orderbook to be filled
    let subscribed_at = now_ms() / 1000 - ORDERBOOK_REQUESTING_TIMEOUT - 1;
    {
        let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
        let mut orderbook = block_on(ordermatch_ctx.orderbook.lock());
        orderbook.topics_subscribed_to.insert(
            orderbook_topic("RICK", "MORTY"),
            OrderbookRequestingState::NotRequested { subscribed_at },
        );
    }

    spawn(async move {
        assert!(cmd_rx.next().await.is_none(), "No commands expected");
    });

    block_on(subscribe_to_orderbook_topic(&ctx, "RICK", "MORTY", true)).unwrap();

    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let orderbook = block_on(ordermatch_ctx.orderbook.lock());

    let actual = orderbook
        .topics_subscribed_to
        .get(&orderbook_topic("RICK", "MORTY"))
        .cloned();
    let expected = Some(OrderbookRequestingState::NotRequested { subscribed_at });
    assert_eq!(actual, expected);
}
*/

#[test]
fn test_taker_request_can_match_with_maker_pubkey() {
    let coin = TestCoin::default().into();

    let maker_pubkey = H256Json::default();

    // default has MatchBy::Any
    let mut order = TakerOrderBuilder::new(&coin, &coin).build_unchecked();
    assert!(order.request.can_match_with_maker_pubkey(&maker_pubkey));

    // the uuids of orders is checked in another method
    order.request.match_by = MatchBy::Orders(HashSet::new());
    assert!(order.request.can_match_with_maker_pubkey(&maker_pubkey));

    let mut set = HashSet::new();
    set.insert(maker_pubkey);
    order.request.match_by = MatchBy::Pubkeys(set);
    assert!(order.request.can_match_with_maker_pubkey(&maker_pubkey));

    order.request.match_by = MatchBy::Pubkeys(HashSet::new());
    assert!(!order.request.can_match_with_maker_pubkey(&maker_pubkey));
}

#[test]
fn test_taker_request_can_match_with_uuid() {
    let uuid = new_uuid();
    let coin = MmCoinEnum::Test(TestCoin::default());

    // default has MatchBy::Any
    let mut order = TakerOrderBuilder::new(&coin, &coin).build_unchecked();
    assert!(order.request.can_match_with_uuid(&uuid));

    // the uuids of orders is checked in another method
    order.request.match_by = MatchBy::Pubkeys(HashSet::new());
    assert!(order.request.can_match_with_uuid(&uuid));

    let mut set = HashSet::new();
    set.insert(uuid);
    order.request.match_by = MatchBy::Orders(set);
    assert!(order.request.can_match_with_uuid(&uuid));

    order.request.match_by = MatchBy::Orders(HashSet::new());
    assert!(!order.request.can_match_with_uuid(&uuid));
}

#[test]
fn test_orderbook_insert_or_update_order() {
    let (_, pubkey, secret) = make_ctx_for_tests();
    let mut orderbook = Orderbook::default();
    let order = make_random_orders(pubkey, &secret, "C1".into(), "C2".into(), 1).remove(0);
    orderbook.insert_or_update_order_update_trie(order);
}

fn pair_trie_root_by_pub(ctx: &MmArc, pubkey: &str, pair: &str) -> H64 {
    let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).unwrap();
    let orderbook = ordermatch_ctx.orderbook.lock();
    *orderbook
        .pubkeys_state
        .get(pubkey)
        .unwrap()
        .trie_roots
        .get(pair)
        .unwrap()
}

fn clone_orderbook_memory_db(ctx: &MmArc) -> MemoryDB<Blake2Hasher64> {
    let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).unwrap();
    let orderbook = ordermatch_ctx.orderbook.lock();
    orderbook.memory_db.clone()
}

fn remove_order(ctx: &MmArc, uuid: Uuid) {
    let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).unwrap();
    let mut orderbook = ordermatch_ctx.orderbook.lock();
    orderbook.remove_order_trie_update(uuid);
}

#[test]
fn test_process_sync_pubkey_orderbook_state_after_new_orders_added() {
    let (ctx, pubkey, secret) = make_ctx_for_tests();
    let orders = make_random_orders(pubkey.clone(), &secret, "C1".into(), "C2".into(), 100);

    for order in orders {
        insert_or_update_order(&ctx, order);
    }

    let alb_ordered_pair = alb_ordered_pair("C1", "C2");
    let pair_trie_root = pair_trie_root_by_pub(&ctx, &pubkey, &alb_ordered_pair);

    let prev_pairs_state = HashMap::from_iter(iter::once((alb_ordered_pair.clone(), pair_trie_root)));

    let mut old_mem_db = clone_orderbook_memory_db(&ctx);

    let new_orders = make_random_orders(pubkey.clone(), &secret, "C1".into(), "C2".into(), 100);
    for order in new_orders {
        insert_or_update_order(&ctx, order.clone());
    }

    let mut result = process_sync_pubkey_orderbook_state(ctx.clone(), pubkey.clone(), prev_pairs_state)
        .unwrap()
        .unwrap();

    // check pair trie root
    let expected_root_hash = pair_trie_root_by_pub(&ctx, &pubkey, &alb_ordered_pair);

    let delta = match result.pair_orders_diff.remove(&alb_ordered_pair).unwrap() {
        DeltaOrFullTrie::Delta(delta) => delta,
        DeltaOrFullTrie::FullTrie(_) => panic!("Must be DeltaOrFullTrie::Delta"),
    };

    let actual_root_hash = delta_trie_root::<Layout, _, _, _, _, _>(
        &mut old_mem_db,
        pair_trie_root,
        delta.into_iter().map(|(uuid, order)| {
            (
                *uuid.as_bytes(),
                order.map(|o| {
                    let o = OrderbookItem::from_p2p_and_info(o, BaseRelProtocolInfo::default(), None);
                    o.trie_state_bytes()
                }),
            )
        }),
    )
    .unwrap();
    assert_eq!(expected_root_hash, actual_root_hash);
}

#[test]
fn test_diff_should_not_be_written_if_hash_not_changed_on_insert() {
    let (ctx, pubkey, secret) = make_ctx_for_tests();
    let orders = make_random_orders(pubkey.clone(), &secret, "C1".into(), "C2".into(), 100);

    for order in orders.clone() {
        insert_or_update_order(&ctx, order);
    }

    let alb_ordered_pair = alb_ordered_pair("C1", "C2");
    let pair_trie_root = pair_trie_root_by_pub(&ctx, &pubkey, &alb_ordered_pair);
    for order in orders {
        insert_or_update_order(&ctx, order);
    }

    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let orderbook = ordermatch_ctx.orderbook.lock();
    let pubkey_state = orderbook.pubkeys_state.get(&pubkey).unwrap();
    assert!(!pubkey_state
        .order_pairs_trie_state_history
        .get(&alb_ordered_pair)
        .expect("Must contain C1:C2 pair")
        .contains_key(&pair_trie_root));
}

#[test]
fn test_process_sync_pubkey_orderbook_state_after_orders_removed() {
    let (ctx, pubkey, secret) = make_ctx_for_tests();
    let orders = make_random_orders(pubkey.clone(), &secret, "C1".into(), "C2".into(), 100);

    for order in orders.clone() {
        insert_or_update_order(&ctx, order);
    }

    let alb_ordered_pair = alb_ordered_pair("C1", "C2");
    let pair_trie_root = pair_trie_root_by_pub(&ctx, &pubkey, &alb_ordered_pair);

    let prev_pairs_state = HashMap::from_iter(iter::once((alb_ordered_pair.clone(), pair_trie_root)));

    let mut old_mem_db = clone_orderbook_memory_db(&ctx);

    // pick 10 orders at random and remove them
    let mut rng = thread_rng();
    let to_remove = orders.choose_multiple(&mut rng, 10);
    for order in to_remove {
        remove_order(&ctx, order.uuid);
    }

    let mut result = process_sync_pubkey_orderbook_state(ctx.clone(), pubkey.clone(), prev_pairs_state)
        .unwrap()
        .unwrap();

    // check pair trie root
    let expected_root_hash = pair_trie_root_by_pub(&ctx, &pubkey, &alb_ordered_pair);

    let delta = match result.pair_orders_diff.remove(&alb_ordered_pair).unwrap() {
        DeltaOrFullTrie::Delta(delta) => delta,
        DeltaOrFullTrie::FullTrie(_) => panic!("Must be DeltaOrFullTrie::Delta"),
    };

    let actual_root_hash = delta_trie_root::<Layout, _, _, _, _, _>(
        &mut old_mem_db,
        pair_trie_root,
        delta
            .into_iter()
            .map(|(uuid, order)| (*uuid.as_bytes(), order.map(|o| encode_message(&o).unwrap()))),
    )
    .unwrap();
    assert_eq!(expected_root_hash, actual_root_hash);
}

#[test]
fn test_diff_should_not_be_written_if_hash_not_changed_on_remove() {
    let (ctx, pubkey, secret) = make_ctx_for_tests();
    let orders = make_random_orders(pubkey.clone(), &secret, "C1".into(), "C2".into(), 100);

    for order in orders.clone() {
        insert_or_update_order(&ctx, order);
    }

    let to_remove: Vec<_> = orders
        .choose_multiple(&mut thread_rng(), 10)
        .map(|order| order.uuid)
        .collect();
    for uuid in &to_remove {
        remove_order(&ctx, *uuid);
    }
    for uuid in &to_remove {
        remove_order(&ctx, *uuid);
    }

    let alb_ordered_pair = alb_ordered_pair("C1", "C2");
    let pair_trie_root = pair_trie_root_by_pub(&ctx, &pubkey, &alb_ordered_pair);

    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let orderbook = ordermatch_ctx.orderbook.lock();
    let pubkey_state = orderbook.pubkeys_state.get(&pubkey).unwrap();
    assert!(!pubkey_state
        .order_pairs_trie_state_history
        .get(&alb_ordered_pair)
        .expect("Must contain C1:C2 pair")
        .contains_key(&pair_trie_root));
}

#[test]
fn test_orderbook_pubkey_sync_request() {
    let mut orderbook = Orderbook::default();
    orderbook.topics_subscribed_to.insert(
        orderbook_topic_from_base_rel("C1", "C2"),
        OrderbookRequestingState::Requested,
    );
    let pubkey = "pubkey";

    let mut trie_roots = HashMap::new();
    trie_roots.insert("C1:C2".to_owned(), [1; 8]);
    trie_roots.insert("C2:C3".to_owned(), [1; 8]);

    let message = PubkeyKeepAlive {
        trie_roots,
        timestamp: now_sec(),
    };

    let request = orderbook.process_keep_alive(pubkey, message, false).unwrap();
    match request {
        OrdermatchRequest::SyncPubkeyOrderbookState {
            trie_roots: pairs_trie_roots,
            ..
        } => {
            assert!(pairs_trie_roots.contains_key("C1:C2"));
            assert!(!pairs_trie_roots.contains_key("C2:C3"));
        },
        _ => panic!("Invalid request {:?}", request),
    }
}

#[test]
fn test_orderbook_pubkey_sync_request_relay() {
    let mut orderbook = Orderbook::default();
    orderbook.topics_subscribed_to.insert(
        orderbook_topic_from_base_rel("C1", "C2"),
        OrderbookRequestingState::Requested,
    );
    let pubkey = "pubkey";

    let mut trie_roots = HashMap::new();
    trie_roots.insert("C1:C2".to_owned(), [1; 8]);
    trie_roots.insert("C2:C3".to_owned(), [1; 8]);

    let message = PubkeyKeepAlive {
        trie_roots,
        timestamp: now_sec(),
    };

    let request = orderbook.process_keep_alive(pubkey, message, true).unwrap();
    match request {
        OrdermatchRequest::SyncPubkeyOrderbookState {
            trie_roots: pairs_trie_roots,
            ..
        } => {
            assert!(pairs_trie_roots.contains_key("C1:C2"));
            assert!(pairs_trie_roots.contains_key("C2:C3"));
        },
        _ => panic!("Invalid request {:?}", request),
    }
}

#[test]
fn test_trie_diff_avoid_cycle_on_insertion() {
    let mut history = TrieDiffHistory::<String, String> { inner: TimedMap::new() };
    history.insert_new_diff(
        [1; 8],
        TrieDiff {
            delta: vec![],
            next_root: [2; 8],
        },
    );
    history.insert_new_diff(
        [2; 8],
        TrieDiff {
            delta: vec![],
            next_root: [3; 8],
        },
    );
    history.insert_new_diff(
        [3; 8],
        TrieDiff {
            delta: vec![],
            next_root: [4; 8],
        },
    );
    history.insert_new_diff(
        [4; 8],
        TrieDiff {
            delta: vec![],
            next_root: [5; 8],
        },
    );
    history.insert_new_diff(
        [5; 8],
        TrieDiff {
            delta: vec![],
            next_root: [2; 8],
        },
    );

    let expected = TrieDiff {
        delta: vec![],
        next_root: [2; 8],
    };

    assert_eq!(&expected, history.inner.get(&[1u8; 8]).unwrap());
}

#[test]
fn test_process_sync_pubkey_orderbook_state_points_to_not_uptodate_trie_root() {
    let (ctx, pubkey, secret) = make_ctx_for_tests();
    let orders = make_random_orders(pubkey.clone(), &secret, "RICK".into(), "MORTY".into(), 10);
    let new_order = make_random_orders(pubkey.clone(), &secret, "RICK".into(), "MORTY".into(), 1)
        .pop()
        .expect("Expected one order");

    for order in orders.iter() {
        insert_or_update_order(&ctx, order.clone());
    }

    let alb_pair = alb_ordered_pair("RICK", "MORTY");

    // update trie root by adding a new order and do not update history
    let (old_root, _new_root) = {
        let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
        let mut orderbook = ordermatch_ctx.orderbook.lock();

        log!("{:?}, found {:?}", pubkey, orderbook.pubkeys_state.keys());
        let old_root = *orderbook
            .pubkeys_state
            .get_mut(&pubkey)
            .expect("!pubkeys_state")
            .trie_roots
            .get(&alb_pair)
            .expect("MORTY:RICK must be in trie_roots");

        let order_bytes = new_order.trie_state_bytes();
        let mut new_root = old_root;
        let mut trie = get_trie_mut(&mut orderbook.memory_db, &mut new_root).expect("!get_trie_mut");
        trie.insert(new_order.uuid.as_bytes(), &order_bytes)
            .expect("Error on order insertion");
        drop(trie);

        // update root in orderbook trie_roots
        orderbook
            .pubkeys_state
            .get_mut(&pubkey)
            .expect("!pubkeys_state")
            .trie_roots
            .insert(alb_pair.clone(), new_root);

        orderbook.order_set.insert(new_order.uuid, new_order.clone());
        (old_root, new_root)
    };

    let mut roots = HashMap::new();
    roots.insert(alb_pair.clone(), old_root);

    let SyncPubkeyOrderbookStateRes {
        mut pair_orders_diff, ..
    } = process_sync_pubkey_orderbook_state(ctx, pubkey, roots)
        .expect("!process_sync_pubkey_orderbook_state")
        .expect("Expected MORTY:RICK delta, returned None");

    let delta = pair_orders_diff.remove(&alb_pair).expect("Expected MORTY:RICK delta");
    let mut full_trie = match delta {
        DeltaOrFullTrie::Delta(_) => panic!("Expected FullTrie, found Delta"),
        DeltaOrFullTrie::FullTrie(full_trie) => full_trie,
    };

    let mut expected: Vec<(Uuid, OrderbookP2PItem)> =
        orders.into_iter().map(|order| (order.uuid, order.into())).collect();
    expected.push((new_order.uuid, new_order.into()));
    full_trie.sort_by(|x, y| x.0.cmp(&y.0));
    expected.sort_by(|x, y| x.0.cmp(&y.0));
    assert_eq!(full_trie, expected);
}

fn check_if_orderbook_contains_only(orderbook: &Orderbook, pubkey: &str, orders: &[OrderbookItem]) {
    let pubkey_state = orderbook.pubkeys_state.get(pubkey).expect("!pubkeys_state");

    // order_set
    let expected_set: HashMap<_, _> = orders.iter().map(|order| (order.uuid, order.clone())).collect();
    assert_eq!(orderbook.order_set, expected_set);

    // ordered
    let mut expected_ordered = HashMap::new();
    for order in orders.iter() {
        let item = OrderedByPriceOrder {
            uuid: order.uuid,
            price: order.price.clone().into(),
        };
        let set = expected_ordered
            .entry((order.base.clone(), order.rel.clone()))
            .or_insert_with(BTreeSet::default);
        set.insert(item);
    }
    assert_eq!(orderbook.ordered, expected_ordered);

    // unordered
    let mut expected_unordered = HashMap::new();
    for order in orders.iter() {
        let set = expected_unordered
            .entry((order.base.clone(), order.rel.clone()))
            .or_insert_with(HashSet::default);
        set.insert(order.uuid);
    }
    assert_eq!(orderbook.unordered, expected_unordered);

    // history
    let actual_keys: HashSet<_> = pubkey_state
        .order_pairs_trie_state_history
        .keys()
        .iter()
        .cloned()
        .collect();
    let expected_keys: HashSet<_> = orders
        .iter()
        .map(|order| alb_ordered_pair(&order.base, &order.rel))
        .collect();
    assert_eq!(actual_keys, expected_keys);

    // orders_uuids
    let expected_uuids: HashSet<_> = orders
        .iter()
        .map(|order| (order.uuid, alb_ordered_pair(&order.base, &order.rel)))
        .collect();
    assert_eq!(pubkey_state.orders_uuids, expected_uuids);

    // trie_roots
    let actual_trie_orders: HashMap<_, _> = pubkey_state
        .trie_roots
        .iter()
        .map(|(alb_pair, trie_root)| {
            let trie = TrieDB::<Layout>::new(&orderbook.memory_db, trie_root).expect("!TrieDB::new");
            let mut trie: Vec<(Uuid, OrderbookItem)> = trie
                .iter()
                .expect("!TrieDB::iter")
                .map(|key_value| {
                    let (key, _) = key_value.expect("Iterator returned an error");
                    let key = TryFromBytes::try_from_bytes(key).expect("!try_from_bytes() key");
                    let value = orderbook.order_set.get(&key).cloned().unwrap();
                    (key, value)
                })
                .collect();
            trie.sort_by(|(uuid_x, _), (uuid_y, _)| uuid_x.cmp(uuid_y));
            (alb_pair.clone(), trie)
        })
        .collect();
    let mut expected_trie_orders = HashMap::new();
    for order in orders.iter() {
        let trie = expected_trie_orders
            .entry(alb_ordered_pair(&order.base, &order.rel))
            .or_insert_with(Vec::default);
        trie.push((order.uuid, order.clone()));
    }
    for (_alb_pair, trie) in expected_trie_orders.iter_mut() {
        trie.sort_by(|(uuid_x, _), (uuid_y, _)| uuid_x.cmp(uuid_y));
    }
    assert_eq!(actual_trie_orders, expected_trie_orders);
}

#[test]
fn test_remove_and_purge_pubkey_pair_orders() {
    let (ctx, pubkey, secret) = make_ctx_for_tests();
    let rick_morty_orders = make_random_orders(pubkey.clone(), &secret, "RICK".into(), "MORTY".into(), 10);
    let rick_kmd_orders = make_random_orders(pubkey.clone(), &secret, "RICK".into(), "KMD".into(), 10);

    for order in rick_morty_orders.iter().chain(rick_kmd_orders.iter()) {
        insert_or_update_order(&ctx, order.clone());
    }

    let rick_morty_pair = alb_ordered_pair("RICK", "MORTY");

    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let mut orderbook = ordermatch_ctx.orderbook.lock();

    remove_pubkey_pair_orders(&mut orderbook, &pubkey, &rick_morty_pair);
    check_if_orderbook_contains_only(&orderbook, &pubkey, &rick_kmd_orders);
}

#[test]
fn test_orderbook_sync_trie_diff_time_cache() {
    let (ctx_bob, pubkey_bob, secret_bob) = make_ctx_for_tests();
    let rick_morty_orders = make_random_orders(pubkey_bob.clone(), &secret_bob, "RICK".into(), "MORTY".into(), 15);

    let rick_morty_pair = alb_ordered_pair("RICK", "MORTY");

    for order in &rick_morty_orders[..5] {
        insert_or_update_order(&ctx_bob, order.clone());
    }

    std::thread::sleep(Duration::from_secs(3));

    for order in &rick_morty_orders[5..10] {
        insert_or_update_order(&ctx_bob, order.clone());
    }

    let ordermatch_ctx_bob = OrdermatchContext::from_ctx(&ctx_bob).unwrap();
    let orderbook_bob = ordermatch_ctx_bob.orderbook.lock();
    let bob_state = orderbook_bob.pubkeys_state.get(&pubkey_bob).unwrap();
    let rick_morty_history_bob = bob_state.order_pairs_trie_state_history.get(&rick_morty_pair).unwrap();
    assert_eq!(rick_morty_history_bob.len(), 5);

    // alice has an outdated state, for which bob doesn't have history anymore as it's expired
    let (ctx_alice, ..) = make_ctx_for_tests();

    for order in &rick_morty_orders[..3] {
        insert_or_update_order(&ctx_alice, order.clone());
    }

    let ordermatch_ctx_alice = OrdermatchContext::from_ctx(&ctx_alice).unwrap();
    let mut orderbook_alice = ordermatch_ctx_alice.orderbook.lock();
    let bob_state_on_alice_side = orderbook_alice.pubkeys_state.get(&pubkey_bob).unwrap();

    let alice_root = bob_state_on_alice_side.trie_roots.get(&rick_morty_pair).unwrap();
    let bob_root = bob_state.trie_roots.get(&rick_morty_pair).unwrap();

    let bob_history_on_sync = DeltaOrFullTrie::from_history(
        rick_morty_history_bob,
        *alice_root,
        *bob_root,
        &orderbook_bob.memory_db,
        |uuid: &Uuid| orderbook_bob.order_set.get(uuid).cloned(),
    )
    .unwrap();

    let full_trie = match bob_history_on_sync {
        DeltaOrFullTrie::FullTrie(trie) => trie,
        _ => panic!("Expected DeltaOrFullTrie::FullTrie"),
    };

    let params = ProcessTrieParams {
        pubkey: &pubkey_bob,
        alb_pair: &rick_morty_pair,
        protocol_infos: &HashMap::new(),
        conf_infos: &HashMap::new(),
    };
    let new_alice_root = process_pubkey_full_trie(
        &mut orderbook_alice,
        full_trie
            .into_iter()
            .map(|(uuid, order)| (uuid, order.into()))
            .collect(),
        params,
    );

    assert_eq!(new_alice_root, *bob_root);

    drop(orderbook_bob);
    drop(orderbook_alice);

    for order in &rick_morty_orders[10..] {
        insert_or_update_order(&ctx_bob, order.clone());
    }

    let mut orderbook_bob = ordermatch_ctx_bob.orderbook.lock();

    orderbook_bob.remove_order_trie_update(rick_morty_orders[12].uuid);

    let bob_state = orderbook_bob.pubkeys_state.get(&pubkey_bob).unwrap();
    let rick_morty_history_bob = bob_state.order_pairs_trie_state_history.get(&rick_morty_pair).unwrap();

    let mut orderbook_alice = ordermatch_ctx_alice.orderbook.lock();
    let bob_state_on_alice_side = orderbook_alice.pubkeys_state.get(&pubkey_bob).unwrap();

    let alice_root = bob_state_on_alice_side.trie_roots.get(&rick_morty_pair).unwrap();
    let bob_root = bob_state.trie_roots.get(&rick_morty_pair).unwrap();

    let bob_history_on_sync = DeltaOrFullTrie::from_history(
        rick_morty_history_bob,
        *alice_root,
        *bob_root,
        &orderbook_bob.memory_db,
        |uuid: &Uuid| orderbook_bob.order_set.get(uuid).cloned(),
    )
    .unwrap();

    // Check that alice gets orders from history this time
    let trie_delta = match bob_history_on_sync {
        DeltaOrFullTrie::Delta(delta) => delta,
        _ => panic!("Expected DeltaOrFullTrie::Delta"),
    };

    let params = ProcessTrieParams {
        pubkey: &pubkey_bob,
        alb_pair: &rick_morty_pair,
        protocol_infos: &HashMap::new(),
        conf_infos: &HashMap::new(),
    };
    let new_alice_root = process_trie_delta(
        &mut orderbook_alice,
        trie_delta
            .into_iter()
            .map(|(uuid, order)| (uuid, order.map(From::from)))
            .collect(),
        params,
    );
    assert_eq!(new_alice_root, *bob_root);
}

#[test]
fn test_orderbook_order_pairs_trie_state_history_updates_expiration_on_insert() {
    let (ctx_bob, pubkey_bob, secret_bob) = make_ctx_for_tests();
    let rick_morty_orders = make_random_orders(pubkey_bob.clone(), &secret_bob, "RICK".into(), "MORTY".into(), 15);

    let rick_morty_pair = alb_ordered_pair("RICK", "MORTY");

    for order in &rick_morty_orders[..5] {
        insert_or_update_order(&ctx_bob, order.clone());
    }

    // After 3 seconds RICK:MORTY pair trie state history will time out and will be empty
    std::thread::sleep(Duration::from_secs(3));

    // Insert some more orders to remove expired timecache RICK:MORTY key
    for order in &rick_morty_orders[5..10] {
        insert_or_update_order(&ctx_bob, order.clone());
    }

    let ordermatch_ctx_bob = OrdermatchContext::from_ctx(&ctx_bob).unwrap();
    let orderbook_bob = ordermatch_ctx_bob.orderbook.lock();
    let bob_state = orderbook_bob.pubkeys_state.get(&pubkey_bob).unwrap();

    // Only the last inserted 5 orders are found
    assert_eq!(
        bob_state
            .order_pairs_trie_state_history
            .get(&rick_morty_pair)
            .unwrap()
            .len(),
        5
    );

    drop(orderbook_bob);

    std::thread::sleep(Duration::from_secs(2));

    // On inserting 5 more orders expiration for RICK:MORTY pair trie state history will be reset
    for order in &rick_morty_orders[10..] {
        insert_or_update_order(&ctx_bob, order.clone());
    }

    let ordermatch_ctx_bob = OrdermatchContext::from_ctx(&ctx_bob).unwrap();
    let orderbook_bob = ordermatch_ctx_bob.orderbook.lock();
    let bob_state = orderbook_bob.pubkeys_state.get(&pubkey_bob).unwrap();

    assert_eq!(
        bob_state
            .order_pairs_trie_state_history
            .get(&rick_morty_pair)
            .unwrap()
            .len(),
        10
    );

    drop(orderbook_bob);

    std::thread::sleep(Duration::from_secs(1));

    let ordermatch_ctx_bob = OrdermatchContext::from_ctx(&ctx_bob).unwrap();
    let orderbook_bob = ordermatch_ctx_bob.orderbook.lock();
    let bob_state = orderbook_bob.pubkeys_state.get(&pubkey_bob).unwrap();

    // After 3 seconds from inserting orders number 6-10 these orders have not expired due to updated expiration on inserting orders 11-15
    assert_eq!(
        bob_state
            .order_pairs_trie_state_history
            .get(&rick_morty_pair)
            .unwrap()
            .len(),
        10
    );
}

#[test]
fn test_trie_state_bytes() {
    let pubkey = "037310a8fb9fd8f198a1a21db830252ad681fccda580ed4101f3f6bfb98b34fab5";
    let base = "RICK";
    let rel = "MORTY";
    let price = BigRational::from_integer(1.into());
    let max_volume = BigRational::from_integer(u64::MAX.into());
    let min_volume = BigRational::from_integer(1.into());
    let uuid = new_uuid();
    let created_at = now_sec();

    #[derive(Serialize)]
    struct OrderbookItemV1 {
        pubkey: String,
        base: String,
        rel: String,
        price: BigRational,
        max_volume: BigRational,
        min_volume: BigRational,
        uuid: Uuid,
        created_at: u64,
    }

    let old = OrderbookItemV1 {
        pubkey: pubkey.to_owned(),
        base: base.to_owned(),
        rel: rel.to_owned(),
        price: price.clone(),
        max_volume: max_volume.clone(),
        min_volume: min_volume.clone(),
        uuid,
        created_at,
    };

    let old_bytes = rmp_serde::to_vec(&old).unwrap();

    let new = OrderbookItem {
        pubkey: pubkey.to_owned(),
        base: base.to_owned(),
        rel: rel.to_owned(),
        price,
        max_volume,
        min_volume,
        uuid,
        created_at,
        base_protocol_info: vec![1, 2, 3],
        rel_protocol_info: vec![4, 5, 6],
        conf_settings: Some(OrderConfirmationsSettings {
            base_confs: 6,
            base_nota: false,
            rel_confs: 3,
            rel_nota: true,
        }),
    };

    let new_bytes = new.trie_state_bytes();

    assert_eq!(old_bytes, new_bytes);
}

#[test]
fn check_get_orderbook_p2p_res_serde() {
    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct OrderbookItemV1 {
        pubkey: String,
        base: String,
        rel: String,
        price: BigRational,
        max_volume: BigRational,
        min_volume: BigRational,
        uuid: Uuid,
        created_at: u64,
    }

    type PubkeyOrdersV1 = Vec<(Uuid, OrderbookItemV1)>;

    impl From<OrderbookItem> for OrderbookItemV1 {
        fn from(o: OrderbookItem) -> Self {
            OrderbookItemV1 {
                pubkey: o.pubkey,
                base: o.base,
                rel: o.rel,
                price: o.price,
                max_volume: o.max_volume,
                min_volume: o.min_volume,
                uuid: o.uuid,
                created_at: o.created_at,
            }
        }
    }

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct GetOrderbookPubkeyItemV1 {
        /// Timestamp of the latest keep alive message received.
        last_keep_alive: u64,
        /// last signed OrdermatchMessage payload
        last_signed_pubkey_payload: Vec<u8>,
        /// Requested orders.
        orders: PubkeyOrdersV1,
    }

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct GetOrderbookResV1 {
        /// Asks and bids grouped by pubkey.
        pubkey_orders: HashMap<String, GetOrderbookPubkeyItemV1>,
    }

    let orders = make_random_orders("".into(), &[1; 32], "RICK".into(), "MORTY".into(), 10);
    let item = GetOrderbookPubkeyItemV1 {
        last_keep_alive: 100,
        last_signed_pubkey_payload: vec![1, 2, 3],
        orders: orders
            .clone()
            .into_iter()
            .map(|order| (order.uuid, order.into()))
            .collect(),
    };

    let v1 = GetOrderbookResV1 {
        pubkey_orders: HashMap::from_iter(std::iter::once(("pubkey".into(), item))),
    };

    let v1_serialized = rmp_serde::to_vec_named(&v1).unwrap();

    let mut new: GetOrderbookRes = rmp_serde::from_slice(&v1_serialized).unwrap();
    new.protocol_infos.insert(
        new_uuid(),
        BaseRelProtocolInfo {
            base: vec![1],
            rel: vec![2],
        },
    );
    new.conf_infos.insert(
        new_uuid(),
        OrderConfirmationsSettings {
            base_confs: 6,
            base_nota: false,
            rel_confs: 3,
            rel_nota: true,
        },
    );

    let new_serialized = rmp_serde::to_vec_named(&new).unwrap();

    let v1_from_new: GetOrderbookResV1 = rmp_serde::from_slice(&new_serialized).unwrap();
    assert_eq!(v1, v1_from_new);

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct GetOrderbookResV2 {
        /// Asks and bids grouped by pubkey.
        pubkey_orders: HashMap<String, GetOrderbookPubkeyItem>,
        #[serde(default)]
        protocol_infos: HashMap<Uuid, BaseRelProtocolInfo>,
    }

    let item = GetOrderbookPubkeyItem {
        last_keep_alive: 100,
        last_signed_pubkey_payload: vec![1, 2, 3],
        orders: orders.into_iter().map(|order| (order.uuid, order.into())).collect(),
    };

    let v2 = GetOrderbookResV2 {
        pubkey_orders: HashMap::from_iter(std::iter::once(("pubkey".into(), item))),
        protocol_infos: HashMap::from_iter(std::iter::once((
            new_uuid(),
            BaseRelProtocolInfo {
                base: vec![1],
                rel: vec![2],
            },
        ))),
    };

    let v2_serialized = rmp_serde::to_vec_named(&v2).unwrap();

    let mut new: GetOrderbookRes = rmp_serde::from_slice(&v2_serialized).unwrap();
    new.conf_infos.insert(
        new_uuid(),
        OrderConfirmationsSettings {
            base_confs: 6,
            base_nota: false,
            rel_confs: 3,
            rel_nota: true,
        },
    );

    let new_serialized = rmp_serde::to_vec_named(&new).unwrap();

    let v2_from_new: GetOrderbookResV2 = rmp_serde::from_slice(&new_serialized).unwrap();
    assert_eq!(v2, v2_from_new);
}

#[test]
fn check_sync_pubkey_state_p2p_res_serde() {
    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    struct OrderbookItemV1 {
        pubkey: String,
        base: String,
        rel: String,
        price: BigRational,
        max_volume: BigRational,
        min_volume: BigRational,
        uuid: Uuid,
        created_at: u64,
    }

    impl From<OrderbookItem> for OrderbookItemV1 {
        fn from(o: OrderbookItem) -> Self {
            OrderbookItemV1 {
                pubkey: o.pubkey,
                base: o.base,
                rel: o.rel,
                price: o.price,
                max_volume: o.max_volume,
                min_volume: o.min_volume,
                uuid: o.uuid,
                created_at: o.created_at,
            }
        }
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct SyncPubkeyOrderbookStateResV1 {
        /// last signed OrdermatchMessage payload from pubkey
        last_signed_pubkey_payload: Vec<u8>,
        pair_orders_diff: HashMap<AlbOrderedOrderbookPair, DeltaOrFullTrie<Uuid, OrderbookItemV1>>,
    }

    let orders = make_random_orders("".into(), &[1; 32], "RICK".into(), "MORTY".into(), 10);

    let v1 = SyncPubkeyOrderbookStateResV1 {
        last_signed_pubkey_payload: vec![1, 2, 3, 4],
        pair_orders_diff: HashMap::from_iter(iter::once((
            alb_ordered_pair("RICK", "MORTY"),
            DeltaOrFullTrie::FullTrie(
                orders
                    .clone()
                    .into_iter()
                    .map(|order| (order.uuid, order.into()))
                    .collect(),
            ),
        ))),
    };

    let v1_serialized = rmp_serde::to_vec_named(&v1).unwrap();

    let mut new: SyncPubkeyOrderbookStateRes = rmp_serde::from_slice(&v1_serialized).unwrap();
    new.protocol_infos.insert(
        new_uuid(),
        BaseRelProtocolInfo {
            base: vec![1],
            rel: vec![2],
        },
    );
    new.conf_infos.insert(
        new_uuid(),
        OrderConfirmationsSettings {
            base_confs: 6,
            base_nota: false,
            rel_confs: 3,
            rel_nota: true,
        },
    );

    let new_serialized = rmp_serde::to_vec_named(&new).unwrap();

    let _v1_from_new: SyncPubkeyOrderbookStateResV1 = rmp_serde::from_slice(&new_serialized).unwrap();

    #[derive(Debug, Deserialize, Serialize)]
    struct SyncPubkeyOrderbookStateResV2 {
        /// last signed OrdermatchMessage payload from pubkey
        last_signed_pubkey_payload: Vec<u8>,
        pair_orders_diff: HashMap<AlbOrderedOrderbookPair, DeltaOrFullTrie<Uuid, OrderbookP2PItem>>,
        #[serde(default)]
        protocol_infos: HashMap<Uuid, BaseRelProtocolInfo>,
    }

    let v2 = SyncPubkeyOrderbookStateResV2 {
        last_signed_pubkey_payload: vec![1, 2, 3, 4],
        pair_orders_diff: HashMap::from_iter(iter::once((
            alb_ordered_pair("RICK", "MORTY"),
            DeltaOrFullTrie::FullTrie(orders.into_iter().map(|order| (order.uuid, order.into())).collect()),
        ))),
        protocol_infos: HashMap::from_iter(std::iter::once((
            new_uuid(),
            BaseRelProtocolInfo {
                base: vec![1],
                rel: vec![2],
            },
        ))),
    };

    let v2_serialized = rmp_serde::to_vec_named(&v2).unwrap();

    let mut new: SyncPubkeyOrderbookStateRes = rmp_serde::from_slice(&v2_serialized).unwrap();
    new.conf_infos.insert(
        new_uuid(),
        OrderConfirmationsSettings {
            base_confs: 6,
            base_nota: false,
            rel_confs: 3,
            rel_nota: true,
        },
    );

    let new_serialized = rmp_serde::to_vec_named(&new).unwrap();

    let _v2_from_new: SyncPubkeyOrderbookStateResV2 = rmp_serde::from_slice(&new_serialized).unwrap();
}

#[test]
fn check_order_serde() {
    let order_json = include_str!("for_tests/check_order_serde_payload.json");
    let _order: Order = json::from_str(order_json).unwrap();
}

#[test]
fn test_maker_order_balance_loops() {
    let ctx = mm_ctx_with_iguana(None);

    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let mut maker_orders_ctx = ordermatch_ctx.maker_orders_ctx.lock();

    let rick_ticker = "RICK";
    let morty_ticker = "MORTY";

    let rick_order = MakerOrder {
        uuid: Uuid::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        base: rick_ticker.into(),
        rel: morty_ticker.into(),
        created_at: now_ms(),
        updated_at: Some(now_ms()),
        matches: HashMap::new(),
        max_base_vol: 0.into(),
        min_base_vol: 0.into(),
        price: 0.into(),
        started_swaps: vec![],
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };

    let morty_order = MakerOrder {
        uuid: Uuid::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
        base: morty_ticker.into(),
        rel: rick_ticker.into(),
        created_at: now_ms(),
        updated_at: Some(now_ms()),
        matches: HashMap::new(),
        max_base_vol: 0.into(),
        min_base_vol: 0.into(),
        price: 0.into(),
        started_swaps: vec![],
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };

    assert!(!maker_orders_ctx.balance_loop_exists(rick_ticker));
    assert!(!maker_orders_ctx.balance_loop_exists(morty_ticker));
    assert_eq!(maker_orders_ctx.count_by_tickers.get(rick_ticker), None);
    assert_eq!(maker_orders_ctx.count_by_tickers.get(morty_ticker), None);

    maker_orders_ctx.add_order(ctx.weak(), rick_order.clone(), None);
    assert!(maker_orders_ctx.balance_loop_exists(rick_ticker));
    assert_eq!(*maker_orders_ctx.count_by_tickers.get(rick_ticker).unwrap(), 1);

    maker_orders_ctx.add_order(ctx.weak(), morty_order.clone(), None);
    assert!(maker_orders_ctx.balance_loop_exists(morty_ticker));
    assert_eq!(*maker_orders_ctx.count_by_tickers.get(morty_ticker).unwrap(), 1);

    let rick_order_2 = MakerOrder {
        uuid: Uuid::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
        base: rick_ticker.into(),
        rel: "MORTY2".into(),
        created_at: now_ms(),
        updated_at: Some(now_ms()),
        matches: HashMap::new(),
        max_base_vol: 0.into(),
        min_base_vol: 0.into(),
        price: 0.into(),
        started_swaps: vec![],
        conf_settings: None,
        changes_history: None,
        save_in_history: false,
        base_orderbook_ticker: None,
        rel_orderbook_ticker: None,
        p2p_privkey: None,
        swap_version: SwapVersion::default(),
        #[cfg(feature = "ibc-routing-for-swaps")]
        order_metadata: OrderMetadata::default(),
        timeout_in_minutes: None,
    };

    maker_orders_ctx.add_order(ctx.weak(), rick_order_2.clone(), None);
    assert_eq!(*maker_orders_ctx.count_by_tickers.get(rick_ticker).unwrap(), 2);

    maker_orders_ctx.remove_order(&rick_order_2.uuid);
    assert!(maker_orders_ctx.balance_loop_exists(rick_ticker));
    assert_eq!(*maker_orders_ctx.count_by_tickers.get(rick_ticker).unwrap(), 1);

    maker_orders_ctx.remove_order(&rick_order.uuid);
    assert!(!maker_orders_ctx.balance_loop_exists(rick_ticker));
    assert_eq!(*maker_orders_ctx.count_by_tickers.get(rick_ticker).unwrap(), 0);

    maker_orders_ctx.remove_order(&morty_order.uuid);
    assert!(!maker_orders_ctx.balance_loop_exists(morty_ticker));
    assert_eq!(*maker_orders_ctx.count_by_tickers.get(morty_ticker).unwrap(), 0);
}

#[test]
fn process_orders_keep_alive_with_null_root_clears_pair_and_does_not_request() {
    let (ctx, _our_pubkey, _our_secret) = make_ctx_for_tests();
    let (maker_pubkey, maker_secret) = pubkey_and_secret_for_test("maker-passphrase");

    // Build one remote order from maker_pubkey for RICK/MORTY and insert it locally.
    let mut orders = make_random_orders(maker_pubkey.clone(), &maker_secret, "RICK".into(), "MORTY".into(), 1);
    let order = orders.pop().expect("one order generated");
    let inserted_uuid = order.uuid;

    {
        let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
        let mut orderbook = ordermatch_ctx.orderbook.lock();

        // Mark the pair as subscribed so keep-alive handling considers it.
        orderbook.topics_subscribed_to.insert(
            orderbook_topic_from_base_rel("RICK", "MORTY"),
            OrderbookRequestingState::Requested,
        );

        // Insert as if it came from the network; this updates trie state as well.
        orderbook.insert_or_update_order_update_trie(order);

        // Sanity: the order is present.
        assert!(
            orderbook.order_set.contains_key(&inserted_uuid),
            "precondition: order must be present"
        );
    }

    // Craft a keep-alive that advertises a null/empty root for (maker_pubkey, alb_pair).
    let alb_pair = alb_ordered_pair("RICK", "MORTY");
    let keep_alive = PubkeyKeepAlive {
        trie_roots: HashMap::from_iter(std::iter::once((alb_pair.clone(), [0u8; 8]))),
        timestamp: now_sec(),
    };

    // For a null root, we must clear the pair locally and NOT request a sync.
    let res = block_on(process_orders_keep_alive(
        ctx.clone(),
        "dummy_peer".to_string(),
        maker_pubkey.clone(),
        keep_alive,
        false,
    ));
    assert!(res.is_ok(), "process_orders_keep_alive returned error");

    // The order must be gone now.
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let orderbook = ordermatch_ctx.orderbook.lock();
    assert!(
        !orderbook.order_set.contains_key(&inserted_uuid),
        "orders for (pubkey, pair) must be cleared by null-root keep-alive"
    );

    // And the remembered trie root for that pair should be the null root.
    let state = orderbook
        .pubkeys_state
        .get(&maker_pubkey)
        .expect("pubkey state missing");
    assert_eq!(
        state.trie_roots.get(&alb_pair).copied(),
        Some([0u8; 8]),
        "null root should be remembered in pubkey state"
    );
}
