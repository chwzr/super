# Interactive Slash Commands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace static text output for `/model`, `/effort`, `/status`, `/resume`, `/mcp`, `/config`, `/agents` with interactive ratatui modals matching Claude Code's exact visual style; remove `/sandbox` and `/feedback`; add a server `GET /auth/usage` endpoint backed by the OpenRouter key usage API.

**Architecture:** Modals render below the activity separator, replacing the input bar. A `Modal` enum in `cli/src/tui/modal.rs` holds all concrete picker/viewer types. `App` gains a `modal: Option<Modal>` field; key events route to the modal when set; `CommandResult::OpenModal` triggers the transition. The server gains a new `GET /auth/usage` route that validates the user's JWT, retrieves their OpenRouter key from the DB, and proxies `GET https://openrouter.ai/api/v1/key` to return live usage figures.

**Tech Stack:** Rust, ratatui, tokio, reqwest (server-side), axum, shared crate types, `cli/src/tui/colors.rs` for the CC 256-color palette.

---

## File Map

| Action | File |
|--------|------|
| New | `cli/src/tui/colors.rs` |
| New | `cli/src/tui/modal.rs` |
| New | `cli/src/tui/modals/mod.rs` |
| New | `cli/src/tui/modals/model_picker.rs` |
| New | `cli/src/tui/modals/effort_picker.rs` |
| New | `cli/src/tui/modals/status_view.rs` |
| New | `cli/src/tui/modals/resume_picker.rs` |
| New | `cli/src/tui/modals/mcp_list.rs` |
| New | `cli/src/tui/modals/config_view.rs` |
| New | `cli/src/tui/modals/agents_view.rs` |
| Modify | `cli/src/tui/mod.rs` |
| Modify | `cli/src/tui/app.rs` |
| Modify | `cli/src/commands/dispatch.rs` |
| Modify | `cli/src/commands/registry.rs` |
| Modify | `cli/src/state/store.rs` |
| Modify | `server/src/domain/auth/ports.rs` |
| Modify | `server/src/domain/auth/service.rs` |
| Modify | `server/src/adapters/openrouter_client.rs` |
| Modify | `server/src/routes/auth.rs` |
| Modify | `PLAN.md` |

---

### Task 1: Blacklist Cleanup — Remove `/sandbox` and `/feedback`

**Files:**
- Modify: `cli/src/commands/dispatch.rs`
- Modify: `cli/src/commands/registry.rs`
- Modify: `PLAN.md`

- [ ] **Step 1: Remove from dispatch.rs**

In `cli/src/commands/dispatch.rs`, delete the two match arms and their function bodies:

```rust
// DELETE these two match arms from dispatch():
"/sandbox" => sandbox(),
"/feedback" => feedback(args),
```

Also delete the `sandbox()` and `feedback()` function bodies. Search for `fn sandbox()` and `fn feedback(` and remove them entirely (including the closing `}`).

- [ ] **Step 2: Remove from registry.rs**

In `cli/src/commands/registry.rs`, delete these two lines from the `builtins` vec inside `register_builtins`:

```rust
// DELETE both of these lines:
("/sandbox",     &[],                         "Toggle sandbox settings",                                                          None,                              CommandKind::Local),
("/feedback",    &["/bug"],                   "Submit feedback about Super",                                                      Some("[report]"),                  CommandKind::Local),
```

- [ ] **Step 3: Append to PLAN.md blacklist**

Open `PLAN.md` and append to the Feature Blacklist section:

```markdown
- `/sandbox` — removed 2026-05-14: sandbox managed at platform level, not CLI UX
- `/feedback` — removed 2026-05-14: feedback routed via platform UI, not CLI
```

- [ ] **Step 4: Build to confirm it compiles**

```bash
cd /Users/chwzr/flxkpe/superworkspace/super
cargo build -p cli 2>&1 | grep -E "^error"
```

Expected: no output (zero errors).

- [ ] **Step 5: Commit**

```bash
git add cli/src/commands/dispatch.rs cli/src/commands/registry.rs PLAN.md
git commit -m "feat(cli): blacklist /sandbox and /feedback slash commands"
```

---

### Task 2: Color Constants + Store Helpers

**Files:**
- Create: `cli/src/tui/colors.rs`
- Modify: `cli/src/tui/mod.rs`
- Modify: `cli/src/state/store.rs`

- [ ] **Step 1: Create colors.rs**

Create `cli/src/tui/colors.rs` with the complete content:

```rust
use ratatui::style::Color;

// Exact 256-color ANSI indexes captured from Claude Code via tmux capture-pane.
pub const CC_BLUE: Color     = Color::Indexed(153); // separator, cursor ❯, title, selected text
pub const CC_GREEN: Color    = Color::Indexed(114); // connected / active / ✔ checkmark
pub const CC_YELLOW: Color   = Color::Indexed(220); // warning / degraded
pub const CC_DIM: Color      = Color::Indexed(246); // secondary / placeholder text
pub const CC_ROSE: Color     = Color::Indexed(174); // logo / cost accent
pub const CC_TAB_BG: Color   = Color::Indexed(153); // active tab background
pub const CC_TAB_FG: Color   = Color::Indexed(16);  // active tab foreground (near-black)
pub const CC_BAR_BG: Color   = Color::Indexed(102); // usage progress bar track
pub const CC_INPUT_BG: Color = Color::Indexed(237); // input bar / modal background
pub const CC_RED: Color      = Color::Indexed(196); // error / disconnected
```

- [ ] **Step 2: Expose colors in tui/mod.rs**

Replace the contents of `cli/src/tui/mod.rs` with:

```rust
pub mod activity;
pub mod app;
pub mod colors;
pub mod header;
pub mod input_bar;
pub mod scroll_area;
pub mod slash_menu;
pub mod splash;
```

(Just add `pub mod colors;` — do not remove the existing lines.)

- [ ] **Step 3: Add set_model and set_effort to Store**

In `cli/src/state/store.rs`, add these two methods inside `impl Store { ... }` after the `notify` method (before the closing `}`):

```rust
    pub fn set_model(&self, model: String) {
        self.set_state(|s| s.model = model);
    }

    pub fn set_effort(&self, effort: String) {
        self.set_state(|s| s.effort_level = Some(effort));
    }
```

- [ ] **Step 4: Build**

```bash
cargo build -p cli 2>&1 | grep -E "^error"
```

Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/colors.rs cli/src/tui/mod.rs cli/src/state/store.rs
git commit -m "feat(cli): add CC color palette constants and Store model/effort helpers"
```

---

### Task 3: Modal Infrastructure + `/model` Picker (End-to-End)

This task wires the entire modal system from `CommandResult::OpenModal` through `App` rendering, proven with one working modal (`/model`).

**Files:**
- Create: `cli/src/tui/modal.rs`
- Create: `cli/src/tui/modals/mod.rs`
- Create: `cli/src/tui/modals/model_picker.rs`
- Modify: `cli/src/tui/mod.rs`
- Modify: `cli/src/commands/dispatch.rs`
- Modify: `cli/src/tui/app.rs`

- [ ] **Step 1: Create modals/model_picker.rs**

Create `cli/src/tui/modals/model_picker.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_BLUE, CC_DIM};

pub struct ModelEntry {
    pub label: &'static str,     // display name shown to user
    pub or_id: &'static str,     // OpenRouter model ID
    pub description: &'static str,
}

