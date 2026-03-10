mod bead;
mod config;
mod events;
mod gen_config;
mod git_ops;
mod proxy;
mod reaper;
mod tools;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use tracing::info;

use crate::config::RefineryConfig;
use crate::events::EventStream;
use crate::gen_config::{Editor, GenerateOptions};
use crate::proxy::DEFAULT_SOCKET_PATH;
use crate::tools::RefineryServer;

#[derive(Parser)]
#[command(name = "rusty-refinery", about = "Beads Refinery Orchestrator — MCP server for PRD-to-agent lifecycle")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Listen on a Unix domain socket (daemon mode)
    Daemon {
        /// Socket path
        #[arg(default_value = DEFAULT_SOCKET_PATH)]
        socket: String,
    },
    /// Connect to daemon UDS and bridge to stdio (proxy mode)
    Proxy {
        /// Socket path
        #[arg(default_value = DEFAULT_SOCKET_PATH)]
        socket: String,
    },
    /// Generate MCP configuration for an editor
    GenerateConfig {
        /// Target editor
        #[arg(value_enum)]
        editor: Editor,
        /// Use proxy mode (connect to daemon UDS) instead of direct stdio
        #[arg(long)]
        proxy: bool,
        /// Custom socket path for proxy mode
        #[arg(long)]
        socket: Option<String>,
        /// Override binary path in generated config
        #[arg(long)]
        binary: Option<PathBuf>,
        /// PLANNING_PATH to include in env
        #[arg(long)]
        planning_path: Option<String>,
        /// REDIS_URL to include in env
        #[arg(long)]
        redis_url: Option<String>,
        /// Set ALLOW_UNSAFE_AGENTS=true in env
        #[arg(long)]
        allow_unsafe: bool,
        /// Save config to the editor's config path relative to the git root (merges with existing)
        #[arg(long)]
        save: bool,
        /// Overwrite existing config file instead of merging (requires --save)
        #[arg(long, requires = "save")]
        replace_file: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::GenerateConfig {
            editor,
            proxy,
            socket,
            binary,
            planning_path,
            redis_url,
            allow_unsafe,
            save,
            replace_file,
        }) => {
            let binary_path = binary.unwrap_or_else(|| {
                std::env::current_exe().unwrap_or_else(|_| PathBuf::from("rusty-refinery"))
            });
            let output = gen_config::generate(&GenerateOptions {
                editor: editor.clone(),
                binary_path,
                proxy,
                socket_path: socket,
                planning_path,
                redis_url,
                allow_unsafe,
            });
            if save {
                match gen_config::save(&editor, &output, replace_file) {
                    Ok(path) => eprintln!("Wrote config to {}", path.display()),
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                eprintln!("# Save to: {}", editor.config_path_hint());
                println!("{output}");
            }
            return Ok(());
        }
        Some(Command::Proxy { socket }) => {
            init_tracing();
            info!(socket_path = %socket, "proxy mode: connecting to daemon");
            proxy::proxy(&socket).await?;
        }
        Some(Command::Daemon { socket }) => {
            init_tracing();
            let config = RefineryConfig::load()?;
            info!("loaded configuration with {} templates", config.templates.len());

            let events = EventStream::connect(&config.options.redis_url).await?;
            info!("connected to Redis at {}", config.options.redis_url);

            let server = RefineryServer::new(config, events);

            proxy::listen(&socket, move |reader, writer| {
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
            init_tracing();
            let config = RefineryConfig::load()?;
            info!("loaded configuration with {} templates", config.templates.len());

            let events = EventStream::connect(&config.options.redis_url).await?;
            info!("connected to Redis at {}", config.options.redis_url);

            let server = RefineryServer::new(config, events);

            let service = server.serve(rmcp::transport::io::stdio()).await?;
            info!("rusty-refinery MCP server running on stdio");
            service.waiting().await?;
        }
    }

    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();
}
