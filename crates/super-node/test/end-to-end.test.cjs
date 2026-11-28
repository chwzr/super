const test = require("node:test")
const assert = require("node:assert/strict")
const { mkdtemp, readFile } = require("node:fs/promises")
const { tmpdir } = require("node:os")
const { basename, join } = require("node:path")
const { MockProvider, Session } = require("../dist/index.js")

// `script` is the mock provider's script, or a function of the session's
// folder that returns it.
async function fixture(script) {
  const directory = await mkdtemp(join(tmpdir(), "super-node-"))
  const turns = typeof script === "function" ? script(directory) : script
  const provider = await MockProvider.start(directory, turns)
  const session = await Session.create({
    cwd: directory,
    model: "mock/mock-1",
    modelsFile: provider.catalogPath,
    thinkingLevel: "off",
    // An allowlist names every tool it offers: the four default tools and
    // the six that come with them in the advanced mode.
    tools: [
      "Read",
      "Write",
      "Edit",
      "Bash",
      "Monitor",
      "TaskStop",
      "Skill",
      "ScheduleWakeup",
      "EnterWorktree",
      "ExitWorktree",
    ],
    noContextFiles: true,
  })
  return { directory, provider, session }
}

test("a prompt streams and writes a real file", async () => {
  const { directory, provider, session } = await fixture((folder) => [
    [
      {
        toolCall: {
          id: "call_1",
          name: "Write",
          arguments: {
            file_path: join(folder, "hello.txt"),
            content: "hello from node\n",
          },
        },
      },
    ],
    [{ text: "Done." }],
  ])
  const events = session.events()
  const seen = []
  const collector = (async () => {
    for await (const event of events) {
      seen.push(event)
      if (event.type === "agent_settled") return
    }
  })()
  await session.prompt("create hello.txt")
  await collector
  assert.equal(await readFile(join(directory, "hello.txt"), "utf8"), "hello from node\n")
  assert.ok(
    seen.some((event) => event.type === "tool_execution_start" && event.toolName === "Write"),
  )
  assert.ok(seen.some((event) => event.type === "tool_execution_end" && event.isError === false))
  const text = seen
    .filter(
      (event) =>
        event.type === "message_update" && event.assistantMessageEvent.type === "text_delta",
    )
    .map((event) => event.assistantMessageEvent.delta ?? "")
    .join("")
  assert.equal(text, "Done.")
  assert.equal(await session.lastAssistantText(), "Done.")
  assert.equal(provider.requests().length, 2)
  const stats = await session.sessionStats()
  assert.equal(stats.tokens.cacheRead, 8)
  assert.equal(stats.tokens.cacheWrite, 0)
  assert.equal(stats.tokens.cacheReadAvailable, true)
  session.close()
})

test("state, models, tools, and ping use typed helpers", async () => {
  const { session } = await fixture([[{ text: "hello" }]])
  const state = await session.state()
  assert.equal(state.model.provider, "mock")
  assert.equal(state.thinkingLevel, "off")
  // In the advanced mode, the default, Monitor and TaskStop come with Bash,
  // and Skill, ScheduleWakeup and the worktree tools with Read and Bash.
  const tools = [
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Monitor",
    "TaskStop",
    "Skill",
    "ScheduleWakeup",
    "EnterWorktree",
    "ExitWorktree",
  ]
  assert.deepEqual(state.tools, tools)
  assert.equal(await session.ping(), true)
  assert.deepEqual(await session.tools(), tools)
  assert.deepEqual(
    (await session.availableModels("mock")).map((model) => model.id),
    ["mock-1"],
  )
  assert.deepEqual(await session.availableThinkingLevels(), ["off"])
  session.close()
})

test("direct bash returns a nonzero exit and enters history", async () => {
  const { session } = await fixture([[{ text: "hello" }]])
  const result = await session.bash("printf node-bash; exit 7")
  assert.equal(result.output, "node-bash")
  assert.equal(result.exitCode, 7)
  const messages = await session.messages()
  assert.equal(messages.at(-1).role, "bashExecution")
  session.close()
})

test("raw execute and clean failures share the RPC response shape", async () => {
  const { session } = await fixture([[{ text: "hello" }]])
  assert.deepEqual(await session.execute({ type: "ping" }), {
    type: "response",
    command: "ping",
    success: true,
    data: { pong: true },
  })
  await assert.rejects(session.setModel("nope", "nope"), /nope\/nope/)
  await assert.rejects(session.execute({ type: "teleport" }), /unknown variant|invalid/i)
  session.close()
})

test("background tasks, agents, and the mode use typed helpers", async () => {
  const { directory, session } = await fixture([
    [
      {
        toolCall: {
          id: "call_bg",
          name: "Bash",
          arguments: { command: "sleep 30; echo late", run_in_background: true, description: "Wait" },
        },
      },
    ],
    [{ text: "It runs." }],
  ])
  await session.prompt("start a command")
  const [task, ...others] = await session.tasks()
  assert.deepEqual(others, [])
  assert.equal(task.kind, "shell")
  assert.equal(task.status, "running")
  assert.equal(task.description, "Wait")
  assert.equal(task.toolUseId, "call_bg")
  const stopped = await session.stopTask(task.id)
  assert.equal(stopped.id, task.id)
  assert.equal((await session.tasks())[0].status, "stopped")
  await assert.rejects(session.stopTask("nope"), /No background task has the id or name 'nope'/)

  assert.deepEqual(await session.agents(), [])
  await assert.rejects(session.agentEntries("a12345678"))

  const read = await session.setMode()
  assert.equal(read.mode, "advanced")
  assert.equal(typeof read.subagents, "boolean")
  assert.deepEqual(await session.setMode({ mode: "basic" }), { ...read, mode: "basic" })
  assert.deepEqual(await session.tools(), ["Read", "Write", "Edit", "Bash"])
  const state = await session.state()
  assert.equal(state.mode, "basic")
  assert.equal(state.subagents, read.subagents)
  assert.equal(basename(state.cwd), basename(directory))
  assert.equal(state.worktree, null)
  await assert.rejects(session.setMode({ mode: "expert" }), /mode 'expert' is not basic or advanced/)
  session.close()
})
