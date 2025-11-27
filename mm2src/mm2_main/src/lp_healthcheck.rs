use async_std::prelude::FutureExt;
use chrono::Utc;
use common::executor::SpawnFuture;
use common::{log, HttpStatusCode, StatusCode};
use compatible_time::{Duration, Instant};
use derive_more::Display;
use futures::channel::oneshot::{self, Receiver, Sender};
use lazy_static::lazy_static;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::MmError;
use mm2_err_handle::prelude::*;
use mm2_libp2p::p2p_ctx::P2PContext;
use mm2_libp2p::{decode_message, encode_message, pub_sub_topic, Libp2pPublic, PeerAddress, TopicPrefix};
use ser_error_derive::SerializeErrorType;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::sync::Mutex;

use crate::lp_network::{broadcast_p2p_msg, P2PRequestError, P2PRequestResult};

pub(crate) const PEER_HEALTHCHECK_PREFIX: TopicPrefix = "hcheck";

const fn healthcheck_message_exp_secs() -> u64 {
    #[cfg(test)]
    return 3;

    #[cfg(not(test))]
    10
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(any(test, target_arch = "wasm32"), derive(PartialEq))]
pub(crate) struct HealthcheckMessage {
    #[serde(deserialize_with = "deserialize_bytes")]
    signature: Vec<u8>,
    data: HealthcheckData,
}

#[derive(Debug, Display)]
enum SignValidationError {
    #[display(
        fmt = "Healthcheck message is expired. Current time in UTC: {now_secs}, healthcheck `expires_at` in UTC: {expires_at_secs}"
    )]
    Expired { now_secs: u64, expires_at_secs: u64 },
    #[display(
        fmt = "Healthcheck message have too high expiration time. Max allowed expiration seconds: {max_allowed_expiration_secs}, received message expiration seconds: {remaining_expiration_secs}"
    )]
    LifetimeOverflow {
        max_allowed_expiration_secs: u64,
        remaining_expiration_secs: u64,
    },
    #[display(fmt = "Public key is not valid.")]
    InvalidPublicKey,
    #[display(fmt = "Signature integrity doesn't match with the public key.")]
    FakeSignature,
    #[display(fmt = "Process failed unexpectedly due to this reason: {reason}")]
    Internal { reason: String },
}

impl HealthcheckMessage {
    pub(crate) fn generate_message(ctx: &MmArc, is_a_reply: bool) -> Result<Self, String> {
        let p2p_ctx = P2PContext::fetch_from_mm_arc(ctx);
        let keypair = p2p_ctx.keypair();
        let sender_public_key = keypair.public().encode_protobuf();

        let data = HealthcheckData {
            sender_public_key,
            expires_at_secs: u64::try_from(Utc::now().timestamp()).map_err(|e| e.to_string())?
                + healthcheck_message_exp_secs(),
            is_a_reply,
        };

        let signature = try_s!(keypair.sign(&try_s!(data.encode())));

        Ok(Self { signature, data })
    }

    fn generate_or_use_cached_message(ctx: &MmArc) -> Result<Self, String> {
        const MIN_DURATION_FOR_REUSABLE_MSG: Duration = Duration::from_secs(5);

        lazy_static! {
            static ref RECENTLY_GENERATED_MESSAGE: Mutex<Option<(HealthcheckMessage, Instant)>> = Mutex::new(None);
        }

        // If recently generated message has longer life than `MIN_DURATION_FOR_REUSABLE_MSG`, we can reuse it to
        // reduce the message generation overhead under high pressure.
        let mut mutexed_msg = RECENTLY_GENERATED_MESSAGE.lock().unwrap();

        if let Some((ref msg, expiration)) = *mutexed_msg {
            if expiration > Instant::now() + MIN_DURATION_FOR_REUSABLE_MSG {
                return Ok(msg.clone());
            }
        }

        let new_msg = HealthcheckMessage::generate_message(ctx, true)?;

        *mutexed_msg = Some((
            new_msg.clone(),
            Instant::now() + Duration::from_secs(healthcheck_message_exp_secs()),
        ));

        Ok(new_msg)
    }

    fn is_received_message_valid(&self) -> Result<PeerAddress, SignValidationError> {
        let now_secs = u64::try_from(Utc::now().timestamp())
            .map_err(|e| SignValidationError::Internal { reason: e.to_string() })?;

        let remaining_expiration_secs = self.data.expires_at_secs.saturating_sub(now_secs);

        if remaining_expiration_secs == 0 {
            return Err(SignValidationError::Expired {
                now_secs,
                expires_at_secs: self.data.expires_at_secs,
            });
        } else if remaining_expiration_secs > healthcheck_message_exp_secs() {
            return Err(SignValidationError::LifetimeOverflow {
                max_allowed_expiration_secs: healthcheck_message_exp_secs(),
                remaining_expiration_secs,
            });
        }

        let Ok(public_key) = Libp2pPublic::try_decode_protobuf(&self.data.sender_public_key) else {
            log::debug!("Couldn't decode public key from the healthcheck message.");

            return Err(SignValidationError::InvalidPublicKey);
        };

        let encoded_message = self
            .data
            .encode()
            .map_err(|e| SignValidationError::Internal { reason: e.to_string() })?;

        if public_key.verify(&encoded_message, &self.signature) {
            Ok(public_key.to_peer_id().into())
        } else {
            Err(SignValidationError::FakeSignature)
        }
    }

