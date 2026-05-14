//! Agent setup subcommand definitions.

use clap::{Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Install Vulcan Agent Skills for a local agent client
    Install {
        /// Agent client to install skills for
        #[arg(long, value_enum, default_value = "claude")]
        target: AgentTarget,

        /// Install scope
        #[arg(long, value_enum, default_value = "user")]
        scope: AgentScope,

        /// Override the skills directory
        #[arg(long)]
        dir: Option<PathBuf>,

        /// Overwrite locally modified skill files
        #[arg(long)]
        force: bool,
    },

    /// Inspect known agent skill locations
    Doctor {
        /// Agent client to inspect
        #[arg(long, value_enum, default_value = "claude")]
        target: AgentTarget,

        /// Install scope to inspect
        #[arg(long, value_enum, default_value = "user")]
        scope: AgentScope,

        /// Override the skills directory
        #[arg(long)]
        dir: Option<PathBuf>,
    },

    /// Combined health check for first-run agent guidance
    Health {
        /// Agent client to inspect. Omit to check all supported targets.
        #[arg(long, value_enum)]
        target: Option<AgentTarget>,

        /// Install scope to inspect
        #[arg(long, value_enum, default_value = "user")]
        scope: AgentScope,

        /// Override the skills directory. Best used with --target.
        #[arg(long)]
        dir: Option<PathBuf>,
    },

    /// Check whether live agent execution is ready for a target client
    LiveReady {
        /// Agent client to inspect
        #[arg(long, value_enum)]
        target: Option<AgentTarget>,

        /// Install scope to inspect
        #[arg(long, value_enum, default_value = "user")]
        scope: AgentScope,
    },

    /// Configure or inspect Vulcan MCP integration for agent clients
    #[command(subcommand)]
    Mcp(AgentMcpCommand),

    /// Inspect the local agent action/session log
    #[command(subcommand)]
    Log(AgentLogCommand),
}

#[derive(Debug, Subcommand)]
pub enum AgentMcpCommand {
    /// Print an MCP config snippet for an agent client
    PrintConfig(AgentMcpConfigArgs),

    /// Inspect expected MCP config for an agent client
    Doctor(AgentMcpDoctorArgs),

    /// Install or update the Vulcan MCP config for an agent client
    Install(AgentMcpInstallArgs),

    /// Switch the wallet used by an already-installed Vulcan MCP server
    SetWallet(AgentMcpSetWalletArgs),

    /// End-to-end probe: read the installed MCP entry, spawn the server with
    /// the exact command/args/env an agent client would use, run the JSON-RPC
    /// handshake, and assert vulcan_* tools come back. Reliable PASS/FAIL test
    /// for "will my next agent session actually see the Vulcan tools?"
    Diagnose(AgentMcpDoctorArgs),
}

#[derive(Debug, Clone, clap::Args)]
pub struct AgentMcpConfigArgs {
    /// Agent client target
    #[arg(long, value_enum, default_value = "cursor")]
    pub target: AgentTarget,

    /// Include live/dangerous tools in the MCP server config
    #[arg(long, default_value = "false")]
    pub dangerous: bool,

    /// Command groups to expose (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub groups: Option<Vec<String>>,
}

#[derive(Debug, Clone, clap::Args)]
pub struct AgentMcpDoctorArgs {
    /// Agent client target
    #[arg(long, value_enum, default_value = "cursor")]
    pub target: AgentTarget,

    /// Install scope
    #[arg(long, value_enum, default_value = "user")]
    pub scope: AgentScope,

    /// Override MCP config file path
    #[arg(long)]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, clap::Args)]
pub struct AgentMcpInstallArgs {
    /// Agent client target
    #[arg(long, value_enum, default_value = "cursor")]
    pub target: AgentTarget,

    /// Install scope
    #[arg(long, value_enum, default_value = "user")]
    pub scope: AgentScope,

    /// Override MCP config file path
    #[arg(long)]
    pub path: Option<PathBuf>,

    /// Include live/dangerous tools in the MCP server config
    #[arg(long, default_value = "false")]
    pub dangerous: bool,

    /// Command groups to expose (comma-separated)
    #[arg(long, value_delimiter = ',')]
    pub groups: Option<Vec<String>>,

    /// Overwrite unrecognized existing config files
    #[arg(long)]
    pub force: bool,

    /// Update only `command` and `args` of an existing entry, preserving the
    /// `env` block (wallet name + password). Non-interactive — no TTY, no
    /// password prompt. Use this to migrate an existing install to the new
    /// absolute-path command without re-entering the wallet password. Fails
    /// if no entry exists at the target pointer.
    #[arg(long, conflicts_with = "force")]
    pub repair: bool,
}

#[derive(Debug, Clone, clap::Args)]
pub struct AgentMcpSetWalletArgs {
    /// Wallet name MCP should unlock on restart
    pub wallet: String,

    /// Agent client target
    #[arg(long, value_enum, default_value = "cursor")]
    pub target: AgentTarget,

    /// Install scope
    #[arg(long, value_enum, default_value = "user")]
    pub scope: AgentScope,

    /// Override MCP config file path
    #[arg(long)]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum AgentLogCommand {
    /// Show recent redacted action log records
    Show {
        /// Maximum number of records to return
        #[arg(long, default_value_t = 50)]
        limit: usize,

        /// Session id to filter, or "current" for this process session
        #[arg(long)]
        session: Option<String>,
    },

    /// Summarize recent actions, positions, PnL, errors, and transactions
    Summary {
        /// Maximum number of records to include
        #[arg(long, default_value_t = 50)]
        limit: usize,

        /// Session id to filter, or "current" for this process session
        #[arg(long)]
        session: Option<String>,
    },

    /// Build a position/session report from live trader state and local logs
    Report {
        /// Maximum number of records to include
        #[arg(long, default_value_t = 50)]
        limit: usize,

        /// Session id to filter, or "current" for this process session
        #[arg(long)]
        session: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AgentTarget {
    /// Claude Code / Claude-compatible skill directories
    Claude,
    /// Cursor skill directories
    Cursor,
    /// OpenAI Codex skill directories
    Codex,
    /// Generic agentskills.io-style directory
    Agentskills,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AgentScope {
    /// Install for the current user
    User,
    /// Install into the current project
    Project,
}
