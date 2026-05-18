use std::path::Path;

#[derive(Clone)]
pub struct SystemPrompt {
    pub sections: Vec<String>,
}

impl SystemPrompt {
    pub fn build(cwd: &Path) -> Self {
        let mut sections = Vec::new();

        // Load CLAUDE.md from cwd and parent directories
        if let Some(content) = load_claude_md(cwd) {
            sections.push(format!("<claude-md>\n{content}\n</claude-md>"));
        }

        Self { sections }
    }

    pub fn add_section(&mut self, section: String) {
        self.sections.push(section);
    }

    pub fn render(&self) -> String {
        self.sections.join("\n\n")
    }
}

fn load_claude_md(cwd: &Path) -> Option<String> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let path = d.join("CLAUDE.md");
        if path.exists() {
            return std::fs::read_to_string(&path).ok();
        }
        dir = d.parent();
    }
    None
}
