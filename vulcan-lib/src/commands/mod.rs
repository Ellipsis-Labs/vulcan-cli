//! Command execution logic.
//!
//! Each module receives parsed CLI args, calls the SDK, and returns typed results.
//! The output layer handles formatting.

pub mod account;
pub mod agent;
pub mod auth;
pub mod conditional_orders;
pub mod history;
pub mod margin;
pub mod market;
pub mod paper;
pub mod portfolio;
pub mod position;
pub mod setup;
pub mod status;
pub mod strategy;
pub mod ta;
pub mod trade;
pub mod trader_state;
pub mod wallet;
