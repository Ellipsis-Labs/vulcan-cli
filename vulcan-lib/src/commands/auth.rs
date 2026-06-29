//! Phoenix API auth command execution.

use crate::auth::{self, ApiAuthMetadata};
use crate::cli::auth::AuthCommand;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use crate::wallet::{ResolvedSigner, Wallet};
use chrono::Utc;
use phoenix_rise::api::{PhoenixHttpAuthConfig, PhoenixHttpClientBuilder};
use serde::Serialize;
use std::io::{self, BufRead, Write};

#[derive(Debug, Clone, Serialize)]
pub struct ApiAuthStatus {
    pub configured: bool,
    pub kind: String,
    pub wallet: Option<String>,
    pub wallet_name: Option<String>,
    pub session_path: String,
    pub expires_at: Option<u64>,
    pub refresh_expires_at: Option<u64>,
    pub refreshable: bool,
    pub valid: bool,
    pub reauth_required: bool,
    pub load_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ApiAuthLoginResult {
    pub status: ApiAuthStatus,
    pub note: String,
}

#[derive(Debug, Serialize)]
pub struct ApiAuthLogoutResult {
    pub cleared: bool,
    pub session_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiAuthAutoLoginResult {
    pub attempted: bool,
    pub succeeded: bool,
    pub skipped_reason: Option<String>,
    pub error: Option<String>,
    pub status: ApiAuthStatus,
}

impl TableRenderable for ApiAuthStatus {
    fn render_table(&self) {
        println!("Phoenix API Auth");
        println!("─────────────────────────────────────────");
        println!("  Configured: {}", self.configured);
        println!("  Kind:       {}", self.kind);
        if let Some(wallet) = &self.wallet {
            println!("  Wallet:     {}", wallet);
        }
        if let Some(name) = &self.wallet_name {
            println!("  Name:       {}", name);
        }
        println!("  Valid:      {}", self.valid);
        println!("  Refreshable: {}", self.refreshable);
        println!("  Reauth:     {}", self.reauth_required);
        if let Some(expires_at) = self.expires_at {
            println!("  Expires:    {}", expires_at);
        }
        if let Some(err) = &self.load_error {
            println!("  Error:      {}", err);
        }
        println!("  Path:       {}", self.session_path);
    }
}

impl TableRenderable for ApiAuthLoginResult {
    fn render_table(&self) {
        println!("Phoenix API login complete.");
        self.status.render_table();
        println!("  Note:       {}", self.note);
    }
}

impl TableRenderable for ApiAuthLogoutResult {
    fn render_table(&self) {
        println!("Phoenix API auth session cleared.");
        println!("  Path: {}", self.session_path);
    }
}

pub async fn execute(ctx: &AppContext, cmd: AuthCommand) -> Result<(), VulcanError> {
    match cmd {
        AuthCommand::Login => {
            let result = login_selected_wallet(ctx).await?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        AuthCommand::Status => {
            let result = status(ctx);
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        AuthCommand::Logout => {
            let result = logout(ctx)?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
    }
    Ok(())
}

pub async fn login_default_wallet(ctx: &AppContext) -> Result<ApiAuthLoginResult, VulcanError> {
    let (wallet, _, _) = crate::commands::trade::resolve_wallet_and_pda(ctx, None).await?;
    login_with_signer(ctx, &wallet, Some(wallet.wallet_name.clone())).await
}

pub async fn login_selected_wallet(ctx: &AppContext) -> Result<ApiAuthLoginResult, VulcanError> {
    let wallet_name = select_wallet_name(ctx)?;
    let (wallet, _, _) =
        crate::commands::trade::resolve_wallet_and_pda(ctx, Some(&wallet_name)).await?;
    login_with_signer(ctx, &wallet, Some(wallet_name)).await
}

pub async fn auto_login_if_possible(ctx: &AppContext) -> ApiAuthAutoLoginResult {
    let current = status(ctx);
    if !current.reauth_required {
        return ApiAuthAutoLoginResult {
            attempted: false,
            succeeded: false,
            skipped_reason: Some("api_auth_already_valid_or_refreshable".to_string()),
            error: None,
            status: current,
        };
    }

    let login_result = if let Some(session_wallet) = &ctx.session_wallet {
        let wallet = session_wallet.resolved_signer();
        login_with_signer(ctx, &wallet, Some(session_wallet.wallet_name.clone())).await
    } else if std::env::var("VULCAN_WALLET_PASSWORD").is_ok() {
        login_selected_wallet(ctx).await
    } else {
        return ApiAuthAutoLoginResult {
            attempted: false,
            succeeded: false,
            skipped_reason: Some("wallet_not_unlocked_noninteractively".to_string()),
            error: None,
            status: current,
        };
    };

    match login_result {
        Ok(result) => ApiAuthAutoLoginResult {
            attempted: true,
            succeeded: true,
            skipped_reason: None,
            error: None,
            status: result.status,
        },
        Err(err) => ApiAuthAutoLoginResult {
            attempted: true,
            succeeded: false,
            skipped_reason: None,
            error: Some(format!("{}: {}", err.code, err.message)),
            status: status(ctx),
        },
    }
}

fn select_wallet_name(ctx: &AppContext) -> Result<String, VulcanError> {
    if let Some(name) = std::env::var("VULCAN_WALLET_NAME")
        .ok()
        .filter(|name| !name.trim().is_empty() && name != "your-wallet")
    {
        if ctx.wallet_store.exists(&name) {
            return Ok(name);
        }
        return Err(VulcanError::auth(
            "WALLET_NOT_FOUND",
            format!(
                "VULCAN_WALLET_NAME '{}' does not match a stored wallet",
                name
            ),
        ));
    }

    let wallets = ctx
        .wallet_store
        .list()
        .map_err(|e| VulcanError::config("WALLET_LIST_FAILED", e.to_string()))?;
    if wallets.is_empty() {
        return Err(VulcanError::config(
            "NO_WALLETS",
            "No wallets found. Create or import a wallet before Phoenix API login.",
        ));
    }
    let default_wallet = ctx
        .wallet_store
        .default_wallet()
        .map_err(|e| VulcanError::config("WALLET_DEFAULT_FAILED", e.to_string()))?;

    use std::io::IsTerminal;
    if !io::stdin().is_terminal() {
        return default_wallet.ok_or_else(|| {
            VulcanError::auth(
                "WALLET_SELECTION_REQUIRED",
                "Phoenix API login needs a wallet selection. Set VULCAN_WALLET_NAME for non-interactive login or run from an interactive terminal.",
            )
        });
    }

    println!("Select wallet for Phoenix API login:");
    for (idx, wallet) in wallets.iter().enumerate() {
        let default_marker = if Some(wallet.as_str()) == default_wallet.as_deref() {
            " (default)"
        } else {
            ""
        };
        println!("  {}) {}{}", idx + 1, wallet, default_marker);
    }
    let default_idx = default_wallet
        .as_deref()
        .and_then(|default| wallets.iter().position(|wallet| wallet == default))
        .unwrap_or(0);
    print!("Choice [{}]: ", default_idx + 1);
    io::stdout()
        .flush()
        .map_err(|e| VulcanError::io("FLUSH_FAILED", e.to_string()))?;
    let mut input = String::new();
    io::stdin()
        .lock()
        .read_line(&mut input)
        .map_err(|e| VulcanError::io("READ_FAILED", e.to_string()))?;
    let trimmed = input.trim();
    let idx = if trimmed.is_empty() {
        default_idx
    } else {
        trimmed
            .parse::<usize>()
            .map_err(|_| {
                VulcanError::validation(
                    "INVALID_WALLET_SELECTION",
                    "Wallet choice must be a number",
                )
            })?
            .checked_sub(1)
            .ok_or_else(|| {
                VulcanError::validation(
                    "INVALID_WALLET_SELECTION",
                    "Wallet choice must be at least 1",
                )
            })?
    };
    wallets.get(idx).cloned().ok_or_else(|| {
        VulcanError::validation("INVALID_WALLET_SELECTION", "Wallet choice is out of range")
    })
}

pub async fn login_with_wallet(
    ctx: &AppContext,
    wallet: &Wallet,
    wallet_name: Option<String>,
) -> Result<ApiAuthLoginResult, VulcanError> {
    let signer = ResolvedSigner::from_unlocked_wallet(
        wallet_name
            .clone()
            .unwrap_or_else(|| wallet.address().to_string()),
        wallet,
    )?;
    login_with_signer(ctx, &signer, wallet_name).await
}

pub async fn login_with_signer(
    ctx: &AppContext,
    wallet: &ResolvedSigner,
    wallet_name: Option<String>,
) -> Result<ApiAuthLoginResult, VulcanError> {
    let auth_config = PhoenixHttpAuthConfig::new()
        .with_file_session_store_path(auth::session_path(&ctx.vulcan_dir))
        .map_err(|e| VulcanError::auth("API_AUTH_SESSION_INIT_FAILED", e.to_string()))?;
    let client = PhoenixHttpClientBuilder::new(&ctx.config.network.api_url)
        .with_auth(auth_config)
        .build()
        .map_err(|e| VulcanError::api("API_AUTH_CLIENT_INIT_FAILED", e.to_string()))?;
    let auth_client = client.auth().ok_or_else(|| {
        VulcanError::internal(
            "API_AUTH_CLIENT_MISSING",
            "authenticated client was not created",
        )
    })?;
    let nonce = auth_client
        .get_wallet_nonce(&wallet.public_key)
        .await
        .map_err(|e| VulcanError::api("API_AUTH_NONCE_FAILED", e.to_string()))?;

    let signer = wallet.signer()?;
    let signature = signer
        .sign_message(nonce.message.as_bytes())
        .await
        .map_err(|e| VulcanError::auth("API_AUTH_SIGN_FAILED", e.to_string()))?;
    auth_client
        .login_with_wallet_signature(
            &wallet.public_key,
            signature.as_ref().to_vec(),
            nonce.nonce_id,
        )
        .await
        .map_err(|e| VulcanError::auth("API_AUTH_LOGIN_FAILED", e.to_string()))?;
    auth::store_metadata(
        &ctx.vulcan_dir,
        &ApiAuthMetadata {
            wallet: Some(wallet.public_key.clone()),
            wallet_name,
            logged_in_at: Some(Utc::now().to_rfc3339()),
        },
    )?;
    Ok(ApiAuthLoginResult {
        status: status(ctx),
        note: "Stored session will be loaded by current and future CLI/MCP API clients through the persistent session store.".to_string(),
    })
}

pub fn logout(ctx: &AppContext) -> Result<ApiAuthLogoutResult, VulcanError> {
    let path = auth::session_path(&ctx.vulcan_dir);
    auth::clear_session(&ctx.vulcan_dir)?;
    Ok(ApiAuthLogoutResult {
        cleared: true,
        session_path: path.to_string_lossy().to_string(),
    })
}

pub fn status(ctx: &AppContext) -> ApiAuthStatus {
    status_for_dir(&ctx.vulcan_dir, ctx.api_auth_error.clone())
}

pub fn status_for_dir(vulcan_dir: &std::path::Path, load_error: Option<String>) -> ApiAuthStatus {
    let path = auth::session_path(vulcan_dir);
    let metadata = auth::load_metadata(vulcan_dir);
    let session = auth::load_session_snapshot(vulcan_dir).ok().flatten();
    let now = Utc::now().timestamp().max(0) as u64;
    let valid = session
        .as_ref()
        .and_then(|snapshot| snapshot.access_expires_at)
        .map(|expires| expires > now)
        .unwrap_or(false);
    let refreshable = session
        .as_ref()
        .map(|snapshot| {
            snapshot.refresh_token.is_some()
                && snapshot
                    .refresh_expires_at
                    .map(|expires| expires > now)
                    .unwrap_or(true)
        })
        .unwrap_or(false);
    ApiAuthStatus {
        configured: session.is_some(),
        kind: if session.is_some() {
            "wallet_session".to_string()
        } else {
            "none".to_string()
        },
        wallet: metadata
            .as_ref()
            .and_then(|metadata| metadata.wallet.clone()),
        wallet_name: metadata.and_then(|metadata| metadata.wallet_name),
        session_path: path.to_string_lossy().to_string(),
        expires_at: session
            .as_ref()
            .and_then(|snapshot| snapshot.access_expires_at),
        refresh_expires_at: session
            .as_ref()
            .and_then(|snapshot| snapshot.refresh_expires_at),
        refreshable,
        valid,
        reauth_required: session.is_none() || (!valid && !refreshable),
        load_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_without_session_requires_reauth() {
        let tmp = tempfile::tempdir().unwrap();
        let status = status_for_dir(tmp.path(), None);

        assert!(!status.configured);
        assert_eq!(status.kind, "none");
        assert!(status.reauth_required);
        assert!(!status.valid);
        assert!(!status.refreshable);
    }

    #[test]
    fn metadata_is_loaded_without_exposing_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        crate::auth::store_metadata(
            tmp.path(),
            &ApiAuthMetadata {
                wallet: Some("wallet-address".to_string()),
                wallet_name: Some("main".to_string()),
                logged_in_at: Some("now".to_string()),
            },
        )
        .unwrap();

        let status = status_for_dir(tmp.path(), None);

        assert_eq!(status.wallet.as_deref(), Some("wallet-address"));
        assert_eq!(status.wallet_name.as_deref(), Some("main"));
        assert!(!serde_json::to_string(&status).unwrap().contains("token"));
    }
}
