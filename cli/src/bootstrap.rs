use std::sync::Arc;

use crate::config::load_config;

pub async fn run() {
    let config = load_config();

    if config.openrouter_api_key.is_none() {
        eprintln!("Not logged in. Run 'super login' first.");
        return;
    }

    let store = Arc::new(crate::state::store::Store::new());
    let engine = crate::conversation::engine::ConversationEngine::new(store.clone(), config.clone());

    // Build tool registry with all tools
    let registry = crate::tools::ToolRegistry::new(store.clone(), config.clone());

    // Load skills and register the SkillTool with loaded skills
    let skills = crate::skills::loader::load_all_skills();
    if !skills.is_empty() {
        registry.register(Arc::new(crate::tools::skill::SkillTool {
            skills: skills.clone(),
        }));
    }

    // Build system prompt
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut system_prompt = crate::conversation::system_prompt::SystemPrompt::build(&cwd);

    // Add skill descriptions to system prompt
    if !skills.is_empty() {
        let skill_desc: Vec<String> = skills
            .iter()
            .map(|s| format!("- {}: {}", s.name, s.description))
            .collect();
        system_prompt.add_section(format!("Available skills:\n{}", skill_desc.join("\n")));
    }

    // Add tool descriptions
    let tool_descriptions =
        registry.tool_descriptions(&crate::state::store::PermissionMode::Default);
    system_prompt.add_section(format!("Available tools:\n{tool_descriptions}"));

    // Launch TUI
    crate::tui::app::run_with_engine(config, store, engine, Arc::new(registry), system_prompt).await;
}
