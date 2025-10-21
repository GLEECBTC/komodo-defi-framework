use super::settle::send_session_settle_request;
use crate::storage::WalletConnectStorageOps;
use crate::{
    error::WalletConnectError,
    metadata::generate_metadata,
    session::{Session, SessionKey, SessionType, THIRTY_DAYS},
    WalletConnectCtxImpl,
};

use chrono::Utc;
use mm2_err_handle::map_to_mm::MapToMmResult;
use mm2_err_handle::prelude::*;
use relay_rpc::rpc::params::session::ProposeNamespaces;
use relay_rpc::{
    domain::{MessageId, Topic},
    rpc::params::{
        session_propose::{Proposer, SessionProposeRequest, SessionProposeResponse},
        RequestParams, ResponseParamsSuccess,
    },
};

/// Creates a new session proposal from topic and metadata.
pub(crate) async fn send_session_proposal_request(
    ctx: &WalletConnectCtxImpl,
    topic: &Topic,
    required_namespaces: ProposeNamespaces,
    optional_namespaces: ProposeNamespaces,
) -> MmResult<(), WalletConnectError> {
    let proposer = Proposer {
        metadata: ctx.metadata.clone(),
        public_key: hex::encode(ctx.key_pair.public_key.as_bytes()),
    };
    let session_proposal = RequestParams::SessionPropose(SessionProposeRequest {
        relays: vec![ctx.relay.clone()],
        proposer,
        required_namespaces,
        optional_namespaces: Some(optional_namespaces),
    });
    let _ = ctx.publish_request(topic, session_proposal).await?;

    Ok(())
}

/// Process session proposal request
/// https://specs.walletconnect.com/2.0/specs/clients/sign/session-proposal
pub async fn reply_session_proposal_request(
    ctx: &WalletConnectCtxImpl,
    proposal: SessionProposeRequest,
    topic: &Topic,
    message_id: &MessageId,
) -> MmResult<(), WalletConnectError> {
    let session = {
        let sender_public_key = hex::decode(&proposal.proposer.public_key)?
            .as_slice()
            .try_into()
            .map_to_mm(|_| WalletConnectError::InternalError("Invalid sender_public_key".to_owned()))?;
        let session_key = SessionKey::from_osrng(&sender_public_key)?;
        let session_topic: Topic = session_key.generate_topic().into();
        let subscription_id = ctx
            .client
            .subscribe(session_topic.clone())
            .await
            .map_to_mm(|err| WalletConnectError::SubscriptionError(err.to_string()))?;

        Session::new(
            ctx,
            session_topic.clone(),
            subscription_id,
            session_key,
            topic.clone(),
            proposal.proposer.metadata,
            SessionType::Controller,
        )
    };
    // TODO: Note that this will always error since we never populate `propose_namespaces`.
    //       But this doesn't matter for now as this method (replying to session proposal) is only relevant when KDF is acting as a wallet.
    // TODO: If the required namespaces aren't supported, we should ideally return SessionReject response.
    session
        .propose_namespaces
        .supported(&proposal.required_namespaces)
        .map_to_mm(|err| WalletConnectError::InternalError(err.to_string()))?;

    {
        // save session to storage
        ctx.session_manager
            .storage()
            .save_session(&session)
            .await
            .mm_err(|err| WalletConnectError::StorageError(err.to_string()))?;

        // Add session to session lists
        ctx.session_manager.add_session(session.clone());
    }

    send_session_settle_request(ctx, &session).await?;

    // Respond to incoming session propose.
    let param = ResponseParamsSuccess::SessionPropose(SessionProposeResponse {
        relay: ctx.relay.clone(),
        responder_public_key: proposal.proposer.public_key,
    });

    ctx.publish_response_ok(topic, param, message_id).await?;

    Ok(())
}

/// Process session propose reponse.
pub(crate) async fn process_session_propose_response(
    ctx: &WalletConnectCtxImpl,
    pairing_topic: &Topic,
    response: &SessionProposeResponse,
) -> MmResult<(), WalletConnectError> {
    let session_key = {
        let other_public_key = hex::decode(&response.responder_public_key)?
            .as_slice()
            .try_into()
            .unwrap();
        let mut session_key = SessionKey::new(ctx.key_pair.public_key);
        session_key.generate_symmetric_key(&ctx.key_pair.secret, &other_public_key)?;
        session_key
    };

    let session = {
        let session_topic: Topic = session_key.generate_topic().into();
        let subscription_id = ctx
            .client
            .subscribe(session_topic.clone())
            .await
            .map_to_mm(|err| WalletConnectError::SubscriptionError(err.to_string()))?;

        let mut session = Session::new(
            ctx,
            session_topic.clone(),
            subscription_id,
            session_key,
            pairing_topic.clone(),
            generate_metadata(),
            SessionType::Proposer,
        );
        session.relay = response.relay.clone();
        session.expiry = Utc::now().timestamp() as u64 + THIRTY_DAYS;
        session.controller.public_key = response.responder_public_key.clone();
        session
    };

    // save session to storage
    ctx.session_manager
        .storage()
        .save_session(&session)
        .await
        .mm_err(|err| WalletConnectError::StorageError(err.to_string()))?;

    // Add session to session lists
    ctx.session_manager.add_session(session.clone());

    // Activate pairing_topic
    ctx.pairing.activate(pairing_topic)?;

    Ok(())
}
