// TODO: a lof of these implementations should be handled in `mm2_net`

/******************************************************************************
 * Copyright © 2023 Pampex LTD and TillyHK LTD                                *
 *                                                                            *
 * See the CONTRIBUTOR-LICENSE-AGREEMENT, COPYING, LICENSE-COPYRIGHT-NOTICE   *
 * and DEVELOPER-CERTIFICATE-OF-ORIGIN files in the LEGAL directory in        *
 * the top-level directory of this distribution for the individual copyright  *
 * holder information and the developer policies on copyright and licensing.  *
 *                                                                            *
 * Unless otherwise agreed in a custom licensing agreement, no part of the    *
 * Komodo DeFi Framework software, including this file may be copied, modified, propagated*
 * or distributed except according to the terms contained in the              *
 * LICENSE-COPYRIGHT-NOTICE file.                                             *
 *                                                                            *
 * Removal or modification of this copyright notice is prohibited.            *
 *                                                                            *
 ******************************************************************************/
//
//  lp_network.rs
//  marketmaker
//
use coins::lp_coinfind;
use common::executor::SpawnFuture;
use common::{log, Future01CompatExt};
use compatible_time::Instant;
use derive_more::Display;
use futures::{channel::oneshot, StreamExt};
use keys::KeyPair;
use mm2_core::mm_ctx::{MmArc, MmWeak};
use mm2_err_handle::prelude::*;
use mm2_libp2p::application::request_response::P2PRequest;
use mm2_libp2p::p2p_ctx::P2PContext;
use mm2_libp2p::{
    decode_message, encode_message, DecodingError, GossipsubEvent, GossipsubMessage, Libp2pPublic, Libp2pSecpPublic,
    MessageId, NetworkPorts, PeerId, TOPIC_SEPARATOR,
};
use mm2_libp2p::{AdexBehaviourCmd, AdexBehaviourEvent, AdexEventRx, AdexResponse};
use mm2_libp2p::{PeerAddresses, RequestResponseBehaviourEvent};
use mm2_metrics::{mm_label, mm_timing};
use serde::de;

use crate::{lp_healthcheck, lp_ordermatch, lp_stats, lp_swap};

pub type P2PRequestResult<T> = Result<T, MmError<P2PRequestError>>;
pub type P2PProcessResult<T> = Result<T, MmError<P2PProcessError>>;

pub trait Libp2pPeerId {
    fn libp2p_peer_id(&self) -> PeerId;
}

impl Libp2pPeerId for KeyPair {
    #[inline(always)]
    fn libp2p_peer_id(&self) -> PeerId {
        peer_id_from_secp_public(self.public_slice()).expect("valid public")
    }
}

#[derive(Debug, Display)]
#[allow(clippy::enum_variant_names)]
pub enum P2PRequestError {
    EncodeError(String),
    DecodeError(String),
    SendError(String),
    ResponseError(String),
    #[display(fmt = "Expected 1 response, found {_0}")]
    ExpectedSingleResponseError(usize),
    ValidationFailed(String),
}

/// Enum covering error cases that can happen during P2P message processing.
#[derive(Debug, Display)]
#[allow(clippy::enum_variant_names)]
pub enum P2PProcessError {
    /// The message could not be decoded.
    DecodeError(String),
    /// Message signature is invalid.
    InvalidSignature(String),
    /// Unexpected message sender.
    #[display(fmt = "Unexpected message sender {_0}")]
    UnexpectedSender(String),
    /// Message did not pass additional validation
    #[display(fmt = "Message validation failed: {_0}")]
    ValidationFailed(String),
}

impl From<rmp_serde::encode::Error> for P2PRequestError {
    fn from(e: rmp_serde::encode::Error) -> Self {
        P2PRequestError::EncodeError(e.to_string())
    }
}

impl From<rmp_serde::decode::Error> for P2PRequestError {
    fn from(e: rmp_serde::decode::Error) -> Self {
        P2PRequestError::DecodeError(e.to_string())
    }
}