const MODELS: &[ModelEntry] = &[
    ModelEntry { label: "claude-opus-4-7",    or_id: "anthropic/claude-opus-4-7",            description: "Most capable model for complex tasks" },
    ModelEntry { label: "claude-opus-4-6",    or_id: "anthropic/claude-opus-4-6",            description: "Previous opus generation" },
    ModelEntry { label: "claude-sonnet-4-6",  or_id: "anthropic/claude-sonnet-4-6",          description: "Balanced intelligence and speed" },
    ModelEntry { label: "claude-haiku-4-5",   or_id: "anthropic/claude-haiku-4-5-20251001",  description: "Fastest model for simple tasks" },
];

pub struct ModelPicker {
    pub cursor: usize,
    pub current_or_id: String,
}

impl ModelPicker {
    pub fn new(current_or_id: String) -> Self {
        let cursor = MODELS
            .iter()
            .position(|m| m.or_id == current_or_id)
            .unwrap_or(0);
        Self { cursor, current_or_id }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModelAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                ModelAction::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < MODELS.len() {
                    self.cursor += 1;
                }
                ModelAction::Continue
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let idx = (c as usize) - ('1' as usize);
                if idx < MODELS.len() {
                    self.cursor = idx;
                }
                ModelAction::Continue
            }
            KeyCode::Enter => ModelAction::Select(MODELS[self.cursor].or_id),
            KeyCode::Esc => ModelAction::Cancel,
            _ => ModelAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();

        for (i, entry) in MODELS.iter().enumerate() {
            let is_cursor = i == self.cursor;
            let is_current = entry.or_id == self.current_or_id;

            let prefix = if is_cursor { "❯ " } else { "  " };
            let number = format!("{}. ", i + 1);
            let checkmark = if is_current { " ✔" } else { "  " };

            let name_style = if is_cursor {
                Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(CC_DIM)
            };
            let check_style = Style::default().fg(crate::tui::colors::CC_GREEN);
            let desc_style = Style::default().fg(CC_DIM);

            lines.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled(number, name_style),
                Span::styled(entry.label, name_style),
                Span::styled(format!("  {}", entry.description), desc_style),
                Span::styled(checkmark, check_style),
            ]));
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(
                "  [enter to select] [esc to cancel]",
                Style::default().fg(CC_DIM),
            ),
        ]));

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum ModelAction {
    Continue,
    Select(&'static str), // the OR model ID
    Cancel,
}
```

- [ ] **Step 2: Create modals/mod.rs**

Create `cli/src/tui/modals/mod.rs`:

```rust
pub mod model_picker;
```

- [ ] **Step 3: Create modal.rs**

Create `cli/src/tui/modal.rs`:

```rust
use crossterm::event::KeyEvent;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::CC_BLUE;
use crate::tui::modals::model_picker::{ModelAction, ModelPicker};

/// Action returned by a modal after handling a key press.
pub enum ModalAction {
    Continue,
    Close,
    SetModel(String),
    SetEffort(String),
}

/// All modal variants. Each wraps a concrete picker/viewer type.
pub enum Modal {
    Model(ModelPicker),
    // Additional variants added in subsequent tasks.
}

impl Modal {
    pub fn title(&self) -> &str {
        match self {
            Modal::Model(_) => "Set Model",
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModalAction {
        match self {
            Modal::Model(p) => match p.handle_key(key) {
                ModelAction::Continue => ModalAction::Continue,
                ModelAction::Select(id) => ModalAction::SetModel(id.to_string()),
                ModelAction::Cancel => ModalAction::Close,
            },
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        // Draw separator line with title, then hand off remaining area.
        if area.height < 2 {
            return;
        }
        let sep_area = Rect { height: 1, ..area };
        let content_area = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        };

        render_separator(f, sep_area, self.title());

        match self {
            Modal::Model(p) => p.render(f, content_area),
        }
    }
}

/// Draw ──────── Title ──────── separator using CC_BLUE.
fn render_separator(f: &mut Frame, area: Rect, title: &str) {
    let total_w = area.width as usize;
    let title_with_spaces = format!(" {} ", title);
    let title_len = title_with_spaces.len();
    let dashes = total_w.saturating_sub(title_len);
    let left = dashes / 2;
    let right = dashes - left;

    let sep_line = Line::from(vec![
        Span::styled("─".repeat(left), Style::default().fg(CC_BLUE)),
        Span::styled(
            title_with_spaces,
            Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD),
        ),
        Span::styled("─".repeat(right), Style::default().fg(CC_BLUE)),
    ]);
    f.render_widget(Paragraph::new(sep_line), area);
}
```

- [ ] **Step 4: Expose modal modules in tui/mod.rs**

In `cli/src/tui/mod.rs`, add two lines:

```rust
pub mod modal;
pub mod modals;
```

The file should now be:

```rust
pub mod activity;
pub mod app;
pub mod colors;
pub mod header;
pub mod input_bar;
pub mod modal;
pub mod modals;
pub mod scroll_area;
pub mod slash_menu;
pub mod splash;
```

- [ ] **Step 5: Add OpenModal variant to CommandResult in dispatch.rs**

In `cli/src/commands/dispatch.rs`, find `pub enum CommandResult` (line ~1006) and add the new variant:

```rust
pub enum CommandResult {
    /// Display text in the scroll area.
    Display(String),
    /// Feed text to the LLM as a prompt.
    Prompt(String),
    /// Exit the application.
    Quit,
    /// Wipe the scroll area (caller already cleared store.messages).
    Cleared,
    /// Run the PKCE login flow against the Super platform server.
    Login,
    /// Clear stored auth tokens.
    Logout,
    /// Open an interactive modal, replacing the input bar.
    OpenModal(crate::tui::modal::Modal),
}
```

- [ ] **Step 6: Wire /model in dispatch.rs to return OpenModal**

In `cli/src/commands/dispatch.rs`, at the top add the import:

```rust
use crate::tui::modal::Modal;
use crate::tui::modals::model_picker::ModelPicker;
```

Then find the `model(args, store)` function and **replace it entirely** with:

```rust
fn model(_args: &str, store: &Store) -> CommandResult {
    let current = store.get_state().model.clone();
    CommandResult::OpenModal(Modal::Model(ModelPicker::new(current)))
}
```

Also remove the old `model` match arm body from `dispatch()` (the `"/model" => model(args, store)` call stays, but `args` is now `_args` in the fn signature).

- [ ] **Step 7: Wire App to handle OpenModal, route keys, render modal**

In `cli/src/tui/app.rs`, make these changes:

**7a — Add imports at the top:**

```rust
use crate::tui::modal::{Modal, ModalAction};
```

**7b — Add `modal` field to `App` struct** (after `auth_inflight`):

```rust
    modal: Option<Modal>,
```

**7c — Initialize `modal: None` in `App::new`** (inside the `Self { ... }` block, after `auth_inflight: None`):

```rust
            modal: None,
```

**7d — Handle `CommandResult::OpenModal` in `submit_text`** (inside the `match crate::commands::dispatch::dispatch(...)` block, add after the `Logout` arm):

```rust
                    CommandResult::OpenModal(m) => {
                        self.modal = Some(m);
                    }
```

**7e — Route key events to modal in `handle_event`** (at the very start of the match block, before the `if key.modifiers.contains(KeyModifiers::CONTROL)` check, insert):

```rust
        // Modal intercepts all keys when open.
        if let Some(ref mut modal) = self.modal {
            match modal.handle_key(key) {
                ModalAction::Continue => return Ok(()),
                ModalAction::Close => { self.modal = None; return Ok(()); }
                ModalAction::SetModel(m) => {
                    self.store.set_model(m);
                    self.modal = None;
                    return Ok(());
                }
                ModalAction::SetEffort(e) => {
                    self.store.set_effort(e);
                    self.modal = None;
                    return Ok(());
                }
            }
        }
```

**7f — Render modal in `render`** (find the section that renders `self.input` and `self.render_hint`, and replace with):

```rust
        if let Some(ref modal) = self.modal {
            let modal_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(header_height),
                    Constraint::Length(1),
                    Constraint::Min(1),
                    Constraint::Length(activity_height),
                    Constraint::Min(4),
                ])
                .split(area);
            self.header.render(f, modal_layout[0]);
            self.scroll_area.render(f, modal_layout[2]);
            self.activity.render(f, modal_layout[3]);
            modal.render(f, modal_layout[4]);
        } else {
            // existing layout
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(header_height),
                    Constraint::Length(1),
                    Constraint::Min(1),
                    Constraint::Length(menu_height),
                    Constraint::Length(activity_height),
                    Constraint::Length(input_height),
                    Constraint::Length(hint_height),
                ])
                .split(area);

            self.header.render(f, layout[0]);
            self.scroll_area.render(f, layout[2]);
            if self.slash_menu_open {
                self.slash_menu.render(f, layout[3], &entries);
            }
            self.activity.render(f, layout[4]);
            self.input.render(f, layout[5]);
            self.render_hint(f, layout[6]);
        }