    #[inline]
    pub(crate) fn encode(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        encode_message(self)
    }

    #[inline]
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, rmp_serde::decode::Error> {
        decode_message(bytes)
    }

    #[inline]
    pub(crate) fn should_reply(&self) -> bool {
        !self.data.is_a_reply
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(any(test, target_arch = "wasm32"), derive(PartialEq))]
struct HealthcheckData {
    #[serde(deserialize_with = "deserialize_bytes")]
    sender_public_key: Vec<u8>,
    expires_at_secs: u64,
    is_a_reply: bool,
}

impl HealthcheckData {
    #[inline]
    fn encode(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        encode_message(self)
    }
}

#[inline]
pub fn peer_healthcheck_topic(peer_address: &PeerAddress) -> String {
    pub_sub_topic(PEER_HEALTHCHECK_PREFIX, &peer_address.to_string())
}

#[derive(Deserialize)]
pub struct RequestPayload {
    peer_address: PeerAddress,
}

fn deserialize_bytes<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct ByteVisitor;

    impl<'de> serde::de::Visitor<'de> for ByteVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a non-empty byte array up to 512 bytes")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Vec<u8>, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut buffer = vec![];
            while let Some(byte) = seq.next_element()? {
                if buffer.len() >= 512 {
                    return Err(serde::de::Error::invalid_length(
                        buffer.len(),
                        &"longest possible length allowed for this field is 512 bytes (with RSA algorithm).",
                    ));
                }

                buffer.push(byte);
            }

            if buffer.is_empty() {
                return Err(serde::de::Error::custom("Can't be empty."));
            }

            Ok(buffer)
        }
    }

    deserializer.deserialize_seq(ByteVisitor)
}

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum HealthcheckRpcError {
    MessageGenerationFailed { reason: String },
    MessageEncodingFailed { reason: String },
    Internal { reason: String },
}

