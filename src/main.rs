mod bead;
mod config;
mod events;
mod git_ops;
mod reaper;
mod tools;

use rmcp::ServiceExt;
use tracing::info;

use crate::config::RefineryConfig;
use crate::events::EventStream;
use crate::tools::RefineryServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let config = RefineryConfig::load()?;
    info!("loaded configuration with {} templates", config.templates.len());

    let events = EventStream::connect(&config.options.redis_url).await?;
    info!("connected to Redis at {}", config.options.redis_url);

    let server = RefineryServer::new(config, events);

    let service = server
        .serve(rmcp::transport::io::stdio())
        .await?;

    info!("rusty-refinery MCP server running on stdio");
    service.waiting().await?;

    Ok(())
}
