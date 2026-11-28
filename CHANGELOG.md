# Changelog

## Unreleased

### Added

- A `mode` setting, with the values `advanced` (the default) and `basic`, in
  `/settings` as `Mode`. The basic mode offers only the core tools and the
  ones you configured, `grep`, `find`, `ls`, `mcp` and `webmcp`, with the
  simple system prompt; subagents and dynamic workflows are off there,
  whatever their switches say. The advanced mode offers every tool and starts
  from its own system prompt. Only the user's settings decide the mode, not a
  project's.
- Rust SDK: `AgentTool::advanced_only` marks a tool that a session in the
  basic mode leaves out. `AgentSession::mode`, `AgentSession::system_prompt`
  and `AgentSession::set_system_prompt` read the mode and read or replace the
  system prompt. `super_agent::placeholder` writes the stand-in for prompt
  text that is not written yet; `super_agent::placeholder::SLOTS` lists every
  slot of prompt text with its text, so one search for `SUPER_PROMPT_TODO(`
  finds all of them.
- Background tasks: work a tool starts that keeps running after its call,
  such as a shell command, a monitor, an agent or a workflow. When one ends,
  the agent is told with a notice that starts a new turn if it is idle and
  arrives at its next step if it works. The notice is recorded in the
  session as a notice, not as something the user typed. `/tasks` in the
  terminal lists the running tasks and stops one; `Esc` and `Ctrl+C` cancel
  the turn and leave the tasks running. A run with `-p` or `--mode json`
  waits for its tasks before it exits and stops them on `Ctrl+C` or
  `SIGTERM`. The daemon keeps a session with running tasks loaded.
- RPC: `get_tasks` and `stop_task` list and stop background tasks, and the
  events `task_started`, `task_updated` and `task_ended` report them.
  `initialize` with `taskControls: true` (on the daemon, `hello` with
  `taskControls: true`) says that a client shows a stop control for each
  task, so its `abort` cancels the turn only.
- Rust SDK: `AgentSession::queue_entry` queues what reaches the agent from
  outside a tool call with the priority `Now`, `Next` or `Later`;
  `AgentSession::background_tasks` registers, lists, finds and stops tasks;
  `super_coding::task_notification` builds a task's notice.
  `Session::tasks`, `Session::stop_task`, `Session::set_task_controls` and
  `Session::execute_by` with a `CommandSender` cover the same for embedders.
- A session remembers what it read. `Edit` and `Write` refuse a file the
  session has not read, and one that changed on disk since; for `Write` over
  an existing file only a read of the whole file counts. A new file needs no
  read, and the session's own edits keep the record current. A subagent
  keeps its own record and a fork starts with a copy of its parent's.
- `Read` makes an image smaller before it sends it: one larger than 2,000
  pixels on a side or 5 MB in base64 is scaled down and, if still too heavy,
  saved again as JPEG.
- `Read` takes `allow_large`, which lets one read fill up to 70% of the free
  room in the context.
- Rust SDK: `AgentSession::file_state` returns the session's record of
  reads, and `AgentTool::for_session` binds a tool to a session's
  `ToolSession`.
- `Bash` takes `run_in_background`: the command runs as a background task,
  for ten minutes by default and two hours at most, with its output in the
  task's output file, and the agent is told when it ends. `Bash` also takes
  `description` and `dangerouslyDisableSandbox`, which has no effect.
- `Monitor`, in the advanced mode, watches a script or a WebSocket in the
  background and brings each line of the script's output, or each frame,
  to the agent as an event; lines within 200 ms come as one. A monitor ends
  after `timeout_ms`, five minutes by default and ten at most, and one with
  more than 60 events within a minute is stopped. Its events are `custom`
  messages of type `monitor_event`.
- `TaskStop`, in the advanced mode, stops a background task by id or by
  name, with `task_id` or its older name `shell_id`. `Monitor` and
  `TaskStop` come with `Bash`.
- Rust SDK: `ToolSession::shell` and `ToolSession::tasks` give a tool the
  main session's shell folder and the session's background tasks
  (`super_agent::TaskHost`); `BackgroundTasks::new_id` and
  `super_coding::monitor_event` are new.
- Subagents, in the advanced mode with the `Subagents` switch on, are
  started with `Agent`, written to with `SendMessage` and listed with
  `ListAgents`; `TaskStop` stops one in the background. A child runs in the
  background by default, and its report reaches the agent in a notice that
  starts a new turn if it is idle; with `run_in_background` set to `false`
  the agent waits for the report. `subagent_type` takes `general-purpose`,
  `Explore`, `Plan` or `fork`, a copy of the whole conversation on the same
  model, or a type from a Markdown file in `.super/agents/` of a trusted
  project or in `~/.super/agent/agents/`. `model` takes any model of the
  catalog and `effort` a thinking level the model accepts. `SendMessage`
  reaches a running child at its next step, resumes one that has ended, and
  lets a child write to `main`. At most 20 children of a session run at
  once, and they nest three levels deep. `Esc` stops the child the agent
  waits for and leaves the ones in the background running. A message
  between agents is a `custom` message of type `agent_message`.
- Rust SDK: `AgentSession::agent_registry`, `agent_types`,
  `agent_type_list`, `agent_link` and `set_agent_resources`,
  `super_coding::agents`, `super_coding::agent_message` and
  `super_coding::system_prompt::prompt_context` are new.
- A subagent's session is saved like the session itself, beside it, and a
  session opened again, in the terminal or in the daemon, knows its
  subagents: `ListAgents` lists them and `SendMessage` resumes one from its
  transcript. While the session is mirrored to the platform, each subagent
  is mirrored to streams nested under the session's,
  `sessions/<machine>/<session>/agents/<agent>/events`, `entries` and
  `commands`; an agent's commands stream answers reading commands only.
- `Agent` with `isolation: "worktree"` runs the child in a git worktree of
  its own under `.super/worktrees/` in the project, on a new branch. A
  worktree the child changed nothing in is removed with its branch when the
  child ends; one with changes stays, and the report names its folder and
  branch.
- RPC: `get_agents` lists a session's subagents, those of earlier runs
  included, and `get_agent_entries` reads a subagent's transcript.
- Daemon: `subscribe_agent` with an agent's path sends the connection that
  agent's events live, at any depth, as `agent_event` lines that carry each
  event as the agent's nested `events` stream does; `unsubscribe_agent` ends
  it. Rust SDK: `Session::agent_view` returns an agent's view by its path,
  and `super_sdk::daemon::AgentEvent` is the line.
- The agents of a workflow run and the child session of a `/loop` job with
  an interval or an `/autoresearch` job are subagents of the session for
  storage and for the apps: each has an agent id, is saved in the session's
  folder of agents, is listed by `get_agents` with an `origin` that names its
  workflow run (`runId`) or its job (`jobId`, `jobKind`), is read with
  `get_agent_entries`, is mirrored to the nested streams, and is known again
  when the session is opened again. `ListAgents` and `SendMessage` reach only
  the agents the session started itself.
- Rust SDK: `Session::agent_sessions` and `Session::subscribe_agents` hand
  out each loaded subagent session as a read-only `Session`
  (`Session::is_agent_view`), `AgentSession::agent_at` reaches an agent by
  its path of ids, `AgentSession::set_agent_observer` and `forward_events`
  let an embedder follow agent sessions, `AgentRegistry::restore` rebuilds a
  registry from the stored agents, `super_coding::worktree` makes, checks
  and removes worktrees, `AgentTool::in_directory` copies a tool to another
  folder, and `SessionStreams::agent` names an agent's streams.
