#[path = "notification/telegram/telegram.rs"]
pub mod telegram;

use crate::lp_message_service::telegram::{ChatIdRegistry, TelegramError, TgClient};
use async_trait::async_trait;
use derive_more::Display;
use futures::lock::Mutex as AsyncMutex;
use mm2_core::mm_ctx::from_ctx;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use serde_json::{self as json};
use std::sync::Arc;

pub type MessageResult<T> = Result<T, MmError<MessageError>>;
pub const MAKER_BOT_ROOM_ID: &str = "maker_bot";
pub const DEFAULT_ROOM_ID: &str = "default";

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum MessageError {
    #[display(fmt = "{_0}")]
    TelegramError(TelegramError),
}

impl From<TelegramError> for MessageError {
    fn from(e: TelegramError) -> Self {
        MessageError::TelegramError(e)
    }
}

#[async_trait]
pub trait MessageServiceTraits {
    async fn send_message(&self, message: String, room_id: &str, disable_notification: bool) -> MessageResult<bool>;
}

#[derive(Default)]
pub struct MessageService {
    services: Vec<Box<dyn MessageServiceTraits + Send + Sync>>,
}

impl MessageService {
    pub async fn send_message(
        &self,
        message: String,
        room_id: &str,
        disable_notification: bool,
    ) -> MessageResult<bool> {
        for service in self.services.iter() {
            service
                .send_message(message.clone(), room_id, disable_notification)
                .await?;
        }
        Ok(true)
    }

    pub fn attach_service(&mut self, service: Box<dyn MessageServiceTraits + Send + Sync>) -> &MessageService {
        self.services.push(service);
        self
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub fn nb_services(&self) -> usize {
        self.services.len()
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub fn new() -> Self {
        Default::default()
    }
}

#[derive(Default)]
pub struct MessageServiceContext {
    pub message_service: AsyncMutex<MessageService>,
}

impl MessageServiceContext {
    /// Obtains a reference to this crate context, creating it if necessary.
    pub fn from_ctx(ctx: &MmArc) -> Result<Arc<MessageServiceContext>, String> {
        Ok(try_s!(from_ctx(&ctx.message_service_ctx, move || {
            Ok(MessageServiceContext::default())
        })))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageServiceCfg {
    telegram: Option<Telegram>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Telegram {
    api_key: String,
    chat_registry: ChatIdRegistry,
}

#[derive(Display)]
pub enum InitMessageServiceError {
    #[display(fmt = "Error deserializing '{field}' config field: {error}")]
    ErrorDeserializingConfig { field: String, error: String },
}

pub async fn init_message_service(ctx: &MmArc) -> Result<(), MmError<InitMessageServiceError>> {
    let maybe_cfg: Option<MessageServiceCfg> =
        json::from_value(ctx.conf["message_service_cfg"].clone()).map_to_mm(|e| {
            InitMessageServiceError::ErrorDeserializingConfig {
                field: "message_service_cfg".to_owned(),
                error: e.to_string(),
            }
        })?;
    if let Some(message_service_cfg) = maybe_cfg {
        let message_service_ctx = MessageServiceContext::from_ctx(ctx).unwrap();
        let mut message_service = message_service_ctx.message_service.lock().await;
        if let Some(telegram) = message_service_cfg.telegram {
            let tg_client = TgClient::new(telegram.api_key, None, telegram.chat_registry);
            message_service.attach_service(Box::new(tg_client));
            let _ = message_service
                .send_message(
                    "message service successfully initialized".to_string(),
                    DEFAULT_ROOM_ID,
                    false,
                )
                .await;
        }
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod message_service_tests {
    use crate::lp_message_service::telegram::{ChatIdRegistry, TgClient};
    use crate::lp_message_service::MessageService;
    use common::block_on;
    use std::collections::HashMap;
    use std::env::var;

    #[test]
    fn test_attach_service() {
        if let Ok(api_key) = var("TELEGRAM_API_KEY") {
            let tg_client = TgClient::new(api_key, None, HashMap::new());
            let nb_services = MessageService::new().attach_service(Box::new(tg_client)).nb_services();
            assert_eq!(nb_services, 1);
        }
    }

    #[test]
    fn test_send_message_service() {
        if let Ok(api_key) = var("TELEGRAM_API_KEY") {
            let chat_registry: ChatIdRegistry = vec![("RustTestChatId".to_string(), "-645728650".to_string())]
                .into_iter()
                .collect();
            let tg_client = TgClient::new(api_key, None, chat_registry);
            let mut message_service = MessageService::new();
            message_service.attach_service(Box::new(tg_client));
            let res = block_on(message_service.send_message(
                "Hey it's the message service, do you hear me?".to_string(),
                "RustTestChatId",
                true,
            ));
            assert!(res.is_ok());
            assert!(res.unwrap());
        }
    }
}