```

Note: Remove the original layout + render block (which currently starts with `let layout = Layout::default()` near the end of `render`) and replace with the if/else above.

- [ ] **Step 8: Build**

```bash
cargo build -p cli 2>&1 | grep -E "^error"
```

Expected: no errors.

- [ ] **Step 9: Smoke-test manually**

```bash
# In a terminal, run super, type /model, verify the numbered list appears
# ❯ cursor should be on current model, ↑/↓ moves it, Enter selects, Esc cancels
cargo run -p cli
```

- [ ] **Step 10: Commit**

```bash
git add cli/src/tui/colors.rs cli/src/tui/modal.rs cli/src/tui/modals/ cli/src/tui/mod.rs cli/src/commands/dispatch.rs cli/src/tui/app.rs
git commit -m "feat(cli): modal system + interactive /model picker"
```

---

### Task 4: `/effort` Picker Modal

**Files:**
- Create: `cli/src/tui/modals/effort_picker.rs`
- Modify: `cli/src/tui/modals/mod.rs`
- Modify: `cli/src/tui/modal.rs`
- Modify: `cli/src/commands/dispatch.rs`

- [ ] **Step 1: Create effort_picker.rs**

Create `cli/src/tui/modals/effort_picker.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_BLUE, CC_DIM};

#[derive(Clone, Copy, PartialEq)]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    Max,
}

impl EffortLevel {
    pub fn all() -> &'static [EffortLevel] {
        &[EffortLevel::Low, EffortLevel::Medium, EffortLevel::High, EffortLevel::Max]
    }

    pub fn label(self) -> &'static str {
        match self {
            EffortLevel::Low    => "Low",
            EffortLevel::Medium => "Medium",
            EffortLevel::High   => "High",
            EffortLevel::Max    => "Max",
        }
    }

    pub fn value(self) -> &'static str {
        match self {
            EffortLevel::Low    => "low",
            EffortLevel::Medium => "medium",
            EffortLevel::High   => "high",
            EffortLevel::Max    => "max",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            EffortLevel::Low    => "Minimal processing, fastest response",
            EffortLevel::Medium => "Balanced performance and quality",
            EffortLevel::High   => "More thorough analysis",
            EffortLevel::Max    => "Maximum capability, slowest",
        }
    }

    fn from_str(s: &str) -> EffortLevel {
        match s {
            "low"  => EffortLevel::Low,
            "high" => EffortLevel::High,
            "max"  => EffortLevel::Max,
            _      => EffortLevel::Medium,
        }
    }
}

pub struct EffortPicker {
    pub level: EffortLevel,
}

impl EffortPicker {
    pub fn new(current: Option<String>) -> Self {
        let level = current
            .as_deref()
            .map(EffortLevel::from_str)
            .unwrap_or(EffortLevel::Medium);
        Self { level }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> EffortAction {
        let all = EffortLevel::all();
        let idx = all.iter().position(|&l| l == self.level).unwrap_or(1);
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => {
                if idx > 0 {
                    self.level = all[idx - 1];
                }
                EffortAction::Continue
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if idx + 1 < all.len() {
                    self.level = all[idx + 1];
                }
                EffortAction::Continue
            }
            KeyCode::Enter => EffortAction::Select(self.level.value()),
            KeyCode::Esc   => EffortAction::Cancel,
            _ => EffortAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let all = EffortLevel::all();
        let idx = all.iter().position(|&l| l == self.level).unwrap_or(1);

        // Build track: evenly spaced label positions.
        // Format: "  Speed ←────────────────────────────→ Intelligence"
        let axis_line = Line::from(vec![
            Span::styled("  Speed ", Style::default().fg(CC_DIM)),
            Span::styled("←", Style::default().fg(CC_DIM)),
            Span::styled("─────────────────────────────", Style::default().fg(CC_DIM)),
            Span::styled("→", Style::default().fg(CC_DIM)),
            Span::styled(" Intelligence", Style::default().fg(CC_DIM)),
        ]);

        // Labels line: "         Low        Medium       High        Max"
        let slot_w = 12usize;
        let mut labels_raw = "  ".to_string();
        for level in all.iter() {
            labels_raw.push_str(&format!("{:<width$}", level.label(), width = slot_w));
        }
        let labels_line = Line::from(Span::styled(labels_raw, Style::default().fg(CC_DIM)));

        // Marker line: ▲ under the selected level.
        let marker_offset = 2 + idx * slot_w + slot_w / 2;
        let mut marker_raw = " ".repeat(marker_offset);
        marker_raw.push('▲');
        let marker_line = Line::from(Span::styled(
            marker_raw,
            Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD),
        ));

        // Current description.
        let desc_line = Line::from(vec![
            Span::styled("  Current: ", Style::default().fg(CC_DIM)),
            Span::styled(
                self.level.label(),
                Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" · {}", self.level.description()),
                Style::default().fg(CC_DIM),
            ),
        ]);

        let hint_line = Line::from(Span::styled(
            "  [← → to adjust] [enter to confirm] [esc to cancel]",
            Style::default().fg(CC_DIM),
        ));

