//! Vulcan CLI entry point.

use clap::Parser;
use vulcan_lib::cli::{Cli, Command};
use vulcan_lib::context::AppContext;
use vulcan_lib::error::VulcanError;
use vulcan_lib::output::render_error;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Initialize logging
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_env_filter("vulcan=debug")
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter("vulcan=warn")
            .with_writer(std::io::stderr)
            .init();
    }

    let mut ctx = match AppContext::new(
        cli.output,
        cli.dry_run,
        cli.yes,
        cli.verbose,
        cli.watch,
        cli.rpc_url,
        cli.api_url,
        cli.wallet.clone(),
    ) {
        Ok(ctx) => ctx,
        Err(e) => {
            let err = VulcanError::config("INIT_FAILED", e.to_string());
            render_error(cli.output, &err);
            std::process::exit(err.exit_code());
        }
    };

    if command_should_auto_api_login(&cli.command) {
        let auto_login = vulcan_lib::commands::auth::auto_login_if_possible(&ctx).await;
        if auto_login.succeeded {
            if let Err(e) = ctx.reload_http_client() {
                eprintln!(
                    "[auth] failed to reload authenticated Phoenix client: {}",
                    e
                );
            }
        } else if let Some(error) = auto_login.error {
            eprintln!("[auth] automatic Phoenix API login skipped after error: {error}");
        }
    }

    let command_name = command_log_name(&cli.command);
    let command_dangerous = command_may_be_dangerous(&cli.command);
    let started = std::time::Instant::now();
    let result = match cli.command {
        Command::Wallet(cmd) => vulcan_lib::commands::wallet::execute(&ctx, cmd).await,
        Command::Market(cmd) => vulcan_lib::commands::market::execute(&ctx, cmd).await,
        Command::Trade(cmd) => vulcan_lib::commands::trade::execute(&ctx, cmd).await,
        Command::Position(cmd) => vulcan_lib::commands::position::execute(&ctx, cmd).await,
        Command::Margin(cmd) => vulcan_lib::commands::margin::execute(&ctx, cmd).await,
        Command::Account(cmd) => vulcan_lib::commands::account::execute(&ctx, cmd).await,
        Command::Auth(cmd) => vulcan_lib::commands::auth::execute(&ctx, cmd).await,
        Command::Portfolio(args) => vulcan_lib::commands::portfolio::execute(&ctx, args).await,
        Command::Paper(cmd) => vulcan_lib::commands::paper::execute(&ctx, cmd).await,
        Command::History(cmd) => vulcan_lib::commands::history::execute(&ctx, cmd).await,
        Command::Agent(cmd) => vulcan_lib::commands::agent::execute(&ctx, cmd).await,
        Command::Strategy(cmd) => vulcan_lib::commands::strategy::execute(&ctx, cmd).await,
        Command::Ta(cmd) => vulcan_lib::commands::ta::execute(&ctx, cmd).await,
        Command::Status => vulcan_lib::commands::status::execute(&ctx).await,
        Command::Update(cmd) => match cmd {
            vulcan_lib::cli::update::UpdateCommand::Check { force } => {
                vulcan_lib::commands::update::execute(&ctx, force).await
            }
        },
        Command::Setup => vulcan_lib::commands::setup::execute(&ctx).await,
        Command::Version => {
            println!("vulcan {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::AgentContext => {
            print!("{}", include_str!("../../CONTEXT.md"));
            Ok(())
        }
        Command::Mcp {
            allow_dangerous,
            groups,
        } => run_mcp(allow_dangerous, groups, cli.verbose).await,
    };

    match result {
        Ok(()) => {
            if let Some(log) = &ctx.agent_log {
                log.record_call(
                    "cli",
                    command_name,
                    &serde_json::Value::Null,
                    command_dangerous,
                    ctx.yes.then_some(true),
                    "ok",
                    started.elapsed().as_millis(),
                    None,
                    None,
                );
            }
        }
        Err(err) => {
            if let Some(log) = &ctx.agent_log {
                log.record_call(
                    "cli",
                    command_name,
                    &serde_json::Value::Null,
                    command_dangerous,
                    ctx.yes.then_some(true),
                    "error",
                    started.elapsed().as_millis(),
                    None,
                    Some(&err.code),
                );
            }
            render_error(ctx.output_format, &err);
            std::process::exit(err.exit_code());
        }
    }
}

