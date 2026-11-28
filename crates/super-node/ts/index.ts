import path from "node:path"

// The addon is deliberately loaded once. Node, Bun, and Deno's Node-compat
// layer all implement N-API and `require` native addons.
// eslint-disable-next-line @typescript-eslint/no-require-imports
const binding = require(
  path.join(__dirname, "..", "native", `${process.platform}-${process.arch}`, "super.node"),
) as NativeBinding

export type ThinkingLevel = "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
export type StreamingBehavior = "steer" | "followUp"
export type QueueMode = "all" | "one-at-a-time"
export type ToolName =
  | "Read"
  | "Write"
  | "Edit"
  | "Bash"
  | "grep"
  | "find"
  | "ls"
  | "mcp"
  // The tools a session adds itself in the advanced mode.
  | "Monitor"
  | "TaskStop"
  | "Agent"
  | "SendMessage"
  | "ListAgents"
  | "Workflow"
  | "Skill"
  | "ScheduleWakeup"
  | "EnterWorktree"
  | "ExitWorktree"
export type Mode = "basic" | "advanced"

export interface ImageInput {
  type: "image"
  data: string
  mimeType: string
}
export interface Model {
  id: string
  name: string
  api: string
  provider: string
  baseUrl: string
  reasoning: boolean
  input: string[]
  contextWindow: number
  maxTokens: number
  cost: { input: number; output: number; cacheRead: number; cacheWrite: number }
  promptCache?: { short?: number; long?: number }
}
export interface SessionOptions {
  cwd?: string
  model?: string
  provider?: string
  apiKey?: string
  modelsFile?: string
  thinkingLevel?: ThinkingLevel
  tools?: (ToolName | string)[]
  excludeTools?: string[]
  noTools?: boolean
  systemPrompt?: string
  appendSystemPrompt?: string
  session?: "in-memory" | "create" | "continue" | `open:${string}` | `fork:${string}`
  sessionDir?: string
  sessionName?: string
  trustProjectFiles?: boolean
  noContextFiles?: boolean
  eventCapacity?: number
}
export interface SessionState {
  model: Model | null
  thinkingLevel: ThinkingLevel
  isStreaming: boolean
  sessionFile: string | null
  sessionId: string
  sessionName: string | null
  messageCount: number
  tools: string[]
  steeringMode: QueueMode
  followUpMode: QueueMode
  autoCompactionEnabled: boolean
  autoRetryEnabled: boolean
  /** The session's own folder. */
  cwd: string
  /** The worktree the session works in, or `null`. */
  worktree: SessionWorktree | null
  mode: Mode
  subagents: boolean
  workflows: boolean
}
export interface SessionWorktree {
  path: string
  branch: string
  created: boolean
  original: string
}
/** The mode and the two agent switches of a session, as `setMode` answers. */
export interface ModeSettings {
  mode: Mode
  subagents: boolean
  workflows: boolean
}
export type TaskKind = "shell" | "monitor" | "agent" | "workflow"
export type TaskStatus = "running" | "completed" | "failed" | "stopped"
/** A background task: a command, a monitor, an agent or a workflow. */
export interface TaskInfo {
  id: string
  name?: string
  kind: TaskKind
  description: string
  status: TaskStatus
  toolUseId?: string
  outputFile?: string
  progress?: string
  summary?: string
  /** Milliseconds since the Unix epoch. */
  startedAt: number
  endedAt?: number
}
/** Where an agent comes from. */
export type AgentOrigin =
  | { kind: "agent" }
  | { kind: "workflow"; runId: string }
  | { kind: "job"; jobId: number; jobKind: "loop" | "autoresearch" }
