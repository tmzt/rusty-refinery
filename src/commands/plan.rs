use std::path::PathBuf;

use tracing::info;

use crate::config::{self, RefineryConfig};

pub async fn run(
    template: Option<String>,
    extra_args: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = RefineryConfig::load()?;

    let planner_name = template
        .or_else(|| config.options.default_planner.clone())
        .unwrap_or_else(|| "planner".to_string());

    let tmpl = config
        .resolve_template(&planner_name)
        .ok_or_else(|| format!("template not found: {planner_name}"))?
        .clone();

    let planning_path = &config.options.planning_path;

    // Interpolate env vars in template args
    let mut args: Vec<String> = tmpl
        .args
        .iter()
        .map(|a| config::interpolate_env(a, &tmpl.env))
        .collect();

    // Auto-add unsafe flags when allowed
    if config.options.allow_unsafe_agents {
        for flag in tmpl.agent_type.unsafe_args() {
            if !args.iter().any(|a| a == flag) {
                args.push(flag.to_string());
            }
        }
    }

    // Auto-configure MCP server pointing back to crk
    let self_bin = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("crk"));
    let self_bin_str = self_bin.to_string_lossy().to_string();
    let (mcp_args, _mcp_tmp) = tmpl.agent_type.mcp_args(&self_bin_str, &["proxy"]);
    args.extend(mcp_args);

    args.extend(extra_args);

    info!(
        template = %tmpl.name,
        agent_type = ?tmpl.agent_type,
        planning_path = %planning_path.display(),
        "launching planner with stdio pass-through"
    );

    // Interpolate env vars in template env values
    let env: std::collections::HashMap<String, String> = tmpl
        .env
        .iter()
        .map(|(k, v)| (k.clone(), config::interpolate_env(v, &tmpl.env)))
        .collect();

    let mut child = tokio::process::Command::new(&tmpl.command)
        .args(&args)
        .current_dir(planning_path)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .envs(&env)
        .spawn()?;

    let status = child.wait().await?;
    std::process::exit(status.code().unwrap_or(1));
}