- `Skill` in the advanced mode: the model loads a skill by its name, and the
  skill's text comes back into the turn; `args` fills in `$ARGUMENTS`. The
  skills are listed without their files. A skill with `context: fork` in its
  frontmatter runs in an agent of its own, of the type `agent` names, waited
  for or, with `background: true`, in the background with a notice when it
  ends. The basic mode lists the skills with their files and the model reads
  them with `Read`, as before.
- `Workflow` takes `scriptPath` to run a script from a file, `name` alone to
  run a saved workflow from `.super/workflows/` or
  `~/.super/agent/workflows/`, `args` for the script's `args`, and
  `resumeFromRunId` to replay the finished agent calls of an earlier run up
  to the first one whose prompt changed. Every run saves its script and its
  journal of finished agent calls in the session's folder of workflows, so a
  run can be resumed after the session was opened again.
- Rust SDK: `AgentResources::skills`, `super_coding::skills::{skill_list_section,
  load, skill_text}`, `RunRecord::run_id` and `script_path`,
  `WorkflowRuntime::find`, `journal_for` and `directory`,
  `super_coding::session::manager::workflows_dir` and
  `super_workflow::Journal::completed`.
- `ScheduleWakeup`, in the advanced mode, brings the agent back after a
  delay of one minute to one hour, with the prompt it gave or with the next
  round of a self-paced loop, or ends the loop. A session has one pending
  wake-up, which keeps a session in the daemon loaded.
- `EnterWorktree` and `ExitWorktree`, in the advanced mode, move the session
  into a git worktree and back: a new one under `.super/worktrees/` on a new
  branch, or one of the repository that exists. While the session is there,
  `Bash`, the file tools, its agents and the working directory in its system
  prompt use the worktree. `ExitWorktree` keeps or removes a worktree it
  made, and does not remove one with uncommitted files or new commits unless
  told to discard them, nor one where a background task still runs.
- The `worktree.baseRef` setting: `fresh`, the default, starts a new
  worktree's branch from the remote's default branch, fetched first; `head`
  from `HEAD`. Agents with `isolation: "worktree"` follow it too.
- When the terminal quits, or a client stops a session in the daemon, while
  the session works in a worktree it made, SUPER asks whether to keep or
  remove the worktree; the terminal and the apps both show the question
  (`approval_request` of kind `worktree`, choices `keep` and `remove`).
- Rust SDK: `AgentSession::wakeups`, `worktree`, `working_folder`,
  `enter_worktree` and `leave_worktree`, `IterativeRuntime::start_self_paced`,
  `Session::settle_worktree`, `super_sdk::worktree_exit_request`,
  `super_coding::worktree::{start_point, changes, list, describe, top}`, and
  the messages `wakeup` and `loop_round` (`super_coding::notifications`).
- A session that works in a worktree works there again when it is opened
  again (`super -c`, `--session`, the daemon, `/remote`, `switch_session`);
  when the worktree's folder is gone it works in its own folder and says so
  once. RPC: `get_state` reports the session's `cwd` and its `worktree`, and
  the event `worktree_changed` tells clients when the session moves. Rust
  SDK: `super_coding::stored_state`, `SessionEvent::WorktreeChanged`,
  `AgentSession::resume_stored_work` and `SessionOptions::file_guard`, which
  `--mode rpc` uses to refuse a session file another process holds before it
  builds the session.
- A pending wake-up and a running self-paced loop are stored with the
  session and come back when it is opened again: a wake-up whose time has
  passed comes at once, a later one at its time, and the loop goes on in the
  round it was in. A daemon that shuts down while wake-ups are pending loads
  those sessions again, after its restart, when the wake-ups are due.
- `super -p` and `--mode json` wait for a pending wake-up and the turn it
  starts, as they wait for background tasks; `Ctrl+C` or `SIGTERM` ends the
  wait. Rust SDK: `AgentSession::wait_for_all_work`.
- A session opened again (`super -c`, `--session`, the daemon, `/remote`,
  `switch_session`) takes back from its transcript what it had read: a file
  it read, or changed itself, counts as read while its modification time and
  size on disk are what they were then, so the model need not read it again.
  A stored agent's session does the same when it is loaded. The results of
  `Read`, `Edit` and `Write` carry the read in `details.fileRead`. Rust SDK:
  `FileRead::to_json` and `from_json`, `super_coding::stored_state::rebuild_reads`.
- Changing `Mode`, `Subagents` or `Dynamic workflows` in `/settings` of a
  terminal attached to the daemon applies to the daemon's session at once.
  RPC: `get_state` reports `mode`, `subagents` and `workflows`, and the new
  command `set_mode` reads and sets them for a session. Rust SDK:
  `AgentSession::set_prompt_builder`, `PromptBuilder` and
  `SessionBackend::set_mode`.
- `super -p`, `--mode json`, `--mode rpc` and the editor mode keep a
  worktree the session made when they end, as they cannot ask, and say where
  it is: `-p` and the editor mode on standard error, `--mode json` and
  `--mode rpc` with a last `worktree_kept` event. The session is in the
  worktree again when it is opened again, and so is a session the daemon
  unloaded while idle.
- `/remote` no longer waits for a pending wake-up: the daemon takes it over
  from the session's file. Rust SDK: `Wakeups::park` and `unpark`.
- Python and TypeScript: `ToolName` names the ten tools a session adds
  itself in the advanced mode. `tasks` and `stop_task` (`stopTask`),
  `agents` and `agent_entries` (`agentEntries`) and `set_mode` (`setMode`)
  wrap `get_tasks`, `stop_task`, `get_agents`, `get_agent_entries` and
  `set_mode`, with types for what they return (`TaskInfo`, `AgentInfo`,
  `AgentEntries`, `ModeSettings`; `TaskInfoData`, `AgentInfoData`,
  `AgentEntriesData`, `ModeData` in Python), and the session state has
  `cwd`, `worktree`, `mode`, `subagents` and `workflows`.
- `super` with piped input or output, the line-based front end on the
  daemon, names tool calls as the full-screen terminal does (a file tool by
  its file, an `Agent` call by its description, `ScheduleWakeup` by its
  delay and reason) where it showed the new tools' arguments as JSON, and
  shows a task's notice, a monitor's event, an agent's message, a wake-up
  and a loop's round as one line each, where it showed nothing.
- While background tasks run, the terminal's footer says how many and which
  key stops them all, `● 2 background tasks running · Ctrl+X to stop`, also
  when it is attached to the daemon. One press stops every command,
  monitor, agent and workflow of the session and says how many it stopped;
  while none runs, `Ctrl+X` copies the last response as before. The key is
  the action `stopBackgroundTasks` in `~/.super/agent/keybindings.json`;
  given a key of its own, it does nothing while no task runs, and the footer
  and `/hotkeys` name that key.
- A workflow run's journal is written call by call as the run goes
  (`<run id>.journal.jsonl`), so a run cut off by the end of its process can
  be resumed with `resumeFromRunId` after a restart. Such a run is announced
  once, as stopped, with a notice when the session is opened again; the
  notice's fixed text is a placeholder until it is written. Rust SDK:
  `super_workflow::AgentRunner::journal_changed`, `JournalChange`,
  `Journal::invalidate_from` and `AgentSession::set_closing`.

### Changed

