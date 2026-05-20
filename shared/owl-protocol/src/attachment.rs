//! Multi-modal user attachments — images, files, etc. supplied alongside a
//! chat message.
//!
//! Pure data types; the actual upload + provider-specific encoding lives in
//! the host (Tauri commands → owl-tower vision adapter).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Kind of attached content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[cfg_attr(feature = "ipc", derive(ts_rs::TS))]
#[cfg_attr(feature = "ipc", ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    /// Image attachment (PNG / JPEG / GIF / WEBP).  `data` is base64-encoded
    /// raw bytes (no `data:` URI prefix); `mime_type` is the standard MIME
    /// string, e.g. `"image/png"`.
    Image {
        mime_type: String,
        /// Base64-encoded raw bytes.
        data:      String,
        /// Optional human-readable description used as a text fallback when
        /// the active provider/model does not support vision input.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alt_text:  Option<String>,
    },
    /// Plain-text file attachment (.md / .txt / source code).  Inlined into
    /// the prompt body verbatim — no extra processing.
    Text {
        filename: String,
        content:  String,
    },
}

impl Attachment {
    /// Best-effort textual representation suitable for non-vision models.
    ///
    /// Lets a downstream tower adapter degrade gracefully when the active
    /// model can't accept the original modality (e.g. rig 0.9 + Anthropic
    /// vision is not yet wired — see `owl_tower::vision`).
    pub fn to_text_fallback(&self) -> String {
        match self {
            Self::Image { mime_type, alt_text, .. } => match alt_text {
                Some(t) => format!("[image attachment: {mime_type} — {t}]"),
                None    => format!("[image attachment: {mime_type} — no alt-text provided]"),
            },
            Self::Text { filename, content } => {
                format!("--- attached file: {filename} ---\n{content}\n--- end of {filename} ---")
            }
        }
    }
}
