# SUPER daemon (`super daemon`): design

Date: 2026-10-29. Linear: ENG-5 (this design), ENG-6 (macOS spike, decided
design), ENG-7 (macOS app side), ENG-13/14/15 (TUI as client), ENG-8/9/11
(app features that need the daemon).

## Goal

One long-running process per user, `super daemon run`, owns every SUPER
session on a device. The terminal `super`, the macOS app and (through the
platform) the iOS app are clients of it. A session started from any of them is
visible and controllable from all of them, survives the client that started
it, and can always be resumed.

The daemon is the engine the CLI already contains, hosted in a long-lived
process: same binary, same session files, same RPC protocol. A `super` with
no daemon keeps running in-process exactly as today.

## Decisions this design rests on

From ENG-6 (Felix, 2026-10-29): the macOS app bundles `super` at
`Contents/Helpers/super` and registers it as a per-user launchd agent; the
terminal CLI stays the user's own install and talks to the daemon over a
socket with a negotiated protocol version; the macOS daemon ships only with
the app, Linux gets `super install` with a systemd user unit; logged-in only,
no root daemon; arm64 only; credentials in `~/.super/agent/auth.json` shared
by CLI and daemon; app updates through Sparkle (ENG-35).

From ENG-5: same binary, two modes; the daemon owns every session; one
protocol over a pluggable carrier (local Unix socket, or S2 streams through
the platform); multi-client fan-out; on-demand start from a terminal.

Decided in this design dialogue (Felix, 2026-10-29):

1. Four slices, daemon core first.
2. A new library crate `crates/super-daemon`; wire types in `super-sdk`;
   CLI wiring in the `super` binary.
3. One attached session per socket connection; daemon-level commands on any
   connection; daemon-level events to every connection; `hello` first.
4. A hosted session nobody is attached to is unloaded after an idle timeout
   (default one hour); resume is lazy, also after a daemon restart.
5. Workspaces are derived from sessions (the distinct working directories),
   not kept as a list.
6. The handshake negotiates a protocol version range; additive changes never
   bump it, breaking changes bump it and the daemon keeps the previous version
   for one release.
7. The daemon writes its own rotated log at `~/.super/agent/logs/daemon.log`
   on every platform, and the same lines to stderr.

## Slices

Each slice is a sub-issue of ENG-5 and one PR in chwzr/super.

| Slice | Content | Done when |
| --- | --- | --- |
| 1. Daemon core | `super daemon run\|status\|stop`, lock, log, Unix socket, hello handshake, session host with lazy load and idle unload, daemon-level commands and events, `docs/daemon.md` | An end-to-end test drives two clients attached to one session over a real Unix socket with the mock provider; `super daemon run` exits 0 on SIGTERM; the ENG-7 prototype has a socket to connect to |
| 2. Platform presence | One host record and heartbeat per daemon, a registry record per session (hosted and stored), S2 mirroring per hosted session, a host-level command stream so an app can start or resume a session in a directory; workspace records derived from sessions | A signed-in daemon shows its host online in the apps with every session and its state; "new session in directory" from the app works; needs a contract change in super-platform (new stream `hosts/{hostId}/commands` and its forward-auth rule) |
| 3. Terminal client and on-demand start | `DaemonClient` in `super-daemon`: socket discovery, hello with version negotiation, the actionable skew error naming the update command for how the CLI was installed, start on demand (`launchctl kickstart` on macOS when the agent is registered, `systemctl --user start` on Linux); `super` attaching to the daemon when the ecosystem is enabled | `super` in a terminal, signed in with a daemon registered, attaches instead of running in-process; without a daemon it runs in-process as today. The TUI rendering over the client is ENG-13 |
| 4. Linux service | `super install` writes and starts a systemd user unit pointing at the running binary, `super uninstall` removes it; `super daemon status` reports the unit | A fresh Linux user runs `super install`, logs out and in, and the daemon is up |

The rest of this document specifies slice 1 and the parts of the protocol
later slices extend.

## Architecture