fn command_log_name(command: &Command) -> &'static str {
    use vulcan_lib::cli::account::AccountCommand;
    use vulcan_lib::cli::auth::AuthCommand;
    use vulcan_lib::cli::history::HistoryCommand;
    use vulcan_lib::cli::margin::MarginCommand;
    use vulcan_lib::cli::paper::PaperCommand;
    use vulcan_lib::cli::position::PositionCommand;
    use vulcan_lib::cli::strategy::{GridCommand, StrategyCommand, TaCommand, TwapCommand};
    use vulcan_lib::cli::trade::TradeCommand;

    match command {
        Command::Wallet(_) => "wallet",
        Command::Market(_) => "market",
        Command::Trade(cmd) => match cmd {
            TradeCommand::MarketBuy { .. } => "trade.market_buy",
            TradeCommand::MarketSell { .. } => "trade.market_sell",
            TradeCommand::LimitBuy { .. } => "trade.limit_buy",
            TradeCommand::LimitSell { .. } => "trade.limit_sell",
            TradeCommand::Cancel { .. } => "trade.cancel",
            TradeCommand::CancelAll { .. } => "trade.cancel_all",
            TradeCommand::Orders { .. } => "trade.orders",
            TradeCommand::SetTpsl { .. } => "trade.set_tpsl",
            TradeCommand::CancelTpsl { .. } => "trade.cancel_tpsl",
        },
        Command::Position(cmd) => match cmd {
            PositionCommand::List => "position.list",
            PositionCommand::Show { .. } => "position.show",
            PositionCommand::Close { .. } => "position.close",
            PositionCommand::CloseAll => "position.close_all",
            PositionCommand::Reduce { .. } => "position.reduce",
            PositionCommand::TpSl { .. } => "position.tp_sl",
        },
        Command::Margin(cmd) => match cmd {
            MarginCommand::Status => "margin.status",
            MarginCommand::Deposit { .. } => "margin.deposit",
            MarginCommand::Withdraw { .. } => "margin.withdraw",
            MarginCommand::Transfer { .. } => "margin.transfer",
            MarginCommand::TransferChildToParent { .. } => "margin.transfer_child_to_parent",
            MarginCommand::SyncParentToChild { .. } => "margin.sync_parent_to_child",
            MarginCommand::LeverageTiers { .. } => "margin.leverage_tiers",
            MarginCommand::AddCollateral { .. } => "margin.add_collateral",
        },
        Command::Account(cmd) => match cmd {
            AccountCommand::Register { .. } => "account.register",
            AccountCommand::Info => "account.info",
            AccountCommand::Subaccounts => "account.subaccounts",
            AccountCommand::CreateSubaccount { .. } => "account.create_subaccount",
        },
        Command::Auth(cmd) => match cmd {
            AuthCommand::Login => "auth.login",
            AuthCommand::Status => "auth.status",
            AuthCommand::Logout => "auth.logout",
        },
        Command::Portfolio(_) => "portfolio",
        Command::Paper(cmd) => match cmd {
            PaperCommand::Init { .. } => "paper.init",
            PaperCommand::Reset { .. } => "paper.reset",
            PaperCommand::Status => "paper.status",
            PaperCommand::Positions => "paper.positions",
            PaperCommand::Orders => "paper.orders",
            PaperCommand::Fills { .. } => "paper.fills",
            PaperCommand::Buy(_) => "paper.buy",
            PaperCommand::Sell(_) => "paper.sell",
            PaperCommand::Cancel { .. } => "paper.cancel",
            PaperCommand::CancelAll { .. } => "paper.cancel_all",
            PaperCommand::Reconcile { .. } => "paper.reconcile",
            PaperCommand::SetTpsl { .. } => "paper.set_tpsl",
            PaperCommand::CancelTpsl { .. } => "paper.cancel_tpsl",
            PaperCommand::Triggers { .. } => "paper.triggers",
        },
        Command::History(cmd) => match cmd {
            HistoryCommand::Trades { .. } => "history.trades",
            HistoryCommand::Orders { .. } => "history.orders",
            HistoryCommand::Collateral { .. } => "history.collateral",
            HistoryCommand::Funding { .. } => "history.funding",
            HistoryCommand::Pnl { .. } => "history.pnl",
        },
        Command::Agent(_) => "agent",
        Command::Ta(cmd) => match cmd {
            vulcan_lib::cli::ta::TaCommand::Compute { .. } => "ta.compute",
            vulcan_lib::cli::ta::TaCommand::Signal { .. } => "ta.signal",
            vulcan_lib::cli::ta::TaCommand::Report { .. } => "ta.report",
        },
        Command::Strategy(cmd) => match cmd {
            StrategyCommand::Twap(TwapCommand::Start { .. }) => "strategy.twap.start",
            StrategyCommand::Twap(TwapCommand::Resume { .. }) => "strategy.twap.resume",
            StrategyCommand::Grid(GridCommand::Start { .. }) => "strategy.grid.start",
            StrategyCommand::Grid(GridCommand::Resume { .. }) => "strategy.grid.resume",
            StrategyCommand::Ta(TaCommand::Start(_)) => "strategy.ta.start",
            StrategyCommand::Ta(TaCommand::Resume { .. }) => "strategy.ta.resume",
            StrategyCommand::Preflight => "strategy.preflight",
            StrategyCommand::Runs { .. } => "strategy.runs",
            StrategyCommand::Status { .. } => "strategy.status",
            StrategyCommand::Monitor { .. } => "strategy.monitor",
            StrategyCommand::ReconcileGrid { .. } => "strategy.reconcile_grid",
            StrategyCommand::WaitNextTick { .. } => "strategy.wait_next_tick",
            StrategyCommand::Report { .. } => "strategy.report",
            StrategyCommand::Pause { .. } => "strategy.pause",
            StrategyCommand::Stop { .. } => "strategy.stop",
            StrategyCommand::Finalize { .. } => "strategy.finalize",
            StrategyCommand::Resume { .. } => "strategy.resume",
        },
        Command::Status => "status",
        Command::Update(_) => "update.check",
        Command::Setup => "setup",
        Command::Version => "version",
        Command::AgentContext => "agent_context",
        Command::Mcp { .. } => "mcp",
    }
}

