//! Bridge integration tests using the in-memory `LogAdapter`.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;

use owl_brain::{AgentRunner, BrainError};
use owl_gateway::{Bridge, IncomingMessage, LogAdapter};
use owl_protocol::attachment::Attachment;

/// Records each prompt + echoes it back as the reply.
struct EchoRunner { seen: Mutex<Vec<String>> }
#[async_trait]
impl AgentRunner for EchoRunner {
    async fn run(&self, prompt: &str) -> Result<String, BrainError> {
        self.seen.lock().unwrap().push(prompt.to_string());
        Ok(format!("echo: {prompt}"))
    }
    async fn run_with_attachments(
        &self, prompt: &str, _: &[Attachment],
    ) -> Result<String, BrainError> {
        self.run(prompt).await
    }
}

#[tokio::test]
async fn bridge_forwards_messages_and_replies() {
    let adapter = Arc::new(LogAdapter::new());
    adapter.push(IncomingMessage {
        chat_id: "c1".into(), from: Some("alice".into()), text: "hello".into(),
    });
    adapter.push(IncomingMessage {
        chat_id: "c2".into(), from: None, text: "second".into(),
    });

    let runner_impl = Arc::new(EchoRunner { seen: Mutex::new(Vec::new()) });
    let runner:  Arc<dyn AgentRunner>           = Arc::clone(&runner_impl) as _;
    let adapter_dyn: Arc<dyn owl_gateway::MessagingAdapter> = Arc::clone(&adapter) as _;

    let bridge = Bridge::new(adapter_dyn, runner);
    let n = bridge.tick().await.unwrap();
    assert_eq!(n, 2);

    let seen = runner_impl.seen.lock().unwrap();
    assert_eq!(*seen, vec!["hello".to_string(), "second".to_string()]);

    let replies = adapter.replies();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0].chat_id, "c1");
    assert_eq!(replies[0].text,    "echo: hello");
    assert_eq!(replies[1].chat_id, "c2");
}

#[tokio::test]
async fn bridge_run_forever_exits_on_empty_batch() {
    let adapter = Arc::new(LogAdapter::new());
    // No messages staged — first poll returns empty → bridge exits.
    let runner: Arc<dyn AgentRunner> = Arc::new(EchoRunner { seen: Mutex::new(vec![]) });
    let adapter_dyn: Arc<dyn owl_gateway::MessagingAdapter> = Arc::clone(&adapter) as _;
    let bridge = Bridge::new(adapter_dyn, runner);
    // Should return Ok(()) without hanging.
    bridge.run_forever().await.unwrap();
}

#[tokio::test]
async fn bridge_recovers_from_runner_error() {
    /// Runner that always errors — bridge must still send an error reply.
    struct FailingRunner;
    #[async_trait]
    impl AgentRunner for FailingRunner {
        async fn run(&self, _: &str) -> Result<String, BrainError> {
            Err(BrainError::Completion("simulated".into()))
        }
        async fn run_with_attachments(
            &self, p: &str, _: &[Attachment],
        ) -> Result<String, BrainError> { self.run(p).await }
    }

    let adapter = Arc::new(LogAdapter::new());
    adapter.push(IncomingMessage {
        chat_id: "c".into(), from: None, text: "ask".into(),
    });

    let runner: Arc<dyn AgentRunner> = Arc::new(FailingRunner);
    let adapter_dyn: Arc<dyn owl_gateway::MessagingAdapter> = Arc::clone(&adapter) as _;
    let bridge = Bridge::new(adapter_dyn, runner);
    let n = bridge.tick().await.unwrap();
    assert_eq!(n, 1);

    let replies = adapter.replies();
    assert_eq!(replies.len(), 1);
    assert!(replies[0].text.contains("agent error"));
}
