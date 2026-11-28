//! Agent Skills (agentskills.io style): Markdown files with YAML frontmatter,
//! discovered from user/project locations, and called by the user through
//! `$name` or `/name` input tokens. The older `/skill:name` form is also
//! accepted.
//!
//! How the model reaches them depends on the mode. In the basic mode the
//! system prompt lists each skill with its file, and the model reads the file
//! ([`format_skills_for_prompt`]). In the advanced mode the session lists
//! them without their files ([`skill_list_section`]) and the model loads one
//! with the `Skill` tool ([`load`]), which can also run it in an agent of its
//! own when its frontmatter asks for that ([`SkillRun`]).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
    pub disable_model_invocation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInvocation {
    pub skill_names: Vec<String>,
    pub request: String,
}

/// Parse leading skill commands and `$name` skill mentions anywhere in user input.
pub fn parse_invocation(input: &str, skills: &[Skill]) -> Option<SkillInvocation> {
    let mut rest = input.trim_start();
    let mut skill_names = Vec::new();

    loop {
        let token_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = &rest[..token_end];
        let candidate = token
            .strip_prefix('$')
            .or_else(|| token.strip_prefix("/skill:"))
            .or_else(|| token.strip_prefix('/'));
        let Some(skill) = candidate
            .filter(|name| !name.is_empty())
            .and_then(|name| skills.iter().find(|skill| skill.name == name))
        else {
            break;
        };
        if !skill_names.contains(&skill.name) {
            skill_names.push(skill.name.clone());
        }
        rest = rest[token_end..].trim_start();
    }

    for name in rest
        .split_whitespace()
        .filter_map(|token| token.strip_prefix('$'))
    {
        if let Some(skill) = skills.iter().find(|skill| skill.name == name)
            && !skill_names.contains(&skill.name)
        {
            skill_names.push(skill.name.clone());
        }
    }

    (!skill_names.is_empty()).then(|| SkillInvocation {
        skill_names,
        request: rest.trim().to_string(),
    })
}

/// Read invoked skill files and build text that is sent only to the model.
pub fn expand_invocation(invocation: &SkillInvocation, skills: &[Skill]) -> Result<String> {
    let mut output = String::from(
        "The user explicitly invoked the following skills. Follow each skill for this turn.\n\n",
    );
    for name in &invocation.skill_names {
        let skill = skills
            .iter()
            .find(|skill| &skill.name == name)
            .with_context(|| format!("invoked skill `{name}` is not available"))?;
        let content = std::fs::read_to_string(&skill.file_path).with_context(|| {
            format!(
                "could not read invoked skill `{name}` at {}",
                skill.file_path.display()
            )
        })?;
        output.push_str(&format!(
            "<invoked_skill name=\"{}\" path=\"{}\">\n{}\n</invoked_skill>\n\n",
            escape_xml(&skill.name),
            escape_xml(&skill.file_path.display().to_string()),
            content
        ));
    }
    output.push_str("<user_request>\n");
    output.push_str(&invocation.request);
    output.push_str("\n</user_request>");
    Ok(output)
}

/// Split a Markdown document into (frontmatter, body).
pub fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---") else {
        return (None, text);
    };
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))
        .unwrap_or(rest);
    for end in ["\n---\n", "\n---\r\n", "\r\n---\r\n", "\r\n---\n"] {
        if let Some(pos) = rest.find(end) {
            return (Some(&rest[..pos]), &rest[pos + end.len()..]);
        }
    }
    if let Some(stripped) = rest
        .strip_suffix("\n---")
        .or_else(|| rest.strip_suffix("\r\n---"))
    {
        return (Some(stripped), "");
    }
    (None, text)
}

