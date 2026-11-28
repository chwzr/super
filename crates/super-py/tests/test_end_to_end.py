from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any

import super_sdk
import pytest
from super_sdk._super import MockProvider


async def make_session(
    tmp_path: Path, script: list[list[dict[str, Any]]]
) -> tuple[Any, super_sdk.Session]:
    provider = await MockProvider.start(str(tmp_path), script)
    session = await super_sdk.Session.create(
        cwd=str(tmp_path),
        model='mock/mock-1',
        models_file=provider.catalog_path,
        thinking_level=super_sdk.ThinkingLevel.OFF,
        # An allowlist names every tool it offers: the four default tools
        # and the six that come with them in the advanced mode.
        tools=[
            'Read',
            'Write',
            'Edit',
            'Bash',
            'Monitor',
            'TaskStop',
            'Skill',
            'ScheduleWakeup',
            'EnterWorktree',
            'ExitWorktree',
        ],
        no_context_files=True,
    )
    return provider, session


@pytest.mark.asyncio
async def test_prompt_streams_and_writes_a_real_file(tmp_path: Path) -> None:
    provider, session = await make_session(
        tmp_path,
        [
            [
                {
                    'toolCall': {
                        'id': 'call_1',
                        'name': 'Write',
                        'arguments': {
                            'file_path': str(tmp_path / 'hello.txt'),
                            'content': 'hello from python\n',
                        },
                    }
                }
            ],
            [{'text': 'Done.'}],
        ],
    )
    events = session.events()
    seen: list[super_sdk.Event] = []

    async def collect() -> None:
        async for event in events:
            seen.append(event)
            if event.type == 'agent_settled':
                return

    collector = asyncio.create_task(collect())
    await session.prompt('create hello.txt')
    await asyncio.wait_for(collector, timeout=5)

    assert (tmp_path / 'hello.txt').read_text() == 'hello from python\n'
    assert any(
        e.type == 'tool_execution_start' and e['toolName'] == 'Write' for e in seen
    )
    assert any(e.type == 'tool_execution_end' and e['isError'] is False for e in seen)
    streamed = ''.join(
        e['assistantMessageEvent'].get('delta', '')
        for e in seen
        if e.type == 'message_update'
        and e['assistantMessageEvent']['type'] == 'text_delta'
    )
    assert streamed == 'Done.'
    assert await session.last_assistant_text() == 'Done.'
    assert len(provider.requests()) == 2
    stats = await session.session_stats()
    assert stats['tokens']['cacheRead'] == 8
    assert stats['tokens']['cacheWrite'] == 0
    assert stats['tokens']['cacheReadAvailable'] is True
    await session.aclose()


@pytest.mark.asyncio
async def test_state_models_tools_and_ping_are_typed(tmp_path: Path) -> None:
    _provider, session = await make_session(tmp_path, [[{'text': 'hello'}]])
    state = await session.state()
    assert state.model is not None
    assert state.model['provider'] == 'mock'
    assert state.thinking_level == 'off'
    # In the advanced mode, the default, Monitor and TaskStop come with Bash,
    # and Skill, ScheduleWakeup and the worktree tools with Read and Bash.
    tools = [
        'Read', 'Write', 'Edit', 'Bash', 'Monitor', 'TaskStop', 'Skill',
        'ScheduleWakeup', 'EnterWorktree', 'ExitWorktree',
    ]
    assert state.tools == tools
    assert await session.ping() is True
    assert await session.tools() == tools
    models = await session.available_models('mock')
    assert [model['id'] for model in models] == ['mock-1']
    assert await session.available_thinking_levels() == ['off']
    await session.aclose()


@pytest.mark.asyncio
async def test_direct_bash_reports_nonzero_exit_and_enters_history(
    tmp_path: Path,
) -> None:
    _provider, session = await make_session(tmp_path, [[{'text': 'hello'}]])
    result = await session.bash('printf python-bash; exit 7')
    assert result.output == 'python-bash'
    assert result.exit_code == 7
    assert result.cancelled is False
    messages = await session.messages()
    assert messages[-1]['role'] == 'bashExecution'
    assert messages[-1]['command'] == 'printf python-bash; exit 7'
    await session.aclose()


@pytest.mark.asyncio
async def test_raw_execute_uses_the_same_protocol_and_failures_raise(
    tmp_path: Path,
) -> None:
    _provider, session = await make_session(tmp_path, [[{'text': 'hello'}]])
    response = await session.execute({'type': 'ping'})
    assert response == {
        'type': 'response',
        'command': 'ping',
        'success': True,
        'data': {'pong': True},
    }
    with pytest.raises(super_sdk.SuperError, match='nope/nope'):
        await session.set_model('nope', 'nope')
    with pytest.raises(ValueError, match='invalid command'):
        await session.execute({'type': 'teleport'})
    await session.aclose()


@pytest.mark.asyncio
async def test_background_tasks_agents_and_the_mode_are_typed(tmp_path: Path) -> None:
    _provider, session = await make_session(
        tmp_path,
        [
            [
                {
                    'toolCall': {
                        'id': 'call_bg',
                        'name': 'Bash',
                        'arguments': {
                            'command': 'sleep 30; echo late',
                            'run_in_background': True,
                            'description': 'Wait',
                        },
                    }
                }
            ],
            [{'text': 'It runs.'}],
        ],
    )
    await session.prompt('start a command')
    [task] = await session.tasks()
    assert task['kind'] == super_sdk.TaskKind.SHELL
    assert task['status'] == super_sdk.TaskStatus.RUNNING
    assert task['description'] == 'Wait'
    assert task['toolUseId'] == 'call_bg'
    stopped = await session.stop_task(task['id'])
    assert stopped['id'] == task['id']
    assert (await session.tasks())[0]['status'] == 'stopped'
    with pytest.raises(super_sdk.SuperError, match="No background task has the id or name 'nope'"):
        await session.stop_task('nope')

    assert await session.agents() == []
    with pytest.raises(super_sdk.SuperError):
        await session.agent_entries('a12345678')

    read = await session.set_mode()
    assert read['mode'] == super_sdk.Mode.ADVANCED
    assert isinstance(read['subagents'], bool)
    assert await session.set_mode(super_sdk.Mode.BASIC) == {**read, 'mode': 'basic'}
    assert await session.tools() == ['Read', 'Write', 'Edit', 'Bash']
    state = await session.state()
    assert state.mode == super_sdk.Mode.BASIC
    assert state.subagents == read['subagents']
    assert Path(state.cwd).name == tmp_path.name
    assert state.worktree is None
    with pytest.raises(super_sdk.SuperError, match="mode 'expert' is not basic or advanced"):
        await session.set_mode('expert')
    assert {tool.value for tool in super_sdk.ToolName} >= {
        'Monitor', 'TaskStop', 'Agent', 'SendMessage', 'ListAgents', 'Workflow',
        'Skill', 'ScheduleWakeup', 'EnterWorktree', 'ExitWorktree',
    }
    await session.aclose()
