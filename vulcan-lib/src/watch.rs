//! Watch mode — live updates via WebSocket.
//!
//! When `--watch` is active, commands render once then subscribe to a WS channel.
//! On each update, the screen is cleared and the command re-renders.

use crate::context::AppContext;
use crate::error::VulcanError;
use phoenix_rise::api::PhoenixWSClient;
use solana_pubkey::Pubkey;

/// What WS channel to subscribe to for live updates.
pub enum WatchKind {
    /// Trader state (orders, positions, margin) — requires authority pubkey.
    TraderState(Pubkey),
    /// Market stats (ticker) — requires symbol.
    Market(String),
    /// L2 orderbook — requires symbol.
    Orderbook(String),
}

/// Connect to WS and run a watch loop.
///
/// `render` is called on each update — it should clear and re-render.
/// The initial render should happen *before* calling this function.
pub async fn watch_loop<F, Fut>(
    ctx: &AppContext,
    kind: WatchKind,
    render: F,
) -> Result<(), VulcanError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<(), VulcanError>>,
{
    let ws_url = ws_url_from_api(&ctx.config.network.api_url)?;

    let client = PhoenixWSClient::new(&ws_url)
        .map_err(|e| VulcanError::api("WS_CONNECT_FAILED", e.to_string()))?;

    match kind {
        WatchKind::TraderState(authority) => {
            let (mut rx, _handle) = client
                .subscribe_to_trader_state(&authority)
                .map_err(|e| VulcanError::api("WS_SUBSCRIBE_FAILED", e.to_string()))?;

            while let Some(_msg) = rx.recv().await {
                clear_screen();
                render().await?;
            }
        }
        WatchKind::Market(symbol) => {
            let (mut rx, _handle) = client
                .subscribe_to_market(symbol)
                .map_err(|e| VulcanError::api("WS_SUBSCRIBE_FAILED", e.to_string()))?;

            while let Some(_msg) = rx.recv().await {
                clear_screen();
                render().await?;
            }
        }
        WatchKind::Orderbook(symbol) => {
            let (mut rx, _handle) = client
                .subscribe_to_orderbook(symbol)
                .map_err(|e| VulcanError::api("WS_SUBSCRIBE_FAILED", e.to_string()))?;

            while let Some(_msg) = rx.recv().await {
                clear_screen();
                render().await?;
            }
        }
    }

    Ok(())
}

fn clear_screen() {
    // ANSI escape: move cursor to top-left and clear screen
    print!("\x1b[2J\x1b[H");
}

/// Derive WebSocket URL from the HTTP API URL.
fn ws_url_from_api(api_url: &str) -> Result<String, VulcanError> {
    let mut url = api_url.to_string();

    // Replace http(s) scheme with ws(s)
    if url.starts_with("https://") {
        url = format!("wss://{}", &url[8..]);
    } else if url.starts_with("http://") {
        url = format!("ws://{}", &url[7..]);
    }

    // Append /ws if not present
    if !url.ends_with("/ws") && !url.ends_with("/ws/") {
        url = format!("{}/ws", url.trim_end_matches('/'));
    }

    Ok(url)
}
