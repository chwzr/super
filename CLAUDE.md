# CLAUDE.md — Operating Instructions

This file tells coding agents how to work in this repository. It is *not* a description of the product — for that, see `plan.md`. Read both before making changes.

The rules below are hard constraints. If a task appears to require violating one, stop and ask before proceeding.

---

## North Star

If you've used Claude Code, Super should feel immediately familiar. When in doubt about behavior, default to whatever Claude Code does.

---

## Hard Constraints

### Behavioral parity with Claude Code

- The CLI must expose the **exact same tool set** as Claude Code.
- Context must be built using the **exact same strategy** as Claude Code: same ordering, same compaction behavior, same file-handling rules.
- The CLI must use the **exact same system prompts** as Claude Code.
- Subagents must match Claude Code's subagent surface **1:1** — same invocation semantics, configuration shape, and lifecycle. No wrapper layer, no renamed primitives.

If a workflow works in Claude Code, the same workflow must work in Super without modification.

### CLI stack

- **Language:** Rust. No exceptions.
- **Required crates** (mandatory, not suggestions):
  - [`rig`](https://github.com/0xPlaygrounds/rig) — LLM/agent framework.
  - [`ratatui`](https://github.com/ratatui/ratatui) — terminal UI.

### LLM provider

- **OpenRouter is the only LLM provider.** Do not add abstractions for "pluggable providers."
- The CLI must **never** accept a raw OpenRouter API key from the user. All credentials come from the platform server.
- The platform server holds the single OpenRouter *management key* and issues per-user keys with quotas.

### Auth & config

- Login is **PKCE only**, mirroring Claude Code's flow.
- Config and tokens live at `~/.super/config.json`. Do not introduce additional config locations.

### Superpowers plugin

- The [Superpowers](https://github.com/obra/superpowers) plugin (skills + hook) is **bundled in by default**, not optional.
- Do **not** build a generic, user-extensible hook system. Port only the *logic* of the Superpowers auto-start hook directly into Super's startup path so skills auto-load on session start.

### Web frontend

- **Build tool:** Vite, scaffolded via the `vp` CLI.
- **Components — only these three sources are allowed:**
  1. Standard shadcn components.
  2. The [`assistant-ui`](https://www.assistant-ui.com/) registry.
  3. The [`tool-ui`](https://tool-ui.com) registry.
- **No hand-written components.** Every UI element must come from one of the three registries above. If something isn't expressible with those components, **stop and escalate** before writing custom code.
- Follow Linear's and Apple's design guidelines. The canonical references live under `design-system/`.

### Remote control protocol

- The platform server's remote-control surface must be **client-agnostic**.
- No web-only assumptions: no browser-specific auth quirks, no component-library-shaped payloads, no HTML/DOM coupling.
- Authentication, session attach/detach, message streaming, and tool-call relay must all be expressible from a non-browser client (iOS/Swift is the next consumer).
- Document the protocol independently of the web frontend.

---

## Reference Material

Treat these directories as authoritative sources of truth. When details in this file or `plan.md` conflict with them, the workspace specs win — flag the conflict.

- **`analysis-workspace/`** — clean-room CLI UI/UX and implementation specifications. Source of truth for any UX detail not covered in `plan.md`.
- **`design-system/`** — Linear and Apple design guidelines. Source of truth for the web frontend's visual language.

---

## CLI UI/UX — Non-Negotiables

- **Splash:** ASCII-art **diamond** rendered inside the scroll area on launch.
- **Layout:** scroll-area on top, activity row in the middle, fixed input bar at the bottom.
- **Thinking blocks:** collapsed by default, displayed as `thinking...` with the same animated glow effect Claude Code uses.
- **Activity indicator glyphs:**
  - Active (animated, cycling in place): `⟣ ⟡ ⟐ ◈ ⟢`
  - Idle: `♦`
- **Activity verbs:** match Claude Code's set (e.g. *Thinking*, *Reading*, *Editing*).

Do not improvise on the glyph set or verbs. If you need a new one, escalate.

---

## Out of Scope — Do Not Build

Treat these as forbidden in v1 unless explicitly unlocked:

- Generic, user-extensible hook system. Only the Superpowers auto-start logic is ported in.
- Custom UI components in the web frontend. Only shadcn, `assistant-ui`, and `tool-ui` are allowed sources.
- LLM providers other than OpenRouter.
- User-supplied OpenRouter keys (all credentials flow through the platform server).
- iOS app implementation (planned; the server must be ready for it, but do not start building the app).
- E2B sandbox integration (planned; architecture must not block it, but do not implement it yet).

---

## When to Stop and Ask

Stop and surface a question to the user before proceeding if a task:

- Would require deviating from Claude Code's tools, context strategy, system prompts, or subagent surface.
- Would require a non-`rig`, non-`ratatui` crate for the CLI's core agent loop or TUI.
- Would require adding an LLM provider other than OpenRouter, or accepting a user-supplied API key.
- Would require a web frontend UI element not available in shadcn / `assistant-ui` / `tool-ui`.
- Would require a generic hook system, a custom config path, or a new auth flow.
- Conflicts with the specs in `analysis-workspace/` or `design-system/`.

When in doubt: **ask, don't improvise.**
