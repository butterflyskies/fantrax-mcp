use std::sync::Arc;

use clap::Parser;
use mcp_session::BoundedSessionManagerBuilder;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tracing_subscriber::EnvFilter;

use fantrax_mcp::config::Config;
use fantrax_mcp::db::Database;
use fantrax_mcp::fantrax::FantraxClient;
use fantrax_mcp::mlb::MlbClient;
use fantrax_mcp::projections::ProjectionClient;
use fantrax_mcp::server::{AppState, FantraxServer};

#[derive(Parser, Debug)]
#[command(
    name = "fantrax-mcp",
    about = "Fantasy baseball MCP server for Fantrax",
    version
)]
struct Cli {
    /// Path to TOML config file
    #[arg(short, long, default_value = "config.toml")]
    config: std::path::PathBuf,

    /// Override server port from config
    #[arg(short, long)]
    port: Option<u16>,

    /// Override bind address from config (e.g. 0.0.0.0 for containers)
    #[arg(long)]
    bind_addr: Option<std::net::IpAddr>,

    /// Override database path from config
    #[arg(long)]
    db_path: Option<String>,

    /// Maximum concurrent MCP sessions
    #[arg(long, default_value = "10")]
    max_sessions: usize,

    /// Idle timeout in seconds (0 = disabled)
    #[arg(long, default_value = "14400")]
    idle_timeout_secs: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    let mut config = Config::load(&cli.config)?;

    if let Some(port) = cli.port {
        config.server.port = port;
    }
    if let Some(addr) = cli.bind_addr {
        config.server.bind_addr = addr;
    }
    if let Some(ref db_path) = cli.db_path {
        config.server.db_path = db_path.clone();
    }

    tracing::info!(
        port = config.server.port,
        leagues = config.leagues.len(),
        "loaded configuration"
    );

    let port = config.server.port;
    let bind_addr = config.server.bind_addr;
    let db_path = config.server.resolved_db_path();
    let db = Database::open(&db_path)
        .map_err(|e| format!("failed to open database at {}: {e}", db_path.display()))?;
    tracing::info!(path = %db_path.display(), "opened SQLite database");

    let secret = config
        .fantrax
        .user_secret_id
        .take()
        .expect("user_secret_id missing from config");
    let client = FantraxClient::new(secret);
    let db = Arc::new(db);
    let mlb = MlbClient::new(Arc::clone(&db));
    let projections = ProjectionClient::new(
        Arc::clone(&db),
        config.projections.source.clone(),
        config.projections.refresh_hours,
    );
    let state = Arc::new(AppState {
        config,
        client,
        mlb,
        projections,
        db,
    });

    // Build MCP service with bounded session management.
    let service = StreamableHttpService::new(
        {
            let state = Arc::clone(&state);
            move || Ok(FantraxServer::new(Arc::clone(&state)))
        },
        {
            let mut builder = BoundedSessionManagerBuilder::new(cli.max_sessions);
            if cli.idle_timeout_secs > 0 {
                builder =
                    builder.idle_timeout(std::time::Duration::from_secs(cli.idle_timeout_secs));
            }
            builder.build()
        },
        StreamableHttpServerConfig::default(),
    );

    let readyz_db = Arc::clone(&state.db);
    let router = axum::Router::new()
        .route("/healthz", axum::routing::get(|| async { "ok" }))
        .route(
            "/readyz",
            axum::routing::get(move || {
                let db = Arc::clone(&readyz_db);
                async move {
                    match db.health_check() {
                        Ok(()) => (axum::http::StatusCode::OK, "ready"),
                        Err(_) => (axum::http::StatusCode::SERVICE_UNAVAILABLE, "not ready"),
                    }
                }
            }),
        )
        .nest_service("/mcp", service);

    let addr = std::net::SocketAddr::from((bind_addr, port));
    tracing::info!(%addr, "fantrax-mcp server starting");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