        let lines = vec![
            Line::raw(""),
            axis_line,
            labels_line,
            marker_line,
            Line::raw(""),
            desc_line,
            Line::raw(""),
            hint_line,
        ];

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum EffortAction {
    Continue,
    Select(&'static str),
    Cancel,
}
```

- [ ] **Step 2: Add mod effort_picker to modals/mod.rs**

```rust
pub mod effort_picker;
pub mod model_picker;
```

- [ ] **Step 3: Add Effort variant to Modal enum in modal.rs**

Add at the top of `cli/src/tui/modal.rs`:

```rust
use crate::tui::modals::effort_picker::{EffortAction, EffortPicker};
```

Add `Effort(EffortPicker)` to the `Modal` enum:

```rust
pub enum Modal {
    Model(ModelPicker),
    Effort(EffortPicker),
}
```

Add to `Modal::title()`:

```rust
            Modal::Effort(_) => "Effort Level",
```

Add to `Modal::handle_key()`:

```rust
            Modal::Effort(p) => match p.handle_key(key) {
                EffortAction::Continue => ModalAction::Continue,
                EffortAction::Select(v) => ModalAction::SetEffort(v.to_string()),
                EffortAction::Cancel  => ModalAction::Close,
            },
```

Add to `Modal::render()` match:

```rust
            Modal::Effort(p) => p.render(f, content_area),
```

- [ ] **Step 4: Wire /effort in dispatch.rs**

Add import:

```rust
use crate::tui::modals::effort_picker::EffortPicker;
```

Replace the existing `effort()` function with:

```rust
fn effort(_args: &str, store: &Store) -> CommandResult {
    let current = store.get_state().effort_level.clone();
    CommandResult::OpenModal(Modal::Effort(EffortPicker::new(current)))
}
```

- [ ] **Step 5: Build**

```bash
cargo build -p cli 2>&1 | grep -E "^error"
```

Expected: no errors.

- [ ] **Step 6: Smoke-test**

Run `cargo run -p cli`, type `/effort`. The horizontal slider with `▲` marker should appear. `←`/`→` moves the marker. Enter confirms, Esc cancels.

- [ ] **Step 7: Commit**

```bash
git add cli/src/tui/modals/effort_picker.rs cli/src/tui/modals/mod.rs cli/src/tui/modal.rs cli/src/commands/dispatch.rs
git commit -m "feat(cli): interactive /effort picker modal"
```

---

### Task 5: Server `GET /auth/usage` Endpoint

**Files:**
- Modify: `server/src/domain/auth/ports.rs`
- Modify: `server/src/adapters/openrouter_client.rs`
- Modify: `server/src/domain/auth/service.rs`
- Modify: `server/src/routes/auth.rs`

- [ ] **Step 1: Add fetch_key_usage to OpenRouterProvider trait**

In `server/src/domain/auth/ports.rs`, add to the `OpenRouterProvider` trait:

```rust
    async fn fetch_key_usage(&self, user_key: &str) -> Result<(f64, f64), AuthError>;
    // Returns (used_usd, limit_usd)
```

- [ ] **Step 2: Implement fetch_key_usage in OpenRouterClient**

In `server/src/adapters/openrouter_client.rs`, add a new response struct and implement the method:

```rust
#[derive(Deserialize)]
struct OpenRouterKeyUsageData {
    usage: f64,
    limit: Option<f64>,
}

#[derive(Deserialize)]
struct OpenRouterKeyUsageResponse {
    data: OpenRouterKeyUsageData,
}
```

Inside the `impl OpenRouterProvider for OpenRouterClient` block, add:

```rust
    async fn fetch_key_usage(&self, user_key: &str) -> Result<(f64, f64), AuthError> {
        let resp = self
            .http
            .get("https://openrouter.ai/api/v1/key")
            .bearer_auth(user_key)
            .send()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AuthError::Internal(format!(
                "OpenRouter usage fetch failed: {body}"
            )));
        }

        let usage: OpenRouterKeyUsageResponse = resp
            .json()
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        let used = usage.data.usage;
        let limit = usage.data.limit.unwrap_or(0.0);
        Ok((used, limit))
    }
```

- [ ] **Step 3: Add get_key_usage to AuthService**

In `server/src/domain/auth/service.rs`, add this method inside `impl AuthService`:

```rust
    pub async fn get_key_usage(&self, token: &str) -> Result<(f64, f64, f64), AuthError> {
        let user_id = self.verify_jwt(token)?;
        let api_key = self
            .repo
            .get_active_api_key(&user_id)
            .await?
            .ok_or_else(|| AuthError::Internal("no OpenRouter key for user".into()))?;

        let (used, limit) = self
            .openrouter
            .fetch_key_usage(&api_key.openrouter_key_value)
            .await?;

        Ok((used, limit, (limit - used).max(0.0)))
    }
```

- [ ] **Step 4: Add UsageResponse type and /usage route to auth.rs**

In `server/src/routes/auth.rs`, add after the existing `use` imports:

```rust
use serde::Serialize;

#[derive(Serialize)]
struct UsageResponse {
    used_usd: f64,
    limit_usd: f64,
    remaining_usd: f64,
}
```

Register the route in `routes_with_state`:

```rust
        .route("/usage", get(usage))
```

Add the handler function:

```rust
async fn usage(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<UsageResponse>, (StatusCode, String)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "missing authorization header".into()))?;

    let (used, limit, remaining) = state
        .service
        .get_key_usage(token)
        .await
        .map_err(|e| {
            if matches!(e, shared::AuthError::InvalidToken | shared::AuthError::TokenExpired) {
                (StatusCode::UNAUTHORIZED, e.to_string())
            } else {
                (StatusCode::BAD_GATEWAY, e.to_string())
            }
        })?;

    Ok(Json(UsageResponse {
        used_usd: used,
        limit_usd: limit,
        remaining_usd: remaining,
    }))
}
```

- [ ] **Step 5: Build server**

```bash
cargo build -p server 2>&1 | grep -E "^error"
```

Expected: no errors.

- [ ] **Step 6: Smoke-test the endpoint (requires running server + valid token)**

```bash
# With server running on port 3000 and a valid token from /authorize:
curl -s -H "Authorization: Bearer <YOUR_TOKEN>" http://localhost:3000/auth/usage | python3 -m json.tool
```

Expected JSON:
```json
{
  "used_usd": 0.0,
  "limit_usd": 20.0,
  "remaining_usd": 20.0
}
```

- [ ] **Step 7: Commit**

```bash
git add server/src/domain/auth/ports.rs server/src/adapters/openrouter_client.rs server/src/domain/auth/service.rs server/src/routes/auth.rs
git commit -m "feat(server): add GET /auth/usage endpoint backed by OpenRouter key usage API"
```

---

### Task 6: `/status` Modal with Tabs + Async Usage Fetch

**Files:**
- Create: `cli/src/tui/modals/status_view.rs`
- Modify: `cli/src/tui/modals/mod.rs`
- Modify: `cli/src/tui/modal.rs`
- Modify: `cli/src/tui/app.rs`
- Modify: `cli/src/commands/dispatch.rs`

- [ ] **Step 1: Create status_view.rs**

Create `cli/src/tui/modals/status_view.rs`:

```rust
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use crossterm::event::{KeyCode, KeyEvent};

use crate::tui::colors::{CC_BLUE, CC_DIM, CC_GREEN, CC_TAB_BG, CC_TAB_FG, CC_BAR_BG, CC_YELLOW};

#[derive(Clone, Copy, PartialEq)]
pub enum StatusTab {
    Settings,
    Status,
    Config,
    Usage,
    Stats,
}

impl StatusTab {
    pub fn all() -> &'static [StatusTab] {
        &[StatusTab::Settings, StatusTab::Status, StatusTab::Config, StatusTab::Usage, StatusTab::Stats]
    }

    pub fn label(self) -> &'static str {
        match self {
            StatusTab::Settings => "Settings",
            StatusTab::Status   => "Status",
            StatusTab::Config   => "Config",
            StatusTab::Usage    => "Usage",
            StatusTab::Stats    => "Stats",
        }
    }

    fn next(self) -> StatusTab {
        let all = StatusTab::all();
        let idx = all.iter().position(|&t| t == self).unwrap_or(0);
        all[(idx + 1) % all.len()]
    }

    fn prev(self) -> StatusTab {
        let all = StatusTab::all();
        let idx = all.iter().position(|&t| t == self).unwrap_or(0);
        all[idx.checked_sub(1).unwrap_or(all.len() - 1)]
    }
}

pub enum UsageState {
    Loading,
    Loaded { used_usd: f64, limit_usd: f64 },
    Error(String),
}

pub struct StatusSnapshot {
    pub version: String,
    pub model: String,
    pub thinking: bool,
    pub effort: Option<String>,
    pub email: Option<String>,
    pub messages: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tool_calls: u64,
    pub cwd: String,
}

