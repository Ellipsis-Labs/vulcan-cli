//! CLI definition — clap derive structs for all command groups.

pub mod account;
pub mod agent;
pub mod auth;
pub mod history;
pub mod margin;
pub mod market;
pub mod paper;
pub mod portfolio;
pub mod position;
pub mod strategy;
pub mod ta;
pub mod trade;
pub mod update;
pub mod wallet;

use crate::output::OutputFormat;
use clap::{Parser, Subcommand};

/// Vulcan — AI-native CLI for Phoenix Perpetuals DEX on Solana.
#[derive(Debug, Parser)]
#[command(
    name = "vulcan",
    version,
    about = "Vulcan — AI-native CLI for Phoenix Perpetuals DEX on Solana"
)]
pub struct Cli {
    /// Output format
    #[arg(short, long, value_enum, default_value = "table", global = true)]
    pub output: OutputFormat,

    /// Simulate the operation without submitting a transaction
    #[arg(long, default_value = "false", global = true)]
    pub dry_run: bool,

    /// Skip confirmation prompts
    #[arg(short, long, default_value = "false", global = true)]
    pub yes: bool,

    /// Solana RPC endpoint override
    #[arg(long, global = true)]
    pub rpc_url: Option<String>,

    /// Phoenix API endpoint override
    #[arg(long, global = true)]
    pub api_url: Option<String>,

    /// Stored wallet name to use instead of the default (`vulcan wallet list`). Ineffective for `vulcan mcp`.
    #[arg(short, long, global = true)]
    pub wallet: Option<String>,

    /// Enable verbose/debug logging to stderr
    #[arg(short, long, default_value = "false", global = true)]
    pub verbose: bool,

    /// Watch for live updates via WebSocket
    #[arg(long, default_value = "false", global = true)]
    pub watch: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Wallet management — create, import, list, and manage encrypted wallets
    #[command(subcommand)]
    Wallet(wallet::WalletCommand),

    /// Market data — prices, orderbooks, candles, funding rates
    #[command(subcommand)]
    Market(market::MarketCommand),

    /// Order management — place, cancel, and manage orders
    #[command(subcommand)]
    Trade(trade::TradeCommand),

    /// Position management — view and manage open positions
    #[command(subcommand)]
    Position(position::PositionCommand),

    /// Collateral management — deposit, withdraw, and monitor margin
    #[command(subcommand)]
    Margin(margin::MarginCommand),

    /// Account management — registration, info, subaccounts
    #[command(subcommand)]
    Account(account::AccountCommand),

    /// Phoenix API authentication — wallet-signed login and session status
    #[command(subcommand)]
    Auth(auth::AuthCommand),

    /// Portfolio snapshot — margin, positions, and open orders in one call
    Portfolio(portfolio::PortfolioArgs),

    /// Local paper trading — simulated perps with live prices and no real funds
    #[command(subcommand)]
    Paper(paper::PaperCommand),

    /// Trade and account history
    #[command(subcommand)]
    History(history::HistoryCommand),

    /// Agent setup — install skills and inspect agent integration
    #[command(subcommand)]
    Agent(agent::AgentCommand),

    /// First-class strategy runners — TWAP and other curated loops
    #[command(subcommand)]
    Strategy(strategy::StrategyCommand),

    /// Technical analysis — indicators over candle history and trigger eval
    #[command(subcommand)]
    Ta(ta::TaCommand),

    /// Check configuration, connectivity, wallet, and registration status
    Status,

    /// Check whether a newer Vulcan release is available on GitHub
    #[command(subcommand)]
    Update(update::UpdateCommand),

    /// Interactive setup wizard — wallet, config, and connectivity
    Setup,

    /// Print version and build information
    Version,

    /// Print agent runtime context (CONTEXT.md) to stdout
    AgentContext,

    /// Start MCP server over stdio
    Mcp {
        /// Allow dangerous commands without explicit acknowledgment
        #[arg(long)]
        allow_dangerous: bool,

        /// Command groups to expose (comma-separated)
        #[arg(long, value_delimiter = ',')]
        groups: Option<Vec<String>>,
    },
}