pub async fn p2p_event_process_loop(ctx: MmWeak, mut rx: AdexEventRx, i_am_relay: bool) {
    loop {
        let adex_event = rx.next().await;
        let ctx = match MmArc::from_weak(&ctx) {
            Some(ctx) => ctx,
            None => return,
        };
        match adex_event {
            Some(AdexBehaviourEvent::Gossipsub(event)) => match event {
                GossipsubEvent::Message {
                    propagation_source,
                    message_id,
                    message,
                } => {
                    let spawner = ctx.spawner();
                    spawner.spawn(process_p2p_message(
                        ctx,
                        propagation_source,
                        message_id,
                        message,
                        i_am_relay,
                    ));
                },
                GossipsubEvent::GossipsubNotSupported { peer_id } => {
                    log::error!("Received unsupported event from Peer: {peer_id}");
                },
                _ => {},
            },
            Some(AdexBehaviourEvent::RequestResponse(RequestResponseBehaviourEvent::InboundRequest {
                peer_id,
                request,
                response_channel,
            })) => {
                if let Err(e) = process_p2p_request(ctx, peer_id, request.req, response_channel.into()) {
                    log::error!("Error on process P2P request: {:?}", e);
                }
            },
            _ => {},
        }
    }
}

async fn process_p2p_message(
    ctx: MmArc,
    peer_id: PeerId,
    message_id: MessageId,
    message: GossipsubMessage,
    i_am_relay: bool,
) {
    let mut to_propagate = false;

    let mut split = message.topic.as_str().split(TOPIC_SEPARATOR);
    match split.next() {
        Some(lp_ordermatch::ORDERBOOK_PREFIX) => {
            if let Err(e) = lp_ordermatch::handle_orderbook_msg(
                ctx.clone(),
                &message.topic,
                peer_id.to_string(),
                &message.data,
                i_am_relay,
            )
            .await
            {
                if e.get_inner().is_warning() {
                    log::warn!("{}", e);
                } else {
                    log::error!("{}", e);
                }
                return;
            }

            to_propagate = true;
        },
        Some(lp_swap::SWAP_PREFIX) => {
            if let Err(e) =
                lp_swap::process_swap_msg(ctx.clone(), split.next().unwrap_or_default(), &message.data).await
            {
                log::error!("{}", e);
                return;
            }

            to_propagate = true;
        },
        Some(lp_swap::SWAP_V2_PREFIX) => {
            if let Err(e) = lp_swap::process_swap_v2_msg(ctx.clone(), split.next().unwrap_or_default(), &message.data) {
                log::error!("{}", e);
                return;
            }

            to_propagate = true;
        },
        Some(lp_swap::WATCHER_PREFIX) => {
            if ctx.is_watcher() {
                if let Err(e) = lp_swap::process_watcher_msg(ctx.clone(), &message.data) {
                    log::error!("{}", e);
                    return;
                }
            }

            to_propagate = true;
        },
        Some(lp_swap::TX_HELPER_PREFIX) => {
            if let Some(ticker) = split.next() {
                if let Ok(Some(coin)) = lp_coinfind(&ctx, ticker).await {
                    if let Err(e) = coin.tx_enum_from_bytes(&message.data) {
                        log::error!("Message cannot continue the process due to: {:?}", e);
                        return;
                    };

                    if coin.is_utxo_in_native_mode() {
                        let fut = coin.send_raw_tx_bytes(&message.data);
                        ctx.spawner().spawn(async {
                            match fut.compat().await {
                                Ok(id) => log::debug!("Transaction broadcasted successfully: {:?} ", id),
                                // TODO (After https://github.com/KomodoPlatform/atomicDEX-API/pull/1433)
                                // Maybe do not log an error if the transaction is already sent to
                                // the blockchain
                                Err(e) => log::error!("Broadcast transaction failed (ignore this error if the transaction already sent by another seednode). {}", e),
                            };
                        })
                    }
                }

                to_propagate = true;
            }
        },
        Some(lp_healthcheck::PEER_HEALTHCHECK_PREFIX) => {
            if let Err(e) = lp_healthcheck::process_p2p_healthcheck_message(&ctx, message).await {
                log::error!("{}", e);
                return;
            }

            to_propagate = true;
        },
        None | Some(_) => (),
    }

    if to_propagate && i_am_relay {
        propagate_message(&ctx, message_id, peer_id);
    }
}

fn process_p2p_request(
    ctx: MmArc,
    _peer_id: PeerId,
    request: Vec<u8>,
    response_channel: mm2_libp2p::AdexResponseChannel,
) -> P2PRequestResult<()> {
    let request = decode_message::<P2PRequest>(&request)?;
    log::debug!("Got P2PRequest {:?}", request);

    let result = match request {
        P2PRequest::Ordermatch(req) => lp_ordermatch::process_peer_request(ctx.clone(), req),
        P2PRequest::NetworkInfo(req) => lp_stats::process_info_request(ctx.clone(), req).map(Some),
    };

    let res = match result {
        Ok(Some(response)) => AdexResponse::Ok { response },
        Ok(None) => AdexResponse::None,
        Err(e) => AdexResponse::Err { error: e },
    };

    let p2p_ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let cmd = AdexBehaviourCmd::SendResponse { res, response_channel };
    p2p_ctx
        .cmd_tx
        .lock()
        .try_send(cmd)
        .map_to_mm(|e| P2PRequestError::SendError(e.to_string()))?;
    Ok(())
}

