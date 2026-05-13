# Phase 1: Platform Server + Full CLI — Design

## Scope

Three pieces delivered together:

1. **Platform Server** — auth + OpenRouter key provisioning (hexagonal architecture)
2. **Super CLI** — full agentic CLI with Claude Code tool parity
3. **Superpowers / Skills** — bundled, auto-loaded on startup

The goal is a working end-to-end agent loop: `super login` → type a prompt → the model uses tools (Bash, Read, Edit, Write, Agent, etc.) → results stream back through the TUI. The CLI implements all Claude Code tools and the Superpowers skill system, matching the behavioral specs in `analysis-workspace/raw/specs/`.

---

## 1. Platform Server

### 1.1 Endpoints

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/auth/register` | Create account (email + password) |
| `POST` | `/auth/authorize` | PKCE token exchange (email + password auth) |
| `POST` | `/auth/refresh` | Refresh access token |
| `GET`  | `/auth/me`    | User profile + provisioned API key |
| `GET`  | `/auth/key`   | Re-fetch per-user OpenRouter key |

### 1.2 Auth flow

The server provides email/password authentication. The CLI still uses PKCE for the token exchange to avoid ever sending the password directly to the CLI.

**Registration:**

1. User visits the platform server's web registration page (or `POST /auth/register`)
2. Provides email + password
3. Server hashes password (bcrypt), creates user record, and immediately provisions a per-user OpenRouter API key via the management key
4. User is redirected to login

**Login (PKCE):**

1. User runs `super login` → CLI generates code_verifier + code_challenge, opens browser to platform server's `/login` page
2. User enters email + password on the server's login page
3. Server validates credentials, creates an authorization code bound to the code_challenge
4. Server redirects browser to localhost callback with `?code=<auth_code>`
5. CLI exchanges code + verifier at `POST /auth/authorize` → receives `access_token` + `refresh_token`
6. CLI stores tokens at `~/.super/config.json`
7. CLI calls `GET /auth/me` with the access token → receives user profile + the already-provisioned OpenRouter API key

### 1.3 OpenRouter key management

- Server holds a single OpenRouter **management API key** (env var `OPENROUTER_MANAGEMENT_KEY`)
- On first user API key request, server calls OpenRouter's key creation API to mint a new API key with:
  - `label`: `super-user-{user_id}`
  - `limit`: $20 USD
- The per-user key is stored server-side (database) and returned to the CLI
- CLI uses this key directly to call OpenRouter via rig — it does NOT proxy requests through the server
- Key rotation: `POST /auth/key` can be called to cycle the key (revoke old, create new)

### 1.4 Data model

```
users:
  id: uuid
  email: string (unique)
  password_hash: string (bcrypt)
  created_at: timestamp

