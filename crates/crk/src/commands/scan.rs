use std::path::PathBuf;

use tracing::info;

use librefinery::config::RefineryConfig;
use librefinery::hooks;

pub async fn run(planning_path: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let config = RefineryConfig::load()?;
    let pp = planning_path
        .map(PathBuf::from)
        .unwrap_or(config.options.planning_path);
    info!(
        planning_path = %pp.display(),
        submodules = config.options.submodules.len(),
        "scanning planning directory"
    );
    hooks::scan_and_sync(&pp, &config.options.redis_url).await?;
    Ok(())
}
