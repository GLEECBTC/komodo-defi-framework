use futures::channel::{mpsc, oneshot};
use mm2_err_handle::prelude::*;

#[cfg(target_arch = "wasm32")]
pub mod webusb_driver;
#[cfg(target_arch = "wasm32")]
pub use webusb_driver::WebUsbError;

// #[cfg(not(target_arch = "wasm32"))] pub mod hid_driver;
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "ios")))]
pub mod libusb;
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "ios")))]
pub use libusb::UsbError;

trait InternalError: Sized {
    fn internal(e: String) -> Self;
}

#[cfg_attr(target_os = "ios", allow(dead_code))]
async fn send_event_recv_response<Event, Ok, Error>(
    event_tx: &mpsc::UnboundedSender<Event>,
    event: Event,
    result_rx: oneshot::Receiver<Result<Ok, MmError<Error>>>,
) -> Result<Ok, MmError<Error>>
where
    Error: InternalError + NotMmError,
{
    if let Err(e) = event_tx.unbounded_send(event) {
        let error = format!("Error sending event: {e}");
        return MmError::err(Error::internal(error));
    }
    match result_rx.await {
        Ok(result) => result,
        Err(e) => {
            let error = format!("Error receiving result: {e}");
            MmError::err(Error::internal(error))
        },
    }
}
