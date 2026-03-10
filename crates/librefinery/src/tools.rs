use std::path::PathBuf;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::error;

use crate::bead::{Bead, BeadRegistry, BeadStatus};
use crate::config::RefineryConfig;
use crate::events::{BeadEvent, EventStream};
use crate::git_ops::GitOps;
use crate::reaper::Reaper;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SyncPrdRequest {
    /// Path to the PRD file to hash and register.
    pub prd_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LaunchAgentRequest {
    /// Bead ID (SHA-1 hash) of the PRD to launch an agent for.
    pub bead_id: String,
    /// Template name from refinery.toml. Uses default_agent if omitted.
    pub template: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BuildPlanRequest {
    /// Bead ID (SHA-1 hash) of the PRD to build a plan for.
    pub bead_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct KillAgentRequest {
    /// Bead ID (SHA-1 hash) of the agent to stop.
    pub bead_id: String,
}

#[derive(Clone)]
pub struct RefineryServer {
    config: RefineryConfig,
    events: Arc<Mutex<EventStream>>,
    beads: Arc<Mutex<BeadRegistry>>,
    reaper: Arc<Reaper>,
    tool_router: ToolRouter<Self>,
}

impl RefineryServer {
    pub fn new(config: RefineryConfig, events: EventStream) -> Self {
        let events = Arc::new(Mutex::new(events));
        let beads = Arc::new(Mutex::new(BeadRegistry::new()));
        let reaper = Arc::new(Reaper::new(events.clone(), beads.clone()));

        // Start the background monitor
        reaper.clone().start_monitor();

        RefineryServer {
            config,
            events,
            beads,
            reaper,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl RefineryServer {
    /// Hash a PRD file and register a new bead. Returns bead ID (SHA-1).
    #[tool(description = "Hash a PRD file and register a new bead. Returns bead ID (SHA-1).")]
    async fn sync_prd(
        &self,
        Parameters(req): Parameters<SyncPrdRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let prd_path = req.prd_path;
        let path = PathBuf::from(&prd_path);
        if !path.exists() {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "PRD file not found: {prd_path}"
            ))]));
        }

        let bead_id = match GitOps::hash_blob(&path) {
            Ok(id) => id,
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Failed to hash PRD: {e}"
                ))]));
            }
        };

        // Check Redis for deduplication
        {
            let mut events = self.events.lock().await;
            match events.check_bead_status(&bead_id).await {
                Ok(Some(status)) if status == "COMPLETE" => {
                    return Ok(CallToolResult::success(vec![Content::text(format!(
                        "Bead {bead_id} already COMPLETE. Skipping."
                    ))]));
                }
                Ok(Some(status)) => {
                    return Ok(CallToolResult::success(vec![Content::text(format!(
                        "Bead {bead_id} exists with status: {status}"
                    ))]));
                }
                Err(e) => {
                    error!(%e, "Redis check failed, continuing without dedup");
                }
                _ => {}
            }
        }

        // Register the bead
        let bead = Bead {
            id: bead_id.clone(),
            prd_path: prd_path.clone(),
            status: BeadStatus::New,
            worktree: None,
            pid: None,
        };

        {
            let mut beads = self.beads.lock().await;
            beads.register(bead);
        }

        // Emit event and set status
        {
            let mut events = self.events.lock().await;
            let _ = events
                .emit(BeadEvent::NewBead {
                    bead_id: bead_id.clone(),
                    prd_path: prd_path.clone(),
                })
                .await;
            let _ = events.set_bead_status(&bead_id, "NEW").await;
        }

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Bead registered: {bead_id}"
        ))]))
    }

    /// Launch an agent from a template in an isolated worktree.
    #[tool(description = "Launch an agent from a template in an isolated worktree.")]
    async fn launch_agent(
        &self,
        Parameters(req): Parameters<LaunchAgentRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let bead_id = req.bead_id;
        let template_name = req
            .template
            .or_else(|| self.config.options.default_agent.clone())
            .unwrap_or_else(|| "coder".to_string());

        let tmpl = match self.config.resolve_template(&template_name) {
            Some(t) => t.clone(),
            None => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Template not found: {template_name}"
                ))]));
            }
        };

        // Verify bead exists
        let prd_path = {
            let beads = self.beads.lock().await;
            match beads.get(&bead_id) {
                Some(bead) => bead.prd_path.clone(),
                None => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Bead not found: {bead_id}. Run sync_prd first."
                    ))]));
                }
            }
        };

        // Resolve target submodule from PRD subdirectory structure
        let (worktree_base, target_info) = if let Some((info, submodule_path)) =
            crate::git_ops::resolve_target_submodule(
                &prd_path,
                &self.config.options.submodules,
                &self.config.options.repo_root,
            )
        {
            (submodule_path, Some(info.name.clone()))
        } else {
            // Fallback to planning path if no submodule match
            (self.config.options.planning_path.clone(), None)
        };

        let worktree_path = match GitOps::create_worktree(&worktree_base, &bead_id, "HEAD").await {
            Ok(p) => p,
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Failed to create worktree: {e}"
                ))]));
            }
        };

        // Update bead with worktree path
        {
            let mut beads = self.beads.lock().await;
            if let Some(bead) = beads.get_mut(&bead_id) {
                bead.worktree = Some(worktree_path.clone());
            }
        }

        // Spawn the agent
        match self.reaper.spawn(&bead_id, &tmpl, &worktree_path).await {
            Ok(pid) => {
                let target = target_info.as_deref().unwrap_or("planning");
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Agent launched for bead {bead_id}\nPID: {pid}\nTemplate: {}\nTarget: {target}\nWorktree: {}\nPRD: {prd_path}",
                    tmpl.name,
                    worktree_path.display()
                ))]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to spawn agent: {e}"
            ))])),
        }
    }

    /// Trigger the planner agent for a bead.
    #[tool(description = "Trigger the planner agent for a bead.")]
    async fn build_plan(
        &self,
        Parameters(req): Parameters<BuildPlanRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let bead_id = req.bead_id;
        let planner_name = self
            .config
            .options
            .default_planner
            .clone()
            .unwrap_or_else(|| "planner".to_string());

        let tmpl = match self.config.resolve_template(&planner_name) {
            Some(t) => t.clone(),
            None => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Planner template not found: {planner_name}"
                ))]));
            }
        };

        // Verify bead exists
        {
            let beads = self.beads.lock().await;
            if beads.get(&bead_id).is_none() {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Bead not found: {bead_id}. Run sync_prd first."
                ))]));
            }
        }

        // Create worktree for planner
        let repo_path = self.config.options.planning_path.clone();
        let worktree_path =
            match GitOps::create_worktree(&repo_path, &format!("{bead_id}-plan"), "HEAD").await {
                Ok(p) => p,
                Err(e) => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Failed to create planner worktree: {e}"
                    ))]));
                }
            };

        match self.reaper.spawn(&bead_id, &tmpl, &worktree_path).await {
            Ok(pid) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Planner launched for bead {bead_id}\nPID: {pid}\nTemplate: {}",
                tmpl.name
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to spawn planner: {e}"
            ))])),
        }
    }

    /// List all beads and their current status.
    #[tool(description = "List all beads and their current status.")]
    async fn list_beads(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let beads = self.beads.lock().await;
        let bead_list = beads.list();

        if bead_list.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No beads registered.",
            )]));
        }

        let mut table = String::from("ID | Status | PRD | PID\n---|--------|-----|----\n");
        for bead in bead_list {
            let pid_str = bead
                .pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string());
            table.push_str(&format!(
                "{}… | {} | {} | {}\n",
                &bead.id[..12.min(bead.id.len())],
                bead.status,
                bead.prd_path,
                pid_str
            ));
        }

        Ok(CallToolResult::success(vec![Content::text(table)]))
    }

    /// Stop a running agent process for a bead.
    #[tool(description = "Stop a running agent process for a bead.")]
    async fn kill_agent(
        &self,
        Parameters(req): Parameters<KillAgentRequest>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let bead_id = req.bead_id;
        match self.reaper.kill(&bead_id).await {
            Ok(()) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Agent for bead {bead_id} stopped."
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Failed to kill agent: {e}"
            ))])),
        }
    }
}