impl HttpStatusCode for HealthcheckRpcError {
    fn status_code(&self) -> common::StatusCode {
        match self {
            HealthcheckRpcError::MessageGenerationFailed { .. }
            | HealthcheckRpcError::Internal { .. }
            | HealthcheckRpcError::MessageEncodingFailed { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

pub async fn peer_connection_healthcheck_rpc(
    ctx: MmArc,
    req: RequestPayload,
) -> Result<bool, MmError<HealthcheckRpcError>> {
    // When things go awry, we want records to clear themselves to keep the memory clean of unused data.
    // This is unrelated to the timeout logic.
    let address_record_exp = Duration::from_secs(healthcheck_message_exp_secs());

    let target_peer_address = req.peer_address;

    let p2p_ctx = P2PContext::fetch_from_mm_arc(&ctx);
    if target_peer_address == p2p_ctx.peer_id().into() {
        // That's us, so return true.
        return Ok(true);
    }

    let message = HealthcheckMessage::generate_message(&ctx, false)
        .map_err(|reason| HealthcheckRpcError::MessageGenerationFailed { reason })?;

    let encoded_message = message
        .encode()
        .map_err(|e| HealthcheckRpcError::MessageEncodingFailed { reason: e.to_string() })?;

    let (tx, rx): (Sender<()>, Receiver<()>) = oneshot::channel();

    {
        let mut book = ctx.healthcheck_response_handler.lock().await;
        book.insert_expirable(target_peer_address.into(), tx, address_record_exp);
    }

    broadcast_p2p_msg(
        &ctx,
        peer_healthcheck_topic(&target_peer_address),
        encoded_message,
        None,
    );

    let timeout_duration = Duration::from_secs(healthcheck_message_exp_secs());
    Ok(rx.timeout(timeout_duration).await == Ok(Ok(())))
}

pub(crate) async fn process_p2p_healthcheck_message(
    ctx: &MmArc,
    message: mm2_libp2p::GossipsubMessage,
) -> P2PRequestResult<()> {
    macro_rules! try_or_return {
        ($exp:expr, $msg: expr) => {
            match $exp {
                Ok(t) => t,
                Err(e) => {
                    log::error!("{}, error: {e:?}", $msg);
                    return;
                },
            }
        };
    }

    let data = HealthcheckMessage::decode(&message.data)
        .map_to_mm(|e| P2PRequestError::DecodeError(format!("Couldn't decode healthcheck message: {e}")))?;
    let sender_peer = data.is_received_message_valid().map_to_mm(|e| {
        P2PRequestError::ValidationFailed(format!("Received an invalid healthcheck message. Error: {e}"))
    })?;

    let ctx = ctx.clone();

    // Pass the remaining work to another thread to free up this one as soon as possible,
    // so KDF can handle a high amount of healthcheck messages more efficiently.
    ctx.spawner().spawn(async move {
        if data.should_reply() {
            // Reply the message so they know we are healthy.

            let msg = try_or_return!(
                HealthcheckMessage::generate_or_use_cached_message(&ctx),
                "Couldn't generate the healthcheck message, this is very unusual!"
            );

            let encoded_msg = try_or_return!(
                msg.encode(),
                "Couldn't encode healthcheck message, this is very unusual!"
            );

            let topic = peer_healthcheck_topic(&sender_peer);
            broadcast_p2p_msg(&ctx, topic, encoded_msg, None);
        } else {
            // The requested peer is healthy; signal the response channel.
            let mut response_handler = ctx.healthcheck_response_handler.lock().await;
            if let Some(tx) = response_handler.remove(&sender_peer.into()) {
                if tx.send(()).is_err() {
                    log::error!("Result channel isn't present for peer '{sender_peer}'.");
                };
            } else {
                log::info!("Peer '{sender_peer}' isn't recorded in the healthcheck response handler.");
            };
        }
    });

    Ok(())
}

#[cfg(any(test, target_arch = "wasm32"))]
mod tests {
    use std::mem::discriminant;
    use std::str::FromStr;

    use super::*;
    use common::cross_test;
    use crypto::CryptoCtx;
    use mm2_libp2p::behaviours::atomicdex::generate_ed25519_keypair;
    use mm2_test_helpers::for_tests::mm_ctx_with_iguana;

    common::cfg_wasm32! {
        use wasm_bindgen_test::*;
        wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
    }

    fn ctx() -> MmArc {
        let ctx = mm_ctx_with_iguana(Some("dummy-value"));
        let p2p_key = {
            let crypto_ctx = CryptoCtx::from_ctx(&ctx).unwrap();
            let key = bitcrypto::sha256(crypto_ctx.mm2_internal_privkey_slice());
            key.take()
        };

        let (cmd_tx, _) = futures::channel::mpsc::channel(0);

        let p2p_context = P2PContext::new(cmd_tx, generate_ed25519_keypair(p2p_key));
        p2p_context.store_to_mm_arc(&ctx);

        ctx
    }

    cross_test!(test_peer_address, {
        #[derive(Deserialize, Serialize)]
        struct PeerAddressTest {
            peer_address: PeerAddress,
        }

        let address_str = "12D3KooWEtuv7kmgGCC7oAQ31hB7AR5KkhT3eEWB2bP2roo3M7rY";
        let json_content = format!("{{\"peer_address\": \"{address_str}\"}}");
        let address_struct: PeerAddressTest = serde_json::from_str(&json_content).unwrap();

        let actual_peer_id = mm2_libp2p::PeerId::from_str(address_str).unwrap();
        let deserialized_peer_id: mm2_libp2p::PeerId = address_struct.peer_address.into();

        assert_eq!(deserialized_peer_id, actual_peer_id);
    });

    cross_test!(test_valid_message, {
        let ctx = ctx();
        let message = HealthcheckMessage::generate_message(&ctx, false).unwrap();
        message.is_received_message_valid().unwrap();
    });

    cross_test!(test_corrupted_messages, {
        let ctx = ctx();

        let mut message = HealthcheckMessage::generate_message(&ctx, false).unwrap();
        message.data.expires_at_secs += healthcheck_message_exp_secs() * 3;
        assert_eq!(
            discriminant(&message.is_received_message_valid().err().unwrap()),
            discriminant(&SignValidationError::LifetimeOverflow {
                max_allowed_expiration_secs: 0,
                remaining_expiration_secs: 0
            })
        );

        let mut message = HealthcheckMessage::generate_message(&ctx, false).unwrap();
        message.data.is_a_reply = !message.data.is_a_reply;
        assert_eq!(
            discriminant(&message.is_received_message_valid().err().unwrap()),
            discriminant(&SignValidationError::FakeSignature)
        );

        let mut message = HealthcheckMessage::generate_message(&ctx, false).unwrap();
        message.data.sender_public_key.push(0);
        assert_eq!(
            discriminant(&message.is_received_message_valid().err().unwrap()),
            discriminant(&SignValidationError::InvalidPublicKey)
        );
    });

    cross_test!(test_expired_message, {
        let ctx = ctx();
        let message = HealthcheckMessage::generate_message(&ctx, false).unwrap();
        common::executor::Timer::sleep(3.).await;
        assert_eq!(
            discriminant(&message.is_received_message_valid().err().unwrap()),
            discriminant(&SignValidationError::Expired {
                now_secs: 0,
                expires_at_secs: 0
            })
        );
    });

    cross_test!(test_encode_decode, {
        let ctx = ctx();
        let original = HealthcheckMessage::generate_message(&ctx, false).unwrap();

        let encoded = original.encode().unwrap();
        assert!(!encoded.is_empty());

        let decoded = HealthcheckMessage::decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    });
}
