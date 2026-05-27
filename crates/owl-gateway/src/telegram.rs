//! Telegram adapter — Bot API long-poll via reqwest.
//!
//! Avoids `teloxide` to keep the dep graph small.  Only implements the
//! 2 endpoints we need: `getUpdates` (poll) and `sendMessage` (reply).
//! Bot token is read from `OWL_TG_TOKEN` at construction.
//!
//! Behaviour:
//!   - `poll()` long-polls 25 s, returns all received text messages,
//!     advances the internal `update_id` cursor so we don't replay.
//!   - `send()` calls Bot API `sendMessage`.
//!
//! Non-text updates (stickers, photos, voice memos) are currently
//! ignored — Phase D MVP.  Voice transcription is a Tier-3 follow-up.

use std::sync::Mutex;

use async_trait::async_trait;
use serde::Deserialize;
use tracing::warn;

use crate::adapter::{GatewayError, IncomingMessage, MessagingAdapter, OutgoingReply};

/// Telegram Bot API adapter.
pub struct TelegramAdapter {
    token:  String,
    client: reqwest::Client,
    /// Cursor for `getUpdates?offset=<last_seen + 1>`.
    last_update_id: Mutex<i64>,
}

impl TelegramAdapter {
    /// Construct from a bot token (env: `OWL_TG_TOKEN`).
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token:  token.into(),
            client: reqwest::Client::new(),
            last_update_id: Mutex::new(0),
        }
    }

    /// Convenience: read the token from `OWL_TG_TOKEN`.
    pub fn from_env() -> Result<Self, GatewayError> {
        let t = std::env::var("OWL_TG_TOKEN")
            .map_err(|_| GatewayError::Config(
                "OWL_TG_TOKEN not set — get a bot token from @BotFather".into()
            ))?;
        Ok(Self::new(t))
    }

    fn base(&self) -> String {
        format!("https://api.telegram.org/bot{}", self.token)
    }
}

#[async_trait]
impl MessagingAdapter for TelegramAdapter {
    async fn poll(&self) -> Result<Vec<IncomingMessage>, GatewayError> {
        let offset = *self.last_update_id.lock().unwrap() + 1;
        let url = format!("{}/getUpdates", self.base());
        let resp = self.client
            .get(&url)
            .query(&[
                ("offset",  offset.to_string()),
                ("timeout", "25".to_string()),
            ])
            .send()
            .await
            .map_err(|e| GatewayError::Transport(e.to_string()))?;
        let body: GetUpdatesResp = resp
            .json()
            .await
            .map_err(|e| GatewayError::Transport(e.to_string()))?;
        if !body.ok {
            return Err(GatewayError::Transport(
                body.description.unwrap_or_else(|| "telegram getUpdates error".into())
            ));
        }

        let mut out = Vec::new();
        let mut max_id = *self.last_update_id.lock().unwrap();
        for u in body.result {
            if u.update_id > max_id { max_id = u.update_id; }
            let Some(m) = u.message else { continue; };
            let Some(text) = m.text else {
                warn!(update = u.update_id, "ignoring non-text Telegram update");
                continue;
            };
            out.push(IncomingMessage {
                chat_id: m.chat.id.to_string(),
                from:    m.from.and_then(|u| u.username.or(Some(u.first_name))),
                text,
            });
        }
        *self.last_update_id.lock().unwrap() = max_id;
        Ok(out)
    }

    async fn send(&self, reply: OutgoingReply) -> Result<(), GatewayError> {
        let url = format!("{}/sendMessage", self.base());
        let resp = self.client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": reply.chat_id,
                "text":    reply.text,
            }))
            .send()
            .await
            .map_err(|e| GatewayError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Transport(
                format!("telegram sendMessage {status}: {body}")
            ));
        }
        Ok(())
    }

    fn label(&self) -> &'static str { "telegram" }
}

#[derive(Deserialize)]
struct GetUpdatesResp {
    ok:          bool,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    result:      Vec<TgUpdate>,
}

#[derive(Deserialize)]
struct TgUpdate {
    update_id: i64,
    #[serde(default)]
    message:   Option<TgMessage>,
}

#[derive(Deserialize)]
struct TgMessage {
    chat: TgChat,
    #[serde(default)]
    from: Option<TgUser>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct TgChat { id: i64 }

#[derive(Deserialize)]
struct TgUser {
    first_name: String,
    #[serde(default)]
    username:   Option<String>,
}
