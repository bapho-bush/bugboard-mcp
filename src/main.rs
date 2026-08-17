mod client;
mod config;
mod errors;
mod handles;
mod normalize;
mod server;
mod write_safety;

use rmcp::{
    ServiceExt,
    transport::{
        StreamableHttpServerConfig, StreamableHttpService, stdio,
        streamable_http_server::session::local::LocalSessionManager,
    },
};

use crate::server::BugboardServer;

const DEFAULT_HTTP_BIND: &str = "127.0.0.1:8000";
const MCP_HTTP_PATH: &str = "/mcp";
type AppResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() -> AppResult {
    if matches!(transport_mode().as_deref(), Some("stdio")) {
        return run_stdio().await;
    }
    run_http().await
}

async fn run_stdio() -> AppResult {
    let service = BugboardServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

async fn run_http() -> AppResult {
    let bind = std::env::var("BUGBOARD_MCP_BIND").unwrap_or_else(|_| DEFAULT_HTTP_BIND.to_owned());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    let addr = listener.local_addr()?;
    let config = http_server_config(&addr);
    let service: StreamableHttpService<BugboardServer, LocalSessionManager> =
        StreamableHttpService::new(|| Ok(BugboardServer::new()), Default::default(), config);
    let router = axum::Router::new().nest_service(MCP_HTTP_PATH, service);

    eprintln!("bugboard-mcp listening on http://{addr}{MCP_HTTP_PATH}");
    axum::serve(listener, router).await?;
    Ok(())
}

pub(crate) fn http_server_config(addr: &std::net::SocketAddr) -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_allowed_hosts(allowed_hosts(addr))
        .with_allowed_origins(allowed_origins(addr))
}

fn transport_mode() -> Option<String> {
    std::env::args()
        .skip(1)
        .find_map(|arg| match arg.as_str() {
            "--stdio" => Some("stdio".to_owned()),
            "--http" => Some("http".to_owned()),
            _ => None,
        })
        .or_else(|| std::env::var("BUGBOARD_MCP_TRANSPORT").ok())
        .map(|value| value.to_ascii_lowercase())
}

pub(crate) fn allowed_hosts(addr: &std::net::SocketAddr) -> Vec<String> {
    let mut hosts = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "::1".to_owned(),
    ];
    hosts.push(addr.to_string());
    hosts
}

pub(crate) fn allowed_origins(addr: &std::net::SocketAddr) -> Vec<String> {
    let port = addr.port();
    let mut origins = vec![
        format!("http://localhost:{port}"),
        format!("http://127.0.0.1:{port}"),
        format!("http://[::1]:{port}"),
    ];
    let bound_origin = format!("http://{addr}");
    if !origins.contains(&bound_origin) {
        origins.push(bound_origin);
    }
    origins
}

#[cfg(test)]
mod tests;
