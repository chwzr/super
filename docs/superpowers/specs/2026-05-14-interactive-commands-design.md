# Interactive Slash Commands Design Spec

## Goal

Implement interactive modal UX for `/model`, `/effort`, `/status`, `/resume`, `/mcp`, `/config`, and `/agents` that matches Claude Code's exact visual and interaction behavior. Additionally remove `/sandbox` and `/feedback` from the command set (blacklisted).

## Architecture

### Modal System

Modals are **not** floating overlays. They render **below** the activity row separator, replacing the input bar area for the duration of the interaction. This matches CC's exact layout pattern.

The `App` struct gains a `modal: Option<Modal>` field. When `Some`, the render loop skips drawing the input bar and draws the modal instead. All key events route to the modal's `handle_key()` method. When the modal returns `ModalAction::Close`, `modal` is set back to `None` and the input bar re-renders.

```
┌───────────────────────────────────────────────────────┐
│  scroll area (messages)                               │
│                                                       │
│                                                       │
├── activity row ───────────────────────────────────────┤
│  ────────────────────────────────────────────────     │  ← separator Indexed(153)
│  [modal content replaces input bar here]              │
└───────────────────────────────────────────────────────┘
```

### File Structure

| File | Change |
|------|--------|
| `cli/src/tui/colors.rs` | New: CC color palette constants |
| `cli/src/tui/modal.rs` | New: `Modal` enum + `ModalAction` + trait impl |
| `cli/src/tui/modals/model_picker.rs` | New: `/model` modal |
| `cli/src/tui/modals/effort_picker.rs` | New: `/effort` modal |
| `cli/src/tui/modals/status_view.rs` | New: `/status` modal (tabs) |
| `cli/src/tui/modals/resume_picker.rs` | New: `/resume` search picker |
| `cli/src/tui/modals/mcp_list.rs` | New: `/mcp` toggle list |
| `cli/src/tui/modals/config_view.rs` | New: `/config` key-value viewer |
| `cli/src/tui/modals/agents_view.rs` | New: `/agents` tabbed viewer |
| `cli/src/tui/mod.rs` | Expose `colors`, `modal`, `modals` |
| `cli/src/tui/app.rs` | Add `modal` field, route keys, render |
| `cli/src/commands/dispatch.rs` | Commands return `OpenModal(Modal)` result; remove `/sandbox`, `/feedback` |
| `cli/src/commands/registry.rs` | Remove `/sandbox`, `/feedback` entries |
| `server/src/routes/auth.rs` | Add `GET /auth/usage` endpoint |
| `server/src/adapters/openrouter_client.rs` | Add `fetch_key_usage()` method |

---

## Color Palette (`cli/src/tui/colors.rs`)

Exact 256-color ANSI indexes captured from CC via tmux `capture-pane -eJp`.

```rust
use ratatui::style::Color;

pub const CC_BLUE: Color     = Color::Indexed(153); // separator, cursor, selected text, title
pub const CC_GREEN: Color    = Color::Indexed(114); // connected/active/checkmark
pub const CC_YELLOW: Color   = Color::Indexed(220); // warning / partial
pub const CC_DIM: Color      = Color::Indexed(246); // dim secondary text
pub const CC_ROSE: Color     = Color::Indexed(174); // logo / cost accent
pub const CC_TAB_BG: Color   = Color::Indexed(153); // active tab background
pub const CC_TAB_FG: Color   = Color::Indexed(16);  // active tab foreground (black)
pub const CC_BAR_BG: Color   = Color::Indexed(102); // usage bar background track
pub const CC_INPUT_BG: Color = Color::Indexed(237); // input bar / modal bg
pub const CC_RED: Color      = Color::Indexed(196); // error / disconnected
```

---

## Modal System (`cli/src/tui/modal.rs`)

```rust
pub enum ModalAction {
    Continue,          // modal stays open, no side effect
    Close,             // dismiss modal, restore input bar
    SetModel(String),  // close + update store.model
    SetEffort(String), // close + update store.effort_level
    ResumeSession(String), // close + load session by id
}

pub trait ModalWidget {
    fn handle_key(&mut self, key: KeyEvent) -> ModalAction;
    fn render(&mut self, frame: &mut Frame, area: Rect);
    fn title(&self) -> &str;
}

pub enum Modal {
    Model(ModelPicker),
    Effort(EffortPicker),
    Status(StatusView),
    Resume(ResumePicker),
    Mcp(McpList),
    Config(ConfigView),
    Agents(AgentsView),
}
```

`CommandResult` in `dispatch.rs` gains a new variant:

```rust
pub enum CommandResult {
    Display(String),
    Prompt(String),
    Clear,
    Quit,
    OpenModal(Modal),  // NEW
}
```

`App` handles `CommandResult::OpenModal(m)` by setting `self.modal = Some(m)`.

The render loop in `App::draw()`:

```rust
if let Some(modal) = &mut self.modal {
    // draw separator + modal in place of input bar
    draw_separator(frame, separator_area, modal.title());
    modal.render(frame, modal_area);
} else {
    self.input.render(frame, input_area);
}
```

