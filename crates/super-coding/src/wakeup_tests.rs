//! Wake-ups and the self-paced loop, driven through whole turns with a
//! scripted provider. The tests run on Tokio's paused clock, so delays of
//! minutes pass at once and in order.

use crate::agent_tools_tests::{Seen, core_tools, lanes, last_tool_result, texts, user_text};
use crate::background_tests::{model, text, tool_call};
use crate::iterative::{JobKind, JobRecord, JobStatus};
use crate::notifications::{LOOP_ROUND, WAKEUP, looped, woken};
use crate::session::manager::SessionManager;
use crate::session_runner::AgentSession;
use crate::settings::Settings;
use crate::tools::schedule_wakeup::ScheduleWakeupTool;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use super_agent::placeholder::{prompt_todo, tool_description, tool_parameter};
use super_agent::{AgentMessage, AgentTool};
use super_ai::{AssistantMessage, Registry, ThinkingLevel};
use tokio_util::sync::CancellationToken;

struct Fixture {
    session: Arc<AgentSession>,
    seen: Arc<Seen>,
    _directory: tempfile::TempDir,
}

fn fixture(script: Vec<(&str, Vec<Option<AssistantMessage>>)>) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let settings = Settings {
        retry: crate::settings::RetrySettings {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let session = AgentSession::new(
        SessionManager::in_memory(directory.path()),
        core_tools(directory.path()),
        Registry::from_builtin(),
        settings,
        "main prompt".into(),
        model(),
        ThinkingLevel::Off,
        None,
        Arc::new(|_| {}),
    );
    let (stream, seen) = lanes(script);
    session.set_stream_fn(Some(stream));
    Fixture {
        session,
        seen,
        _directory: directory,
    }
}

/// The text a round of the loop with `goal` brings: the lane its requests
/// are answered in.
fn round_text(goal: &str) -> String {
    format!("{goal}\n\n{}", prompt_todo("Loop.self_paced_instructions"))
}

fn wake_up(id: &str, args: Value) -> Option<AssistantMessage> {
    Some(tool_call(id, "ScheduleWakeup", args))
}

/// Let the paused clock run `seconds` forward, and every task that is due
/// run with it.
async fn pass(seconds: u64) {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

async fn finished(job: &JobRecord) {
    let mut updates = job.subscribe();
    tokio::time::timeout(Duration::from_secs(36_000), async {
        while !job.snapshot().status.is_finished() {
            updates.changed().await.unwrap();
        }
    })
    .await
    .expect("the job finishes");
}

/// The custom messages of `kind` in the session's history.
fn custom(session: &AgentSession, kind: &str) -> Vec<AgentMessage> {
    session
        .manager
        .lock()
        .unwrap()
        .build_session_context()
        .messages
        .into_iter()
        .filter(
            |message| matches!(message, AgentMessage::Custom(custom) if custom.custom_type == kind),
        )
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_wake_up_that_comes_due_starts_a_turn_with_its_prompt() {
    let fixture = fixture(vec![(
        "go",
        vec![
            wake_up(
                "w1",
                json!({"delaySeconds": 120, "prompt": "check the build", "reason": "the build takes two minutes", "noop": true}),
            ),
            Some(text("waiting")),
            Some(text("the build passed")),
        ],
    )]);
    let session = &fixture.session;
    session.prompt(vec![AgentMessage::user("go")]).await;
    let requests = fixture.seen.of("go");
    assert_eq!(requests.len(), 2);
    assert_eq!(
        texts(&last_tool_result(&requests[1].context)),
        ["The wake-up comes in 120 seconds."]
    );
    let pending = session.wakeups().pending().expect("a wake-up waits");
    assert_eq!(pending.delay_seconds, 120);
    assert_eq!(pending.prompt.as_deref(), Some("check the build"));
    assert_eq!(
        pending.reason.as_deref(),
        Some("the build takes two minutes")
    );
    assert!(pending.noop);
    assert_eq!(pending.job, None);
    // It keeps the session loaded in the daemon, but print mode does not
    // wait for it.
    assert!(session.has_background_work());

    pass(119).await;
    assert_eq!(fixture.seen.count("go"), 2, "not due yet");
    pass(2).await;
    session.wait_for_background_work().await;
    let requests = fixture.seen.of("go");
    assert_eq!(requests.len(), 3);
    let last_user = requests[2]
        .context
        .messages
        .iter()
        .rev()
        .find_map(user_text)
        .unwrap();
    assert_eq!(last_user, "check the build");
    // Recorded as a wake-up, with its reason and noop for clients.
    let recorded = custom(session, WAKEUP);
    assert_eq!(recorded.len(), 1);
    let wakeup = woken(&recorded[0]).unwrap();
    assert_eq!(
        wakeup.reason.as_deref(),
        Some("the build takes two minutes")
    );
    assert!(wakeup.noop);
    assert!(session.wakeups().pending().is_none());
    assert!(!session.has_background_work());
}

#[tokio::test(start_paused = true)]
async fn the_delay_is_kept_between_a_minute_and_an_hour_and_a_new_wake_up_replaces_the_old() {
    let fixture = fixture(Vec::new());
    let session = &fixture.session;
    let tool = ScheduleWakeupTool::new(Arc::downgrade(session));
    let call = |args: Value| {
        let tool = &tool;
        async move {
            tool.execute(
                "call",
                tool.prepare_arguments(args),
                CancellationToken::new(),
                None,
            )
            .await
            .map_err(|error| format!("{error:#}"))
        }
    };
    let short = call(json!({"delaySeconds": 5, "prompt": "soon"}))
        .await
        .unwrap();
    assert_eq!(short.output_text(), "The wake-up comes in 60 seconds.");
    assert_eq!(short.details["requestedDelaySeconds"], 5.0);
    assert_eq!(short.details["wakeup"]["delaySeconds"], 60);
    let long = call(json!({"delaySeconds": "86400", "prompt": "later", "reason": null}))
        .await
        .unwrap();
    assert_eq!(long.output_text(), "The wake-up comes in 3600 seconds.");
    // The second replaced the first: one wake-up waits, the later one.
    let pending = session.wakeups().pending().unwrap();
    assert_eq!(pending.prompt.as_deref(), Some("later"));
    pass(120).await;
    assert!(
        custom(session, WAKEUP).is_empty(),
        "the first was cancelled"
    );

    assert_eq!(
        call(json!({"prompt": "no delay"})).await.unwrap_err(),
        "Give delaySeconds, the seconds until the wake-up (60 to 3600), or set stop to end the loop."
    );
    assert_eq!(
        call(json!({"delaySeconds": 60})).await.unwrap_err(),
        "Give the prompt the wake-up brings back; no loop runs in this session."
    );
    let stopped = call(json!({"stop": true, "reason": "not needed"}))
        .await
        .unwrap();
    assert_eq!(stopped.output_text(), "The pending wake-up is cancelled.");
    assert_eq!(stopped.details["reason"], "not needed");
    assert!(session.wakeups().pending().is_none());
    assert!(!session.has_background_work());
    let nothing = call(json!({"stop": "true"})).await.unwrap();
    assert_eq!(
        nothing.output_text(),
        "No loop runs and no wake-up is pending, so there was nothing to stop."
    );
    pass(4_000).await;
    assert!(custom(session, WAKEUP).is_empty());

    // The schema is the interface's, every description a placeholder.
    assert_eq!(tool.description(), tool_description("ScheduleWakeup"));
    let parameter = |name: &str| tool_parameter("ScheduleWakeup", name);
    assert_eq!(
        tool.parameters(),
        json!({
            "type": "object",
            "properties": {
                "delaySeconds": {"type": "number", "description": parameter("delaySeconds")},
                "reason": {"type": "string", "description": parameter("reason")},
                "prompt": {"type": "string", "description": parameter("prompt")},
                "stop": {"type": "boolean", "description": parameter("stop")},
                "noop": {"type": "boolean", "description": parameter("noop")},
            },
            "additionalProperties": false,
        })
    );
}

#[tokio::test(start_paused = true)]
async fn a_self_paced_loop_runs_in_the_session_while_the_model_schedules_its_rounds() {
    let goal = "tidy the docs";
    let fixture = fixture(vec![(
        &round_text(goal),
        vec![
            wake_up(
                "w1",
                json!({"delaySeconds": 300, "reason": "wait for CI", "prompt": goal}),
            ),
            Some(text("round 1 done")),
            wake_up("w2", json!({"delaySeconds": 60, "noop": true})),
            Some(text("round 2 done")),
            // Round 3 schedules nothing: the loop is over.
            Some(text("all done")),
        ],
    )]);
    let session = &fixture.session;
    let lane = round_text(goal);
    let job = session
        .iterative_jobs()
        .unwrap()
        .start_self_paced(goal, None)
        .unwrap();
    assert!(job.is_self_paced());
    let snapshot = job.snapshot();
    assert_eq!(snapshot.limit, Some(100));
    assert_eq!(snapshot.interval, None);

    pass(1).await;
    let requests = fixture.seen.of(&lane);
    assert_eq!(requests.len(), 2, "round 1 ran at once");
    assert_eq!(
        texts(&last_tool_result(&requests[1].context)),
        ["The loop continues with round 2 in 300 seconds."]
    );
    assert_eq!(job.snapshot().iteration, 1);
    assert_eq!(
        session.wakeups().running_loop(),
        Some((job.snapshot().id, 1, 100))
    );
    pass(290).await;
    assert_eq!(
        fixture.seen.count(&lane),
        2,
        "round 2 waits for its wake-up"
    );
    pass(20).await;
    assert_eq!(fixture.seen.count(&lane), 4, "round 2 ran");
    assert_eq!(job.snapshot().iteration, 2);
    pass(61).await;
    finished(&job).await;

    let snapshot = job.snapshot();
    assert_eq!(snapshot.status, JobStatus::Completed);
    assert_eq!(snapshot.iteration, 3);
    assert_eq!(snapshot.latest_result.as_deref(), Some("all done"));
    assert_eq!(fixture.seen.count(&lane), 5);
    // Every round is a turn of the session with the loop's prompt.
    let rounds = custom(session, LOOP_ROUND);
    assert_eq!(rounds.len(), 3);
    let (round, max_rounds, prompt, woken_by) = looped(&rounds[1]).unwrap();
    assert_eq!((round, max_rounds, prompt.as_str()), (2, 100, goal));
    assert_eq!(woken_by.unwrap().reason.as_deref(), Some("wait for CI"));
    assert!(looped(&rounds[0]).unwrap().3.is_none());
    assert!(looped(&rounds[2]).unwrap().3.unwrap().noop);
    let round_message = fixture.seen.of(&lane)[2]
        .context
        .messages
        .iter()
        .rev()
        .find_map(user_text)
        .unwrap();
    assert_eq!(round_message, lane);
    assert_eq!(session.wakeups().running_loop(), None);
    assert!(session.wakeups().pending().is_none());
    assert!(!session.has_background_work());
}

#[tokio::test(start_paused = true)]
async fn stop_ends_the_loop_and_cancels_its_pending_wake_up() {
    let goal = "watch the deploy";
    let fixture = fixture(vec![(
        &round_text(goal),
        vec![
            wake_up("w1", json!({"delaySeconds": 600})),
            Some(text("deploy still running")),
            // The user asks in between; the model ends the loop.
            wake_up("w2", json!({"stop": true, "reason": "the user took over"})),
            Some(text("stopped")),
        ],
    )]);
    let session = &fixture.session;
    let lane = round_text(goal);
    let job = session
        .iterative_jobs()
        .unwrap()
        .start_self_paced(goal, None)
        .unwrap();
    pass(1).await;
    assert!(session.wakeups().pending().is_some());
    session
        .prompt(vec![AgentMessage::user("stop watching")])
        .await;
    let requests = fixture.seen.of(&lane);
    assert_eq!(requests.len(), 4);
    assert_eq!(
        texts(&last_tool_result(&requests[3].context)),
        ["The loop ends after round 1."]
    );
    finished(&job).await;
    assert_eq!(job.snapshot().status, JobStatus::Completed);
    assert_eq!(job.snapshot().iteration, 1);
    assert!(session.wakeups().pending().is_none());
    pass(700).await;
    assert_eq!(fixture.seen.count(&lane), 4, "no round after the stop");
    assert!(!session.has_background_work());
}

#[tokio::test(start_paused = true)]
async fn a_self_paced_loop_stops_after_100_rounds() {
    let goal = "keep going";
    let mut replies = Vec::new();
    for round in 1..=100 {
        replies.push(wake_up(&format!("w{round}"), json!({"delaySeconds": 60})));
        replies.push(Some(text(&format!("round {round}"))));
    }
    let fixture = fixture(vec![(&round_text(goal), replies)]);
    let session = &fixture.session;
    let lane = round_text(goal);
    let job = session
        .iterative_jobs()
        .unwrap()
        .start_self_paced(goal, Some(1_000))
        .unwrap();
    assert_eq!(job.snapshot().limit, Some(100), "100 at most");
    finished(&job).await;
    let snapshot = job.snapshot();
    assert_eq!(snapshot.status, JobStatus::Completed);
    assert_eq!(snapshot.iteration, 100);
    let requests = fixture.seen.of(&lane);
    assert_eq!(requests.len(), 200);
    assert_eq!(
        texts(&last_tool_result(&requests[199].context)),
        ["Round 100 of 100 was the loop's last, the most it may run, so the loop ends here."]
    );
    assert_eq!(custom(session, LOOP_ROUND).len(), 100);
    assert!(session.wakeups().pending().is_none());
    pass(120).await;
    assert_eq!(fixture.seen.count(&lane), 200);

    // A smaller cap is kept too, and the job view's stop ends a loop.
    let fixture = fixture_with_rounds("two rounds", 3);
    let job = fixture
        .session
        .iterative_jobs()
        .unwrap()
        .start_self_paced("two rounds", Some(2))
        .unwrap();
    finished(&job).await;
    assert_eq!(job.snapshot().iteration, 2);
    assert_eq!(fixture.seen.count(&round_text("two rounds")), 4);
}

/// A fixture whose model schedules a one-minute wake-up in each of `rounds`
/// rounds of the loop with `goal`.
fn fixture_with_rounds(goal: &str, rounds: usize) -> Fixture {
    let mut replies = Vec::new();
    for round in 1..=rounds {
        replies.push(wake_up(&format!("w{round}"), json!({"delaySeconds": 60})));
        replies.push(Some(text(&format!("round {round}"))));
    }
    fixture(vec![(&round_text(goal), replies)])
}

#[tokio::test(start_paused = true)]
async fn the_job_view_stops_a_self_paced_loop_and_a_session_runs_one_at_a_time() {
    let fixture = fixture_with_rounds("forever", 5);
    let session = &fixture.session;
    let runtime = session.iterative_jobs().unwrap();
    let job = runtime.start_self_paced("forever", None).unwrap();
    pass(1).await;
    let Err(error) = runtime.start_self_paced("another", None) else {
        panic!("a second loop started");
    };
    assert_eq!(
        error.to_string(),
        "a self-paced loop runs in this session already"
    );
    job.stop();
    finished(&job).await;
    assert_eq!(job.snapshot().status, JobStatus::Stopped);
    assert!(session.wakeups().pending().is_none());
    assert_eq!(session.wakeups().running_loop(), None);
    pass(120).await;
    assert_eq!(fixture.seen.count(&round_text("forever")), 2);
    // Another one may run now.
    let next = runtime.start_self_paced("forever", Some(1)).unwrap();
    finished(&next).await;
    assert_eq!(next.snapshot().status, JobStatus::Completed);
}

#[tokio::test(start_paused = true)]
async fn the_loop_with_an_interval_still_runs_in_a_child_session() {
    let goal = "check the queue";
    let child_lane = "parent context";
    let fixture = fixture(vec![(
        child_lane,
        vec![
            Some(text("first look\n[continue]")),
            Some(text("second look\n[continue]")),
            Some(text("third look\n[continue]")),
        ],
    )]);
    let session = &fixture.session;
    session
        .manager
        .lock()
        .unwrap()
        .append_message(AgentMessage::user(child_lane))
        .unwrap();
    let job = session
        .iterative_jobs()
        .unwrap()
        .start(JobKind::Loop, goal, None, Some(Duration::from_secs(600)))
        .unwrap();
    assert!(!job.is_self_paced());
    pass(1).await;
    assert_eq!(fixture.seen.count(child_lane), 1);
    pass(599).await;
    pass(1).await;
    assert_eq!(fixture.seen.count(child_lane), 2, "every interval");
    job.stop();
    finished(&job).await;
    assert_eq!(job.snapshot().status, JobStatus::Stopped);
    assert_eq!(job.snapshot().iteration, 2);
    // Nothing of it reached the session itself or its wake-ups.
    assert!(custom(session, LOOP_ROUND).is_empty());
    assert_eq!(session.wakeups().running_loop(), None);
    assert!(session.wakeups().pending().is_none());
    let request = &fixture.seen.of(child_lane)[1];
    let iteration = request
        .context
        .messages
        .iter()
        .rev()
        .find_map(user_text)
        .unwrap();
    assert!(
        iteration.contains("iteration 2 for this goal"),
        "{iteration}"
    );
    assert!(!iteration.contains(&prompt_todo("Loop.self_paced_instructions")));
}

#[tokio::test(start_paused = true)]
async fn a_round_lasts_while_its_background_work_runs_and_its_notice_can_continue_the_loop() {
    let goal = "build and check";
    let fixture = fixture(vec![(
        &round_text(goal),
        vec![
            // Round 1 ends its turn without a wake-up while a build runs.
            Some(text("the build runs")),
            // The build's notice starts a turn, which continues the loop.
            wake_up("w1", json!({"delaySeconds": 60})),
            Some(text("built; checking again soon")),
            // Round 2 leaves nothing running and schedules nothing.
            Some(text("checked")),
        ],
    )]);
    let session = &fixture.session;
    let lane = round_text(goal);
    let build = session
        .background_tasks()
        .register(crate::tasks::TaskSpec::new(
            crate::tasks::TaskKind::Shell,
            "cargo build",
        ))
        .unwrap();
    let job = session
        .iterative_jobs()
        .unwrap()
        .start_self_paced(goal, None)
        .unwrap();
    pass(1).await;
    assert_eq!(fixture.seen.count(&lane), 1);
    pass(600).await;
    assert!(
        !job.snapshot().status.is_finished(),
        "the round lasts while the build runs"
    );
    build.finish(crate::tasks::TaskOutcome::completed(
        "cargo build (exit code 0)",
    ));
    pass(1).await;
    assert_eq!(fixture.seen.count(&lane), 3, "the notice's turn ran");
    pass(60).await;
    finished(&job).await;
    assert_eq!(job.snapshot().status, JobStatus::Completed);
    assert_eq!(job.snapshot().iteration, 2);
    assert_eq!(fixture.seen.count(&lane), 4);
}

/// Two `ScheduleWakeup` calls of one answer take effect in its order, every
/// time: a stop after a wake-up leaves none pending, and the second of two
/// wake-ups is the one that waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_wake_up_calls_of_one_answer_take_effect_in_its_order() {
    let answer = |second: Value| {
        let mut answer = tool_call(
            "w1",
            "ScheduleWakeup",
            json!({"delaySeconds": 60, "prompt": "WAKE-ONE"}),
        );
        answer
            .content
            .push(super_ai::ContentBlock::ToolCall(super_ai::ToolCall {
                id: "w2".into(),
                name: "ScheduleWakeup".into(),
                arguments: second,
                thought_signature: None,
            }));
        answer
    };
    for _ in 0..10 {
        let fixture = fixture(vec![(
            "go",
            vec![
                Some(answer(json!({"stop": true}))),
                Some(text("stopped")),
                Some(answer(json!({"delaySeconds": 120, "prompt": "WAKE-TWO"}))),
                Some(text("replaced")),
            ],
        )]);
        let session = &fixture.session;
        session.prompt(vec![AgentMessage::user("go")]).await;
        assert!(session.wakeups().pending().is_none(), "the stop came second");
        session.prompt(vec![AgentMessage::user("go")]).await;
        assert_eq!(
            session.wakeups().pending().unwrap().prompt.as_deref(),
            Some("WAKE-TWO")
        );
        session.wakeups().stop();
    }
}