/// The YAML of a frontmatter. A value written without quotes that YAML does
/// not take as it stands, such as `description: Reviews code: read only`,
/// is read as the text it is: when the whole does not parse, each
/// `key: value` line whose value holds `: ` or ` #` is read again with the
/// value quoted. `None` when even that is not YAML.
pub fn frontmatter_value(frontmatter: &str) -> Option<serde_yaml::Value> {
    if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(frontmatter) {
        return Some(value);
    }
    let quoted: Vec<String> = frontmatter
        .lines()
        .map(|line| {
            let Some((key, value)) = line.split_once(": ") else {
                return line.to_string();
            };
            let plain_key = !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            let value = value.trim();
            let needs_quotes = value.contains(": ") || value.contains(" #");
            let structured = value.starts_with(['"', '\'', '[', '{', '|', '>', '&', '*', '!']);
            if plain_key && needs_quotes && !structured {
                let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
                format!("{key}: \"{escaped}\"")
            } else {
                line.to_string()
            }
        })
        .collect();
    serde_yaml::from_str(&quoted.join("\n")).ok()
}

/// A skill's file; `None` when it cannot be read or its frontmatter is not
/// YAML ([`frontmatter_value`]), so that a skill whose frontmatter keeps it
/// from the model is never offered to it.
fn parse_skill(path: &Path) -> Option<Skill> {
    let text = std::fs::read_to_string(path).ok()?;
    let (frontmatter, _body) = split_frontmatter(&text);
    let mut name = None;
    let mut description = None;
    let mut disable_model_invocation = false;
    if let Some(fm) = frontmatter {
        let value = frontmatter_value(fm)?;
        name = value["name"].as_str().map(String::from);
        description = value["description"].as_str().map(String::from);
        disable_model_invocation = value["disable-model-invocation"].as_bool().unwrap_or(false);
    }
    let fallback_name = || {
        if path.file_name().and_then(|f| f.to_str()) == Some("SKILL.md") {
            path.parent()?.file_name()?.to_str().map(String::from)
        } else {
            path.file_stem()?.to_str().map(String::from)
        }
    };
    let name = name.or_else(fallback_name)?;
    Some(Skill {
        description: description.unwrap_or_else(|| format!("Skill {name}")),
        name,
        file_path: path.to_path_buf(),
        disable_model_invocation,
    })
}

/// Recursively find `SKILL.md` files under `dir`. When `include_root_md` is
/// set, direct `.md` children of `dir` also count as single-file skills.
fn scan_dir(dir: &Path, include_root_md: bool, out: &mut Vec<Skill>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let skill_md = dir.join("SKILL.md");
    if skill_md.is_file()
        && let Some(skill) = parse_skill(&skill_md)
    {
        out.push(skill);
    }
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, false, out);
        } else if include_root_md
            && path.extension().and_then(|e| e.to_str()) == Some("md")
            && path.file_name().and_then(|f| f.to_str()) != Some("SKILL.md")
            && let Some(skill) = parse_skill(&path)
        {
            out.push(skill);
        }
    }
}

/// Discover skills from every location pi checks (super equivalents).
pub fn discover(cwd: &Path, project_trusted: bool, extra_paths: &[PathBuf]) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        scan_dir(&home.join(".super/agent/skills"), true, &mut out);
        scan_dir(&home.join(".agents/skills"), false, &mut out);
    }
    if project_trusted {
        scan_dir(&cwd.join(".super/skills"), true, &mut out);
        // .agents/skills in cwd and ancestors up to a git root.
        for dir in cwd.ancestors() {
            scan_dir(&dir.join(".agents/skills"), false, &mut out);
            if dir.join(".git").exists() {
                break;
            }
        }
    }
    for path in extra_paths {
        if path.is_dir() {
            scan_dir(path, true, &mut out);
        } else if let Some(skill) = parse_skill(path) {
            out.push(skill);
        }
    }
    // Later discoveries with the same name lose (first location wins).
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.name.clone()));
    out
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    format_skills_for_prompt_with_tool(skills, "Read")
}

pub fn format_skills_for_prompt_with_tool(skills: &[Skill], tool: &str) -> String {
    let visible: Vec<&Skill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();
    if visible.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "The following skills provide specialized instructions for specific tasks.\nUse {tool} to load a skill's file when the task matches its description.\nWhen a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.\n\n<available_skills>\n",
    );
    for skill in visible {
        out.push_str("  <skill>\n");
        out.push_str(&format!("    <name>{}</name>\n", escape_xml(&skill.name)));
        out.push_str(&format!(
            "    <description>{}</description>\n",
            escape_xml(&skill.description)
        ));
        out.push_str(&format!(
            "    <location>{}</location>\n",
            escape_xml(&skill.file_path.display().to_string())
        ));
        out.push_str("  </skill>\n");
    }
    out.push_str("</available_skills>");
    out
}

