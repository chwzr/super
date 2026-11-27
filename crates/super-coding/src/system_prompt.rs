//! System prompt assembly: the body of the mode (in the basic mode identity,
//! tool list and guidelines), appended custom text, project context blocks,
//! skills XML, and the cwd line.

use crate::context_files::ContextFile;
use crate::settings::Mode;
use crate::skills::Skill;

pub struct SystemPromptOptions<'a> {
    /// Which body the prompt starts with when `custom_prompt` is `None`.
    pub mode: Mode,
    /// Replaces the default prompt body in either mode (context/skills/cwd
    /// still append).
    pub custom_prompt: Option<&'a str>,
    pub append: Option<&'a str>,
    pub selected_tools: &'a [String],
    pub cwd: &'a str,
    pub context_files: &'a [ContextFile],
    pub skills: &'a [Skill],
}

fn tool_snippet(name: &str) -> Option<&'static str> {
    Some(match name {
        "Read" => "Read file contents (text and images)",
        "Bash" => "Execute shell commands",
        "Edit" => "Make targeted text replacements in files",
        "Write" => "Create or overwrite files",
        "grep" => "Search file contents for patterns (respects .gitignore)",
        "find" => "Find files by glob pattern (respects .gitignore)",
        "ls" => "List directory contents",
        _ => return None,
    })
}

pub fn build_system_prompt(options: &SystemPromptOptions) -> String {
    let cwd = options.cwd.replace('\\', "/");
    let mut prompt = match (options.custom_prompt, options.mode) {
        (Some(custom), _) => custom.to_string(),
        (None, Mode::Basic) => default_prompt(options),
        (None, Mode::Advanced) => advanced_prompt(),
    };

    if let Some(append) = options.append {
        prompt.push_str("\n\n");
        prompt.push_str(append);
    }

    if !options.context_files.is_empty() {
        prompt.push_str("\n\n<project_context>\n\n");
        prompt.push_str("Project-specific instructions and guidelines:\n\n");
        for file in options.context_files {
            prompt.push_str(&format!(
                "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n",
                file.path.display(),
                file.content
            ));
        }
        prompt.push_str("</project_context>\n");
    }

    let file_read_tool = options
        .selected_tools
        .iter()
        .find(|tool| matches!(tool.as_str(), "Read" | "Bash"));
    if let Some(tool) = file_read_tool
        && !options.skills.is_empty()
    {
        prompt.push('\n');
        prompt.push_str(&crate::skills::format_skills_for_prompt_with_tool(
            options.skills,
            tool,
        ));
        prompt.push('\n');
    }

    prompt.push_str(&format!("\nCurrent working directory: {cwd}"));
    prompt
}

/// The body of the advanced mode's prompt, which is not written yet.
fn advanced_prompt() -> String {
    super_agent::placeholder::prompt_todo("SystemPrompt.advanced")
}

