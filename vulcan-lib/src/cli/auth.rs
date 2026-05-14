//! Phoenix API auth subcommand definitions.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Log in to the Phoenix API by signing a wallet challenge
    Login,

    /// Show redacted Phoenix API session status
    Status,

    /// Clear the stored Phoenix API session
    Logout,
}
