//! Portfolio subcommand definitions.

use clap::Args;

#[derive(Debug, Args)]
pub struct PortfolioArgs {
    /// Sections to include (default: all). Comma-separated: margin,positions,orders.
    #[arg(long, value_delimiter = ',')]
    pub include: Vec<String>,
}
