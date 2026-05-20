//! Scripted mock completion engine for deterministic testing.

use std::collections::VecDeque;
use std::sync::Mutex;

use rig::completion::message::AssistantContent;
use rig::completion::{CompletionError, CompletionRequest, CompletionResponse, Usage};
use rig::one_or_many::OneOrMany;

/// A mock `rig` completion model that returns pre-scripted responses.
///
/// Usage:
/// ```rust,ignore
/// let engine = MockEngine::new().on("hello", "Hi there!");
/// ```
#[derive(Clone)]
pub struct MockEngine {
    queue: std::sync::Arc<Mutex<VecDeque<String>>>,
}

impl MockEngine {
    /// Create an empty mock engine.
    pub fn new() -> Self {
        Self { queue: std::sync::Arc::new(Mutex::new(VecDeque::new())) }
    }

    /// Enqueue a scripted response (prompt is not matched; responses are FIFO).
    pub fn on(self, _prompt: &str, response: &str) -> Self {
        self.queue.lock().unwrap().push_back(response.to_owned());
        self
    }

    /// Pop the next scripted response, or return a default.
    pub fn next_response(&self) -> String {
        self.queue
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| "mock response".to_owned())
    }
}

impl Default for MockEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Phantom client type — `MockEngine` doesn't need a real provider client,
/// but `CompletionModel::Client` is required by the trait contract.
#[derive(Clone, Default)]
pub struct MockClient;

impl rig::completion::CompletionModel for MockEngine {
    type Response          = String;
    type StreamingResponse = MockStreamingResponse;
    type Client            = MockClient;

    fn make(_client: &Self::Client, _model: impl Into<String>) -> Self {
        Self::new()
    }

    async fn completion(
        &self,
        _request: CompletionRequest,
    ) -> Result<CompletionResponse<String>, CompletionError> {
        let text = self.next_response();
        Ok(CompletionResponse {
            choice:       OneOrMany::one(AssistantContent::text(text.clone())),
            usage:        Usage::default(),
            message_id:   None,
            raw_response: text,
        })
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
    ) -> Result<
        rig::streaming::StreamingCompletionResponse<Self::StreamingResponse>,
        CompletionError,
    > {
        // Tests using MockEngine never exercise the streaming path; if a test
        // ever needs it we can switch this to a real channel-backed stream.
        Err(CompletionError::ProviderError(
            "MockEngine::stream is not implemented".into(),
        ))
    }
}

/// Placeholder streaming-response type for [`MockEngine`].  Required by the
/// `CompletionModel` contract; never materialized because `stream()` errors.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MockStreamingResponse;

impl Unpin for MockStreamingResponse {}

impl rig::completion::GetTokenUsage for MockStreamingResponse {
    fn token_usage(&self) -> Option<Usage> { None }
}
