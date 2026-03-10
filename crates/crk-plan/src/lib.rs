mod agent;
mod create;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum PlanAction {
    /// Invoke the planning agent with stdio pass-through (default)
    #[command(alias = "run")]
    Agent {
        /// Template name (defaults to default_planner from refinery.toml)
        #[arg(long)]
        template: Option<String>,
        /// Additional arguments passed to the planner command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra_args: Vec<String>,
    },
    /// Create a plan for a submodule (PRD, bead hierarchy, Redis entry, initial commit)
    Create {
        /// Name of the submodule to create a plan for
        submodule: String,
    },
}

pub async fn run(action: Option<PlanAction>) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        Some(PlanAction::Agent { template, extra_args }) => {
            agent::run(template, extra_args).await
        }
        Some(PlanAction::Create { submodule }) => {
            create::run(&submodule).await
        }
        None => {
            // Default: run the planning agent
            agent::run(None, vec![]).await
        }
    }
}
