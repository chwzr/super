# The SUPER daemon

`super daemon run` keeps every SUPER session on a device alive in one
long-running process. Clients (the terminal, the macOS app, `super daemon
status`) connect to it over a local Unix socket, attach to a session, and
speak the RPC protocol from [rpc.md](rpc.md) to it. A session outlives the
client that started it; any client can attach to any session; a session the
daemon unloaded can always be resumed.

On macOS the SUPER app bundles the daemon and registers it with launchd. On
Linux `super install` (coming) registers a systemd user unit. Running `super`
in a terminal without a daemon works as it always did.

## Commands

| Command | What it does |
| --- | --- |
| `super daemon run [--socket PATH]` | Run in the foreground until `shutdown`, SIGTERM or SIGINT. Exit 0 on a deliberate stop, 1 on any error. Refuses to start while another daemon holds `~/.super/run/daemon.lock`. |
| `super daemon status [--socket PATH]` | Print version, pid, uptime, sessions and clients. Exit 1 when no daemon answers, 2 when it refuses the protocol. |
| `super daemon stop [--socket PATH]` | Ask the daemon to shut down, or send SIGTERM to the pid in the lock file when the socket is dead. |

## Files

| Path | Purpose |
| --- | --- |
| `~/.super/run/daemon.sock` | The socket. Directory mode 0700, socket 0600; a peer with another uid is refused. |
| `~/.super/run/daemon.lock` | Exclusive lock with the daemon's pid inside. |
| `~/.super/agent/logs/daemon.log` | Log, rotated at 5 MiB keeping `daemon.log.1`. `SUPER_LOG=daemon=debug` adds detail. Never contains tokens, prompts or record bodies. |
| `~/.super/agent/sessions/` | The session files, shared with the terminal. |
| `~/.super/agent/settings.json` | `{"daemon": {"idleUnloadMinutes": 60}}`: minutes an idle, unattached session stays loaded; `0` keeps sessions loaded until stopped. |

The daemon is started by launchd or systemd with a bare environment, so it
reads the login shell's PATH once at start and uses it for every session's
tools.

## The connection

One JSON object per line, `\n` separated, exactly as in [rpc.md](rpc.md).
The first line must be `hello`; anything else is answered with an error and
the connection closes.

```json
{"id":"1","type":"hello","client":"tui","name":"Terminal","version":"0.0.38","protocolVersions":[1,1]}
{"type":"response","id":"1","command":"hello","success":true,"data":{"protocolVersion":1,"daemonVersion":"0.0.41","hostId":"01J…","hostName":"mbp","platform":"macos","pid":4242,"startedAt":"2026-10-29T09:00:00Z"}}
```

`client` is `tui`, `cli`, `macos`, `ios` or `other`. `name` is what other
clients see as the origin of your commands and approvals (`by: "Terminal"`).
`protocolVersions` is `[min, max]`; the daemon answers with the highest
version both sides speak. With no overlap the response fails with
`"this SUPER daemon speaks protocol 2 to 3; this client speaks 1"` and
`data.protocolVersions` naming the daemon's range, then the connection closes.
Additive changes (new commands, optional fields, events) never bump the
version; a breaking change bumps it and the daemon keeps the previous version
for one release.

A connection is attached to at most one session. After `attach`, session
commands (`prompt`, `get_state`, `get_entries`, `answer_approval`, ...) go to
that session unchanged and its events come back unchanged. A session command
on an unattached connection fails with `not attached to a session`.
Daemon-level commands work at any time; daemon-level events reach every
connection.

## Daemon-level commands

| Command | Payload | Response `data` |
| --- | --- | --- |
| `status` | | `daemonVersion`, `protocolVersions`, `hostId`, `hostName`, `platform`, `pid`, `startedAt`, `uptimeSeconds`, `hostedSessions`, `storedSessions`, `clients`, `socketPath`, `sessionDir` |
| `list_sessions` | `cwd?` | `sessions`: hosted and stored sessions, newest first |
| `list_workspaces` | | `workspaces`: `{path, name, sessionCount, latestState, latestModifiedAt}` derived from the sessions' directories |
| `list_directories` | `path` | `path`, `directories`: `{name, path}` for the subdirectories, hidden ones skipped |
| `create_session` | `cwd`, `model?`, `provider?`, `thinkingLevel?`, `name?` | `session` |
| `resume_session` | `sessionId` | `session`; loads a stored session |
| `attach` | `sessionId` | `session`, `previousSessionId`; loads the session when stored, switches when already attached |
| `detach` | | `sessionId` |
| `stop_session` | `sessionId` | `sessionId`; aborts the turn and unloads, the file stays |
| `shutdown` | | the daemon exits 0 |

A session:

```json
{"sessionId":"a1b2c3d4","cwd":"/Users/felix/dev/super","workspace":"/Users/felix/dev/super",
 "sessionFile":"/Users/felix/.super/agent/sessions/--Users-felix-dev-super--/2026…_a1b2c3d4.jsonl",
 "name":null,"firstMessage":"Fix the login bug","state":"working","blockedReason":null,
 "hosted":true,"attachedClients":2,"model":"super/gpt-5","thinkingLevel":"medium",
 "modifiedAt":"2026-10-29T11:58:03Z"}
```

`state` is `idle`, `working` or `blocked` (an approval is waiting;
`blockedReason` is its title) for a hosted session and `ended` for a stored
one. Ended means "not loaded", never "gone". `model` and `thinkingLevel` are
null for a stored session.

## Daemon-level events

| Event | Payload | When |
| --- | --- | --- |
| `session_created` | `session` | after `create_session` |
| `session_updated` | `session` | state, name, model, thinking level or attached-client count changed |
| `session_ended` | `sessionId`, `reason` (`stopped`, `idle`, `shutdown`) | the session was unloaded; attached connections become unattached |
| `daemon_shutdown` | `reason` | the daemon is about to exit |

## Lifecycle

- A hosted session with no attached client, in state `idle`, with no command
  or event for `idleUnloadMinutes` is unloaded and reported as `ended`.
  `attach` or `resume_session` loads it again with its full history. A
  `working` or `blocked` session is never unloaded.
- While hosting a session the daemon holds `<file>.jsonl.lock` beside it and
  refuses a file another process holds (`session a1b2c3d4 is open in another
  process`).
- `shutdown`, SIGTERM and SIGINT abort in-flight turns, unload every session
  and exit 0. After a restart nothing is hosted; sessions load lazily.
- Workflow approvals are asked as `approval_request` events and answered with
  `answer_approval` from any attached client; `approval_resolved.by` names the
  client's `hello.name`.
