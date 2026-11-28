"""Typed Python representations of the shared SUPER wire protocol."""

from __future__ import annotations

from enum import StrEnum
from typing import Any, Required, TypedDict


class ThinkingLevel(StrEnum):
    OFF = 'off'
    MINIMAL = 'minimal'
    LOW = 'low'
    MEDIUM = 'medium'
    HIGH = 'high'
    XHIGH = 'xhigh'
    MAX = 'max'


class StreamingBehavior(StrEnum):
    STEER = 'steer'
    FOLLOW_UP = 'followUp'


class ToolName(StrEnum):
    READ = 'Read'
    WRITE = 'Write'
    EDIT = 'Edit'
    BASH = 'Bash'
    GREP = 'grep'
    FIND = 'find'
    LS = 'ls'
    MCP = 'mcp'
    # The tools a session adds itself in the advanced mode.
    MONITOR = 'Monitor'
    TASK_STOP = 'TaskStop'
    AGENT = 'Agent'
    SEND_MESSAGE = 'SendMessage'
    LIST_AGENTS = 'ListAgents'
    WORKFLOW = 'Workflow'
    SKILL = 'Skill'
    SCHEDULE_WAKEUP = 'ScheduleWakeup'
    ENTER_WORKTREE = 'EnterWorktree'
    EXIT_WORKTREE = 'ExitWorktree'


class Mode(StrEnum):
    BASIC = 'basic'
    ADVANCED = 'advanced'


class TaskKind(StrEnum):
    SHELL = 'shell'
    MONITOR = 'monitor'
    AGENT = 'agent'
    WORKFLOW = 'workflow'


class TaskStatus(StrEnum):
    RUNNING = 'running'
    COMPLETED = 'completed'
    FAILED = 'failed'
    STOPPED = 'stopped'


class QueueMode(StrEnum):
    ALL = 'all'
    ONE_AT_A_TIME = 'one-at-a-time'


class ImageInput(TypedDict):
    type: Required[str]
    data: Required[str]
    mimeType: Required[str]


class PromptCacheData(TypedDict, total=False):
    short: int
    long: int


class ModelData(TypedDict, total=False):
    id: Required[str]
    name: Required[str]
    api: Required[str]
    provider: Required[str]
    baseUrl: Required[str]
    reasoning: Required[bool]
    input: Required[list[str]]
    contextWindow: Required[int]
    maxTokens: Required[int]
    cost: Required[dict[str, float]]
    promptCache: PromptCacheData


class MessageData(TypedDict, total=False):
    role: Required[str]
    content: Any
    timestamp: int
    command: str
    output: str
    exitCode: int | None
    cancelled: bool
    truncated: bool


class AssistantMessageEvent(TypedDict, total=False):
    type: Required[str]
    contentIndex: int
    delta: str
    content: str
    id: str
    toolName: str
    toolCall: dict[str, Any]


class EventData(TypedDict, total=False):
    type: Required[str]
    assistantMessageEvent: AssistantMessageEvent
    message: MessageData
    messages: list[MessageData]
    toolCallId: str
    toolName: str
    args: dict[str, Any]
    result: dict[str, Any]
    partialResult: dict[str, Any]
    isError: bool
    steering: list[str]
    followUp: list[str]
    skipped: int
    delta: str
    id: str


class CommandData(TypedDict, total=False):
    type: Required[str]
    message: str
    images: list[ImageInput]
    streamingBehavior: StreamingBehavior | str
    provider: str
    modelId: str
    level: ThinkingLevel | str
    mode: QueueMode | str
    enabled: bool
    command: str
    search: str | None
    since: str | None
    name: str
    customInstructions: str | None
    sessionPath: str
    entryId: str
    outputPath: str | None
    taskId: str
    agent: str | None
    subagents: bool | None
    workflows: bool | None


class ResponseData(TypedDict, total=False):
    type: Required[str]
    id: str
    command: Required[str]
    success: Required[bool]
    data: Any
    error: str


class SessionStateData(TypedDict, total=False):
    model: ModelData | None
    thinkingLevel: ThinkingLevel
    isStreaming: bool
    sessionFile: str | None
    sessionId: str
    sessionName: str | None
    messageCount: int
    tools: list[str]
    steeringMode: QueueMode
    followUpMode: QueueMode
    autoCompactionEnabled: bool
    autoRetryEnabled: bool
    cwd: str
    worktree: WorktreeData | None
    mode: Mode
    subagents: bool
    workflows: bool


class WorktreeData(TypedDict):
    """The worktree a session works in, as ``get_state`` reports it."""

    path: str
    branch: str
    created: bool
    original: str


class ModeData(TypedDict):
    """The mode and the two agent switches, as ``set_mode`` answers."""

    mode: Mode
    subagents: bool
    workflows: bool


class TaskInfoData(TypedDict, total=False):
    """A background task: a command, a monitor, an agent or a workflow."""

    id: Required[str]
    name: str
    kind: Required[TaskKind]
    description: Required[str]
    status: Required[TaskStatus]
    toolUseId: str
    outputFile: str
    progress: str
    summary: str
    startedAt: Required[int]
    endedAt: int


class AgentOriginData(TypedDict, total=False):
    """Where an agent comes from: ``kind`` is ``agent``, ``workflow`` (with
    ``runId``) or ``job`` (with ``jobId`` and ``jobKind``)."""

    kind: Required[str]
    runId: str
    jobId: int
    jobKind: str


class AgentInfoData(TypedDict, total=False):
    id: Required[str]
    path: Required[str]
    name: Required[str]
    type: Required[str]
    status: Required[TaskStatus]
    background: Required[bool]
    sessionFile: str
    worktree: dict[str, str]
    origin: Required[AgentOriginData]


class AgentEntriesData(TypedDict):
    entries: list[dict[str, Any]]
    leafId: str | None
    sessionId: str
    sessionFile: str | None


class BashResultData(TypedDict, total=False):
    output: str
    exitCode: int | None
    cancelled: bool
    truncated: bool
    fullOutputPath: str | None


class EntriesData(TypedDict):
    entries: list[dict[str, Any]]
    leafId: str | None


class TreeData(TypedDict):
    tree: list[dict[str, Any]]
    leafId: str | None


class TokenUsageData(TypedDict):
    input: int
    output: int
    cacheRead: int
    cacheWrite: int
    cacheWrite1h: int
    cacheReadAvailable: bool
    total: int


class SessionStatsData(TypedDict, total=False):
    sessionFile: str | None
    sessionId: str
    userMessages: int
    assistantMessages: int
    toolCalls: int
    toolResults: int
    totalMessages: int
    tokens: TokenUsageData
    cost: float
    contextUsage: dict[str, int]
