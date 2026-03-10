use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BeadStatus {
    New,
    Running,
    Complete,
    Failed,
}

impl fmt::Display for BeadStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BeadStatus::New => write!(f, "NEW"),
            BeadStatus::Running => write!(f, "RUNNING"),
            BeadStatus::Complete => write!(f, "COMPLETE"),
            BeadStatus::Failed => write!(f, "FAILED"),
        }
    }
}

impl BeadStatus {
    pub fn _from_str_status(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "RUNNING" => BeadStatus::Running,
            "COMPLETE" => BeadStatus::Complete,
            "FAILED" => BeadStatus::Failed,
            _ => BeadStatus::New,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Bead {
    pub id: String,
    pub prd_path: String,
    pub status: BeadStatus,
    pub worktree: Option<PathBuf>,
    pub pid: Option<u32>,
}

pub struct BeadRegistry {
    beads: HashMap<String, Bead>,
    by_status: HashMap<BeadStatus, HashSet<String>>,
}

impl BeadRegistry {
    pub fn new() -> Self {
        BeadRegistry {
            beads: HashMap::new(),
            by_status: HashMap::new(),
        }
    }

    pub fn register(&mut self, bead: Bead) {
        self.by_status
            .entry(bead.status)
            .or_default()
            .insert(bead.id.clone());
        self.beads.insert(bead.id.clone(), bead);
    }

    pub fn get(&self, id: &str) -> Option<&Bead> {
        self.beads.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Bead> {
        self.beads.get_mut(id)
    }

    pub fn list(&self) -> Vec<&Bead> {
        self.beads.values().collect()
    }

    pub fn _list_by_status(&self, status: BeadStatus) -> Vec<&Bead> {
        self.by_status
            .get(&status)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.beads.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn update_status(&mut self, id: &str, status: BeadStatus) {
        if let Some(bead) = self.beads.get_mut(id) {
            let old_status = bead.status;
            if old_status != status {
                if let Some(set) = self.by_status.get_mut(&old_status) {
                    set.remove(id);
                    if set.is_empty() {
                        self.by_status.remove(&old_status);
                    }
                }
                self.by_status
                    .entry(status)
                    .or_default()
                    .insert(id.to_string());
                bead.status = status;
            }
        }
    }
}
