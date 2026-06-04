# WebSearch: Replace DuckDuckGo with OpenRouter Server Tool

## Summary

Replace the DuckDuckGo Instant Answer API fallback in `WebSearch` with a
delegation to OpenRouter's `openrouter:web_search` server-side tool, matching
Claude Code's approach of letting the API layer execute the search.

## Motivation

- **Current state**: WebSearch calls DuckDuckGo's Instant Answer API, which
  returns limited abstract/related-topic snippets — not real web search results.
- **Target state**: WebSearch delegates to OpenRouter's server-side web search
  (`openrouter:web_search`), which uses native provider search (Anthropic,
  OpenAI, xAI) or Exa/Parallel as fallback. The model refines queries and
  synthesizes results. Claude Code uses the same delegation pattern (via
  Anthropic's `web_search_20250305`).

## Design

### Overview

`WebSearch.call()` becomes a thin proxy. It makes a secondary, non-streaming
model call to `/v1/messages` with `{ "type": "openrouter:web_search" }` in the
tools array. OpenRouter intercepts the tool call, executes the search
server-side, and returns results. The WebSearch tool parses the model's
response and returns structured results.

```
WebSearch.call(input)
  ├─ Build prompt: "Search the web for: {query}"
  ├─ Build request body (Anthropic format via build_request_body)
  │    ├─ model: haiku (via resolve_slug)
  │    ├─ tools: [ { type: "openrouter:web_search", parameters: {...} } ]
  │    └─ stream: false
  ├─ POST to {base_url}/v1/messages
  ├─ Parse response text + url_citation annotations
  └─ Return { query, results: [{title, url, snippet}], durationMs }
```

### Changes

#### `cli/src/tools/web_search.rs` — rewrite `call()`

- Remove: `url_encode`, DuckDuckGo HTTP call, `AbstractText`/`RelatedTopics` parsing.
- Add: secondary model call using the same `/v1/messages` endpoint pattern
  already used in `web_fetch.rs` (`apply_prompt_to_content`).
- The server tool entry in the tools array uses `"type":
  "openrouter:web_search"` with optional `parameters` containing
  `allowed_domains` (from input) and `excluded_domains` (mapped from
  `blocked_domains`).
- Domain filtering: `allowed_domains` maps directly; `blocked_domains` maps to
  `excluded_domains` (OpenRouter's parameter name).
- Parse the response: extract text from assistant content blocks. If the
  response contains `url_citation` annotations (OpenRouter attaches these to
  assistant messages when web search results are used), extract `title` and
  `url` from each annotation as structured result entries. If no annotations
  are present, return the raw text as a single result. In either case, include
  `query` and `durationMs` in the output.

#### `cli/src/conversation/anthropic.rs` — support server tool entries

The current `build_request_body` only handles user-defined tools (with `name`,
`description`, `input_schema`). Add a separate function
`build_web_search_request_body` (or extend `build_request_body` with an
`extra_tools: Vec<Value>` parameter) that appends server tool entries using the
`type` field: `{ "type": "openrouter:web_search", "parameters": {...} }`.
`WebSearch.call()` calls this to build its secondary request body. The main
conversation loop's call to `build_request_body` is unaffected.

### What stays the same

- Tool name: `WebSearch`
- Prompt: `prompts/web_search.txt` — already matches Claude Code
- Input schema: `query` (required), `allowed_domains`, `blocked_domains`
- Output schema: `query`, `results [{title, url, snippet}]`, `durationMs`
- Domain filtering behavior (parameter names differ: `excluded_domains` vs
  `blocked_domains` but semantics are the same)

### What's removed

- `url_encode` helper
- Direct HTTP call to `api.duckduckgo.com`
- Parsing of DuckDuckGo JSON response fields (`AbstractText`, `AbstractURL`,
  `RelatedTopics`)

## Dependencies

- `reqwest` — already in `Cargo.toml`
- `serde_json` — already in `Cargo.toml`
- `crate::providers::resolve_slug` — already used by `web_fetch.rs`
- `crate::conversation::anthropic::build_request_body` — needs minor extension

## Risks

- **OpenRouter beta feature**: `openrouter:web_search` is documented as beta.
  API may change. Mitigation: the tool surface is small; adapting to API changes
  is low-effort.
- **Model must support tool calling**: Haiku 4.5+ supports tool calling. If the
  resolved Haiku model lacks tool support, OpenRouter returns an error, which
  surfaces to the user.
- **Cost**: `web_search` has per-request pricing ($0.005–$0.01 depending on
  engine). Acceptable for a feature gated behind explicit model invocation.

## Testing

- Unit: verify input validation rejects empty queries
- Unit: verify response parsing handles text-only, annotations-only, and mixed
  responses
- Integration: smoke test with a real query through the tool loop
