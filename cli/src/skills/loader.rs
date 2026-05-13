// Skill struct used by the tools module and skill loader
#[derive(Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub base_directory: Option<std::path::PathBuf>,
}

// Skill loader — will be implemented in a later task