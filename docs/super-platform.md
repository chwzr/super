# SUPER platform provider

The `super` provider connects SUPER to the SUPER platform at
`https://super.flxkpe.io`: one sign-in gives the terminal agent an inference
gateway and the platform's model list. This page describes what the CLI does;
the platform's HTTP API is documented in the platform repository.

## Signed out

Without a `super` entry in `~/.super/agent/auth.json`, SUPER makes no request
to the platform and lists no `super` models. Every other provider, flag, and
setting works as before.

## Signing in

`/login` lists `♦ SUPER` first, with the platform host as its subtitle.
`super login super` does the same from the command line, and `--device-auth`
is implied: the flow is always the OAuth 2.0 device authorization grant
(RFC 8628).

1. SUPER asks the platform for a device code and shows the verification URL
   and the code as `XXXX-XXXX`. It opens the browser when one is available.
2. You approve the device on the platform page. Esc (or Ctrl-C on the command
   line) cancels while SUPER waits. `slow_down` and `expired_token` answers
   are handled as the RFC describes.
3. SUPER looks up the account, stores the credential, loads the model list,
   and prints `Signed in to SUPER as you@example.com · 12 models`.

The credential is an ordinary OAuth entry under the provider ID `super`:
`access`, `refresh`, `expires`, `accountId` (the platform user ID), and
`deviceId` (the device the platform registered). Access tokens last one hour
and are refreshed one minute early with the rotating refresh token, exactly
like the other OAuth providers. When a refresh is rejected, SUPER keeps
running with its other providers and shows one notice:
`SUPER sign-in expired. /login to restore remote control.`

The platform rotates refresh tokens and treats a second refresh of the same
token as theft: it signs the whole device out. Several `super` processes
share `~/.super/agent/auth.json` (a terminal session and the headless
`super` the macOS app runs, for example), so a refresh holds the credential
lock beside the file (`.auth.json.lock`) from the moment it finds the stored
token stale until the replacement is written; a process that was waiting
finds the new token and sends no request. A refresh the platform rejected
is not sent again: the process remembers the rejected token and answers
with the notice above until `/login` stores a different one. A model request
without a usable credential (print mode included) says
`not signed in to SUPER, or the sign-in expired: /login, or `super login
super` on the command line`.

## Models

`GET /api/models` returns the catalog in SUPER's own `Model` shape. SUPER
writes it to `~/.super/agent/models.json` as the `super` provider, replacing
the previous `super` entries and leaving other providers untouched, so the
models appear in `/model` as `super/<id>` with the display name `SUPER`.

The list is fetched on sign-in and once per start while signed in. In the
interactive mode the refresh starts after the first frame and never blocks
input; `--mode rpc` and print mode refresh in the background as well. When
the platform cannot be reached, the saved list is used. A `401` from the
platform triggers one token refresh and one retry before the notice above.

Inference uses the OpenAI-compatible gateway (`/v1/chat/completions`) through
the same chat-completions adapter as any other `openai-completions` provider,
with `Authorization: Bearer <access token>`. Streaming, tool calls, and image
input work as for other OpenAI-compatible providers. Reasoning models send
`reasoning_effort`: SUPER's thinking levels `minimal`, `low`, `medium`, and
`high` map to the same names; `xhigh` and `max` are sent as `high` unless the
platform's model entry carries a `thinkingLevelMap`. Thinking `off` sends
nothing unless the entry maps it; the platform maps `off` to `none` for models
whose upstream (OpenRouter) otherwise keeps reasoning on, so `off` is off.

The session title, the idle recap, and `/btw` are short helper requests with
a tight output budget. When a model spends that budget on reasoning it would
not switch off and returns no text, SUPER asks once more with a 4096-token
budget; if the answer is still empty, the notice says so
(`the model spent its 4096-token budget on reasoning …`).

## Signing out

`/logout super` and `super logout super` revoke the refresh token on the
platform (`POST /oauth/revoke`), remove the credential, and remove the `super`
models from the catalog. The running session continues locally. When the
platform cannot be reached, the local credential and models are removed
anyway and SUPER says that the device was not revoked.

## Platform URL

| Source | Example |
| ------ | ------- |
| Default | `https://super.flxkpe.io` |
| Setting `platform.url` in `~/.super/agent/settings.json` | `{"platform": {"url": "http://localhost:8080"}}` |
| Environment variable `SUPER_PLATFORM_URL` | `SUPER_PLATFORM_URL=http://localhost:8080 super` |

The variable wins over the setting. A trailing slash is ignored. Unknown keys
under `platform` are preserved when settings are saved.

## For other code in the workspace

`super_ai::auth::super_platform` exposes what remote control and other
features need without touching the auth file format:

- `stored_credential()` — the current `super` `OAuthCredential`, if any.
- `stored_device_id()` — the platform device ID of this sign-in.
- `is_signed_in()` — whether a credential is stored; no network access.
- `base_url()` — the platform base URL after settings and environment.
- `OAuthConfig::default()` — a request configuration for that URL.
- `SIGN_IN_EXPIRED` — the notice text for an unrecoverable refresh.

`super_remote::credentials` holds what every signed-in process shares:
`PlatformCredentials` (the `RemoteCredentials` backed by the stored sign-in),
`is_signed_in()`, `resolve_platform_url(setting)` and
`configure_platform_url(setting)`, used by the CLI and by the daemon.

`crates/super/src/platform.rs` holds the CLI and TUI glue:
`signed_in_message(email, parts)` builds the sign-in line from parts, so
another feature can append its own clause.
