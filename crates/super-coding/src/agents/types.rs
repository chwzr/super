//! Agent types: the instructions a child agent starts from, the model and
//! effort it runs on, and which of its parent's tools it gets.
//!
//! Four types are built in: `general-purpose` (the default), `Explore` and
//! `Plan`, which read but do not edit and start no agents, and `fork`, which
//! carries on the parent's conversation. More come from definition files:
//! Markdown files in `.super/agents/` of a trusted project and in
//! `~/.super/agent/agents/`, whose frontmatter gives `name`, `description`,
//! `model`, `effort` and `tools` and whose body is the type's instructions.
//! A file can replace a built-in type of the same name, except `fork`.

use serde::Serialize;
use std::path::{Path, PathBuf};
use super_agent::placeholder::prompt_todo;

pub const GENERAL_PURPOSE: &str = "general-purpose";
pub const EXPLORE: &str = "Explore";
pub const PLAN: &str = "Plan";
pub const FORK: &str = "fork";

/// The tool a child needs to start agents of its own.
pub const AGENT_TOOL: &str = "Agent";

/// The tools that change files, which a read-only type does without.
const EDITING_TOOLS: [&str; 2] = ["Write", "Edit"];

/// Which of the parent's tools a child of a type gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolChoice {
    /// Every tool the parent has.
    All,
    /// Every tool the parent has except these.
    AllExcept(Vec<String>),
    /// Only these, of the tools the parent has.
    Only(Vec<String>),
}

impl ToolChoice {
    /// Whether a child of this type gets the tool `name`.
    pub fn allows(&self, name: &str) -> bool {
        match self {
            ToolChoice::All => true,
            ToolChoice::AllExcept(names) => !names.iter().any(|known| known == name),
            ToolChoice::Only(names) => names.iter().any(|known| known == name),
        }
    }
}

/// Where a type comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTypeSource {
    BuiltIn,
    /// A definition file in `~/.super/agent/agents/`.
    User(PathBuf),
    /// A definition file in the project's `.super/agents/`.
    Project(PathBuf),
}

/// One type of agent the `Agent` tool can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentType {
    pub name: String,
    pub description: String,
    /// What the child's system prompt starts with (a fork's first message).
    pub instructions: String,
    /// A model of the registry, or `inherit` for the parent's; `None` is the
    /// parent's too.
    pub model: Option<String>,
    /// A thinking level; `None` is the parent's.
    pub effort: Option<String>,
    pub tools: ToolChoice,
    pub source: AgentTypeSource,
}

impl AgentType {
    fn built_in(name: &str, tools: ToolChoice) -> Self {
        AgentType {
            name: name.to_string(),
            description: prompt_todo(&format!("AgentType.{name}.description")),
            instructions: prompt_todo(&format!("AgentType.{name}.instructions")),
            model: None,
            effort: None,
            tools,
            source: AgentTypeSource::BuiltIn,
        }
    }

    /// The built-in `fork`: a copy of the parent's conversation on the
    /// parent's model, which starts no agents.
    pub fn is_fork(&self) -> bool {
        self.name == FORK && self.source == AgentTypeSource::BuiltIn
    }

    /// Whether a child of this type may start agents of its own (when it is
    /// not as deep as agents may nest).
    pub fn can_start_agents(&self) -> bool {
        !self.is_fork() && self.tools.allows(AGENT_TOOL)
    }
}

/// The four built-in types, in the order they are listed.
pub fn built_in_types() -> Vec<AgentType> {
    let read_only = || {
        ToolChoice::AllExcept(
            EDITING_TOOLS
                .iter()
                .chain(&[AGENT_TOOL])
                .map(|name| name.to_string())
                .collect(),
        )
    };
    vec![
        AgentType::built_in(GENERAL_PURPOSE, ToolChoice::All),
        AgentType::built_in(EXPLORE, read_only()),
        AgentType::built_in(PLAN, read_only()),
        AgentType::built_in(FORK, ToolChoice::All),
    ]
}

/// The built-in types with the types of definition files: a file's type
/// replaces a built-in one of the same name, except `fork`, and the others
/// follow in the order they were found.
pub fn with_definitions(definitions: &[AgentType]) -> Vec<AgentType> {
    let mut types = built_in_types();
    for definition in definitions {
        if definition.name == FORK {
            continue;
        }
        match types.iter_mut().find(|known| known.name == definition.name) {
            Some(known) => *known = definition.clone(),
            None => types.push(definition.clone()),
        }
    }
    types
}

/// `~/.super/agent/agents/`, where the user's definition files are.
pub fn user_agent_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".super/agent/agents"))
}

/// `.super/agents/` in the project.
pub fn project_agent_dir(cwd: &Path) -> PathBuf {
    cwd.join(".super/agents")
}

/// The types of the user's definition files and, in a trusted project, of
/// the project's; a project file wins over a user file of the same name.
pub fn discover(cwd: &Path, project_trusted: bool) -> Vec<AgentType> {
    let project = project_trusted.then(|| project_agent_dir(cwd));
    discover_in(user_agent_dir().as_deref(), project.as_deref())
}