/// A skill as the advanced mode lists it to the model: no file, since the
/// model loads it with the `Skill` tool by its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
}

/// The list of skills the advanced mode puts after the system prompt while
/// the session offers the `Skill` tool: the placeholder `Skill.list`, then
/// one entry per skill. Empty when there is no skill.
pub fn skill_list_section(skills: &[SkillSummary]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = super_agent::placeholder::prompt_todo("Skill.list");
    out.push_str("\n<available_skills>\n");
    for skill in skills {
        out.push_str("  <skill>\n");
        out.push_str(&format!("    <name>{}</name>\n", escape_xml(&skill.name)));
        out.push_str(&format!(
            "    <description>{}</description>\n",
            escape_xml(&skill.description)
        ));
        out.push_str("  </skill>\n");
    }
    out.push_str("</available_skills>");
    out
}

/// How a skill runs in an agent of its own instead of in the turn that
/// loads it. A skill asks for this with `context: fork` in its frontmatter;
/// `agent` names the agent type (`general-purpose` when it is missing),
/// `model` and `effort` choose the agent's model and thinking level as the
/// `Agent` tool's parameters of those names do, and `background: true` runs
/// the agent in the background instead of waiting for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillRun {
    pub agent: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub background: bool,
}

/// A skill file as the `Skill` tool loads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSkill {
    /// The text after the frontmatter.
    pub body: String,
    /// Set when the skill runs in an agent of its own.
    pub run: Option<SkillRun>,
}

/// Read a skill's file: its body, and whether its frontmatter asks for an
/// agent of its own.
pub fn load(skill: &Skill) -> Result<LoadedSkill> {
    let text = std::fs::read_to_string(&skill.file_path).with_context(|| {
        format!(
            "Could not read the skill {} at {}",
            skill.name,
            skill.file_path.display()
        )
    })?;
    Ok(parse_loaded(&text))
}

fn parse_loaded(text: &str) -> LoadedSkill {
    let (frontmatter, body) = split_frontmatter(text);
    let value = frontmatter
        .and_then(frontmatter_value)
        .unwrap_or(serde_yaml::Value::Null);
    let text_of = |key: &str| {
        value[key]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(String::from)
    };
    let run = (value["context"].as_str().map(str::trim) == Some("fork")).then(|| SkillRun {
        agent: text_of("agent"),
        model: text_of("model"),
        effort: text_of("effort"),
        background: value["background"].as_bool().unwrap_or(false),
    });
    LoadedSkill {
        body: body.to_string(),
        run,
    }
}

