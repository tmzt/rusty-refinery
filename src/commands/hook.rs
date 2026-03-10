use std::path::PathBuf;

use tracing::error;

use crate::config::RefineryConfig;
use crate::hooks;

pub async fn install(planning_path: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let config = RefineryConfig::load()?;
    let pp = planning_path
        .map(PathBuf::from)
        .unwrap_or(config.options.planning_path);
    let bin = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("crk"));
    match hooks::install(&pp, &bin.to_string_lossy()) {
        Ok(msg) => eprintln!("{msg}"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub async fn uninstall(planning_path: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let config = RefineryConfig::load()?;
    let pp = planning_path
        .map(PathBuf::from)
        .unwrap_or(config.options.planning_path);
    match hooks::uninstall(&pp) {
        Ok(msg) => eprintln!("{msg}"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub async fn post_commit() -> Result<(), Box<dyn std::error::Error>> {
    let config = RefineryConfig::load()?;
    if let Err(e) = hooks::post_commit(&config.options.planning_path, &config.options.redis_url).await {
        error!(%e, "post-commit hook failed");
        std::process::exit(1);
    }
    Ok(())
}