pub fn broadcast_p2p_msg(ctx: &MmArc, topic: String, msg: Vec<u8>, from: Option<PeerId>) {
    let ctx = ctx.clone();
    let cmd = match from {
        Some(from) => AdexBehaviourCmd::PublishMsgFrom { topic, msg, from },
        None => AdexBehaviourCmd::PublishMsg { topic, msg },
    };
    let p2p_ctx = P2PContext::fetch_from_mm_arc(&ctx);
    if let Err(e) = p2p_ctx.cmd_tx.lock().try_send(cmd) {
        log::error!("broadcast_p2p_msg cmd_tx.send error {:?}", e);
    };
}

/// Subscribe to the given `topic`.
///
/// # Safety
///
/// The function locks the [`MmCtx::p2p_ctx`] mutex.
pub fn subscribe_to_topic(ctx: &MmArc, topic: String) {
    let p2p_ctx = P2PContext::fetch_from_mm_arc(ctx);
    let cmd = AdexBehaviourCmd::Subscribe { topic };
    if let Err(e) = p2p_ctx.cmd_tx.lock().try_send(cmd) {
        log::error!("subscribe_to_topic cmd_tx.send error {:?}", e);
    };
}

/// Unsubscribe from the given `topic`.
pub fn unsubscribe_from_topic(ctx: &MmArc, topic: String) {
    let p2p_ctx = P2PContext::fetch_from_mm_arc(ctx);
    let cmd = AdexBehaviourCmd::Unsubscribe { topic };
    if let Err(e) = p2p_ctx.cmd_tx.lock().try_send(cmd) {
        log::error!("unsubscribe_from_topic cmd_tx.send error {:?}", e);
    };
}

pub async fn request_any_relay<T: de::DeserializeOwned>(
    ctx: MmArc,
    req: P2PRequest,
) -> P2PRequestResult<Option<(T, PeerId)>> {
    let encoded = encode_message(&req)?;

    let (response_tx, response_rx) = oneshot::channel();
    let p2p_ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let cmd = AdexBehaviourCmd::RequestAnyRelay {
        req: encoded,
        response_tx,
    };
    p2p_ctx
        .cmd_tx
        .lock()
        .try_send(cmd)
        .map_to_mm(|e| P2PRequestError::SendError(e.to_string()))?;
    match response_rx
        .await
        .map_to_mm(|e| P2PRequestError::ResponseError(e.to_string()))?
    {
        Some((from_peer, response)) => {
            let response = decode_message::<T>(&response)?;
            Ok(Some((response, from_peer)))
        },
        None => Ok(None),
    }
}

pub enum PeerDecodedResponse<T> {
    Ok(T),
    None,
    Err(String),
}

#[allow(dead_code)]
pub async fn request_relays<T: de::DeserializeOwned>(
    ctx: MmArc,
    req: P2PRequest,
) -> P2PRequestResult<Vec<(PeerId, PeerDecodedResponse<T>)>> {
    let encoded = encode_message(&req)?;

    let (response_tx, response_rx) = oneshot::channel();
    let p2p_ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let cmd = AdexBehaviourCmd::RequestRelays {
        req: encoded,
        response_tx,
    };
    p2p_ctx
        .cmd_tx
        .lock()
        .try_send(cmd)
        .map_to_mm(|e| P2PRequestError::SendError(e.to_string()))?;
    let responses = response_rx
        .await
        .map_to_mm(|e| P2PRequestError::ResponseError(e.to_string()))?;
    Ok(parse_peers_responses(responses))
}

pub async fn request_peers<T: de::DeserializeOwned>(
    ctx: MmArc,
    req: P2PRequest,
    peers: Vec<String>,
) -> P2PRequestResult<Vec<(PeerId, PeerDecodedResponse<T>)>> {
    let encoded = encode_message(&req)?;

    let (response_tx, response_rx) = oneshot::channel();
    let p2p_ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let cmd = AdexBehaviourCmd::RequestPeers {
        req: encoded,
        peers,
        response_tx,
    };
    p2p_ctx
        .cmd_tx
        .lock()
        .try_send(cmd)
        .map_to_mm(|e| P2PRequestError::SendError(e.to_string()))?;
    let responses = response_rx
        .await
        .map_to_mm(|e| P2PRequestError::ResponseError(e.to_string()))?;
    Ok(parse_peers_responses(responses))
}

