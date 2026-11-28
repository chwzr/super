# SUPER SDK

The SDK embeds SUPER's agent loop, model providers, tools, history, compaction,
and streaming events in another application. Rust, Python 3.11+, and TypeScript
use one Rust dispatcher and one event encoder, so they have the same behavior.
For another language, use [RPC mode](rpc.md).

## Rust

Add the workspace crate (or its published version) and Tokio:

```toml
[dependencies]
super-agent-sdk = "0.0.42"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

```rust
use super_sdk::Session;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let session = Session::builder().tools(["Read", "Bash"]).build().await?;
    let mut events = session.events();
    let printer = tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if event.event_type() == "message_update" {
                if let Some(delta) = event.0["assistantMessageEvent"]["delta"].as_str() {
                    print!("{delta}");
                }
            }
            if event.event_type() == "agent_settled" { break; }
        }
    });
    session.prompt("What files are here?").await?;
    printer.await?;
    Ok(())
}
```

`SessionOptions` configures `cwd`, model/provider/key, `models_file`, thinking
level, tool allow/exclude lists, custom Rust tools, prompts, project trust,
event capacity, and session persistence. The `mode` of the settings it loads,
or of `SessionOptions::settings`, picks the tools and the body of the system
prompt: `advanced` (the default) or `basic`, as in the terminal. The command
`set_mode` changes the mode and the two switches of a running session;
`AgentSession::update_settings` builds the system prompt again for a new mode
with the `PromptBuilder` the SDK installs (`AgentSession::set_prompt_builder`),
and a session whose engine runs elsewhere passes the change on
(`SessionBackend::set_mode`). SDK sessions are in-memory by default;
choose `SessionSource::Create` to persist one. Rust callers can restore caller-owned
history without creating a file through `SessionBuilder::restore_entries` or
`SessionSource::InMemoryEntries`.

## Python 3.11+

Published wheels target each CPython version from 3.11 through 3.15,
including free-threaded 3.14t and 3.15t. PyO3 0.29 does not support 3.13t.

```sh
pip install super-agent-sdk
```

```python
import asyncio
from super_sdk import Event, Session, ToolName


async def main() -> None:
    async with await Session.create(tools=[ToolName.READ, ToolName.BASH]) as session:

        async def print_events() -> None:
            async for event in session.events():
                if event.type == 'message_update':
                    update = event['assistantMessageEvent']
                    if update['type'] == 'text_delta':
                        print(update['delta'], end='', flush=True)
                if event.type == 'agent_settled':
                    return

        printer = asyncio.create_task(print_events())
        await session.prompt('What files are here?')
        await printer


asyncio.run(main())
```

The package uses Python 3.11 `StrEnum` values (`ThinkingLevel`, `ToolName`,
`StreamingBehavior`, `QueueMode`, `Mode`, `TaskKind`, `TaskStatus`) and
exports `TypedDict` declarations for commands, responses, models, messages,
events, statistics, background tasks (`TaskInfoData`), agents
(`AgentInfoData`, `AgentEntriesData`) and the mode (`ModeData`). `ToolName`
names every tool, the ten a session adds itself in the advanced mode
(`Monitor`, `TaskStop`, `Agent`, `SendMessage`, `ListAgents`, `Workflow`,
`Skill`, `ScheduleWakeup`, `EnterWorktree`, `ExitWorktree`) among them, as
does the TypeScript `ToolName` type. Event JSON is
converted directly into Python dict/list objects in Rust, without a per-token
`json.loads`.

## TypeScript: Node, Bun, and Deno

```sh
bun add @chwzr/super-agent-sdk
```

```ts
import { Session } from "@chwzr/super-agent-sdk"

