# rusty-refinery

An MCP server that orchestrates PRD-to-agent lifecycle: hashes PRD files, deduplicates via Redis, spawns agent processes in isolated git worktrees, and emits events to a Redis stream.

## Building

```
cargo build --release
```

## Running

Stdio mode (default) — MCP server on stdin/stdout:

```
rusty-refinery
```

Daemon mode — long-lived process listening on a Unix domain socket:

```
rusty-refinery --daemon
```

Proxy mode — connects to the daemon and bridges to stdio:

```
rusty-refinery --proxy
```

See [Daemon and Proxy Modes](#daemon-and-proxy-modes) for details.

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `PLANNING_PATH` | `./submodules/planning` | Path to the planning repo |
| `REDIS_URL` | `redis://127.0.0.1/` | Redis connection URL |
| `ALLOW_UNSAFE_AGENTS` | `false` | Enable unsafe agent variants (see below) |

## Configuration

rusty-refinery reads `refinery.toml` from the working directory at startup.

### Options

```toml
[options]
default_agent = "coder"       # template used when launch_agent omits template name
default_planner = "planner"   # template used by build_plan
```

### Templates

Each template defines a command, arguments, environment variables, and an optional unsafe variant.

Template variables `{BEAD_ID}` and `{WORKTREE_PATH}` are substituted at launch time in both `args` and `env` values.

```toml
[templates.coder]
command = "claude"
args = ["--dangerously-skip-permissions"]
env = { "BEAD_ID" = "{BEAD_ID}" }
unsafe_variant = "coder_unsafe"

[templates.planner]
command = "claude"
args = ["--dangerously-skip-permissions", "-p", "Review and create implementation plan"]
env = { "BEAD_ID" = "{BEAD_ID}", "WORKTREE" = "{WORKTREE_PATH}" }
```

### Example: Standard Configuration

A minimal safe configuration with a coder and planner:

```toml
[options]
default_agent = "coder"
default_planner = "planner"

[templates.coder]
command = "claude"
args = ["--dangerously-skip-permissions"]
env = { "BEAD_ID" = "{BEAD_ID}" }

[templates.planner]
command = "claude"
args = ["--dangerously-skip-permissions", "-p", "Review and create implementation plan"]
env = { "BEAD_ID" = "{BEAD_ID}" }
```

### Example: Unsafe / YOLO Configuration

This configuration includes unsafe agent variants that bypass additional safety checks. These variants are **only used when `ALLOW_UNSAFE_AGENTS=true` is set in the environment**. Without it, the standard variant is always selected even if `unsafe_variant` is defined.

```toml
[options]
default_agent = "coder"
default_planner = "planner"

[templates.coder]
command = "claude"
args = ["--dangerously-skip-permissions"]
env = { "BEAD_ID" = "{BEAD_ID}" }
unsafe_variant = "coder_unsafe"

[templates.coder_unsafe]
command = "claude"
args = ["--dangerously-skip-permissions", "--unsafe"]
env = { "BEAD_ID" = "{BEAD_ID}" }

[templates.planner]
command = "claude"
args = ["--dangerously-skip-permissions", "-p", "Review and create implementation plan"]
env = { "BEAD_ID" = "{BEAD_ID}" }
unsafe_variant = "planner_unsafe"

[templates.planner_unsafe]
command = "claude"
args = ["--dangerously-skip-permissions", "--unsafe", "-p", "Review and create implementation plan"]
env = { "BEAD_ID" = "{BEAD_ID}" }
```

To enable unsafe variants:

```bash
export ALLOW_UNSAFE_AGENTS=true
cargo run
```

When `ALLOW_UNSAFE_AGENTS` is unset or any value other than `true`/`1`, the refinery will always resolve to the safe template regardless of `unsafe_variant` being configured. If an MCP client requests an unsafe template directly while the environment gate is off, a "Security Policy Violation" is the expected behavior.

## Planning Agent Configuration

The `build_plan` tool spawns whichever template is set as `default_planner`. Below are configurations for two popular CLI agents.

### Claude Code as Planner

```toml
[options]
default_planner = "planner"

[templates.planner]
command = "claude"
args = [
    "--dangerously-skip-permissions",
    "-p",
    "You are an architectural planner. Read the PRD at the PLANNING_PATH, analyze the codebase in this worktree, and produce a step-by-step implementation plan in PLAN.md. Do not write code."
]
env = { "BEAD_ID" = "{BEAD_ID}", "PLANNING_PATH" = "{WORKTREE_PATH}" }
```

Claude Code uses `-p` to pass an initial prompt non-interactively. The agent runs in the worktree directory and has full file access to analyze the codebase.

### Gemini CLI as Planner

```toml
[options]
default_planner = "planner"

[templates.planner]
command = "gemini"
args = [
    "--prompt",
    "You are an architectural planner. Read the PRD at the PLANNING_PATH, analyze the codebase in this worktree, and produce a step-by-step implementation plan in PLAN.md. Do not write code."
]
env = { "BEAD_ID" = "{BEAD_ID}", "PLANNING_PATH" = "{WORKTREE_PATH}" }
```

Gemini CLI uses `--prompt` for non-interactive execution. Ensure `gemini` is installed and authenticated (`gemini auth login`) before use.

### Mixed Agent Configuration

You can use different agents for different roles. For example, Gemini for planning and Claude for coding:

```toml
[options]
default_agent = "coder"
default_planner = "planner"

[templates.coder]
command = "claude"
args = ["--dangerously-skip-permissions"]
env = { "BEAD_ID" = "{BEAD_ID}" }

[templates.planner]
command = "gemini"
args = [
    "--prompt",
    "Read the PRD and produce a detailed implementation plan in PLAN.md."
]
env = { "BEAD_ID" = "{BEAD_ID}" }
```

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

   Response: `Bead registered: a1b2c3d4e5f6...`

2. **Build a plan** for the bead:

   ```json
   {
     "method": "tools/call",
     "params": {
       "name": "build_plan",
       "arguments": { "bead_id": "a1b2c3d4e5f6..." }
     }
   }
   ```

   This creates a worktree `wt-<bead_id>-plan`, spawns the planner template inside it, and begins monitoring the process. The planner agent reads the PRD and writes its output (e.g. `PLAN.md`) into the worktree.

3. **Check status** while the planner runs:

   ```json
   {
     "method": "tools/call",
     "params": { "name": "list_beads" }
   }
   ```

   ```
   ID           | Status  | PRD                                      | PID
   -------------|---------|------------------------------------------|------
   a1b2c3d4e5f6 | RUNNING | ./submodules/planning/prds/feature-x.md  | 48210
   ```

4. **Launch the coder** once the plan is ready:

   ```json
   {
     "method": "tools/call",
     "params": {
       "name": "launch_agent",
       "arguments": { "bead_id": "a1b2c3d4e5f6..." }
     }
   }
   ```

   This creates a separate worktree `wt-<bead_id>` and spawns the default coder template.

5. **Stop an agent** if needed:

   ```json
   {
     "method": "tools/call",
     "params": {
       "name": "kill_agent",
       "arguments": { "bead_id": "a1b2c3d4e5f6..." }
     }
   }
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

| Tool | Description |
|---|---|
| `sync_prd` | Hash a PRD file and register a new bead. Deduplicates against Redis — skips if already COMPLETE. |
| `launch_agent` | Launch an agent from a template in an isolated git worktree. |
| `build_plan` | Trigger the planner agent for a bead. |
| `list_beads` | List all beads and their current status. |
| `kill_agent` | Stop a running agent process for a bead. |

## Event Sourcing

All lifecycle events are emitted to the Redis stream `beads:events` via XADD:

- `NEW_BEAD` — PRD hashed and registered
- `AGENT_SPAWN` — process started from template
- `SIGCHLD` — agent process exited (includes exit code)
- `HEARTBEAT` — periodic status update for running agents

## Daemon and Proxy Modes

rusty-refinery supports three execution modes, similar to Docker's client/daemon architecture:

| Mode | Command | Description |
|---|---|---|
| stdio | `rusty-refinery` | MCP server on stdin/stdout (default) |
| daemon | `rusty-refinery --daemon [SOCKET]` | Listen on a Unix domain socket |
| proxy | `rusty-refinery --proxy [SOCKET]` | Connect to daemon UDS, bridge to stdio |

The default socket path is `/tmp/rusty-refinery.sock`.

**Why not just stdio?** Agent subprocesses are designed to survive MCP disconnections. If an MCP client (like Zed or Claude Code) launches rusty-refinery directly, closing the client kills the refinery and all its child agents. The daemon/proxy split solves this — the daemon and its agents live independently, and proxy sessions can come and go.

### Starting the Daemon

```bash
PLANNING_PATH=/home/user/project/submodules/planning \
REDIS_URL=redis://127.0.0.1/ \
  rusty-refinery --daemon &
```

With a custom socket path:

```bash
rusty-refinery --daemon /run/user/1000/refinery.sock &
```

With YOLO mode:

```bash
PLANNING_PATH=/home/user/project/submodules/planning \
REDIS_URL=redis://127.0.0.1/ \
ALLOW_UNSAFE_AGENTS=true \
  rusty-refinery --daemon &
```

Or as a systemd user service (`~/.config/systemd/user/rusty-refinery.service`):

```ini
[Unit]
Description=Rusty Refinery MCP Server
After=redis.service

[Service]
ExecStart=/home/user/project/submodules/rusty-refinery/target/release/rusty-refinery --daemon
Environment=PLANNING_PATH=/home/user/project/submodules/planning
Environment=REDIS_URL=redis://127.0.0.1/
Restart=on-failure

[Install]
WantedBy=default.target
```

### Connecting via Proxy

The proxy mode connects to the daemon's UDS and bridges it to stdio, making it transparent to any MCP client:

```bash
rusty-refinery --proxy
```

The client sees a normal stdio MCP server. The proxy forwards everything to the long-lived daemon. When the proxy exits, the daemon and its agents keep running.

## Zed Editor Integration

rusty-refinery can be used as an MCP server from [Zed](https://zed.dev/) via its context server support. Zed launches the **proxy**, which connects to the already-running **daemon**.

### Local Zed Configuration

Start the daemon first, then configure Zed (`~/.config/zed/settings.json` or project `.zed/settings.json`):

```json
{
  "context_servers": {
    "rusty-refinery": {
      "command": {
        "path": "/path/to/rusty-refinery",
        "args": ["--proxy"]
      }
    }
  }
}
```

With a custom socket path:

```json
{
  "context_servers": {
    "rusty-refinery": {
      "command": {
        "path": "/path/to/rusty-refinery",
        "args": ["--proxy", "/run/user/1000/refinery.sock"]
      }
    }
  }
}
```

### Remote Setup (SSH)

When Zed connects to a remote machine via SSH, it runs MCP commands on the remote host. The daemon must already be running there.

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
  /home/ubuntu/project/submodules/rusty-refinery/target/release/rusty-refinery --daemon &
```

Then in your project's `.zed/settings.json`:

```json
{
  "context_servers": {
    "rusty-refinery": {
      "command": {
        "path": "/home/ubuntu/project/submodules/rusty-refinery/target/release/rusty-refinery",
        "args": ["--proxy"]
      }
    }
  }
}
```

All paths must be absolute on the remote filesystem. Zed launches the proxy on the remote host over SSH; the proxy connects to the daemon's socket locally.

### Verifying the Connection

Once configured, restart Zed or reload the project. The refinery's tools (`sync_prd`, `launch_agent`, `build_plan`, `list_beads`, `kill_agent`) should appear in the assistant panel's available tools. If they don't:

1. Verify the daemon is running: `pgrep -f 'rusty-refinery --daemon'`
2. Test the proxy: `echo '{}' | rusty-refinery --proxy`
3. Check Zed's log output (`View > Toggle Log`) for MCP connection errors
4. Verify Redis is reachable from the host where the daemon runs

## License

MIT
