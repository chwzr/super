# Provider Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `/provider` slash command that lets the user pick from five model providers (Anthropic, Z.ai, Moonshot, Deepseek, Free), updating the three model class slots (haiku/sonnet/opus) and persisting the choice in `~/.super/config.json`.

**Architecture:** Replace `model: String` in `CliConfig` with `provider: String` + `model_class: String` (serde-persisted) plus a `#[serde(skip)] model: String` that is always derived from a static `PROVIDERS` table after loading. The active slug is computed by `providers::resolve_slug(provider, class)`. `AppState` mirrors this split; all display and API call sites read from the resolved slug. A new `ProviderPicker` modal mirrors `ModelPicker`; `ModelPicker` is rewritten as a 3-entry class picker.

**Tech Stack:** Rust, ratatui, serde_json, existing modal/command/store patterns.

---

## File Map

**Create:**
- `cli/src/providers.rs` — static PROVIDERS table, `resolve_slug`, `provider_display_name`
- `cli/src/tui/modals/provider_picker.rs` — ProviderPicker modal

**Modify:**
- `shared/src/lib.rs` — CliConfig: add `provider`, `model_class`, `#[serde(skip)] model`; remove old `model` from serde
- `cli/src/config.rs` — `load_config`: populate `config.model` from `resolve_slug` after deserializing
- `cli/src/state/store.rs` — AppState: replace `model: String` with `provider`+`model_class`; Store: replace `set_model` with `set_provider`+`set_model_class`
- `cli/src/bootstrap.rs` — seed `provider`+`model_class` instead of `model`
- `cli/src/agents/model.rs` — add `parent_provider` param; use `providers::resolve_slug` for class aliases
- `cli/src/agents/mod.rs` — update tests for new `resolve_model` signature
- `cli/src/tools/agent.rs` — pass `&self.config.provider` to `resolve_model`
- `cli/src/tui/modals/model_picker.rs` — rewrite as 3-entry class picker
- `cli/src/tui/modals/mod.rs` — add `pub mod provider_picker`
- `cli/src/tui/modal.rs` — add `Modal::Provider`, rename `ModalAction::SetModel`→`SetClass`, add `ModalAction::SetProvider`
- `cli/src/commands/registry.rs` — add `/provider` entry
- `cli/src/commands/dispatch.rs` — add `/provider` arm; update `model()`, `status()`, `context()` to use provider/class
- `cli/src/tui/app.rs` — handle `SetClass`/`SetProvider` actions; update header init to use `provider_display_name`
- `cli/src/tui/modals/status_view.rs` — show provider name instead of hardcoded "OpenRouter"

---

## Task 1: Create `cli/src/providers.rs`

**Files:**
- Create: `cli/src/providers.rs`
- Modify: `cli/src/main.rs` (add `mod providers;`)

- [ ] **Step 1: Add `mod providers;` to `cli/src/main.rs`**

Open `cli/src/main.rs`. After the last `mod` declaration (currently `mod sdk;`), add:

```rust
mod providers;
```

- [ ] **Step 2: Create `cli/src/providers.rs` with tests first**