/// [`discover`] in the given folders.
pub fn discover_in(user: Option<&Path>, project: Option<&Path>) -> Vec<AgentType> {
    let mut found: Vec<AgentType> = Vec::new();
    let folders = [user.map(|dir| (dir, false)), project.map(|dir| (dir, true))];
    for (dir, from_project) in folders.into_iter().flatten() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("md")
            })
            .collect();
        paths.sort();
        for path in paths {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let source = if from_project {
                AgentTypeSource::Project(path.clone())
            } else {
                AgentTypeSource::User(path.clone())
            };
            let Some(definition) = parse_definition(&path, &text, source) else {
                continue;
            };
            found.retain(|known| known.name != definition.name);
            found.push(definition);
        }
    }
    found
}

/// The type a definition file describes. The name falls back to the file's
/// stem, the description to nothing; `tools` is a list or a comma-separated
/// string, `*` or no `tools` meaning every tool. A file named `fork`, with a
/// name that is not one line, or with a frontmatter that is not YAML (see
/// [`frontmatter_value`](crate::skills::frontmatter_value)) is skipped, so
/// that a type never gets every tool because its `tools` could not be read.
pub fn parse_definition(path: &Path, text: &str, source: AgentTypeSource) -> Option<AgentType> {
    let (frontmatter, body) = crate::skills::split_frontmatter(text);
    let value = match frontmatter {
        Some(frontmatter) => crate::skills::frontmatter_value(frontmatter)?,
        None => serde_yaml::Value::Null,
    };
    let text_of = |key: &str| {
        value[key]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let name = text_of("name")
        .or_else(|| path.file_stem()?.to_str().map(str::to_string))
        .filter(|name| !name.contains(['\n', '\r']))?;
    if name == FORK {
        return None;
    }
    let tools = match &value["tools"] {
        serde_yaml::Value::Sequence(items) => names(items.iter().filter_map(|item| item.as_str())),
        serde_yaml::Value::String(list) => names(list.split(',')),
        _ => ToolChoice::All,
    };
    Some(AgentType {
        description: text_of("description").unwrap_or_default(),
        instructions: body.trim().to_string(),
        model: text_of("model"),
        effort: text_of("effort"),
        tools,
        source,
        name,
    })
}

fn names<'a>(items: impl Iterator<Item = &'a str>) -> ToolChoice {
    let names: Vec<String> = items
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    if names.iter().any(|name| name == "*") {
        ToolChoice::All
    } else {
        ToolChoice::Only(names)
    }
}

/// A type as the model is told of it: its name and description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentTypeSummary {
    pub name: String,
    pub description: String,
}

/// The name and description of each of `types`, in their order.
pub fn type_list(types: &[AgentType]) -> Vec<AgentTypeSummary> {
    types
        .iter()
        .map(|agent_type| AgentTypeSummary {
            name: agent_type.name.clone(),
            description: agent_type.description.clone(),
        })
        .collect()
}