- `run_workflow` is now `Workflow`. It is offered with every request while
  the Subagents and Dynamic workflows switches are on in the advanced mode,
  not only on a turn that asks for a workflow. A run starts in the
  background as a task of kind `workflow` with the run's id, the call
  returns at once, and a notice brings the workflow's answer when it ends;
  `TaskStop` and `/tasks` stop it, and `Esc` no longer does. The question
  before a run stays. A turn that asks for a workflow still says so to the
  model. The guide to writing a script is no longer added to the prompt on
  such turns: it is the built-in skill `workflow-authoring`, loaded with
  `Skill`. The descriptions of `Workflow` and `Skill` and their parameters,
  and the fixed text around a loaded skill and the list of skills, are
  placeholders until their text is written.
- The note on a stopped workflow no longer promises that relaunching the
  same script reuses its results. A workflow stopped from `/workflows` is
  announced with a notice whose task id is the run's id for
  `resumeFromRunId` and whose summary says how far the run came; its fixed
  text is a placeholder until it is written.
- In the advanced mode the system prompt no longer lists the skills with
  their files; the list comes with the `Skill` tool. An agent's system prompt
  repeats no list of skills either.

- The six earlier agent tools, `spawn_agent`, `send_message`,
  `followup_task`, `wait_agent`, `list_agents` and `interrupt_agent`, are
  gone, and so are their paragraph in the system prompt and the Rust module
  `super_coding::subagents`. `TaskStop` comes with the agent tools as well
  as with `Bash`. The descriptions of `Agent`, `SendMessage` and
  `ListAgents`, the instructions and descriptions of the agent types and the
  system prompt's section on agents are placeholders until their text is
  written.
- A child agent's run, and a workflow agent's or a loop's, lasts until the
  child's own background commands and monitors have ended and the turns
  their notices and events start are done. A message that reaches a child
  after its last turn, before its end is recorded, belongs to that run, so
  the parent's notice carries the child's answer to it.
- An agent type whose definition file lists its `tools` gives its children
  those tools and no other: the tools a session adds itself, such as
  `SendMessage`, `Skill`, `Monitor` and `TaskStop`, come only when the list
  names them. The built-in types keep their tools.
- A subagent stored while the session worked in a folder that is gone since,
  such as a worktree it removed, loads again and resumes in the session's
  folder.
- A child session, a subagent's, a workflow agent's or a loop's, is stored
  in a folder named after its parent's session file instead of next to it,
  and the session lists leave it out. Child sessions stored next to their
  parent by earlier versions are left out of the lists too.

- The four core tools are named `Read`, `Write`, `Edit` and `Bash`. The
  model sees these names, and `defaultTools`, `--tools`, `--exclude-tools`
  and the SDKs' tool lists take them. The lower-case names `read`, `write`,
  `edit` and `bash` are no longer recognized, so a saved `defaultTools`
  entry such as `"-bash"` has to be written as `"-Bash"`. A session saved
  before this change still opens and shows its tool calls under their old
  names. `grep`, `find`, `ls`, `mcp` and `webmcp` keep their names.
- `--tools` and `--exclude-tools`, the SDKs' `tools` and `excludeTools`,
  and a `-Name` entry of `defaultTools` also choose among the tools a
  session adds itself: the agent tools, `Monitor`, `TaskStop`, `Workflow`,
  `Skill`, `ScheduleWakeup`, `EnterWorktree` and `ExitWorktree`. So
  `--tools Read` offers `Read` only, and `--exclude-tools Monitor` leaves
  `Monitor` out, for the session and the agents it starts. Rust SDK:
  `super_coding::ToolSelection`, `AgentSession::set_tool_selection` and
  `super_sdk::tools::tool_selection`.
- An `abort` from an RPC client that did not say it shows a stop control for
  each background task, a remote client through the platform among them,
  also stops the session's background tasks; so does `session/cancel` from
  an ACP client.
- `Read`, `Edit` and `Write` take an absolute `file_path` in place of `path`,
  and a relative path is refused. `Read` numbers its lines and returns 2,000
  by default; a read of a whole file is refused above 256 KB, and a read
  returns at most 25,000 tokens: a read of a whole file above that returns
  its first part, a read with `offset` or `limit` above it is refused.
  `Edit` makes one replacement per call with `old_string`, `new_string` and
  `replace_all`; the `edits` array and the `oldText` and `newText` shapes are
  gone. An empty `old_string` creates a file that does not exist yet. A
  skill named with a relative `--skill` path is listed with its absolute
  path, which `Read` takes.
- The descriptions of `Read`, `Edit` and `Write` and of their parameters are
  placeholders until their text is written.
- Gemini requests leave `exclusiveMinimum` and `exclusiveMaximum` out of tool
  schemas, which its API does not accept.
- With fish as `shellPath`, a `cd` in the main session carries over to the
  next command too, as it does for POSIX shells, and each command starts
  in the session's folder again after `config.fish` ran.
- `Edit` finds `old_string` when it differs from the file only in its quotes:
  when it is not in the file as given, curly single and double quotes count
  as their straight forms, in the file and in `old_string`. The text that is
  in the file is replaced, and where it has curly quotes the straight quotes
  of `new_string` are written curly in the same style: an apostrophe inside
  a word as a right single quote, a quote at the start or after a space as
  an opening one. Text that is in the file as given is replaced as before.
- Gemini requests give every schema in a tool's parameters a type, which its
  API requires: a parameter that takes any value, such as `Workflow`'s
  `args`, is sent as a string, so a session with dynamic workflows on works
  with Gemini models. A property named like a schema keyword, such as
  `default`, is no longer left out. Other providers get the schema as the
  tool has it.
- `Bash` takes `timeout` in milliseconds, 120,000 by default and 600,000 at
  most, where it took seconds without a default; its `cwd` parameter is gone.
  The shell starts as a login shell from the user's profile (POSIX shells
  and fish). In the main session a `cd` carries over to the next command
  while it stays inside the session folder, also past a command's own `EXIT`
  trap, and a command that removes its own folder gets no error about it
  that it did not print. A command that only sleeps is
  refused in the foreground. Up to 30,000 characters of output come back;
  longer output is saved to a file, and the agent gets its path and the
  first 2,000 bytes. An exit code other than 0 comes back as
  `Exit code N` with the output, except 1 from `grep`, `rg`, `egrep`,
  `fgrep`, `find`, `diff`, `test`, `[`, `git grep` or `git diff` as the last
  command. A Cursor model's native shell calls follow: its working folder is
  entered with `cd`, and its timeout stays in milliseconds.
- The descriptions of `Bash`, `Monitor` and `TaskStop` and of their
  parameters are placeholders until their text is written; `Bash` has one
  for each shell family.

- In the advanced mode `/loop` without an interval runs self-paced in the
  session itself: each round is a turn with the goal, the model schedules the
  next one with `ScheduleWakeup` or ends the loop, a round that leaves nothing
  running and nothing scheduled ends it, and it stops after 100 rounds at
  most (`--iterations` sets fewer). It no longer runs its iterations back to
  back in a child session there. `/loop` with an interval, `/loop` in the
  basic mode and `/autoresearch` work as before. The `/loop` command's
  description in the command list reads "Run a goal loop".
- The descriptions of `ScheduleWakeup`, `EnterWorktree` and `ExitWorktree`
  and of their parameters, the instructions a loop's round carries and the
  question about a worktree at a session's end are placeholders until their
  text is written.

### Fixed