const session = await Session.create({ tools: ["Read", "Bash"] })
const events = session.events()
session.promptDetached("What files are here?")
for await (const event of events) {
  if (event.type === "message_update" && event.assistantMessageEvent.type === "text_delta") {
    process.stdout.write(event.assistantMessageEvent.delta ?? "")
  }
  if (event.type === "agent_settled") break
}
session.close()
```

The native addon uses N-API rather than V8-specific APIs, so the same package
loads in Node, Bun, and Deno's Node compatibility layer. Deno needs `--allow-ffi`
plus permissions required by enabled tools.

## Browser / WebAssembly

SUPER offers two separate browser topologies.

### Local agent kernel

`@chwzr/super-agent-sdk-wasm` runs the conversation, model/tool turn loop, schema
validation, events, cancellation, limits, and checkpoints inside WebAssembly.
It needs no native SUPER process, WebSocket, or JSPI support. The host explicitly
provides model and tool capabilities:

```sh
wasm-pack build crates/super-core-wasm --target web --release
```

```ts
import init, { SuperAgent, createOpenAICompatibleProvider } from "@chwzr/super-agent-sdk-wasm"
await init()

const provider = createOpenAICompatibleProvider({
  url: "https://gateway.example/v1/chat/completions",
  apiKey: shortLivedBrowserToken,
})
const agent = SuperAgent.create(
  {
    model: { id: "my-model", provider: "openai", api: "openai-completions" },
  },
  provider,
)

agent.registerTool(
  {
    name: "lookup",
    description: "Look up a value",
    parameters: { type: "object", properties: { key: { type: "string" } }, required: ["key"] },
  },
  async (args, { signal }) => lookup(args, { signal }),
)

