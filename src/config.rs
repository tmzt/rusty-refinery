use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;

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
    pub redis_url: String,
    pub allow_unsafe_agents: bool,
}

#[derive(Debug, Clone)]
pub struct AgentTemplate {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub unsafe_variant: Option<String>,
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
}

#[derive(Deserialize)]
struct RawTemplate {
    command: String,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    unsafe_variant: Option<String>,
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
        });

        let planning_path = std::env::var("PLANNING_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./submodules/planning"));

        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1/".to_string());

        let allow_unsafe_agents = std::env::var("ALLOW_UNSAFE_AGENTS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let templates = raw
            .templates
            .unwrap_or_default()
            .into_iter()
            .map(|(name, raw_tmpl)| {
                let tmpl = AgentTemplate {
                    name: name.clone(),
                    command: raw_tmpl.command,
                    args: raw_tmpl.args.unwrap_or_default(),
                    env: raw_tmpl.env.unwrap_or_default(),
                    unsafe_variant: raw_tmpl.unsafe_variant,
                };
                (name, tmpl)
            })
            .collect();

        Ok(RefineryConfig {
            options: Options {
                default_agent: raw_opts.default_agent,
                default_planner: raw_opts.default_planner,
                planning_path,
                redis_url,
                allow_unsafe_agents,
            },
            templates,
        })
    }

    /// Resolve the template to use, considering unsafe variants.
    pub fn resolve_template(&self, name: &str) -> Option<&AgentTemplate> {
        let tmpl = self.templates.get(name)?;
        if self.options.allow_unsafe_agents {
            if let Some(ref unsafe_name) = tmpl.unsafe_variant {
                if let Some(unsafe_tmpl) = self.templates.get(unsafe_name) {
                    return Some(unsafe_tmpl);
                }
            }
        }
        Some(tmpl)
    }
}