export interface AgentInfo {
  id: string
  /** What `agents` and `agentEntries` take: ids joined by `/`. */
  path: string
  name: string
  type: string
  status: TaskStatus
  background: boolean
  sessionFile?: string
  worktree?: { path: string; branch: string; base: string; project: string }
  origin: AgentOrigin
}
export interface AgentEntries {
  entries: Record<string, unknown>[]
  leafId: string | null
  sessionId: string
  sessionFile: string | null
}
export interface BashResult {
  output: string
  exitCode: number | null
  cancelled: boolean
  truncated: boolean
  fullOutputPath: string | null
}
export interface TokenUsage {
  input: number
  output: number
  cacheRead: number
  cacheWrite: number
  cacheWrite1h: number
  cacheReadAvailable: boolean
  total: number
}
export interface SessionStats {
  sessionFile: string | null
  sessionId: string
  userMessages: number
  assistantMessages: number
  toolCalls: number
  toolResults: number
  totalMessages: number
  tokens: TokenUsage
  cost: number
  contextUsage: { tokens: number; contextWindow: number; percent: number }
}
export interface AssistantDelta {
  type:
    | "start"
    | "text_start"
    | "text_delta"
    | "text_end"
    | "thinking_start"
    | "thinking_delta"
    | "thinking_end"
    | "toolcall_start"
    | "toolcall_delta"
    | "toolcall_end"
    | "done"
    | "error"
  contentIndex?: number
  delta?: string
  content?: string
  id?: string
  toolName?: string
  toolCall?: Record<string, unknown>
}
export interface BaseEvent {
  type: string
}
export interface MessageUpdateEvent extends BaseEvent {
  type: "message_update"
  assistantMessageEvent: AssistantDelta
}
export interface ToolEvent extends BaseEvent {
  type: "tool_execution_start" | "tool_execution_update" | "tool_execution_end"
  toolCallId: string
  toolName: string
  args?: Record<string, unknown>
  isError?: boolean
  result?: Record<string, unknown>
  partialResult?: Record<string, unknown>
}
export interface AgentSettledEvent extends BaseEvent {
  type: "agent_settled"
}
export interface EventLagEvent extends BaseEvent {
  type: "event_lag"
  skipped: number
}
export type SuperEvent =
  | MessageUpdateEvent
  | ToolEvent
  | AgentSettledEvent
  | EventLagEvent
  | BaseEvent

export interface Command {
  type: string
  [field: string]: unknown
}
export interface Response<T = unknown> {
  type: "response"
  id?: string
  command: string
  success: boolean
  data?: T
  error?: string
}

interface NativeEventStream {
  nextJson(): Promise<string | null>
}
interface NativeSession {
  executeJson(json: string): Promise<string>
  prompt(message: string): Promise<void>
  promptDetached(message: string): void
  events(): NativeEventStream
  abort(): void
  waitIdle(): Promise<void>
  close(): void
}
interface NativeSessionConstructor {
  create(optionsJson: string): Promise<NativeSession>
}
interface NativeMockProvider {
  readonly catalogPath: string
  requestsJson(): string
  stop(): void
}
interface NativeMockProviderConstructor {
  start(directory: string, scriptJson: string): Promise<NativeMockProvider>
}
interface NativeBinding {
  NativeSession: NativeSessionConstructor
  MockProvider?: NativeMockProviderConstructor
}

class EventIterator implements AsyncIterableIterator<SuperEvent> {
  readonly #native: NativeEventStream
  constructor(native: NativeEventStream) {
    this.#native = native
  }
  [Symbol.asyncIterator](): AsyncIterableIterator<SuperEvent> {
    return this
  }
  async next(): Promise<IteratorResult<SuperEvent>> {
    const json = await this.#native.nextJson()
    if (json === null) return { done: true, value: undefined }
    return { done: false, value: JSON.parse(json) as SuperEvent }
  }
}

/** One embeddable conversation with the SUPER coding agent. */
export class Session {
  readonly #native: NativeSession
  private constructor(native: NativeSession) {
    this.#native = native
  }

  static async create(options: SessionOptions = {}): Promise<Session> {
    return new Session(await binding.NativeSession.create(JSON.stringify(options)))
  }

  /** Escape hatch: every typed method below calls this shared dispatcher. */
  async execute<T = unknown>(command: Command): Promise<Response<T>> {
    return JSON.parse(await this.#native.executeJson(JSON.stringify(command))) as Response<T>
  }
  async #require<T>(command: Command): Promise<T> {
    const response = await this.execute<T>(command)
    if (!response.success) throw new Error(response.error ?? `${command.type} failed`)
    return response.data as T
  }

