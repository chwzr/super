//! Write tool: create a file, or overwrite one the session has read whole.
//!
//! Rules:
//! - `file_path` is absolute;
//! - missing parent folders are created, and a new file needs no read;
//! - overwriting a file needs a read of the whole file in one call; a
//!   partial read is not enough;
//! - the write is refused when the file changed on disk since that read,
//!   unless its content is still what was read;
//! - a folder, or anything that is not a regular file, is refused.

use crate::placeholder;
use crate::tool::{AgentTool, DynTool, ToolResult, ToolSession, ToolUpdateSink};
use crate::tools::files::{record_own_change, unchanged};
use crate::tools::mutation_queue::lock_path;
use crate::tools::path::{absolute, file_key};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use super_ai::ContentBlock;
use tokio_util::sync::CancellationToken;

pub struct WriteTool {
    /// The session's folder, used only to suggest an absolute path.
    pub cwd: PathBuf,
    /// The session this tool writes for; [`AgentTool::for_session`] binds it.
    pub session: ToolSession,
}

impl WriteTool {
    /// A Write tool with a file record of its own.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            session: ToolSession::new(),
        }
    }

    /// Refuse to overwrite `path` unless the session read the whole file and
    /// it is still what was read.
    async fn check_overwrite(
        &self,
        path: &Path,
        key: &Path,
        meta: &std::fs::Metadata,
    ) -> anyhow::Result<()> {
        let Some(read) = self.session.files.get(key).filter(|read| read.whole) else {
            anyhow::bail!(
                "{} exists and has not been read whole; read the whole file with Read before you overwrite it.",
                path.display()
            );
        };
        if read.same_stamp(meta.modified().ok(), meta.len()) {
            return Ok(());
        }
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|error| not_writable(path, error))?;
        if !unchanged(&read, meta, &bytes) {
            anyhow::bail!(
                "{} has changed on disk since it was read; read it again before you overwrite it.",
                path.display()
            );
        }
        Ok(())
    }
}

fn not_writable(path: &Path, error: std::io::Error) -> anyhow::Error {
    anyhow::anyhow!("Could not write {}: {error}.", path.display())
}

#[async_trait::async_trait]
impl AgentTool for WriteTool {
    fn name(&self) -> &str {
        "Write"
    }

