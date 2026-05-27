//! `owl gateway` — long-running messaging bridge.
//!
//! Subcommands:
//!   - `start --telegram`  — runs the Telegram bridge (env `OWL_TG_TOKEN`)
//!
//! For Phase D MVP the runner is a logging stub (same as `owl schedule
//! daemon`); plumbing the real reasoning loop in is tracked under the
//! "extract chat::build_loop into a reusable factory" follow-up.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};
use tracing::info;

use owl_brain::AgentRunner;
use owl_gateway::{Bridge, MessagingAdapter};

#[derive(Args)]
pub struct GatewayCommand {
    #[command(subcommand)]
    pub sub: GatewaySub,
}

#[derive(Subcommand)]
pub enum GatewaySub {
    /// Start a long-running gateway bridge for the chosen platform.
    Start {
        /// Bridge Telegram chat → agent.  Reads `OWL_TG_TOKEN`.
        #[arg(long, default_value_t = false)]
        telegram: bool,
    },
}

impl GatewayCommand {
    pub async fn run(self) -> Result<()> {
        match self.sub {
            GatewaySub::Start { telegram } => {
                if !telegram {
                    return Err(anyhow!(
                        "no platform selected — try `owl gateway start --telegram`"
                    ));
                }
                #[cfg(feature = "telegram")]
                {
                    use owl_gateway::telegram::TelegramAdapter;
                    let adapter: Arc<dyn MessagingAdapter> =
                        Arc::new(TelegramAdapter::from_env()
                            .map_err(|e| anyhow!("{e}"))?);
                    let runner: Arc<dyn AgentRunner> = Arc::new(LoggingRunner);
                    let bridge = Bridge::new(adapter, runner);
                    info!("telegram gateway started — Ctrl-C to exit");
                    bridge.run_forever().await
                        .map_err(|e| anyhow!("bridge: {e}"))?;
                    Ok(())
                }
                #[cfg(not(feature = "telegram"))]
                {
                    Err(anyhow!(
                        "owl-cli built without `telegram` feature — rebuild with `--features telegram`"
                    ))
                }
            }
        }
    }
}

/// Same placeholder runner as `owl schedule daemon` — emits a trace line
/// per message and returns an empty reply.  Replaced when chat-loop
/// wiring is extracted into a shared factory.
struct LoggingRunner;
#[async_trait::async_trait]
impl AgentRunner for LoggingRunner {
    async fn run(&self, prompt: &str) -> Result<String, owl_brain::BrainError> {
        info!(prompt, "gateway forwarded (logging stub)");
        Ok(format!("(stub) received: {prompt}"))
    }
    async fn run_with_attachments(
        &self, prompt: &str, _: &[owl_protocol::attachment::Attachment],
    ) -> Result<String, owl_brain::BrainError> {
        self.run(prompt).await
    }
}