api_keys:
  user_id: uuid (FK → users)
  openrouter_key_id: string (OpenRouter's key ID)
  openrouter_key_value: string (encrypted at rest)
  created_at: timestamp
  revoked_at: timestamp (nullable)

refresh_tokens:
  user_id: uuid (FK → users)
  token_hash: string (sha256 of refresh token)
  expires_at: timestamp
```

### 1.5 Architecture: hexagonal (ports and adapters)

```
┌──────────────────────────────────────────────────────┐
│                  HTTP layer (axum)                    │
│  routes/auth.rs  —  deserialize, call domain,       │
│                     map domain errors → HTTP status  │
├──────────────────────────────────────────────────────┤
│                   Domain layer                        │
│                                                     │
│  auth/service.rs   —  register, login, refresh,     │
│                       key provisioning orchestration │
│  auth/ports.rs     —  trait AuthRepository          │
│                                                     │
├──────────────────────────────────────────────────────┤
│                 Adapter layer                         │
│  adapters/sqlite_auth_repo.rs                        │
│    impl AuthRepository for SqliteAuthRepo            │
│  adapters/openrouter_client.rs                       │
│    calls OpenRouter key management API               │
└──────────────────────────────────────────────────────┘
```

**Domain (no framework coupling):**
- All business logic lives in the domain layer — plain Rust structs and trait definitions
- `AuthService` takes `Arc<dyn AuthRepository>` — no knowledge of SQLite, HTTP, or axum
- Errors are domain enums (`AuthError`), not HTTP status codes

**Ports (traits):**

```rust
pub trait AuthRepository {
    fn create_user(&self, email: &str, password_hash: &str) -> Result<User, AuthError>;
    fn find_user_by_email(&self, email: &str) -> Result<Option<User>, AuthError>;
    fn create_refresh_token(&self, user_id: &str, expires_at: DateTime) -> Result<String, AuthError>;
    fn consume_refresh_token(&self, token_hash: &str) -> Result<Option<(String, String)>, AuthError>;
    fn store_api_key(&self, user_id: &str, key_id: &str, key_value: &str) -> Result<(), AuthError>;
    fn get_active_api_key(&self, user_id: &str) -> Result<Option<ApiKey>, AuthError>;
    fn revoke_api_key(&self, user_id: &str, key_id: &str) -> Result<(), AuthError>;
}

pub trait OpenRouterProvider {
    fn create_key(&self, label: &str, limit_usd: u32) -> Result<OpenRouterKey, Error>;
    fn revoke_key(&self, key_id: &str) -> Result<(), Error>;
}
```

**Adapters:**
- `SqliteAuthRepo` implements `AuthRepository` using `rusqlite`
- `OpenRouterClient` implements `OpenRouterProvider` using `reqwest`

**HTTP layer (axum):**
- Thin route handlers — extract body, call service method, map result to response
- Shared application state holds `Arc<AuthService>`

### 1.6 Tech choices

- **Runtime / framework:** Rust + axum (async, tower-based)
- **Database:** SQLite via `rusqlite` (single-file, zero-ops for phase 1)
- **Password hashing:** bcrypt via `bcrypt` crate
- **JWT:** `jsonwebtoken` crate
- **HTTP client:** `reqwest` (for calling OpenRouter management API)
- **Deployment:** single binary, single process

---

## 2. Super CLI

The CLI implements all 13 P0 modules from the analysis workspace module map, matching Claude Code's behavioral parity as specified in `analysis-workspace/raw/specs/`.

### 2.1 Module implementation scope

| Module | Phase 1 scope |
|---|---|
| M01 Configuration | Load `~/.super/config.json`. Single-file config. Settings validation. |
| M02 Auth | PKCE login against platform server. Token refresh on 401. Credential caching. |
| M03 Permissions | Full implementation: 7 modes (default, acceptEdits, bypass, plan, dontAsk, auto, bubble), pattern-matching rules, allow/deny/ask decision engine. |
| M04 Session/State | Single immutable store (Zustand-like pattern). Conversation messages, tool permission context, model selection, task tracking. State change side effects (onChange). |
| M05 Conversation Engine | Full agent loop: system prompt assembly → context management → streaming via rig → tool-use extraction → multi-turn iteration. Compaction, prompt caching via OpenRouter headers, model selection, structured output. CLAUDE.md loading. |
| M06 Tool Framework | Full tool registry with permission-aware assembly, multi-turn execution orchestration, concurrent-safe parallel execution. All tool contract interface methods. |
| M07 Shell Execution | Bash tool with security validation (22-category checks), process management, signal handling, background execution. |
| M08 File Operations | Read, Edit, Write, Glob, Grep, NotebookEdit, Config tools. Full validation pipelines (read-first checks, staleness detection, string matching with quote normalization). File history tracking. |
| M09 MCP Integration | Full MCP client: stdio transport, tool/command/resource discovery, OAuth auth flow, channel permissions, reconnection. MCP tools merged with built-ins. |
| M10 Commands/Skills | Slash command system with registration, resolution, and dispatch (prompt/local/local-jsx). All Claude Code commands. Skills discovered from file paths. |
| M11 Bootstrap | `main()` → fast-path dispatch → init → setup → onboarding → REPL launch. |
| M12 SDK Interface | Wire protocol message types and control protocol. `query()` function for programmatic use. |
| M13 REPL/TUI | Full layout: scroll area + activity indicator + input bar with Claude Code glyphs. Message rendering, permission dialogs, agent/task UI, notification queue, keybinding system. |

### 2.2 Tool set — complete Claude Code parity

Every tool from `claude-code-src` is implemented, matching the behavioral specs in `analysis-workspace/raw/specs/behavioral-docs/tools.md`.

**File Operations (M08):**
| Tool | Spec | Key behaviors |
|---|---|---|
| Read | SPEC-M08-001–007 | File reading, image pipeline, PDF handling, deduplication, token budget, ENOENT suggestions |
| Edit | SPEC-M08-012–013, 020 | Exact string replacement, 11-step validation pipeline, quote normalization, uniqueness check, staleness detection |
| Write | SPEC-M08-014, 020 | File create/overwrite, read-first requirement, LF line endings |
| Glob | SPEC-M08-015 | Pattern-based file search, mtime-sorted results, configurable limits |
| Grep | SPEC-M08-016 | ripgrep-style search, output modes (content/files_with_matches/count), context lines |
| NotebookEdit | SPEC-M08-024 | Jupyter notebook cell editing (replace/insert/delete), nbformat compatibility |
| Config | SPEC-M08-017 | Read/write settings, GET auto-allowed, SET requires confirmation, shouldDefer batching |

**Shell Execution (M07):**
| Tool | Spec | Key behaviors |
|---|---|---|
| Bash | SPEC-M07-001–011 | Command execution, 22-category security validation, process groups, signal handling, background tasks, output truncation, PWD reset |

**Agent & Task System (M14, M17):**
| Tool | Spec | Key behaviors |
|---|---|---|
| Agent | SPEC-M14-001 | Sub-agent spawning (sync/async/background), agent definition resolution, worktree isolation, auto-background timeout (120s), handoff classification |
| TaskCreate | SPEC-M17-002, 005 | Task creation with subject/description/metadata, hook execution |
| TaskGet | SPEC-M17-005 | Task retrieval by ID |
| TaskList | SPEC-M17-005 | Task listing with dependency filtering |
| TaskUpdate | SPEC-M17-002, 005 | Status transitions (pending→in_progress→completed/failed), dependency management |
| TaskStop | SPEC-M17-005 | Background task termination |
| TaskOutput | SPEC-M17-005–009 | Background task output retrieval (blocking/non-blocking), disk spillover, circular buffer |
| TodoWrite | — | Structured task list management |
| ToolSearch | SPEC-M06-008 | Dynamic tool discovery by name or description |
| EnterPlanMode | SPEC-M14-008 | Transition to read-only plan mode |
| ExitPlanMode | SPEC-M06-007 | Exit plan mode with user approval |
| EnterWorktree | SPEC-M14-006 | Git worktree creation and session isolation |
| ExitWorktree | SPEC-M14-006 | Worktree cleanup (keep/remove with safety checks) |
| SendMessage | SPEC-M14-010 | Inter-agent messaging (shutdown_request, plan_approval) |

**Web Operations (M16):**
| Tool | Spec | Key behaviors |
|---|---|---|
| WebFetch | SPEC-M16-001, 003 | URL fetch → HTML-to-markdown → Haiku processing, domain safety check, redirect handling, preapproved domains, caching |
| WebSearch | SPEC-M16-002 | Server-side web search via OpenRouter, domain filtering |

**Special Purpose (M06):**
| Tool | Spec | Key behaviors |
|---|---|---|
| AskUserQuestion | SPEC-M06-006 | 1–4 multiple choice questions, multiSelect, annotations |
| Skill | — | Invoke a skill by name |
| EnterPlanMode | SPEC-M14-008 | Enter plan (read-only) mode |
| ExitPlanMode | SPEC-M06-007 | Request exit from plan mode |
| LSP | — | Go-to-definition, find-references, hover, document symbols, workspace symbols, call hierarchy |
| CronCreate | SPEC-M17-010–017 | Schedule recurring/one-shot prompts via 5-field cron, durable persistence |
| CronDelete | SPEC-M17-012 | Cancel a scheduled job |
| CronList | SPEC-M17-012 | List scheduled jobs |
| Sleep | SPEC-M06-009 | Pause execution for a duration |
| Monitor | SPEC-M06-009 | File/directory watching via inotify/polling |
| StructuredOutput | — | Validates and returns structured output when `--output-format` is used |

**MCP Tools (M09):**
| Tool | Spec | Key behaviors |
|---|---|---|
| ListMcpResources | SPEC-M09-004 | List MCP server resources with LRU caching |
| ReadMcpResource | SPEC-M09-004 | Read a specific MCP resource by URI |
| McpAuth | SPEC-M09-005 | MCP server authentication (OAuth/XAA) |

### 2.3 TUI layout

```
┌─────────────────────────────────────────┐
│  ◆ (diamond splash on launch)           │
│                                         │
│  User: > hello                          │
│                                         │
│  Agent: Hi! I'm Super...                │
│                                         │
│  (scrollable history)                   │
│                                         │
├─────────────────────────────────────────┤
│  ⟣  Thinking                            │
├─────────────────────────────────────────┤
│  > _                                    │
└─────────────────────────────────────────┘
```

- **Scroll area (top):** ratatui `Paragraph` in a scrollable block. Chronological message log. Splash on launch.
- **Activity row (middle):** Activity glyph (animated cycle `⟣ ⟡ ⟐ ◈ ⟢` when active, `♦` when idle) + verb from Claude Code's set.
- **Input bar (bottom):** Fixed, single-line text input. ratatui `TextArea`-style.
- **Thinking blocks:** Collapsed by default, displayed as `thinking...` with animated glow effect. Expandable on demand.
- **Permission dialogs:** Modal overlays for tool-use confirmation (allow/deny/always allow).

### 2.4 Agent loop (full)

```
user types prompt → Enter
  → set activity: "Thinking", start glyph animation
  → build system prompt (M05):
      - CLAUDE.md content
      - loaded skills content
      - tool descriptions
      - system prompt sections (parity with Claude Code)
  → build messages: system_prompt + conversation_history + user_message
  → rig::Client (OpenRouter) with prompt caching headers
  → streaming completion
  → parse response: text content + tool_use blocks
  → for each tool_use:
      - permission check (M03): allow/deny/ask decision
      - if ask: render permission dialog, wait for user input
      - execute tool (M06): validate input → call tool → capture result
      - render tool result in scroll area
      - if agent tool: spawn sub-agent loop (sync/async)
  → on completion: set activity idle, render full response
  → append to conversation history (M04)
  → check compaction threshold, compact if needed
```

### 2.5 Superpowers / Skills integration

Port the Superpowers auto-start logic into the CLI startup path — no generic hook system.

- On startup, discover skills from:
  - Bundled skills (shipped with the binary)
  - `~/.super/skills/` (user-installed)
  - `.claude/skills/` in project directory (project skills)
- Skills are markdown files with optional YAML frontmatter
- Loaded skill content is injected into the system prompt on session start
- The `Skill` tool allows the model to invoke skills by name at runtime
- Skill discovery is re-triggered on file changes

### 2.6 Config

`~/.super/config.json`:

```json
{
  "access_token": "...",
  "refresh_token": "...",
  "openrouter_api_key": "...",
  "api_base_url": "https://api.super.example.com",
  "model": "anthropic/claude-sonnet-4-6",
  "permissions": {},
  "settings": {}
}
```

### 2.7 Crate plan

```toml
[dependencies]
rig-core = "..."           # LLM agent framework
ratatui = "..."            # Terminal UI
crossterm = "..."          # Terminal backend
tokio = { ..., features = ["full"] }
serde = { ..., features = ["derive"] }
serde_json = "..."
serde_yaml = "..."         # Skill frontmatter parsing
reqwest = { ..., features = ["json"] }
dirs = "..."               # ~/.super path
clap = { ..., features = ["derive"] }
uuid = { ..., features = ["v4"] }
sha2 = "..."               # PKCE code challenge
base64 = "..."             # PKCE encoding
tree-sitter = "..."        # Bash command parsing (for security validation)
tree-sitter-bash = "..."
regex = "..."              # Permission rule matching, grep
glob = "..."               # Glob pattern matching
notify = "..."             # File watching (skills, config)
```

---

## 3. What's explicitly deferred (post-v1)

- Hook system (generic, 27-event). Only Superpowers auto-start is ported in.
- iOS app
- E2B sandbox integration
- Web frontend
- Remote control / bridge (CCR)
- Daemon mode and background sessions (M25)
- Voice mode (M24)
- IDE integration (M23) — LSP tool is implemented, but IDE detection/notifications are deferred
- Coordinator mode
- Swarm/teammate multi-agent (TeamCreate, TeamDelete, tmux/iTerm2 backends)
- Session replay (M26)
- Plugin marketplace (M20)
- Feature-flagged tools: PowerShell, Brief/SendUserMessage, RemoteTrigger, MonitorTool, PushNotificationTool, REPL, and all ant-only / production-only tools

---

## 4. Build order

1. **Platform server** — auth + key provisioning, hexagonal architecture, all 5 endpoints
2. **CLI: bootstrap + TUI shell** — config load, login flow, ratatui layout (scroll area, activity indicator, input bar, diamond splash)
3. **CLI: conversation engine** — rig integration, streaming, system prompts, context management, compaction
4. **CLI: tool framework + file ops** — tool registry, permission assembly, Read/Edit/Write/Glob/Grep/NotebookEdit/Config
5. **CLI: Bash tool** — shell execution, security validation, process management
6. **CLI: permission system** — 7 modes, pattern-matching rules, decision engine
7. **CLI: agent + task system** — Agent tool, TaskCreate/Get/Update/List/Stop/Output, TodoWrite, EnterPlanMode/ExitPlanMode
8. **CLI: web operations** — WebFetch, WebSearch
9. **CLI: MCP integration** — client, transports, tool merge
10. **CLI: commands + skills** — slash commands, Superpowers auto-load, Skill tool
11. **CLI: remaining tools** — LSP, CronCreate/CronDelete/CronList, Sleep, ToolSearch, StructuredOutput, AskUserQuestion, EnterWorktree/ExitWorktree, SendMessage