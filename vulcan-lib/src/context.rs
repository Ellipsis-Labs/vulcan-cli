//! Application context — shared state across commands.

use crate::agent_log::{default_log_path, new_session_id, AgentLogSink};
use crate::config::VulcanConfig;
use crate::mcp::session_wallet::SessionWallet;
use crate::output::OutputFormat;
use crate::wallet::WalletStore;
use anyhow::Result;
use phoenix_rise::types::PhoenixMetadata;
use phoenix_rise::{PhoenixHttpClient, PhoenixTxBuilder};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::OnceCell;

/// Shared application context available to all commands.
pub struct AppContext {
    pub config: VulcanConfig,
    pub wallet_store: WalletStore,
    pub output_format: OutputFormat,
    pub dry_run: bool,
    pub yes: bool,
    pub verbose: bool,
    pub watch: bool,
    pub vulcan_dir: PathBuf,
    pub http_client: PhoenixHttpClient,
    pub raw_http_client: reqwest::Client,
    /// Stable identifier for this CLI invocation or MCP server process.
    pub session_id: String,
    /// Local append-only action log used by agents for session summaries.
    pub agent_log: Option<Arc<AgentLogSink>>,
    /// Pre-decrypted session wallet for MCP mode (None in CLI mode).
    pub session_wallet: Option<Arc<SessionWallet>>,
    /// Non-fatal API auth session load error. HTTP falls back to public mode.
    pub api_auth_error: Option<String>,
    /// Global CLI `--wallet` name: use this stored wallet instead of the default when set.
    /// Ignored when `session_wallet` is present (MCP). Not used by `vulcan mcp` (pass `None`).
    pub wallet_override: Option<String>,
    /// Lazily-initialized metadata (fetched on first use).
    metadata: OnceCell<PhoenixMetadata>,
}

impl Clone for AppContext {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            wallet_store: self.wallet_store.clone(),
            output_format: self.output_format,
            dry_run: self.dry_run,
            yes: self.yes,
            verbose: self.verbose,
            watch: self.watch,
            vulcan_dir: self.vulcan_dir.clone(),
            http_client: self.http_client.clone(),
            raw_http_client: self.raw_http_client.clone(),
            session_id: self.session_id.clone(),
            agent_log: self.agent_log.clone(),
            session_wallet: self.session_wallet.clone(),
            api_auth_error: self.api_auth_error.clone(),
            wallet_override: self.wallet_override.clone(),
            metadata: OnceCell::new(),
        }
    }
}