fn command_may_be_dangerous(command: &Command) -> bool {
    use vulcan_lib::cli::account::AccountCommand;
    use vulcan_lib::cli::auth::AuthCommand;
    use vulcan_lib::cli::margin::MarginCommand;
    use vulcan_lib::cli::position::PositionCommand;
    use vulcan_lib::cli::strategy::{
        GridCommand, StrategyCommand, StrategyModeArg, TaCommand, TwapCommand,
    };
    use vulcan_lib::cli::trade::TradeCommand;

    match command {
        Command::Trade(cmd) => !matches!(cmd, TradeCommand::Orders { .. }),
        Command::Position(cmd) => matches!(
            cmd,
            PositionCommand::Close { .. }
                | PositionCommand::Reduce { .. }
                | PositionCommand::TpSl { .. }
        ),
        Command::Margin(cmd) => matches!(
            cmd,
            MarginCommand::Deposit { .. }
                | MarginCommand::Withdraw { .. }
                | MarginCommand::Transfer { .. }
                | MarginCommand::TransferChildToParent { .. }
                | MarginCommand::SyncParentToChild { .. }
                | MarginCommand::AddCollateral { .. }
        ),
        Command::Account(cmd) => matches!(
            cmd,
            AccountCommand::Register { .. } | AccountCommand::CreateSubaccount { .. }
        ),
        Command::Auth(cmd) => matches!(cmd, AuthCommand::Login | AuthCommand::Logout),
        Command::Paper(_) => false,
        Command::Strategy(cmd) => {
            ({
                match cmd {
                    StrategyCommand::Twap(TwapCommand::Start(args)) => matches!(
                        args.mode,
                        StrategyModeArg::ConfirmEach | StrategyModeArg::AutoExecute
                    ),
                    StrategyCommand::Grid(GridCommand::Start(args)) => matches!(
                        args.mode,
                        StrategyModeArg::ConfirmEach | StrategyModeArg::AutoExecute
                    ),
                    StrategyCommand::Ta(TaCommand::Start(args)) => matches!(
                        args.mode,
                        StrategyModeArg::ConfirmEach | StrategyModeArg::AutoExecute
                    ),
                    _ => false,
                }
            }) || matches!(
                cmd,
                StrategyCommand::Twap(TwapCommand::Resume { .. })
                    | StrategyCommand::Grid(GridCommand::Resume { .. })
                    | StrategyCommand::Finalize {
                        cancel_orders: true,
                        ..
                    }
                    | StrategyCommand::Finalize {
                        close_position: true,
                        ..
                    }
                    | StrategyCommand::Resume { .. }
            )
        }
        _ => false,
    }
}

