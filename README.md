# rusty-refinery

An MCP server that orchestrates PRD-to-agent lifecycle: hashes PRD files, deduplicates via Redis, spawns agent processes in isolated git worktrees, and emits events to a Redis stream.

The binary is called `crk`.

## Installation

From git:

```bash
cargo install --git https://github.com/tmzt/rusty-refinery crk
```

From source:

```bash
make install        # installs to ~/.local/bin/crk
```

Or build manually:

```bash
cargo build --release
install -m 755 target/release/crk ~/.local/bin/crk
```

## Running

Stdio mode (default) — MCP server on stdin/stdout:

```
crk
```

Daemon mode — long-lived process listening on a Unix domain socket:

```
crk daemon
```

Proxy mode — connects to the daemon and bridges to stdio:

```
crk proxy
```

Scan planning directory for PRD files and sync to Redis:

```
crk scan
```

Manage submodules (create, list):

```
crk submodule create <NAME>
crk submodule list
```

Invoke MCP tools directly from the CLI:

```
crk tools sync-prd --prd-path <PATH>
crk tools list-beads
crk tools launch-agent --bead-id <ID>
crk tools build-plan --bead-id <ID>
crk tools kill-agent --bead-id <ID>
```

Generate editor config — output MCP config JSON for your editor:

```
crk generate-config <EDITOR> [OPTIONS]
```

Generate shell completions:

```
crk completions bash > ~/.bash_completion.d/crk
crk completions zsh > ~/.zfunc/_crk
```