```
crates/super-sdk/src/daemon.rs      wire types: Hello, HelloReply, DaemonCommand,
                                    DaemonEvent, SessionSummary, SessionState,
                                    PROTOCOL_VERSION; pure serde, compiles on wasm
crates/super-daemon/                library crate
  src/lib.rs                        DaemonConfig, run(config) -> exit code
  src/paths.rs                      ~/.super/run, socket, lock, log, session dir
  src/lock.rs                       exclusive lock file with the pid inside
  src/log.rs                        rotated file logger plus stderr
  src/env.rs                        login-shell PATH resolution
  src/host.rs                       SessionHost: hosted sessions, listing, lazy
                                    load, idle unload, state tracking, events
  src/server.rs                     Unix listener, peer check, one task per
                                    connection, dispatch
  src/client.rs                     DaemonClient (connect, hello, call); used by
                                    `status` and `stop`, grown in slice 3
  tests/socket_end_to_end.rs        real socket, mock provider
crates/super/src/args.rs            `Command::Daemon { Run, Status, Stop }`
crates/super/src/daemon_cli.rs      argument handling, output formatting
docs/daemon.md                      the protocol as seen by a client
```

`super-daemon` depends on `super-sdk` (feature `rpc` is not needed: the
daemon has its own dispatcher), `super-coding` (session listing, settings),
`tokio`, `fs2`, `serde_json`, `dirs`, `chrono`. Slice 2 adds `super-remote`.
`super_remote::SessionState` moves to `super_sdk::daemon::SessionState` and
`super-remote` re-exports it, so the registry and the daemon share one type.

Every hosted session is an `Arc<super_sdk::Session>` built from
`SessionOptions` the way `--mode rpc` builds one. The agent loop, tools,
providers and session files need no change.

Windows: the crate compiles (CI runs clippy and tests there) but
`super daemon run` returns "the SUPER daemon needs a Unix socket and is not
available on Windows" and exits 1; the socket code is `#[cfg(unix)]`.

## Process model

### `super daemon run`

1. Load settings (`~/.super/agent/settings.json`), resolve `DaemonConfig`:
   socket `~/.super/run/daemon.sock` (`--socket PATH` overrides it), lock
   `~/.super/run/daemon.lock`, log `~/.super/agent/logs/daemon.log`, session
   dir as `setup::session_dir` resolves it, idle unload from
   `daemon.idleUnloadMinutes` (default 60).
2. Create `~/.super/run` with mode 0700.
3. Take the exclusive lock (`fs2`) and write the pid into it. If held: print
   "another SUPER daemon is running (pid N)" and exit 1.
4. Resolve the login shell's PATH once (`$SHELL -lc 'echo $PATH'` with a
   timeout, falling back to the current PATH) and set it in the process
   environment, so sessions started by a launchd-launched daemon find `git`,
   `node` and the user's tools. The app's `childPath()` does this today.
5. Remove a stale socket file, bind the listener, chmod the socket to 0600.
6. Log "daemon <version> listening on <socket> (pid N)" and serve.
7. Shutdown on SIGTERM, SIGINT or the `shutdown` command: stop accepting,
   send `daemon_shutdown` to every connection, abort every in-flight turn,
   drop the hosted sessions (their files are complete at every entry
   boundary), close the connections, remove the socket file, release the
   lock, exit 0. Bounded by five seconds.

Exit codes: 0 only on a deliberate shutdown; 1 for any error (lock held,
cannot bind, settings unreadable). launchd restarts on non-zero exits
(`KeepAlive {SuccessfulExit: false}`), so "deliberately stopped" must be 0.

The process never daemonises or forks: launchd and systemd supervise the pid
they started.

### `super daemon status`

Connects to the socket, sends `hello` as client `cli`, prints one block:

```
SUPER daemon 0.0.41 running (pid 4242, up 3h 12m)
socket   ~/.super/run/daemon.sock
sessions 2 hosted, 14 on disk
clients  3
```

Exit 0 when a daemon answers, 1 when the socket is missing or refused
("no SUPER daemon is running"), 2 when a daemon answers but refuses the
protocol (the error names both ranges).

### `super daemon stop`

Sends `shutdown` over the socket and waits until the connection closes
(bounded). When the socket is dead but the lock file holds a live pid, sends
SIGTERM to it. Prints "stopped" or "no SUPER daemon is running". Exit 0 in
both cases; 1 on an error.

On macOS the app registers the daemon with launchd; `super daemon stop`
stops it until the next login or `launchctl kickstart`. ENG-7 handles
disable and re-enable through `SMAppService`.

### Socket security