pub struct StatusView {
    pub tab: StatusTab,
    pub usage: UsageState,
    pub snap: StatusSnapshot,
}

impl StatusView {
    pub fn new(snap: StatusSnapshot) -> Self {
        Self {
            tab: StatusTab::Settings,
            usage: UsageState::Loading,
            snap,
        }
    }

    pub fn set_usage(&mut self, result: Result<(f64, f64), String>) {
        self.usage = match result {
            Ok((used, limit)) => UsageState::Loaded { used_usd: used, limit_usd: limit },
            Err(e) => UsageState::Error(e),
        };
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> StatusAction {
        match key.code {
            KeyCode::Right | KeyCode::Tab => {
                self.tab = self.tab.next();
                StatusAction::Continue
            }
            KeyCode::Left | KeyCode::BackTab => {
                self.tab = self.tab.prev();
                StatusAction::Continue
            }
            KeyCode::Esc | KeyCode::Char('q') => StatusAction::Cancel,
            _ => StatusAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        if area.height == 0 { return; }
        let mut lines: Vec<Line> = Vec::new();

        // Tab bar
        let tab_line: Line = {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(Span::raw(" "));
            for &tab in StatusTab::all() {
                if tab == self.tab {
                    spans.push(Span::styled(
                        format!(" {} ", tab.label()),
                        Style::default().fg(CC_TAB_FG).bg(CC_TAB_BG).add_modifier(Modifier::BOLD),
                    ));
                } else {
                    spans.push(Span::styled(
                        format!(" {} ", tab.label()),
                        Style::default().fg(CC_DIM),
                    ));
                }
                spans.push(Span::raw(" │ "));
            }
            Line::from(spans)
        };
        lines.push(tab_line);
        lines.push(Line::raw(""));

        match self.tab {
            StatusTab::Settings => self.render_settings(&mut lines),
            StatusTab::Status   => self.render_status(&mut lines),
            StatusTab::Config   => self.render_config(&mut lines),
            StatusTab::Usage    => self.render_usage(&mut lines),
            StatusTab::Stats    => self.render_stats(&mut lines),
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  [← → or Tab to switch tabs] [esc to close]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }

    fn kv<'a>(key: &'static str, val: String) -> Line<'a> {
        Line::from(vec![
            Span::styled(format!("  {:<20}", key), Style::default().fg(CC_DIM)),
            Span::raw(val),
        ])
    }

    fn render_settings(&self, lines: &mut Vec<Line>) {
        lines.push(Self::kv("Account", self.snap.email.clone().unwrap_or_else(|| "(not signed in)".into())));
        let model = self.snap.model.rsplit_once('/').map(|(_, r)| r).unwrap_or(&self.snap.model);
        lines.push(Self::kv("Model", model.to_string()));
        lines.push(Self::kv("Effort", self.snap.effort.clone().unwrap_or_else(|| "medium".into())));
        lines.push(Self::kv("Extended thinking", if self.snap.thinking { "enabled".into() } else { "disabled".into() }));
        lines.push(Self::kv("Working dir", self.snap.cwd.clone()));
    }

    fn render_status(&self, lines: &mut Vec<Line>) {
        let ok = Style::default().fg(CC_GREEN);
        let check = Span::styled("✔", ok);
        lines.push(Line::from(vec![check.clone(), Span::raw("  API connection     "), Span::styled("connected", ok)]));
        lines.push(Line::from(vec![check.clone(), Span::raw("  Auth               "), Span::styled("authenticated", ok)]));
        lines.push(Line::from(vec![check,         Span::raw("  OpenRouter         "), Span::styled("reachable", ok)]));
    }

    fn render_config(&self, lines: &mut Vec<Line>) {
        let cfg = crate::config::load_config();
        lines.push(Self::kv("api_base_url", cfg.api_base_url.clone()));
        let model = cfg.model.clone();
        lines.push(Self::kv("model", model));
        lines.push(Self::kv("access_token", if cfg.access_token.is_some() { "••••••••".into() } else { "(none)".into() }));
        lines.push(Self::kv("openrouter_key", if cfg.openrouter_api_key.is_some() { "••••••••".into() } else { "(none)".into() }));
    }

    fn render_usage(&self, lines: &mut Vec<Line>) {
        match &self.usage {
            UsageState::Loading => {
                lines.push(Line::from(Span::styled("  Loading usage data…", Style::default().fg(CC_DIM))));
            }
            UsageState::Error(e) => {
                lines.push(Line::from(Span::styled(
                    format!("  Usage data unavailable: {e}"),
                    Style::default().fg(CC_YELLOW),
                )));
            }
            UsageState::Loaded { used_usd, limit_usd } => {
                let pct = if *limit_usd > 0.0 { (used_usd / limit_usd * 100.0) as usize } else { 0 };
                let bar_total = 24usize;
                let filled = (bar_total * pct / 100).min(bar_total);
                let empty = bar_total - filled;

                let bar_line = Line::from(vec![
                    Span::raw("  "),
                    Span::styled("█".repeat(filled), Style::default().fg(CC_BLUE).bg(CC_BAR_BG)),
                    Span::styled(" ".repeat(empty), Style::default().bg(CC_BAR_BG)),
                    Span::raw(format!("  {}% used", pct)),
                ]);

                lines.push(Line::from(Span::styled(
                    "API Usage (this billing period)",
                    Style::default().fg(CC_DIM).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::raw(""));
                lines.push(bar_line);
                lines.push(Line::raw(""));
                lines.push(Line::from(Span::raw(format!(
                    "  ${:.2} used of ${:.2} limit  ·  ${:.2} remaining",
                    used_usd, limit_usd, (limit_usd - used_usd).max(0.0),
                ))));
            }
        }
    }

    fn render_stats(&self, lines: &mut Vec<Line>) {
        lines.push(Self::kv("Messages this session", self.snap.messages.to_string()));
        lines.push(Self::kv("Tool calls", self.snap.tool_calls.to_string()));
        lines.push(Self::kv("Input tokens", format!("{}", self.snap.input_tokens)));
        lines.push(Self::kv("Output tokens", format!("{}", self.snap.output_tokens)));
    }
}

pub enum StatusAction {
    Continue,
    Cancel,
}
```

- [ ] **Step 2: Add mod status_view to modals/mod.rs**

```rust
pub mod effort_picker;
pub mod model_picker;
pub mod status_view;
```

- [ ] **Step 3: Add Status variant to Modal enum in modal.rs**

Add import:

```rust
use crate::tui::modals::status_view::{StatusAction, StatusView};
```

Add to `Modal` enum:

```rust
    Status(StatusView),
```

Add to `Modal::title()`:

```rust
            Modal::Status(_) => "Super",
```

Add to `Modal::handle_key()`:

```rust
            Modal::Status(v) => match v.handle_key(key) {
                StatusAction::Continue => ModalAction::Continue,
                StatusAction::Cancel   => ModalAction::Close,
            },
```

Add to `Modal::render()`:

```rust
            Modal::Status(v) => v.render(f, content_area),
```

Add new `ModalAction::StatusUsageReady { used_usd: f64, limit_usd: f64 }` — actually, usage updates are handled directly by App (not via ModalAction). No change to ModalAction needed.

- [ ] **Step 4: Add status_fetch channel to App**

In `cli/src/tui/app.rs`:

**4a — Add import:**

```rust
use crate::tui::modals::status_view::StatusSnapshot;
```

**4b — Add field to App struct** (after `auth_inflight`):

```rust
    status_fetch: Option<tokio::sync::mpsc::UnboundedReceiver<Result<(f64, f64), String>>>,
```

**4c — Initialize in App::new:**

```rust
            status_fetch: None,
```

**4d — Handle StatusUsage update in process_pending** (at the start of `process_pending`, before the auth_inflight block):

```rust
        if let Some(rx) = self.status_fetch.as_mut() {
            match rx.try_recv() {
                Ok(result) => {
                    if let Some(Modal::Status(ref mut sv)) = self.modal {
                        sv.set_usage(result.map_err(|e| e));
                    }
                    self.status_fetch = None;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    self.status_fetch = None;
                }
            }
        }
```

**4e — Handle OpenModal(Modal::Status) in submit_text** — after `CommandResult::OpenModal(m)` sets `self.modal = Some(m)`, spawn the usage fetch. Replace:

```rust
                    CommandResult::OpenModal(m) => {
                        self.modal = Some(m);
                    }
```

with:

```rust
                    CommandResult::OpenModal(m) => {
                        let is_status = matches!(m, crate::tui::modal::Modal::Status(_));
                        self.modal = Some(m);
                        if is_status {
                            self.spawn_status_fetch();
                        }
                    }
```

**4f — Add spawn_status_fetch method to App impl:**

```rust
    fn spawn_status_fetch(&mut self) {
        let base_url = self._config.api_base_url.clone();
        let token = crate::config::load_config()
            .access_token
            .unwrap_or_default();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let result = async {
                let resp = reqwest::Client::new()
                    .get(format!("{}/auth/usage", base_url))
                    .bearer_auth(&token)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                if !resp.status().is_success() {
                    return Err(format!("HTTP {}", resp.status()));
                }
                let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
                let used = body["used_usd"].as_f64().unwrap_or(0.0);
                let limit = body["limit_usd"].as_f64().unwrap_or(0.0);
                Ok((used, limit))
            }.await;
            let _ = tx.send(result);
        });
        self.status_fetch = Some(rx);
    }
```

Make sure `reqwest` is in `cli/Cargo.toml`. Check with:

```bash
grep "reqwest" /Users/chwzr/flxkpe/superworkspace/super/cli/Cargo.toml
```

If missing, add `reqwest = { version = "0.11", features = ["json"] }` to `cli/Cargo.toml`.

- [ ] **Step 5: Wire /status in dispatch.rs**

Add import:

```rust
use crate::tui::modals::status_view::{StatusSnapshot, StatusView};
```

Replace the existing `status()` function with:

```rust
fn status(store: &Store) -> CommandResult {
    let state = store.get_state();
    let config = load_config();
    let email = config.access_token.as_ref().map(|_| {
        // Email not stored locally; show placeholder until /me cache is added.
        "signed in".to_string()
    });
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into());

