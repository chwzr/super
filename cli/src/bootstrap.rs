use std::sync::Arc;

use crate::config::load_config;

pub async fn run() {
    let config = load_config();

    if config.openrouter_api_key.is_none() {
        eprintln!("Not logged in. Run 'super login' first.");
        return;
    }

    let store = Arc::new(crate::state::store::Store::new());
    // Seed the store's provider and model_class from config.
    {
        let provider    = config.provider.clone();
        let model_class = config.model_class.clone();
        store.set_state(|s| {
            s.provider    = provider;
            s.model_class = model_class;
        });
    }

    // Build tool registry with all tools.
    let cwd_for_agents = std::env::current_dir().unwrap_or_default();
    let agent_registry = Arc::new(crate::agents::AgentRegistry::load(&cwd_for_agents));
    let registry = crate::tools::ToolRegistry::new(
        store.clone(),
        config.clone(),
        agent_registry,
    );

    // Load skills and (re-)register the SkillTool with loaded skills.
    let skills = crate::skills::loader::load_all_skills();
    if !skills.is_empty() {
        registry.register(Arc::new(crate::tools::skill::SkillTool {
            skills: skills.clone(),
        }));
    }

    // Session bus is the spine for all engine events. The TUI will subscribe
    // in a later task; for now we just hand the engine its publishing handle.
    let bus = Arc::new(crate::conversation::session_bus::SessionBus::new(
        uuid::Uuid::new_v4().to_string(),
    ));

    // Spawn sidechain JSONL writer so any subagent activity gets persisted.
    let sidechain_dir = crate::conversation::sidechain::default_sidechain_dir(bus.session_id());
    crate::conversation::sidechain::spawn_sidechain_writer(bus.clone(), sidechain_dir);

    let engine = crate::conversation::engine::ConversationEngine::new(
        store.clone(),
        config.clone(),
        registry.clone(),
        bus.clone(),
    );

    // Build system prompt.
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut system_prompt = crate::conversation::system_prompt::SystemPrompt::build(&cwd);

    // Add skill descriptions to system prompt.
    if !skills.is_empty() {
        let skill_desc: Vec<String> = skills
            .iter()
            .map(|s| format!("- {}: {}", s.name, s.description))
            .collect();
        system_prompt.add_section(format!("Available skills:\n{}", skill_desc.join("\n")));
    }

    // Add tool descriptions.
    let tool_descriptions =
        registry.tool_descriptions(&crate::state::store::PermissionMode::Default);
    system_prompt.add_section(format!("Available tools:\n{tool_descriptions}"));

    // Launch TUI.
    crate::tui::app::run_with_engine(config, store, engine, registry, bus, system_prompt).await;
}
