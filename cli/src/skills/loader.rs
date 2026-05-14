use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub base_directory: Option<std::path::PathBuf>,
}

pub fn load_all_skills() -> Vec<Skill> {
    let mut skills = Vec::new();
    // Load from ~/.super/skills/
    if let Some(home) = dirs::home_dir() {
        let user_dir = home.join(".super").join("skills");
        load_skills_from_dir(&user_dir, &mut skills);
    }
    // Load from .claude/skills/ in cwd
    if let Ok(cwd) = std::env::current_dir() {
        let project_dir = cwd.join(".claude").join("skills");
        load_skills_from_dir(&project_dir, &mut skills);
    }
    skills
}

fn load_skills_from_dir(dir: &std::path::Path, skills: &mut Vec<Skill>) {
    if !dir.exists() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "md") {
                if let Some(skill) = load_skill_file(&path) {
                    skills.push(skill);
                }
            }
        }
    }
}

fn load_skill_file(path: &std::path::Path) -> Option<Skill> {
    let content = std::fs::read_to_string(path).ok()?;
    let (frontmatter, body) = parse_frontmatter(&content)?;
    let name = frontmatter
        .get("name")
        .cloned()
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_default()
        });
    let description = frontmatter.get("description").cloned().unwrap_or_default();
    Some(Skill {
        name,
        description,
        content: body.to_string(),
        base_directory: path.parent().map(|p| p.to_path_buf()),
    })
}

fn parse_frontmatter(content: &str) -> Option<(HashMap<String, String>, String)> {
    let content = content.trim();
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end = rest.find("---")?;
    let fm = &rest[..end];
    let body = &rest[end + 3..];
    let mut map = HashMap::new();
    for line in fm.lines() {
        if let Some((k, v)) = line.split_once(':') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    Some((map, body.trim().to_string()))
}