  events(): AsyncIterableIterator<SuperEvent> {
    return new EventIterator(this.#native.events())
  }
  async prompt(message: string): Promise<void> {
    await this.#native.prompt(message)
  }
  promptDetached(message: string): void {
    this.#native.promptDetached(message)
  }
  abort(): void {
    this.#native.abort()
  }
  async waitIdle(): Promise<void> {
    await this.#native.waitIdle()
  }
  close(): void {
    this.#native.close()
  }
  async dispose(): Promise<void> {
    this.close()
  }
  async [Symbol.asyncDispose](): Promise<void> {
    this.close()
  }

  async steer(message: string): Promise<void> {
    await this.#require({ type: "steer", message })
  }
  async followUp(message: string): Promise<void> {
    await this.#require({ type: "follow_up", message })
  }
  async state(): Promise<SessionState> {
    return this.#require({ type: "get_state" })
  }
  async messages(): Promise<Record<string, unknown>[]> {
    return (await this.#require<{ messages: Record<string, unknown>[] }>({ type: "get_messages" }))
      .messages
  }
  async entries(
    since?: string,
  ): Promise<{ entries: Record<string, unknown>[]; leafId: string | null }> {
    return this.#require({ type: "get_entries", ...(since === undefined ? {} : { since }) })
  }
  async lastAssistantText(): Promise<string | null> {
    return (await this.#require<{ text: string | null }>({ type: "get_last_assistant_text" })).text
  }
  async sessionStats(): Promise<SessionStats> {
    return this.#require({ type: "get_session_stats" })
  }
  async tools(): Promise<string[]> {
    const data = await this.#require<{ tools: { name: string }[] }>({ type: "get_tools" })
    return data.tools.map((tool) => tool.name)
  }
  async setModel(provider: string, modelId: string): Promise<Model> {
    return this.#require({ type: "set_model", provider, modelId })
  }
  async availableModels(search?: string): Promise<Model[]> {
    const command: Command = { type: "get_available_models" }
    if (search !== undefined) command.search = search
    return (await this.#require<{ models: Model[] }>(command)).models
  }
  async setThinkingLevel(level: ThinkingLevel): Promise<void> {
    await this.#require({ type: "set_thinking_level", level })
  }
  async availableThinkingLevels(): Promise<ThinkingLevel[]> {
    return (
      await this.#require<{ levels: ThinkingLevel[] }>({ type: "get_available_thinking_levels" })
    ).levels
  }
  async setSteeringMode(mode: QueueMode): Promise<void> {
    await this.#require({ type: "set_steering_mode", mode })
  }
  async setFollowUpMode(mode: QueueMode): Promise<void> {
    await this.#require({ type: "set_follow_up_mode", mode })
  }
  async compact(customInstructions?: string): Promise<Record<string, number>> {
    const command: Command = { type: "compact" }
    if (customInstructions !== undefined) command.customInstructions = customInstructions
    return this.#require(command)
  }
  async setAutoCompaction(enabled: boolean): Promise<void> {
    await this.#require({ type: "set_auto_compaction", enabled })
  }
  async setAutoRetry(enabled: boolean): Promise<void> {
    await this.#require({ type: "set_auto_retry", enabled })
  }
  async bash(command: string): Promise<BashResult> {
    return this.#require({ type: "bash", command })
  }
  async abortBash(): Promise<void> {
    await this.#require({ type: "abort_bash" })
  }
  async ping(): Promise<boolean> {
    return (await this.#require<{ pong: boolean }>({ type: "ping" })).pong
  }

  /** Every background task the session remembers, oldest first. */
  async tasks(): Promise<TaskInfo[]> {
    return (await this.#require<{ tasks: TaskInfo[] }>({ type: "get_tasks" })).tasks
  }
  /** Stop one running background task by its id or name. */
  async stopTask(taskId: string): Promise<TaskInfo> {
    return (await this.#require<{ task: TaskInfo }>({ type: "stop_task", taskId })).task
  }
  /** The session's agents, or with `agent` (a path) the agents of that agent. */
  async agents(agent?: string): Promise<AgentInfo[]> {
    const command: Command = { type: "get_agents" }
    if (agent !== undefined) command.agent = agent
    return (await this.#require<{ agents: AgentInfo[] }>(command)).agents
  }
  /** An agent's transcript, after the entry `since` when given. */
  async agentEntries(agent: string, since?: string): Promise<AgentEntries> {
    const command: Command = { type: "get_agent_entries", agent }
    if (since !== undefined) command.since = since
    return this.#require(command)
  }
  /**
   * Set the mode and the agent switches given, for this session only, and
   * answer all three; with nothing given, only read them.
   */
  async setMode(
    settings: { mode?: Mode; subagents?: boolean; workflows?: boolean } = {},
  ): Promise<ModeSettings> {
    return this.#require({ type: "set_mode", ...settings })
  }
}

/** Test-only scripted provider. Present in development builds and omitted from slim wheels. */
export class MockProvider {
  readonly #native: NativeMockProvider
  private constructor(native: NativeMockProvider) {
    this.#native = native
  }
  static async start(directory: string, script: unknown[][]): Promise<MockProvider> {
    if (binding.MockProvider === undefined)
      throw new Error("this package was built without the mock feature")
    return new MockProvider(await binding.MockProvider.start(directory, JSON.stringify(script)))
  }
  get catalogPath(): string {
    return this.#native.catalogPath
  }
  requests(): Record<string, unknown>[] {
    return JSON.parse(this.#native.requestsJson()) as Record<string, unknown>[]
  }
  stop(): void {
    this.#native.stop()
  }
}
