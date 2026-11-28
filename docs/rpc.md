# RPC mode

RPC mode runs SUPER headlessly for any language or runtime. Commands are JSON
objects sent to standard input, one per line. Responses and asynchronous agent
events are JSON objects read from standard output, one per line.

```sh
super --mode rpc --no-session [--provider anthropic] [--model claude-sonnet-4-5]
```

The same protocol is what a client speaks to a session hosted by the SUPER
daemon after attaching; see [daemon.md](daemon.md).

For browsers, use a WebSocket listener:

```sh
super --mode rpc --rpc-listen 127.0.0.1:9944 --no-session
```

**Security:** RPC has no authentication. The default tools can read, modify, and
execute files. Bind only to loopback (`127.0.0.1` or `[::1]`) unless an
authenticated proxy and operating-system sandbox protect the process.

## Framing and correlation

A line feed (`\n`) is the only record separator. Strip one optional `\r` before
it. Do not use a generic reader that also splits on Unicode `U+2028` or `U+2029`:
those characters are valid inside JSON strings. Node's `readline` is not
protocol-safe. Buffer bytes and split on `\n`.

Every command may include `id`. Its response echoes it. Events normally have no
id. A `bash_execution_update` event echoes the id of its direct `bash` command.
A successful `prompt` response means accepted, not finished. Wait for
`agent_settled`. Successful `prompt` responses include `data.disposition`:
`started` means a new turn started; `queued` means the input entered the steering
or follow-up queue. Successful `steer` and `follow_up` responses report `queued`.

```json
{"id":"1","type":"prompt","message":"List files"}
{"type":"response","id":"1","command":"prompt","success":true,"data":{"disposition":"started"}}
{"type":"agent_start"}
{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Here"}}
{"type":"agent_settled"}
```

Failures are responses and do not close the connection:

```json
{
  "type": "response",
  "command": "set_model",
  "success": false,
  "error": "model not found: bad/model"
}
```

Malformed input uses command `parse`.

## Commands

All payload fields use camelCase.

### Prompt and lifecycle

