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

- `get_state`: model, thinking level, streaming state, session id/file/name, message count, tool names, queue modes, retry and compaction state.
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

### Tools and shell

- `get_tools`: enabled tool names, such as `Read`, `Write`, `Edit`, and `Bash`. In the advanced mode `Monitor` and `TaskStop` come with `Bash`. A session in the basic mode (the `mode` setting) lists no agent, workflow, `Monitor` or `TaskStop` tools.
- `bash` with `command`: runs immediately, streams `bash_execution_update`, returns output/exitCode/cancelled/truncated/fullOutputPath, and records a `bashExecution` message for the next model request.
- `abort_bash`.

### Tasks

See [Background tasks](#background-tasks) for what a task is.

- `get_tasks`: `data.tasks`, every background task the session remembers,
  oldest first, as the `task` object below.
- `stop_task` with `taskId`, the task's id or name: stops one running task
  and returns `data.task` with `status: "stopped"`. Fails when no task has
  that id or name, or when the task is not running any more.

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

Approvals: `approval_request` with `approvalId`, `kind`, `title`, `detail`,
and `choices` (`[{id, label, detail}]`); `approval_resolved` with
`approvalId`, `choice`, and `by` (`local` for the terminal, `rpc` for a direct
client, otherwise the remote device name).

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
the notice arrives at its next step, between two tool rounds. The notice is
a message with `role: "custom"` and `customType: "task_notification"`; its
`details` carry the `task` and the `outcome` (`status`, `summary` and, when
there is one, `result` and `usage`). Show it as a notice, not as something
the user typed. It is part of the session history, so `get_entries` and
`get_messages` return it too. A task stopped with `stop_task`, with `abort`
or by the model gets no notice: it ends with `task_ended` and
`status: "stopped"`.

A monitor (`kind: "monitor"`) also reaches the agent while it runs: each of
its events is a `custom` message with `customType: "monitor_event"`, queued
and delivered the way a notice is. Its `details` carry the `task`, the
`event` (the lines of the event) and `omittedLines` (lines left out of a
long event, which are in the task's output file). Show it as a notice too.

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