pub async fn request_one_peer<T: de::DeserializeOwned>(
    ctx: MmArc,
    req: P2PRequest,
    peer: String,
) -> P2PRequestResult<Option<T>> {
    let start = Instant::now();
    let mut responses = request_peers::<T>(ctx.clone(), req, vec![peer.clone()]).await?;
    let elapsed = start.elapsed();
    mm_timing!(ctx.metrics, "peer.outgoing_request.timing", elapsed, "peer" => peer);
    if responses.len() != 1 {
        return MmError::err(P2PRequestError::ExpectedSingleResponseError(responses.len()));
    }
    let (_, response) = responses.remove(0);
    match response {
        PeerDecodedResponse::Ok(response) => Ok(Some(response)),
        PeerDecodedResponse::None => Ok(None),
        PeerDecodedResponse::Err(e) => MmError::err(P2PRequestError::ResponseError(e)),
    }
}

fn parse_peers_responses<T: de::DeserializeOwned>(
    responses: Vec<(PeerId, AdexResponse)>,
) -> Vec<(PeerId, PeerDecodedResponse<T>)> {
    responses
        .into_iter()
        .map(|(peer_id, res)| {
            let res = match res {
                AdexResponse::Ok { response } => match decode_message::<T>(&response) {
                    Ok(res) => PeerDecodedResponse::Ok(res),
                    Err(e) => PeerDecodedResponse::Err(ERRL!("{}", e)),
                },
                AdexResponse::None => PeerDecodedResponse::None,
                AdexResponse::Err { error } => PeerDecodedResponse::Err(error),
            };
            (peer_id, res)
        })
        .collect()
}

pub fn propagate_message(ctx: &MmArc, message_id: MessageId, propagation_source: PeerId) {
    let ctx = ctx.clone();
    let p2p_ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let cmd = AdexBehaviourCmd::PropagateMessage {
        message_id,
        propagation_source,
    };
    if let Err(e) = p2p_ctx.cmd_tx.lock().try_send(cmd) {
        log::error!("propagate_message cmd_tx.send error {:?}", e);
    };
}

pub fn add_reserved_peer_addresses(ctx: &MmArc, peer: PeerId, addresses: PeerAddresses) {
    let ctx = ctx.clone();
    let p2p_ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let cmd = AdexBehaviourCmd::AddReservedPeer { peer, addresses };
    if let Err(e) = p2p_ctx.cmd_tx.lock().try_send(cmd) {
        log::error!("add_reserved_peer_addresses cmd_tx.send error {:?}", e);
    };
}

#[derive(Clone, Debug, Display, Serialize)]
pub enum NetIdError {
    #[display(fmt = "Netid {netid} is larger than max {max_netid}")]
    LargerThanMax { netid: u16, max_netid: u16 },
    #[display(fmt = "{netid} netid is deprecated.")]
    Deprecated { netid: u16 },
}

pub fn lp_ports(netid: u16) -> Result<(u16, u16, u16), MmError<NetIdError>> {
    const LP_RPCPORT: u16 = 7783;
    let max_netid = (65535 - 40 - LP_RPCPORT) / 4;
    if netid > max_netid {
        return MmError::err(NetIdError::LargerThanMax { netid, max_netid });
    }

    let other_ports = if netid != 0 {
        let net_mod = netid % 10;
        let net_div = netid / 10;
        (net_div * 40) + LP_RPCPORT + net_mod
    } else {
        LP_RPCPORT
    };
    Ok((other_ports + 10, other_ports + 20, other_ports + 30))
}

pub fn lp_network_ports(netid: u16) -> Result<NetworkPorts, MmError<NetIdError>> {
    let (_, network_port, network_wss_port) = lp_ports(netid)?;
    Ok(NetworkPorts {
        tcp: network_port,
        wss: network_wss_port,
    })
}

pub fn peer_id_from_secp_public(secp_public: &[u8]) -> Result<PeerId, MmError<DecodingError>> {
    let public_key = Libp2pSecpPublic::try_from_bytes(secp_public)?;
    Ok(PeerId::from_public_key(&Libp2pPublic::from(public_key)))
}