fn command_should_auto_api_login(command: &Command) -> bool {
    use vulcan_lib::cli::account::AccountCommand;
    use vulcan_lib::cli::agent::AgentCommand;
    use vulcan_lib::cli::auth::AuthCommand;
    use vulcan_lib::cli::margin::MarginCommand;
    use vulcan_lib::cli::position::PositionCommand;
    use vulcan_lib::cli::strategy::{
        GridCommand, StrategyCommand, StrategyModeArg, TaCommand, TwapCommand,
    };
    use vulcan_lib::cli::trade::TradeCommand;

    match command {
        Command::Agent(AgentCommand::Health { .. }) | Command::Status => true,
        Command::Auth(AuthCommand::Status) => true,
        Command::Auth(AuthCommand::Login | AuthCommand::Logout) => false,
        Command::Portfolio(_) => true,
        Command::Trade(cmd) => matches!(
            cmd,
            TradeCommand::MarketBuy { .. }
                | TradeCommand::MarketSell { .. }
                | TradeCommand::LimitBuy { .. }
                | TradeCommand::LimitSell { .. }
                | TradeCommand::Cancel { .. }
                | TradeCommand::CancelAll { .. }
                | TradeCommand::Orders { .. }
                | TradeCommand::SetTpsl { .. }
                | TradeCommand::CancelTpsl { .. }
        ),
        Command::Position(cmd) => matches!(
            cmd,
            PositionCommand::List
                | PositionCommand::Show { .. }
                | PositionCommand::Close { .. }
                | PositionCommand::CloseAll
                | PositionCommand::Reduce { .. }
                | PositionCommand::TpSl { .. }
        ),
        Command::Margin(cmd) => matches!(
            cmd,
            MarginCommand::Status
                | MarginCommand::Deposit { .. }
                | MarginCommand::Withdraw { .. }
                | MarginCommand::Transfer { .. }
                | MarginCommand::TransferChildToParent { .. }
                | MarginCommand::SyncParentToChild { .. }
                | MarginCommand::AddCollateral { .. }
        ),
        Command::Account(cmd) => matches!(
            cmd,
            AccountCommand::Register { .. }
                | AccountCommand::Info
                | AccountCommand::Subaccounts
                | AccountCommand::CreateSubaccount { .. }
        ),
        Command::Strategy(StrategyCommand::Twap(TwapCommand::Start(args))) => {
            matches!(
                args.mode,
                StrategyModeArg::ConfirmEach | StrategyModeArg::AutoExecute
            )
        }
        Command::Strategy(StrategyCommand::Grid(GridCommand::Start(args))) => {
            matches!(
                args.mode,
                StrategyModeArg::ConfirmEach | StrategyModeArg::AutoExecute
            )
        }
        Command::Strategy(StrategyCommand::Ta(TaCommand::Start(args))) => {
            matches!(
                args.mode,
                StrategyModeArg::ConfirmEach | StrategyModeArg::AutoExecute
            )
        }
        Command::Strategy(StrategyCommand::Ta(TaCommand::Resume { .. })) => true,
        Command::Strategy(StrategyCommand::Resume { .. })
        | Command::Strategy(StrategyCommand::Finalize { .. })
        | Command::Strategy(StrategyCommand::ReconcileGrid { .. })
        | Command::Strategy(StrategyCommand::Twap(TwapCommand::Resume { .. }))
        | Command::Strategy(StrategyCommand::Grid(GridCommand::Resume { .. })) => true,
        Command::Setup => true,
        _ => false,
    }
}