Key routing in `App::handle_key()`:

```rust
if let Some(modal) = &mut self.modal {
    match modal.handle_key(key_event) {
        ModalAction::Close => { self.modal = None; }
        ModalAction::SetModel(m) => { self.store.set_model(m); self.modal = None; }
        ModalAction::SetEffort(e) => { self.store.set_effort(e); self.modal = None; }
        ModalAction::ResumeSession(id) => { /* load session */ self.modal = None; }
        ModalAction::Continue => {}
    }
    return; // don't pass to input bar
}
```

---

## `/model` Picker

**File:** `cli/src/tui/modals/model_picker.rs`

### Layout
```
────────────────────────────── Set Model ──────────────────────────────   ← Indexed(153)
  1. claude-opus-4-6         (default)                                    ← Indexed(246) dim
  2. claude-sonnet-4-6       Balanced intelligence and speed               ← Indexed(246)
❯ 3. claude-haiku-4-5        Fastest model for simple tasks               ← Indexed(153) selected
  4. claude-3-5-haiku        ...
  ...
[enter to select] [esc to cancel]                                         ← Indexed(246)
```

### Models List (hardcoded, matching CC's set)

| # | Slug | OR model ID | Description |
|---|------|-------------|-------------|
| 1 | claude-opus-4-7 | `anthropic/claude-opus-4-7` | Most capable model for complex tasks |
| 2 | claude-opus-4-6 | `anthropic/claude-opus-4-6` | Previous opus generation |
| 3 | claude-sonnet-4-6 | `anthropic/claude-sonnet-4-6` | Balanced intelligence and speed |
| 4 | claude-haiku-4-5 | `anthropic/claude-haiku-4-5-20251001` | Fastest model for simple tasks |

### Interaction
- `↑`/`↓`: move cursor
- `1`-`4`: jump to item by number
- `Enter`: emit `ModalAction::SetModel(or_model_id)`
- `Esc`: emit `ModalAction::Close`

### State
```rust
pub struct ModelPicker {
    models: Vec<ModelEntry>,
    cursor: usize,
    current_model: String, // show ✔ next to active
}
```

---

## `/effort` Picker

**File:** `cli/src/tui/modals/effort_picker.rs`

### Layout (captured from CC)
```
──────────────────── Effort Level ─────────────────────────────────────
  Speed ←─────────────────────────────────────────────→ Intelligence
         Low        Medium       High        Max
                      ▲
  Current: medium · Balanced performance and quality
  [← → to adjust] [enter to confirm] [esc to cancel]
```

- Track line: `─────────────────────────────────────────`
- `▲` marker position computed from level index (0=Low, 1=Medium, 2=High, 3=Max)
- Level label under `▲` in `Indexed(153)` bold
- Description line: `Indexed(246)` dim

### Levels
| Value | Label | Description |
|-------|-------|-------------|
| `low` | Low | Minimal processing, fastest response |
| `medium` | Medium | Balanced performance and quality |
| `high` | High | More thorough analysis |
| `max` | Max | Maximum capability, slowest |

### Interaction
- `←`/`→` or `h`/`l`: move marker
- `Enter`: emit `ModalAction::SetEffort(value)`
- `Esc`: emit `ModalAction::Close`

---

## `/status` View

**File:** `cli/src/tui/modals/status_view.rs`

### Tabs
```
 Settings │ Status │ Config │ Usage │ Stats
```
Active tab: fg `Indexed(16)` on bg `Indexed(153)`. Inactive: default fg.

Tab navigation: `←`/`→` or `Tab`/`Shift+Tab`.

### Settings Tab
```
Account         felix.koppe@cavorit.de
Model           claude-sonnet-4-6
Effort          medium
Context left    71k tokens
```

### Status Tab
```
✔ API connection     connected         ← Indexed(114)
✔ Auth               authenticated
✔ OpenRouter         reachable
```

### Config Tab
Key-value dump of `~/.super/config.json` (pretty-printed, truncated to area height).

### Usage Tab (requires server call)

On open, fires `GET {api_base_url}/auth/usage` with bearer token.  
Shows spinner while loading (`⟡` animated), then:

```
API Usage (this period)
████████████████░░░░░░░░  68% used
$6.82 used of $10.00 limit  ·  $3.18 remaining
```
- Filled bar: `Indexed(153)` fg on `Indexed(102)` bg
- Empty bar: spaces on `Indexed(102)` bg
- Error fallback: `"Usage data unavailable"` in `Indexed(220)`

### Stats Tab
```
Messages this session   12
Tool calls              7
Input tokens            14 293
Output tokens           2 847
```

### Interaction
- `←`/`→` or `Tab`/`Shift+Tab`: switch tabs
- `Esc` or `q`: emit `ModalAction::Close`

---

## `/resume` Picker

**File:** `cli/src/tui/modals/resume_picker.rs`

Phase 1: empty state (conversations not persisted yet).

```
─────────────────── Resume Session ────────────────────────────────────
  Search: [                                          ]

  No previous sessions found.
  Sessions will appear here once conversation persistence is implemented.

[esc to cancel]
```