- The tool calls of one answer run in their order where it matters: `Edit`,
  `Write` and `Bash` each run alone, after the calls before them and before
  the calls after them, so a `Read` and then a `Write` or `Edit` of the same
  file, or a `cd` and then another command, give the same result every time.
  Calls of other tools next to each other still run at the same time. Before,
  every call of an answer started at once. When a run is cancelled, each call
  it did not run is answered as aborted. `SendMessage`, `ListAgents` and
  `TaskStop` run alone too, so they find the agent an `Agent` call before them
  in the same answer started, and two messages to one agent reach it, or
  resume it, in the order of the answer.
- A conversation in which a tool call has no result, because the process
  that ran the call ended first, is sent with an aborted result for that
  call. A session or an agent cut off that way, for example by the end of
  `super -p` or of `--mode rpc`, can be resumed without the provider refusing
  the request.
- Notices, monitor events and messages from agents that wait together reach
  the agent together, in one step or one new turn. With the default steering
  mode, `one-at-a-time`, they came one a step, so the second of two commands
  that ended during a long command was told one tool round later, and three
  that ended while the agent was idle cost three model requests. The steering
  and follow-up modes pace only the messages the user queued.
- `TaskStop` and `stop_task` stop a background agent, a background skill
  among them, by its name as well as by its id: the agent's task now has the
  agent's name while no other running task has it. A workflow run that a
  `Workflow` call named with `name` is stopped by that name; its task was
  named after the script.
- `TaskStop` and `stop_task` stop a background command or a monitor by the
  `description` its call gave it, as they stop an agent by its description.
  A name that several running tasks share is refused with "Several running
  background tasks are named '…'; give the id of one of them.", and one that
  a task that ended shares with a running one stops the running one; it
  answered about the one that ended, so the running one could not be
  stopped by name.
- The full-screen terminal names a background task by its id in `/tasks`,
  its notice and a monitor's event, with its description once, as the tool
  result that started it and an agent's messages name it. Since agents'
  tasks are named by their description, `/tasks` showed the description in
  place of the id and its columns did not line up.
- `Workflow` with `name` finds the project's saved workflows in the project
  SUPER found the session's skills in, with the same trust: a project trusted
  for one run with `--approve`, and a session that works in a worktree, no
  longer miss them.
- What a login profile prints is no longer part of a `Bash` result, a
  background command's output file or a monitor's events: what the shell
  writes before the command runs is left out on macOS and Linux, and the
  profile's environment and folder change still reach the command. A job the
  profile starts with its output sent elsewhere does not hold up a command
  or a monitor until it ends. A POSIX shell runs the command with `eval`, so
  a syntax error in it comes back with the shell's message (zsh gave only the
  exit code) and the command's own line numbers. A `shellPath` that is there
  but cannot run fails with "Could not start shell …" too.
- A session opened again refuses a file it had read and that changed on disk
  since with the same words as a session that kept running, "has changed on
  disk since it was read", where it said the file had not been read yet.
- After moving back in the conversation with `/tree` or `fork` over RPC,
  `Edit` and `Write` refuse a file that was read only on the branch left
  behind, also when the session is opened again; they took it as read,
  though the conversation the model gets has no such read.
- A read of the whole file and a read of a part of it in one answer leave
  the whole read in the record, whichever ends last, so a `Write` after them
  no longer succeeds in one run and is refused in the next.
- A name in `--tools`, `--exclude-tools` or `defaultTools` that no tool has,
  such as `read` from an earlier version, is named in a warning on standard
  error when SUPER starts, with the tool's name in its new case (`read (use
  Read)`). Such a name was left out without a word, so a saved selection could
  start sessions without file tools.
- Rust SDK: `super_sdk::tools::unknown_tool_names` finds those names and
  `SESSION_TOOLS` lists the tools a session adds itself.
- The warning about names of no tool also comes from `--mode rpc` and ACP on
  standard error, from the daemon in its log for each session it starts, and
  in the full-screen terminal as a line of the transcript, where standard
  error was drawn over. It also names those in the `tools` of an agent type,
  such as `read` in `tools: read, bash`, whose agents got no tool for them.
  Rust SDK: `super_sdk::tools::tool_name_warnings` and
  `SessionOptions::tool_name_warnings`.
- An agent whose folder is gone, such as a worktree the session worked in and
  removed, goes on in the session's folder when a message resumes it in the
  same process too, not only after the session was opened again; its
  commands failed there with an error about the shell. A command whose folder
  does not exist now fails with "The folder <path> does not exist, so the
  command cannot start there." A `shellPath` that names no installed shell
  still fails with "Could not start shell …".
- Rust SDK: `ShellProcess::start` starts a shell command with an error that
  says what is missing.
- `SendMessage` to a stored agent whose type file is gone since, or whose
  model is no longer in the model list, is refused with a sentence that
  names the type or the model. The agent was resumed as a general-purpose
  agent with every tool, or on the session's model, in the middle of a
  conversation it had under its own type.
- RPC and the daemon answer a command whose fields do not fit, such as
  `get_agent_entries` without `agent`, with the command's `id` and its own
  `command`, so a client that waits for that id gets its answer; it came
  without an id as command `parse`. Rust SDK: `protocol::parse_failure`.
- `switch_session` back to a session the process left a moment before opens
  it, with its agents. Within half a second it was refused as "open in
  another SUPER process", because the process's own lock on that file had not
  moved yet. Rust SDK: `session_lock::held_by_this_process` tells such a file.
- An agent's stored session can no longer be opened or forked as a session of
  its own by its file's path: `--session`, `--fork`, `switch_session` and the
  SDK's `SessionSource::Open` and `Fork` refuse it. Opened that way it ran as a
  main session with every tool and wrote into the agent's transcript, and a
  fork copied its `subagent` entry, which hid the new session from every list.
  Rust SDK: `SessionManager::is_agent_session` and `refuse_agent_session`.
- An agent type whose `tools` names `Monitor` or `TaskStop` gives its agents
  those tools without `Bash` too, and `--tools` that names them offers them
  without `Bash`; they were left out without a word. `ListAgents` still comes
  only with `Agent`, as the README now says. Rust SDK: `ToolSelection::names`.
- An agent with `isolation: "worktree"` started while the session works in a
  worktree gets its own worktree at the top of the project, starting at the
  commit the session's worktree is at. It was made inside the session's
  worktree, so `ExitWorktree` with `remove` deleted it, with the agent's
  uncommitted work, without asking for `discard_changes`.
- `--mode rpc` sends its client the `worktree_changed` event with `missing`
  when a session opened again worked in a worktree that is gone. The session
  announced it before the client listened. Rust SDK:
  `SessionOptions::defer_stored_work` and `Session::resume_stored_work` let a
  host listen first; the RPC servers start the stored work once they do.
- `--mode rpc` asks its client before a workflow runs while
  `workflows.confirm` is on, with an `approval_request` of kind `workflow`,
  as the daemon and the terminal do. A plain RPC session started every run
  without asking, which mattered once `Workflow` was offered with every
  request.
- In ACP the client is asked with `session/request_permission` before a
  workflow run starts while `workflows.confirm` is on ("Run workflow …?",
  with `Run it` and `Cancel`), and the run waits for the answer; it started
  without asking. `super -p` and `--mode json`, which have nobody to ask,
  still start it, as the README now says.
- The question before a workflow run is withdrawn when its turn is stopped
  (`abort`, Esc) while it waits, or when the session's runs are (Dynamic
  workflows or Subagents turned off, the mode set to `basic`): clients get
  `approval_resolved` with `cancel`, the `Workflow` call ends and no run
  starts. `abort` waited for an answer, and a later `approve` started the
  run, also after the switches were turned off.
- A change of the mode or of a switch while a run goes on reaches the run's
  next request: its system prompt and tools, and a tool the change took away
  no longer runs. They changed only with the next run, so an agent could
  still start in a session set to `basic`.
- The terminal's question at quitting in a worktree names the worktree's
  folder beside `keep` and `remove`.
- When one answer makes several tool calls, the full-screen terminal shows
  each call's output and result under that call, also in a terminal attached
  to the daemon. They went to the latest unfinished call in the order they
  ended, so a background command's task id could stand under another call.
- In ACP, a notice, a monitor event or a message from an agent that comes
  while no prompt runs starts a turn, whose updates reach the client as
  `session/update` notifications, as in every other mode. It waited for the
  next prompt, whose response then carried the reply to it after the reply to
  the prompt.
- In the line front end of `super attach`, `Ctrl+C` while no turn runs stops
  the session's background tasks and says how many, and detaches only when
  none runs. It detached and left them running, and the front end had no
  other way to stop them but `/stop`.
- A terminal attached to the daemon that quits after the daemon is gone says
  so, "the SUPER daemon is gone, so session <id> does not run; `super -c`
  resumes it", where it said the session kept running in the daemon.
- The terminal's last lines when it quits ("session saved: …", "resume with:
  super -c") start on clean lines: the rest of the last frame below the
  editor is erased first, where pieces of its footer stayed beside them.

## 0.0.42 - 2026-11-27

### Fixed

- A session answers again after a request that failed before the model said
  anything, such as one the platform refused because the budget was used up.
  The failed turn stayed in the history as an assistant message without
  content, and for a model reached through the OpenAI-compatible API it was
  sent again with every later request; a provider that refuses such a message
  then failed each of them with `HTTP 400`, also after the budget was raised.
  These messages are no longer sent. The Anthropic, Google and Responses
  adapters already left them out.
- A model added while a session runs can be chosen without restarting the
  daemon. A session in the daemon kept the model catalog of the moment it was
  loaded: `/model` in an attached terminal offered a model the platform had
  added, and choosing it failed with `model not found`. A session now reads
  `models.json` again when `set_model` names a model it does not know, and
  for a `super` model the file lacks it asks the platform. The same holds
  for a session created with such a model, and for a provider added with
  `super provider add`.
- `get_available_models` answers with the catalog as it is now: `models.json`
  is read again, and a signed-in session asks the platform for the account's
  models first, at most once a minute and for at most five seconds.
- `super daemon run` hands the `platform.url` setting to token refreshes and
  model requests, which used the default platform unless
  `SUPER_PLATFORM_URL` was set.

### Changed

- Rust SDK: `Session::set_model` is `async`, because it can read the catalog
  and ask the platform. `AgentSession::registry` is a method that returns the
  current catalog, and `AgentSession::set_registry` replaces it.

## 0.0.41 - 2026-11-16

### Changed

- The SUPER platform is at `https://supersovereign.eu`, where it was at
  `https://super.flxkpe.io`. The new address is the default for sign-in, the
  models and remote control; the `platform.url` setting and
  `SUPER_PLATFORM_URL` override it as before. A stored sign-in stays valid:
  it is the same platform under a new address.

