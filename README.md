# SUPER

A fast terminal coding agent built in Rust and based on
[Pi](https://github.com/earendil-works/pi).

SUPER gives you one focused interface for coding with OpenAI Codex, Anthropic,
Google, OpenRouter, Bedrock, GitHub Copilot, and other model providers. It
supports local tools, persistent sessions, OAuth login, and MCP without a
plugin runtime.

## Why SUPER

- **Fast:** one native binary with a responsive terminal interface.
- **Flexible:** more than 1,000 models from the Pi model catalog.
- **Focused:** `read`, `write`, `edit`, and `bash` are the default tools.
- **Persistent:** resume, branch, compact, import, and export sessions.
- **Connected:** use local or remote MCP servers, including OAuth servers.

## Performance

SUPER release binaries use target-specific profile-guided optimization (PGO).
macOS releases also use safe identical code folding (ICF). The Cargo `dist`
profile inherits the release settings: optimization level 3, fat link-time
optimization, one code generation unit, stripped symbols, and abort-on-panic.

These selected `just bench` results used an Apple M4, macOS 26.5.1, and Rust
1.98.0 on 2026-08-31. The command measures internal code with the Cargo
`release` profile. Lower values are better.

| Benchmark | Work | Median | p95 |
| --- | --- | ---: | ---: |
| File search | 100,000 files, three warm queries | 5.122 ms | 5.277 ms |
| File search | 500,000 files, three warm queries | 10.639 ms | 11.836 ms |
| SSE parsing | 10,000 events | 1.144 ms | 1.486 ms |
| Grep | 1,000 files and 200 matches | 14.611 ms | 16.121 ms |
| Incremental Markdown | 200 streaming prefix renders | 15.672 ms | 16.299 ms |
| Unchanged frame | 10,000 logical rows | 133.220 us | 134.583 us |

The subagent overhead benchmark used the same machine and release profile. It
compared the same empty root session with four normal tools. The off case had
no control tools. The on case had all six subagent control tools. It did not
call a model or start a child. Each release run used 21 matched samples. The
sample order changed between off and on to reduce process drift. Each sample
used 500 session constructions or 10,000 context preparations.

- Three root session construction trials measured off/on medians of
  262.996/261.164 us, 266.224/266.563 us, and 254.168/261.607 us. The matched
  changes were -0.70%, +0.13%, and +2.93%. The p95 ranges overlapped: 278.852
  to 573.608 us off and 272.210 to 365.396 us on. The result does not show a
  stable construction regression above process noise.
- Three model-request context trials measured off medians of 207, 217, and
  200 ns. The on medians were 293, 298, and 293 ns. The median matched increase
  was 86 ns, or 41.55%. The median matched p95 increase was 95 ns, or 45.24%.
  The percentage is large because the complete operation takes less than
  0.4 us.

Run only this comparison with:

```bash
cargo nextest run -p super-coding --release --run-ignored only --no-capture -E 'test(~benchmark_performance_subagent_overhead)'
```

The matched PGO benchmark used the same machine and three held-out trials.
Both executables used the `dist` profile and macOS ICF.

| PGO result | Ordinary | PGO | Change |
| --- | ---: | ---: | ---: |
| `super --help` startup | 3.696 ms | 3.676 ms | 0.52% faster |
| Geometric mean latency | 1.000x | 0.985x | 1.51% faster |
| Executable size | 17.16 MiB | 14.87 MiB | 13.37% smaller |
| gzip size | 8.17 MiB | 7.36 MiB | 9.94% smaller |

## Install

macOS and Linux:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/chwzr/super/main/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/chwzr/super/main/install.ps1 | iex
```

The installer selects the correct release, verifies its SHA-256 checksum, and
installs `super` in your user binary directory. The repository and its releases
must be public for these commands to work without GitHub authentication.

## Quick start

Sign in with your ChatGPT subscription and start SUPER:

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

Use SUPER interactively or for one task:

```bash
super "explain this repository"
super -p "summarize the current changes"
cat error.log | super -p "find the cause"
```

## Terminal workflow

- Type `/` to find commands.
- Type `@` to find and attach files.
- Type `!command` to run a shell command.
- Press `Shift+Tab` to change reasoning effort.
- Press `Esc` or `Ctrl+C` to stop active work.
- Press `Ctrl+D` on an empty input to exit.
- Use the Up arrow to restore earlier prompts.

Useful commands include `/login`, `/model`, `/mcp`, `/compact`, `/resume`,
`/export`, `/settings`, and `/hotkeys`.

SUPER can accept a new instruction while the agent works. Press `Enter` to
steer the current task, or `Alt+Enter` to queue a follow-up task.

## Subagents

Subagents are off by default. Open `/settings` and change `Subagents` to `on`
to give the main agent these control tools: `spawn_agent`, `send_message`,
`followup_task`, `wait_agent`, `list_agents`, and `interrupt_agent`.

Each child is a separate SUPER session, but it uses the same working directory.
SUPER allows four active child turns and one child level. A child starts with
fresh context unless the main agent explicitly copies parent turns. Project
settings cannot enable this feature. `--no-tools` also keeps it off.

See [subagents.md](subagents.md) for the design analysis and tradeoffs.

## Models and login

SUPER supports browser and headless OAuth for OpenAI Codex and Anthropic. It
also supports API keys, environment variables, cloud credentials, and
compatible credentials from OpenAI Codex, Claude Code, Pi, OpenCode,
OpenClaw, and Hermes.

```bash
super login openai-codex
super login anthropic --device-auth
super login anthropic --api-key YOUR_KEY
super auth
super logout openai-codex
super --list-models
super --model sonnet:high
```

## MCP

Add local or remote MCP servers from the terminal:

```bash
super mcp add local -- npx -y @modelcontextprotocol/server-everything
super mcp add remote --url https://example.com/mcp --auth oauth
super mcp login remote
super mcp list
super mcp test local
```

Use `--scope project` to save a server in `.mcp.json`. Use
`super mcp login remote --no-browser` for a headless OAuth flow. The `/mcp`
command manages servers inside the TUI.

## Configuration

SUPER stores user configuration in `~/.super/agent`. Project configuration is
loaded only after you trust the project.

| Path | Purpose |
| --- | --- |
| `~/.super/agent/settings.json` | User settings |
| `.super/settings.json` | Project settings |
| `~/.super/agent/models.json` | Custom providers and models |
| `~/.super/agent/mcp.json` | User MCP servers |
| `.mcp.json` | Project MCP servers |
| `~/.super/agent/skills/` | User skills |
| `.super/skills/` | Project skills |

Run `super --help` for command-line options and `/settings` for common TUI
settings.

## Pi compatibility

SUPER currently tracks [Pi v0.84.4](https://github.com/earendil-works/pi/releases/tag/v0.84.4).
It keeps Pi-compatible session files, model data, core commands, compaction,
and OpenAI Codex WebSocket transport. The tracked Pi release is recorded in
`Cargo.toml`.

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
2. Add the release notes to `CHANGELOG.md`.
3. Run `cargo check --workspace` to update `Cargo.lock`.
4. Run `just release-check VERSION`.
5. Commit the version, lock file, and changelog.
6. Run `just release VERSION` from a clean `main` branch.

The release command runs all checks, creates the tag, and starts the GitHub
release workflow.

## License

MIT. SUPER is inspired by [Pi](https://github.com/earendil-works/pi), which is
also licensed under MIT.
