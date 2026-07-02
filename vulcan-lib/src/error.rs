//! Error types and categories for Vulcan CLI
//!
//! Every error carries a category that maps to a deterministic exit code
//! and tells agents whether the error is retryable.

use serde::Serialize;
use std::fmt;

/// Error categories with deterministic exit codes.
///
/// These map 1:1 to the categories in `error-catalog.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Validation,
    Auth,
    Config,
    Api,
    Network,
    RateLimit,
    TxFailed,
    Io,
    DangerousGate,
    Internal,
}

impl ErrorCategory {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Validation => 1,
            Self::Auth => 2,
            Self::Config => 3,
            Self::Api => 4,
            Self::Network => 5,
            Self::RateLimit => 6,
            Self::TxFailed => 7,
            Self::Io => 8,
            Self::DangerousGate => 9,
            Self::Internal => 10,
        }
    }

    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Network | Self::RateLimit | Self::Io)
    }
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Validation => "validation",
            Self::Auth => "auth",
            Self::Config => "config",
            Self::Api => "api",
            Self::Network => "network",
            Self::RateLimit => "rate_limit",
            Self::TxFailed => "tx_failed",
            Self::Io => "io",
            Self::DangerousGate => "dangerous_gate",
            Self::Internal => "internal",
        };
        f.write_str(s)
    }
}

/// The main error type for Vulcan.
#[derive(Debug)]
pub struct VulcanError {
    pub category: ErrorCategory,
    pub code: String,
    pub message: String,
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl VulcanError {
    pub fn new(
        category: ErrorCategory,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            category,
            code: code.into(),
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    pub fn validation(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Validation, code, message)
    }

    pub fn auth(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Auth, code, message)
    }

    pub fn config(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Config, code, message)
    }

    pub fn api(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Api, code, message)
    }

    pub fn network(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Network, code, message)
    }

    pub fn rate_limit(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::RateLimit, code, message)
    }

    pub fn tx_failed(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::TxFailed, code, message)
    }

    pub fn io(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Io, code, message)
    }

    pub fn internal(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Internal, code, message)
    }

    pub fn exit_code(&self) -> i32 {
        self.category.exit_code()
    }

    /// Return a short recovery hint for agents based on the error code.
    pub fn recovery_hint(&self) -> &'static str {
        match self.code.as_str() {
            "CONFIRMATION_REQUIRED" => "Add --yes flag to confirm, or --dry-run to simulate",
            "NOT_IMPLEMENTED" => "This feature is not yet available",
            "NO_DEFAULT_WALLET" => "Run: vulcan wallet set-default <NAME>",
            "WALLET_NOT_FOUND" => "Run 'vulcan wallet list' to see available wallets",
            "DECRYPT_FAILED" => "Wrong password. Check VULCAN_WALLET_PASSWORD env var",
            "NO_TRADER_ACCOUNT" => "Register first: vulcan account register --yes",
            "INSUFFICIENT_SOL_FOR_REGISTRATION" => {
                "Fund the wallet with SOL for cross-margin trader account rent, then retry registration"
            }
            "REGISTRATION_SIMULATION_FAILED" => {
                "Check the simulation logs, payer SOL balance, and account state before retrying registration"
            }
            "REGISTER_API_FAILED" => "Check registration code and API URL. Run 'vulcan status' to verify",
            "TX_SEND_FAILED" => "Check wallet SOL balance and account state",
            "CONFIG_ERROR" | "CONFIG_LOAD_FAILED" | "INIT_FAILED" => {
                "Run 'vulcan setup' to configure"
            }
            "PASSWORD_READ_FAILED" => "Set VULCAN_WALLET_PASSWORD env var for non-interactive use",
            "UNKNOWN_MARKET" => "Run 'vulcan market list' to see available markets",
            "MISSING_ARG" => "Check tool schema for required fields",
            "UNKNOWN_TOOL" => "Run MCP tools/list to see available tools",
            "BLOCKHASH_FAILED"
            | "RPC_BALANCE_FAILED"
            | "REGISTRATION_RENT_FETCH_FAILED"
            | "REGISTRATION_SIMULATION_RPC_FAILED" => "Check rpc_url in config",
            "EXCHANGE_FETCH_FAILED" | "TRADERS_FETCH_FAILED" | "MARKETS_FETCH_FAILED" => {
                "Run 'vulcan status' to check API connectivity"
            }
            "UPDATE_CHECK_FAILED" => "Network error while checking GitHub Releases — try again later",
            "UPDATE_RATE_LIMITED" => {
                "GitHub API rate limit exceeded — wait an hour and retry, or run 'vulcan update check --force' less often"
            }
            _ if self.category.is_retryable() => "Transient error — safe to retry",
            _ => "",
        }
    }
}

impl fmt::Display for VulcanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.category, self.code, self.message)
    }
}

impl std::error::Error for VulcanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}