- `{"type":"prompt","message":"...","images":[],"streamingBehavior":"steer"}`: accept a prompt. `streamingBehavior` is optional only while idle. Use `steer` or `followUp` while busy.
- `{"type":"steer","message":"...","images":[]}`: deliver after current-turn tools.
- `{"type":"follow_up","message":"...","images":[]}`: deliver once the agent stops.
- `{"type":"abort"}`: cancel the turn and respond once idle. Unless the
  client said in `initialize` that it shows a stop control for each
  background task, `abort` also stops the background tasks (see
  [Background tasks](#background-tasks)).
- `{"type":"clear_queue"}`: returns `data.messages`.
- `{"type":"ping"}`: returns `data.pong: true`.
- `{"type":"initialize","taskControls":true}`: what the client says about
  itself, best as its first command. `taskControls: true` means it shows a
  stop control for each background task, so its `abort` cancels the turn
  only. It counts for the client that sent it: in `--mode rpc` that is every
  direct client of the process, a remote client by its name. A client that
  never sends it, or sends `false`, is one without such controls. A daemon
  client says this in `hello` instead ([daemon.md](daemon.md#the-connection)).

An image is `{"type":"image","data":"<base64>","mimeType":"image/png"}`.

### State and sessions

- `get_state`: model, thinking level, streaming state, session id/file/name, message count, tool names, queue modes, retry and compaction state, `mode` (`basic` or `advanced`) with the `subagents` and `workflows` switches, the session's folder (`cwd`) and, while it works in one, its `worktree` (`path`, `branch`, `created`: whether `EnterWorktree` made it, `original`: the folder it goes back to), else `null`.
- `get_messages`: active conversation after branching/compaction.
- `get_entries`, optionally `since`: append-only entries after a durable entry-id cursor and current `leafId`.
- `get_tree`: recursive `{entry, children, label}` nodes and `leafId`.
- `get_last_assistant_text`: nullable `data.text`.
- `get_session_stats`: message/tool counts, cost, context use, and `tokens` with `input`, `output`, `cacheRead`, `cacheWrite`, `cacheReadAvailable`, and `total`.
- `set_session_name` with `name`, and `auto: true` for a title a client generated from the first prompt rather than a name someone typed (front ends take it as the title without showing a rename).
- `new_session`: replace active history with an empty in-memory session.
- `switch_session` with `sessionPath`.
- `fork` with `entryId`: move to the selected point and return editable user text.
- `get_fork_messages`: user entries available for branching.
- `export_html`, optional `outputPath`: writes an HTML transcript and returns its path.

### Models and thinking

- `set_model` with `provider` and `modelId`. A model the session does not know is looked for in `models.json` as it is now, and a `super` model after that on the platform, so a model added while the session runs can be chosen.
- `get_available_models`, optional `search` substring matched case-insensitively against `provider/id`: `data.models`, the models of providers that are configured in `models.json` or have a credential (a stored login or API key, or a key in the environment), the same set the TUI's `/model` picker and `super --list-models` offer. Models of providers without credentials are left out. Each model carries `provider`, `id`, `name`, `reasoning`, `input`, `cost`, `contextWindow` and `maxTokens`, in registry order. `set_model` still accepts any model the registry knows. The list is read when it is asked for, not when the session started: `models.json` is read again, and a signed-in session asks the platform for the account's models first, at most once a minute.
- `set_thinking_level` with one of `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`. Unsupported levels fail rather than silently downgrade.
- `get_available_thinking_levels`.

### Queue, retry, and compaction

- `set_steering_mode` / `set_follow_up_mode` with `mode` `all` or `one-at-a-time`.
- `compact`, optional `customInstructions`.
- `set_auto_compaction` / `set_auto_retry` with boolean `enabled`.
- `set_mode` with optional `mode` (`basic` or `advanced`), `subagents` and `workflows` (booleans, the Subagents and Dynamic workflows switches): sets those given for this session from now on, builds its system prompt again when the mode changed, and answers with all three (`data.mode`, `data.subagents`, `data.workflows`); with none given it only reads them. The user's settings file is not changed. A mode it does not know is refused: `mode 'expert' is not basic or advanced.` Turning Subagents off, or the mode to `basic`, stops the session's agents and workflow runs, as `/settings` does.

### Tools and shell

- `get_tools`: enabled tool names, such as `Read`, `Write`, `Edit`, and `Bash`. In the advanced mode `Monitor` and `TaskStop` come with `Bash`, `Skill` comes with `Read` or `Bash`, and so do `ScheduleWakeup`, `EnterWorktree` and `ExitWorktree` in a session that is no agent's, with the Subagents switch on `Agent`, `SendMessage`, `ListAgents` and `TaskStop` are there, and with the Dynamic workflows switch on too, `Workflow` and `Skill`. A session in the basic mode (the `mode` setting) lists no agent, workflow, `Skill`, `Monitor`, `TaskStop`, `ScheduleWakeup`, `EnterWorktree` or `ExitWorktree` tools.
- `bash` with `command`: runs immediately, streams `bash_execution_update`, returns output/exitCode/cancelled/truncated/fullOutputPath, and records a `bashExecution` message for the next model request.
- `abort_bash`.

### Tasks

See [Background tasks](#background-tasks) for what a task is.

- `get_tasks`: `data.tasks`, every background task the session remembers,
  oldest first, as the `task` object below.
- `stop_task` with `taskId`, the task's id or name: stops one running task
  and returns `data.task` with `status: "stopped"`. Fails when no task has
  that id or name, or when the task is not running any more.

### Agents

See [Agents](#agents-1) for how an agent's session is stored and sent.

- `get_agents`, optional `agent`: `data.agents`, the agents of the session,
  oldest first, those of an earlier run of the session included: the ones
  it started with the `Agent` tool, the agents of its workflow runs, and the
  child sessions of its `/loop` jobs with an interval and its `/autoresearch`
  jobs. Each is `{id, path, name, type, status, background, sessionFile,
  worktree, origin}`: `path` is what the agent commands take, `status` is
  `running`, `completed`, `failed` or `stopped`, `background` says whether
  its latest run is or was a background task, and `sessionFile` and
  `worktree` (`{path, branch, base, project}`) are there when the agent has
  them. `origin` says where it comes from: `{"kind": "agent"}` for an
  `Agent` call (or a skill that runs in an agent), `{"kind": "workflow",
  "runId": "wf_…"}` for an agent of a workflow run, and `{"kind": "job",
  "jobId": 3, "jobKind": "loop"}` (or `"autoresearch"`) for a job's child
  session. A workflow's agent has the `type` `workflow` and the name of its
  `label`, else of its task (`wf_<phase>_<n>`); a job's child has the type
  `loop` or `autoresearch` and the name `loop_<job id>` or
  `autoresearch_<job id>`; its status is that of its latest iteration. With
  `agent`, the agents that agent started.
- `get_agent_entries` with `agent`, optional `since`: the entries of the
  agent's session, its transcript, as `get_entries` answers for the session
  (`entries` after the `since` entry id, and `leafId`), with the agent's
  `sessionId` and `sessionFile`. Fails when no agent has that path.

`agent` is an agent's `path`: its id for an agent of this session, and the
ids from there down, separated by `/`, for an agent of an agent
(`a1b2c3d4e/a5f6g7h8i`). An agent that had ended before the session was
loaded again is read from its stored session.

An agent's live events reach a client through the platform on the agent's
nested `events` stream ([Agents](#agents-1)) and, on the daemon's socket,
after `subscribe_agent` with the agent's path, as `agent_event` lines that
carry each event unchanged ([daemon.md](daemon.md#an-agents-events)).
`--mode rpc` and the SDK's `execute` have no such command: an embedder
follows an agent with `Session::agent_view(path)` and its `events`.

### Approvals

Some steps wait for a human: a dynamic workflow before it starts, the project
trust question, a login prompt. Every surface sees the same question and any
surface may answer it. The first answer wins.

- `get_approvals`: `data.approvals`, each `{approvalId, kind, title, detail, choices}`.
- `answer_approval` with `approvalId` and `choice` (a choice id from the request). Fails when the id is unknown, already answered, or the choice is not offered.

`kind` is `workflow` (choices `approve`, `cancel`), `trust` (`trust`,
`untrust`), `login` or `other`. Login prompts are announced but cannot be
answered over RPC in this version.

## Events

Lifecycle: `agent_start`, `agent_end`, `agent_settled`, `turn_start`, `turn_end`,
`message_start`, `message_update`, `message_end`.

Tools: `tool_execution_start`, `tool_execution_update`, `tool_execution_end`,
and direct-shell `bash_execution_update`. The `args` of `Read`, `Edit` and
`Write` name the file in `file_path`, an absolute path; `Edit` has
`old_string`, `new_string` and `replace_all`. Sessions stored before this
change carry `path` (and `edits` for `edit`) under the old lower-case tool
names. An `Edit` result keeps its unified diff in `details.diff`.

Session: `queue_update`, `compaction_start`, `compaction_end`, `retry`,
`model_changed`, `workflow_progress`, `workflow_outcome`, and `event_lag`.

Background tasks: `task_started`, `task_updated` (its `progress` line
changed) and `task_ended`, each with `task`.

Approvals: `approval_request` with `approvalId`, `kind` (`workflow`,
`trust`, `login`, `worktree` or `other`), `title`, `detail`, and `choices`
(`[{id, label, detail}]`); `approval_resolved` with `approvalId`, `choice`,
and `by` (`local` for the terminal, `rpc` for a direct client, otherwise the
remote device name). A `worktree` question comes when a session that works
in a worktree it made with `EnterWorktree` ends: its `detail` is the
worktree's folder and its choices are `keep` and `remove`; see
[Wake-ups, loops and worktrees](#wake-ups-loops-and-worktrees).

Commands from another surface: `command_applied` with `origin` (the name
that surface gave itself: a daemon client's `hello.name`, the device a
platform command came from), `command` (the command as sent, `imageCount` in
place of images), `success`, `error` and `data` (the response's data). Every
subscriber receives it after such a command ran, the sender included; reads
(`get_*`), `ping` and `answer_approval` (announced as `approval_resolved`)
are not announced, and neither is a command a direct `--mode rpc` client
sends. A prompt that started a run is visible only this way: the run's
events begin with the reply, not with the prompt.

`agent_settled` follows every run, including runs the interactive terminal
started itself when the session is mirrored for remote control.

`message_update.assistantMessageEvent` types are `start`, `text_start`,
`text_delta`, `text_end`, `thinking_start`, `thinking_delta`, `thinking_end`,
`toolcall_start`, `toolcall_delta`, `toolcall_end`, `done`, and `error`. Assemble
live output by `contentIndex`. `message_end.message` is authoritative.

## Background tasks

A background task is work a tool started that keeps running after the tool
call returned: a shell command, a monitor, an agent or a workflow. Every
session keeps a list of them.

```json
{"type":"task_started","task":{"id":"b3k9x0q2a","kind":"shell","description":"npm test","status":"running","toolUseId":"call_1","outputFile":"/tmp/super-tasks/…/b3k9x0q2a.output","startedAt":1764280000000}}
{"type":"task_ended","task":{"id":"b3k9x0q2a","kind":"shell","description":"npm test","status":"completed","toolUseId":"call_1","outputFile":"/tmp/super-tasks/…/b3k9x0q2a.output","summary":"…","startedAt":1764280000000,"endedAt":1764280042000}}
```

A `task` has `id`, `kind` (`shell`, `monitor`, `agent` or `workflow`),
`description`, `status` (`running`, `completed`, `failed` or `stopped`),
`startedAt` and, once it ended, `endedAt` (milliseconds since the Unix
epoch), and, when they are known, `name`, `toolUseId` (the tool call that
started it), `outputFile` (its full output or transcript), `progress` and
`summary`.

When a task ends by itself, the session queues a notice for the agent. While
the agent is idle the notice starts a new run: `agent_start`, the notice's
`message_start` and `message_end`, the reply, `agent_settled`. While it works
the notice arrives at its next step, between two tool rounds. Notices that
wait together arrive together, in one run or one step; `one-at-a-time` of
`set_steering_mode` and `set_follow_up_mode` paces only what a client queued
with `steer` and `follow_up`. The notice is
a message with `role: "custom"` and `customType: "task_notification"`; its
`details` carry the `task` and the `outcome` (`status`, `summary` and, when
there is one, `result` and `usage`). Show it as a notice, not as something
the user typed. It is part of the session history, so `get_entries` and
`get_messages` return it too. A task stopped with `stop_task`, with `abort`
or by the model gets no notice: it ends with `task_ended` and
`status: "stopped"`.

A `Workflow` call starts its run as a task of kind `workflow` whose id is the
run's id (`wf_` and eight letters or digits) and whose `name` is the call's
`name`, else the script's, while no other running task has it; its notice's `result` is the
workflow's answer. The call's result has `details.runId`, `details.name`,
`details.scriptPath` (where the run's script was saved, or `null`) and
`details.resumedFrom` (the run whose finished agent calls it replays, or
`null`). The question before a run starts is the `approval_request` of kind
`workflow` above. The session stores each run's start and end (`custom`
entries with `customType: "workflow_run"`: `runId`, `name`, `description`,
`status`, `at`). A run that the end of its process cut off is announced once
when the session is loaded again: a notice with `status: "stopped"`, the run's
id as `task.id`, a summary that says how many of its agent calls had
finished, and a placeholder `outcome.result`; its stored entry then says
`stopped` with `cutOff: true`. A skill that runs in an agent of its own is a `Skill` call
whose result has `details.skill` and `details.agent`, and in the background
a task of kind `agent`.

A monitor (`kind: "monitor"`) also reaches the agent while it runs: each of
its events is a `custom` message with `customType: "monitor_event"`, queued
and delivered the way a notice is. Its `details` carry the `task`, the
`event` (the lines of the event) and `omittedLines` (lines left out of a
long event, which are in the task's output file). Show it as a notice too.

A message between agents (`SendMessage`) is a `custom` message with
`customType: "agent_message"`, queued and delivered the same way. Its
`details` carry `from` (`main`, or the id of the agent that sent it), `to`,
`summary` (or `null`) and `message`. The main session gets one when one of
its agents writes to `main`. Show it as a notice too.

An agent in the background (`kind: "agent"`) is a task with the agent's id
and, while no other running task has it, the agent's name as its `name`, so
`stop_task` takes either; its notice's `result` is the agent's report and `outputFile` its stored
transcript. A message that resumes an agent that had ended starts a new run
of the same task id. An `Agent` call's `details` carry the `agent` (`id`,
`name`, `type`, `status`, `background`, `sessionFile`, and `worktree` while
it has one) and, for an agent the call waited for, its `usage`; a
`SendMessage` call's carry `to` and, for an agent, the `agent` and
`resumed`. The notice of an agent that left changes in its worktree names it
in `outcome.worktree`.

A `Bash` call with `run_in_background` answers at once; its `details` carry
`backgroundTaskId` and `outputFile`. A `Bash` call in the foreground has
`details.exitCode` and, when the output was too long to return,
`details.fullOutputPath`. A `Monitor` call's `details` carry `taskId`,
`outputFile` and `timeoutMs`, and a `TaskStop` call's the stopped `task`.

`abort` cancels the turn. Whether the background tasks stop with it depends
on the client: one that shows a stop control for each task (`initialize`
with `taskControls: true`, or `hello.taskControls` on the daemon) keeps them
running and stops them one by one with `stop_task`; any other client stops
them all with `abort`, since it has no other way to.

## Wake-ups, loops and worktrees

A `ScheduleWakeup` call answers at once. Its `details.wakeup` is the pending
wake-up: `id`, `delaySeconds` (kept between 60 and 3600), `dueAt`
(milliseconds since the Unix epoch), `reason`, `noop`, `prompt` and, during a
self-paced loop of the terminal, `job` and `round`; `details.requestedDelaySeconds`
is the delay the model asked for. A call with `stop` has `details.stopped`
(`loop` with its `round`, or the cancelled `wakeup`, or `null`). A session has
one pending wake-up. It is stored in the session with its self-paced loop (a
`custom` entry with `customType: "session_wakeup"`: `wakeup` and `loop`, with
the loop's `job`, `goal`, `round` and `maxRounds`), and comes back when the
session is loaded again: a wake-up whose time has passed fires at once, a
later one at its time, and the loop goes on in the round it was in (with a
new `job` id); a loop that was cut off before its round scheduled a wake-up
runs that round again.

When a wake-up outside a loop is due, the session queues a `custom` message
with `customType: "wakeup"`: its text is the wake-up's `prompt`, and its
`details.wakeup` is the wake-up. It starts a run like a notice when the agent
is idle, and waits for the end of the run that works otherwise. A round of a
self-paced loop (the terminal's `/loop` without an interval in the advanced
mode) is a `custom` message with `customType: "loop_round"`, whose
`details` carry `job`, `round`, `maxRounds`, `prompt` (the loop's goal) and
`wakeup` (the wake-up that started it, or `null` for the first round). Show
both as notices; `reason` and `noop` are worth showing with them.

An `EnterWorktree` call's `details.worktree` has `worktree` (`path`,
`branch`, `base`, `project`), `created` (`false` for a worktree entered by
its `path`), `original` (the folder the session goes back to) and
`enteredAt`, and `details.start` is the ref a new branch started from
(`refs/remotes/origin/<branch>`, or `null` for `HEAD`). An `ExitWorktree`
call's `details` carry `action`, `left` (whether the session left a
worktree) and `worktree`, and `removed` for a removed one. While a session
works in a worktree, its tools, the agents it starts and the working
directory in its system prompt use the worktree. Each move is the event
`worktree_changed`, with `folder` (the folder the session works in now) and
`worktree` (as in `get_state`, or `null`). The worktree is stored in the
session (a `custom` entry with `customType: "session_worktree"`): a session
loaded again works in it again while its folder exists. When the folder is
gone, the session works in its own folder, stores that, and says so once:
`worktree_changed` then also carries `missing`, the worktree's folder, and
the stored entry has `missing` too, which the terminal shows as a line.

A session that ends in a worktree it made asks its clients whether to keep
or remove it, with an `approval_request` of kind `worktree` (above). Answer
it with `answer_approval` and `keep` or `remove`; `remove` stops the
session's background tasks and deletes the worktree with its branch and its
changes. The interactive terminal asks when it quits; the daemon asks when a
client stops the session with `stop_session`, and keeps the worktree when no
answer comes within a minute. `--mode rpc` cannot ask at its end: it keeps
the worktree, and its last line on standard output, after the client closed
its input, is the event `worktree_kept` with the `worktree` (as in
`get_state`); with `--rpc-listen` it names the worktree on standard error
instead. The session is in that worktree again when it is loaded.

## Agents

An agent the `Agent` tool starts runs in a session of its own, which is
stored and sent like the session itself, but is not a session of its own in
any list. So does every agent of a workflow run and the child session of a
`/loop` job with an interval or an `/autoresearch` job; they differ only in
their `origin` and in that the model's `ListAgents` and `SendMessage` do not
reach them, since they run as their workflow or job says:

- **Stored** next to its parent's session file, in a folder named after it:
  `<session file without .jsonl>/agents/<agent id>.jsonl`, and an agent's
  agents one folder further. The listings of stored sessions (the terminal's
  pickers, the daemon's `list_sessions`, the platform's registry) leave them
  out, and `--session <id>` does not find them. The child's first entries
  are a `custom` entry with `customType: "subagent"` (`agentId`, `taskName`,
  `agentType`, `origin`, `canonicalPath`, `depth`, `toolUseId`, `parentSessionId`,
  `model`, `thinkingLevel`, `worktree`), and every run's end adds one with
  `customType: "agent_run"` (`status`, `background`, `worktree`).
- **Rebuilt** when the session is loaded again, after a restart or in the
  daemon: `get_agents` and the model's `ListAgents` list the stored agents,
  and a message to one resumes it from its transcript. An agent whose run was
  cut off by the end of its process counts as `stopped`.
- **Sent** to its own S2 streams, nested under its parent's, while the
  session is mirrored for remote control:
  `sessions/<machine>/<session>/agents/<agent id>/events`, `entries` and
  `commands`, one level further for an agent of an agent. They are written
  from the moment the agent's session is loaded: when it starts, and when a
  stored one is resumed or read. The events are those of a session, from the
  agent's `agent_start` to its `agent_settled`; the entries are its
  transcript.
- **Read only.** An agent runs as its parent says. Its `commands` stream,
  and the view of its session the SDK hands out, answer the reading
  commands (`get_*` and `ping`) and refuse every other one with "An agent's
  session takes reading commands only; send this command to the session that
  started the agent." Stop an agent with `stop_task` on its parent's
  session; the model writes to it with `SendMessage`.

An agent started with `isolation: "worktree"` works in a git worktree of its
own, `.super/worktrees/agent-<id>` at the top of the project, on the new
branch `worktree-agent-<id>`. When a run ends with nothing changed in it, the
worktree and its branch are removed; otherwise both stay, and the agent's
`worktree` names them. A resumed agent gets its worktree back first.

## Minimal Python subprocess client

```python
import json, subprocess

process = subprocess.Popen(
    ['super', '--mode', 'rpc', '--no-session'],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    text=True,
)
process.stdin.write(
    json.dumps({'id': '1', 'type': 'prompt', 'message': 'Hello'}) + '\n'
)
process.stdin.flush()
for raw_line in process.stdout:
    value = json.loads(raw_line.removesuffix('\n').removesuffix('\r'))
    if value.get('type') == 'message_update':
        update = value['assistantMessageEvent']
        if update['type'] == 'text_delta':
            print(update['delta'], end='', flush=True)
    if value.get('type') == 'agent_settled':
        break
```

## Minimal Go subprocess client

```go
cmd := exec.Command("super", "--mode", "rpc", "--no-session")
in, _ := cmd.StdinPipe(); out, _ := cmd.StdoutPipe(); _ = cmd.Start()
_ = json.NewEncoder(in).Encode(map[string]any{"id":"1", "type":"prompt", "message":"Hello"})
scanner := bufio.NewScanner(out) // Scanner splits on LF, as required.
for scanner.Scan() {
    var value map[string]any
    _ = json.Unmarshal(scanner.Bytes(), &value)
    if value["type"] == "agent_settled" { break }
}
```
