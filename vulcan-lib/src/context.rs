//! Application context — shared state across commands.

use crate::agent_log::{default_log_path, new_session_id, AgentLogSink};
use crate::config::VulcanConfig;
use crate::mcp::session_wallet::SessionWallet;
use crate::output::OutputFormat;
use crate::wallet::WalletStore;
use anyhow::Result;
use phoenix_rise::math::{
    BaseLots, BasisPoints, Constant, LeverageTier, LeverageTiers, PerpAssetMetadata,
    QuoteLotsPerBaseLotPerTick, SignedQuoteLotsPerBaseLot, Ticks,
};
use phoenix_rise::accounts::perp_asset_map::{
    LeverageTier as AccountLeverageTier, PerpAssetMap,
    PerpAssetMetadata as AccountPerpAssetMetadata,
};
use phoenix_rise::api::{PhoenixHttpClient, PhoenixMetadata};
use phoenix_rise::core::PhoenixTxBuilder;
use phoenix_rise::types::prelude::ExchangeSnapshotView;
use serde::{Deserialize, Serialize};
use solana_commitment_config::CommitmentConfig;
use solana_pubkey::Pubkey;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::OnceCell;

const EXCHANGE_SNAPSHOT_CACHE_TTL_SECS: u64 = 6 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedExchangeSnapshot {
    api_url: String,
    fetched_at_unix: u64,
    snapshot: ExchangeSnapshotView,
}

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
                let exchange = self.exchange_snapshot().await?;
                let view: phoenix_rise::api::ExchangeView =
                    exchange.into_exchange_response().into();
                Ok(PhoenixMetadata::new(view))
            })
            .await
    }

    /// Get the exchange snapshot using a persistent local cache.
    pub async fn exchange_snapshot(
        &self,
    ) -> Result<ExchangeSnapshotView, crate::error::VulcanError> {
        if let Some(snapshot) =
            load_cached_exchange_snapshot(&self.vulcan_dir, &self.config.network.api_url, false)?
        {
            return Ok(snapshot);
        }

        match self.http_client.get_exchange_snapshot().await {
            Ok(snapshot) => {
                store_cached_exchange_snapshot(
                    &self.vulcan_dir,
                    &self.config.network.api_url,
                    &snapshot,
                )?;
                Ok(snapshot)
            }
            Err(err) => {
                if let Some(snapshot) = load_cached_exchange_snapshot(
                    &self.vulcan_dir,
                    &self.config.network.api_url,
                    true,
                )? {
                    return Ok(snapshot);
                }
                Err(crate::error::VulcanError::api(
                    "EXCHANGE_SNAPSHOT_FETCH_FAILED",
                    err.to_string(),
                ))
            }
        }
    }

    /// Hydrate SDK margin metadata from the on-chain PerpAssetMap.
    ///
    /// The exchange snapshot carries ix-building keys/configs. PerpAssetMap
    /// carries live mark prices and on-chain risk params, so this is a robust
    /// fallback when API mark-price hydration is unavailable.
    pub(crate) async fn hydrate_metadata_from_perp_asset_map_rpc(
        &self,
        metadata: &mut PhoenixMetadata,
    ) -> Result<(), crate::error::VulcanError> {
        let perp_asset_map = Pubkey::from_str(&metadata.keys().perp_asset_map).map_err(|e| {
            crate::error::VulcanError::validation("INVALID_PERP_ASSET_MAP", e.to_string())
        })?;
        let account = self
            .rpc_client_async()
            .get_account(&perp_asset_map)
            .await
            .map_err(|e| {
                crate::error::VulcanError::api("PERP_ASSET_MAP_FETCH_FAILED", e.to_string())
            })?;
        let perp_asset_map = PerpAssetMap::try_from_account_bytes(&account.data).map_err(|e| {
            crate::error::VulcanError::api("PERP_ASSET_MAP_DECODE_FAILED", e.to_string())
        })?;

        for entry in perp_asset_map.iter() {
            let entry = entry.map_err(|e| {
                crate::error::VulcanError::api("PERP_ASSET_MAP_DECODE_FAILED", e.to_string())
            })?;
            if !entry.metadata.is_active() {
                continue;
            }
            let symbol = entry.symbol.as_str();
            let perp_metadata = perp_asset_metadata_from_account(symbol, &entry.metadata)?;
            metadata
                .all_perp_asset_metadata_mut()
                .insert(symbol.to_ascii_uppercase(), perp_metadata);
        }

        Ok(())
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
            CommitmentConfig::confirmed(),
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
            CommitmentConfig::confirmed(),
        )
    }
}