The socket lives in a 0700 directory with mode 0600. On every accepted
connection the daemon reads the peer credentials
(`tokio::net::UnixStream::peer_cred`) and closes the connection when the uid
differs from its own. No token: filesystem permissions and the uid check are
the authentication, as for any per-user socket.

### Logging

`super-daemon/src/log.rs`: a process-wide logger writing
`2026-10-29T12:00:00.123Z INFO server: client 7 attached a1b2c3d4` lines to
`~/.super/agent/logs/daemon.log` and to stderr. Levels `error`, `warn`,
`info`, `debug`; default `info`; `SUPER_LOG=daemon=debug` (the same variable
`super-remote` reads) enables debug. The file rotates at 5 MiB, keeping one
previous file (`daemon.log.1`). Never logs tokens, prompts or record bodies;
command types and session ids only.

## Protocol

Everything on a connection is one JSON object per line, `\n` separated, as
in `docs/rpc.md`. The session RPC (`protocol.rs`) is unchanged. The daemon
adds a handshake, daemon-level commands and daemon-level events, defined in
`super_sdk::daemon` and documented in `docs/daemon.md`.

### Handshake

The first line a client sends must be `hello`. Any other first line gets
`{"type":"response","command":"hello","success":false,"error":"say hello
first"}` and the connection closes.

```json
{"id":"1","type":"hello","client":"tui","name":"Terminal","version":"0.0.38","protocolVersions":[1,1]}
{"type":"response","id":"1","command":"hello","success":true,
 "data":{"protocolVersion":1,"daemonVersion":"0.0.41","hostId":"01J…","hostName":"mbp",
         "platform":"macos","pid":4242,"startedAt":"2026-10-29T09:00:00Z"}}
```

- `client`: `tui`, `cli`, `macos`, `ios` or `other`.
- `name`: shown to other clients as the origin of commands and approvals
  (`by: "SUPER.app"`). Defaults to the client kind.
- `protocolVersions`: `[min, max]` the client speaks. The daemon advertises
  its own range and picks the highest version in both. With no overlap it
  answers `success: false` with `error` "this SUPER daemon speaks protocol
  2 to 3; this client speaks 1" and `data: {daemonVersion, protocolVersions:
  [2,3]}`, then closes. The client turns that into the right update hint
  (slice 3).
- Slice 1 ships `PROTOCOL_VERSION = 1` and the range `[1, 1]` on both sides.

Version rule: new commands, new optional fields and new events are additive
and do not bump the version (an unknown event is ignored; an unknown command
gets the usual error naming it). Renaming or removing a command, or changing
what one means, bumps it, and the daemon keeps serving the previous version
for one release.

### Connection model

A connection is attached to at most one session. Session commands
(`prompt`, `get_state`, `get_entries`, `answer_approval`, ...) go to the
attached session unchanged and their responses and events come back
unchanged, so after `attach` the stream is byte for byte the `--mode rpc`
stream. A session command on an unattached connection fails with "not
attached to a session". Daemon-level commands work on any connection at any
time. Daemon-level events go to every connection.

Each session command runs in its own task, as in `rpc.rs`, so `abort`
arrives while a `prompt` streams. One writer task per connection keeps lines
whole. Events are delivered through the session's existing broadcast
channel; a slow client gets `event_lag` and re-reads state, as today.

### Daemon-level commands

| Command | Payload | Response data |
| --- | --- | --- |
| `hello` | see above | see above |
| `status` | | `daemonVersion`, `protocolVersions`, `hostId`, `hostName`, `platform`, `pid`, `startedAt`, `uptimeSeconds`, `hostedSessions`, `storedSessions`, `clients`, `socketPath`, `sessionDir` |
| `list_sessions` | `cwd?` | `sessions: [SessionSummary]`, hosted and stored, newest first |
| `list_workspaces` | | `workspaces: [{path, name, sessionCount, latestState, latestModifiedAt}]`, derived from the sessions' working directories |
| `list_directories` | `path` | `path`, `directories: [{name, path}]` (subdirectories, hidden ones skipped, sorted; error when `path` is not a directory) |
| `create_session` | `cwd`, `model?`, `provider?`, `thinkingLevel?`, `name?` | `session: SessionSummary` |
| `resume_session` | `sessionId` | `session: SessionSummary`; loads a stored session, no-op when hosted |
| `attach` | `sessionId` | `session: SessionSummary`, `previousSessionId?`; loads the session when stored; switches when already attached elsewhere |
| `detach` | | `sessionId?` |
| `stop_session` | `sessionId` | `sessionId`; aborts the turn, unloads, file stays |
| `shutdown` | | none; the daemon exits 0 |

`SessionSummary`:

```json
{"sessionId":"a1b2c3d4","cwd":"/Users/felix/dev/super","workspace":"/Users/felix/dev/super",
 "sessionFile":"/Users/felix/.super/agent/sessions/--Users-felix-dev-super--/2026…_a1b2c3d4.jsonl",
 "name":null,"firstMessage":"Fix the login bug","state":"working","blockedReason":null,
 "hosted":true,"attachedClients":2,"model":"super/gpt-5","thinkingLevel":"medium",
 "modifiedAt":"2026-10-29T11:58:03Z"}