- Search box: same style as input bar, focused on open
- Empty state text: `Indexed(246)` dim
- When sessions exist (future): list with date, first message preview, token count

### Interaction
- Typing: filters list (noop in phase 1)
- `↑`/`↓`: navigate list
- `Enter`: emit `ModalAction::ResumeSession(id)` (noop in phase 1)
- `Esc`: emit `ModalAction::Close`

---

## `/mcp` List

**File:** `cli/src/tui/modals/mcp_list.rs`

Reads `~/.claude/claude_desktop_config.json` and `.claude/claude_desktop_config.json` for server definitions, and `disabledMcpServers` from `~/.super/config.json`.

### Layout
```
─────────────────── MCP Servers ───────────────────────────────────────
  Enabled Servers
  ✔ filesystem        /Users/chwzr/...          stdio      ← Indexed(114)
  ✔ git               /usr/local/bin/...        stdio

  Disabled Servers
  ◯ slack             ...                       sse        ← Indexed(246)

  ✔ enabled  ◯ disabled  △ error
  [enter to toggle] [esc to close]
```

### Interaction
- `↑`/`↓`: move cursor
- `Enter`: toggle enabled/disabled (writes `disabledMcpServers` in `~/.super/config.json`)
- `Esc`: emit `ModalAction::Close`

---

## `/config` View

**File:** `cli/src/tui/modals/config_view.rs`

Read-only display of `~/.super/config.json` fields.

```
─────────────────── Super Config ──────────────────────────────────────
  Search: [                                          ]

  api_base_url        https://api.superplatform.dev
  model               anthropic/claude-sonnet-4-6
  effort              medium
  permissions         ...
```

- Search: filters visible keys (live, case-insensitive substring match)
- Values > 60 chars: truncated with `…`
- Sensitive fields (`access_token`, `refresh_token`, `openrouter_api_key`): shown as `••••••••`

### Interaction
- Typing: filters keys
- `↑`/`↓`: scroll list
- `Esc`: emit `ModalAction::Close`

---

## `/agents` View

**File:** `cli/src/tui/modals/agents_view.rs`

Three tabs: `Agents | Running | Library`

### Agents Tab
Two-tier display matching CC's `/agents` output:

```
─────────────────── Agents ────────────────────────────────────────────
 Agents │ Running │ Library

  User agents (~/.claude/agents/)
    (no custom agents)

  Project agents (.claude/agents/)
    (no project agents)

[esc to close]
```

### Running Tab
Lists currently running agent subprocesses (none expected in v1).

### Library Tab
Lists built-in/bundled agent templates (empty in v1, reserved for future Superpowers agent library).

### Interaction
- `←`/`→` or `Tab`: switch tabs
- `Esc`: emit `ModalAction::Close`

---

## Server: `/auth/usage` Endpoint

**File:** `server/src/routes/auth.rs`

```
GET /auth/usage
Authorization: Bearer <access_token>
```

**Response:**
```json
{
  "used_usd": 6.82,
  "limit_usd": 10.00,
  "remaining_usd": 3.18
}
```

**Flow:**
1. Extract + validate bearer token (same middleware as `/auth/me`)
2. Look up `openrouter_api_key` for the user from DB
3. Call `openrouter_client.fetch_key_usage(key)` → `GET https://openrouter.ai/api/v1/key`
4. Parse OpenRouter response: `data.usage` (USD), `data.limit` (USD)
5. Return `used_usd`, `limit_usd`, `remaining_usd = limit - used`

**Error cases:**
- `401`: invalid/expired token → `{"error": "unauthorized"}`
- `502`: OpenRouter call failed → `{"error": "upstream_error", "message": "..."}`
- `404`: no OR key for user → `{"error": "no_key"}`

**OpenRouter API shape** (`GET /api/v1/key`):
```json
{
  "data": {
    "usage": 6.82,
    "limit": 10.0,
    "is_free_tier": false,
    "rate_limit": { ... }
  }
}
```

---

## Blacklist Additions

Append to `PLAN.md` Feature Blacklist:

```
- `/sandbox` — removed 2026-05-14: sandboxing managed at platform level, not CLI
- `/feedback` — removed 2026-05-14: feedback routed via platform UI, not CLI
```

Remove from:
- `cli/src/commands/registry.rs`: delete `/sandbox` and `/feedback` entries
- `cli/src/commands/dispatch.rs`: delete `"/sandbox" => sandbox()` and `"/feedback" => feedback(args)` arms + their function bodies

---

## Testing

Each modal: manual verification via tmux parity harness (`/tmp/tmuxdrive.sh`) comparing CC output side-by-side.

Server endpoint: `curl -H "Authorization: Bearer <token>" http://localhost:3000/auth/usage` returns valid JSON.

Compile gate: `cargo build --workspace` must pass with no warnings after all changes.

---

## Out of Scope

- Persistent conversation storage for `/resume` (phase 1: empty state only)
- MCP server process management (list only, no start/stop)
- Agent execution from the agents view
- Config editing (read-only for now)