fn exchange_cache_dir(vulcan_dir: &Path) -> PathBuf {
    vulcan_dir.join("cache")
}

fn exchange_snapshot_cache_path(vulcan_dir: &Path) -> PathBuf {
    exchange_cache_dir(vulcan_dir).join("exchange-snapshot.json")
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn load_cached_exchange_snapshot(
    vulcan_dir: &Path,
    api_url: &str,
    allow_stale: bool,
) -> Result<Option<ExchangeSnapshotView>, crate::error::VulcanError> {
    let path = exchange_snapshot_cache_path(vulcan_dir);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let Ok(cached) = serde_json::from_str::<CachedExchangeSnapshot>(&content) else {
        return Ok(None);
    };
    if cached.api_url != api_url {
        return Ok(None);
    }
    let age = now_unix().saturating_sub(cached.fetched_at_unix);
    if !allow_stale && age > EXCHANGE_SNAPSHOT_CACHE_TTL_SECS {
        return Ok(None);
    }
    Ok(Some(cached.snapshot))
}

fn store_cached_exchange_snapshot(
    vulcan_dir: &Path,
    api_url: &str,
    snapshot: &ExchangeSnapshotView,
) -> Result<(), crate::error::VulcanError> {
    let dir = exchange_cache_dir(vulcan_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| crate::error::VulcanError::config("CACHE_WRITE_FAILED", e.to_string()))?;
    let payload = serde_json::to_string_pretty(&CachedExchangeSnapshot {
        api_url: api_url.to_string(),
        fetched_at_unix: now_unix(),
        snapshot: snapshot.clone(),
    })
    .map_err(|e| crate::error::VulcanError::config("CACHE_WRITE_FAILED", e.to_string()))?;
    std::fs::write(exchange_snapshot_cache_path(vulcan_dir), payload)
        .map_err(|e| crate::error::VulcanError::config("CACHE_WRITE_FAILED", e.to_string()))
}

fn perp_asset_metadata_from_account(
    symbol: &str,
    account_metadata: &AccountPerpAssetMetadata,
) -> Result<PerpAssetMetadata, crate::error::VulcanError> {
    let static_params = account_metadata.static_market_params();
    let risk_params = account_metadata.risk_params();
    let mark_price_ticks = Ticks::new_checked(account_metadata.oracle_price().mark_price.price.ticks)
        .map_err(|e| crate::error::VulcanError::api("PERP_ASSET_MAP_INVALID", e.to_string()))?;
    let leverage_tiers = leverage_tiers_from_account(&risk_params.leverage_tiers)?;
    let risk_factors = risk_factors_from_account(&risk_params.risk_factors)?;
    let mut metadata = PerpAssetMetadata::new(
        symbol.to_ascii_uppercase(),
        static_params.asset_id() as u64,
        static_params.base_lot_decimals,
        mark_price_ticks,
        QuoteLotsPerBaseLotPerTick::new(static_params.tick_size),
        leverage_tiers,
        risk_factors,
        risk_params.cancel_order_risk_factor,
        u16_checked(risk_params.upnl_risk_factor, "upnl_risk_factor")?,
        u16_checked(
            risk_params.upnl_risk_factor_for_withdrawals,
            "upnl_risk_factor_for_withdrawals",
        )?,
    );
    metadata.cumulative_funding_rate = SignedQuoteLotsPerBaseLot::new(
        account_metadata.funding_accumulator().cumulative_funding_rate,
    );
    Ok(metadata)
}

fn leverage_tiers_from_account(
    tiers: &[AccountLeverageTier],
) -> Result<LeverageTiers, crate::error::VulcanError> {
    if tiers.len() != 4 {
        return Err(crate::error::VulcanError::api(
            "PERP_ASSET_MAP_INVALID",
            format!("Expected 4 leverage tiers, got {}", tiers.len()),
        ));
    }
    let convert = |tier: &AccountLeverageTier| -> Result<LeverageTier, crate::error::VulcanError> {
        Ok(LeverageTier {
            upper_bound_size: BaseLots::new(tier.upper_bound_size),
            max_leverage: Constant::new(tier.max_leverage),
            limit_order_risk_factor: BasisPoints::new(u16_checked(
                tier.limit_order_risk_factor,
                "leverage_tier.limit_order_risk_factor",
            )? as u64),
        })
    };
    LeverageTiers::new([
        convert(&tiers[0])?,
        convert(&tiers[1])?,
        convert(&tiers[2])?,
        convert(&tiers[3])?,
    ])
    .map_err(|e| crate::error::VulcanError::api("PERP_ASSET_MAP_INVALID", e))
}

fn risk_factors_from_account(risk_factors: &[u16]) -> Result<[u16; 3], crate::error::VulcanError> {
    if risk_factors.len() < 3 {
        return Err(crate::error::VulcanError::api(
            "PERP_ASSET_MAP_INVALID",
            format!(
                "Expected at least 3 risk factors, got {}",
                risk_factors.len()
            ),
        ));
    }
    Ok([risk_factors[0], risk_factors[1], risk_factors[2]])
}

fn u16_checked(value: u64, field: &str) -> Result<u16, crate::error::VulcanError> {
    u16::try_from(value).map_err(|_| {
        crate::error::VulcanError::api(
            "PERP_ASSET_MAP_INVALID",
            format!("{field} does not fit in u16: {value}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoenix_rise::types::prelude::{AuthoritySet, ExchangeStateSnapshot};

    fn snapshot_fixture(slot: u64) -> ExchangeSnapshotView {
        ExchangeSnapshotView {
            version: 1,
            sequence_number: None,
            slot,
            slot_index: 0,
            exchange: ExchangeStateSnapshot {
                program_id: "program".to_string(),
                global_config: "global-config".to_string(),
                current_authorities: AuthoritySet {
                    root_authority: "root".to_string(),
                    risk_authority: "risk".to_string(),
                    market_authority: "market".to_string(),
                    oracle_authority: "oracle".to_string(),
                    adl_authority: "adl".to_string(),
                    cancel_authority: "cancel".to_string(),
                    backstop_authority: "backstop".to_string(),
                },
                canonical_mint: "canonical".to_string(),
                usdc_mint: "usdc".to_string(),
                global_vault: "vault".to_string(),
                perp_asset_map: "perp-map".to_string(),
                global_trader_index: vec!["gti-0".to_string()],
                active_trader_buffer: vec!["atb-0".to_string()],
                withdraw_queue: "withdraw-queue".to_string(),
                withdrawals_available: true,
                exchange_status_bits: 0,
                exchange_status_features: Vec::new(),
                active: true,
                gated: false,
            },
            markets: Vec::new(),
        }
    }

    #[test]
    fn exchange_snapshot_cache_round_trips_fresh_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let snapshot = snapshot_fixture(42);

        store_cached_exchange_snapshot(tmp.path(), "https://api.example", &snapshot).unwrap();
        let loaded =
            load_cached_exchange_snapshot(tmp.path(), "https://api.example", false).unwrap();

        assert_eq!(loaded.unwrap().slot, 42);
    }

    #[test]
    fn exchange_snapshot_cache_rejects_stale_unless_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let payload = CachedExchangeSnapshot {
            api_url: "https://api.example".to_string(),
            fetched_at_unix: 1,
            snapshot: snapshot_fixture(7),
        };
        std::fs::create_dir_all(exchange_cache_dir(tmp.path())).unwrap();
        std::fs::write(
            exchange_snapshot_cache_path(tmp.path()),
            serde_json::to_string(&payload).unwrap(),
        )
        .unwrap();

        assert!(
            load_cached_exchange_snapshot(tmp.path(), "https://api.example", false)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            load_cached_exchange_snapshot(tmp.path(), "https://api.example", true)
                .unwrap()
                .unwrap()
                .slot,
            7
        );
    }

    #[test]
    fn exchange_snapshot_cache_ignores_invalid_json() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(exchange_cache_dir(tmp.path())).unwrap();
        std::fs::write(exchange_snapshot_cache_path(tmp.path()), "not-json").unwrap();

        assert!(
            load_cached_exchange_snapshot(tmp.path(), "https://api.example", true)
                .unwrap()
                .is_none()
        );
    }
}