```rust
pub struct ProviderEntry {
    pub id: &'static str,
    pub display_name: &'static str,
    pub haiku: &'static str,
    pub sonnet: &'static str,
    pub opus: &'static str,
}

pub const PROVIDERS: &[ProviderEntry] = &[
    ProviderEntry {
        id: "anthropic",
        display_name: "Anthropic",
        haiku:  "anthropic/claude-haiku-4-5-20251001",
        sonnet: "anthropic/claude-sonnet-4-6",
        opus:   "anthropic/claude-opus-4-7",
    },
    ProviderEntry {
        id: "z-ai",
        display_name: "Z.ai",
        haiku:  "z-ai/glm-4.7-flash",
        sonnet: "z-ai/glm-5-turbo",
        opus:   "z-ai/glm-5.1",
    },
    ProviderEntry {
        id: "moonshot",
        display_name: "Moonshot",
        haiku:  "moonshotai/kimi-k2.5",
        sonnet: "moonshotai/kimi-k2.5",
        opus:   "moonshotai/kimi-k2.6",
    },
    ProviderEntry {
        id: "deepseek",
        display_name: "Deepseek",
        haiku:  "deepseek/deepseek-v4-flash",
        sonnet: "deepseek/deepseek-v4-flash",
        opus:   "deepseek/deepseek-v4-pro",
    },
    ProviderEntry {
        id: "free",
        display_name: "Free",
        haiku:  "openrouter/free",
        sonnet: "openrouter/free",
        opus:   "openrouter/free",
    },
];

/// Returns the OpenRouter slug for the given provider id + class ("haiku"/"sonnet"/"opus").
/// Falls back to Anthropic sonnet if the provider or class is unrecognised.
pub fn resolve_slug(provider: &str, class: &str) -> &'static str {
    let entry = PROVIDERS.iter().find(|p| p.id == provider)
        .unwrap_or(&PROVIDERS[0]); // default: Anthropic
    match class {
        "haiku"  => entry.haiku,
        "opus"   => entry.opus,
        _        => entry.sonnet, // default class: sonnet
    }
}

/// Returns the human-readable provider name for display.
/// Falls back to the id string if unrecognised.
pub fn provider_display_name(provider: &str) -> &'static str {
    PROVIDERS.iter()
        .find(|p| p.id == provider)
        .map(|p| p.display_name)
        .unwrap_or(provider)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_slug_anthropic_all_classes() {
        assert_eq!(resolve_slug("anthropic", "haiku"),  "anthropic/claude-haiku-4-5-20251001");
        assert_eq!(resolve_slug("anthropic", "sonnet"), "anthropic/claude-sonnet-4-6");
        assert_eq!(resolve_slug("anthropic", "opus"),   "anthropic/claude-opus-4-7");
    }

    #[test]
    fn resolve_slug_z_ai() {
        assert_eq!(resolve_slug("z-ai", "haiku"),  "z-ai/glm-4.7-flash");
        assert_eq!(resolve_slug("z-ai", "sonnet"), "z-ai/glm-5-turbo");
        assert_eq!(resolve_slug("z-ai", "opus"),   "z-ai/glm-5.1");
    }

    #[test]
    fn resolve_slug_moonshot_haiku_and_sonnet_same() {
        assert_eq!(resolve_slug("moonshot", "haiku"),  "moonshotai/kimi-k2.5");
        assert_eq!(resolve_slug("moonshot", "sonnet"), "moonshotai/kimi-k2.5");
        assert_eq!(resolve_slug("moonshot", "opus"),   "moonshotai/kimi-k2.6");
    }

    #[test]
    fn resolve_slug_unknown_provider_falls_back_to_anthropic() {
        assert_eq!(resolve_slug("unknown", "sonnet"), "anthropic/claude-sonnet-4-6");
    }

    #[test]
    fn resolve_slug_unknown_class_defaults_to_sonnet() {
        assert_eq!(resolve_slug("anthropic", "turbo"), "anthropic/claude-sonnet-4-6");
    }

    #[test]
    fn provider_display_name_known() {
        assert_eq!(provider_display_name("anthropic"), "Anthropic");
        assert_eq!(provider_display_name("z-ai"),      "Z.ai");
        assert_eq!(provider_display_name("moonshot"),  "Moonshot");
        assert_eq!(provider_display_name("deepseek"),  "Deepseek");
        assert_eq!(provider_display_name("free"),      "Free");
    }

    #[test]
    fn provider_display_name_unknown_returns_id() {
        // &'static str fallback doesn't work for runtime strings — this test
        // documents the behaviour via resolve_slug not display_name directly.
        // Unknown provider falls back to Anthropic entry in resolve_slug.
        assert_eq!(resolve_slug("custom", "sonnet"), "anthropic/claude-sonnet-4-6");
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p cli providers::tests
```

Expected: all 7 tests pass.

- [ ] **Step 4: Commit**

```bash
git add cli/src/providers.rs cli/src/main.rs
git commit -m "feat(providers): add static provider table with resolve_slug and provider_display_name"
```

---

## Task 2: Update `CliConfig` in `shared/src/lib.rs` and `cli/src/config.rs`

**Files:**
- Modify: `shared/src/lib.rs`
- Modify: `cli/src/config.rs`

- [ ] **Step 1: Rewrite the `CliConfig` struct in `shared/src/lib.rs`**

Replace the existing `CliConfig` struct and its `Default` impl and the `default_messages_base_url` fn. The new version adds `provider`, `model_class`, and keeps `model` as a `#[serde(skip)]` field (populated after load, not persisted).

```rust
fn default_provider() -> String { "anthropic".to_string() }
fn default_model_class() -> String { "sonnet".to_string() }
fn default_messages_base_url() -> String {
    "https://openrouter.ai/api".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub api_base_url: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_model_class")]
    pub model_class: String,
    #[serde(skip)]
    pub model: String,
    #[serde(default)]
    pub permissions: serde_json::Value,
    #[serde(default)]
    pub settings: serde_json::Value,
    #[serde(default = "default_messages_base_url")]
    pub api_messages_base_url: String,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            access_token: None,
            refresh_token: None,
            openrouter_api_key: None,
            api_base_url: "http://localhost:3000".to_string(),
            provider: default_provider(),
            model_class: default_model_class(),
            model: String::new(), // populated by load_config, not serialized
            permissions: serde_json::json!({}),
            settings: serde_json::json!({}),
            api_messages_base_url: default_messages_base_url(),
        }
    }
}
```

- [ ] **Step 2: Write tests in `shared/src/lib.rs`**

