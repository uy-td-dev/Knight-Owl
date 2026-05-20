//! Trait-erased runner for injecting the reasoning loop into app-layer state.

use async_trait::async_trait;

use owl_protocol::attachment::Attachment;

use crate::BrainError;

/// Object-safe interface for running the reasoning loop.
///
/// Implemented by `ReasoningLoop<M>` — allows the app layer to hold
/// `Arc<dyn AgentRunner>` without knowing the concrete model type.
#[async_trait]
pub trait AgentRunner: Send + Sync {
    /// Run the reasoning loop for the given prompt, returning the final answer.
    async fn run(&self, prompt: &str) -> Result<String, BrainError>;

    /// Run with multi-modal attachments (images, files).
    ///
    /// Default impl inlines each attachment as text via
    /// [`Attachment::to_text_fallback`] then delegates to [`Self::run`].
    /// Vision-aware implementations override this to send a native
    /// multi-modal request to the active model.
    async fn run_with_attachments(
        &self,
        prompt: &str,
        attachments: &[Attachment],
    ) -> Result<String, BrainError> {
        if attachments.is_empty() {
            return self.run(prompt).await;
        }
        let inlined: String = attachments
            .iter()
            .map(Attachment::to_text_fallback)
            .collect::<Vec<_>>()
            .join("\n\n");
        let composed = format!("{inlined}\n\n{prompt}");
        self.run(&composed).await
    }
}
