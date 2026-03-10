use rmcp::ServiceExt;
use tracing::info;

use crate::config::RefineryConfig;
use crate::events::EventStream;
use crate::proxy;
use crate::tools::RefineryServer;

pub async fn run(socket: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config = RefineryConfig::load()?;
    info!("loaded configuration with {} templates", config.templates.len());

    let events = EventStream::connect(&config.options.redis_url).await?;
    info!("connected to Redis at {}", config.options.redis_url);

    let server = RefineryServer::new(config, events);

    proxy::listen(socket, move |reader, writer| {
        let server = server.clone();
        async move {
            let service = server.serve((reader, writer)).await?;
            service.waiting().await?;
            Ok(())
        }
    })
    .await?;

    Ok(())
}
