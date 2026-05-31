# WebFetch AI Processing

## Summary

Replace the placeholder in `web_fetch.rs:145` that ignores the user's `prompt` argument
with actual AI-based processing: convert HTML to markdown, send the content + prompt
to a small model via OpenRouter, and return the model's response. Mirror Claude Code's
`WebFetchTool` implementation.

## Architecture

Two structural changes enable tools to make their own LLM API calls:

1. **Thread API credentials through `ToolCallContext`** — add `api_key`,
   `api_messages_base_url`, and `provider` fields so any tool can call
   OpenRouter independently of the conversation engine loop.

2. **Rewrite `WebFetchTool::call`** — after fetching and converting HTML to
   markdown, call OpenRouter with a Haiku-equivalent model to process the
   content against the user's prompt.

## Data flow

```
URL → reqwest GET (30s timeout)
    → html2md::parse_html (HTML → markdown)
    → truncate to 100KB
    → check preapproved domain + under 100KB? → return raw markdown (skip AI)
    → build prompt: "Web page content:\n---\n{markdown}\n---\n\n{prompt}\n\nBe concise."
    → OpenRouter POST /v1/chat/completions (non-streaming, resolve_slug(provider, "haiku"))
    → return AI response as "result"
```

## Files changed

| File | Change |
|---|---|
| `cli/src/tools/contract.rs` | Add `api_key: Option<String>`, `api_messages_base_url: String`, `provider: String` to `ToolCallContext` |
| `cli/src/conversation/engine.rs` | Populate new `ToolCallContext` fields from `self.config` |
| `cli/Cargo.toml` | Add `html2md` crate dependency |
| `cli/src/tools/web_fetch.rs` | Rewrite `call`: html2md conversion, preapproved check, OpenRouter secondary model call |
| `cli/src/tools/web_fetch_preapproved.rs` | Populate with preapproved host list and `is_preapproved_url()` function |

## Preapproved domains

Ported from Claude Code's `WebFetchTool/preapproved.ts`. A hardcoded set of
~130 code-related domains (docs.rs, github.com/anthropics, react.dev, etc.).

Preapproved domain + converted markdown under 100KB → return raw content
directly, skipping the AI call. Everything else goes through the small model.

## Error handling

- Missing API key → `is_error: true`, "no API key configured"
- Secondary model call fails → `is_error: true` with error message
- Abort signal from context → short-circuit and return error

## Out of scope (follow-ups)

- LRU cache for fetched URLs (Claude Code has 15-min TTL, 50MB max)
- Cross-host redirect detection and re-request messaging
- Domain blocklist preflight check (calls api.anthropic.com)
