# Phase 1: Platform Server + CLI Foundation — Design

## Scope

Two minimal but functional pieces that together form the skeleton of Super:

1. **Platform Server** — auth + OpenRouter key provisioning
2. **Super CLI** — Rust shell that authenticates, prompts, and streams responses

The goal is a working end-to-end loop: `super login` (PKCE) → type a prompt → streaming response from OpenRouter (via rig) displayed in the TUI. The CLI is structured so the remaining P0 module slots can be filled in incrementally.

---

## 1. Platform Server

### 1.1 Endpoints

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/auth/authorize` | PKCE token exchange |
| `POST` | `/auth/refresh` | Refresh access token |
| `GET`  | `/auth/me`    | User profile + provisioned API key |
| `GET`  | `/auth/key`   | Re-fetch per-user OpenRouter key |

### 1.2 Auth flow

Standard PKCE (S256):

1. User runs `super login` → CLI generates code_verifier + code_challenge, opens browser to platform server's `/login` page
2. User authenticates (OAuth provider or email/password — server-side concern)
3. Server redirects browser to localhost callback with `?code=<auth_code>`
4. CLI exchanges code + verifier at `POST /auth/authorize` → receives `access_token` + `refresh_token`
5. CLI stores tokens at `~/.super/config.json`
6. On first `/auth/me` call after login, server provisions a per-user OpenRouter API key (if one doesn't exist) and returns it

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
  email: string
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

### 1.5 Tech choices

- **Runtime:** Node.js (TypeScript) — the server is a thin coordination layer, not performance-critical
- **Framework:** Express or Fastify
- **Database:** SQLite (single-file, zero-ops for phase 1)
- **Deployment:** single process, single machine

---

## 2. Super CLI

### 2.1 Module slots (P0 skeleton)

The CLI is organized around the P0 modules from the analysis workspace. In phase 1, most are stubs — just enough structure so the real implementation slots in cleanly.

| Module | Phase 1 scope |
|---|---|
| M01 Configuration | Load `~/.super/config.json`. Single-file config. No multi-scope merge yet. |
| M02 Auth | PKCE login against platform server. Token refresh on 401. |
| M03 Permissions | **Stub** — hardcoded "allow all" for now. |
| M04 Session/State | In-memory conversation state. No persistence yet. |
| M05 Conversation Engine | System prompt + user message → rig → streaming response. No compaction. No caching. No retry beyond rig defaults. |
| M06 Tool Framework | **Stub** — empty tool registry. |
| M07-M08 Tools | **Not implemented** — no Bash/Read/Edit/Write yet. |
| M09 MCP | **Not implemented.** |
| M10 Commands/Skills | Superpowers auto-load on startup. No slash commands yet. |
| M11 Bootstrap | `main()` → load config → check auth → load skills → launch TUI |
| M12 SDK | **Not implemented.** |
| M13 REPL/TUI | Full layout: scroll area + activity indicator + input bar with Claude Code glyphs and diamond splash. |

### 2.2 TUI layout

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

### 2.3 Agent loop (minimal)

```
user types prompt → Enter
  → set activity: "Thinking", start glyph animation
  → build messages: system_prompt + conversation_history + user_message
  → rig::Client (OpenRouter) → streaming completion
  → render each chunk into scroll area as it arrives
  → on complete: set activity idle, render full response
  → append to conversation history
```

No tool use handling yet — the model can request tools but the CLI will error gracefully (no tools registered).

### 2.4 Superpowers integration

Port the Superpowers auto-start logic: on startup, discover skills from bundled and config paths, load their content. Skills are available as system prompt context. This is the ONLY hook-like behavior — no generic hook system.

### 2.5 Headless mode

`super --print "prompt"` or `super -p "prompt"` — same agent loop but output goes to stdout instead of TUI. Required for later remote control and E2B integration.

### 2.6 Config

`~/.super/config.json`:

```json
{
  "access_token": "...",
  "refresh_token": "...",
  "openrouter_api_key": "...",
  "api_base_url": "https://api.super.example.com",
  "model": "anthropic/claude-sonnet-4-6"
}
```

### 2.7 Crate plan

```toml
[dependencies]
rig-core = "..."          # LLM agent framework
ratatui = "..."           # Terminal UI
crossterm = "..."         # Terminal backend
tokio = { ..., features = ["full"] }
serde = { ..., features = ["derive"] }
serde_json = "..."
reqwest = { ..., features = ["json"] }
dirs = "..."              # ~/.super path
clap = { ..., features = ["derive"] }
```

---

## 3. What's explicitly deferred

- All tool implementations (Bash, Read, Edit, Write, Glob, Grep, Agent, WebFetch, WebSearch, etc.)
- Permission system (stub only)
- Multi-scope settings merge
- Session persistence (JSONL transcripts)
- Context compaction
- Prompt caching
- MCP integration
- Subagents
- Remote control / bridge
- Web frontend
- E2B sandbox

---

## 4. Build order

1. **Platform server** — auth + key provisioning working
2. **CLI skeleton** — config load, login, TUI layout (no LLM yet)
3. **Agent loop** — rig integration, streaming responses in TUI
4. **Superpowers** — skill auto-loading on startup
5. **Headless mode** — `-p` flag

Server first because the CLI's login flow depends on it. The CLI can be developed against a running server instance.