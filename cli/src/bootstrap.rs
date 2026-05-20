use std::sync::Arc;

use crate::config::load_config;
use crate::conversation::message_queue::MessageQueue;

pub async fn run() {
    let config = load_config();

    if config.openrouter_api_key.is_none() {
        eprintln!("Not logged in. Run 'super login' first.");
        return;
    }

    let store = Arc::new(crate::state::store::Store::new());
    // Seed the store's provider and model_class from config.
    {
        let provider = config.provider.clone();
        let model_class = config.model_class.clone();
        store.set_state(|s| {
            s.provider = provider;
            s.model_class = model_class;
        });
    }

    // Create the cron jobs map and wake channel
    let cron_jobs: Arc<
        std::sync::Mutex<
            std::collections::HashMap<String, crate::conversation::cron_runtime::CronJob>,
        >,
    > = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let (cron_wake_tx, cron_wake_rx) = tokio::sync::watch::channel(false);

    // Create the shared message queue
    let queue = Arc::new(MessageQueue::new());

    // Build tool registry with all tools.
    let cwd_for_agents = std::env::current_dir().unwrap_or_default();
    let agent_registry = Arc::new(crate::agents::AgentRegistry::load(&cwd_for_agents));
    let registry = crate::tools::ToolRegistry::new(
        store.clone(),
        config.clone(),
        agent_registry,
        queue.clone(),
        cron_jobs.clone(),
        cron_wake_tx,
    );

    // Spawn cron runtime
    let cron_runtime = crate::conversation::cron_runtime::CronRuntime::new(
        cron_jobs,
        queue.clone(),
        cron_wake_rx,
    );
    tokio::spawn(async move { cron_runtime.run().await });

    // Load skills: bundled (embedded in binary) + user/project.
    // Bundled skills are extracted to ~/.super/plugins/superpowers/ on first run.
    let bundled_skills = crate::skills::bundled::extract_bundled_skills();
    let local_skills = crate::skills::loader::load_all_skills();

    // Merge: local (user/project) overrides bundled on name collision.
    let mut by_name: std::collections::HashMap<String, crate::skills::loader::Skill> =
        bundled_skills
            .into_iter()
            .map(|s| (s.name.clone(), s))
            .collect();
    for s in local_skills {
        by_name.insert(s.name.clone(), s);
    }
    let all_skills: Vec<crate::skills::loader::Skill> = by_name.into_values().collect();

    // Register SkillTool (always — even when no skills are loaded the tool must
    // exist so the model can receive a clear error on invocation).
    registry.register(Arc::new(crate::tools::skill::SkillTool {
        skills: all_skills.clone(),
    }));

    // Session bus is the spine for all engine events. The TUI will subscribe
    // in a later task; for now we just hand the engine its publishing handle.
    let bus = Arc::new(crate::conversation::session_bus::SessionBus::new(
        uuid::Uuid::new_v4().to_string(),
    ));

    // Spawn sidechain JSONL writer so any subagent activity gets persisted.
    let sidechain_dir = crate::conversation::sidechain::default_sidechain_dir(bus.session_id());
    crate::conversation::sidechain::spawn_sidechain_writer(bus.clone(), sidechain_dir);

    let mut engine = crate::conversation::engine::ConversationEngine::new(
        store.clone(),
        config.clone(),
        registry.clone(),
        bus.clone(),
        queue.clone(),
    );
    engine.skills = Arc::new(all_skills);

    // Build system prompt.
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut system_prompt = crate::conversation::system_prompt::SystemPrompt::build(&cwd);

    // Add tool descriptions.
    let tool_descriptions =
        registry.tool_descriptions(&crate::state::store::PermissionMode::Default);
    system_prompt.add_section(format!("Available tools:\n{tool_descriptions}"));

    // Launch TUI.
    crate::tui::app::run_with_engine(config, store, engine, registry, bus, system_prompt, queue).await;
}
