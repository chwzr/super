//! Extracts Superpowers skills embedded at compile time into
//! ~/.super/plugins/superpowers/ and parses them as Skill objects.

include!(concat!(env!("OUT_DIR"), "/bundled_gen.rs"));

use super::loader::{LoadedFrom, Skill};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Extract all bundled skills to `~/.super/plugins/superpowers/` (idempotent)
/// and return them as `Skill` objects. Extraction failures are logged and
/// skipped — the skill is still usable from in-memory content but
/// companion files (visual-companion.md etc.) won't be accessible via Read.
pub fn extract_bundled_skills() -> Vec<Skill> {
    let root = match bundled_root() {
        Some(r) => r,
        None => return parse_in_memory_only(),
    };

    let mut skills = Vec::new();
    for def in BUNDLED_SKILLS {
        let skill_dir = root.join(def.name);
        let base = match extract_skill(def, &skill_dir) {
            Ok(()) => Some(skill_dir),
            Err(e) => {
                eprintln!(
                    "[super] warning: could not extract skill '{}': {e}",
                    def.name
                );
                None
            }
        };
        if let Some(skill) = skill_from_def(def, base) {
            skills.push(skill);
        }
    }
    skills
}

fn bundled_root() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".super").join("plugins").join("superpowers"))
}

fn extract_skill(def: &BundledSkillDef, skill_dir: &Path) -> std::io::Result<()> {
    for file in def.files {
        let target = skill_dir.join(file.rel_path);
        if target.exists() {
            continue; // idempotent fast path
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(mut fh) => {
                if let Err(e) = fh.write_all(file.content.as_bytes()) {
                    let _ = std::fs::remove_file(&target); // best-effort cleanup on partial write
                    return Err(e);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn skill_from_def(def: &BundledSkillDef, base_directory: Option<PathBuf>) -> Option<Skill> {
    let skill_content = def.files.iter().find(|f| f.rel_path == "SKILL.md")?.content;
    super::loader::parse_skill_str(skill_content, def.name, base_directory, LoadedFrom::Bundled)
}

fn parse_in_memory_only() -> Vec<Skill> {
    BUNDLED_SKILLS
        .iter()
        .filter_map(|def| skill_from_def(def, None))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skills_constant_is_non_empty() {
        assert!(
            !BUNDLED_SKILLS.is_empty(),
            "BUNDLED_SKILLS should contain at least one skill"
        );
    }

    #[test]
    fn every_bundled_skill_has_skill_md() {
        for def in BUNDLED_SKILLS {
            let has_skill_md = def.files.iter().any(|f| f.rel_path == "SKILL.md");
            assert!(has_skill_md, "skill '{}' is missing SKILL.md", def.name);
        }
    }

    #[test]
    fn every_bundled_skill_parses_to_non_empty_name() {
        for def in BUNDLED_SKILLS {
            let skill = skill_from_def(def, None);
            assert!(skill.is_some(), "skill '{}' failed to parse", def.name);
            assert!(
                !skill.unwrap().name.is_empty(),
                "skill '{}' parsed with empty name",
                def.name
            );
        }
    }

    #[test]
    fn using_superpowers_is_not_user_invocable() {
        let skill = BUNDLED_SKILLS
            .iter()
            .find(|d| d.name == "using-superpowers")
            .and_then(|d| skill_from_def(d, None));
        let skill = skill.expect("using-superpowers not found in BUNDLED_SKILLS");
        assert!(
            !skill.user_invocable,
            "using-superpowers should have user-invocable: false"
        );
    }

    #[test]
    fn extract_skill_is_idempotent() {
        // Pick the first bundled skill and extract it twice into a tempdir.
        // Second extraction must not error and must not duplicate files.
        let def = BUNDLED_SKILLS.first().expect("BUNDLED_SKILLS empty");
        let tmp = std::env::temp_dir().join(format!(
            "super_extract_idemp_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let skill_dir = tmp.join(def.name);

        extract_skill(def, &skill_dir).expect("first extraction failed");
        let first_count: usize = walkdir::WalkDir::new(&skill_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .count();
        assert!(first_count > 0, "no files extracted");

        // Second call must succeed and not change the file count.
        extract_skill(def, &skill_dir).expect("second extraction errored");
        let second_count: usize = walkdir::WalkDir::new(&skill_dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .count();
        assert_eq!(first_count, second_count, "file count changed");

        std::fs::remove_dir_all(&tmp).ok();
    }
}
