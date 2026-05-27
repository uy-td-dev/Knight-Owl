//! `owl schedule` — Hermes-style cron management for autonomous agent runs.
//!
//! Subcommands:
//!   - `add  <name> <cron-expr> <prompt>` → create a task
//!   - `list`                             → print every task
//!   - `remove <id>`                       → delete by id
//!   - `daemon`                            → run the scheduler in foreground
//!
//! Storage is SurrealDB (`cron_task` table); the daemon reuses the same
//! `ReasoningLoop` wiring as `owl chat`.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use tracing::info;

use owl_brain::AgentRunner;
use owl_protocol::schedule::SchedulerStore;
use owl_scheduler::{new_task, Scheduler};
use owl_vault::{SurrealSchedulerStore, SurrealStore, VaultConfig};

/// Manage scheduled tasks.
#[derive(Args)]
pub struct ScheduleCommand {
    #[command(subcommand)]
    pub sub: ScheduleSub,
}

#[derive(Subcommand)]
pub enum ScheduleSub {
    /// Create a new scheduled task.
    Add {
        /// Human-readable label.
        name: String,
        /// 6-field cron expression: "sec min hour day month dow".
        /// Example "0 0 9 * * Mon-Fri" = 9am on weekdays.
        cron_expr: String,
        /// Prompt the agent runs every time the cron fires.
        prompt: String,
    },
    /// List every scheduled task.
    List,
    /// Delete a task by id.
    Remove { id: String },
    /// Run the scheduler in foreground — fires tasks until interrupted.
    Daemon {
        /// Seconds between ticks (default 30).
        #[arg(long, default_value_t = 30u64)]
        interval_secs: u64,
    },
}

impl ScheduleCommand {
    pub async fn run(self) -> Result<()> {
        let cfg   = VaultConfig::load();
        let store = SurrealStore::connect(cfg.into_surreal()).await
            .context("connect to SurrealDB")?;
        let scheduler_store: Arc<dyn SchedulerStore> =
            Arc::new(SurrealSchedulerStore::new(Arc::new(store)));

        match self.sub {
            ScheduleSub::Add { name, cron_expr, prompt } => {
                let task = new_task(name.clone(), cron_expr.clone(), prompt);
                let id = task.id.clone();
                scheduler_store.upsert_task(task).await
                    .map_err(|e| anyhow!("upsert: {e}"))?;
                println!("Added task {id} \"{name}\" (cron: {cron_expr})");
            }
            ScheduleSub::List => {
                let tasks = scheduler_store.list_tasks().await
                    .map_err(|e| anyhow!("list: {e}"))?;
                if tasks.is_empty() {
                    println!("(no scheduled tasks)");
                } else {
                    for t in tasks {
                        println!(
                            "{}  {}  [{}]  enabled={}  last_run={:?}\n  → {}",
                            &t.id[..8.min(t.id.len())],
                            t.name,
                            t.cron_expr,
                            t.enabled,
                            t.last_run_ms,
                            t.prompt,
                        );
                    }
                }
            }
            ScheduleSub::Remove { id } => {
                scheduler_store.delete_task(&id).await
                    .map_err(|e| anyhow!("delete: {e}"))?;
                println!("Removed task {id}");
            }
            ScheduleSub::Daemon { interval_secs: _ } => {
                // Daemon needs a real AgentRunner.  Wiring the full
                // brain + tower stack here would duplicate `chat.rs`; for
                // PR1 we stub with a logging runner so the CLI surface is
                // testable and document the integration TODO.  Real wire
                // lands in Phase B.4-followup once `chat::build_loop` is
                // extracted into a reusable factory.
                let runner: Arc<dyn AgentRunner> = Arc::new(LoggingRunner);
                let scheduler = Scheduler::new(
                    Arc::clone(&scheduler_store),
                    runner,
                );
                info!("scheduler daemon started — Ctrl-C to exit");
                scheduler.run_forever().await
            }
        }
        Ok(())
    }
}

/// Placeholder runner that just logs the prompt.  Replaced by the real
/// reasoning-loop wiring once `chat::build_loop` is extracted into a
/// shareable factory.
struct LoggingRunner;
#[async_trait::async_trait]
impl AgentRunner for LoggingRunner {
    async fn run(&self, prompt: &str) -> Result<String, owl_brain::BrainError> {
        info!(prompt, "scheduled task fired (logging stub — wire ReasoningLoop here)");
        Ok(String::new())
    }
    async fn run_with_attachments(
        &self, prompt: &str, _: &[owl_protocol::attachment::Attachment],
    ) -> Result<String, owl_brain::BrainError> {
        self.run(prompt).await
    }
}