## 0.0.40 - 2026-11-13

### Changed

- The registry of machines and sessions is rows in the platform's database
  instead of the `registry` S2 stream. A signed-in `super` announces its
  machine with `PUT /api/machines/{machineId}`, sends a heartbeat every 30
  seconds and each changed session with
  `PUT /api/machines/{machineId}/sessions/{sessionId}`; the daemon sends its
  complete list in one request when its link starts. Nothing is sent again
  every six hours. `/sessions` reads the other machines with one
  `GET /api/registry`, in which the platform says which machines are online
  and which run a daemon. A session that ends with nothing said in it is
  removed from the registry; one the platform refuses is not sent again
  until it changes. This needs a platform that has these endpoints: nothing
  is written to or read from the old stream.
- A computer that runs sessions is a "machine" everywhere, where it was a
  "host" or a "device": `~/.super/agent/machine.json` with `machineId` (the
  id in an existing `host.json` is taken over once), the streams
  `machines/{machineId}/commands` and `machines/{machineId}/events`, and
  "this machine" in `/sessions`. "Device" stays for a sign-in.
- The daemon socket protocol is version 2: `hello` and `status` answer
  `machineId` and `machineName` in place of `hostId` and `hostName`. Version
  1 is not kept, so the daemon and its clients are updated together; a
  client on the other version is refused with both ranges, as before.

### Fixed

