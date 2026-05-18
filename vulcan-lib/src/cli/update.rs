//! Update command CLI definitions.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum UpdateCommand {
    /// Check whether a newer Vulcan release is published on GitHub.
    Check {
        /// Bypass the on-disk cache and hit the GitHub API directly.
        #[arg(long)]
        force: bool,
    },
}
