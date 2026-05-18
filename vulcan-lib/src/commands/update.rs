//! Update detection — read-only check against the GitHub Releases API.
//!
//! Compares the running binary's `CARGO_PKG_VERSION` to the latest published release
//! on `Ellipsis-Labs/vulcan-cli` and returns a structured result with an
//! install-path-aware update hint. Detection only: this module never writes to the
//! Vulcan binary or invokes the install script — it tells the agent/user how to
//! update, and they decide.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_REPO: &str = "Ellipsis-Labs/vulcan-cli";
const CACHE_FILE: &str = "update-cache.json";
const CACHE_TTL_SECS: u64 = 6 * 60 * 60;
const HTTP_TIMEOUT_SECS: u64 = 5;
const RELEASES_PAGE_URL: &str = "https://github.com/Ellipsis-Labs/vulcan-cli/releases";
const INSTALL_SH_URL: &str =
    "https://github.com/Ellipsis-Labs/vulcan-cli/releases/latest/download/install.sh";
const RESTART_HINT: &str = "If a vulcan MCP server is running (e.g. inside Claude Code), \
restart that session afterward — the live process keeps using the old binary in memory \
until it restarts.";

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateMethod {
    InstallScriptDefault,
    InstallScriptCustomDir,
    PackageManager,
}

#[derive(Debug, Serialize, Clone)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_url: String,
    pub install_path: String,
    pub update_method: UpdateMethod,
    pub update_command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_hint: Option<String>,
    pub checked_at: String,
    pub from_cache: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_error: Option<String>,
}