    fn description(&self) -> String {
        placeholder::tool_description("Write")
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": placeholder::tool_parameter("Write", "file_path"),
                },
                "content": {
                    "type": "string",
                    "description": placeholder::tool_parameter("Write", "content"),
                },
            },
            "required": ["file_path", "content"],
            "additionalProperties": false,
        })
    }

    fn for_session(&self, session: &ToolSession) -> Option<DynTool> {
        Some(Arc::new(WriteTool {
            cwd: self.cwd.clone(),
            session: session.clone(),
        }))
    }

    fn in_directory(&self, cwd: &std::path::Path) -> Option<DynTool> {
        Some(Arc::new(WriteTool {
            cwd: cwd.to_path_buf(),
            session: self.session.clone(),
        }))
    }

    async fn execute(
        &self,
        _id: &str,
        args: Value,
        cancel: CancellationToken,
        _on_update: Option<ToolUpdateSink>,
    ) -> anyhow::Result<ToolResult> {
        let path = absolute(&self.cwd, args["file_path"].as_str().unwrap_or_default())?;
        let content = args["content"].as_str().unwrap_or_default();
        let key = file_key(&path).await;
        let _guard = lock_path(&key).await;
        if cancel.is_cancelled() {
            anyhow::bail!("Operation aborted");
        }

        let created = match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.is_dir() => anyhow::bail!(
                "{} is a folder, not a file; give the path of a file to write.",
                path.display()
            ),
            Ok(meta) if !meta.is_file() => anyhow::bail!(
                "{} is not a regular file; Write writes regular files only.",
                path.display()
            ),
            Ok(meta) => {
                self.check_overwrite(&path, &key, &meta).await?;
                false
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|error| {
                        anyhow::anyhow!(
                            "Could not create the folders for {}: {error}.",
                            path.display()
                        )
                    })?;
                }
                true
            }
            Err(error) => return Err(not_writable(&path, error)),
        };
        if cancel.is_cancelled() {
            anyhow::bail!("Operation aborted");
        }
        tokio::fs::write(&path, content)
            .await
            .map_err(|error| not_writable(&path, error))?;
        record_own_change(&self.session.files, &path, content.as_bytes()).await;

        let bytes = content.len();
        Ok(ToolResult {
            content: vec![ContentBlock::text(format!(
                "Wrote {bytes} bytes to {}.",
                path.display()
            ))],
            details: json!({"created": created, "bytes": bytes}),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::read::ReadTool;

    struct Files {
        dir: tempfile::TempDir,
        session: ToolSession,
    }

    impl Files {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                session: ToolSession::new(),
            }
        }

        fn path(&self, name: &str) -> String {
            self.dir.path().join(name).display().to_string()
        }

        fn create(&self, name: &str, content: &str) -> String {
            std::fs::write(self.dir.path().join(name), content).unwrap();
            self.path(name)
        }

        fn content(&self, name: &str) -> String {
            std::fs::read_to_string(self.dir.path().join(name)).unwrap()
        }

        async fn read(&self, args: Value) {
            ReadTool::new(self.dir.path().to_path_buf())
                .for_session(&self.session)
                .unwrap()
                .execute("r", args, CancellationToken::new(), None)
                .await
                .unwrap();
        }

        async fn write(&self, args: Value) -> anyhow::Result<ToolResult> {
            let tool = WriteTool::new(self.dir.path().to_path_buf())
                .for_session(&self.session)
                .unwrap();
            crate::validate::validate_arguments(&tool.parameters(), &args)
                .map_err(anyhow::Error::msg)?;
            tool.execute("w", args, CancellationToken::new(), None)
                .await
        }
    }

    #[tokio::test]
    async fn a_new_file_needs_no_read_and_gets_its_folders() {
        let files = Files::new();
        let path = files.path("a/b/c.txt");
        let result = files
            .write(json!({"file_path": path, "content": "hello"}))
            .await
            .unwrap();
        assert!(result.output_text().contains("5 bytes"));
        assert_eq!(result.details["created"], true);
        assert_eq!(files.content("a/b/c.txt"), "hello");
        // The session wrote it, so it may overwrite it without a read.
        files
            .write(json!({"file_path": path, "content": "again"}))
            .await
            .unwrap();
        assert_eq!(files.content("a/b/c.txt"), "again");
    }

    #[tokio::test]
    async fn overwriting_needs_a_read_of_the_whole_file() {
        let files = Files::new();
        let path = files.create("f.txt", "one\ntwo\n");
        let error = files
            .write(json!({"file_path": path, "content": "new"}))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("has not been read whole"),
            "{error}"
        );

        files.read(json!({"file_path": path, "limit": 1})).await;
        let error = files
            .write(json!({"file_path": path, "content": "new"}))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("has not been read whole"),
            "{error}"
        );
        assert_eq!(files.content("f.txt"), "one\ntwo\n");

        files.read(json!({"file_path": path})).await;
        files
            .write(json!({"file_path": path, "content": "new"}))
            .await
            .unwrap();
        assert_eq!(files.content("f.txt"), "new");
    }

    #[tokio::test]
    async fn a_file_changed_since_the_read_is_refused_unless_its_content_is_the_same() {
        let files = Files::new();
        let path = files.create("f.txt", "first\n");
        files.read(json!({"file_path": path})).await;
        std::fs::write(&path, "second\n").unwrap();
        let error = files
            .write(json!({"file_path": path, "content": "third"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("changed on disk"), "{error}");
        assert_eq!(files.content("f.txt"), "second\n");

        // Back to what was read: only its time differs.
        files.read(json!({"file_path": path})).await;
        let key = file_key(Path::new(&path)).await;
        let mut read = files.session.files.get(&key).unwrap();
        read.modified = None;
        files.session.files.record(key, read);
        files
            .write(json!({"file_path": path, "content": "third"}))
            .await
            .unwrap();
        assert_eq!(files.content("f.txt"), "third");
    }

    #[tokio::test]
    async fn a_folder_is_refused() {
        let files = Files::new();
        std::fs::create_dir(files.dir.path().join("sub")).unwrap();
        let error = files
            .write(json!({"file_path": files.path("sub"), "content": "x"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("folder"), "{error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_file_that_is_not_regular_is_refused() {
        let error = Files::new()
            .write(json!({"file_path": "/dev/null", "content": "x"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not a regular file"), "{error}");
    }

    #[tokio::test]
    async fn relative_paths_and_the_old_parameter_name_are_refused() {
        let files = Files::new();
        let error = files
            .write(json!({"file_path": "f.txt", "content": "x"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("absolute"), "{error}");
        let error = files
            .write(json!({"path": files.path("f.txt"), "content": "x"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("file_path"), "{error}");
        assert!(!files.dir.path().join("f.txt").exists());
    }

    #[tokio::test]
    async fn description_and_parameters_are_placeholders() {
        let tool = WriteTool::new("/".into());
        assert_eq!(tool.description(), placeholder::tool_description("Write"));
        let parameters = tool.parameters();
        for name in ["file_path", "content"] {
            assert_eq!(
                parameters["properties"][name]["description"],
                placeholder::tool_parameter("Write", name)
            );
        }
    }
}