async fn run_mcp(
    allow_dangerous: bool,
    groups: Option<Vec<String>>,
    verbose: bool,
) -> Result<(), VulcanError> {
    use rmcp::ServiceExt;
    use std::sync::Arc;

    let mut mcp_ctx = AppContext::new(
        vulcan_lib::output::OutputFormat::Json,
        false, // dry_run
        true,  // yes (auto-confirm for MCP)
        verbose,
        false, // watch
        None,
        None,
        None, // `--wallet` is not used for MCP; session uses VULCAN_WALLET_NAME / default
    )
    .map_err(|e| VulcanError::config("MCP_INIT_FAILED", e.to_string()))?;

    // Unlock session wallet if dangerous tools are enabled
    if allow_dangerous {
        let wallet_name = match std::env::var("VULCAN_WALLET_NAME")
            .ok()
            .filter(|name| !name.trim().is_empty() && name != "your-wallet")
        {
            Some(name) => name,
            None => match mcp_ctx
                .wallet_store
                .default_wallet()
                .map_err(|e| VulcanError::config("CONFIG_ERROR", e.to_string()))?
            {
                Some(name) => name,
                None => {
                    return Err(VulcanError::config(
                        "NO_DEFAULT_WALLET",
                        "No wallet selected for MCP. Set VULCAN_WALLET_NAME or use 'vulcan wallet set-default <NAME>'",
                    ));
                }
            },
        };

        let wallet_file = mcp_ctx
            .wallet_store
            .load(&wallet_name)
            .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;

        // Plugin hosts (Claude Code's `userConfig`, etc.) pass an empty string when the
        // user leaves a sensitive field blank. Treat that the same as unset: fall
        // through to the interactive prompt for human-driven runs, or fail with a
        // clear error for MCP-stdio runs where there's nothing to prompt. Remote
        // signer records do not use VULCAN_WALLET_PASSWORD; they resolve their own
        // provider-specific env vars.
        let password = if wallet_file.is_local_encrypted() {
            match std::env::var("VULCAN_WALLET_PASSWORD")
                .ok()
                .filter(|p| !p.is_empty() && p != "your-password")
            {
                Some(pw) => Some(pw),
                None => {
                    use std::io::IsTerminal;
                    if !std::io::stdin().is_terminal() {
                        return Err(mcp_wallet_password_required_error());
                    }
                    eprint!("Wallet password (for MCP session): ");
                    Some(
                        rpassword::read_password()
                            .map_err(|e| VulcanError::io("PASSWORD_READ_FAILED", e.to_string()))?,
                    )
                }
            }
        } else {
            None
        };

        let signer =
            vulcan_lib::wallet::ResolvedSigner::from_wallet_file(&wallet_file, password.as_deref())
                .await?;
        let session_wallet = vulcan_lib::mcp::session_wallet::SessionWallet::from_resolved(signer)?;

        eprintln!(
            "[mcp] Session wallet unlocked: {} ({})",
            wallet_file.name, wallet_file.public_key
        );
        mcp_ctx.session_wallet = Some(Arc::new(session_wallet));
        let auto_login = vulcan_lib::commands::auth::auto_login_if_possible(&mcp_ctx).await;
        if auto_login.succeeded {
            mcp_ctx
                .reload_http_client()
                .map_err(|e| VulcanError::config("MCP_AUTH_CLIENT_RELOAD_FAILED", e.to_string()))?;
            eprintln!("[mcp] Phoenix API session refreshed with wallet signature");
        } else if let Some(error) = auto_login.error {
            eprintln!("[mcp] Phoenix API auto-login skipped after error: {error}");
        }
    }

    let ctx = Arc::new(mcp_ctx);
    let server = vulcan_lib::mcp::server::VulcanMcpServer::new(ctx, allow_dangerous, groups);

    eprintln!("[mcp] Starting Vulcan MCP server over stdio...");

    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| VulcanError::internal("MCP_SERVE_FAILED", e.to_string()))?;

    service
        .waiting()
        .await
        .map_err(|e| VulcanError::internal("MCP_WAIT_FAILED", e.to_string()))?;

    Ok(())
}

fn mcp_wallet_password_required_error() -> VulcanError {
    VulcanError::auth(
        "WALLET_PASSWORD_REQUIRED",
        "Live MCP session needs a wallet password but VULCAN_WALLET_PASSWORD is unset or empty. \
         If you installed Vulcan as a plugin (Claude Code marketplace), fill in the `Wallet password` field \
         in the plugin's settings UI - the host stores it in your OS keychain and passes it in here. \
         For manual MCP setups, set VULCAN_WALLET_PASSWORD in the MCP server's env block.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use vulcan_lib::cli::account::AccountCommand;
    use vulcan_lib::cli::margin::MarginCommand;
    use vulcan_lib::cli::portfolio::PortfolioArgs;
    use vulcan_lib::cli::position::PositionCommand;
    use vulcan_lib::cli::trade::TradeCommand;

    #[test]
    fn trader_portfolio_reads_prepare_api_auth_session() {
        let commands = vec![
            Command::Portfolio(PortfolioArgs {
                include: Vec::new(),
            }),
            Command::Trade(TradeCommand::Orders { symbol: None }),
            Command::Position(PositionCommand::List),
            Command::Position(PositionCommand::Show {
                symbol: "SOL".to_string(),
            }),
            Command::Margin(MarginCommand::Status),
            Command::Account(AccountCommand::Info),
            Command::Account(AccountCommand::Subaccounts),
        ];

        for command in commands {
            assert!(command_should_auto_api_login(&command), "{command:?}");
        }
    }
}