```

`model` and `thinkingLevel` are null for a stored session (the cheap listing
does not read them). `workspace` equals `cwd` until workspaces mean more.

`create_session` requires an absolute path to an existing directory; the
daemon does not create directories. Model and thinking level come from the
request, else from the settings' defaults, else the first model with a
credential, as `SessionOptions::build` resolves them. Project trust follows
the stored decision and `defaultProjectTrust`, as `--mode rpc` does; the
interactive trust question comes with ENG-13. Workflows ask for approval
through `approval_request` (the gate `--mode rpc` installs) instead of
auto-approving.

### Daemon-level events

| Event | Payload | When |
| --- | --- | --- |
| `session_created` | `session` | after `create_session` |
| `session_updated` | `session` | state, name, model, thinking level or attached-client count changed |
| `session_ended` | `sessionId`, `reason` (`stopped`, `idle`, `shutdown`) | the session was unloaded |
| `daemon_shutdown` | `reason` | before the daemon exits |

Attached connections receive `session_ended` for their session and become
unattached.

### Session command errors

Unknown command types, malformed JSON and commands before `hello` are
answered, never silently dropped, in the `response` shape `rpc.md`
describes. `attach` and `resume_session` fail with "no session a1b2c3d4" for
an unknown id and with "session a1b2c3d4 is open in another process" when
its lock is held.

## Session host

### Identity and states

A session is its JSONL file under the session directory
(`<dir>/<cwd slug>/<stamp>_<uuid>.jsonl`), addressed by the id in its header.
Sessions written by a `super` running without the daemon are the same files
and are listed as stored.

A session is hosted (an `Arc<Session>` in memory) or stored (file only).
Clients see `idle`, `working` and `blocked` for a hosted session and `ended`
for a stored one. `ended` means "not loaded", never "gone".

State tracking per hosted session, from its event stream: `agent_start` and
`turn_start` set `working`; `agent_settled` sets `idle` unless an approval
is open; `approval_request` sets `blocked` with the request title as
`blockedReason`; `approval_resolved` sets `working` while a run is in flight,
else `idle`. One function with unit tests; the same mapping feeds the
registry in slice 2.

### Loading and unloading

- `create_session` builds a hosted session with `SessionSource::Create`.
- `attach` and `resume_session` on a stored session find its file
  (`SessionManager::find_by_id`) and build with `SessionSource::Open`.
- While a session is hosted, the daemon holds an exclusive `fs2` lock on
  `<file>.jsonl.lock` beside it (the same shape as the existing
  `.jsonl.remote.lock`). A file whose lock another process holds is refused
  with an error naming the session. Slice 3 makes a standalone `super` take
  the same lock, so the two never write one file.
- `stop_session` aborts the running turn, waits for idle (bounded), drops
  the session, emits `session_ended {reason: "stopped"}`.
- An idle sweep every minute unloads hosted sessions with no attached
  client, state `idle`, and no command or event for `idleUnloadMinutes`
  (default 60, `0` disables). Unloading is safe: model, thinking level, name
  and compaction are entries in the file. A `blocked` or `working` session is
  never unloaded. Emits `session_ended {reason: "idle"}`.
- After a daemon restart nothing is hosted; sessions load lazily on the
  first `attach`, `resume_session` or (through slice 2) platform command. A
  turn interrupted by the restart is interrupted; the transcript up to that
  point is intact.

### Listing

`list_sessions` merges the hosted sessions with `SessionManager::list_all`
(or `list` for a `cwd`), hosted ones winning, sorted by modification time.
Slice 1 rescans the session directory on each call; a cached index is a
later optimisation if the scan shows up in the app's sidebar refresh.

`list_workspaces` groups that listing by `cwd`. The name is the last path
component. There is no workspace file and nothing to add or remove: ENG-11's
"new workspace" picks a directory with `list_directories` and calls
`create_session` in it.

### Origins and approvals

Every session command runs through `Session::execute_from(command, id,
Some(origin))` with the connection's `hello.name`, so `approval_resolved.by`
and the TUI's attribution name the client. The daemon installs no observer;
`SessionNotice` stays a terminal concern.

## What slice 1 leaves out, on purpose

- No S2 mirroring and no registry writes, even when signed in. `super daemon
  run` in slice 1 is local only; slice 2 adds presence.
- No `DaemonClient` beyond what `status` and `stop` need; no on-demand start;
  the TUI does not attach yet (slice 3, ENG-13).
- No `super install` (slice 4) and nothing in super-app (ENG-7).
- No per-session `seq` on events: `event_lag` per subscriber already tells a
  client when to re-read state.
- No tool-call approval gate (the `before_tool_call` idea in
  `ideas/01-daemon.md`); workflow and trust approvals exist today and are
  enough for this slice.

## Testing

- Unit tests in `super-daemon`: lock held and pid readback; log rotation at
  the size cap; hello negotiation (overlap, no overlap, error text); state
  mapping from events; listing merge of hosted over stored; `list_workspaces`
  grouping; `list_directories` filtering and errors; decoding a line into a
  daemon command or a session command.
- `crates/super-daemon/tests/socket_end_to_end.rs`: the daemon on a temp
  socket with a temp session dir and `super_sdk::mock::MockProvider` through
  `models_file`. Two `UnixStream` clients: hello; `create_session`; both
  attach; one prompts; both receive the same `agent_start` to
  `agent_settled` sequence and both see `session_updated` working then idle;
  `detach` on one; `stop_session` sends `session_ended` to the other and a
  later `get_state` fails with "not attached"; a stored session resumes on
  `attach`; an idle sweep with a tiny timeout unloads and `attach` loads
  again; a first line that is not `hello` is refused; a client with
  `protocolVersions [9, 9]` is refused with both ranges in the error;
  `shutdown` sends `daemon_shutdown` and `run` returns 0.
- Signal test: `run` returns 0 after SIGTERM (the test sends the signal to
  itself through `libc::kill(getpid(), SIGTERM)` in a `#[cfg(unix)]` test, or
  drives the same cancellation token the signal handler cancels).
