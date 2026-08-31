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

The following `just bench` results used an Apple M4, macOS 26.5.1, and Rust
1.98.0 on 2026-08-31. This command measures internal code with the Cargo
`release` profile. Shipped binaries add PGO and macOS ICF. Lower values are
better.

| Benchmark | Work | Median | p95 |
| --- | --- | ---: | ---: |
| File search | 10,000 files, three warm queries | 486.875 us | 493.542 us |
| File search prefix | 10,000 files, five extended queries | 515.583 us | 549.459 us |
| File search | 100,000 files, three warm queries | 5.122 ms | 5.277 ms |
| File search prefix | 100,000 files, five extended queries | 5.570 ms | 5.655 ms |
| File search | 500,000 files, three warm queries | 10.639 ms | 11.836 ms |
| File search prefix | 500,000 files, five extended queries | 14.261 ms | 14.831 ms |
| Transcript render | 2,885 logical rows | 89.020 us | 93.708 us |
| Spinner render | 2,885 logical rows | 83.208 us | 84.604 us |
| Context usage | 225 messages, about 900 KiB | 262 ns | 270 ns |
| Bounded read | 13 MiB file, offset 190,000 | 12.121 ms | 12.564 ms |
| Head read | First 50 lines of a 13 MiB file | 46.000 us | 65.542 us |
| Stream snapshots | 2,000 deltas, 40 KiB result | 106.084 us | 115.417 us |
| Auth file read | 30 OAuth entries | 36.531 us | 37.981 us |
| Cached auth read | 30 OAuth entries | 3.907 us | 4.464 us |
| SSE parsing | 10,000 events | 1.144 ms | 1.486 ms |
| Session append | 500 saved user messages | 1.912 ms | 2.283 ms |
| Grep | 1,000 files and 200 matches | 14.611 ms | 16.121 ms |
| Markdown render | One 40 KiB document | 895.500 us | 907.479 us |
| Growing Markdown | 200 full prefix renders | 78.109 ms | 79.828 ms |
| Incremental Markdown | 200 streaming prefix renders | 15.672 ms | 16.299 ms |
| Full frame render | 1,800 logical rows | 327.611 us | 383.777 us |
| Unchanged frame | 10,000 logical rows | 133.220 us | 134.583 us |
| Last-row frame change | 10,000 logical rows | 130.862 us | 132.683 us |
| ANSI width | 2,000 styled lines | 136.595 us | 146.345 us |

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
