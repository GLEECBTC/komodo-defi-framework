use http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_TYPE};
use hyper::{body::Bytes, Body, Request, Response};
use mm2_core::mm_ctx::MmArc;
use serde_json::json;

pub const SSE_ENDPOINT: &str = "/event-stream";

/// Handles broadcasted messages from `mm2_event_stream` continuously.
pub async fn handle_sse(request: Request<Body>, ctx_h: u32) -> Response<Body> {
    let ctx = match MmArc::from_ffi_handle(ctx_h) {
        Ok(ctx) => ctx,
        Err(err) => return handle_internal_error(err).await,
    };

    let Some(event_streaming_config) = ctx.event_streaming_configuration() else {
        return handle_internal_error("Event streaming is disabled".to_string()).await;
    };

    let client_id = request
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|p| p.strip_prefix("id=")))
        .and_then(|v| v.parse::<u64>().ok())
        // Default to zero when client ID isn't passed, most of the cases we will have a single user/client.
        .unwrap_or(0);

    let event_stream_manager = ctx.event_stream_manager.clone();
    let Ok(mut rx) = event_stream_manager.new_client(client_id) else {
        return handle_internal_error("ID already in use".to_string()).await;
    };
    let body = Body::wrap_stream(async_stream::stream! {
        while let Some(event) = rx.recv().await {
            // The event's filter will decide whether to expose the event data to this client or not.
            // This happens based on the events that this client has subscribed to.
            let (event_type, message) = event.get();
            let data = json!({
                "_type": event_type,
                "message": message,
            });

            yield Ok::<_, hyper::Error>(Bytes::from(format!("data: {data} \n\n")));
        }
    });

    let response = Response::builder()
        .status(200)
        .header(CONTENT_TYPE, "text/event-stream")
        .header(CACHE_CONTROL, "no-cache")
        .header(
            ACCESS_CONTROL_ALLOW_ORIGIN,
            event_streaming_config.access_control_allow_origin,
        )
        .body(body);

    match response {
        Ok(res) => res,
        Err(err) => handle_internal_error(err.to_string()).await,
    }
}

/// Fallback function for handling errors in SSE connections
async fn handle_internal_error(message: String) -> Response<Body> {
    Response::builder()
        .status(500)
        .body(Body::from(message))
        .expect("Returning 500 should never fail.")
}
