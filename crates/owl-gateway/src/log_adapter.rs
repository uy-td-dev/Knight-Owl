//! In-memory adapter for tests + local debugging.
//!
//! Queue messages with [`LogAdapter::push`], inspect captured replies via
//! [`LogAdapter::replies`].  Returning an empty batch from `poll` signals
//! the bridge to exit cleanly.

use std::sync::Mutex;

use async_trait::async_trait;

use crate::adapter::{GatewayError, IncomingMessage, MessagingAdapter, OutgoingReply};

/// Test adapter.  Holds two FIFO queues — one of incoming messages the
/// caller stages, one of replies the bridge sent.
pub struct LogAdapter {
    incoming: Mutex<Vec<IncomingMessage>>,
    replies:  Mutex<Vec<OutgoingReply>>,
}

impl LogAdapter {
    pub fn new() -> Self {
        Self { incoming: Mutex::new(Vec::new()), replies: Mutex::new(Vec::new()) }
    }

    /// Stage a message that the next `poll` will return.
    pub fn push(&self, msg: IncomingMessage) {
        self.incoming.lock().unwrap().push(msg);
    }

    /// Inspect every reply the bridge has sent so far.
    pub fn replies(&self) -> Vec<OutgoingReply> {
        self.replies.lock().unwrap().clone()
    }
}

impl Default for LogAdapter {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl MessagingAdapter for LogAdapter {
    async fn poll(&self) -> Result<Vec<IncomingMessage>, GatewayError> {
        // Drain and return — an empty queue tells the bridge to stop.
        Ok(std::mem::take(&mut *self.incoming.lock().unwrap()))
    }

    async fn send(&self, reply: OutgoingReply) -> Result<(), GatewayError> {
        self.replies.lock().unwrap().push(reply);
        Ok(())
    }

    fn label(&self) -> &'static str { "log" }
}
