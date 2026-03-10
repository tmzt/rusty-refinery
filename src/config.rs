use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;

use crate::git_ops::SubmoduleMap;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read refinery.toml: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse refinery.toml: {0}")]
    Parse(#[from] toml::de::Error),
}

#[derive(Debug, Clone)]
pub struct RefineryConfig {
    pub options: Options,
    pub templates: HashMap<String, AgentTemplate>,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub default_agent: Option<String>,
    pub default_planner: Option<String>,
    pub planning_path: PathBuf,
    pub repos_path: PathBuf,
    pub submodules_path: PathBuf,
    pub repo_root: PathBuf,
    pub redis_url: String,
    pub allow_unsafe_agents: bool,
    pub submodules: SubmoduleMap,
    /// GitHub remote config: None = no remote, Some = auto-add remote on create-submodule.
    pub github_remote: Option<GitHubRemote>,
}

/// GitHub (or compatible) remote configuration for new submodules.
#[derive(Debug, Clone)]
pub struct GitHubRemote {
    /// The remote name (e.g., "github", "origin")
    pub remote_name: String,
    /// Either a GitHub username (expanded to git@github.com:USER/REPO.git)
    /// or a full URL template with {NAME} placeholder.
    pub url_or_account: String,
}

impl GitHubRemote {
    /// Build the remote URL for a given submodule name.
    pub fn url_for(&self, submodule_name: &str) -> String {
        if self.url_or_account.contains("://") || self.url_or_account.contains('@') {
            // It's a URL template — substitute {NAME}
            self.url_or_account.replace("{NAME}", submodule_name)
        } else {
            // It's a GitHub account name
            format!("git@github.com:{}/{submodule_name}.git", self.url_or_account)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentType {
    Claude,
    Gemini,
    Codex,
    Unknown,
}

impl AgentType {
    /// Detect agent type from the command name.
    pub fn from_command(command: &str) -> Self {
        let base = command.rsplit('/').next().unwrap_or(command);
        match base {
            "claude" | "claude-code" => AgentType::Claude,
            "gemini" | "gemini-cli" => AgentType::Gemini,
            "codex" => AgentType::Codex,
            _ => AgentType::Unknown,
        }
    }

    fn from_str_opt(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "claude" => AgentType::Claude,
            "gemini" => AgentType::Gemini,
            "codex" => AgentType::Codex,
            _ => AgentType::Unknown,
        }
    }

    /// Args to enable insecure/yolo mode for this agent.
    pub fn unsafe_args(&self) -> Vec<&'static str> {
        match self {
            AgentType::Claude => vec!["--dangerously-skip-permissions"],
            AgentType::Gemini => vec!["--sandbox=none"],
            AgentType::Codex => vec!["--full-auto"],
            AgentType::Unknown => vec![],
        }
    }

    /// Produce CLI args to configure an MCP server for this agent.
    /// Returns args to add and an optional temp file path that must be kept alive.
    pub fn mcp_args(&self, server_command: &str, server_args: &[&str]) -> (Vec<String>, Option<tempfile::NamedTempFile>) {
        match self {
            AgentType::Claude => {
                // Claude uses --mcp-config <json-file>
                let mut servers = serde_json::Map::new();
                let mut entry = serde_json::Map::new();
                entry.insert(
                    "command".to_string(),
                    serde_json::Value::String(server_command.to_string()),
                );
                if !server_args.is_empty() {
                    entry.insert(
                        "args".to_string(),
                        serde_json::Value::Array(
                            server_args
                                .iter()
                                .map(|a| serde_json::Value::String(a.to_string()))
                                .collect(),
                        ),
                    );
                }
                servers.insert("rusty-refinery".to_string(), serde_json::Value::Object(entry));

                let root = serde_json::json!({ "mcpServers": servers });

                if let Ok(mut tmp) = tempfile::NamedTempFile::new() {
                    use std::io::Write;
                    if serde_json::to_writer_pretty(&mut tmp, &root).is_ok() {
                        let _ = tmp.flush();
                        let path = tmp.path().to_string_lossy().to_string();
                        return (vec!["--mcp-config".to_string(), path], Some(tmp));
                    }
                }
                (vec![], None)
            }
            AgentType::Gemini => {
                // Gemini uses --mcp "command arg1 arg2"
                let mut cmd_str = server_command.to_string();
                for a in server_args {
                    cmd_str.push(' ');
                    cmd_str.push_str(a);
                }
                (vec!["--mcp".to_string(), cmd_str], None)
            }
            AgentType::Codex => {
                // Codex uses --mcp-config <json-file>, same as Claude
                let mut servers = serde_json::Map::new();
                let mut entry = serde_json::Map::new();
                entry.insert(
                    "command".to_string(),
                    serde_json::Value::String(server_command.to_string()),
                );
                if !server_args.is_empty() {
                    entry.insert(
                        "args".to_string(),
                        serde_json::Value::Array(
                            server_args
                                .iter()
                                .map(|a| serde_json::Value::String(a.to_string()))
                                .collect(),
                        ),
                    );
                }
                servers.insert("rusty-refinery".to_string(), serde_json::Value::Object(entry));

                let root = serde_json::json!({ "mcpServers": servers });

                if let Ok(mut tmp) = tempfile::NamedTempFile::new() {
                    use std::io::Write;
                    if serde_json::to_writer_pretty(&mut tmp, &root).is_ok() {
                        let _ = tmp.flush();
                        let path = tmp.path().to_string_lossy().to_string();
                        return (vec!["--mcp-config".to_string(), path], Some(tmp));
                    }
                }
                (vec![], None)
            }
            AgentType::Unknown => (vec![], None),
        }
    }

    /// Produce CLI args to pass a prompt non-interactively.
    pub fn _prompt_args(&self, prompt: &str) -> Vec<String> {
        match self {
            AgentType::Claude => vec!["-p".to_string(), prompt.to_string()],
            AgentType::Gemini => vec!["--prompt".to_string(), prompt.to_string()],
            AgentType::Codex => vec![prompt.to_string()],
            AgentType::Unknown => vec![prompt.to_string()],
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentTemplate {
    pub name: String,
    pub command: String,
    pub agent_type: AgentType,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

// Raw TOML structures for deserialization
#[derive(Deserialize)]
struct RawConfig {
    options: Option<RawOptions>,
    templates: Option<HashMap<String, RawTemplate>>,
}

#[derive(Deserialize)]
struct RawOptions {
    default_agent: Option<String>,
    default_planner: Option<String>,
    repos_path: Option<String>,
    submodules_path: Option<String>,
    /// GitHub account name or full URL template (e.g., "tmzt" or "git@gitlab.com:{NAME}.git")
    github_account: Option<String>,
    /// Remote name for the GitHub remote (default: "github")
    github_remote_name: Option<String>,
}

#[derive(Deserialize)]
struct RawTemplate {
    command: String,
    agent_type: Option<String>,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
}

/// Interpolate `{VAR}` patterns in a string. Resolves from the provided env map
/// first, then falls back to system environment variables.
pub fn interpolate_env(s: &str, env: &HashMap<String, String>) -> String {
    let mut result = s.to_string();
    while let Some(start) = result.find('{') {
        if let Some(end) = result[start..].find('}') {
            let end = start + end;
            let var = &result[start + 1..end];
            let val = env
                .get(var)
                .cloned()
                .or_else(|| std::env::var(var).ok())
                .unwrap_or_default();
            result = format!("{}{}{}", &result[..start], val, &result[end + 1..]);
        } else {
            break;
        }
    }
    result
}

impl RefineryConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let raw: RawConfig = if std::path::Path::new("refinery.toml").exists() {
            let contents = std::fs::read_to_string("refinery.toml")?;
            toml::from_str(&contents)?
        } else {
            RawConfig {
                options: None,
                templates: None,
            }
        };

        let raw_opts = raw.options.unwrap_or(RawOptions {
            default_agent: None,
            default_planner: None,
            repos_path: None,
            submodules_path: None,
            github_account: None,
            github_remote_name: None,
        });

        let planning_path = std::env::var("PLANNING_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./submodules/planning"));

        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1/".to_string());

        let allow_unsafe_agents = std::env::var("ALLOW_UNSAFE_AGENTS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        // Discover parent repo root and first-level submodules
        let repo_root = crate::git_ops::find_repo_root(&planning_path)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let submodules = crate::git_ops::discover_submodules(&repo_root)
            .unwrap_or_default();

        let templates = raw
            .templates
            .unwrap_or_default()
            .into_iter()
            .map(|(name, raw_tmpl)| {
                let agent_type = raw_tmpl
                    .agent_type
                    .as_deref()
                    .map(AgentType::from_str_opt)
                    .unwrap_or_else(|| AgentType::from_command(&raw_tmpl.command));
                let tmpl = AgentTemplate {
                    name: name.clone(),
                    command: raw_tmpl.command,
                    agent_type,
                    args: raw_tmpl.args.unwrap_or_default(),
                    env: raw_tmpl.env.unwrap_or_default(),
                };
                (name, tmpl)
            })
            .collect();

        let repos_path = raw_opts
            .repos_path
            .map(PathBuf::from)
            .unwrap_or_else(|| repo_root.join("repos/submodules"));

        let submodules_path = raw_opts
            .submodules_path
            .map(PathBuf::from)
            .unwrap_or_else(|| repo_root.join("submodules"));

        let github_remote = raw_opts.github_account.map(|acct| GitHubRemote {
            remote_name: raw_opts.github_remote_name.unwrap_or_else(|| "github".to_string()),
            url_or_account: acct,
        });

        Ok(RefineryConfig {
            options: Options {
                default_agent: raw_opts.default_agent,
                default_planner: raw_opts.default_planner,
                planning_path,
                repos_path,
                submodules_path,
                repo_root,
                redis_url,
                allow_unsafe_agents,
                submodules,
                github_remote,
            },
            templates,
        })
    }

    /// Resolve a template by name.
    pub fn resolve_template(&self, name: &str) -> Option<&AgentTemplate> {
        self.templates.get(name)
    }
}