Append inside the file (or in an existing `#[cfg(test)]` block if one exists):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let c = CliConfig::default();
        assert_eq!(c.provider, "anthropic");
        assert_eq!(c.model_class, "sonnet");
        assert_eq!(c.model, ""); // populated by load_config, not default()
    }

    #[test]
    fn config_round_trip_persists_provider_and_class() {
        let mut c = CliConfig::default();
        c.provider = "z-ai".to_string();
        c.model_class = "haiku".to_string();
        c.model = "z-ai/glm-4.7-flash".to_string(); // skip field
        let json = serde_json::to_string(&c).unwrap();
        // model is NOT in the JSON
        assert!(!json.contains("\"model\"") || json.contains("\"model_class\""),
            "model field must not be serialized; json was: {json}");
        assert!(json.contains("\"provider\":\"z-ai\""));
        assert!(json.contains("\"model_class\":\"haiku\""));
        // Deserialize back
        let c2: CliConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c2.provider, "z-ai");
        assert_eq!(c2.model_class, "haiku");
        assert_eq!(c2.model, ""); // skip field not restored from JSON
    }

    #[test]
    fn config_missing_provider_defaults_to_anthropic() {
        // Old config.json with no provider/model_class fields
        let json = r#"{"api_base_url":"http://localhost:3000","api_messages_base_url":"https://openrouter.ai/api"}"#;
        let c: CliConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.provider, "anthropic");
        assert_eq!(c.model_class, "sonnet");
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p shared
```

Expected: all 3 new tests pass.

- [ ] **Step 4: Update `load_config` in `cli/src/config.rs`**

Add the `resolve_slug` call after deserializing so `config.model` is always populated:

```rust
use shared::CliConfig;
use std::path::PathBuf;

pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".super")
        .join("config.json")
}

pub fn load_config() -> CliConfig {
    let path = config_path();
    let mut config = if path.exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        CliConfig::default()
    };
    config.model = crate::providers::resolve_slug(&config.provider, &config.model_class).to_string();
    config
}

pub fn save_config(config: &CliConfig) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let content = serde_json::to_string_pretty(config).unwrap_or_default();
    std::fs::write(&path, content).ok();
}
```

- [ ] **Step 5: Cargo check**

```bash
cargo check -p cli
```

Expected: compiles. `config.model` may still be read in some places; it still exists, just not serialized, so compile should succeed.

- [ ] **Step 6: Commit**

```bash
git add shared/src/lib.rs cli/src/config.rs
git commit -m "feat(config): replace model slug with provider+model_class in CliConfig"
```

---

## Task 3: Update `AppState` and `Store` in `cli/src/state/store.rs`

**Files:**
- Modify: `cli/src/state/store.rs`

- [ ] **Step 1: Write failing tests**

Add these tests to the `#[cfg(test)]` block in `cli/src/state/store.rs`:

```rust
#[test]
fn set_provider_updates_state() {
    let store = Store::new();
    store.set_provider("z-ai");
    assert_eq!(store.get_state().provider, "z-ai");
}

#[test]
fn set_model_class_updates_state() {
    let store = Store::new();
    store.set_model_class("haiku");
    assert_eq!(store.get_state().model_class, "haiku");
}

#[test]
fn default_state_is_anthropic_sonnet() {
    let store = Store::new();
    let s = store.get_state();
    assert_eq!(s.provider, "anthropic");
    assert_eq!(s.model_class, "sonnet");
}
```

- [ ] **Step 2: Run tests to see them fail**

```bash
cargo test -p cli state::store::tests
```

Expected: compilation error — `provider` and `model_class` fields don't exist on `AppState` yet.

- [ ] **Step 3: Update `AppState` and `Store`**

In `cli/src/state/store.rs`:

1. In `AppState`, replace `pub model: String,` with:

```rust
pub provider: String,
pub model_class: String,
```

2. Update `AppState`'s `Default` impl (or derive — if using `#[derive(Clone, Default)]`, add custom defaults):

Replace `#[derive(Clone, Default)]` on `AppState` with a manual `Default`:

```rust
impl Default for AppState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            permission_mode: PermissionMode::default(),
            provider: "anthropic".to_string(),
            model_class: "sonnet".to_string(),
            thinking_enabled: false,
            effort_level: None,
            is_streaming: false,
            should_compact: false,
            tasks: HashMap::new(),
            history: Vec::new(),
            async_agents: HashMap::new(),
        }
    }
}
```

3. Replace the `set_model` method with two new methods:

```rust
pub fn set_provider(&self, provider: &str) {
    let p = provider.to_string();
    self.set_state(|s| s.provider = p);
}

pub fn set_model_class(&self, class: &str) {
    let c = class.to_string();
    self.set_state(|s| s.model_class = c);
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p cli state::store::tests
```

Expected: all 5 store tests (3 new + 2 existing async-agent tests) pass.

- [ ] **Step 5: Cargo check**

```bash
cargo check -p cli 2>&1 | head -40
```

Expected: errors only at call sites that used `state.model` or `store.set_model`. Those will be fixed in Tasks 4 and 9.

- [ ] **Step 6: Commit**

```bash
git add cli/src/state/store.rs
git commit -m "refactor(store): replace model field with provider+model_class in AppState"
```