impl AppContext {
    /// Build an AppContext from global CLI flags and config.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        output_format: OutputFormat,
        dry_run: bool,
        yes: bool,
        verbose: bool,
        watch: bool,
        rpc_url: Option<String>,
        api_url: Option<String>,
        wallet_override: Option<String>,
    ) -> Result<Self> {
        let mut config = VulcanConfig::load()?;

        // CLI flags override config
        if let Some(rpc) = rpc_url {
            config.network.rpc_url = rpc;
        }
        if let Some(api) = api_url {
            config.network.api_url = api;
        }
        let wallet_override = wallet_override
            .filter(|w| !w.trim().is_empty())
            .map(|w| w.trim().to_string());
        let vulcan_dir = VulcanConfig::dir();
        std::fs::create_dir_all(&vulcan_dir)?;

        let wallet_store = WalletStore::new(&vulcan_dir)?;
        let session_id = new_session_id();
        let agent_log = if config.agent_log.enabled {
            let path = config
                .agent_log
                .path
                .clone()
                .unwrap_or_else(|| default_log_path(&vulcan_dir));
            Some(Arc::new(AgentLogSink::new(
                path,
                session_id.clone(),
                verbose,
                config.agent_log.capture_position_snapshots,
                config.agent_log.retain_sessions,
                crate::agent_log::mb_to_bytes(config.agent_log.max_file_mb),
                config.agent_log.archive_session_summaries,
            )))
        } else {
            None
        };

        // Build HTTP client from config and an optional local wallet-signed API session.
        let (http_client, api_auth_error) = crate::auth::build_http_client(&config, &vulcan_dir)?;
        let raw_http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        Ok(Self {
            config,
            wallet_store,
            output_format,
            dry_run,
            yes,
            verbose,
            watch,
            vulcan_dir,
            http_client,
            raw_http_client,
            session_id,
            agent_log,
            session_wallet: None,
            api_auth_error,
            wallet_override,
            metadata: OnceCell::new(),
        })
    }

    /// Rebuild the Phoenix HTTP client after auth session changes.
    pub fn reload_http_client(&mut self) -> Result<()> {
        let (http_client, api_auth_error) =
            crate::auth::build_http_client(&self.config, &self.vulcan_dir)?;
        self.http_client = http_client;
        self.api_auth_error = api_auth_error;
        self.metadata = OnceCell::new();
        Ok(())
    }

    /// Get exchange metadata, fetching it lazily on first call.
    pub async fn metadata(&self) -> Result<&PhoenixMetadata, crate::error::VulcanError> {
        self.metadata
            .get_or_try_init(|| async {
                let exchange = self.http_client.get_exchange().await.map_err(|e| {
                    crate::error::VulcanError::api("EXCHANGE_FETCH_FAILED", e.to_string())
                })?;
                let view: phoenix_rise::types::ExchangeView = exchange.into();
                Ok(PhoenixMetadata::new(view))
            })
            .await
    }

    /// Create a transaction builder from cached metadata.
    pub async fn tx_builder(&self) -> Result<PhoenixTxBuilder<'_>, crate::error::VulcanError> {
        let metadata = self.metadata().await?;
        Ok(PhoenixTxBuilder::new(metadata))
    }

    /// Build a blocking Solana RPC client at `confirmed` commitment.
    /// `confirmed` is the right default for trading: ~1-2s latency, effectively
    /// irreversible. `finalized` (~12s) is overkill for tx confirmation.
    pub fn rpc_client(&self) -> solana_rpc_client::rpc_client::RpcClient {
        solana_rpc_client::rpc_client::RpcClient::new_with_commitment(
            self.config.network.rpc_url.clone(),
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
        )
    }

    /// Resolve which stored-wallet name to use for read-only commands.
    /// Precedence: explicit per-call override > global `--wallet` flag (`wallet_override`)
    /// > default wallet. Errors if no wallet can be determined.
    pub fn resolved_wallet_name(
        &self,
        explicit: Option<&str>,
    ) -> Result<String, crate::error::VulcanError> {
        let chosen = explicit
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| self.wallet_override.clone());
        if let Some(name) = chosen {
            if !self.wallet_store.exists(&name) {
                return Err(crate::error::VulcanError::auth(
                    "WALLET_NOT_FOUND",
                    format!(
                        "Wallet '{}' not found. Run `vulcan wallet list` to see stored wallets.",
                        name
                    ),
                ));
            }
            return Ok(name);
        }
        self.wallet_store
            .default_wallet()
            .map_err(|e| crate::error::VulcanError::config("CONFIG_ERROR", e.to_string()))?
            .ok_or_else(|| {
                crate::error::VulcanError::config(
                    "NO_DEFAULT_WALLET",
                    "No wallet selected. Pass `-w <name>`, set VULCAN_WALLET_NAME, or run `vulcan wallet set-default <NAME>`.",
                )
            })
    }

    /// Async counterpart of [`rpc_client`].
    pub fn rpc_client_async(&self) -> solana_rpc_client::nonblocking::rpc_client::RpcClient {
        solana_rpc_client::nonblocking::rpc_client::RpcClient::new_with_commitment(
            self.config.network.rpc_url.clone(),
            solana_sdk::commitment_config::CommitmentConfig::confirmed(),
        )
    }
}
