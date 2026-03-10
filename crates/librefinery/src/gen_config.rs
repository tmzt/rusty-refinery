use std::path::PathBuf;

use clap::ValueEnum;

#[derive(Debug, Clone, ValueEnum)]
pub enum Editor {
    /// VS Code / VS Code Insiders
    Vscode,
    /// Zed editor
    Zed,
    /// Cursor editor
    Cursor,
    /// Claude Desktop app
    Claude,
    /// Windsurf (Codeium)
    Windsurf,
    /// Antigravity
    Antigravity,
    /// Zen
    Zen,
}

impl Editor {
    pub fn config_path_hint(&self) -> &'static str {
        match self {
            Editor::Vscode => ".vscode/mcp.json",
            Editor::Zed => ".zed/settings.json (or ~/.config/zed/settings.json)",
            Editor::Cursor => ".cursor/mcp.json",
            Editor::Claude => {
                "~/.config/claude/claude_desktop_config.json (Linux) or ~/Library/Application Support/Claude/claude_desktop_config.json (macOS)"
            }
            Editor::Windsurf => ".windsurf/mcp.json",
            Editor::Antigravity => ".antigravity/mcp.json",
            Editor::Zen => ".zen/mcp.json",
        }
    }

    /// Relative config path for --save (resolved against git root).
    pub fn config_rel_path(&self) -> &'static str {
        match self {
            Editor::Vscode => ".vscode/mcp.json",
            Editor::Zed => ".zed/settings.json",
            Editor::Cursor => ".cursor/mcp.json",
            Editor::Claude => ".claude/claude_desktop_config.json",
            Editor::Windsurf => ".windsurf/mcp.json",
            Editor::Antigravity => ".antigravity/mcp.json",
            Editor::Zen => ".zen/mcp.json",
        }
    }
}

/// Find the top-level git repository root (the one with an actual .git directory,
/// not a submodule whose .git is a file).
fn find_git_root() -> Result<PathBuf, String> {
    let mut dir = std::env::current_dir().map_err(|e| format!("cannot get cwd: {e}"))?;
    loop {
        let git_path = dir.join(".git");
        if git_path.is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err("not in a git repo (no .git directory found)".to_string());
        }
    }
}

/// Deep-merge two JSON objects. Values in `patch` override values in `base`.
/// Objects are merged recursively; other types are replaced.
fn merge_json(base: &mut serde_json::Value, patch: &serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(patch_map)) => {
            for (key, patch_val) in patch_map {
                let entry = base_map
                    .entry(key.clone())
                    .or_insert(serde_json::Value::Null);
                merge_json(entry, patch_val);
            }
        }
        (base, patch) => {
            *base = patch.clone();
        }
    }
}

/// Save config to the editor's config path relative to the git root.
/// If the file already exists, merges the rusty-refinery entry into it.
/// With --replace-file, overwrites the file entirely instead of merging.
/// Returns the absolute path written.
pub fn save(editor: &Editor, content: &str, force: bool) -> Result<PathBuf, String> {
    let root = find_git_root()?;
    let rel = editor.config_rel_path();
    let dest = root.join(rel);

    let output = if dest.exists() && !force {
        // Merge into existing config
        let existing = std::fs::read_to_string(&dest)
            .map_err(|e| format!("failed to read {}: {e}", dest.display()))?;
        let mut existing_json: serde_json::Value = serde_json::from_str(&existing)
            .map_err(|e| format!("failed to parse {}: {e}", dest.display()))?;
        let new_json: serde_json::Value = serde_json::from_str(content)
            .map_err(|e| format!("failed to parse generated config: {e}"))?;
        merge_json(&mut existing_json, &new_json);
        serde_json::to_string_pretty(&existing_json)
            .map_err(|e| format!("failed to serialize merged config: {e}"))?
    } else {
        content.to_string()
    };

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }

    std::fs::write(&dest, &output)
        .map_err(|e| format!("failed to write {}: {e}", dest.display()))?;

    Ok(dest)
}