/// Public CLI-facing methods that mirror the MCP tools.
/// These call the same logic but return simple Result<String, String>.
impl RefineryServer {
    pub async fn cli_sync_prd(&self, prd_path: String) -> Result<String, String> {
        let path = PathBuf::from(&prd_path);
        if !path.exists() {
            return Err(format!("PRD file not found: {prd_path}"));
        }

        let bead_id = GitOps::hash_blob(&path)
            .map_err(|e| format!("Failed to hash PRD: {e}"))?;

        {
            let mut events = self.events.lock().await;
            match events.check_bead_status(&bead_id).await {
                Ok(Some(status)) if status == "COMPLETE" => {
                    return Ok(format!("Bead {bead_id} already COMPLETE. Skipping."));
                }
                Ok(Some(status)) => {
                    return Ok(format!("Bead {bead_id} exists with status: {status}"));
                }
                Err(e) => {
                    error!(%e, "Redis check failed, continuing without dedup");
                }
                _ => {}
            }
        }

        {
            let mut beads = self.beads.lock().await;
            beads.register(Bead {
                id: bead_id.clone(),
                prd_path: prd_path.clone(),
                status: BeadStatus::New,
                worktree: None,
                pid: None,
            });
        }

        {
            let mut events = self.events.lock().await;
            let _ = events
                .emit(BeadEvent::NewBead {
                    bead_id: bead_id.clone(),
                    prd_path: prd_path.clone(),
                })
                .await;
            let _ = events.set_bead_status(&bead_id, "NEW").await;
        }

        Ok(format!("Bead registered: {bead_id}"))
    }

