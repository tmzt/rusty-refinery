mod bead;
mod config;
mod events;
mod git_ops;
mod proxy;
mod reaper;
mod tools;

use rmcp::ServiceExt;
use tracing::info;

use crate::config::RefineryConfig;
use crate::events::EventStream;
use crate::proxy::DEFAULT_SOCKET_PATH;
use crate::tools::RefineryServer;

fn print_usage() {
    eprintln!("Usage: rusty-refinery [MODE]");
    eprintln!();
    eprintln!("Modes:");
    eprintln!("  (default)           Run MCP server on stdio");
    eprintln!("  --daemon [SOCKET]   Listen on a Unix domain socket (default: {DEFAULT_SOCKET_PATH})");
    eprintln!("  --proxy  [SOCKET]   Connect to daemon UDS and bridge to stdio (default: {DEFAULT_SOCKET_PATH})");
    eprintln!("  --help              Show this help");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    let mode = args.get(1).map(|s| s.as_str());

    if mode == Some("--help") || mode == Some("-h") {
        print_usage();
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    match mode {
        Some("--proxy") => {
            let socket_path = args.get(2).map(|s| s.as_str()).unwrap_or(DEFAULT_SOCKET_PATH);
            info!(socket_path, "proxy mode: connecting to daemon");
            proxy::proxy(socket_path).await?;
        }
        Some("--daemon") => {
            let socket_path = args
                .get(2)
                .map(|s| s.as_str())
                .unwrap_or(DEFAULT_SOCKET_PATH);

            let config = RefineryConfig::load()?;
            info!("loaded configuration with {} templates", config.templates.len());

            let events = EventStream::connect(&config.options.redis_url).await?;
            info!("connected to Redis at {}", config.options.redis_url);

            let server = RefineryServer::new(config, events);

            proxy::listen(socket_path, move |reader, writer| {
                let server = server.clone();
                async move {
                    let service = server.serve((reader, writer)).await?;
                    service.waiting().await?;
                    Ok(())
                }
            })
            .await?;
        }
        None => {
            let config = RefineryConfig::load()?;
            info!("loaded configuration with {} templates", config.templates.len());

            let events = EventStream::connect(&config.options.redis_url).await?;
            info!("connected to Redis at {}", config.options.redis_url);

            let server = RefineryServer::new(config, events);

            let service = server.serve(rmcp::transport::io::stdio()).await?;
            info!("rusty-refinery MCP server running on stdio");
            service.waiting().await?;
        }
        Some(other) => {
            eprintln!("Unknown option: {other}");
            print_usage();
            std::process::exit(1);
        }
    }

    Ok(())
}