const result = await agent.prompt("Look up alpha", console.log)
console.log(result.text)
```

If a model returns tool-call content, the WASM loop validates and executes the
registered callback, adds its result to conversation history, and performs the
next model turn. Model and tool callbacks receive `AbortSignal`. Browser agents
also expose `steer`, `followUp`, `abort`, `setModel`, `setThinkingLevel`,
`messages`, `clearHistory`, `state`, and bounded versioned `checkpoint` data.
See `crates/super-core-wasm/README.md` and its no-server browser demo.

Browsers still cannot acquire ambient native filesystem/process authority.
Applications can register safe implementations backed by browser storage, the
File System Access API, WebContainers, or a remote sandbox. Use short-lived
model credentials in public browser applications.

### Remote native agent

`@super-sdk/wasm` remains the small WebSocket RPC client for applications that
need SUPER's native filesystem and shell implementations:

```sh
super --mode rpc --rpc-listen 127.0.0.1:9944 --no-session
wasm-pack build crates/super-wasm --target web
```

```ts
import init, { SuperClient } from "@super-sdk/wasm"
await init()
const client = await SuperClient.connect("ws://127.0.0.1:9944")
client.onEvent((event) => console.log(event))
await client.prompt("What files are here?")
```

The current RPC WebSocket has no authentication or Origin enforcement. Treat it
as development-only even on loopback until handshake authentication lands.

## Consistent operations

| Operation          | Rust                        | Python                      | TypeScript                  | RPC `type`                   |
| ------------------ | --------------------------- | --------------------------- | --------------------------- | ---------------------------- |
| create             | `Session::create` / builder | `Session.create`            | `Session.create`            | process startup              |
| prompt and wait    | `prompt`                    | `prompt`                    | `prompt`                    | `prompt` + wait for event    |
| accept immediately | `prompt_detached`           | `prompt_detached`           | `promptDetached`            | `prompt`                     |
| events             | `events().recv()`           | `async for ... in events()` | `for await ... of events()` | stdout/WebSocket lines       |
| steer              | `steer`                     | `steer`                     | `steer`                     | `steer`                      |
| follow up          | `follow_up`                 | `follow_up`                 | `followUp`                  | `follow_up`                  |
| abort / wait       | `abort` / `wait_idle`       | `abort` / `wait_idle`       | `abort` / `waitIdle`        | `abort`                      |
| background tasks   | `tasks` / `stop_task`       | `tasks` / `stop_task`       | `tasks` / `stopTask`        | `get_tasks` / `stop_task`    |
| agents             | `execute`                   | `agents` / `agent_entries`  | `agents` / `agentEntries`   | `get_agents` / `get_agent_entries` |
| mode               | `execute`                   | `set_mode`                  | `setMode`                   | `set_mode`                   |
| state/messages     | `state` / `messages`        | `state` / `messages`        | `state` / `messages`        | `get_state` / `get_messages` |
| model              | `set_model`                 | `set_model`                 | `setModel`                  | `set_model`                  |
| thinking           | `set_thinking_level`        | `set_thinking_level`        | `setThinkingLevel`          | `set_thinking_level`         |
| compact            | `compact`                   | `compact`                   | `compact`                   | `compact`                    |
| shell              | `bash`                      | `bash`                      | `bash`                      | `bash`                       |
| all commands       | `execute`                   | `execute`                   | `execute`                   | command object               |

Python uses snake_case and TypeScript uses camelCase by each language's normal
convention. Wire fields are always camelCase.

## Prompting and events

`prompt()` waits for completion in in-process SDKs. The RPC command and
`promptDetached()` return at acceptance so clients can send `abort` while work
continues. If a prompt arrives while streaming, pass `streamingBehavior:
"steer"` or `"followUp"`. Omitting it is an error.

Subscribe before sending a prompt. Important event types are `agent_start`,
`turn_start`, `message_start`, `message_update`, `tool_execution_start`,
`tool_execution_update`, `tool_execution_end`, `message_end`, `turn_end`,
`agent_end`, and SDK-level `agent_settled`. `message_update` contains deltas,
not a repeatedly growing snapshot. A bounded event channel prevents an idle
consumer from growing memory without limit. `event_lag` says how many events a
slow subscriber missed.

## Background tasks

A tool can start work that outlives its call: a shell command, a monitor,
an agent or a workflow. `Session::tasks` lists them and `Session::stop_task`
stops one by id or by name (RPC `get_tasks` and `stop_task`). When one ends
by itself, the session announces it to the agent with a notice: an idle
session starts a run for it, which ends with `agent_settled` as a prompt's
run does and which `wait_idle` waits for; a busy one reads it at its next
step. Clients see `task_started`, `task_updated` and `task_ended` events and
the notice as a `custom` message of type `task_notification`
([rpc.md](rpc.md#background-tasks)).

`abort` cancels the turn and, unless the embedder said it shows a stop
control for each task (`Session::set_task_controls(true)`, or the
`initialize` command with `taskControls: true`), stops the background tasks
too. `close` always stops them. A command a surface relays on behalf of
someone else goes through `Session::execute_by` with a `CommandSender`,
which names the origin and whether that sender has such controls; the
daemon passes what the client said in `hello`.

In Rust, a tool that runs in the background registers its task with the
harness session: `agent_session().background_tasks().register(TaskSpec)`
returns a `TaskHandle`, whose `stop_signal` is cancelled when the task is
stopped and whose `finish(TaskOutcome)` ends it and queues its notice.
`BackgroundTasks::new_id` gives an id before the task is registered, for a
tool that names its output file after it. `AgentSession::queue_entry(message,
Priority)` queues anything else that reaches the agent from outside a tool
call, with the priority `Now`, `Next` (the default) or `Later`; a monitor's
events go this way, as `super_coding::monitor_event` messages. A tool in
`super-agent`, which cannot see the session, reaches the registry through
`ToolSession::tasks` (`super_agent::TaskHost`), as `Bash` does with
`run_in_background`.

## Agents

In the advanced mode with the Subagents switch on, a session offers `Agent`,
`SendMessage` and `ListAgents` ([README](../README.md#subagents)). A child
agent that runs in the background is a background task of kind `agent` whose
id is the agent's id; its notice's `result` is the agent's report.

In Rust, `AgentSession::agent_registry` returns the session's
`super_coding::AgentRegistry`, the agents it started, with `list` and `find`
returning `AgentInfo` (`id`, `name`, `type`, `status`, `background`,
`sessionFile`, `worktree`, `origin`) and `session_of` an agent's own
`AgentSession`. The registry also keeps the agents of the session's workflow
runs and the child sessions of its loop and autoresearch jobs, whose
`origin` (`super_coding::agents::AgentOrigin`: `Agent`, `Workflow { run_id
}`, `Job { job_id, job_kind }`) says so; they are stored, rebuilt and
mirrored like the others, and `list_started` leaves them out, as the
model's `ListAgents` does.
`AgentSession::agent_at("a1b2c3d4e/a5f6g7h8i")` reaches an agent's agent by
the path of ids.
`AgentSession::agent_types` returns the agent types (`super_coding::AgentType`)
and `AgentSession::agent_type_list` their names and descriptions. An agent's
session knows who started it from `AgentSession::agent_link`.

An agent's system prompt is its type's instructions followed by the part of
the session's prompt after its body: the appended text, the project context
and the working directory. `SessionOptions` hands a session that part, the
types of the definition files and the skills. An embedder that builds its own
`AgentSession` does so with
`AgentSession::set_agent_resources(AgentResources { definitions,
prompt_context, skills })`, using `super_coding::agents::discover(cwd,
trusted)`, `super_coding::system_prompt::prompt_context` (with no skills) and
`super_coding::skills::discover`; without it an agent's prompt ends with the
working directory only, only the built-in types exist, and the `Skill` tool
has only the built-in skills. A message between agents is a
`super_coding::agent_message`.

## Skills and workflows

In the advanced mode a session with `Read` or `Bash`, or with the workflow
tool, has the `Skill` tool. It loads the skills of
`AgentResources::skills` by name, and the built-in skill
`workflow-authoring`, the guide to writing a workflow script, while dynamic
workflows are on. The session lists them after the system prompt on each
request (`super_coding::skills::skill_list_section`), and
`super_coding::system_prompt::prompt_context` leaves the list out in the
advanced mode; the basic mode keeps the list with the skills' files in the
system prompt and offers no `Skill` tool. A skill with `context: fork` in
its frontmatter runs in an agent of the session's `AgentRegistry`, waited for
or, with `background: true`, as a background task of kind `agent`.

While `subagents.enabled` and `workflows.enabled` are on in the advanced
mode, the main session offers `Workflow` with every request. A run is a
background task of kind `workflow` whose id is the run's id (`wf_` and eight
letters or digits, `RunRecord::run_id`); it ends with a notice, and
`Session::stop_task` stops it. `WorkflowRuntime::prepare` saves every run's
script in the session's folder of workflows
(`super_coding::session::manager::workflows_dir`, `<file without
.jsonl>/workflows/<run id>.js`; `<temp>/super-workflows/<session id>/` for a
session in memory), and its journal of finished agent calls beside it
(`<run id>.journal.jsonl`), each call as it finishes: the engine reports
every change of a run's journal to its runner
(`super_workflow::AgentRunner::journal_changed`,
`super_workflow::JournalChange`), and the session's runner adds it to the
file. `WorkflowRuntime::journal_for(run_id)` reads it back for a run that
resumes another, also one the end of its process cut off. The session stores
when each run starts and ends (`workflow_run` entries,
`super_coding::stored_state`); a run that still runs by that record when the
session is loaded again was cut off: it is marked stopped and its notice
(status `stopped`) is queued by `resume_stored_work`, once.
`AgentSession::set_closing` tells the session that its host ends it, so that
a run stopped by that end counts as cut off; `Session::close` calls it. The approver installed with `AgentSession::set_workflow_approver`
still answers before a run starts.

An agent's session is stored like the session itself, in the folder beside
the session's file (`<file without .jsonl>/agents/<agent id>.jsonl`,
`super_coding::session::manager::agents_dir`), and is left out of
`SessionManager::list` and `list_all`. A session built on a stored file
rebuilds its registry from that folder (`AgentRegistry::restore`), so
`list` has the agents of earlier runs at once, and an agent's session is
loaded from its file the first time it is needed: a `SendMessage` that
resumes it, `session_of`, `agent_at`, or a client's `get_agent_entries`.
`AgentRegistry::built_sessions` lists the loaded ones. An agent whose stored
folder no longer exists, and that has no worktree of its own, is loaded to
work in the session's folder (`AgentSession::working_folder`); its file
keeps the folder it was made in (`SessionManager::set_cwd` changes the folder
in memory only).

A `Session` wraps every loaded agent session as a `Session` of its own, a
view whose events are the agent's and which answers reading commands only
(`Session::is_agent_view`; other commands fail with
`super_sdk::AGENT_VIEW_REFUSAL`). `Session::agent_sessions` lists the views
by agent id, and `Session::subscribe_agents` returns them with a receiver of
`super_sdk::AgentChange`: `Loaded { id, session, events }`, with the agent's
events subscribed before its run starts, and `Reset` when the session moved
to another conversation. `Session::agent_view(path)` returns the view of
the agent at a path of ids (`a1b2c3d4e/a5f6g7h8i`), at any depth, loading a
stored one on the way; the daemon's `subscribe_agent` sends its `events` to
a socket client. The RPC commands `get_agents` and
`get_agent_entries` list a session's agents and read one's transcript
([rpc.md](rpc.md#agents)). `super_remote::SessionTransport` mirrors each view
to streams nested under the session's (`SessionStreams::agent`). An embedder
that builds its own `AgentSession` hears of the agent sessions through
`AgentSession::set_agent_observer` (`super_coding::agents::AgentObserver`)
and sends an agent session's events where it wants them with
`AgentSession::forward_events`; until then they go nowhere.

An agent with `isolation: "worktree"` works in a git worktree made by
`super_coding::worktree` (`create`, `has_changes`, `remove`, `recreate`),
with its built-in tools copied to that folder through
`AgentTool::in_directory`; a custom tool keeps the folder it was built for
unless it implements that method. Its branch starts where the
`worktree.baseRef` setting says (`worktree::start_point`).

The module `super_coding::subagents` and its six tools are gone.

## Wake-ups and worktrees

In the advanced mode a session that is no agent's, with `Read` or `Bash`,
also offers `ScheduleWakeup`, `EnterWorktree` and `ExitWorktree`.

`ScheduleWakeup` schedules the session's one pending wake-up
(`AgentSession::wakeups()`, `super_coding::Wakeups`: `pending`, `stop`,
`running_loop`). Its delay is kept between 60 and 3,600 seconds. When it is
due its prompt is queued with `Priority::Later`, as a `custom` message of type
`wakeup`, which starts a run of an idle session like a notice does. It holds
the session (`has_background_work`) until then, but `wait_for_background_work`
does not wait for it (`Wakeups::is_waiting` and `changes` tell a host that
waits; `AgentSession::wait_for_all_work` waits for the session's background
work and then for its wake-ups and the turns they start, as `super -p`
does). The pending wake-up and the self-paced loop are stored with the
session (`Wakeups::stored`, `super_coding::wakeup::StoredWakeups`) and come
back with it; `resume_stored_work` starts them. A self-paced loop
(`IterativeRuntime::start_self_paced`, which the terminal's `/loop` without an
interval uses in the advanced mode) runs its rounds as turns of the session
itself, each a `custom` message of type `loop_round`; the model continues it
with `ScheduleWakeup` and ends it with `stop`, and it ends by itself when the
session has nothing left to do and no wake-up is pending, or after 100 rounds.

`EnterWorktree` moves the session into a git worktree: its tools are copied
there with `AgentTool::in_directory`, the working directory in its system
prompt names it, and the agents, workflow agents and loop children it starts
work there. `AgentSession::worktree()` returns the `EnteredWorktree`
(`worktree`, `created`, `original`, `enteredAt`), `working_folder()` the
folder it works in, and a host can move a session itself with
`enter_worktree` and `leave_worktree`. `ExitWorktree` goes back, keeping or
removing the worktree (`super_coding::tools::worktree::exit_worktree`). A
new conversation (`replace_manager`) starts in the session folder again; the
worktree stays on disk. Each move emits `SessionEvent::WorktreeChanged`.

What a session keeps besides its conversation is stored in its file as
`custom` entries (`super_coding::stored_state`: `session_worktree`,
`session_wakeup`, `workflow_run`; the last entry of a type is the state, and
a fork copies none of them). `AgentSession` brings them back when it is built
from a stored session and in `replace_manager`, reading only: the worktree
it worked in, what it had read, its wake-up and self-paced loop. What writes
the file or starts turns waits for `AgentSession::resume_stored_work()`: a
worktree whose folder is gone is written down and announced
(`WorktreeChanged { missing }`), a wake-up is armed, the loop goes on, and a
workflow run that was cut off is marked and announced. `Session::create` and
`Session::attach` call it once the session's turn starter is installed; an
embedder that drives an `AgentSession` without the SDK calls it once it is
ready and holds the session's file. With `SessionOptions::defer_stored_work`
set, `Session::create` leaves it to the host, which calls
`Session::resume_stored_work()` once it listens to the session's events, so
that the announcements reach its client; `rpc::serve_streams` and
`serve_websocket` call it after they subscribe, and `--mode rpc` over stdio
defers. `SessionOptions::file_guard` is asked about the session's file
before the session is built from it, so a host can refuse a file another
process holds (`--mode rpc` does).

A session that ends in a worktree it made asks whether to keep or remove it:
`Session::settle_worktree(wait)` opens the question
(`super_sdk::worktree_exit_request`, an `ApprovalRequest` of kind
`Worktree` whose title is a placeholder and whose choices are
`WORKTREE_KEEP` and `WORKTREE_REMOVE`), waits at most `wait` for an answer
from any surface (`answer_approval`), and does what was chosen; without an
answer it keeps the worktree. The daemon calls it when a client stops a
session. An embedder that ends a session itself calls it before `close` if
it wants the question asked.

## Tools and safety

Defaults are `Read`, `Write`, `Edit`, and `Bash`. Optional built-ins are `grep`,
`find`, `ls`, and `mcp`. In the advanced mode a session with `Bash` also offers
`Monitor` and `TaskStop`, which work on its background tasks; leave `Bash` out
and they are gone too, unless the agent tools are offered, which bring
`TaskStop`. The allowlist (`tools`) and the exclusion list (`exclude_tools`)
apply to these tools too, and to the agent tools, `Workflow`, `Skill`,
`ScheduleWakeup`, `EnterWorktree` and `ExitWorktree`: an allowlist of `Read`
alone offers `Read` alone (`super_coding::ToolSelection`,
`AgentSession::set_tool_selection`). An agent's session starts from its
parent's selection, narrowed by its type (`ToolSelection::narrowed`): a type
that names its tools offers only those of them the parent's selection
admits. Set a read-only allowlist for untrusted
prompts. Project
resource loading is off by default in SDK sessions. Explicitly enable
`trust_project_files` only for a trusted directory.

Rust callers can implement `super_agent::AgentTool` and pass `custom_tools`.
A tool that returns `true` from `AgentTool::advanced_only` is left out of a
session in the basic mode; the others are there in both modes.

`Read`, `Edit` and `Write` take an absolute `file_path`. `Edit` takes
`old_string`, `new_string` and `replace_all`; the `edits` array and the
`path`, `oldText` and `newText` names of earlier versions are refused. A
session keeps a record of what each read returned, and `Edit` and `Write`
refuse a file that was not read or that changed on disk since
([README](../README.md#file-tools)). `AgentSession::file_state` returns that
record (`super_agent::FileStates`). Every `Read`, `Edit` and `Write` result
carries what it recorded in `details.fileRead` (`FileRead::to_json`: `path`,
`at`, `modified`, `len`, `whole`, `fingerprint`), and a session loaded from
its file (and a stored agent's session when it is loaded) takes the record
back from there (`super_coding::stored_state::rebuild_reads`), keeping a file
only while its modification time and size on disk are what they were then.
A session hands every tool its own
`super_agent::ToolSession`, the record and the free room of its context,
through `AgentTool::for_session`; a custom tool that keeps state per session
returns a copy of itself bound to it, and the default `None` uses the tool as
it is. A tool built outside a session gets a `ToolSession` of its own. A
`ToolSession` also carries the folder a `cd` left the main session's shell
in (`shell`, `None` in a subagent, whose commands start in the session folder)
and the session's background tasks (`tasks`); outside a session both are
`None`, so `Bash` keeps no `cd` and refuses `run_in_background` there.

`Bash` takes `command`, `timeout` in milliseconds (120,000 by default, 600,000
at most; in the background 600,000 and 7,200,000), `description`,
`run_in_background` and `dangerouslyDisableSandbox`, which has no effect; its
`cwd` parameter is gone and refused. Output and exit codes follow the rules in
the [README](../README.md#shell-tools).
Cross-language custom callback tools are intentionally not supported yet: a
callback during an agent tool batch needs cancellation, streaming updates, and
safe runtime re-entry semantics, and pretending a plain synchronous callback
is equivalent would be unsafe.
