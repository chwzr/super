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
prompt: `advanced` (the default) or `basic`, as in the terminal. SDK sessions are in-memory by default;
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
`StreamingBehavior`, `QueueMode`) and exports `TypedDict` declarations for
commands, responses, models, messages, events, and statistics. Event JSON is
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
| background tasks   | `tasks` / `stop_task`       | `execute`                   | `execute`                   | `get_tasks` / `stop_task`    |
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

## Tools and safety

Defaults are `Read`, `Write`, `Edit`, and `Bash`. Optional built-ins are `grep`,
`find`, `ls`, and `mcp`. In the advanced mode a session with `Bash` also offers
`Monitor` and `TaskStop`, which work on its background tasks; leave `Bash` out
and they are gone too. Set a read-only allowlist for untrusted prompts. Project
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
record (`super_agent::FileStates`). A session hands every tool its own
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