    let snap = StatusSnapshot {
        version: env!("CARGO_PKG_VERSION").to_string(),
        model: state.model.clone(),
        thinking: state.thinking_enabled,
        effort: state.effort_level.clone(),
        email,
        messages: state.messages.len(),
        input_tokens: 0,
        output_tokens: 0,
        tool_calls: 0,
        cwd,
    };
    CommandResult::OpenModal(Modal::Status(StatusView::new(snap)))
}
```

- [ ] **Step 6: Build**

```bash
cargo build -p cli 2>&1 | grep -E "^error"
```

- [ ] **Step 7: Smoke-test**

Run `cargo run -p cli`, type `/status`. The tabbed view should appear. `←`/`→` switches tabs. Usage tab shows "Loading…" then (if server is running with valid token) shows the bar chart.

- [ ] **Step 8: Commit**

```bash
git add cli/src/tui/modals/status_view.rs cli/src/tui/modals/mod.rs cli/src/tui/modal.rs cli/src/tui/app.rs cli/src/commands/dispatch.rs
git commit -m "feat(cli): interactive /status modal with tabs and async usage fetch"
```

---

### Task 7: `/resume` Picker (Empty State)

**Files:**
- Create: `cli/src/tui/modals/resume_picker.rs`
- Modify: `cli/src/tui/modals/mod.rs`
- Modify: `cli/src/tui/modal.rs`
- Modify: `cli/src/commands/dispatch.rs`

- [ ] **Step 1: Create resume_picker.rs**

Create `cli/src/tui/modals/resume_picker.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::CC_DIM;

pub struct ResumePicker {
    pub query: String,
}

impl ResumePicker {
    pub fn new() -> Self {
        Self { query: String::new() }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ResumeAction {
        match key.code {
            KeyCode::Esc => ResumeAction::Cancel,
            KeyCode::Backspace => {
                self.query.pop();
                ResumeAction::Continue
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                ResumeAction::Continue
            }
            _ => ResumeAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let search_display = format!("  Search: [{}]", self.query);
        let lines = vec![
            Line::raw(""),
            Line::from(Span::styled(search_display, Style::default().fg(CC_DIM))),
            Line::raw(""),
            Line::from(Span::styled(
                "  No previous sessions found.",
                Style::default().fg(CC_DIM),
            )),
            Line::from(Span::styled(
                "  Sessions will appear here once conversation persistence is implemented.",
                Style::default().fg(CC_DIM),
            )),
            Line::raw(""),
            Line::from(Span::styled("  [esc to cancel]", Style::default().fg(CC_DIM))),
        ];
        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum ResumeAction {
    Continue,
    Cancel,
}
```

- [ ] **Step 2: Add mod resume_picker to modals/mod.rs**

```rust
pub mod effort_picker;
pub mod model_picker;
pub mod resume_picker;
pub mod status_view;
```

- [ ] **Step 3: Add Resume variant to Modal in modal.rs**

Import + variant + title + key + render (follow the same pattern as previous tasks):

```rust
use crate::tui::modals::resume_picker::{ResumeAction, ResumePicker};
```

```rust
pub enum Modal {
    Model(ModelPicker),
    Effort(EffortPicker),
    Status(StatusView),
    Resume(ResumePicker),
}
```

In `title()`: `Modal::Resume(_) => "Resume Session",`

In `handle_key()`:
```rust
            Modal::Resume(p) => match p.handle_key(key) {
                ResumeAction::Continue => ModalAction::Continue,
                ResumeAction::Cancel   => ModalAction::Close,
            },
```

In `render()`: `Modal::Resume(p) => p.render(f, content_area),`

- [ ] **Step 4: Wire /resume in dispatch.rs**

```rust
use crate::tui::modals::resume_picker::ResumePicker;
```

Replace `fn resume()`:

```rust
fn resume() -> CommandResult {
    CommandResult::OpenModal(Modal::Resume(ResumePicker::new()))
}
```

- [ ] **Step 5: Build + commit**

```bash
cargo build -p cli 2>&1 | grep -E "^error"
git add cli/src/tui/modals/resume_picker.rs cli/src/tui/modals/mod.rs cli/src/tui/modal.rs cli/src/commands/dispatch.rs
git commit -m "feat(cli): /resume modal (empty state with search box)"
```

---

### Task 8: `/mcp` Toggle List

**Files:**
- Create: `cli/src/tui/modals/mcp_list.rs`
- Modify: `cli/src/tui/modals/mod.rs`
- Modify: `cli/src/tui/modal.rs`
- Modify: `cli/src/commands/dispatch.rs`

- [ ] **Step 1: Create mcp_list.rs**

Create `cli/src/tui/modals/mcp_list.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use serde::Deserialize;
use std::collections::HashSet;

use crate::tui::colors::{CC_BLUE, CC_DIM, CC_GREEN};

#[derive(Clone)]
pub struct McpServer {
    pub name: String,
    pub transport: String,
    pub enabled: bool,
}

#[derive(Deserialize, Default)]
struct McpConfig {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: std::collections::HashMap<String, serde_json::Value>,
}

fn load_mcp_servers() -> Vec<McpServer> {
    let disabled = load_disabled_set();
    let mut servers: Vec<McpServer> = Vec::new();

    for path in mcp_config_paths() {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<McpConfig>(&content) {
                for (name, def) in cfg.mcp_servers {
                    let transport = def["type"]
                        .as_str()
                        .or_else(|| def["command"].as_str().map(|_| "stdio"))
                        .unwrap_or("stdio")
                        .to_string();
                    let enabled = !disabled.contains(&name);
                    if !servers.iter().any(|s| s.name == name) {
                        servers.push(McpServer { name, transport, enabled });
                    }
                }
            }
        }
    }
    servers
}

fn mcp_config_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".claude").join("claude_desktop_config.json"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join(".claude").join("claude_desktop_config.json"));
    }
    paths
}

