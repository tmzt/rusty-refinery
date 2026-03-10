use crate::config::RefineryConfig;
use crate::events::EventStream;
use crate::tools::RefineryServer;

async fn make_server() -> Result<RefineryServer, Box<dyn std::error::Error>> {
    let config = RefineryConfig::load()?;
    let events = EventStream::connect(&config.options.redis_url).await?;
    Ok(RefineryServer::new(config, events))
}

pub async fn sync_prd(prd_path: String) -> Result<(), Box<dyn std::error::Error>> {
    let server = make_server().await?;
    match server.cli_sync_prd(prd_path).await {
        Ok(msg) => println!("{msg}"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub async fn launch_agent(
    bead_id: String,
    template: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let server = make_server().await?;
    match server.cli_launch_agent(bead_id, template).await {
        Ok(msg) => println!("{msg}"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub async fn build_plan(bead_id: String) -> Result<(), Box<dyn std::error::Error>> {
    let server = make_server().await?;
    match server.cli_build_plan(bead_id).await {
        Ok(msg) => println!("{msg}"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub async fn list_beads() -> Result<(), Box<dyn std::error::Error>> {
    let server = make_server().await?;
    println!("{}", server.cli_list_beads().await);
    Ok(())
}

pub async fn kill_agent(bead_id: String) -> Result<(), Box<dyn std::error::Error>> {
    let server = make_server().await?;
    match server.cli_kill_agent(bead_id).await {
        Ok(msg) => println!("{msg}"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}
