mod commands;

use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

use librefinery::gen_config::Editor;
use librefinery::proxy::DEFAULT_SOCKET_PATH;

#[derive(Parser)]
#[command(name = "crk", about = "Beads Refinery Orchestrator — MCP server for PRD-to-agent lifecycle")]
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
    /// Manage plans (create, run planner). Defaults to agent if no subcommand given.
    #[command(subcommand_required = false)]
    Plan {
        #[command(subcommand)]
        action: Option<crk_plan::PlanAction>,
    },
    /// Scan planning directory and sync all discovered PRD files
    Scan {
        /// Path to the planning repo (overrides PLANNING_PATH)
        #[arg(long)]
        planning_path: Option<String>,
    },
    /// Manage git hooks for automatic PRD detection
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Manage submodules (create, list)
    Submodule {
        #[command(subcommand)]
        action: SubmoduleAction,
    },
    /// Invoke MCP tools directly from the CLI
    Tools {
        #[command(subcommand)]
        action: ToolAction,
    },
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
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

#[derive(Subcommand)]
enum HookAction {
    /// Install a post-commit hook in the planning repo
    Install {
        /// Path to the planning repo (overrides PLANNING_PATH)
        #[arg(long)]
        planning_path: Option<String>,
    },
    /// Uninstall the post-commit hook from the planning repo
    Uninstall {
        /// Path to the planning repo (overrides PLANNING_PATH)
        #[arg(long)]
        planning_path: Option<String>,
    },
    /// Handle a post-commit event (called by the git hook)
    PostCommit,
}

#[derive(Subcommand)]
enum SubmoduleAction {
    /// Create a new submodule with local repo, remote, and planning directory
    Create {
        /// Name of the new submodule
        name: String,
    },
    /// List discovered submodules
    #[command(alias = "ls")]
    List,
}

#[derive(Subcommand)]
enum ToolAction {
    /// Hash a PRD file and register a new bead
    SyncPrd {
        /// Path to the PRD file to hash and register
        #[arg(long)]
        prd_path: String,
    },
    /// Launch an agent from a template in an isolated worktree
    LaunchAgent {
        /// Bead ID (SHA-1 hash) of the PRD
        #[arg(long)]
        bead_id: String,
        /// Template name from refinery.toml (uses default_agent if omitted)
        #[arg(long)]
        template: Option<String>,
    },
    /// Trigger the planner agent for a bead
    BuildPlan {
        /// Bead ID (SHA-1 hash) of the PRD
        #[arg(long)]
        bead_id: String,
    },
    /// List all beads and their current status
    ListBeads,
    /// Stop a running agent process for a bead
    KillAgent {
        /// Bead ID (SHA-1 hash) of the agent to stop
        #[arg(long)]
        bead_id: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Daemon { socket }) => {
            init_tracing();
            commands::daemon::run(&socket).await?;
        }
        Some(Command::Proxy { socket }) => {
            init_tracing();
            commands::proxy::run(&socket).await?;
        }
        Some(Command::Plan { action }) => {
            init_tracing();
            crk_plan::run(action).await?;
        }
        Some(Command::Scan { planning_path }) => {
            init_tracing();
            commands::scan::run(planning_path).await?;
        }
        Some(Command::Hook { action }) => {
            match action {
                HookAction::Install { planning_path } => {
                    commands::hook::install(planning_path).await?;
                }
                HookAction::Uninstall { planning_path } => {
                    commands::hook::uninstall(planning_path).await?;
                }
                HookAction::PostCommit => {
                    init_tracing();
                    commands::hook::post_commit().await?;
                }
            }
        }
        Some(Command::Tools { action }) => {
            init_tracing();
            match action {
                ToolAction::SyncPrd { prd_path } => {
                    commands::tools::sync_prd(prd_path).await?;
                }
                ToolAction::LaunchAgent { bead_id, template } => {
                    commands::tools::launch_agent(bead_id, template).await?;
                }
                ToolAction::BuildPlan { bead_id } => {
                    commands::tools::build_plan(bead_id).await?;
                }
                ToolAction::ListBeads => {
                    commands::tools::list_beads().await?;
                }
                ToolAction::KillAgent { bead_id } => {
                    commands::tools::kill_agent(bead_id).await?;
                }
            }
        }
        Some(Command::Completions { shell }) => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "crk",
                &mut std::io::stdout(),
            );
        }
        Some(Command::Submodule { action }) => {
            match action {
                SubmoduleAction::Create { name } => {
                    init_tracing();
                    commands::create_submodule::run(&name).await?;
                }
                SubmoduleAction::List => {
                    commands::create_submodule::list().await?;
                }
            }
        }
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
            commands::generate_config::run(
                editor, proxy, socket, binary, planning_path,
                redis_url, allow_unsafe, save, replace_file,
            );
        }
        None => {
            init_tracing();
            commands::stdio::run().await?;
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
