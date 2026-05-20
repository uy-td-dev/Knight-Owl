//! Vision / multi-modal input — adapter helpers.
//!
//! ## Activation status
//!
//! `rig 0.9` (currently used) only exposes `Prompt::prompt(&str)` — no
//! native multi-modal content blocks.  Until the workspace bumps to a rig
//! version that supports image inputs (or a custom HTTP path is added per
//! provider), every [`Attachment`] is degraded to a text fallback via
//! [`compose_prompt_with_fallback`].
//!
//! When upgrading rig:
//!
//! 1. Add a `compose_prompt_with_vision()` that returns a provider-native
//!    multi-modal request (e.g. Anthropic `MessageParam::Content(Vec<Block>)`).
//! 2. Switch the brain caller from `compose_prompt_with_fallback` to the
//!    vision-aware variant when the active model id is in
//!    [`is_vision_capable`].
//!
//! [`Attachment`]: owl_protocol::attachment::Attachment

use owl_protocol::attachment::Attachment;

/// Inline every [`Attachment`] as text and prepend to `prompt`.
///
/// Safe to use with any model — vision-capable or not.  Loses fidelity for
/// images (only the alt-text survives) but keeps the request shape uniform.
pub fn compose_prompt_with_fallback(prompt: &str, attachments: &[Attachment]) -> String {
    if attachments.is_empty() {
        return prompt.to_string();
    }
    let inlined: String = attachments
        .iter()
        .map(Attachment::to_text_fallback)
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{inlined}\n\n{prompt}")
}

/// Heuristic — does the given model id support image input?
///
/// Used by future vision wire-up (post rig bump) to decide whether to send
/// a native multi-modal request or fall back to text inlining.  Conservative:
/// returns `true` only for known-vision models; unknown ids → `false`.
pub fn is_vision_capable(model_id: &str) -> bool {
    let m = model_id.to_lowercase();
    // Anthropic vision: all Claude 3+ family.
    if m.starts_with("claude-3") || m.starts_with("claude-sonnet-4")
        || m.starts_with("claude-opus-4") || m.starts_with("claude-haiku-4") {
        return true;
    }
    // Google Gemini: all 1.5+ and 2+ models.
    if m.starts_with("gemini-1.5") || m.starts_with("gemini-2") {
        return true;
    }
    // Ollama vision models — by convention (LLaVA, BakLLaVA, etc.).
    if m.contains("llava") || m.contains("vision") || m == "moondream" {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_attachments_returns_prompt_unchanged() {
        assert_eq!(compose_prompt_with_fallback("hi", &[]), "hi");
    }

    #[test]
    fn image_attachment_inlines_alt_text() {
        let att = Attachment::Image {
            mime_type: "image/png".into(),
            data:      "ZmFrZQ==".into(),
            alt_text:  Some("a cat".into()),
        };
        let composed = compose_prompt_with_fallback("describe this", &[att]);
        assert!(composed.contains("[image attachment: image/png — a cat]"));
        assert!(composed.contains("describe this"));
    }

    #[test]
    fn vision_capability_known_models() {
        assert!( is_vision_capable("claude-sonnet-4-6"));
        assert!( is_vision_capable("claude-3-5-sonnet"));
        assert!( is_vision_capable("gemini-2.5-flash"));
        assert!( is_vision_capable("llama3.2-vision"));
        assert!(!is_vision_capable("llama3.2"));
        assert!(!is_vision_capable("unknown-model"));
    }
}