    pub async fn cli_launch_agent(
        &self,
        bead_id: String,
        template: Option<String>,
    ) -> Result<String, String> {
        let template_name = template
            .or_else(|| self.config.options.default_agent.clone())
            .unwrap_or_else(|| "coder".to_string());

        let tmpl = self
            .config
            .resolve_template(&template_name)
            .ok_or_else(|| format!("Template not found: {template_name}"))?
            .clone();

        let prd_path = {
            let beads = self.beads.lock().await;
            beads
                .get(&bead_id)
                .map(|b| b.prd_path.clone())
                .ok_or_else(|| format!("Bead not found: {bead_id}. Run sync-prd first."))?
        };

        let (worktree_base, target_info) =
            if let Some((info, submodule_path)) = crate::git_ops::resolve_target_submodule(
                &prd_path,
                &self.config.options.submodules,
                &self.config.options.repo_root,
            ) {
                (submodule_path, Some(info.name.clone()))
            } else {
                (self.config.options.planning_path.clone(), None)
            };

        let worktree_path = GitOps::create_worktree(&worktree_base, &bead_id, "HEAD")
            .await
            .map_err(|e| format!("Failed to create worktree: {e}"))?;

        {
            let mut beads = self.beads.lock().await;
            if let Some(bead) = beads.get_mut(&bead_id) {
                bead.worktree = Some(worktree_path.clone());
            }
        }

        match self.reaper.spawn(&bead_id, &tmpl, &worktree_path).await {
            Ok(pid) => {
                let target = target_info.as_deref().unwrap_or("planning");
                Ok(format!(
                    "Agent launched for bead {bead_id}\nPID: {pid}\nTemplate: {}\nTarget: {target}\nWorktree: {}\nPRD: {prd_path}",
                    tmpl.name,
                    worktree_path.display()
                ))
            }
            Err(e) => Err(format!("Failed to spawn agent: {e}")),
        }
    }

    pub async fn cli_build_plan(&self, bead_id: String) -> Result<String, String> {
        let planner_name = self
            .config
            .options
            .default_planner
            .clone()
            .unwrap_or_else(|| "planner".to_string());

        let tmpl = self
            .config
            .resolve_template(&planner_name)
            .ok_or_else(|| format!("Planner template not found: {planner_name}"))?
            .clone();

        {
            let beads = self.beads.lock().await;
            if beads.get(&bead_id).is_none() {
                return Err(format!("Bead not found: {bead_id}. Run sync-prd first."));
            }
        }

        let repo_path = self.config.options.planning_path.clone();
        let worktree_path =
            GitOps::create_worktree(&repo_path, &format!("{bead_id}-plan"), "HEAD")
                .await
                .map_err(|e| format!("Failed to create planner worktree: {e}"))?;

        match self.reaper.spawn(&bead_id, &tmpl, &worktree_path).await {
            Ok(pid) => Ok(format!(
                "Planner launched for bead {bead_id}\nPID: {pid}\nTemplate: {}",
                tmpl.name
            )),
            Err(e) => Err(format!("Failed to spawn planner: {e}")),
        }
    }

    pub async fn cli_list_beads(&self) -> String {
        let beads = self.beads.lock().await;
        let bead_list = beads.list();

        if bead_list.is_empty() {
            return "No beads registered.".to_string();
        }

        let mut table = String::from("ID | Status | PRD | PID\n---|--------|-----|----\n");
        for bead in bead_list {
            let pid_str = bead
                .pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string());
            table.push_str(&format!(
                "{}… | {} | {} | {}\n",
                &bead.id[..12.min(bead.id.len())],
                bead.status,
                bead.prd_path,
                pid_str
            ));
        }
        table
    }

    pub async fn cli_kill_agent(&self, bead_id: String) -> Result<String, String> {
        self.reaper
            .kill(&bead_id)
            .await
            .map(|()| format!("Agent for bead {bead_id} stopped."))
            .map_err(|e| format!("Failed to kill agent: {e}"))
    }
}

#[tool_handler]
impl ServerHandler for RefineryServer {}