pub struct GenerateOptions {
    pub editor: Editor,
    pub binary_path: PathBuf,
    pub proxy: bool,
    pub socket_path: Option<String>,
    pub planning_path: Option<String>,
    pub redis_url: Option<String>,
    pub allow_unsafe: bool,
}

pub fn generate(opts: &GenerateOptions) -> String {
    let binary = opts.binary_path.to_string_lossy();

    let mut args: Vec<String> = Vec::new();
    if opts.proxy {
        args.push("proxy".to_string());
        if let Some(ref sock) = opts.socket_path {
            args.push(sock.clone());
        }
    }

    let mut env = serde_json::Map::new();
    if let Some(ref p) = opts.planning_path {
        env.insert(
            "PLANNING_PATH".to_string(),
            serde_json::Value::String(p.clone()),
        );
    }
    if let Some(ref r) = opts.redis_url {
        env.insert(
            "REDIS_URL".to_string(),
            serde_json::Value::String(r.clone()),
        );
    }
    if opts.allow_unsafe {
        env.insert(
            "ALLOW_UNSAFE_AGENTS".to_string(),
            serde_json::Value::String("true".to_string()),
        );
    }

    match opts.editor {
        Editor::Zed => generate_zed(&binary, &args, &env),
        _ => generate_standard(&binary, &args, &env, &opts.editor),
    }
}

/// Standard MCP config format used by VS Code, Cursor, Windsurf, Claude Desktop, etc.
fn generate_standard(
    binary: &str,
    args: &[String],
    env: &serde_json::Map<String, serde_json::Value>,
    editor: &Editor,
) -> String {
    let mut server = serde_json::Map::new();
    server.insert(
        "command".to_string(),
        serde_json::Value::String(binary.to_string()),
    );
    if !args.is_empty() {
        server.insert(
            "args".to_string(),
            serde_json::Value::Array(args.iter().map(|a| serde_json::Value::String(a.clone())).collect()),
        );
    }
    if !env.is_empty() {
        server.insert(
            "env".to_string(),
            serde_json::Value::Object(env.clone()),
        );
    }

    // Claude Desktop uses a slightly different top-level key
    let wrapper_key = match editor {
        Editor::Claude => "mcpServers",
        _ => "servers",
    };

    let mut servers = serde_json::Map::new();
    servers.insert("rusty-refinery".to_string(), serde_json::Value::Object(server));

    let mut root = serde_json::Map::new();
    root.insert(wrapper_key.to_string(), serde_json::Value::Object(servers));

    serde_json::to_string_pretty(&serde_json::Value::Object(root)).unwrap()
}

/// Zed uses context_servers with a nested command object containing path/args/env.
fn generate_zed(
    binary: &str,
    args: &[String],
    env: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let mut command = serde_json::Map::new();
    command.insert(
        "path".to_string(),
        serde_json::Value::String(binary.to_string()),
    );
    if !args.is_empty() {
        command.insert(
            "args".to_string(),
            serde_json::Value::Array(args.iter().map(|a| serde_json::Value::String(a.clone())).collect()),
        );
    }
    command.insert(
        "env".to_string(),
        serde_json::Value::Object(env.clone()),
    );

    let mut server = serde_json::Map::new();
    server.insert(
        "command".to_string(),
        serde_json::Value::Object(command),
    );
    server.insert(
        "settings".to_string(),
        serde_json::Value::Object(serde_json::Map::new()),
    );

    let mut servers = serde_json::Map::new();
    servers.insert("rusty-refinery".to_string(), serde_json::Value::Object(server));

    let mut root = serde_json::Map::new();
    root.insert(
        "context_servers".to_string(),
        serde_json::Value::Object(servers),
    );

    serde_json::to_string_pretty(&serde_json::Value::Object(root)).unwrap()
}