- `crates/super/tests/daemon_cli.rs`: `super daemon status` without a daemon
  exits 1 and prints "no SUPER daemon is running"; `super daemon run --socket
  <tmp>` answers `status` and exits 0 on `super daemon stop --socket <tmp>`
  (binary tests through `CARGO_BIN_EXE_super`).
- CI: `cargo test --workspace` on ubuntu and macos; clippy with
  `-D warnings` including windows, where the unix-only code is cfg-gated.

## Later slices, sketched so slice 1 does not paint them in

- Slice 2: `super-daemon/src/presence.rs` starts one `Remote`, one
  `RegistryWriter` per daemon (the writer grows from one `SessionRecord` to a
  map keyed by session id, with the host record once), one
  `SessionTransport` per hosted session, and tails
  `hosts/{hostId}/commands` for `create_session` and `resume_session` sent by
  controllers; stored sessions are published as `ended` so the apps can
  resume them. super-platform adds the stream and allows controllers to
  append to `hosts/*/commands`.
- Slice 3: `DaemonClient::connect_or_start()`; the skew error reads how the
  CLI was installed (symlink into `SUPER.app`, Homebrew cellar, `install.sh`
  into `~/.local/bin`, cargo) and names the matching update command; `super`
  takes the session lock when running in-process.
- Slice 4: the unit `~/.config/systemd/user/super-daemon.service` with
  `ExecStart=<current_exe> daemon run`, `Restart=on-failure`; `super install`
  runs `systemctl --user daemon-reload`, `enable --now`; `super uninstall`
  reverses it.