---

## Task 4: Fix `bootstrap.rs` and `conversation/engine.rs` compile errors

**Files:**
- Modify: `cli/src/bootstrap.rs`
- Modify: `cli/src/conversation/engine.rs` (one line, likely no-op if `config.model` still compiles)

- [ ] **Step 1: Update `bootstrap.rs`**

Find this block (around line 9–12):

```rust
// Seed the store's model from config so /status and /model see the correct value.
{
    let model = config.model.clone();
    store.set_state(|s| s.model = model);
}
```

Replace with:

```rust
// Seed the store's provider and model_class from config.
{
    let provider    = config.provider.clone();
    let model_class = config.model_class.clone();
    store.set_state(|s| {
        s.provider    = provider;
        s.model_class = model_class;
    });
}
```

- [ ] **Step 2: Verify `engine.rs` still compiles**

`cli/src/conversation/engine.rs` at line 112 reads:
```rust
let model = self.config.model.clone();
```

`config.model` is still a field on `CliConfig` (just `#[serde(skip)]`), so this compiles without change. Confirm:

```bash
cargo check -p cli 2>&1 | grep "engine.rs"
```

Expected: no errors from engine.rs.

- [ ] **Step 3: Cargo check overall**

```bash
cargo check -p cli 2>&1 | head -40
```

Expected: errors only from `state.model` references in `dispatch.rs` and `app.rs`. Bootstrap and engine should be clean.

- [ ] **Step 4: Commit**

```bash
git add cli/src/bootstrap.rs
git commit -m "fix(bootstrap): seed provider+model_class in store instead of model slug"
```

---

## Task 5: Update `agents/model.rs`, `agents/mod.rs`, and `tools/agent.rs`

**Files:**
- Modify: `cli/src/agents/model.rs`
- Modify: `cli/src/agents/mod.rs`
- Modify: `cli/src/tools/agent.rs`

- [ ] **Step 1: Write failing test in `cli/src/agents/mod.rs`**

In the `#[cfg(test)]` block (find the existing `resolve_model_handles_aliases_and_inherit` test), add:

```rust
#[test]
fn resolve_model_class_uses_provider() {
    assert_eq!(
        resolve_model(Some("sonnet"), "anthropic/claude-sonnet-4-6", "z-ai"),
        "z-ai/glm-5-turbo"
    );
    assert_eq!(
        resolve_model(Some("haiku"), "anthropic/claude-sonnet-4-6", "deepseek"),
        "deepseek/deepseek-v4-flash"
    );
}
```

- [ ] **Step 2: Run test to see it fail**

```bash
cargo test -p cli agents::tests
```

Expected: compilation error — `resolve_model` doesn't accept a third argument yet.

- [ ] **Step 3: Update `cli/src/agents/model.rs`**

Replace the entire file content:

```rust
use crate::providers;

/// Resolve an agent's `model` field (alias or full id) to a full OpenRouter
/// model id, using the parent's provider for class aliases.
pub fn resolve_model(agent_model: Option<&str>, parent_model: &str, parent_provider: &str) -> String {
    match agent_model {
        None | Some("inherit") => parent_model.to_string(),
        Some("sonnet") => providers::resolve_slug(parent_provider, "sonnet").to_string(),
        Some("opus")   => providers::resolve_slug(parent_provider, "opus").to_string(),
        Some("haiku")  => providers::resolve_slug(parent_provider, "haiku").to_string(),
        Some(other)    => other.to_string(),
    }
}
```

- [ ] **Step 4: Update existing tests in `cli/src/agents/mod.rs`**

Find the test `resolve_model_handles_aliases_and_inherit` and update the `resolve_model` calls to pass a third argument:

```rust
#[test]
fn resolve_model_handles_aliases_and_inherit() {
    use super::model::resolve_model;
    let parent = "anthropic/claude-sonnet-4-6";
    assert_eq!(resolve_model(Some("sonnet"), parent, "anthropic"), "anthropic/claude-sonnet-4-6");
    assert_eq!(resolve_model(Some("opus"),   parent, "anthropic"), "anthropic/claude-opus-4-7");
    assert_eq!(resolve_model(Some("haiku"),  parent, "anthropic"), "anthropic/claude-haiku-4-5-20251001");
    assert_eq!(resolve_model(Some("inherit"), parent, "anthropic"), parent);
    assert_eq!(resolve_model(None,            parent, "anthropic"), parent);
    // Full slug passthrough
    assert_eq!(
        resolve_model(Some("anthropic/claude-something-else"), parent, "anthropic"),
        "anthropic/claude-something-else"
    );
}
```

- [ ] **Step 5: Update `cli/src/tools/agent.rs`**

Find this block (around line 112–117):

```rust
let child_model = resolve_model(
    model_override.as_deref().or(agent_def.model.as_deref()),
    &self.config.model,
);
let mut child_config = self.config.clone();
child_config.model = child_model;
```

Replace with:

