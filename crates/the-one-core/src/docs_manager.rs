use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocEntry {
    pub path: String, // relative to managed_root
    pub size_bytes: u64,
    pub modified_epoch_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport {
    pub new: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
}

pub struct DocsManager {
    managed_root: PathBuf, // <project>/.the-one/docs/
    trash_root: PathBuf,   // <project>/.the-one/docs/.trash/
}

impl DocsManager {
    pub fn new(project_root: &Path) -> Result<Self, CoreError> {
        let managed_root = project_root.join(".the-one").join("docs");
        let trash_root = managed_root.join(".trash");
        fs::create_dir_all(&managed_root)?;
        fs::create_dir_all(&trash_root)?;
        Ok(Self {
            managed_root,
            trash_root,
        })
    }

    pub fn managed_root(&self) -> &Path {
        &self.managed_root
    }

    /// Create a new markdown file. Fails if file already exists.
    pub fn create(
        &self,
        relative_path: &str,
        content: &str,
        max_doc_bytes: usize,
        max_docs: usize,
    ) -> Result<PathBuf, CoreError> {
        Self::validate_path(relative_path)?;
        Self::validate_doc_size(content, max_doc_bytes)?;
        self.validate_doc_count(max_docs)?;

        let full_path = self.managed_root.join(relative_path);
        if full_path.exists() {
            return Err(CoreError::Document(format!(
                "file already exists: {relative_path}"
            )));
        }
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&full_path, content)?;
        Ok(full_path)
    }

    /// Update an existing markdown file. Fails if file doesn't exist.
    pub fn update(
        &self,
        relative_path: &str,
        content: &str,
        max_doc_bytes: usize,
    ) -> Result<PathBuf, CoreError> {
        Self::validate_path(relative_path)?;
        Self::validate_doc_size(content, max_doc_bytes)?;

        let full_path = self.managed_root.join(relative_path);
        if !full_path.exists() {
            return Err(CoreError::Document(format!(
                "file not found: {relative_path}"
            )));
        }
        fs::write(&full_path, content)?;
        Ok(full_path)
    }

    /// Soft-delete: move file to .trash/ preserving directory structure.
    pub fn delete(&self, relative_path: &str) -> Result<(), CoreError> {
        Self::validate_path(relative_path)?;
        let full_path = self.managed_root.join(relative_path);
        if !full_path.exists() {
            return Err(CoreError::Document(format!(
                "file not found: {relative_path}"
            )));
        }
        let trash_path = self.trash_root.join(relative_path);
        if let Some(parent) = trash_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&full_path, &trash_path)?;
        // Clean up empty parent directories in managed root
        self.cleanup_empty_dirs(relative_path);
        Ok(())
    }

    /// Get full content of a file.
    pub fn get(&self, relative_path: &str) -> Result<String, CoreError> {
        Self::validate_path(relative_path)?;
        let full_path = self.managed_root.join(relative_path);
        if !full_path.exists() {
            return Err(CoreError::Document(format!(
                "file not found: {relative_path}"
            )));
        }
        Ok(fs::read_to_string(&full_path)?)
    }

    /// Get a specific section by heading, bounded by max_bytes.
    pub fn get_section(
        &self,
        relative_path: &str,
        heading: &str,
        max_bytes: usize,
    ) -> Result<Option<String>, CoreError> {
        let content = self.get(relative_path)?;
        // Find section by heading
        let mut in_section = false;
        let mut section_lines: Vec<&str> = Vec::new();
        let mut section_level = 0;

        for line in content.lines() {
            if let Some((level, title)) = parse_heading(line) {
                if title == heading {
                    in_section = true;
                    section_level = level;
                    section_lines.push(line);
                    continue;
                }
                if in_section && level <= section_level {
                    break; // Next heading at same or higher level
                }
            }
            if in_section {
                section_lines.push(line);
            }
        }

        if section_lines.is_empty() {
            return Ok(None);
        }
        let section = section_lines.join("\n");
        if section.len() > max_bytes {
            Ok(Some(section[..max_bytes].to_string()))
        } else {
            Ok(Some(section))
        }
    }

    /// List all managed documents.
    pub fn list(&self) -> Result<Vec<DocEntry>, CoreError> {
        let mut entries = Vec::new();
        self.walk_dir(&self.managed_root, &self.managed_root, &mut entries)?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    /// Move/rename a document within managed folder.
    pub fn move_doc(&self, from: &str, to: &str) -> Result<(), CoreError> {
        Self::validate_path(from)?;
        Self::validate_path(to)?;
        let from_path = self.managed_root.join(from);
        let to_path = self.managed_root.join(to);
        if !from_path.exists() {
            return Err(CoreError::Document(format!("source not found: {from}")));
        }
        if to_path.exists() {
            return Err(CoreError::Document(format!(
                "destination already exists: {to}"
            )));
        }
        if let Some(parent) = to_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&from_path, &to_path)?;
        self.cleanup_empty_dirs(from);
        Ok(())
    }

    /// List contents of trash.
    pub fn trash_list(&self) -> Result<Vec<DocEntry>, CoreError> {
        let mut entries = Vec::new();
        self.walk_dir(&self.trash_root, &self.trash_root, &mut entries)?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    /// Restore a file from trash to its original location.
    pub fn trash_restore(&self, relative_path: &str) -> Result<(), CoreError> {
        Self::validate_path(relative_path)?;
        let trash_path = self.trash_root.join(relative_path);
        let restore_path = self.managed_root.join(relative_path);
        if !trash_path.exists() {
            return Err(CoreError::Document(format!(
                "not in trash: {relative_path}"
            )));
        }
        if restore_path.exists() {
            return Err(CoreError::Document(format!(
                "restore target already exists: {relative_path}"
            )));
        }
        if let Some(parent) = restore_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&trash_path, &restore_path)?;
        Ok(())
    }

    /// Permanently delete all files in trash.
    pub fn trash_empty(&self) -> Result<(), CoreError> {
        if self.trash_root.exists() {
            fs::remove_dir_all(&self.trash_root)?;
            fs::create_dir_all(&self.trash_root)?;
        }
        Ok(())
    }

    // --- Validation ---

    fn validate_path(relative_path: &str) -> Result<(), CoreError> {
        if relative_path.is_empty() {
            return Err(CoreError::Document("path cannot be empty".to_string()));
        }
        if relative_path.contains("..") {
            return Err(CoreError::Document(
                "path traversal not allowed".to_string(),
            ));
        }
        if !relative_path.ends_with(".md") {
            return Err(CoreError::Document("only .md files allowed".to_string()));
        }
        // Only allow alphanumeric, hyphen, underscore, dot, forward slash
        for c in relative_path.chars() {
            if !c.is_alphanumeric() && c != '-' && c != '_' && c != '.' && c != '/' {
                return Err(CoreError::Document(format!(
                    "invalid character in path: {c}"
                )));
            }
        }
        Ok(())
    }

    fn validate_doc_size(content: &str, max_bytes: usize) -> Result<(), CoreError> {
        if content.len() > max_bytes {
            return Err(CoreError::Document(format!(
                "document size {} exceeds limit {max_bytes}",
                content.len()
            )));
        }
        Ok(())
    }

    fn validate_doc_count(&self, max_docs: usize) -> Result<(), CoreError> {
        let entries = self.list()?;
        if entries.len() >= max_docs {
            return Err(CoreError::Document(format!(
                "document count {} would exceed limit {max_docs}",
                entries.len() + 1
            )));
        }
        Ok(())
    }

    fn cleanup_empty_dirs(&self, relative_path: &str) {
        // Walk up from the file's parent, removing empty directories
        let mut path = self.managed_root.join(relative_path);
        path.pop(); // Remove filename
        while path > self.managed_root {
            if path == self.trash_root {
                break;
            }
            if fs::read_dir(&path)
                .map(|mut d| d.next().is_none())
                .unwrap_or(true)
            {
                let _ = fs::remove_dir(&path);
                path.pop();
            } else {
                break;
            }
        }
    }

    fn walk_dir(
        &self,
        dir: &Path,
        root: &Path,
        entries: &mut Vec<DocEntry>,
    ) -> Result<(), CoreError> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                // Skip .trash when walking managed root
                if path.file_name().is_some_and(|n| n == ".trash") {
                    continue;
                }
                self.walk_dir(&path, root, entries)?;
            } else if path.extension().is_some_and(|ext| ext == "md") {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|e| CoreError::Document(format!("path strip error: {e}")))?
                    .to_string_lossy()
                    .to_string();
                let metadata = fs::metadata(&path)?;
                let modified = metadata
                    .modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH)
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                entries.push(DocEntry {
                    path: relative,
                    size_bytes: metadata.len(),
                    modified_epoch_ms: modified,
                });
            }
        }
        Ok(())
    }
}

fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return None;
    }
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let title = rest.trim().to_string();
    if title.is_empty() {
        return None;
    }
    Some((hashes, title))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const MAX_DOC_BYTES: usize = 1_000_000;
    const MAX_DOCS: usize = 100;

    fn setup() -> (tempfile::TempDir, DocsManager) {
        let dir = tempdir().unwrap();
        let mgr = DocsManager::new(dir.path()).unwrap();
        (dir, mgr)
    }

    #[test]
    fn test_create_and_get() {
        let (_dir, mgr) = setup();
        let content = "# Hello\nWorld";
        mgr.create("hello.md", content, MAX_DOC_BYTES, MAX_DOCS)
            .unwrap();
        let got = mgr.get("hello.md").unwrap();
        assert_eq!(got, content);
    }

    #[test]
    fn test_create_with_subdirectory() {
        let (_dir, mgr) = setup();
        let content = "# Nested";
        let path = mgr
            .create("sub/dir/file.md", content, MAX_DOC_BYTES, MAX_DOCS)
            .unwrap();
        assert!(path.exists());
        assert_eq!(mgr.get("sub/dir/file.md").unwrap(), content);
    }

    #[test]
    fn test_create_duplicate_fails() {
        let (_dir, mgr) = setup();
        mgr.create("dup.md", "first", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap();
        let err = mgr
            .create("dup.md", "second", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn test_update_existing() {
        let (_dir, mgr) = setup();
        mgr.create("up.md", "v1", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap();
        mgr.update("up.md", "v2", MAX_DOC_BYTES).unwrap();
        assert_eq!(mgr.get("up.md").unwrap(), "v2");
    }

    #[test]
    fn test_update_nonexistent_fails() {
        let (_dir, mgr) = setup();
        let err = mgr
            .update("nonexistent.md", "content", MAX_DOC_BYTES)
            .unwrap_err();
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn test_delete_and_trash() {
        let (_dir, mgr) = setup();
        mgr.create("del.md", "bye", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap();
        mgr.delete("del.md").unwrap();

        // Gone from main list
        let list = mgr.list().unwrap();
        assert!(list.iter().all(|e| e.path != "del.md"));

        // Present in trash
        let trash = mgr.trash_list().unwrap();
        assert!(trash.iter().any(|e| e.path == "del.md"));
    }

    #[test]
    fn test_trash_restore() {
        let (_dir, mgr) = setup();
        mgr.create("rest.md", "restore me", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap();
        mgr.delete("rest.md").unwrap();
        mgr.trash_restore("rest.md").unwrap();

        assert_eq!(mgr.get("rest.md").unwrap(), "restore me");
        let trash = mgr.trash_list().unwrap();
        assert!(trash.iter().all(|e| e.path != "rest.md"));
    }

    #[test]
    fn test_trash_empty() {
        let (_dir, mgr) = setup();
        mgr.create("a.md", "a", MAX_DOC_BYTES, MAX_DOCS).unwrap();
        mgr.create("b.md", "b", MAX_DOC_BYTES, MAX_DOCS).unwrap();
        mgr.delete("a.md").unwrap();
        mgr.delete("b.md").unwrap();

        assert_eq!(mgr.trash_list().unwrap().len(), 2);
        mgr.trash_empty().unwrap();
        assert_eq!(mgr.trash_list().unwrap().len(), 0);
    }

    #[test]
    fn test_path_traversal_rejected() {
        let (_dir, mgr) = setup();
        let err = mgr
            .create("../escape.md", "bad", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap_err();
        assert!(err.to_string().contains("path traversal"));

        let err = mgr
            .create("foo/../../escape.md", "bad", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap_err();
        assert!(err.to_string().contains("path traversal"));
    }

    #[test]
    fn test_non_md_rejected() {
        let (_dir, mgr) = setup();
        let err = mgr
            .create("file.txt", "nope", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap_err();
        assert!(err.to_string().contains("only .md files allowed"));

        let err = mgr
            .create("file", "nope", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap_err();
        assert!(err.to_string().contains("only .md files allowed"));
    }

    #[test]
    fn test_move_doc() {
        let (_dir, mgr) = setup();
        mgr.create("old.md", "moving", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap();
        mgr.move_doc("old.md", "new.md").unwrap();

        assert!(mgr.get("old.md").is_err());
        assert_eq!(mgr.get("new.md").unwrap(), "moving");
    }

    #[test]
    fn test_list_returns_sorted() {
        let (_dir, mgr) = setup();
        mgr.create("charlie.md", "c", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap();
        mgr.create("alpha.md", "a", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap();
        mgr.create("bravo.md", "b", MAX_DOC_BYTES, MAX_DOCS)
            .unwrap();

        let list = mgr.list().unwrap();
        let names: Vec<&str> = list.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, vec!["alpha.md", "bravo.md", "charlie.md"]);
    }

    #[test]
    fn test_doc_size_limit() {
        let (_dir, mgr) = setup();
        let small_limit = 10;
        let big_content = "a".repeat(20);
        let err = mgr
            .create("big.md", &big_content, small_limit, MAX_DOCS)
            .unwrap_err();
        assert!(err.to_string().contains("exceeds limit"));
    }

    #[test]
    fn test_doc_count_limit() {
        let (_dir, mgr) = setup();
        mgr.create("one.md", "1", MAX_DOC_BYTES, 2).unwrap();
        mgr.create("two.md", "2", MAX_DOC_BYTES, 2).unwrap();
        let err = mgr
            .create("three.md", "3", MAX_DOC_BYTES, 2)
            .unwrap_err();
        assert!(err.to_string().contains("exceed limit"));
    }

    #[test]
    fn test_get_section() {
        let (_dir, mgr) = setup();
        let content = "# Intro\nHello world\n## Details\nSome details here\nMore details\n## Other\nOther stuff";
        mgr.create("sections.md", content, MAX_DOC_BYTES, MAX_DOCS)
            .unwrap();

        // Get the Details section
        let section = mgr
            .get_section("sections.md", "Details", MAX_DOC_BYTES)
            .unwrap();
        assert!(section.is_some());
        let section = section.unwrap();
        assert!(section.contains("## Details"));
        assert!(section.contains("Some details here"));
        assert!(section.contains("More details"));
        // Should NOT contain the next section
        assert!(!section.contains("Other stuff"));

        // Non-existent section
        let none = mgr
            .get_section("sections.md", "NonExistent", MAX_DOC_BYTES)
            .unwrap();
        assert!(none.is_none());
    }
}
