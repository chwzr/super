# Provider Command Design

**Issue:** #15  
**Date:** 2026-05-15  
**Status:** Approved

## Overview

Introduce a `/provider` slash command that lets the user select which model provider backs the three model classes (haiku / sonnet / opus). The selection persists in `~/.super/config.json`. The splash/header line changes from `"Sonnet 4.6 · OpenRouter"` to `"Sonnet 4.6 · Anthropic"` (or whichever provider is active). Default is Anthropic.

## Section 1 — Data Model

### `shared/src/lib.rs` — `CliConfig`

Remove `model: String`. Add two fields:

```rust
#[serde(default = "default_provider")]
pub provider: String,       // e.g. "anthropic"

#[serde(default = "default_model_class")]
pub model_class: String,    // "haiku" | "sonnet" | "opus"
```

Defaults: `provider = "anthropic"`, `model_class = "sonnet"`.

Existing config files that have `model` but lack these fields will deserialize to the defaults (sonnet/Anthropic).

### New file: `cli/src/providers.rs`

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
        id: "anthropic", display_name: "Anthropic",
        haiku:  "anthropic/claude-haiku-4-5-20251001",
        sonnet: "anthropic/claude-sonnet-4-6",
        opus:   "anthropic/claude-opus-4-7",
    },
    ProviderEntry {
        id: "z-ai", display_name: "Z.ai",
        haiku:  "z-ai/glm-4.7-flash",
        sonnet: "z-ai/glm-5-turbo",
        opus:   "z-ai/glm-5.1",
    },
    ProviderEntry {
        id: "moonshot", display_name: "Moonshot",
        haiku:  "moonshotai/kimi-k2.5",
        sonnet: "moonshotai/kimi-k2.5",
        opus:   "moonshotai/kimi-k2.6",
    },
    ProviderEntry {
        id: "deepseek", display_name: "Deepseek",
        haiku:  "deepseek/deepseek-v4-flash",
        sonnet: "deepseek/deepseek-v4-flash",
        opus:   "deepseek/deepseek-v4-pro",
    },
    ProviderEntry {
        id: "free", display_name: "Free",
        haiku:  "openrouter/free",
        sonnet: "openrouter/free",
        opus:   "openrouter/free",
    },
];

/// Returns the OpenRouter slug for a given provider id + class.
/// Falls back to Anthropic sonnet if either is unrecognised.
/// Implementation: find PROVIDERS entry by id, match class arm (haiku/sonnet/opus).
pub fn resolve_slug(provider: &str, class: &str) -> &'static str { /* … */ }

/// Returns the display name for a provider id (e.g. "anthropic" → "Anthropic").
/// Implementation: find PROVIDERS entry by id, return display_name; fall back to provider id.
pub fn provider_display_name(provider: &str) -> &'static str { /* … */ }
```

`resolve_slug` is the **single source of truth** for the active OpenRouter slug. Every call site that previously read `config.model` calls this instead.

### Store state

`Store` currently holds `model: String`. Replace with `provider: String` + `model_class: String`. Add:

- `store.set_provider(p: &str)` — updates provider, saves config
- `store.set_model_class(c: &str)` — updates model_class, saves config

Both persist immediately to `~/.super/config.json`.

## Section 2 — `/provider` Command

### Registry (`commands/registry.rs`)

Add one entry:

```
("/provider", &[], "Select model provider", None, CommandKind::Local)
```

### Dispatch (`commands/dispatch.rs`)

```rust
"/provider" => provider(store),
```

```rust
fn provider(store: &Store) -> CommandResult {
    let current = store.get_state().provider.clone();
    CommandResult::OpenModal(Modal::Provider(ProviderPicker::new(current)))
}
```

### `ProviderPicker` (`tui/modals/provider_picker.rs`)

Mirrors `ModelPicker` in structure:

- Lists 5 providers by display name with a one-line description
- Arrow keys / `j`/`k` / number keys (1–5) to navigate
- Enter to select, Esc to cancel
- Checkmark on the currently active provider
- On `Enter`: calls `store.set_provider(selected_id)`

Provider descriptions:

| Provider | Description |
|----------|-------------|
| Anthropic | Claude models (default) |
| Z.ai | GLM models |
| Moonshot | Kimi models |
| Deepseek | Deepseek V4 models |
| Free | OpenRouter free tier |

## Section 3 — Updated `/model` Command

`ModelPicker` becomes a 3-entry **class picker**. It is constructed with the current `provider` and `model_class` to display the right slug label per row.

```rust
pub struct ClassEntry {
    pub class: &'static str,      // "haiku" | "sonnet" | "opus"
    pub description: &'static str,
}
```

The label shown for each row is `friendly_model_name(resolve_slug(provider, class))` — so on Z.ai, the sonnet row shows the GLM-5 Turbo display name.

| Class | Description |
|-------|-------------|
| haiku | Fastest model for simple tasks |
| sonnet | Balanced intelligence and speed |
| opus | Most capable for complex tasks |

On `Enter`: calls `store.set_model_class(selected_class)`.

## Section 4 — Display Updates

### Header (`tui/app.rs` + `tui/header.rs`)

Replace the hardcoded `"OpenRouter".to_string()` with:

```rust
providers::provider_display_name(&config.provider).to_string()
```

Result: splash/header shows e.g. `"Sonnet 4.6 · Anthropic"`.

### `friendly_model_name` (`tui/app.rs`)

The existing Anthropic slug→name mappings stay. Non-Anthropic slugs fall through to the existing catch-all: `slug.rsplit_once('/').map(|(_, r)| r).unwrap_or(slug)`. No signature change needed — it continues to accept a full slug string.

### Status view (`tui/modals/status_view.rs`)

The connectivity row currently reads `"OpenRouter reachable"`. Change to:

```rust
format!("{}  reachable", providers::provider_display_name(&store.get_state().provider))
```

The underlying connectivity check (hitting the OpenRouter endpoint) is unchanged; only the label reflects the selected provider brand.

## Section 5 — Agent Model Resolver

`cli/src/agents/model.rs` currently resolves class aliases to hardcoded Anthropic slugs. Make it provider-aware:

```rust
pub fn resolve_model(alias: &str, provider: &str) -> &'static str {
    match alias {
        "haiku" | "sonnet" | "opus" => providers::resolve_slug(provider, alias),
        other => other, // already a full slug, pass through unchanged
    }
}
```

All callers thread in `provider` from the store. No behavioral change for the Anthropic default; other providers' class aliases now resolve to their correct slugs.

## Non-Goals

- Per-class provider selection (every class uses the same provider)
- User-supplied model slugs outside the provider table
- Adding new providers at runtime