- A session on another machine shows when it was last used. A session in
  the registry carries `lastActivityAt`, the time of its newest entry (its
  file's modification time), from the daemon and from a `super` in a
  terminal alike, and `/sessions` shows it. Before, the apps and `/sessions`
  showed when the session's registry record was written, which the daemon
  did every six hours and whenever its link to the platform started, so a
  session read as last active a few hours ago at most, however long ago it
  was used.

## 0.0.39 - 2026-10-30

### Changed

- RPC `get_available_models` lists the models of providers that are
  configured in `models.json` or have a credential, the same set the TUI's
  `/model` picker and `super --list-models` offer, instead of every model the
  registry knows. The optional `search` substring and the response shape are
  unchanged; `set_model` still accepts any known model (ENG-49).

## 0.0.38 - 2026-10-30

### Added

- Signed in, `super` in a terminal attaches to the daemon's session for the
  directory instead of running the engine itself, starts a registered
  daemon on demand (`launchctl kickstart` on macOS, `systemctl --user start`
  on Linux), and leaves the session running when it detaches. A daemon on
  another protocol version is refused with both ranges and the update
  command for how this `super` was installed. `super` running in-process
  holds the session file's lock, so the daemon never writes the same file.
  `SUPER_DAEMON_SOCKET` points clients at another socket; `daemon.attach:
  false` keeps the in-process TUI. `DaemonClient` in `super-daemon` is the
  client library (ENG-38).
- `super install` runs the daemon as a systemd user service on Linux: it
  writes `~/.config/systemd/user/super-daemon.service` (`ExecStart=<super>
  daemon run`, `Restart=on-failure`), enables and starts it, and prints the
  unit and its state. `super uninstall` stops, disables and removes it, and
  `super daemon status` reports the unit. On macOS both refuse and point at
  the SUPER app, which ships the daemon (ENG-39).
- The daemon is present on the platform while signed in: host and session
  records in the registry (both with `daemon: true`, so an app recognises a
  daemon that hosts no session yet; stored sessions as `ended`), one
  mirror per hosted session, and the host-level streams
  `hosts/{hostId}/commands` and `hosts/{hostId}/events` through which an app
  starts, resumes, stops and lists sessions on a device (ENG-37).
- `super daemon run|status|stop`: a per-user daemon that hosts sessions and
  serves them to any number of local clients over `~/.super/run/daemon.sock`.
  A connection says `hello`, attaches to a session, and from then on speaks
  the RPC protocol unchanged; `list_sessions`, `create_session`, `attach`,
  `stop_session` and friends manage sessions. Idle sessions nobody watches
  are unloaded after `daemon.idleUnloadMinutes` and resume on the next
  attach. See `docs/daemon.md`.
- The running indicator is the brand diamond turning (`♦ ♢ ⬥ ⬘ ⬗ ⬙ ⬖`), the
  same sequence the SUPER app shows.
- The SUPER platform (`super.flxkpe.io`) is a built-in provider. `/login`
  lists `♦ SUPER` first and signs in with a device code; `super login super`
  does the same from the command line. The platform's models appear in
  `/model` as `super/…` and stream through its OpenAI-compatible gateway. The
  list refreshes on sign-in and once per start, in the background, and the
  saved list is used offline. `/logout super` revokes the device and removes
  the models. The `platform.url` setting and the `SUPER_PLATFORM_URL`
  variable select another platform for development. Signed out, nothing
  changes and the platform is never contacted.
- Remote control. While signed in to the SUPER platform, saved sessions are
  mirrored to the account's S2 streams, so the SUPER app and the web can
  follow and steer them. The footer shows `♦ remote`, remote input is
  attributed in the transcript, `/remote` inspects or toggles it, and
  `--no-remote` opts a session out. `super --mode rpc` mirrors as well.
  A resumed session remembers which remote commands it already ran
  (`~/.super/agent/remote/cursors`) and does not execute them again.
- RPC approvals: `approval_request` and `approval_resolved` events plus
  `get_approvals` and `answer_approval` commands let any client answer a
  workflow or project trust question.
- SDK: `Session::attach` wraps a harness session something else built, and
  `mirrored_sink` publishes its events to SDK subscribers. `Session::execute_from`
  names the surface a command came from.

### Fixed

- A session the daemon created and stopped before its first message can be
  resumed and attached to again instead of failing with `no session`: the
  daemon writes the session file's header when it creates the session, and
  a lookup by id finds a file that has only its header. A `super` run that
  ends before its first message still writes nothing.
- A platform token refresh holds the credential lock beside `auth.json`
  until the new token is written, so two `super` processes (a terminal
  session and the headless one the macOS app runs) never refresh the same
  token; the platform treats that as theft and signs the device out. A
  refresh the platform rejected is not sent again until `/login` stores a
  new token, instead of every 30 seconds from remote control. A model
  request without a usable platform credential says so (`not signed in to
  SUPER, or the sign-in expired …`) instead of `no API key for provider
  super`.
- The session title and the idle recap work with the SUPER platform's
  reasoning models. These helper requests run with thinking off and a tight
  output budget; when a model keeps reasoning anyway and spends the budget
  before answering, SUPER asks once more with a 4096-token budget instead of
  failing with "automatic recap failed", and an answer that is still empty
  is explained in one line.

## 0.0.37 - 2026-10-09

### Added

- Custom themes now work. Load a theme file with `--theme <file>`, the
  `themes` setting, or by placing JSON files in `~/.super/agent/themes/` or
  `.super/themes/`. Set `"theme"` to a theme's `name` to select it; the
  settings picker cycles through every available theme, and `/reload` picks
  up theme file changes.

### Changed

- A prompt sent from the SUPER app appears in the transcript like one typed
  here, without a `♦ prompt from …` notice. Steers and follow-ups keep their
  origin tag.
- The built-in dark theme is now Super Neo Citrus Dark (deep green base,
  lime accent) instead of Catppuccin Mocha.

## 0.0.36 - 2026-10-08

### Added

- Publish a Homebrew formula on each stable release. Install SUPER with
  `brew install chwzr/tap/super`. On Linux, the formula installs the
  static musl build, so SUPER does not require glibc. For a Homebrew
  install, `super update` stops and tells you to run
  `brew upgrade chwzr/tap/super`.

### Changed

- Publish the JavaScript SDKs on npm as `@chwzr/super-agent-sdk` and
  `@chwzr/super-agent-sdk-wasm`.

## 0.0.35 - 2026-10-06

### Changed

- Use mimalloc for memory allocation in the CLI, Python module, and Node
  module to improve performance.

## 0.0.34 - 2026-10-05

### Added

- Add native Herdr and cmux support for interactive sessions, including
  work status, decision notifications, and saved session resume commands.

### Fixed

- Answer Cursor web search and fetch permission requests instead of ending
  the turn. Return a refusal for native questions, mode changes, and plan
  requests that SUPER cannot perform so the model can continue with SUPER tools.

## 0.0.33 - 2026-10-05

### Fixed

- Select Git Bash, PowerShell 7, Windows PowerShell, or `cmd.exe` automatically
  on Windows when `shellPath` is not set. Tell the model which shell and
  command syntax to use, including Cursor-native Shell calls.

## 0.0.32 - 2026-10-05

### Fixed

- Run Cursor-native Shell requests through SUPER's enabled `bash` tool and
  return Cursor's expected shell results, including streamed completion.
  Preserve working directories and convert shell timeouts to seconds.
- Preserve integer tool arguments from Cursor so agent wait timeouts work.
- Load `SKILL.md` from the directory passed to `--skill`.

## 0.0.31 - 2026-10-05

### Added

- Add multiple accounts per provider with `/accounts` or
  `super login PROVIDER --add-account`. When the active account is rate
  limited or out of quota, SUPER resends the request with the next ready
  account before any output appears. This works for OpenAI Codex (HTTP and
  WebSocket), Anthropic, Cursor, Google, Meta, xAI, and every other provider
  with a login. The footer shows the active account when a provider has two
  or more accounts.
- Manage accounts interactively: `/accounts` opens provider, account, and
  action pickers to add, use, or remove accounts.
- Add `--no-auth` to `super provider add` and `/provider add` for proxies,
  such as CodexLB, that handle authentication themselves.
- Keep `retry-after` and Claude subscription reset headers, and the status
  and type of Responses WebSocket error frames, in provider error messages.

## 0.0.30 - 2026-10-03

### Fixed

- Accept keyboard repeat events so held arrow keys move through TUI menus
  and model pickers in terminals that use the Kitty keyboard protocol.

## 0.0.29 - 2026-10-03

### Added

- Test Cursor through TLS interception proxies with automatic and forced H2,
  forced H1 with and without ALPN, and forced H2 rejection without ALPN
  before sending input.

### Fixed

- Refresh Cursor's account model list when the TUI opens the model picker or
  selects a Cursor model, including after login in the same session. Use the
  discovered model entry for selection instead of the startup fallback list,
  and show discovery errors in the TUI. Reuse a successful discovery for the
  active session; clear the cache on Cursor login or logout.
- Replace the Cursor fallback roster after account model discovery so stale
  built-in names cannot override the account's actual model IDs. Preserve
  Cursor's declared `auto` alias.
- Resolve `cursor/auto` through Cursor's account-default model API before
  sending input, and send both model selection fields required by Cursor.
- Answer Cursor's request-context handshake with SUPER tools and system
  instructions instead of rejecting it as an unsupported native tool.
- Stop retrying permanent model and protocol errors merely because the
  message contains `request failed`. Respect Cursor's `isRetryable: false`.

## 0.0.28 - 2026-10-02

### Fixed

- Select Cursor's HTTP/2 `Run` or HTTP/1 `RunSSE` and `BidiAppend` transport
  through normal ALPN negotiation before sending input. Add
  `SUPER_CURSOR_TRANSPORT=auto|http1|http2` for an explicit transport choice.
  Do not retry submitted prompts or tool results. Remove the custom TLS ALPN
  configuration and direct certificate loader.

## 0.0.27 - 2026-10-02

### Added

- Test all built-in providers through a TLS proxy that accepts only
  HTTP/1.1 in ALPN. Check that Cursor still sends HTTP/2.

### Fixed

- Accept TLS proxies that select HTTP/1.1 through ALPN while keeping Cursor
  model runs on HTTP/2. Proxies could close the connection when SUPER offered
  only HTTP/2 during TLS setup.

## 0.0.26 - 2026-10-02

### Added

- Test all 44 built-in providers with direct TLS, a TLS interception
  proxy without ALPN, proxy bypass, and an untrusted proxy certificate.
  Check actual Responses WebSocket upgrades as well as HTTP requests.

### Fixed

- Use HTTP/2 for Cursor runs when corporate TLS proxies omit ALPN. Keep model
  discovery on `api2.cursor.sh` with HTTP/1.1 support.
- Use proxy settings and trusted root certificates for OpenAI, Codex, and
  Azure Responses WebSocket connections.
- Use the configured base URL for Google Vertex requests.

## 0.0.25 - 2026-10-02

### Fixed

- Send a current Cursor CLI version for model requests and model lists. The
  old version could cause Cursor to reject model requests after a successful
  sign-in.
- Use native absolute paths in ACP test inputs so the tests can run on Windows.

## 0.0.24 - 2026-10-01

### Added

- Sign in to Cursor from the CLI. Use `super login cursor --no-browser` on a
  server or SSH session, then open the printed URL on another computer.
- Let the model edit its conversation context with
  `--experimental-context-file`. Valid edits are saved in session checkpoints.
  The default is off.

### Fixed

- Keep complete Windows sign-in URLs. Do not treat URL query fields such as
  `uuid` as shell commands.
- Find Git Bash on Windows and use the correct arguments for Bash, PowerShell,
  and `cmd.exe` in agent tools, the SDK, and the TUI.
- Start Windows MCP servers that use `.cmd` launchers. Resolve their commands
  against the configured path and working directory.
- Expand native home paths, enable Windows terminal input, and restore the
  original console modes after normal exit or a panic.
- Use the Windows Google credential directory and `gcloud.cmd` for Google
  Application Default Credentials.

### Changed

- Use the shared provider login methods in the CLI. Replace manual UTF-8
  boundary checks and the Python mean calculation with standard functions.
- Run nextest on Windows, Linux, and macOS. Run documentation tests separately
  and add a Windows panic-cleanup regression check.

## 0.0.23 - 2026-09-30

### Added

- Sign in with a ChatGPT subscription through `super login openai`. SUPER saves
  the login and refreshes it when needed. Legacy Codex login remains available.
- Choose session tools with `defaultTools`. Replace the defaults or adjust
  them with `+name` and `-name` entries in user and trusted project settings.
- RPC prompt responses now report whether input started a turn or entered a
  queue. Steering and follow-up responses report queued input.

### Changed

- Updated the Pi baseline and model catalog to v0.99.1. Added GPT-6.1 Sol and
  Claude Sonnet 5.5, selected GPT-6.1 Sol as the Codex default, and updated
  Fireworks, Together, and OpenCode Go to Kimi K3.
- Made Jev compaction and dynamic reasoning generally available. Both remain
  opt-in through `/settings` and require TypeSafe credentials.
- Updated the README with clearer login, tool selection, and setup guidance.

### Fixed

- Reject incomplete Responses tool streams before tools run. Keep MCP error
  content and details, and preserve reasoning separately from answer text.
- Apply model sampling defaults and request overrides. Calculate OpenAI costs
  from the reported service tier.
- Stop browser login after an authorization denial. Show the ChatGPT usage
  page for subscription limits and retry temporary subscription errors.
- Accept null optional read limits, opening wrappers in file completion, and
  short `#rgb` theme colors.

## 0.0.22 - 2026-09-28

### Fixed

- Wrap long Markdown table cells across lines instead of truncating them with an ellipsis.

## 0.0.20 - 2026-09-26

### Added

- Prebuilt Python wheels for CPython 3.11–3.15, including free-threaded
  3.14t and 3.15t, on the existing Linux, macOS, and Windows targets.

### Fixed

- Publish the Rust and Node SDKs as `super-agent-sdk` to match PyPI. Rename the
  internal AI crate to `super-agent-ai` to avoid a crates.io name owned by
  another publisher, and the browser package to `super-agent-sdk-wasm`.
- Build npm packages with Bun and publish them with npm trusted publishing;
  announce the GitHub Release only after all registries accept the packages.
- Avoid a macOS linker incompatibility with newer SDKs.

Retagging does not replace the previously uploaded PyPI 0.0.20 wheels or
remove the already published PyPI and crates.io 0.0.21 packages.

## 0.0.19 - 2026-09-26

### Added

- Published the Rust SDK to crates.io, the native Node SDK to npm using Bun,
  and Python SDK wheels to PyPI using trusted publishing on release tags.
- Added local voice dictation and streaming Deepgram dictation with ElevenLabs
  playback in the interactive editor.
- Improved fresh-install and first-launch experience.

### Fixed

- Identified MCP servers and tools in TUI call titles.

## 0.0.18 - 2026-09-23

### Added

- Added Meta Model API keys and Muse subscription device login.
- Updated the Pi model catalog with Claude Opus 5.5, GPT-6 Sol and Luna, Grok
  4.7, and Meta Muse models.

### Changed

- Moved Jev reasoning selection to a checkpoint before each model generation,
  including the first response. Kept leased choices for later generations.
- Updated Pi compatibility to v0.87.1, including Grok 4.7 and Muse Spark 1.3
  provider defaults, Claude Code 2.1.280 request compatibility, and
  continuation-oriented split-turn compaction summaries.

### Fixed

- Kept the ACP `Cancelled` stop reason when a task is cancelled during Jev
  preparation.
- Rejected stale Jev choices after a model or effort change, and showed the
  saved-effort fallback when Jev could not select an effort.
- Omitted empty text parts from image-only OpenAI-compatible messages, rejected
  invalid prompt frontmatter with a warning, and skipped late cache refreshes
  that would miss the existing prompt cache.

## 0.0.17 - 2026-09-22

### Changed

- Expanded TypeSafe Jev reasoning-effort selection to every
  reasoning model and provider, using each model's supported effort levels and
  existing provider-native mappings.

## 0.0.16 - 2026-09-22

### Added

- Added TypeSafe Jev reasoning-effort selection for GPT-6 Astra,
  with bounded runtime context, reusable generation leases, early reassessment
  after tool failures or new user input, and safe fixed-effort fallback.
- Added colored conversation-pane notices when Jev changes reasoning effort,
  using the same effort colors as the TUI input border.

### Fixed

- Hid unsupported native terminal progress signals in Ghostty.

## 0.0.15 - 2026-09-20

### Added

- Added short, automatically generated session descriptions to terminal tabs
  while preserving explicit session names.

### Changed

- Polished project wording across user messages, documentation, and release
  configuration.

## 0.0.14 - 2026-09-20

### Added

- Added Databricks Unity Gateway, Azure Databricks, and Snowflake Cortex
  providers with bearer-token login, account URLs, and complete model catalogs.
- Added SUPER-branded idle and working terminal-tab titles, plus native terminal
  progress signals for supported terminal emulators.
- Updated Pi compatibility to v0.86.0 with the current provider catalog,
  offline Radius models, prompt-cache lifetime data and cost-aware warming,
  per-model compaction budgets, and a `/bug` shortcut to the GitHub issue form.

### Fixed

- Added provider session-affinity headers, unsigned-thinking replay controls,
  Gemini discrete thinking levels, Mistral reasoning effort, one-hour cache
  pricing for Anthropic and Bedrock, Cloudflare 520 retries, Azure peak-load
  retries, capped and cancellable retry waits, and safe compaction after large
  tool results.

## 0.0.13 - 2026-09-19

### Added

- Added historical cache-usage reports to `super cache-usage` and
  `/cache-usage`, with session and provider filters, fixed-scale charts,
  current TUI cache rates, and cached-token data in the Python, Node, and
  WebAssembly SDKs.

### Fixed

- Made the doctor WebSocket probe use a valid 16-byte handshake key so strict
  servers do not reject a valid endpoint.

## 0.0.12 - 2026-09-18

### Added

- Added TypeSafe Jev compaction, with TypeSafe API-key login,
  credential-gated opt-in settings, verbatim tool-history filtering, and safe
  fallback to summary compaction.

## 0.0.11 - 2026-09-16

### Added

- Added native stable ACP v1 support over standard input and output, including
  persistent sessions, model and thinking controls, cancellation, streamed
  tool updates and diffs, and session-local stdio or HTTP MCP servers.
- Added WebMCP support for discovering and calling tools exposed by open
  Chromium pages, with origin controls, bounded results, and cancellation.

## 0.0.10 - 2026-09-13

### Added

- Added `/update` to update the installed SUPER binary from the interactive
  terminal.
- Added session-local `/fast` toggle support for provider low-latency tiers.
  Supported paths include Anthropic, OpenAI, OpenAI Codex, OpenRouter, xAI,
  Amazon Bedrock, and Google Vertex AI.
- Added a native Cursor provider with direct HTTP/2 transport, browser login,
  live model discovery, image input, and SUPER tool execution.
- Documented the 41 built-in providers in the README.

### Changed

- Updated Anthropic OAuth compatibility to Claude Code protocol 2.1.258 and
  the current Pi Black request format.

## 0.0.9 - 2026-09-12

### Fixed

- Limited `/model`, scoped model selection, model cycling, and `--list-models`
  to providers with saved credentials, detected API keys, or manual entries in
  `models.json`.
- Added guidance in the `/model` selector explaining that `/login` connects
  additional providers.

## 0.0.8 - 2026-09-12

### Added

- Added `super doctor` and `super doctor --summary` with local health checks and
  reachability tables for every provider SSE, WebSocket, AWS event-stream, and
  login destination.
- Added Azure OpenAI authentication with API keys, bearer tokens, and the
  Microsoft Entra default credential chain.
- Added Responses WebSocket and cached WebSocket transport for the standard
  OpenAI and Azure OpenAI providers.

### Changed

- Made cross-agent session discovery and import faster and reduced repeated
  session-file parsing.

### Fixed

- Made the Linux installer use the musl release when the system glibc is not
  available or is too old.

## 0.0.7 - 2026-09-07

### Added

- Added native Windows ARM64 release artifacts.

### Changed

- Made loop and autoresearch jobs run until completion or an explicit stop when
  no iteration limit is given.
- Added optional compound intervals such as `15m` and `2d4h` to `/loop`.
- Reorganized the README around product value, first use, core workflows,
  integrations, and measured performance.

## 0.0.6 - 2026-09-07

### Added

- Added CLI and TUI commands to add, list, and remove OpenAI-compatible Chat
  Completions, Responses, and Codex API providers, including CodexLB.
- Added bounded `/loop` and `/autoresearch` jobs. Each job branches into a new
  session, and `/jobs` shows live progress and controls for multiple jobs.
- Added bold Markdown text, cyan underlined terminal hyperlinks, automatic
  links for bare HTTP and HTTPS URLs, and fenced-code syntax highlighting.

### Changed

- Added repeatable interactive startup and idle-memory measurements to the
  benchmark suite.
- Made Catppuccin Mocha the default dark terminal theme.
- Made `Ctrl+R` expand and collapse long tool results, as the TUI hint states.

## 0.0.5 - 2026-09-05

### Added

- Updated Pi compatibility to v0.85.1, including GPT-6 Astra, the refreshed
  model catalog, tiered model prices, persistent Claude effort, and restorable
  in-memory sessions.
- Added custom-provider request controls for vLLM priority and Responses output
  token limits.
- Added project and global resume support for SUPER, Pi, Claude Code, and OpenAI
  Codex sessions.

### Changed

- Shared the model registry across agents, indexed model updates, reduced
  transcript copies, made session writes atomic, cleaned up idle mutation locks,
  and made transient-error detection stricter.
- Hid header-only sessions and delayed session-file creation until the first
  entry.

### Fixed

- Wrap long TUI rows instead of hiding their remaining text behind an ellipsis.
- Flush terminal OpenAI Codex SSE events even when EOF has no blank separator.
- Keep skills available when Bash is the only enabled file-reading tool.
- Make the model/thinking picker default-save shortcut configurable.
- Open the skill search menu for inline `$` mentions and invoke the selected
  skill without replacing preceding prompt text.
- Made the resume picker compact and kept terminal rendering stable after a
  resumed OpenAI Codex session.

## 0.0.4 - 2026-09-04

### Added

- Added the `super-sdk` Rust crate with a high-level session builder, typed
  dispatcher, bounded streaming events, model/session controls, direct shell
  execution, and a hermetic mock provider.
- Added Python 3.11+ PyO3 bindings with async APIs, `StrEnum` options, and typed
  protocol dictionaries.
- Added N-API TypeScript bindings for Node.js, Bun, and Deno.
- Added a WebAssembly browser client for RPC WebSocket sessions.
- Added language-neutral JSONL RPC mode over stdin/stdout and WebSocket with
  Pi-compatible command and event names.
- Added a full browser WebAssembly agent with portable provider support.

### Changed

- Added SDK documentation and release checks for each language binding.
- Added SDK, RPC, and browser WebAssembly performance benchmarks with mean
  results.
- Reduced repeated terminal redraws during rapid window resizing.

### Fixed

- Added a Deno-compatible ESM entry point for the TypeScript SDK.
- Restored keyboard shortcuts, including `Ctrl+W`, Kitty and keypad Enter,
  and `Shift+Tab`.
- Fixed terminal rendering after rapid window size changes.

## 0.0.3 - 2026-09-02

### Added

- Added dynamic workflows with deterministic orchestration scripts, parallel
  child agents, approval and progress views, saved slash commands, and run
  controls.

### Changed

- Added performance coverage for workflow parsing, execution, progress
  snapshots, terminal rendering, and request preparation.

### Fixed

- Made workflow activation consistent for startup, interactive, print, JSON,
  queued, and slash-command prompts.
- Added verified workflow outcomes that cannot be replaced by model text.
- Fixed stale pause state in the workflow progress view.
- Unified child-turn cancellation, result handling, and usage accounting for
  manual subagents and workflow agents.

## 0.0.2 - 2026-09-01

### Added

- Added opt-in Codex-style subagents with controls for parallel work.
- Added `super update` and a launch notice when a newer release is available.
- Added profile-guided release builds and repeatable performance benchmarks.

### Changed

- Updated Pi compatibility to v0.84.4, including model thinking levels and
  interactive commands.
- Reduced release binary and compressed download sizes.

### Fixed

- Fixed multiline paste and Shift+Enter newline input.
- Restored the blinking block cursor in the terminal interface.
- Made profile-guided release tests portable across workspace locations.
- Fixed the Windows profile-guided release build, which collected empty
  profiles because the program did not return from main.

## 0.0.1 - 2026-08-28

- Matched Pi's interactive command menu and keyboard behavior.
- Added browser and headless provider authentication.
- Added automatic import of credentials from other coding agents.
- Added cached WebSocket transport for OpenAI-compatible response APIs.
- Added cargo-dist release archives, checksums, installers, and GitHub
  attestations for macOS, Linux, and Windows.
- Reduced release binary size with measured fat LTO, abort-on-panic, and
  macOS safe ICF settings.
- Added just commands for release checks and confirmed tag publication.
- Added the Rust coding-agent runtime, provider adapters, sessions, tools, and terminal UI.