impl TableRenderable for UpdateCheckResult {
    fn render_table(&self) {
        println!("Vulcan Update Check");
        println!("─────────────────────────────────────────");
        println!("  Installed:        {}", self.current_version);
        println!("  Latest published: {}", self.latest_version);
        println!(
            "  Update available: {}",
            if self.update_available { "yes" } else { "no" }
        );
        println!("  Install path:     {}", self.install_path);
        println!(
            "  Source:           {}",
            if self.from_cache { "cache" } else { "github" }
        );
        if let Some(err) = &self.check_error {
            println!("  Check error:      {}", err);
        }
        if self.update_available {
            println!();
            println!("  Release notes: {}", self.release_url);
            println!();
            println!("  {}", self.update_command);
            if let Some(hint) = &self.restart_hint {
                println!("  {}", hint);
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct UpdateCache {
    checked_at_unix: u64,
    latest_version: String,
    release_url: String,
}

impl UpdateCache {
    fn path(vulcan_dir: &Path) -> PathBuf {
        vulcan_dir.join(CACHE_FILE)
    }

    fn load(vulcan_dir: &Path) -> Option<Self> {
        let bytes = std::fs::read(Self::path(vulcan_dir)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn is_fresh(&self) -> bool {
        let now = now_unix();
        now.saturating_sub(self.checked_at_unix) < CACHE_TTL_SECS
    }

    fn save(&self, vulcan_dir: &Path) -> std::io::Result<()> {
        let path = Self::path(vulcan_dir);
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: Option<String>,
}

pub async fn execute(ctx: &AppContext, force: bool) -> Result<(), VulcanError> {
    let result = execute_check_inner(ctx, force).await?;
    render_success(ctx.output_format, &result, serde_json::Value::Null);
    Ok(())
}

/// Run an update check, returning a structured result. Failures with no usable
/// cache bubble up as `VulcanError`; callers that need a "never fail" path for
/// health views should use [`execute_check_for_health`] instead.
pub async fn execute_check_inner(
    ctx: &AppContext,
    force_refresh: bool,
) -> Result<UpdateCheckResult, VulcanError> {
    let install_path = resolve_install_path();
    let update_method = classify_install(&install_path);

    let cache = if force_refresh {
        None
    } else {
        UpdateCache::load(&ctx.vulcan_dir)
    };

    let (latest_version, release_url, from_cache, checked_at_unix, check_error) =
        match cache.as_ref().filter(|c| c.is_fresh()) {
            Some(c) => (
                c.latest_version.clone(),
                c.release_url.clone(),
                true,
                c.checked_at_unix,
                None,
            ),
            None => match fetch_latest_release(&ctx.raw_http_client).await {
                Ok((version, url)) => {
                    let now = now_unix();
                    let fresh = UpdateCache {
                        checked_at_unix: now,
                        latest_version: version.clone(),
                        release_url: url.clone(),
                    };
                    let _ = fresh.save(&ctx.vulcan_dir);
                    (version, url, false, now, None)
                }
                Err(err) => match cache {
                    Some(stale) => (
                        stale.latest_version,
                        stale.release_url,
                        true,
                        stale.checked_at_unix,
                        Some(err.message.clone()),
                    ),
                    None => return Err(err),
                },
            },
        };

    let update_available = semver_gt(&latest_version, CURRENT_VERSION);
    let update_command = render_update_command(&update_method, &install_path);
    let restart_hint = update_available.then(|| RESTART_HINT.to_string());

    Ok(UpdateCheckResult {
        current_version: CURRENT_VERSION.to_string(),
        latest_version,
        update_available,
        release_url,
        install_path: install_path.to_string_lossy().to_string(),
        update_method,
        update_command,
        restart_hint,
        checked_at: format_unix(checked_at_unix),
        from_cache,
        check_error,
    })
}

/// Best-effort check that never fails — used by the health resource so a slow or
/// rate-limited GitHub doesn't take down the entire health output.
pub async fn execute_check_for_health(ctx: &AppContext) -> UpdateCheckResult {
    match execute_check_inner(ctx, false).await {
        Ok(result) => result,
        Err(err) => {
            let install_path = resolve_install_path();
            let update_method = classify_install(&install_path);
            let update_command = render_update_command(&update_method, &install_path);
            UpdateCheckResult {
                current_version: CURRENT_VERSION.to_string(),
                latest_version: "unknown".to_string(),
                update_available: false,
                release_url: RELEASES_PAGE_URL.to_string(),
                install_path: install_path.to_string_lossy().to_string(),
                update_method,
                update_command,
                restart_hint: None,
                checked_at: format_unix(0),
                from_cache: false,
                check_error: Some(format!("{}: {}", err.code, err.message)),
            }
        }
    }
}

async fn fetch_latest_release(client: &reqwest::Client) -> Result<(String, String), VulcanError> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );
    let user_agent = format!("vulcan-cli/{}", CURRENT_VERSION);
    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .header("User-Agent", user_agent)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| {
            VulcanError::network(
                "UPDATE_CHECK_FAILED",
                format!("GitHub API request failed: {e}"),
            )
        })?;

    let status = resp.status();
    if status == reqwest::StatusCode::FORBIDDEN {
        let remaining = resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok());
        if remaining == Some("0") {
            return Err(VulcanError::rate_limit(
                "UPDATE_RATE_LIMITED",
                "GitHub API rate limit exceeded — try again later",
            ));
        }
    }
    if !status.is_success() {
        return Err(VulcanError::network(
            "UPDATE_CHECK_FAILED",
            format!("GitHub API returned HTTP {status}"),
        ));
    }

    let release: GithubRelease = resp.json().await.map_err(|e| {
        VulcanError::network(
            "UPDATE_CHECK_FAILED",
            format!("could not parse GitHub release JSON: {e}"),
        )
    })?;

    let version = release.tag_name.trim_start_matches('v').to_string();
    let release_url = release
        .html_url
        .unwrap_or_else(|| RELEASES_PAGE_URL.to_string());
    Ok((version, release_url))
}

fn semver_gt(latest: &str, current: &str) -> bool {
    use semver::Version;
    match (Version::parse(latest), Version::parse(current)) {
        (Ok(l), Ok(c)) => l > c,
        _ => false,
    }
}

fn resolve_install_path() -> PathBuf {
    std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .unwrap_or_else(|_| PathBuf::from("vulcan"))
}

fn classify_install(install_path: &Path) -> UpdateMethod {
    let path_str = install_path.to_string_lossy();

    const PM_PREFIXES: &[&str] = &[
        "/opt/homebrew/",
        "/usr/local/Cellar/",
        "/nix/store/",
        "/snap/",
        "/var/lib/flatpak/",
        "/usr/",
    ];
    if PM_PREFIXES.iter().any(|p| path_str.starts_with(p)) {
        return UpdateMethod::PackageManager;
    }

    let parent = match install_path.parent() {
        Some(p) => p,
        None => return UpdateMethod::PackageManager,
    };
    if !is_dir_user_writable(parent) {
        return UpdateMethod::PackageManager;
    }

    if let Some(home) = dirs::home_dir() {
        if install_path == home.join(".local").join("bin").join("vulcan") {
            return UpdateMethod::InstallScriptDefault;
        }
    }
    UpdateMethod::InstallScriptCustomDir
}

fn is_dir_user_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".vulcan-write-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn render_update_command(method: &UpdateMethod, install_path: &Path) -> String {
    match method {
        UpdateMethod::InstallScriptDefault => {
            format!("Run: curl -fsSL {INSTALL_SH_URL} | sh -s -- --no-agent-skills")
        }
        UpdateMethod::InstallScriptCustomDir => {
            let parent = install_path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "$HOME/.local/bin".to_string());
            format!(
                "Run: curl -fsSL {INSTALL_SH_URL} | VULCAN_INSTALL_DIR={parent} sh -s -- --no-agent-skills"
            )
        }
        UpdateMethod::PackageManager => format!(
            "Update via your package manager — vulcan binary lives at {} (not user-managed).",
            install_path.display()
        ),
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn format_unix(secs: u64) -> String {
    if secs == 0 {
        return "never".to_string();
    }
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs as i64, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_gt_handles_normal_bumps() {
        assert!(semver_gt("0.5.5", "0.5.4"));
        assert!(semver_gt("0.6.0", "0.5.99"));
        assert!(semver_gt("1.0.0", "0.99.99"));
        assert!(!semver_gt("0.5.4", "0.5.4"));
        assert!(!semver_gt("0.5.3", "0.5.4"));
    }

    #[test]
    fn semver_gt_returns_false_on_parse_failure() {
        assert!(!semver_gt("not-a-version", "0.5.4"));
        assert!(!semver_gt("0.5.5", "garbage"));
    }

    #[test]
    fn package_manager_prefixes_classify_as_pm() {
        for path in [
            "/opt/homebrew/bin/vulcan",
            "/usr/local/Cellar/vulcan/0.5.4/bin/vulcan",
            "/nix/store/abc123-vulcan-0.5.4/bin/vulcan",
            "/snap/vulcan/current/bin/vulcan",
            "/usr/bin/vulcan",
        ] {
            assert_eq!(
                classify_install(Path::new(path)),
                UpdateMethod::PackageManager,
                "{} should classify as PackageManager",
                path
            );
        }
    }

    #[test]
    fn render_command_for_default_install_omits_dir_var() {
        let cmd = render_update_command(
            &UpdateMethod::InstallScriptDefault,
            Path::new("/home/x/.local/bin/vulcan"),
        );
        assert!(cmd.contains("curl"));
        assert!(cmd.contains("install.sh"));
        assert!(!cmd.contains("VULCAN_INSTALL_DIR"));
        assert!(cmd.contains("--no-agent-skills"));
    }

    #[test]
    fn render_command_for_custom_dir_includes_dir_var() {
        let cmd = render_update_command(
            &UpdateMethod::InstallScriptCustomDir,
            Path::new("/home/x/opt/vulcan"),
        );
        assert!(cmd.contains("VULCAN_INSTALL_DIR=/home/x/opt"));
        assert!(cmd.contains("--no-agent-skills"));
    }

    #[test]
    fn render_command_for_pm_path_points_to_package_manager() {
        let cmd = render_update_command(
            &UpdateMethod::PackageManager,
            Path::new("/opt/homebrew/bin/vulcan"),
        );
        assert!(cmd.contains("package manager"));
        assert!(cmd.contains("/opt/homebrew/bin/vulcan"));
        assert!(!cmd.contains("curl"));
    }
}