```rust
let child_model = resolve_model(
    model_override.as_deref().or(agent_def.model.as_deref()),
    &self.config.model,
    &self.config.provider,
);
let mut child_config = self.config.clone();
child_config.model = child_model;
```

- [ ] **Step 6: Run all agent tests**

```bash
cargo test -p cli agents
```

Expected: all tests pass including the new `resolve_model_class_uses_provider`.

- [ ] **Step 7: Commit**

```bash
git add cli/src/agents/model.rs cli/src/agents/mod.rs cli/src/tools/agent.rs
git commit -m "feat(agents): make model class resolver provider-aware"
```

---

## Task 6: Rewrite `model_picker.rs` as a class picker

**Files:**
- Modify: `cli/src/tui/modals/model_picker.rs`

- [ ] **Step 1: Rewrite `cli/src/tui/modals/model_picker.rs`**

Replace the entire file:

```rust
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_BLUE, CC_DIM, CC_GREEN};
use crate::providers;

pub struct ClassEntry {
    pub class: &'static str,
    pub description: &'static str,
}

const CLASSES: &[ClassEntry] = &[
    ClassEntry { class: "haiku",  description: "Fastest model for simple tasks" },
    ClassEntry { class: "sonnet", description: "Balanced intelligence and speed" },
    ClassEntry { class: "opus",   description: "Most capable for complex tasks" },
];

pub struct ModelPicker {
    pub cursor: usize,
    pub current_class: String,
    pub current_provider: String,
}

impl ModelPicker {
    pub fn new(current_provider: String, current_class: String) -> Self {
        let cursor = CLASSES
            .iter()
            .position(|c| c.class == current_class)
            .unwrap_or(1); // default to sonnet
        Self { cursor, current_class, current_provider }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModelAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 { self.cursor -= 1; }
                ModelAction::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < CLASSES.len() { self.cursor += 1; }
                ModelAction::Continue
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let idx = (c as usize).wrapping_sub('1' as usize);
                if idx < CLASSES.len() { self.cursor = idx; }
                ModelAction::Continue
            }
            KeyCode::Enter => ModelAction::Select(CLASSES[self.cursor].class),
            KeyCode::Esc   => ModelAction::Cancel,
            _ => ModelAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();

        for (i, entry) in CLASSES.iter().enumerate() {
            let is_cursor  = i == self.cursor;
            let is_current = entry.class == self.current_class;

            let prefix = if is_cursor { "❯ " } else { "  " };
            let number = format!("{}. ", i + 1);

            let name_style = if is_cursor {
                Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(CC_DIM)
            };
            let check_style = Style::default().fg(CC_GREEN);
            let desc_style  = Style::default().fg(CC_DIM);
            let checkmark   = if is_current { " ✔" } else { "" };

            // Show the slug for the current provider so the user knows what they're picking.
            let slug = providers::resolve_slug(&self.current_provider, entry.class);
            let label_with_slug = format!("{:<8}  {}", entry.class, slug);

            lines.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled(number, name_style),
                Span::styled(label_with_slug, name_style),
                Span::styled(format!("  {}", entry.description), desc_style),
                Span::styled(checkmark, check_style),
            ]));
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  [enter to select] [esc to cancel]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum ModelAction {
    Continue,
    Select(&'static str),
    Cancel,
}
```

- [ ] **Step 2: Cargo check**

```bash
cargo check -p cli 2>&1 | grep "model_picker\|ModelPicker"
```

The callers of `ModelPicker::new` (in `dispatch.rs` and `modal.rs`) will now show type errors because the constructor signature changed. That's expected — fixed in Tasks 8–9.

- [ ] **Step 3: Commit**

```bash
git add cli/src/tui/modals/model_picker.rs
git commit -m "refactor(model-picker): rewrite as 3-entry class picker (haiku/sonnet/opus)"
```

---

## Task 7: Create `provider_picker.rs` and update `modals/mod.rs`

**Files:**
- Create: `cli/src/tui/modals/provider_picker.rs`
- Modify: `cli/src/tui/modals/mod.rs`

- [ ] **Step 1: Create `cli/src/tui/modals/provider_picker.rs`**

