use rmcp::ServiceExt;
use tracing::info;

use crate::config::RefineryConfig;
use crate::events::EventStream;
use crate::tools::RefineryServer;

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = RefineryConfig::load()?;
    info!("loaded configuration with {} templates", config.templates.len());

    let events = EventStream::connect(&config.options.redis_url).await?;
    info!("connected to Redis at {}", config.options.redis_url);

    let server = RefineryServer::new(config, events);

    let service = server.serve(rmcp::transport::io::stdio()).await?;
    info!("crk MCP server running on stdio");
    service.waiting().await?;

    Ok(())
}