/// The slot that puts the list of types in front of the model: the
/// `Agent.type_list` placeholder, a line break and an `<agent-types>` block
/// with one `<agent-type>` per type.
pub fn type_list_section(list: &[AgentTypeSummary]) -> String {
    let mut section = format!("{}\n<agent-types>\n", prompt_todo("Agent.type_list"));
    for summary in list {
        section.push_str(&format!(
            "<agent-type><name>{}</name><description>{}</description></agent-type>\n",
            escape(&summary.name),
            escape(&summary.description)
        ));
    }
    section.push_str("</agent-types>");
    section
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_types_have_placeholder_texts_and_their_tools() {
        let types = built_in_types();
        let names: Vec<&str> = types.iter().map(|known| known.name.as_str()).collect();
        assert_eq!(names, [GENERAL_PURPOSE, EXPLORE, PLAN, FORK]);
        for known in &types {
            assert_eq!(
                known.description,
                prompt_todo(&format!("AgentType.{}.description", known.name))
            );
            assert_eq!(
                known.instructions,
                prompt_todo(&format!("AgentType.{}.instructions", known.name))
            );
        }
        let explore = &types[1];
        for tool in ["Write", "Edit", "Agent"] {
            assert!(!explore.tools.allows(tool), "{tool}");
        }
        assert!(explore.tools.allows("Read") && explore.tools.allows("Bash"));
        assert!(!explore.can_start_agents());
        assert!(types[0].can_start_agents());
        assert!(types[3].is_fork() && !types[3].can_start_agents());
    }

    #[test]
    fn a_definition_file_gives_name_description_model_effort_tools_and_instructions() {
        let text = "---\nname: reviewer\ndescription: Reviews a diff\nmodel: openai/gpt-test\neffort: high\ntools: Read, Bash\n---\n\nLook at the diff.\n";
        let path = Path::new("/p/.super/agents/review.md");
        let definition =
            parse_definition(path, text, AgentTypeSource::Project(path.into())).unwrap();
        assert_eq!(definition.name, "reviewer");
        assert_eq!(definition.description, "Reviews a diff");
        assert_eq!(definition.model.as_deref(), Some("openai/gpt-test"));
        assert_eq!(definition.effort.as_deref(), Some("high"));
        assert_eq!(
            definition.tools,
            ToolChoice::Only(vec!["Read".into(), "Bash".into()])
        );
        assert_eq!(definition.instructions, "Look at the diff.");
        assert!(!definition.can_start_agents());

        let listed = parse_definition(
            Path::new("/u/helper.md"),
            "---\ntools:\n  - Read\n  - Agent\n---\nHelp.",
            AgentTypeSource::User("/u/helper.md".into()),
        )
        .unwrap();
        assert_eq!(listed.name, "helper");
        assert_eq!(listed.description, "");
        assert!(listed.can_start_agents());
        let everything = parse_definition(
            Path::new("/u/all.md"),
            "---\ntools: \"*\"\n---\nAll.",
            AgentTypeSource::User("/u/all.md".into()),
        )
        .unwrap();
        assert_eq!(everything.tools, ToolChoice::All);
        assert!(
            parse_definition(
                Path::new("/u/fork.md"),
                "no frontmatter",
                AgentTypeSource::User("/u/fork.md".into())
            )
            .is_none()
        );
    }

    /// A value YAML does not take as it stands, such as a description with
    /// `: ` in it, is read as its text, and the other fields keep theirs;
    /// a frontmatter that is no YAML even so leaves the file out.
    #[test]
    fn a_value_with_a_colon_is_read_as_text_and_broken_frontmatter_is_left_out() {
        let path = Path::new("/u/t_colon.md");
        let definition = parse_definition(
            path,
            "---\nname: t_colon\ndescription: Reviews code: read only\ntools: Read\nmodel: thinker\neffort: low\n---\nMY-COLON\n",
            AgentTypeSource::User(path.into()),
        )
        .unwrap();
        assert_eq!(definition.description, "Reviews code: read only");
        assert_eq!(definition.tools, ToolChoice::Only(vec!["Read".into()]));
        assert_eq!(definition.model.as_deref(), Some("thinker"));
        assert_eq!(definition.effort.as_deref(), Some("low"));
        assert_eq!(definition.instructions, "MY-COLON");
        let flow = parse_definition(
            path,
            "---\ndescription: \"quoted: fine\"\ntools: [Read, Bash]\n---\nX",
            AgentTypeSource::User(path.into()),
        )
        .unwrap();
        assert_eq!(flow.description, "quoted: fine");
        assert_eq!(
            flow.tools,
            ToolChoice::Only(vec!["Read".into(), "Bash".into()])
        );
        assert!(
            parse_definition(
                path,
                "---\ntools: [Read\ndescription: broken\n---\nX",
                AgentTypeSource::User(path.into()),
            )
            .is_none()
        );
    }

    #[test]
    fn project_files_win_over_user_files_and_files_over_built_in_types_but_fork() {
        let user = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("a.md"),
            "---\nname: reviewer\ndescription: user\n---\nUser.",
        )
        .unwrap();
        std::fs::write(
            user.path().join("b.md"),
            "---\nname: Explore\ndescription: mine\n---\nLook.",
        )
        .unwrap();
        std::fs::write(user.path().join("notes.txt"), "not a definition").unwrap();
        std::fs::write(
            project.path().join("r.md"),
            "---\nname: reviewer\ndescription: project\n---\nProject.",
        )
        .unwrap();
        std::fs::write(
            project.path().join("fork.md"),
            "---\ndescription: not allowed\n---\nNo.",
        )
        .unwrap();

        let found = discover_in(Some(user.path()), Some(project.path()));
        let names: Vec<(&str, &str)> = found
            .iter()
            .map(|known| (known.name.as_str(), known.description.as_str()))
            .collect();
        assert_eq!(names, [("Explore", "mine"), ("reviewer", "project")]);
        let user_only = discover_in(Some(user.path()), None);
        let reviewer = user_only.iter().find(|known| known.name == "reviewer");
        assert_eq!(reviewer.unwrap().description, "user");

        let types = with_definitions(&found);
        let names: Vec<&str> = types.iter().map(|known| known.name.as_str()).collect();
        assert_eq!(names, [GENERAL_PURPOSE, EXPLORE, PLAN, FORK, "reviewer"]);
        assert_eq!(types[1].instructions, "Look.");
        assert!(types[3].is_fork());
    }

    #[test]
    fn the_type_list_is_data_behind_its_placeholder() {
        let list = vec![
            AgentTypeSummary {
                name: "general-purpose".into(),
                description: "any <task>".into(),
            },
            AgentTypeSummary {
                name: "reviewer".into(),
                description: String::new(),
            },
        ];
        assert_eq!(
            type_list_section(&list),
            format!(
                "{}\n<agent-types>\n<agent-type><name>general-purpose</name><description>any &lt;task&gt;</description></agent-type>\n<agent-type><name>reviewer</name><description></description></agent-type>\n</agent-types>",
                prompt_todo("Agent.type_list")
            )
        );
    }
}