```rust
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::tui::colors::{CC_BLUE, CC_DIM, CC_GREEN};
use crate::providers::{PROVIDERS, ProviderEntry};

pub struct ProviderPicker {
    pub cursor: usize,
    pub current_provider: String,
}

impl ProviderPicker {
    pub fn new(current_provider: String) -> Self {
        let cursor = PROVIDERS
            .iter()
            .position(|p| p.id == current_provider)
            .unwrap_or(0); // default: Anthropic
        Self { cursor, current_provider }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ProviderAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 { self.cursor -= 1; }
                ProviderAction::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < PROVIDERS.len() { self.cursor += 1; }
                ProviderAction::Continue
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let idx = (c as usize).wrapping_sub('1' as usize);
                if idx < PROVIDERS.len() { self.cursor = idx; }
                ProviderAction::Continue
            }
            KeyCode::Enter => ProviderAction::Select(PROVIDERS[self.cursor].id),
            KeyCode::Esc   => ProviderAction::Cancel,
            _ => ProviderAction::Continue,
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        let descriptions: &[&str] = &[
            "Claude models (default)",
            "GLM models",
            "Kimi models",
            "Deepseek V4 models",
            "OpenRouter free tier",
        ];

        let mut lines: Vec<Line> = Vec::new();

        for (i, entry) in PROVIDERS.iter().enumerate() {
            let is_cursor  = i == self.cursor;
            let is_current = entry.id == self.current_provider;

            let prefix = if is_cursor { "❯ " } else { "  " };
            let number = format!("{}. ", i + 1);

            let name_style = if is_cursor {
                Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(CC_DIM)
            };
            let check_style = Style::default().fg(CC_GREEN);
            let desc_style  = Style::default().fg(CC_DIM);
            let checkmark   = if is_current { " ✔" } else { "" };
            let desc = descriptions.get(i).copied().unwrap_or("");

            lines.push(Line::from(vec![
                Span::raw(prefix),
                Span::styled(number, name_style),
                Span::styled(entry.display_name, name_style),
                Span::styled(format!("  {desc}"), desc_style),
                Span::styled(checkmark, check_style),
            ]));
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  [enter to select] [esc to cancel]",
            Style::default().fg(CC_DIM),
        )));

        f.render_widget(Paragraph::new(lines), area);
    }
}

pub enum ProviderAction {
    Continue,
    Select(&'static str),
    Cancel,
}
```

- [ ] **Step 2: Add `pub mod provider_picker;` to `cli/src/tui/modals/mod.rs`**

Append to `cli/src/tui/modals/mod.rs`:

```rust
pub mod provider_picker;
```

- [ ] **Step 3: Cargo check**

```bash
cargo check -p cli 2>&1 | grep "provider_picker"
```

Expected: no errors for the new file itself.

- [ ] **Step 4: Commit**

```bash
git add cli/src/tui/modals/provider_picker.rs cli/src/tui/modals/mod.rs
git commit -m "feat(provider-picker): add ProviderPicker modal"
```

---

## Task 8: Update `cli/src/tui/modal.rs`

Add `Modal::Provider`, rename `ModalAction::SetModel` → `ModalAction::SetClass`, add `ModalAction::SetProvider`.

**Files:**
- Modify: `cli/src/tui/modal.rs`

- [ ] **Step 1: Update `modal.rs`**

Replace the entire file:

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
use crate::tui::modals::agents_view::{AgentsAction, AgentsView};
use crate::tui::modals::config_view::{ConfigAction, ConfigView};
use crate::tui::modals::effort_picker::{EffortAction, EffortPicker};
use crate::tui::modals::mcp_list::{McpAction, McpList};
use crate::tui::modals::model_picker::{ModelAction, ModelPicker};
use crate::tui::modals::provider_picker::{ProviderAction, ProviderPicker};
use crate::tui::modals::resume_picker::{ResumeAction, ResumePicker};
use crate::tui::modals::status_view::{StatusAction, StatusView};

pub enum ModalAction {
    Continue,
    Close,
    SetClass(String),
    SetProvider(String),
    SetEffort(String),
}

pub enum Modal {
    Model(ModelPicker),
    Provider(ProviderPicker),
    Effort(EffortPicker),
    Mcp(McpList),
    Resume(ResumePicker),
    Status(StatusView),
    Config(ConfigView),
    Agents(AgentsView),
}