fn load_disabled_set() -> HashSet<String> {
    let cfg = crate::config::load_config();
    // disabledMcpServers stored in settings as JSON array under "disabledMcpServers"
    cfg.settings
        .get("disabledMcpServers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn save_disabled_set(disabled: &HashSet<String>) {
    let mut cfg = crate::config::load_config();
    let arr: Vec<serde_json::Value> = disabled
        .iter()
        .map(|s| serde_json::Value::String(s.clone()))
        .collect();
    if let Some(obj) = cfg.settings.as_object_mut() {
        obj.insert("disabledMcpServers".into(), serde_json::Value::Array(arr));
    }
    crate::config::save_config(&cfg);
}

pub struct McpList {
    pub servers: Vec<McpServer>,
    pub cursor: usize,
}

impl McpList {
    pub fn new() -> Self {
        Self {
            servers: load_mcp_servers(),
            cursor: 0,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> McpAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 { self.cursor -= 1; }
                McpAction::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < self.servers.len() { self.cursor += 1; }
                McpAction::Continue
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(srv) = self.servers.get_mut(self.cursor) {
                    srv.enabled = !srv.enabled;
                    // Persist immediately.
                    let disabled: HashSet<String> = self
                        .servers
                        .iter()
                        .filter(|s| !s.enabled)
                        .map(|s| s.name.clone())
                        .collect();
                    save_disabled_set(&disabled);
                }
                McpAction::Continue
            }
            KeyCode::Esc => McpAction::Cancel,
            _ => McpAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::raw(""));

        if self.servers.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No MCP servers configured.",
                Style::default().fg(CC_DIM),
            )));
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "  Add servers in ~/.claude/claude_desktop_config.json",
                Style::default().fg(CC_DIM),
            )));
        } else {
            for (i, srv) in self.servers.iter().enumerate() {
                let is_cursor = i == self.cursor;
                let prefix = if is_cursor { "❯ " } else { "  " };
                let (glyph, glyph_style) = if srv.enabled {
                    ("✔", Style::default().fg(CC_GREEN))
                } else {
                    ("◯", Style::default().fg(CC_DIM))
                };
                let name_style = if is_cursor {
                    Style::default().fg(CC_BLUE)
                } else {
                    Style::default().fg(CC_DIM)
                };
                lines.push(Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(glyph, glyph_style),
                    Span::raw("  "),
                    Span::styled(format!("{:<24}", srv.name), name_style),
                    Span::styled(srv.transport.clone(), Style::default().fg(CC_DIM)),
                ]));
            }
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  ✔ enabled  ◯ disabled   [enter to toggle] [esc to close]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum McpAction {
    Continue,
    Cancel,
}
```

- [ ] **Step 2: Check that CliConfig has a settings field**

Open `shared/src/lib.rs` and verify that `CliConfig` has:

```rust
pub settings: Option<serde_json::Value>,
```

If not, add it. Also verify `config::save_config` is available in `cli/src/config.rs`.

- [ ] **Step 3: Add mod mcp_list, Modal::Mcp variant, wire dispatch**

Follow the exact same pattern as previous tasks:
- Add `pub mod mcp_list;` to `modals/mod.rs`
- Import + add `Mcp(McpList)` to `Modal` enum in `modal.rs`
- Add `Modal::Mcp(_) => "MCP Servers"` in `title()`
- Add key routing in `handle_key()` mapping `McpAction` to `ModalAction`
- Add `Modal::Mcp(p) => p.render(f, content_area)` in `render()`
- In `dispatch.rs`: add import `use crate::tui::modals::mcp_list::McpList;`
- Replace `fn mcp(args: &str)` with:

```rust
fn mcp(_args: &str) -> CommandResult {
    CommandResult::OpenModal(Modal::Mcp(McpList::new()))
}
```

- [ ] **Step 4: Build + commit**

```bash
cargo build -p cli 2>&1 | grep -E "^error"
git add cli/src/tui/modals/mcp_list.rs cli/src/tui/modals/mod.rs cli/src/tui/modal.rs cli/src/commands/dispatch.rs
git commit -m "feat(cli): interactive /mcp toggle list modal"
```

---

### Task 9: `/config` Viewer

**Files:**
- Create: `cli/src/tui/modals/config_view.rs`
- Modify: `cli/src/tui/modals/mod.rs`
- Modify: `cli/src/tui/modal.rs`
- Modify: `cli/src/commands/dispatch.rs`

- [ ] **Step 1: Create config_view.rs**

Create `cli/src/tui/modals/config_view.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_BLUE, CC_DIM};

const SENSITIVE_KEYS: &[&str] = &["access_token", "refresh_token", "openrouter_api_key"];

pub struct ConfigEntry {
    pub key: String,
    pub value: String,
}

pub struct ConfigView {
    pub query: String,
    pub entries: Vec<ConfigEntry>,
    pub scroll: usize,
}

impl ConfigView {
    pub fn new() -> Self {
        let entries = Self::load_entries();
        Self { query: String::new(), entries, scroll: 0 }
    }