/// The text the `Skill` tool hands over for a skill: the placeholder
/// `header` (`Skill.loaded` for the turn that loads it, `Skill.agent_prompt`
/// for an agent that runs it), then a block with the skill's name, its file
/// when it has one, and its body. Every `$ARGUMENTS` in the body becomes
/// `args`; when the body has none, `args` goes into the block as its own
/// element.
pub fn skill_text(
    header: &str,
    name: &str,
    file: Option<&Path>,
    body: &str,
    args: Option<&str>,
) -> String {
    let args = args.map(str::trim).filter(|args| !args.is_empty());
    let (body, separate) = match args {
        Some(args) if body.contains("$ARGUMENTS") => (body.replace("$ARGUMENTS", args), None),
        Some(args) => (body.to_string(), Some(args)),
        None => (body.to_string(), None),
    };
    let mut out = super_agent::placeholder::prompt_todo(header);
    out.push_str("\n<skill>\n");
    out.push_str(&format!("<name>{}</name>\n", escape_xml(name)));
    if let Some(file) = file {
        out.push_str(&format!(
            "<path>{}</path>\n",
            escape_xml(&file.display().to_string())
        ));
    }
    if let Some(args) = separate {
        out.push_str(&format!("<arguments>{}</arguments>\n", escape_xml(args)));
    }
    out.push_str("<content>\n");
    out.push_str(body.trim_matches('\n'));
    out.push_str("\n</content>\n</skill>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_split() {
        let (fm, body) = split_frontmatter("---\nname: x\n---\nbody here");
        assert_eq!(fm, Some("name: x"));
        assert_eq!(body, "body here");
        let (fm, body) = split_frontmatter("no frontmatter");
        assert!(fm.is_none());
        assert_eq!(body, "no frontmatter");
    }

    #[test]
    fn discovers_skill_dirs_and_root_md() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("pdf-tools")).unwrap();
        std::fs::write(
            root.join("pdf-tools/SKILL.md"),
            "---\nname: pdf-tools\ndescription: Work with PDFs\n---\nInstructions",
        )
        .unwrap();
        std::fs::write(
            root.join("quick.md"),
            "---\ndescription: Quick one\n---\nGo",
        )
        .unwrap();
        let mut out = Vec::new();
        scan_dir(root, true, &mut out);
        let names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"pdf-tools"));
        assert!(names.contains(&"quick"));

        let direct = discover(root, false, &[root.join("pdf-tools")]);
        let loaded = direct
            .iter()
            .find(|skill| skill.name == "pdf-tools")
            .unwrap();
        assert_eq!(loaded.file_path, root.join("pdf-tools/SKILL.md"));
    }

    /// A skill whose description has `: ` in it keeps its other fields,
    /// such as the one that keeps it from the model; one whose frontmatter
    /// is no YAML is left out.
    #[test]
    fn a_skill_with_a_colon_in_a_value_keeps_its_fields() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("mine.md"),
            "---\ndescription: Deploys: only by hand\ndisable-model-invocation: true\n---\nGo",
        )
        .unwrap();
        std::fs::write(
            root.join("broken.md"),
            "---\ndescription: [never closed\n---\nGo",
        )
        .unwrap();
        let mut out = Vec::new();
        scan_dir(root, true, &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].name, "mine");
        assert_eq!(out[0].description, "Deploys: only by hand");
        assert!(out[0].disable_model_invocation);
        let loaded = parse_loaded("---\ncontext: fork\nagent: Explore\ndescription: a: b\n---\nBody");
        assert_eq!(loaded.run.unwrap().agent.as_deref(), Some("Explore"));
    }

    #[test]
    fn prompt_xml_escapes() {
        let skills = vec![Skill {
            name: "a<b".into(),
            description: "uses & things".into(),
            file_path: "/s/SKILL.md".into(),
            disable_model_invocation: false,
        }];
        let xml = format_skills_for_prompt(&skills);
        assert!(xml.contains("<name>a&lt;b</name>"));
        assert!(xml.contains("uses &amp; things"));
    }

    fn skill(name: &str, path: PathBuf) -> Skill {
        Skill {
            name: name.into(),
            description: format!("Use {name}"),
            file_path: path,
            disable_model_invocation: false,
        }
    }

    #[test]
    fn parses_chained_dollar_slash_and_compatible_skill_tokens() {
        let skills = vec![
            skill("review", "review/SKILL.md".into()),
            skill("tests", "tests/SKILL.md".into()),
            skill("docs", "docs/SKILL.md".into()),
        ];
        assert_eq!(
            parse_invocation("  $review /tests /skill:docs check this", &skills),
            Some(SkillInvocation {
                skill_names: vec!["review".into(), "tests".into(), "docs".into()],
                request: "check this".into(),
            })
        );
        assert_eq!(
            parse_invocation("please use $review and $tests", &skills),
            Some(SkillInvocation {
                skill_names: vec!["review".into(), "tests".into()],
                request: "please use $review and $tests".into(),
            })
        );
        assert_eq!(parse_invocation("$unknown request", &skills), None);
    }

    #[test]
    fn expansion_reads_each_skill_once_and_keeps_the_request_last() {
        let dir = tempfile::tempdir().unwrap();
        let review = dir.path().join("review.md");
        let tests = dir.path().join("tests.md");
        std::fs::write(&review, "Review instructions").unwrap();
        std::fs::write(&tests, "Test instructions").unwrap();
        let skills = vec![skill("review", review), skill("tests", tests)];
        let invocation = parse_invocation("$review /tests inspect it", &skills).unwrap();

        let expanded = expand_invocation(&invocation, &skills).unwrap();

        assert_eq!(expanded.matches("Review instructions").count(), 1);
        assert_eq!(expanded.matches("Test instructions").count(), 1);
        assert!(expanded.ends_with("<user_request>\ninspect it\n</user_request>"));
    }

    #[test]
    fn the_advanced_list_names_skills_without_their_files() {
        let list = skill_list_section(&[
            SkillSummary {
                name: "review".into(),
                description: "Review <staged> changes".into(),
            },
            SkillSummary {
                name: "tests".into(),
                description: "Write tests".into(),
            },
        ]);
        let placeholder = super_agent::placeholder::prompt_todo("Skill.list");
        assert_eq!(
            list,
            format!(
                "{placeholder}\n<available_skills>\n  <skill>\n    <name>review</name>\n    <description>Review &lt;staged&gt; changes</description>\n  </skill>\n  <skill>\n    <name>tests</name>\n    <description>Write tests</description>\n  </skill>\n</available_skills>"
            )
        );
        assert!(!list.contains("<location>"));
        assert!(skill_list_section(&[]).is_empty());
    }

    #[test]
    fn the_frontmatter_says_whether_a_skill_runs_in_an_agent() {
        let inline = parse_loaded("---\nname: x\ndescription: y\n---\nDo the thing.\n");
        assert_eq!(inline.body, "Do the thing.\n");
        assert_eq!(inline.run, None);

        let forked = parse_loaded("---\nname: x\ncontext: fork\n---\nDo it.");
        assert_eq!(forked.run, Some(SkillRun::default()));

        let background = parse_loaded(
            "---\nname: x\ncontext: fork\nagent: Explore\nmodel: mock-1\neffort: low\nbackground: true\n---\nLook.",
        );
        assert_eq!(
            background.run,
            Some(SkillRun {
                agent: Some("Explore".into()),
                model: Some("mock-1".into()),
                effort: Some("low".into()),
                background: true,
            })
        );
        // `background` alone asks for nothing: only `context: fork` does.
        assert_eq!(parse_loaded("---\nbackground: true\n---\nx").run, None);
        assert_eq!(parse_loaded("No frontmatter at all.").run, None);
    }

    #[test]
    fn the_arguments_replace_their_marker_or_go_into_the_block() {
        let header = super_agent::placeholder::prompt_todo("Skill.loaded");
        let file = Path::new("/skills/review/SKILL.md");
        assert_eq!(
            skill_text(
                "Skill.loaded",
                "review",
                Some(file),
                "Review $ARGUMENTS now.\n",
                Some("src/a.rs")
            ),
            format!(
                "{header}\n<skill>\n<name>review</name>\n<path>/skills/review/SKILL.md</path>\n<content>\nReview src/a.rs now.\n</content>\n</skill>"
            )
        );
        assert_eq!(
            skill_text("Skill.loaded", "review", None, "Review.", Some("a & b")),
            format!(
                "{header}\n<skill>\n<name>review</name>\n<arguments>a &amp; b</arguments>\n<content>\nReview.\n</content>\n</skill>"
            )
        );
        // Without arguments the marker stays as the skill wrote it.
        assert!(
            skill_text(
                "Skill.loaded",
                "review",
                None,
                "Use $ARGUMENTS.",
                Some("  ")
            )
            .contains("Use $ARGUMENTS.")
        );
    }

    #[test]
    fn expansion_error_names_an_unreadable_skill() {
        let skills = vec![skill("missing", "/no/such/skill/SKILL.md".into())];
        let invocation = parse_invocation("$missing run", &skills).unwrap();
        let error = expand_invocation(&invocation, &skills).unwrap_err();
        assert!(format!("{error:#}").contains("invoked skill `missing`"));
    }
}
