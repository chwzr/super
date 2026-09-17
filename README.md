# SUPER

A fast terminal coding agent that keeps the interface simple and gives you
control of the model, tools, sessions, and automation.

SUPER has 41 built-in providers, including OpenAI Codex, Cursor, Anthropic,
Google, OpenRouter, Bedrock, and GitHub Copilot. You can also add
OpenAI-compatible providers. SUPER is built in Rust and based on
[Pi](https://github.com/earendil-works/pi).

## Why SUPER

- **Start quickly.** The native terminal interface reaches its first warm frame
  in about 5 ms.
- **Keep your work.** Resume, branch, compact, import, and export persistent
  sessions.
- **Use your preferred model.** Choose from more than 1,000 catalog models or
  add your own compatible provider.
- **Automate long tasks.** Run scheduled loops, measured autoresearch, parallel
  subagents, and dynamic workflows.
- **Connect your tools.** Use local and remote MCP servers, including OAuth
  servers.
- **Build on it.** Embed SUPER through Rust, Python, TypeScript, WebAssembly,
  JSONL RPC, or WebSocket RPC.

SUPER uses four focused tools by default: `read`, `write`, `edit`, and `bash`.
Catppuccin Mocha is the default dark theme.

## Install

macOS and Linux:

```bash
curl -LsSf https://raw.githubusercontent.com/chwzr/super/main/install.sh | sh
```

Windows, including ARM64:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://raw.githubusercontent.com/chwzr/super/main/install.ps1 | iex"
```

The installer selects the correct release, verifies its SHA-256 checksum, and
installs `super` in your user binary directory. On Linux, if the glibc
release needs a newer `GLIBC_*` version than the system provides, it
automatically installs the matching musl release.

Update later with:

```bash
super update
```

## Start in two commands

Sign in with a ChatGPT subscription and open SUPER:

```bash
super login openai-codex
super
```

For a server or SSH session:

```bash
super login openai-codex --device-auth
```

Anthropic login and credential import are also available:

```bash
super login anthropic
super auth import
```

Use the interactive terminal or run one task:

```bash
super "explain this repository"
super -p "summarize the current changes"
cat error.log | super -p "find the cause"
```

## Work in the terminal

- Type `/` to find commands.
- Type `@` to find and attach files.
- Type `!command` to run a shell command.
- Press `Shift+Tab` to change reasoning effort.
- Press `Esc` or `Ctrl+C` to stop active work.
- Press `Ctrl+R` to expand or collapse long tool results.
- Press `Ctrl+D` on an empty input to exit.
- Use the Up arrow to restore earlier prompts.

SUPER renders bold Markdown, cyan underlined terminal links, bare web links, and
syntax colors for fenced code.

Send a new instruction while the agent works. Press `Enter` to steer the
current task, or `Alt+Enter` to queue the instruction for later.

Useful commands include `/login`, `/model`, `/mcp`, `/compact`, `/resume`,
`/loop`, `/autoresearch`, `/jobs`, `/provider`, `/export`, `/fast`, `/update`,
`/settings`, and `/hotkeys`.

Use `/fast` to toggle the low-latency tier for a supported provider. This
setting applies only to the current session, and provider costs can increase.
Use `/update` to update the installed SUPER binary.

## Continue work from another agent

Run `/resume` to continue a SUPER, Pi, Claude Code, or OpenAI Codex session.
The picker starts with sessions from the current working directory. Press
`Ctrl+G` to switch between project and global results.

## Automate long tasks

### Loop and autoresearch

Use a loop for repeated work. With no limit, it runs until the goal is complete
or you stop it:

```text
/loop make the parser tests pass
```

Put an interval before the goal to wait between turns. The first turn starts
immediately. Compound intervals support days, hours, minutes, seconds,
milliseconds, microseconds, and nanoseconds:

```text
/loop 15m check the deployment and fix new errors
/loop 2d4h review dependency updates
```

Use `--iterations` for a fixed number of turns:

```text
/loop make the parser tests pass --iterations 8
```

Autoresearch establishes a baseline, tests one small change at a time, keeps
improvements, and reverts regressions. It is also unlimited by default:

```text
/autoresearch reduce Markdown render time; verify with the existing benchmark
/autoresearch reduce Markdown render time; verify with the existing benchmark --iterations 20
```

A loop interval and `--iterations` are mutually exclusive. Autoresearch does
not accept an interval. The maximum explicit iteration limit is 100.

Each job branches from the current conversation into a persistent SUPER
session. Run `/jobs`, `/loop` without a goal, or `/autoresearch` without a goal
to manage jobs.

| Key | Action |
| --- | --- |
| Up or Down | Select a job or scroll its latest result |
| Enter or Right | Open the selected job |
| `p` | Pause or resume between iterations |
| `x` | Stop the selected job |
| Escape or Left | Return or close the view |

### Subagents

Subagents let one task branch into focused child sessions. Open `/settings`
and set `Subagents` to `on`. SUPER then gives the main agent tools to start,
guide, wait for, and stop child agents.

Each child uses the same working directory. SUPER allows four active child turns
and one child level. Project settings cannot enable subagents, and `--no-tools`
keeps them off.

### Dynamic workflows

A dynamic workflow coordinates many child agents with a short generated
script. The script holds the plan, while only final results return to the main
conversation. Enable subagents first, then use:

```text
/workflow audit every tool file for missing path checks
use a workflow to compare the provider adapters
```

Run `/workflows` to inspect, pause, resume, stop, restart, or save a workflow.
A saved workflow becomes a reusable slash command after `/reload`.

One workflow can start up to 1,000 agents, with 16 active at once. Workflow
scripts cannot read files, use the network, load modules, or start processes.
Only their child agents use SUPER tools.

## Models and integrations

### Diagnose installation and network access

Run a full health report before you use a provider, or when a corporate
firewall stops a connection:

```bash
super doctor
super doctor --summary
```

The full report shows each provider connection type, host, port, HTTP result,
and elapsed time. It lists separate SSE, WebSocket, AWS event-stream, and login
destinations. You can send the failed rows to a network team as a firewall
allowlist request. The summary report shows one row for each provider.

Any HTTP status means that the destination is reachable. For example, `401`
is normal when the probe does not send a credential. `SKIP` means that the
provider needs local configuration, such as an Azure resource name or a Google
Cloud location. The command does not send a prompt, use provider credentials,
or create model cost. It checks reachability, not credential validity.

### Login and model selection

SUPER supports browser and headless OAuth, API keys, environment variables, and
cloud credentials. It can import compatible credentials from OpenAI Codex,
Claude Code, Pi, OpenCode, OpenClaw, and Hermes.

```bash
super login openai-codex
super login anthropic --device-auth
super login anthropic --api-key YOUR_KEY
super auth
super logout openai-codex
super --list-models
super --model sonnet:high
```

Use a Cursor subscription through SUPER's native HTTP/2 provider:

```bash
super login cursor
super --model cursor/auto
```

SUPER talks directly to Cursor's Agent service. It does not start Cursor's
`agent` command, Cursor desktop, Node.js, Bun, or a local proxy. You can also
set `CURSOR_ACCESS_TOKEN` instead of saving a login. When Cursor is selected,
SUPER refreshes the model list for the signed-in account and keeps a built-in
fallback list if discovery is not available.

### Your own OpenAI-compatible provider

Add a Chat Completions, Responses, or Codex Responses provider. The model then
appears in the normal model selector.

```bash
super provider add local \
  --base-url http://127.0.0.1:8000/v1 \
  --api chat-completions \
  --model local-model
super provider list
super --model local/local-model
super provider remove local
```

Use `--api responses` for a Responses API server. Use `--api-key-env NAME` to
read a key from an environment variable, or use `super login <provider>
--api-key KEY` to save one.

CodexLB can reuse the OpenAI Codex login stored by SUPER:

```bash
super login openai-codex
super provider add codex-lb \
  --base-url http://127.0.0.1:2455/backend-api/codex \
  --api codex \
  --model gpt-5.6-sol \
  --reasoning
super --model codex-lb/gpt-5.6-sol
```

Use `--api-key-env CODEX_LB_API_KEY` when CodexLB needs its own key. Repeat
`--header KEY=VALUE` when a gateway needs custom headers.

The `auto`, `websocket`, and `websocket-cached` transport settings use the
Responses WebSocket API for the built-in OpenAI, OpenAI Codex, and Azure OpenAI
providers. `websocket-cached` reuses the connection and sends only new input
after a successful response. Other providers keep their normal streaming
transport.

The TUI supports the same basic operations:

```text
/provider add <id> <chat-completions|responses|codex> <base-url> <model> [KEY_ENV|auth:<provider>]
/provider list
/provider remove <id>
```

Restart the TUI after an add or remove operation so the current session loads
the changed model catalog.

### MCP

Add local or remote MCP servers:

```bash
super mcp add local -- npx -y @modelcontextprotocol/server-everything
super mcp add remote --url https://example.com/mcp --auth oauth
super mcp login remote
super mcp list
super mcp test local
```

Use `--scope project` to save a server in `.mcp.json`. Use `super mcp login
remote --no-browser` for headless OAuth. Use `/mcp` to manage servers in the
TUI.

### WebMCP

SUPER can discover and call tools that open Chrome pages expose through the
experimental [WebMCP API](https://webmachinelearning.github.io/webmcp/). WebMCP
lets a page give the agent a structured tool instead of making the agent find
and click page controls.

#### Browser setup

Use a recent Chrome or Chromium build. WebMCP and its Chrome protocol are
experimental and can change.

1. Open `chrome://flags/#enable-webmcp-testing` and enable **WebMCP testing**.
2. If `chrome://flags/#devtools-webmcp-support` is present, enable it too.
3. Restart Chrome.
4. Open `chrome://inspect/#remote-debugging` and enable remote debugging.
5. Open a page that registers WebMCP tools.

See the [Chrome WebMCP guide](https://developer.chrome.com/docs/ai/webmcp) for
the current browser requirements and demo pages.

#### Use

WebMCP support is available in the interactive TUI:

```text
/webmcp
/webmcp connect
/webmcp list
/webmcp disconnect
```

`/webmcp` and `/webmcp connect` connect to Chrome and add one `webmcp` agent
tool to the current session. Tool discovery continues in the background.
`/webmcp list` shows the active page origins and tool names. The
`/webmcp disconnect` command closes the browser connection and removes the
agent tool.

The `webmcp` agent tool has three actions:

- `list` returns page origins, titles, URLs, and tool names. It does not return
  descriptions.
- `describe` returns the description and input schema for one exact origin and
  tool name.
- `call` calls one exact origin and tool name with JSON arguments and returns
  the page result.

SUPER updates the tool list when a page adds or removes tools, navigates, opens,
or closes. A call has a 60-second limit. SUPER also sends a browser cancellation
request when the user cancels a call or when the time limit ends.

#### Settings and security

Page tool metadata and results are untrusted content. Limit access to known
sites when possible. Put WebMCP settings in `~/.super/agent/settings.json` or in
the trusted project file `.super/settings.json`:

```json
{
  "webmcp": {
    "allowedOrigins": ["https://example.com"],
    "disallowedOrigins": ["blocked.example"],
    "cdp": 9222
  }
}
```

`allowedOrigins` and `disallowedOrigins` accept a complete origin or a host
name. If `allowedOrigins` is absent, SUPER permits all normal page origins after
the user connects. The deny list always wins. `cdp` accepts a local
remote-debugging port or a complete `ws://` or `wss://` browser WebSocket URL.
Plain `ws://` connections must use a loopback address such as `127.0.0.1` or
`localhost`.

SUPER makes no browser connection and adds no `webmcp` agent tool until the user
runs `/webmcp`. Calls require the exact page origin and tool name. SUPER ignores
internal `chrome:` and `devtools:` pages, validates tool names, marks page data
as untrusted, and limits page output sent to the model to 100,000 bytes.

### Agent Client Protocol

SUPER is a native [Agent Client Protocol](https://agentclientprotocol.com/)
agent. It implements stable ACP v1 in Rust. It does not start an adapter or a
second SUPER process. The ACP client starts this command and exchanges JSON-RPC
messages with it through standard input and output:

```bash
super acp
```

`super acp` is not an interactive terminal. It waits for an ACP client. Before
you configure a client, sign in to a model provider and check the available
models:

```bash
super login openai-codex
super auth
super --list-models
```

Add SUPER as a custom agent in the Zed settings file:

```json
{
  "agent_servers": {
    "SUPER": {
      "type": "custom",
      "command": "super",
      "args": ["acp"],
      "env": {}
    }
  }
}
```

Restart Zed, select SUPER from its custom-agent list, and start a thread. If
`super` is not on the application PATH, set `command` to the full path of the
SUPER executable.

Put global SUPER options before `acp`. For example, select a model and thinking
level in the Zed configuration with these arguments:

```json
"args": ["--model", "sonnet:high", "acp"]
```

Use `"args": ["--no-session", "acp"]` for conversations that must not write
session history. Persistent sessions are the default. Clients can list, load,
resume, close, and delete them. Clients that support ACP session controls can
also change the model and thinking level during a session.

SUPER accepts text, images, resource links, and embedded resources. It streams
answers, reasoning, usage, tool status, file locations, and file diffs. A
cancel request stops active work and work that is waiting to start. Tools run
locally in the working directory that the ACP client supplies.

An ACP client can add stdio or streamable-HTTP MCP servers to one session.
These server definitions stay in memory and do not change SUPER MCP files.
Draft ACP v2, audio prompts, legacy MCP SSE, and client-side filesystem or
terminal delegation are not enabled.

## Build with SUPER

SUPER provides SDKs for Rust, Python 3.11+, TypeScript on Node, Bun, and Deno,
and browser applications through WebAssembly. All SDKs use the same streaming
event protocol.

```rust
let session = super_sdk::Session::builder().tools(["read", "bash"]).build().await?;
session.prompt("What files are here?").await?;
```

```python
async with await super_sdk.Session.create(tools=[super_sdk.ToolName.READ]) as session:
    await session.prompt("What files are here?")
```

```typescript
const session = await Session.create({ tools: ["read", "bash"] });
await session.prompt("What files are here?");
```

`@super-sdk/core-wasm` runs the full agent and model/tool loop in a browser. It
does not need a SUPER server. `@super-sdk/wasm` is the remote client for
applications that need native filesystem and shell tools.

### JSONL RPC

For other languages, start JSONL RPC over standard input and output:

```bash
super --mode rpc --no-session
```

### RPC over WebSocket

For browser and remote clients, start the WebSocket transport:

```bash
super --mode rpc --rpc-listen 127.0.0.1:9944 --no-session
```

Connect to `ws://127.0.0.1:9944`. WebSocket clients use the same JSON commands
and events as JSONL RPC. All connected clients share one SUPER session.

See the [SDK guide](docs/sdk.md), [RPC protocol](docs/rpc.md), and
[browser WebAssembly guide](crates/super-core-wasm/README.md).

## Performance

SUPER benchmarks local work, not model or network latency. Results below are
from the latest full release benchmark run.

### Startup and memory

| Measure | Mean |
| --- | ---: |
| Warm time to first frame | 5.037 ms |
| Warm time to first input | 5.094 ms |
| One idle session | 15.802 MiB RSS |
| Ten idle sessions | 159.010 MiB RSS |
| Extra RSS per added session | 15.912 MiB |

Startup results use ten launches after one warm-up. Memory results use three
trials. RSS is the resident memory reported by macOS. Do not compare these
values directly with Linux proportional set size.

### Core operations

| User action | Test size | Mean | p95 |
| --- | --- | ---: | ---: |
| File search | 100,000 files, three warm queries | 4.606 ms | 4.889 ms |
| File search | 500,000 files, three warm queries | 7.110 ms | 7.690 ms |
| SSE parsing | 10,000 events | 0.931 ms | 0.973 ms |
| Grep | 1,000 files and 200 matches | 6.933 ms | 8.340 ms |
| Incremental Markdown | 200 streaming prefix renders | 12.427 ms | 12.576 ms |
| Rust syntax highlighting | One 200-line fence | 5.830 ms | 6.165 ms |
| Unchanged frame | 10,000 logical rows | 0.876 ms | 0.885 ms |

### SDK, RPC, and browser WebAssembly

These tests use an immediate local model or `ping`. They do not call an
external model.

| Surface | Work | Mean |
| --- | --- | ---: |
| Native SDK | Shared in-process command dispatch | 113 ns |
| JSONL RPC | Encode, decode, in-memory transport, and dispatch | 14.439 us |
| Browser WASM | Warm full-agent prompt, 100 samples | 0.077 ms |
| Browser WASM | 25 isolated agents in parallel, 11 batches | 0.749 ms |

Fresh WebAssembly module initialization averaged 11.269 ms per Deno process.
The release module is 574,734 bytes raw and 209,375 bytes gzip. It starts with
17 linear-memory pages, or 1,114,112 bytes.

### Agent automation

Subagents are off by default. Session setup had no measured slowdown when they
were enabled. Their six control tools added 71 ns to request preparation.

| Measure | State or size | Mean | p95 |
| --- | --- | ---: | ---: |
| Subagent session setup | Off | 358.845 us | 364.466 us |
| Subagent session setup | On | 358.352 us | 361.740 us |
| Request preparation | Subagents off | 171 ns | 177 ns |
| Request preparation | Subagents on | 242 ns | 249 ns |
| Workflow script parsing | 200 lines | 57.884 us | 59.922 us |
| Workflow interpreter | 1,000 agent calls | 2.185 ms | 2.423 ms |
| Workflow progress snapshot | 500 agents, 5 phases | 43.075 us | 64.816 us |
| Workflow phase view | 500 agents, 5 phases | 11.298 us | 12.207 us |
| Workflow agent detail | One prompt and result | 4.886 us | 5.021 us |
| Workflow unchanged view | 500 agents, cached | 311 ns | 317 ns |
| Job detail view | Long goal and result | 45.193 us | 46.595 us |

The workflow interpreter used 2.185 us per agent call. Arming a workflow added
651 ns to request preparation and kept the total below 1 us. Loop and
autoresearch jobs sleep between model turns and update the TUI through small
events.

### TUI rendering and resize

| Measure | Test size | Mean | p95 |
| --- | --- | ---: | ---: |
| Full renderer | 1,800 logical rows | 0.398 ms | 0.436 ms |
| Unchanged renderer | 10,000 logical rows | 0.876 ms | 0.885 ms |
| Last-row update | 10,000 logical rows | 0.895 ms | 0.916 ms |
| Cached transcript render | 2,885 logical rows | 0.058 ms | 0.063 ms |
| Spinner transcript render | 2,885 logical rows | 0.055 ms | 0.057 ms |
| Full resize redraw | 1,800 logical rows | 0.435 ms | 0.477 ms |

SUPER combines rapid resize events and redraws once 75 ms after the final
change. The full resize test wrote 178,231 bytes.

### Profile-guided release builds

| Measure | Standard build | Optimized build | Change |
| --- | ---: | ---: | ---: |
| `super --help` startup | 3.696 ms | 3.676 ms | 0.52% faster |
| Geometric mean latency | 1.000x | 0.985x | 1.51% faster |
| Executable size | 17.16 MiB | 14.87 MiB | 13.37% smaller |
| gzip size | 8.17 MiB | 7.36 MiB | 9.94% smaller |

### Method

The tests use release builds and local deterministic fixtures. The startup
test used a 160 by 40 terminal and `super --no-session` on macOS 26.5.1 with an
Apple M4. Startup values are the mean of ten warm launches. Memory values are
the mean of three idle samples. Core, SDK, RPC, and WebAssembly tests also ran
on the Apple M4. Profile-guided results use separate held-out runs. Lower is
better.

Run the complete native and browser suite with `just bench`. It requires
cargo-nextest, wasm-pack, Deno, and Node. Harness results are written to
`target/harness-benchmark.json`.

## Configuration

SUPER stores user configuration in `~/.super/agent`. It loads project
configuration only after you trust the project.

| Path | Purpose |
| --- | --- |
| `~/.super/agent/settings.json` | User settings |
| `.super/settings.json` | Project settings |
| `~/.super/agent/models.json` | Custom providers and models |
| `~/.super/agent/mcp.json` | User MCP servers |
| `.mcp.json` | Project MCP servers |
| `~/.super/agent/skills/` | User skills |
| `.super/skills/` | Project skills |
| `~/.super/agent/workflows/` | Personal workflow scripts |
| `.super/workflows/` | Trusted project workflow scripts |

Open `/settings` for common TUI settings. Run `super --help` for all command-line
options. Custom themes live in `~/.super/agent/settings.json`.

## Compatibility

SUPER tracks [Pi v0.85.1](https://github.com/earendil-works/pi/releases/tag/v0.85.1).
It keeps Pi-compatible session files, model data, core commands, compaction,
and OpenAI Responses WebSocket transport. `Cargo.toml` records the tracked
release.

## Development

Use Rust stable and cargo-nextest:

```bash
cargo nextest run --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
just pgo-test
just pgo-bench
```

## Release

1. Set `[workspace.package].version` in `Cargo.toml`.
2. Add release notes to `CHANGELOG.md`.
3. Run `cargo check --workspace` to update `Cargo.lock`.
4. Run `just release-check VERSION`.
5. Commit the version, lock file, and changelog.
6. Run `just release VERSION` from a clean `main` branch.

The release command runs all checks, creates the tag, and starts the GitHub
release workflow.

## License

MIT. SUPER is inspired by Pi, which is also licensed under MIT.
