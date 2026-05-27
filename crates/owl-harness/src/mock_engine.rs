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
    /// Usage returned by every streaming response.  `None` means the
    /// mock provider doesn't report token usage (default).
    usage: std::sync::Arc<Mutex<Option<Usage>>>,
}

impl MockEngine {
    /// Create an empty mock engine.
    pub fn new() -> Self {
        Self {
            queue: std::sync::Arc::new(Mutex::new(VecDeque::new())),
            usage: std::sync::Arc::new(Mutex::new(None)),
        }
    }

    /// Enqueue a scripted response (prompt is not matched; responses are FIFO).
    pub fn on(self, _prompt: &str, response: &str) -> Self {
        self.queue.lock().unwrap().push_back(response.to_owned());
        self
    }

    /// Set the per-call token usage every streaming response reports.
    /// Set once; applies to every subsequent `stream()` invocation.
    pub fn with_usage(self, usage: Usage) -> Self {
        *self.usage.lock().unwrap() = Some(usage);
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

    /// Snapshot of the configured token usage (cloned).
    pub fn current_usage(&self) -> Option<Usage> {
        *self.usage.lock().unwrap()
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
        use rig::streaming::{RawStreamingChoice, StreamingCompletionResponse};

        let text  = self.next_response();
        let usage = self.current_usage();
        // Yield the whole scripted response as one Message chunk followed by a
        // FinalResponse marker — matches how real providers signal end-of-turn.
        let items: Vec<Result<RawStreamingChoice<MockStreamingResponse>, CompletionError>> = vec![
            Ok(RawStreamingChoice::Message(text)),
            Ok(RawStreamingChoice::FinalResponse(MockStreamingResponse { usage })),
        ];
        let stream = futures_util::stream::iter(items);
        Ok(StreamingCompletionResponse::stream(Box::pin(stream)))
    }
}

/// Streaming-response type for [`MockEngine`].
///
/// Carries an optional [`Usage`] so tests can verify Phase F token
/// capture.  Real providers populate this from the underlying API
/// response; the mock lets tests inject any value.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MockStreamingResponse {
    pub usage: Option<Usage>,
}

impl Unpin for MockStreamingResponse {}

impl rig::completion::GetTokenUsage for MockStreamingResponse {
    fn token_usage(&self) -> Option<Usage> { self.usage }
}