    fn load_entries() -> Vec<ConfigEntry> {
        let cfg = crate::config::load_config();
        // Serialize to JSON then iterate keys for a generic view.
        let val = serde_json::to_value(&cfg).unwrap_or(serde_json::Value::Null);
        let mut entries = Vec::new();
        if let Some(obj) = val.as_object() {
            for (k, v) in obj {
                let display = if SENSITIVE_KEYS.contains(&k.as_str()) && !v.is_null() {
                    "••••••••".to_string()
                } else {
                    match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => "(not set)".to_string(),
                        other => other.to_string(),
                    }
                };
                entries.push(ConfigEntry { key: k.clone(), value: display });
            }
        }
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        entries
    }

    fn filtered(&self) -> Vec<&ConfigEntry> {
        let q = self.query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| q.is_empty() || e.key.to_lowercase().contains(&q))
            .collect()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ConfigAction {
        match key.code {
            KeyCode::Esc => ConfigAction::Cancel,
            KeyCode::Backspace => {
                self.query.pop();
                self.scroll = 0;
                ConfigAction::Continue
            }
            KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                ConfigAction::Continue
            }
            KeyCode::Down => {
                self.scroll += 1;
                ConfigAction::Continue
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.scroll = 0;
                ConfigAction::Continue
            }
            _ => ConfigAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let filtered = self.filtered();
        let mut lines: Vec<Line> = Vec::new();

        lines.push(Line::from(vec![
            Span::styled("  Search: ", Style::default().fg(CC_DIM)),
            Span::raw(format!("[{}]", self.query)),
        ]));
        lines.push(Line::raw(""));

        let visible_start = self.scroll.min(filtered.len().saturating_sub(1));
        for entry in filtered.iter().skip(visible_start) {
            let val_display = if entry.value.len() > 60 {
                format!("{}…", &entry.value[..57])
            } else {
                entry.value.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<28}", entry.key), Style::default().fg(CC_BLUE)),
                Span::styled(val_display, Style::default().fg(CC_DIM)),
            ]));
        }

        if filtered.is_empty() {
            lines.push(Line::from(Span::styled("  (no matching keys)", Style::default().fg(CC_DIM))));
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  [type to filter] [↑↓ to scroll] [esc to close]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum ConfigAction {
    Continue,
    Cancel,
}
```

- [ ] **Step 2: Add mod, variant, wire — same pattern as previous tasks**

- `pub mod config_view;` in `modals/mod.rs`
- `Config(ConfigView)` in `Modal` enum
- `Modal::Config(_) => "Super Config"` in `title()`
- Key routing: `ConfigAction::Cancel => ModalAction::Close`, else `ModalAction::Continue`
- Render dispatch
- In `dispatch.rs`, replace `fn config_panel()` with:

```rust
fn config_panel() -> CommandResult {
    CommandResult::OpenModal(Modal::Config(crate::tui::modals::config_view::ConfigView::new()))
}
```

- [ ] **Step 3: Build + commit**

```bash
cargo build -p cli 2>&1 | grep -E "^error"
git add cli/src/tui/modals/config_view.rs cli/src/tui/modals/mod.rs cli/src/tui/modal.rs cli/src/commands/dispatch.rs
git commit -m "feat(cli): /config read-only viewer modal with search"
```

---

### Task 10: `/agents` Tabbed View

**Files:**
- Create: `cli/src/tui/modals/agents_view.rs`
- Modify: `cli/src/tui/modals/mod.rs`
- Modify: `cli/src/tui/modal.rs`
- Modify: `cli/src/commands/dispatch.rs`

- [ ] **Step 1: Create agents_view.rs**

Create `cli/src/tui/modals/agents_view.rs`:

```rust
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_DIM, CC_TAB_BG, CC_TAB_FG};

#[derive(Clone, Copy, PartialEq)]
pub enum AgentsTab {
    Agents,
    Running,
    Library,
}

impl AgentsTab {
    fn all() -> &'static [AgentsTab] {
        &[AgentsTab::Agents, AgentsTab::Running, AgentsTab::Library]
    }
    fn label(self) -> &'static str {
        match self {
            AgentsTab::Agents  => "Agents",
            AgentsTab::Running => "Running",
            AgentsTab::Library => "Library",
        }
    }
    fn next(self) -> AgentsTab {
        let all = Self::all();
        let idx = all.iter().position(|&t| t == self).unwrap_or(0);
        all[(idx + 1) % all.len()]
    }
    fn prev(self) -> AgentsTab {
        let all = Self::all();
        let idx = all.iter().position(|&t| t == self).unwrap_or(0);
        all[idx.checked_sub(1).unwrap_or(all.len() - 1)]
    }
}

fn scan_agents(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().into_string().ok()?;
            if name.ends_with(".md") { Some(name.trim_end_matches(".md").to_string()) } else { None }
        })
        .collect()
}

pub struct AgentsView {
    pub tab: AgentsTab,
}

impl AgentsView {
    pub fn new() -> Self {
        Self { tab: AgentsTab::Agents }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> AgentsAction {
        match key.code {
            KeyCode::Right | KeyCode::Tab => { self.tab = self.tab.next(); AgentsAction::Continue }
            KeyCode::Left | KeyCode::BackTab => { self.tab = self.tab.prev(); AgentsAction::Continue }
            KeyCode::Esc | KeyCode::Char('q') => AgentsAction::Cancel,
            _ => AgentsAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();

        // Tab bar
        let mut spans: Vec<Span> = vec![Span::raw(" ")];
        for &tab in AgentsTab::all() {
            if tab == self.tab {
                spans.push(Span::styled(
                    format!(" {} ", tab.label()),
                    Style::default().fg(CC_TAB_FG).bg(CC_TAB_BG).add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(format!(" {} ", tab.label()), Style::default().fg(CC_DIM)));
            }
            spans.push(Span::raw(" │ "));
        }
        lines.push(Line::from(spans));
        lines.push(Line::raw(""));

        match self.tab {
            AgentsTab::Agents  => self.render_agents(&mut lines),
            AgentsTab::Running => self.render_running(&mut lines),
            AgentsTab::Library => self.render_library(&mut lines),
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  [← → or Tab to switch] [esc to close]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }

    fn render_agents(&self, lines: &mut Vec<Line>) {
        let user_agents = dirs::home_dir()
            .map(|h| scan_agents(&h.join(".claude").join("agents")))
            .unwrap_or_default();
        let proj_agents = std::env::current_dir()
            .map(|cwd| scan_agents(&cwd.join(".claude").join("agents")))
            .unwrap_or_default();

        lines.push(Line::from(Span::styled(
            "  User agents (~/.claude/agents/)",
            Style::default().fg(CC_DIM).add_modifier(Modifier::BOLD),
        )));
        if user_agents.is_empty() {
            lines.push(Line::from(Span::styled("    (no custom agents)", Style::default().fg(CC_DIM))));
        } else {
            for name in &user_agents {
                lines.push(Line::from(Span::styled(format!("    {name}"), Style::default().fg(CC_DIM))));
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  Project agents (.claude/agents/)",
            Style::default().fg(CC_DIM).add_modifier(Modifier::BOLD),
        )));
        if proj_agents.is_empty() {
            lines.push(Line::from(Span::styled("    (no project agents)", Style::default().fg(CC_DIM))));
        } else {
            for name in &proj_agents {
                lines.push(Line::from(Span::styled(format!("    {name}"), Style::default().fg(CC_DIM))));
            }
        }
    }

    fn render_running(&self, lines: &mut Vec<Line>) {
        lines.push(Line::from(Span::styled("  No running agents.", Style::default().fg(CC_DIM))));
    }

    fn render_library(&self, lines: &mut Vec<Line>) {
        lines.push(Line::from(Span::styled(
            "  Agent library not yet available.",
            Style::default().fg(CC_DIM),
        )));
    }
}

pub enum AgentsAction {
    Continue,
    Cancel,
}
```

- [ ] **Step 2: Add mod, variant, wire — same pattern**

- `pub mod agents_view;` in `modals/mod.rs`
- `Agents(AgentsView)` in `Modal` enum
- `Modal::Agents(_) => "Agents"` in `title()`
- Key/render dispatch
- In `dispatch.rs`, replace `fn agents()`:

```rust
fn agents() -> CommandResult {
    CommandResult::OpenModal(Modal::Agents(crate::tui::modals::agents_view::AgentsView::new()))
}
```

- [ ] **Step 3: Final build of all packages**

```bash
cargo build --workspace 2>&1 | grep -E "^error"
```

Expected: no errors.

- [ ] **Step 4: Verify all commands in tmux**

Using the parity harness (`/tmp/tmuxdrive.sh`), test each interactive command:
- `/model` — numbered list, cursor moves, enter sets model
- `/effort` — slider moves left/right, enter confirms
- `/status` — tabs switch, Usage tab shows data or loading
- `/resume` — search box, empty state message
- `/mcp` — list of servers (or empty), enter toggles
- `/config` — key-value list, type to filter
- `/agents` — tabs for Agents/Running/Library

- [ ] **Step 5: Commit**

```bash
git add cli/src/tui/modals/agents_view.rs cli/src/tui/modals/mod.rs cli/src/tui/modal.rs cli/src/commands/dispatch.rs
git commit -m "feat(cli): /agents tabbed viewer modal + all interactive commands complete"
```
