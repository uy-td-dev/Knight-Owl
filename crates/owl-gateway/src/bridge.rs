//! `Bridge` — routes platform messages through an `AgentRunner`.

use std::sync::Arc;

use tracing::{info, warn};

use owl_brain::AgentRunner;

use crate::adapter::{MessagingAdapter, OutgoingReply};

/// Bridges incoming platform messages to an agent runner + ships its
/// reply back out.
///
/// One bridge per adapter — multi-platform deployments spawn multiple
/// bridges in parallel tokio tasks.
pub struct Bridge {
    adapter: Arc<dyn MessagingAdapter>,
    runner:  Arc<dyn AgentRunner>,
}

impl Bridge {
    pub fn new(adapter: Arc<dyn MessagingAdapter>, runner: Arc<dyn AgentRunner>) -> Self {
        Self { adapter, runner }
    }

    /// One pass: pull the current batch, dispatch each message to the
    /// agent, reply.  Returns the number of messages handled.  An empty
    /// batch is the adapter's shutdown signal — the caller stops looping.
    pub async fn tick(&self) -> Result<usize, crate::GatewayError> {
        let batch = self.adapter.poll().await?;
        if batch.is_empty() {
            return Ok(0);
        }
        let mut handled = 0usize;
        for msg in batch {
            info!(
                platform = self.adapter.label(),
                chat     = %msg.chat_id,
                from     = ?msg.from,
                "incoming"
            );
            let reply_text = match self.runner.run(&msg.text).await {
                Ok(r)  => r,
                Err(e) => {
                    warn!(err = %e, "agent run failed");
                    format!("⚠ agent error: {e}")
                }
            };
            if let Err(e) = self.adapter.send(OutgoingReply {
                chat_id: msg.chat_id,
                text:    reply_text,
            }).await {
                warn!(err = %e, "reply send failed");
            }
            handled += 1;
        }
        Ok(handled)
    }

    /// Loop forever — calls [`Self::tick`] until the adapter returns an
    /// empty batch (its shutdown signal).
    pub async fn run_forever(&self) -> Result<(), crate::GatewayError> {
        loop {
            let n = self.tick().await?;
            if n == 0 {
                info!(platform = self.adapter.label(), "adapter shutdown — exiting");
                return Ok(());
            }
        }
    }
}
