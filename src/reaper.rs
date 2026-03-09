use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::process::Child;
use tokio::sync::Mutex;
use tracing::{error, info};

use crate::bead::{BeadRegistry, BeadStatus};
use crate::config::AgentTemplate;
use crate::events::{BeadEvent, EventStream};

pub struct Reaper {
    processes: Arc<Mutex<HashMap<String, Child>>>,
    events: Arc<Mutex<EventStream>>,
    beads: Arc<Mutex<BeadRegistry>>,
}

impl Reaper {
    pub fn new(events: Arc<Mutex<EventStream>>, beads: Arc<Mutex<BeadRegistry>>) -> Self {
        Reaper {
            processes: Arc::new(Mutex::new(HashMap::new())),
            events,
            beads,
        }
    }

    /// Spawn an agent process from a template in the given worktree directory.
    pub async fn spawn(
        &self,
        bead_id: &str,
        template: &AgentTemplate,
        worktree: &Path,
    ) -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
        let worktree_str = worktree.to_string_lossy().to_string();

        // Substitute template variables in args
        let args: Vec<String> = template
            .args
            .iter()
            .map(|a| {
                a.replace("{BEAD_ID}", bead_id)
                    .replace("{WORKTREE_PATH}", &worktree_str)
            })
            .collect();

        let mut cmd = tokio::process::Command::new(&template.command);
        cmd.args(&args).current_dir(worktree);

        // Set template environment variables with substitution
        for (key, val) in &template.env {
            let val = val
                .replace("{BEAD_ID}", bead_id)
                .replace("{WORKTREE_PATH}", &worktree_str);
            cmd.env(key, val);
        }

        let child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);

        self.processes
            .lock()
            .await
            .insert(bead_id.to_string(), child);

        // Update bead state
        {
            let mut beads = self.beads.lock().await;
            beads.update_status(bead_id, BeadStatus::Running);
            if let Some(bead) = beads.get_mut(bead_id) {
                bead.pid = Some(pid);
            }
        }

        // Emit spawn event
        {
            let mut events = self.events.lock().await;
            let _ = events
                .emit(BeadEvent::AgentSpawn {
                    bead_id: bead_id.to_string(),
                    pid,
                    template: template.name.clone(),
                })
                .await;
            let _ = events.set_bead_status(bead_id, "RUNNING").await;
        }

        info!(bead_id, pid, template = %template.name, "agent spawned");
        Ok(pid)
    }

    /// Kill a running agent process.
    pub async fn kill(
        &self,
        bead_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut procs = self.processes.lock().await;
        if let Some(mut child) = procs.remove(bead_id) {
            child.kill().await?;
            let pid = child.id().unwrap_or(0);

            drop(procs); // release lock before acquiring others

            {
                let mut beads = self.beads.lock().await;
                beads.update_status(bead_id, BeadStatus::Failed);
            }

            {
                let mut events = self.events.lock().await;
                let _ = events
                    .emit(BeadEvent::SigChld {
                        bead_id: bead_id.to_string(),
                        pid,
                        exit_code: -1,
                    })
                    .await;
                let _ = events.set_bead_status(bead_id, "FAILED").await;
            }

            info!(bead_id, "agent killed");
        }
        Ok(())
    }

    /// Start a background monitor loop that detects child process exits.
    pub fn start_monitor(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

                let bead_ids: Vec<String> = {
                    let procs = self.processes.lock().await;
                    procs.keys().cloned().collect()
                };

                for bead_id in bead_ids {
                    let mut procs = self.processes.lock().await;
                    if let Some(child) = procs.get_mut(&bead_id) {
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                let pid = child.id().unwrap_or(0);
                                let exit_code = status.code().unwrap_or(-1);
                                procs.remove(&bead_id);
                                drop(procs);

                                let new_status = if exit_code == 0 {
                                    BeadStatus::Complete
                                } else {
                                    BeadStatus::Failed
                                };
                                let status_str = new_status.to_string();

                                {
                                    let mut beads = self.beads.lock().await;
                                    beads.update_status(&bead_id, new_status);
                                    if let Some(bead) = beads.get_mut(&bead_id) {
                                        bead.pid = None;
                                    }
                                }

                                {
                                    let mut events = self.events.lock().await;
                                    if let Err(e) = events
                                        .emit(BeadEvent::SigChld {
                                            bead_id: bead_id.clone(),
                                            pid,
                                            exit_code,
                                        })
                                        .await
                                    {
                                        error!(%e, "failed to emit SIGCHLD event");
                                    }
                                    let _ =
                                        events.set_bead_status(&bead_id, &status_str).await;
                                }

                                info!(
                                    bead_id,
                                    pid,
                                    exit_code,
                                    "agent process exited"
                                );
                            }
                            Ok(None) => {
                                // Still running — emit heartbeat
                                drop(procs);
                                let status = {
                                    let beads = self.beads.lock().await;
                                    beads
                                        .get(&bead_id)
                                        .map(|b| b.status.to_string())
                                        .unwrap_or_else(|| "UNKNOWN".to_string())
                                };
                                let mut events = self.events.lock().await;
                                let _ = events
                                    .emit(BeadEvent::Heartbeat {
                                        bead_id: bead_id.clone(),
                                        status,
                                    })
                                    .await;
                            }
                            Err(e) => {
                                error!(bead_id, %e, "failed to check child status");
                            }
                        }
                    }
                }
            }
        });
    }
}