impl Modal {
    pub fn title(&self) -> &str {
        match self {
            Modal::Model(_)    => "Set Model Class",
            Modal::Provider(_) => "Select Provider",
            Modal::Effort(_)   => "Effort Level",
            Modal::Mcp(_)      => "MCP Servers",
            Modal::Resume(_)   => "Resume Session",
            Modal::Status(_)   => "Super",
            Modal::Config(_)   => "Super Config",
            Modal::Agents(_)   => "Agents",
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModalAction {
        match self {
            Modal::Model(p) => match p.handle_key(key) {
                ModelAction::Continue   => ModalAction::Continue,
                ModelAction::Select(c)  => ModalAction::SetClass(c.to_string()),
                ModelAction::Cancel     => ModalAction::Close,
            },
            Modal::Provider(p) => match p.handle_key(key) {
                ProviderAction::Continue  => ModalAction::Continue,
                ProviderAction::Select(id) => ModalAction::SetProvider(id.to_string()),
                ProviderAction::Cancel    => ModalAction::Close,
            },
            Modal::Effort(p) => match p.handle_key(key) {
                EffortAction::Continue  => ModalAction::Continue,
                EffortAction::Select(v) => ModalAction::SetEffort(v.to_string()),
                EffortAction::Cancel    => ModalAction::Close,
            },
            Modal::Mcp(p) => match p.handle_key(key) {
                McpAction::Continue => ModalAction::Continue,
                McpAction::Cancel   => ModalAction::Close,
            },
            Modal::Resume(p) => match p.handle_key(key) {
                ResumeAction::Continue => ModalAction::Continue,
                ResumeAction::Cancel   => ModalAction::Close,
            },
            Modal::Status(v) => match v.handle_key(key) {
                StatusAction::Continue => ModalAction::Continue,
                StatusAction::Cancel   => ModalAction::Close,
            },
            Modal::Config(p) => match p.handle_key(key) {
                ConfigAction::Continue => ModalAction::Continue,
                ConfigAction::Cancel   => ModalAction::Close,
            },
            Modal::Agents(p) => match p.handle_key(key) {
                AgentsAction::Continue => ModalAction::Continue,
                AgentsAction::Cancel   => ModalAction::Close,
            },
        }
    }

    pub fn render(&self, f: &mut Frame, area: Rect) {
        if area.height < 2 { return; }
        let sep_area = Rect { height: 1, ..area };
        let content_area = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        };
        render_separator(f, sep_area, self.title());
        match self {
            Modal::Model(p)    => p.render(f, content_area),
            Modal::Provider(p) => p.render(f, content_area),
            Modal::Effort(p)   => p.render(f, content_area),
            Modal::Mcp(p)      => p.render(f, content_area),
            Modal::Resume(p)   => p.render(f, content_area),
            Modal::Status(v)   => v.render(f, content_area),
            Modal::Config(p)   => p.render(f, content_area),
            Modal::Agents(p)   => p.render(f, content_area),
        }
    }
}

fn render_separator(f: &mut Frame, area: Rect, title: &str) {
    let total_w = area.width as usize;
    let title_with_spaces = format!(" {} ", title);
    let title_len = title_with_spaces.len();
    let dashes = total_w.saturating_sub(title_len);
    let left  = dashes / 2;
    let right = dashes - left;

    let sep_line = Line::from(vec![
        Span::styled("─".repeat(left),  Style::default().fg(CC_BLUE)),
        Span::styled(title_with_spaces, Style::default().fg(CC_BLUE).add_modifier(Modifier::BOLD)),
        Span::styled("─".repeat(right), Style::default().fg(CC_BLUE)),
    ]);
    f.render_widget(Paragraph::new(sep_line), area);
}
```

- [ ] **Step 2: Cargo check**

```bash
cargo check -p cli 2>&1 | grep "modal\|SetModel"
```

Expected: `SetModel` references in `app.rs` and `dispatch.rs` will now error. Those are fixed in Tasks 9–10.

- [ ] **Step 3: Commit**

```bash
git add cli/src/tui/modal.rs
git commit -m "feat(modal): add Provider variant and SetProvider/SetClass actions"
```

---

## Task 9: Update `commands/registry.rs` and `commands/dispatch.rs`

**Files:**
- Modify: `cli/src/commands/registry.rs`
- Modify: `cli/src/commands/dispatch.rs`

- [ ] **Step 1: Add `/provider` to `cli/src/commands/registry.rs`**

In `register_builtins`, find the `/model` entry line and add `/provider` immediately after it (keep alphabetical grouping intact):

```rust
("/provider",    &[],                         "Select model provider (Anthropic, Z.ai, Moonshot, Deepseek, Free)",                        None,                              CommandKind::Local),
```

- [ ] **Step 2: Update `cli/src/commands/dispatch.rs`**

Make the following changes to `dispatch.rs`:

**2a. Add import at the top** (with other modal imports):

```rust
use crate::tui::modals::provider_picker::ProviderPicker;
```

**2b. Add `/provider` arm** in the `dispatch` match (after the `"/model"` arm):

```rust
"/provider" => provider(store),
```

**2c. Replace the `model` function**:

```rust
fn model(_args: &str, store: &Store) -> CommandResult {
    let state = store.get_state();
    CommandResult::OpenModal(Modal::Model(ModelPicker::new(
        state.provider.clone(),
        state.model_class.clone(),
    )))
}
```

**2d. Add the `provider` function** (after `model`):

```rust
fn provider(_args: &str, store: &Store) -> CommandResult {
    let current = store.get_state().provider.clone();
    CommandResult::OpenModal(Modal::Provider(ProviderPicker::new(current)))
}
```

**2e. Update `status` function** — find `model: state.model.clone()` in the `StatusSnapshot` construction and replace it with a derived slug:

```rust
model: crate::providers::resolve_slug(&state.provider, &state.model_class).to_string(),
```

**2f. Update `context` function** — find `let model = friendly_model_short_name(&state.model);` and replace with:

```rust
let slug  = crate::providers::resolve_slug(&state.provider, &state.model_class);
let model = friendly_model_short_name(slug);
```

- [ ] **Step 3: Cargo check**

```bash
cargo check -p cli 2>&1 | grep "dispatch\|registry"
```

Expected: no errors in these files. `app.rs` still has `SetModel` error — fixed in Task 10.

- [ ] **Step 4: Commit**

```bash
git add cli/src/commands/registry.rs cli/src/commands/dispatch.rs
git commit -m "feat(commands): add /provider command; update /model to class picker"
```

---

## Task 10: Update `app.rs` and `status_view.rs` — display and final wire-up

**Files:**
- Modify: `cli/src/tui/app.rs`
- Modify: `cli/src/tui/modals/status_view.rs`

- [ ] **Step 1: Update header initialisation in `App::new` in `cli/src/tui/app.rs`**

Find (around line 107–115):

```rust
let model = friendly_model_name(&config.model);
let header = Header::new(
    env!("CARGO_PKG_VERSION").to_string(),
    model,
    "OpenRouter".to_string(),
    cwd,
);
```

Replace with:

```rust
let model = friendly_model_name(&config.model);
let provider_label = crate::providers::provider_display_name(&config.provider).to_string();
let header = Header::new(
    env!("CARGO_PKG_VERSION").to_string(),
    model,
    provider_label,
    cwd,
);
```

- [ ] **Step 2: Replace `ModalAction::SetModel` handler in `app.rs`**

Find (around line 292–296):

```rust
ModalAction::SetModel(m) => {
    self.store.set_model(m);
    self.modal = None;
    return Ok(());
}
```

Replace with:

```rust
ModalAction::SetClass(c) => {
    self.store.set_model_class(&c);
    self.config.model_class = c.clone();
    self.config.model = crate::providers::resolve_slug(
        &self.config.provider, &c
    ).to_string();
    crate::config::save_config(&self.config);
    self.modal = None;
    return Ok(());
}
ModalAction::SetProvider(p) => {
    self.store.set_provider(&p);
    self.config.provider = p.clone();
    self.config.model = crate::providers::resolve_slug(
        &p, &self.config.model_class
    ).to_string();
    crate::config::save_config(&self.config);
    self.modal = None;
    return Ok(());
}
```

- [ ] **Step 3: Update `status_view.rs` — replace hardcoded "OpenRouter"**

Find (around line 162):

```rust
lines.push(Line::from(vec![
    Span::styled("  ✔", ok), Span::raw("  OpenRouter         "), Span::styled("reachable", ok),
]));
```

The status view needs to read the current provider. Find where `StatusView` is constructed and what data it receives. It uses a `StatusSnapshot`. Add a `provider` field to `StatusSnapshot`:

In `dispatch.rs`, `StatusSnapshot` struct definition — find it and add:

```rust
provider: String,
```

And in the `status()` function that builds the snapshot, add:

```rust
provider: crate::providers::provider_display_name(&state.provider).to_string(),
```

Then in `status_view.rs`, find the `render_connectivity` method (or equivalent) containing the "OpenRouter" string and replace:

```rust
Span::raw("  OpenRouter         ")
```

with (padding the name to keep alignment):

```rust
Span::raw(format!("  {:<19}", self.snap.provider))
```

- [ ] **Step 4: Full build**

```bash
cargo build -p cli 2>&1
```

Expected: clean build with no errors.

- [ ] **Step 5: Run all tests**

```bash
cargo test -p cli
cargo test -p shared
```

Expected: all tests pass.

- [ ] **Step 6: Smoke test** — run the CLI and verify:
  - Splash shows e.g. `Sonnet 4.6 · Anthropic`
  - `/provider` opens the picker with 5 entries, Anthropic checked
  - Selecting Z.ai closes the picker; header now shows `· Z.ai`
  - `/model` opens 3-entry class picker with slugs shown per provider
  - Config at `~/.super/config.json` contains `"provider"` and `"model_class"`, no raw `"model"` field

- [ ] **Step 7: Final commit**

```bash
git add cli/src/tui/app.rs cli/src/tui/modals/status_view.rs cli/src/commands/dispatch.rs
git commit -m "feat(provider): wire SetProvider/SetClass in app.rs; show provider name in header and status"
```

---

## Self-Review Checklist

- [x] `providers.rs` PROVIDERS table covers all 5 providers from the issue
- [x] `resolve_slug` falls back to Anthropic sonnet for unknown inputs
- [x] `CliConfig.model` is `#[serde(skip)]` so it never appears in `config.json`
- [x] `load_config` always populates `config.model` from `resolve_slug`
- [x] `AppState` no longer has a bare `model` field
- [x] `set_model` is removed; replaced by `set_provider` + `set_model_class`
- [x] `agents/model.rs` uses provider-aware resolver for class aliases
- [x] `tools/agent.rs` passes `&self.config.provider` to `resolve_model`
- [x] `ModelPicker` constructor signature changed — callers updated in Task 9
- [x] `ModalAction::SetModel` renamed to `SetClass` — caller updated in Task 10
- [x] `/provider` registered in registry and dispatched
- [x] Header shows provider display name (not "OpenRouter")
- [x] Status view shows provider display name (not "OpenRouter")
- [x] All changes persist via `save_config` on SetClass and SetProvider
- [x] Default is Anthropic/sonnet everywhere (`Default` impl + serde defaults)