fn default_prompt(options: &SystemPromptOptions) -> String {
    let visible: Vec<String> = options
        .selected_tools
        .iter()
        .filter_map(|name| tool_snippet(name).map(|snippet| format!("- {name}: {snippet}")))
        .collect();
    let tools_list = if visible.is_empty() {
        "(none)".to_string()
    } else {
        visible.join("\n")
    };

    let mut guidelines: Vec<&str> = Vec::new();
    let has = |t: &str| options.selected_tools.iter().any(|x| x == t);
    if has("Bash") && !has("grep") && !has("find") && !has("ls") {
        guidelines.push("Use Bash for file operations like ls, rg, find");
    }
    guidelines.push("Be concise in your responses");
    guidelines.push("Show file paths clearly when working with files");
    let guidelines_list = guidelines
        .iter()
        .map(|g| format!("- {g}"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "You are an expert coding assistant operating inside super, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.\n\nAvailable tools:\n{tools_list}\n\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n\nGuidelines:\n{guidelines_list}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_options<'a>(tools: &'a [String]) -> SystemPromptOptions<'a> {
        SystemPromptOptions {
            mode: Mode::Basic,
            custom_prompt: None,
            append: None,
            selected_tools: tools,
            cwd: "/work",
            context_files: &[],
            skills: &[],
        }
    }

    #[test]
    fn default_prompt_lists_tools_and_cwd() {
        let tools = vec!["Read".to_string(), "Bash".to_string()];
        let p = build_system_prompt(&base_options(&tools));
        assert!(p.contains("- Read:"));
        assert!(p.contains("- Bash:"));
        assert!(p.contains("Use Bash for file operations"));
        assert!(p.ends_with("Current working directory: /work"));
    }

    #[test]
    fn skills_are_available_with_bash_as_the_only_reader() {
        let tools = vec!["Bash".to_string()];
        let skill = Skill {
            name: "example".into(),
            description: "Example skill".into(),
            file_path: "/skills/example/SKILL.md".into(),
            disable_model_invocation: false,
        };
        let mut options = base_options(&tools);
        options.skills = std::slice::from_ref(&skill);
        let prompt = build_system_prompt(&options);
        assert!(prompt.contains("<name>example</name>"));
        assert!(prompt.contains("Use Bash to load a skill's file"));

        let no_reader = vec!["Edit".to_string()];
        options.selected_tools = &no_reader;
        assert!(!build_system_prompt(&options).contains("<available_skills>"));
    }

    #[test]
    fn custom_prompt_keeps_appends() {
        let tools = vec!["Read".to_string()];
        let mut o = base_options(&tools);
        o.custom_prompt = Some("You are a poet.");
        o.append = Some("Rhyme everything.");
        let p = build_system_prompt(&o);
        assert!(p.starts_with("You are a poet."));
        assert!(p.contains("Rhyme everything."));
        assert!(!p.contains("expert coding assistant"));
    }

    #[test]
    fn the_advanced_body_is_a_placeholder_before_the_same_dynamic_parts() {
        let tools = vec!["Read".to_string(), "Bash".to_string()];
        let skill = Skill {
            name: "example".into(),
            description: "Example skill".into(),
            file_path: "/skills/example/SKILL.md".into(),
            disable_model_invocation: false,
        };
        let files = vec![ContextFile {
            path: "/p/AGENTS.md".into(),
            content: "be nice".into(),
        }];
        let mut basic = base_options(&tools);
        basic.append = Some("Rhyme everything.");
        basic.context_files = &files;
        basic.skills = std::slice::from_ref(&skill);
        let basic_prompt = build_system_prompt(&basic);
        let advanced = SystemPromptOptions {
            mode: Mode::Advanced,
            ..basic
        };
        let advanced_prompt = build_system_prompt(&advanced);
        let placeholder = super_agent::placeholder::prompt_todo("SystemPrompt.advanced");

        assert!(advanced_prompt.starts_with(&format!("{placeholder}\n\n")));
        assert!(!advanced_prompt.contains("expert coding assistant"));
        assert!(!advanced_prompt.contains("- Read:"));
        // Everything after the body is what the basic mode appends.
        let basic_body = default_prompt(&advanced);
        assert_eq!(
            advanced_prompt.strip_prefix(placeholder.as_str()),
            basic_prompt.strip_prefix(basic_body.as_str())
        );
        assert!(advanced_prompt.contains("Rhyme everything."));
        assert!(advanced_prompt.contains("<project_instructions path=\"/p/AGENTS.md\">"));
        assert!(advanced_prompt.contains("<name>example</name>"));
        assert!(advanced_prompt.ends_with("Current working directory: /work"));
    }

    #[test]
    fn a_custom_prompt_replaces_the_body_in_both_modes() {
        let tools = vec!["Read".to_string()];
        for mode in [Mode::Basic, Mode::Advanced] {
            let mut options = base_options(&tools);
            options.mode = mode;
            options.custom_prompt = Some("You are a poet.");
            let prompt = build_system_prompt(&options);
            assert!(prompt.starts_with("You are a poet."));
            assert!(!prompt.contains(super_agent::placeholder::PREFIX));
        }
    }

    #[test]
    fn context_files_wrapped() {
        let tools = vec!["Read".to_string()];
        let files = vec![ContextFile {
            path: "/p/AGENTS.md".into(),
            content: "be nice".into(),
        }];
        let mut o = base_options(&tools);
        o.context_files = &files;
        let p = build_system_prompt(&o);
        assert!(p.contains("<project_context>"));
        assert!(p.contains("<project_instructions path=\"/p/AGENTS.md\">"));
        assert!(p.contains("be nice"));
    }
}
