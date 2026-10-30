# The SUPER daemon

`super daemon run` keeps every SUPER session on a device alive in one
long-running process. Clients (the terminal, the macOS app, `super daemon
status`) connect to it over a local Unix socket, attach to a session, and
speak the RPC protocol from [rpc.md](rpc.md) to it. A session outlives the
client that started it; any client can attach to any session; a session the
daemon unloaded can always be resumed.

On macOS the SUPER app bundles the daemon and registers it with launchd. On
Linux `super install` registers a systemd user unit (see
[On Linux](#on-linux-a-systemd-user-service)). Signed in to the platform,
`super` in a terminal attaches to the daemon (see
[The terminal](#the-terminal)); running `super` without a daemon or without
an account works as it always did.

## Commands

| Command | What it does |
| --- | --- |
| `super daemon run [--socket PATH]` | Run in the foreground until `shutdown`, SIGTERM or SIGINT. Exit 0 on a deliberate stop, 1 on any error. Refuses to start while another daemon holds `~/.super/run/daemon.lock`. |
| `super daemon status [--socket PATH]` | Print version, pid, uptime, sessions and the clients connected besides itself. Exit 1 when no daemon answers, 2 when it refuses the protocol. |
| `super daemon stop [--socket PATH]` | Ask the daemon to shut down, or send SIGTERM to the pid in the lock file when the socket is dead or the daemon speaks another protocol version. |
| `super install` | Linux: write the systemd user unit, enable it and start the daemon. |
| `super uninstall` | Linux: stop and disable the unit and remove it. |

Without `--socket` every command uses `SUPER_DAEMON_SOCKET` when it is set,
else `~/.super/run/daemon.sock`; the lock sits beside a socket given either
way. `SUPER_DAEMON_PROTOCOL_VERSIONS=2-3 super daemon run` makes a daemon
advertise another protocol range, to see what clients say to a newer or
older daemon.

## Files

| Path | Purpose |
| --- | --- |
| `~/.super/run/daemon.sock` | The socket. Directory mode 0700, socket 0600; a peer with another uid is refused. |
| `~/.super/run/daemon.lock` | Exclusive lock with the daemon's pid inside. |
| `~/.super/agent/logs/daemon.log` | Log, rotated at 5 MiB keeping `daemon.log.1`. `SUPER_LOG=daemon=debug` adds detail. Never contains tokens, prompts or record bodies. |
| `~/.super/agent/sessions/` | The session files, shared with the terminal. |
| `~/.super/agent/settings.json` | `{"daemon": {"idleUnloadMinutes": 60, "attach": true}}`: minutes an idle, unattached session stays loaded (`0` keeps sessions loaded until stopped); whether `super` in a terminal attaches to the daemon when signed in. |
| `~/.config/systemd/user/super-daemon.service` | Linux: the unit `super install` writes (`$XDG_CONFIG_HOME/systemd/user` when that is set). |

The daemon is started by launchd or systemd with a bare environment, so it
reads the login shell's PATH once at start and uses it for every session's
tools.

## On Linux: a systemd user service

```sh
super install     # write the unit, enable it, start the daemon
super uninstall   # stop and disable the unit, remove it
```

`super install` writes `~/.config/systemd/user/super-daemon.service`:

```ini
# Written by `super install`; `super uninstall` removes it.
[Unit]
Description=SUPER daemon: hosts this user's SUPER sessions
Documentation=https://github.com/chwzr/super/blob/main/docs/daemon.md

[Service]
ExecStart=/home/felix/.local/bin/super daemon run
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

then runs `systemctl --user daemon-reload` and `systemctl --user enable --now
super-daemon`, waits up to ten seconds for the daemon to answer on its
socket, and prints the unit path, the binary it runs and the unit's state.
`ExecStart` is the binary you ran `super install` with, or the `super` on
your PATH when that is a symlink to it (Homebrew), so an upgrade keeps the
unit working. A path with spaces is quoted; systemd cannot run one with a
quote, a backslash or a `$` in it, so `super install` refuses those before
writing anything. Running `super install` again is safe: the same unit is
only enabled and started, a different one (super moved) replaces it and
restarts the daemon.

The daemon now starts with your systemd user manager, which runs from your
first login to your last logout. On an always-on machine (a server, a box you
reach over SSH) keep the manager, and the daemon, running without a login:

```sh
loginctl enable-linger <user>    # may ask for sudo
```

- `super daemon status` ends with the unit's line, for example
  `service  enabled, active (/home/felix/.config/systemd/user/super-daemon.service)`,
  also when no daemon answers.
- `Restart=on-failure` brings the daemon back five seconds after a crash.
  `super daemon stop` and `systemctl --user stop super-daemon` exit 0, which
  systemd takes as a deliberate stop: the daemon stays down until the next
  login or `systemctl --user start super-daemon`.
- After `super update`, `systemctl --user restart super-daemon` runs the new
  version.
- Start failures are in `journalctl --user -u super-daemon`; the daemon's own
  log is `~/.super/agent/logs/daemon.log`.
- A daemon you started by hand keeps the lock: the unit retries every five
  seconds until `super daemon stop` ends the other one.

`super uninstall` runs `systemctl --user disable --now super-daemon`, removes
the unit file, reloads systemd and clears a failed state. With nothing
installed it says so. Without a reachable user manager it removes the unit
file and its `default.target.wants` link and says that nothing was stopped.

Without systemd, or without a user manager for your login (inside many
containers, or after `su`), `super install` writes nothing and says what is
missing; run `super daemon run` under the service manager you use instead.
On macOS `super install` refuses: the daemon ships with the SUPER app, which
registers it with launchd.

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

- `create_session` writes the session file (its header) at once, so a
  session stopped before its first message can be resumed and attached to by
  id. Once unloaded, such an empty session is left out of the listings, as
  the terminal's session picker leaves it out.
- A hosted session with no attached client, in state `idle`, with no command
  or event for `idleUnloadMinutes` is unloaded and reported as `ended`.
  `attach` or `resume_session` loads it again with its full history. A
  `working` or `blocked` session is never unloaded.
- While hosting a session the daemon holds `<file>.jsonl.lock` beside it and
  refuses a file another process holds (`session a1b2c3d4 is open in another
  process`). A `super` running the engine in-process (terminal, `-p`,
  `--mode json`, `--mode rpc`, `super acp`) holds the same lock on the file
  it writes, so the two never append to one file: it refuses to start on a
  file the daemon hosts, and the session picker, `switch_session` and ACP
  `session/load` refuse to open one.
- `shutdown`, SIGTERM and SIGINT abort in-flight turns, unload every session
  and exit 0. After a restart nothing is hosted; sessions load lazily.
- Workflow approvals are asked as `approval_request` events and answered with
  `answer_approval` from any attached client; `approval_resolved.by` names the
  client's `hello.name`.

## The terminal

`super` in a terminal attaches to the daemon instead of running the engine
itself when all of these hold:

- it is signed in to the SUPER platform, `remote.enabled` is not false and
  `daemon.attach` is not false;
- it is a plain interactive run: no `-p`, `--mode`, subcommand, `--no-remote`
  or `--no-session`, and none of the options that shape how the engine is
  built (`--tools`, `--skill`, `--system-prompt`, `--approve`,
  `--session-dir`, `--resume`, `--fork`, ...);
- a daemon answers on the socket, or one is registered to start (below).

It then picks a session in the current directory: `super` joins the newest
session the daemon is running there, else creates one (with `--model`,
`--provider`, `--thinking` and `--name` when given, which always create a
new session); `super -c` attaches to the newest session there, loading a
stored one; `super --session <id or file>` attaches to that session.
Message arguments, `@file` included, become the first prompt.

The front end is line based for now (the full TUI over the connection is
ENG-13): the reply streams as it arrives, every tool call is one line, an
approval is asked as numbered choices, and a line you type is sent as a
prompt, or as a steer while the session works. Ctrl+C aborts the turn, or
detaches when nothing runs; `/detach` and Ctrl+D detach, `/stop` stops the
session, `/sessions` switches to another one (below). Detaching leaves the
session running in the daemon, where the apps see it and `super` attaches
again. With piped input, `super` sends each
line and exits once the session settles after the last one.

When no daemon answers and none can be started, `super` runs in-process as
it always did (`--verbose` says why). When the daemon speaks another
protocol version, `super` stops with the skew message below; `super
--no-remote` runs without the daemon until the update.

### Starting on demand

When the default socket is dead, the client asks the service manager that
owns the daemon to start it, so the daemon keeps one supervisor however it
was started:

- macOS: `launchctl kickstart gui/<uid>/io.flxkpe.super.daemon` when
  `launchctl print gui/<uid>/io.flxkpe.super.daemon` finds the agent the
  SUPER app registers;
- Linux: `systemctl --user start super-daemon.service` when
  `systemctl --user cat super-daemon.service` finds the unit `super install`
  writes.

It then waits up to five seconds for the socket. Nothing registered means
no daemon, and nothing is started. A socket named by `SUPER_DAEMON_SOCKET`
other than the default is connected to, never started.

### Version skew

A daemon and a client with no protocol version in common cannot talk. The
client names both ranges and the update that fixes it, read from how this
`super` was installed:

```
the SUPER daemon speaks protocol 2 to 3, this build speaks 1: run `brew upgrade chwzr/tap/super`
```

| This `super` is | The hint |
| --- | --- |
| a symlink into (or inside) `SUPER.app/Contents/Helpers` | `managed by the SUPER app: update the app` |
| in a Homebrew cellar | ``run `brew upgrade chwzr/tap/super` `` |
| in `~/.local/bin` (or `SUPER_INSTALL_DIR`) from `install.sh`, or anywhere else | ``run `super update` `` |
| in `~/.cargo/bin` | ``run `cargo install --git https://github.com/chwzr/super super` `` |
| a build in a checkout's `target/` | `rebuild it from an up-to-date checkout` |

When the daemon is the older side the hint is about the daemon instead:
`update the SUPER app, which runs the daemon` on macOS, a restart on the
current build elsewhere. `super daemon status` prints the same message and
exits 2. `super update` refuses to replace a binary inside the app bundle
and says the app manages it.

### Moving a running session to the daemon: `/remote`

A `super` that runs its session in-process (no daemon when it started,
signed out, `--no-remote`, `daemon.attach: false`) hands that session to the
daemon with `/remote`, alias `/rc`. The apps then see it, and it keeps
running after the terminal closes. In order:

1. Signed out, it signs in to the platform first, with the device code flow
   `/login super` uses.
2. It connects to the daemon, or makes one available:
   - a daemon answering on the socket is used as it is;
   - a daemon registered with launchd or systemd is started there, as above;
   - on Linux with nothing registered it does what `super install` does and
     says so; without a systemd user manager it starts `super daemon run` in
     the background instead;
   - on macOS without the SUPER app's agent it explains that the app
     provides the daemon and stops;
   - a socket named by `SUPER_DAEMON_SOCKET` is connected to, never started.
3. It waits until the turn, queued messages, background requests that write
   the session and open questions are done: a session never moves mid-turn.
   `/remote cancel` keeps the session where it is.
4. The terminal stops mirroring the session itself, lets go of the session
   file's lock, the daemon loads the session from its file
   (`resume_session`), and the terminal attaches to it. A new session with
   nothing written yet has no file; the daemon starts one in the same
   directory with the same model instead.

On screen that is one line, `♦ session a1b2c3d4 now runs in the SUPER daemon
(pid 4242) and shows up in the apps; this terminal is attached to it`, and the
terminal carries on attached to the session, with the transcript where it
was. The apps list the session a few seconds later.

A failure at any step (the sign-in, no daemon, a session file outside the
daemon's session directory, the daemon refusing to load it) is reported and
leaves the session running in the terminal as before, with its lock taken
back and its mirror restarted. Should another process hold the file by then
(a daemon that loaded it after the terminal stopped waiting for it), the
terminal exits instead of writing beside it, and `super` attaches to the
session where it runs. `--no-session` sessions are refused (there is
no file to resume), and so is `remote.enabled: false` (the apps would not see
the session). In a terminal attached already, `/remote` reports the session
and the daemon it is attached to. `/remote status`, `on`, `off` and `name`
still control the terminal's own mirror of an in-process session.

### Switching sessions: `/sessions`

`/sessions` lists every session the terminal can reach, grouped by device,
then by workspace, with the apps' status model:

```
Sessions
● mbp  this device
  super  /Users/felix/dev/super
→   ◐ working         Fix the login bug  current · 2m
    ○ idle            Add tests for the parser  1h
    + new session in super
  + new workspace on mbp…
● linuxbox  online
  api  /home/felix/api
    ? needs an answer Migrate the schema  Workflow approval required · 5m
↑/↓ move · enter open · type to filter · esc close
```

- This device comes first, its sessions from the daemon over the socket
  (`list_sessions`). Signed in, other devices follow from the platform's
  `registry` stream, online ones first: a device is online when anything
  about it is at most 90 seconds old, as in the apps. A session is
  `working`, `needs an answer` (blocked), `idle` or `ended` (not loaded);
  on a device that is offline, a session that has not ended reads
  `offline`, since its last state may be stale.
- Typing filters by title, workspace and device; Enter opens the row under
  the cursor; Esc clears the filter, then closes.
- A session on this device: the connection attaches to it and its last
  messages are replayed. The previous session keeps running in the daemon.
  An ended session is loaded.
- A session on another device: the terminal drives it through the platform
  the way the apps do. Commands go to `sessions/{hostId}/{sessionId}/commands`
  with `t`, `from` (this device), `name` (`Terminal on <host>`) and `exp`
  headers; events and the answers come from its `events` stream and are
  rendered like a local session's. An ended session there is resumed first
  with `resume_session` on `hosts/{hostId}/commands`, which needs a daemon on
  that device. `/stop` stops it through its daemon; `/detach` stops
  following it and leaves it running.
- `+ new session in <workspace>` starts a session in that directory
  (`create_session`, over the socket or through `hosts/{hostId}/commands`).
  `+ new workspace on <device>…` browses the device's directories with
  `list_directories`, starting at the parent of its newest workspace (the
  home directory on this device), and starts a session in the one chosen.
  Both are offered for devices whose daemon answers. As in the apps, a
  device runs a daemon when its newest host record says so, a host record
  said so within the last six hours (a standalone `super` there writes host
  records without the flag), or one of its session records says so.
- When the daemon refuses the session picked (another process has its file
  open, say), the terminal stays attached to the session it showed.

In the full-screen terminal, which runs its session in-process, `/sessions`
looks the same. Picking a session leaves the in-process session saved (it
is listed as ended and can be picked again) and the terminal continues
attached to the session picked, as after `/remote`. Switching waits for no
turn: while one runs it is refused.

## The client library

`super_daemon::client::DaemonClient` is the Rust client the terminal uses:

- `DaemonClient::connect(&info)` finds the socket (`SUPER_DAEMON_SOCKET`,
  else `~/.super/run/daemon.sock`) and says `hello` with `info`'s kind
  (`tui`, `cli`, ...), name, version and protocol range; `hello()` and
  `protocol_version()` report what the daemon answered.
  `connect_or_start(&info)` adds the start on demand.
- Typed calls for every daemon-level command: `status`, `list_sessions`,
  `list_workspaces`, `list_directories`, `create_session`,
  `resume_session`, `attach`, `detach`, `stop_session`, `shutdown`.
  `execute(command)` sends a session command to the attached session and
  returns its response.
- `take_events()` returns the connection's event stream: daemon-level
  events as `ClientEvent::Daemon`, the attached session's events, exactly
  as `--mode rpc` emits them, as `ClientEvent::Session`. `closed()`
  resolves when the daemon closes the connection.
- Errors say what happened: `NotRunning`, `Unavailable` (with the reason
  nothing was started), `Skew` (the message above), `Command` (a command
  that came back with `success: false`), `Closed`, `Timeout`, and
  `ForeignSocket`: the client only talks to a daemon running as the same
  user, wherever `SUPER_DAEMON_SOCKET` points.

## Through the platform

While the user is signed in to the SUPER platform (and `remote.enabled` is
not false), the daemon keeps a link to it, checked every 30 seconds:

- `registry`: one `host` record when the link starts, a heartbeat every
  30 seconds, and one `session` record per session the daemon knows, hosted
  ones with their live state and stored ones as `ended`. Every record, the
  host record included, is sent again every six hours. The host record
  carries `"daemon": true`, so the apps know the daemon runs there before it
  has a session; a standalone `super` leaves the field out. Daemon sessions
  carry `"daemon": true` and `"resumeArgv": ["super", "--session", <file>]`.

  ```json
  {"kind":"host","hostId":"01J…","name":"mbp","platform":"macos","deviceId":"…","daemon":true,"at":1761728400000}
  ```

- `sessions/{hostId}/{sessionId}/{events,entries,commands}`: every hosted
  session is mirrored exactly like a terminal session, from the moment it
  loads until it unloads.
- `hosts/{hostId}/commands`: controllers append daemon-level commands here
  with the same headers as session commands (`t`, `from`, `name`, `exp`).
  `status`, `list_sessions`, `list_workspaces`, `list_directories`,
  `create_session`, `resume_session` and `stop_session` run exactly as over
  the socket; `hello`, `attach`, `detach` and `shutdown` answer
  `not available over the platform`. Expired commands are dropped and the
  cursor in `~/.super/agent/remote/cursors/` keeps a restarted daemon from
  running recent commands again.
- `hosts/{hostId}/events`: the response to every host command (with its
  `id`, also on a parse error) and every daemon-level event
  (`session_created`, `session_updated`, `session_ended`, `daemon_shutdown`).

Signing out, or a token the platform rejects, stops the link; signing in
again starts it within 30 seconds. The daemon never rewrites an event body,
so a platform error inside a model response (`budget_exceeded`,
`model_not_available`) reaches every client as it is.