See [Daemon and Proxy Modes](#daemon-and-proxy-modes) and [Generating Editor Configs](#generating-editor-configs) for details.

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `PLANNING_PATH` | `./submodules/planning` | Path to the planning repo |
| `REDIS_URL` | `redis://127.0.0.1/` | Redis connection URL |
| `ALLOW_UNSAFE_AGENTS` | `false` | Enable unsafe agent variants (see below) |

## Configuration

`crk` reads `refinery.toml` from the working directory at startup.

### Options

```toml
[options]
default_agent = "coder"           # template used when launch_agent omits template name
default_planner = "planner"       # template used by build_plan
repos_path = "./repos/submodules" # where local repos are created (submodule create)
submodules_path = "./submodules"  # where submodules are checked out
github_account = "tmzt"           # GitHub user for remote URLs (optional)
github_remote_name = "github"     # remote name for the GitHub remote (default: "github")
```

The `github_account` field supports three formats:

- **GitHub username**: `"tmzt"` — expands to `git@github.com:tmzt/{NAME}.git`
- **SSH URL template**: `"git@gitlab.com:myorg/{NAME}.git"` — `{NAME}` is replaced with the submodule name
- **HTTPS URL template**: `"https://gitlab.com/myorg/{NAME}.git"`

If `github_account` is omitted, `submodule create` skips adding a remote.

### Templates

Each template defines a command and optional arguments/env. The agent type is auto-detected from the command name (`claude`, `gemini`, `codex`) or set explicitly with `agent_type`.

Template variables `{BEAD_ID}`, `{WORKTREE_PATH}`, and any system environment variable can be used in `args` and `env` values via `{VAR}` interpolation.

```toml
[templates.coder]
command = "claude"
args = ["-p", "Implement the task described in the PRD."]
env = { "BEAD_ID" = "{BEAD_ID}" }
```

No need to add `--dangerously-skip-permissions`, `--sandbox=none`, or `--full-auto` — these are injected automatically when `ALLOW_UNSAFE_AGENTS=true`.

### Convention Over Configuration

`crk` auto-configures agents based on the command name:

| Command | Agent Type | Unsafe Flag | MCP Config | Prompt Flag |
|---|---|---|---|---|
| `claude` | Claude | `--dangerously-skip-permissions` | `--mcp-config <tmpfile>` | `-p` |
| `gemini` | Gemini | `--sandbox=none` | `--mcp <command>` | `--prompt` |
| `codex` | Codex | `--full-auto` | `--mcp-config <tmpfile>` | positional |

Override detection with `agent_type`:

```toml
[templates.my_agent]
command = "/usr/local/bin/my-claude-wrapper"
agent_type = "claude"
args = ["-p", "Do the thing."]
```

### Example: Minimal Configuration

```toml
[options]
default_agent = "coder"
default_planner = "planner"

[templates.coder]
command = "claude"
args = ["-p", "Implement the task described in the PRD."]
env = { "BEAD_ID" = "{BEAD_ID}" }

[templates.planner]
command = "claude"
args = ["-p", "Review and create implementation plan"]
env = { "BEAD_ID" = "{BEAD_ID}" }
```

### Example: Mixed Agents

```toml
[options]
default_agent = "coder"
default_planner = "planner"

[templates.coder]
command = "claude"
args = ["-p", "Implement the task described in the PRD."]
env = { "BEAD_ID" = "{BEAD_ID}" }

[templates.planner]
command = "gemini"
args = ["--prompt", "Review and create implementation plan"]
env = { "BEAD_ID" = "{BEAD_ID}" }

[templates.codex_coder]
command = "codex"
args = ["Implement the task described in the PRD."]
env = { "BEAD_ID" = "{BEAD_ID}" }
```

### Unsafe / YOLO Mode

Set `ALLOW_UNSAFE_AGENTS=true` and the refinery automatically adds the correct insecure flag for each agent type. No need for separate unsafe templates.

```bash
export ALLOW_UNSAFE_AGENTS=true
crk daemon &
```

### Auto MCP Server Injection

When using `crk plan`, the refinery automatically injects itself as an MCP server into the agent. This means the planner can call refinery tools (`sync_prd`, `list_beads`, etc.) during planning. The injection method is agent-specific:

- **Claude/Codex**: writes a temp JSON file and passes `--mcp-config <path>`
- **Gemini**: passes `--mcp "crk proxy"`

## Interacting with the Planning Agent

### Typical Workflow

1. **Sync a PRD** to register it as a bead:

   The MCP client calls `sync_prd` with the path to a PRD file. The refinery hashes the file, checks Redis for duplicates, and registers a new bead.

   ```json
   {
     "method": "tools/call",
     "params": {
       "name": "sync_prd",
       "arguments": { "prd_path": "./submodules/planning/prds/feature-x.md" }
     }
   }
   ```

   Or from the CLI:

   ```bash
   crk tools sync-prd --prd-path ./submodules/planning/prds/feature-x.md
   ```

   Response: `Bead registered: a1b2c3d4e5f6...`

2. **Build a plan** for the bead:

   ```bash
   crk tools build-plan --bead-id a1b2c3d4e5f6...
   ```

   This creates a worktree `wt-<bead_id>-plan`, spawns the planner template inside it, and begins monitoring the process. The planner agent reads the PRD and writes its output (e.g. `PLAN.md`) into the worktree.

3. **Check status** while the planner runs:

   ```bash
   crk tools list-beads
   ```

   ```
   ID           | Status  | PRD                                      | PID
   -------------|---------|------------------------------------------|------
   a1b2c3d4e5f6 | RUNNING | ./submodules/planning/prds/feature-x.md  | 48210
   ```

4. **Launch the coder** once the plan is ready:

   ```bash
   crk tools launch-agent --bead-id a1b2c3d4e5f6...
   ```

   This creates a separate worktree `wt-<bead_id>` and spawns the default coder template.

5. **Stop an agent** if needed:

   ```bash
   crk tools kill-agent --bead-id a1b2c3d4e5f6...
   ```

### Monitoring via Redis

Events stream to `beads:events` in real time. You can tail them with:

```bash
redis-cli XREAD BLOCK 0 STREAMS beads:events $
```

Or read the full history:

```bash
redis-cli XRANGE beads:events - +
```

## MCP Tools

| Tool | CLI Equivalent | Description |
|---|---|---|
| `sync_prd` | `crk tools sync-prd` | Hash a PRD file and register a new bead. Deduplicates against Redis — skips if already COMPLETE. |
| `launch_agent` | `crk tools launch-agent` | Launch an agent from a template in an isolated git worktree. |
| `build_plan` | `crk tools build-plan` | Trigger the planner agent for a bead. |
| `list_beads` | `crk tools list-beads` | List all beads and their current status. |
| `kill_agent` | `crk tools kill-agent` | Stop a running agent process for a bead. |

## Git Hooks — Automatic PRD Detection

`crk` can install a post-commit hook in the planning repo that automatically syncs changed PRD files whenever they are committed.

### Installing the Hook

```bash
crk hook install
```

Or with a custom planning path:

```bash
crk hook install --planning-path /path/to/planning
```

This writes a `post-commit` hook in the planning repo's `.git/hooks/` directory. On each commit, the hook:

1. Checks if `RUSTY_REFINERY_SKIP_HOOK=1` is set (to avoid double-triggering from agent commits)
2. Runs `git diff-tree` to find changed files in `prds/*.md`
3. Hashes each changed PRD and registers it as a new bead in Redis

### Uninstalling the Hook

```bash
crk hook uninstall
```

Only removes hooks installed by `crk`. Pre-existing hooks are left untouched.

### Agent Commit Safety

When the refinery spawns an agent process, it automatically sets `RUSTY_REFINERY_SKIP_HOOK=1` in the agent's environment. This prevents the hook from re-syncing PRDs that the agent itself committed, avoiding infinite loops.

## Submodule Management

`crk` discovers first-level submodules from the parent repo's `.gitmodules` and maps PRD subdirectories to their checkout paths. This means a PRD at `prds/rusty-genius/feature.md` automatically targets the `rusty-genius` submodule — agents create worktrees in the right repo.

### Creating Submodules

```bash
crk submodule create my-new-module
```

This:
1. Creates a local git repo at `{repos_path}/my-new-module` with a `main` branch
2. Adds a GitHub remote (if `github_account` is set in `refinery.toml`)
3. Registers it as a submodule at `{submodules_path}/my-new-module`
4. Creates a `prds/my-new-module/` directory in the planning repo

### Listing Submodules

```bash
crk submodule list
```

Shows all first-level submodules with their checkout paths.

### Scanning for PRDs

```bash
crk scan
```

Walks the planning directory's `prds/` tree, discovers all `.md` files organized by submodule subdirectory, and syncs each one to Redis.

## Event Sourcing

All lifecycle events are emitted to the Redis stream `beads:events` via XADD:

- `NEW_BEAD` — PRD hashed and registered
- `AGENT_SPAWN` — process started from template
- `SIGCHLD` — agent process exited (includes exit code)
- `HEARTBEAT` — periodic status update for running agents

## Daemon and Proxy Modes

`crk` supports three execution modes, similar to Docker's client/daemon architecture:

| Mode | Command | Description |
|---|---|---|
| stdio | `crk` | MCP server on stdin/stdout (default) |
| daemon | `crk daemon [SOCKET]` | Listen on a Unix domain socket |
| proxy | `crk proxy [SOCKET]` | Connect to daemon UDS, bridge to stdio |

The default socket path is `/tmp/crk.sock`.

**Why not just stdio?** Agent subprocesses are designed to survive MCP disconnections. If an MCP client (like Zed or Claude Code) launches `crk` directly, closing the client kills the refinery and all its child agents. The daemon/proxy split solves this — the daemon and its agents live independently, and proxy sessions can come and go.

### Starting the Daemon

```bash
PLANNING_PATH=/home/user/project/submodules/planning \
REDIS_URL=redis://127.0.0.1/ \
  crk daemon &
```

With a custom socket path:

```bash
crk daemon /run/user/1000/refinery.sock &
```

With YOLO mode:

```bash
PLANNING_PATH=/home/user/project/submodules/planning \
REDIS_URL=redis://127.0.0.1/ \
ALLOW_UNSAFE_AGENTS=true \
  crk daemon &
```

Or as a systemd user service (`~/.config/systemd/user/crk.service`):

```ini
[Unit]
Description=Rusty Refinery MCP Server
After=redis.service

[Service]
ExecStart=/home/user/project/submodules/rusty-refinery/target/release/crk daemon
Environment=PLANNING_PATH=/home/user/project/submodules/planning
Environment=REDIS_URL=redis://127.0.0.1/
Restart=on-failure

[Install]
WantedBy=default.target
```

### Connecting via Proxy

The proxy mode connects to the daemon's UDS and bridges it to stdio, making it transparent to any MCP client:

```bash
crk proxy
```

The client sees a normal stdio MCP server. The proxy forwards everything to the long-lived daemon. When the proxy exits, the daemon and its agents keep running.

## Generating Editor Configs

The `generate-config` subcommand outputs MCP configuration JSON for your editor. Supported editors: `vscode`, `zed`, `cursor`, `claude`, `windsurf`, `antigravity`, `zen`.

```bash
crk generate-config <EDITOR> [OPTIONS]
```

Options:

| Flag | Description |
|---|---|
| `--proxy` | Use proxy mode (recommended with daemon) |
| `--socket <PATH>` | Custom socket path for proxy mode |
| `--binary <PATH>` | Override binary path in output |
| `--planning-path <PATH>` | Set `PLANNING_PATH` in env |
| `--redis-url <URL>` | Set `REDIS_URL` in env |
| `--allow-unsafe` | Set `ALLOW_UNSAFE_AGENTS=true` in env |

### Examples

Generate a Zed config that connects via proxy to the daemon:

```bash
crk generate-config zed --proxy
```

```json
{
  "context_servers": {
    "rusty-refinery": {
      "command": {
        "path": "/path/to/crk",
        "args": ["proxy"],
        "env": {}
      },
      "settings": {}
    }
  }
}
```

Generate a VS Code config with environment and custom socket:

```bash
crk generate-config vscode --proxy \
  --planning-path /home/user/project/planning \
  --redis-url redis://10.0.0.5/ \
  --socket /run/user/1000/refinery.sock
```

```json
{
  "servers": {
    "rusty-refinery": {
      "command": "/path/to/crk",
      "args": ["proxy", "/run/user/1000/refinery.sock"],
      "env": {
        "PLANNING_PATH": "/home/user/project/planning",
        "REDIS_URL": "redis://10.0.0.5/"
      }
    }
  }
}
```

Generate a Claude Desktop config (direct stdio, no daemon):

```bash
crk generate-config claude
```

```json
{
  "mcpServers": {
    "rusty-refinery": {
      "command": "/path/to/crk"
    }
  }
}
```

Generate a Cursor config with YOLO mode:

```bash
crk generate-config cursor --proxy --allow-unsafe
```

The output includes a comment showing where to save the file. Redirect to create the config directly:

```bash
crk generate-config vscode --proxy > .vscode/mcp.json
```

## Editor Integration

Start the daemon first, then configure your editor to launch the proxy. The proxy is lightweight and stateless — it's safe for the editor to start and stop it at will.

### Zed

```bash
crk generate-config zed --proxy > .zed/settings.json
```

Or merge manually into your existing settings. See [Zed MCP docs](https://zed.dev/docs/assistant/context-servers) for details.

### VS Code

```bash
mkdir -p .vscode && crk generate-config vscode --proxy > .vscode/mcp.json
```

### Cursor

```bash
mkdir -p .cursor && crk generate-config cursor --proxy > .cursor/mcp.json
```

### Claude Desktop

```bash
crk generate-config claude > ~/.config/claude/claude_desktop_config.json
```

Note: Claude Desktop manages the server lifecycle itself. Use direct stdio mode (no `--proxy`) if the desktop app is always running, or use `--proxy` if you want agents to persist independently.

### Remote Setup (SSH)

For editors with SSH remote development (Zed, VS Code, Cursor), the daemon runs on the remote host and the editor launches the proxy there.

Prepare the remote host:

```bash
ssh your-remote-host

# Build
cd /home/ubuntu/project/submodules/rusty-refinery
cargo build --release

# Start Redis
redis-server --daemonize yes

# Start the daemon
PLANNING_PATH=/home/ubuntu/project/submodules/planning \
REDIS_URL=redis://127.0.0.1/ \
  ./target/release/crk daemon &
```

Then generate the config using the remote binary path:

```bash
crk generate-config zed --proxy \
  --binary /home/ubuntu/project/submodules/rusty-refinery/target/release/crk
```

All paths in the generated config must be absolute on the remote filesystem. The editor launches the proxy over SSH; the proxy connects to the daemon's socket locally.

### Verifying the Connection

Once configured, restart your editor or reload the project. The refinery's tools (`sync_prd`, `launch_agent`, `build_plan`, `list_beads`, `kill_agent`) should appear in the available tools. If they don't:

1. Verify the daemon is running: `pgrep -f 'crk daemon'`
2. Test the proxy: `echo '{}' | crk proxy`
3. Check your editor's log output for MCP connection errors
4. Verify Redis is reachable from the host where the daemon runs

## License

MIT
