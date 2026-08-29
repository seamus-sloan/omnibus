//! Binary entry point: parse config, build the authenticated client, and
//! serve the MCP tool router over stdio until the client disconnects.
//! Logging goes to stderr only — stdout belongs to the MCP transport.

use std::sync::Arc;

use rmcp::{transport::stdio, ServiceExt};
use tracing_subscriber::EnvFilter;

use omnibus_mcp::client::OmnibusClient;
use omnibus_mcp::config::Config;
use omnibus_mcp::server::OmnibusMcp;

const USAGE: &str = "\
omnibus-mcp — read-only MCP stdio server for an Omnibus instance

USAGE:
  omnibus-mcp [--url <base-url>] [--username <name>] [--password <password>]

Each value falls back to its environment variable when the flag is absent:
  --url       OMNIBUS_MCP_URL       e.g. http://localhost:3000
  --username  OMNIBUS_MCP_USERNAME
  --password  OMNIBUS_MCP_PASSWORD
";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprint!("{USAGE}");
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();

    let config = Config::load(args, |key| std::env::var(key).ok())?;
    let client = OmnibusClient::new(config)?;
    let service = OmnibusMcp::new(Arc::new(client)).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
