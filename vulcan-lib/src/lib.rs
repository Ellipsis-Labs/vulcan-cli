//! Vulcan — AI-native CLI for Phoenix Perpetuals DEX on Solana.
//!
//! This is the core library crate. The binary crate (`vulcan`) handles
//! argument parsing and dispatches to command handlers here.

pub mod agent_install_state;
pub mod agent_log;
pub mod auth;
pub mod cli;
pub mod commands;
pub mod config;
pub mod context;
pub mod crypto;
pub mod error;
pub mod history;
pub mod indicators;
pub mod mcp;
pub mod output;
pub mod paper;
pub mod secrets;
pub mod strategy;
pub mod wallet;
pub mod watch;
