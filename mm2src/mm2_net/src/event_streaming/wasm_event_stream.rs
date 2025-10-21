use mm2_core::mm_ctx::MmArc;
use serde_json::json;
use web_sys::SharedWorker;

struct SendableSharedWorker(SharedWorker);

unsafe impl Send for SendableSharedWorker {}

struct SendableMessagePort(web_sys::MessagePort);

unsafe impl Send for SendableMessagePort {}

/// Handles broadcasted messages from `mm2_event_stream` continuously for WASM.
pub async fn handle_worker_stream(ctx: MmArc, worker_path: String) {
    let worker = SendableSharedWorker(
        SharedWorker::new(&worker_path).unwrap_or_else(|_| {
            panic!(
                "Failed to create a new SharedWorker with path '{worker_path}'.\n\
                This could be due to the file missing or the browser being incompatible.\n\
                For more details, please refer to https://developer.mozilla.org/en-US/docs/Web/API/SharedWorker#browser_compatibility"
            )
        }),
    );

    let port = SendableMessagePort(worker.0.port());
    port.0.start();

    let event_stream_manager = ctx.event_stream_manager.clone();
    let mut rx = event_stream_manager
        .new_client(0)
        .expect("A different wasm client is already listening. Only one client is allowed at a time.");

    while let Some(event) = rx.recv().await {
        let (event_type, message) = event.get();
        let data = json!({
            "_type": event_type,
            "message": message,
        });
        let message_js = wasm_bindgen::JsValue::from_str(&data.to_string());
        port.0.post_message(&message_js)
            .expect("Failed to post a message to the SharedWorker.\n\
            This could be due to the browser being incompatible.\n\
            For more details, please refer to https://developer.mozilla.org/en-US/docs/Web/API/MessagePort/postMessage#browser_compatibility");
    }
}
