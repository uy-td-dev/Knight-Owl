//! CLI command definitions.

pub mod chat;
pub mod gateway;
pub mod git_ingest;
pub mod index;
pub mod schedule;
pub mod watch;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Knight-Owl — a modular multi-LLM AI agent.
#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

impl Cli {
    /// Dispatch the selected subcommand.
    pub async fn run(self) -> Result<()> {
        match self.command {
            Command::Chat(cmd)      => cmd.run().await,
            Command::Gateway(cmd)   => cmd.run().await,
            Command::GitIngest(cmd) => cmd.run().await,
            Command::Index(cmd)     => cmd.run().await,
            Command::Schedule(cmd)  => cmd.run().await,
            Command::Watch(cmd)     => cmd.run().await,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Send a message and receive a response.
    Chat(chat::ChatCommand),
    /// Bridge external messaging platforms (Telegram, Discord, …) to the agent.
    Gateway(gateway::GatewayCommand),
    /// Ingest git commit history into the L4 knowledge graph.
    GitIngest(git_ingest::GitIngestCommand),
    /// Index a workspace into the code graph (run before `chat --workspace`).
    Index(index::IndexCommand),
    /// Manage scheduled / cron-driven autonomous tasks.
    Schedule(schedule::ScheduleCommand),
    /// Watch a workspace and re-index on file changes.
    Watch(watch::WatchCommand),
}
