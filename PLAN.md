# Super — Coding Agent CLI

This document describes **what Super is and what we're building.** For rules on *how* to work in this repo (constraints, forbidden patterns, escalation triggers), see `CLAUDE.md`.

---

## Overview

Super is a coding agent CLI built in Rust that mirrors Claude Code's behavior, toolset, and UX as closely as possible, while routing all model inference through OpenRouter via a managed platform server. It ships with the Superpowers plugin built in, supports subagents, and can be remotely controlled from web and (later) iOS frontends. It can also be launched together with a Git repository inside an E2B sandbox (planned for post-v1).

The north star: **if you've used Claude Code, Super should feel immediately familiar.**

---

## Goals

1. **Behavioral parity with Claude Code.** Identical tool set, identical context-building strategy, identical system prompts.
2. **Built-in agentic enhancements.** The [Superpowers](https://github.com/obra/superpowers) plugin (skills + hook logic) is bundled in by default, not optional.
3. **Provider isolation.** OpenRouter is the sole LLM provider, and credentials are delegated through the Super platform server — never user-provided directly.
4. **Remote control.** CLI sessions can be driven from web and (later) iOS frontends using the same architecture Claude Code uses for remote control.
5. **Sandboxed execution.** The CLI can be launched against a Git repo inside an E2B sandbox (post-v1).

---

## Architecture

### Components

1. **Super CLI** — Rust binary, terminal UI, runs the agent loop.
2. **Platform Server** — Backend that:
   - Handles PKCE-based authentication.
   - Holds the OpenRouter *management key*.
   - Provisions a per-user OpenRouter API key with usage limits.
   - Brokers remote-control sessions between the CLI and frontends.
3. **Web Frontend** — Remote-control client built with Vite + `vp` CLI; UI built exclusively from shadcn, `assistant-ui`, and `tool-ui` components.
4. **iOS App (post-v1)** — Native Swift remote-control client, speaks the same protocol as the web frontend.
5. **E2B Integration (post-v1)** — Launches `super` together with a Git repo inside an E2B sandbox.

### Auth & Credential Flow

- User runs `super login` → PKCE flow against the platform server (mirrors Claude Code's flow).
- Platform server issues/refreshes a per-user OpenRouter API key with quotas.
- CLI receives credentials from the platform server only. It never accepts a raw OpenRouter key from the user.
- Token + config persisted at `~/.super/config.json`.

---

## CLI Implementation

### Stack

- **Language:** Rust.
- **Required crates:**
  - [`rig`](https://github.com/0xPlaygrounds/rig) — LLM/agent framework.
  - [`ratatui`](https://github.com/ratatui/ratatui) — terminal UI.

### Tools, Context & Prompts

- The CLI exposes the **exact same tool set** as Claude Code.
- Context is constructed using the **exact same strategy** as Claude Code (ordering, compaction, file-handling).
- The CLI uses the **exact same system prompts** as Claude Code.

### Subagents

Match Claude Code's subagent surface 1:1 — same invocation semantics, configuration shape, and lifecycle. No wrapper layer, no renamed primitives.

### Superpowers Integration

- The Superpowers plugin (skills + hook) is bundled in by default.
- No generic hook system. Instead, port the *logic* of the Superpowers auto-start hook directly into Super's startup path so skills auto-load on session start.

### Configuration

- Path: `~/.super/config.json`.
- Stores: auth token, refresh token, user-level preferences, model selection, local overrides.

### Headless Mode

The CLI must be launchable in a headless / non-interactive mode. This is required for:
- Sandbox environments (E2B, post-v1).
- Remote-control sessions where the TUI is rendered by a different client.

---

## CLI UI/UX

The UI is deliberately minimal and styled to feel like Claude Code.

### Layout

```
┌─────────────────────────────────────────┐
│                                         │
│   Scroll Area (history)                 │
│   - Splash (on launch)                  │
│   - User messages                       │
│   - Agent messages                      │
│   - Tool calls                          │
│   - Thinking blocks (collapsed)         │
│                                         │
├─────────────────────────────────────────┤
│  [activity indicator] [activity verb]   │
├─────────────────────────────────────────┤
│  > input bar (fixed)                    │
└─────────────────────────────────────────┘
```

### Splash Screen

On launch, render an ASCII-art **diamond** inside the scroll area (matching Claude Code's launch behavior).

### Scroll Area

Chronological log of:
- User messages
- Agent messages
- Tool calls (with results)
- Thinking blocks — **collapsed by default**, shown as `thinking...` with the same animated glow effect Claude Code uses. Expandable on demand.

### Activity Indicator

Sits directly above the input bar, alongside an activity verb (same verbs as Claude Code, e.g. *Thinking*, *Reading*, *Editing*).

- **Active state:** cycles through these glyphs in place, animated: `⟣  ⟡  ⟐  ◈  ⟢`
- **Idle state:** static glyph: `♦`

### Input Bar

Fixed at the bottom of the viewport. Standard text-input affordances.

### Reference Spec

Full UI/UX and implementation details live in `analysis-workspace/` (clean-room specifications). Treat that directory as the source of truth for any UX detail not covered here.

---

## Platform Server

### Responsibilities

1. **Auth.** PKCE login flow (Claude Code parity).
2. **OpenRouter key delegation.**
   - Holds a single OpenRouter management key.
   - Issues a unique, rate-limited OpenRouter API key per user.
   - Rotates / revokes keys as needed.
3. **Remote-control session brokering.**
   - Lets web/iOS frontends attach to a running CLI session.
   - Same architecture as Claude Code's remote-control feature.

### Quotas & Billing

- **Default quota:** $20 USD per user, enforced on the per-user OpenRouter API key.
- **Overage:** users may optionally opt in to usage beyond the $20 cap. Overage is metered and billed separately.
- The platform server is the single source of truth for quota state and is responsible for raising/lowering limits on the underlying OpenRouter key.

### Remote-Control Protocol

The remote-control surface must be **client-agnostic**. The web frontend is the first consumer; a native Swift iOS app will follow, and additional clients may come later.

- The protocol is a transport- and client-neutral API. No web-only assumptions (no browser-specific auth quirks, no component-library-shaped payloads, no HTML/DOM coupling).
- Authentication, session attach/detach, message streaming, and tool-call relay must all be expressible from a non-browser client.
- The protocol is documented independently of the web frontend so the iOS team can implement against the spec rather than reverse-engineering the web client.
- A CLI instance running inside an E2B sandbox (post-v1) attaches via the same protocol — the sandbox is just another place `super` runs.

---

## Web Frontend

### Stack

- **Build tool:** Vite, scaffolded via the `vp` CLI.
- **Component sources (the only ones allowed):**
  1. Standard shadcn components.
  2. The [`assistant-ui`](https://www.assistant-ui.com/) registry.
  3. The [`tool-ui`](https://tool-ui.com) registry.
- **No hand-written components.** Every UI element must come from one of the three registries above. If something isn't expressible with components from those registries, escalate before writing custom code.

### Design Guidelines

Follow Linear's and Apple's design guidelines. Both reference docs live under `design-system/` and are the canonical CI/design source for the frontend.

---

## Post-v1: iOS App

- Native Swift implementation.
- Speaks the remote-control protocol defined above — no protocol additions or changes required for the iOS client to work.
- Not in v1 scope; the only v1 requirement is that the server and protocol be ready for it.

---

## Post-v1: E2B Sandbox Integration

E2B integration ships after v1. v1 architectural decisions must not preclude it.

- The CLI must be launchable in headless / non-interactive mode (already a v1 requirement; see *CLI Implementation → Headless Mode*).
- The remote-control protocol must work for a CLI instance running inside an E2B sandbox attached to a user's frontend session.
- Repo-mount + auto-start semantics should be designed for, even if not implemented in v1.

Once implemented, E2B integration will boot a sandbox, clone/mount a Git repo, start `super`, and expose it for remote control via the platform server.

---

## Out of Scope (v1)

- Generic, user-extensible hook system. Only the Superpowers auto-start logic is ported in.
- Custom UI components in the web frontend. Only shadcn, `assistant-ui`, and `tool-ui` are allowed sources.
- LLM providers other than OpenRouter.
- User-supplied OpenRouter keys (all credentials flow through the platform server).
- iOS app (planned; server must be ready for it).
- E2B sandbox integration (planned; architecture must not block it).
