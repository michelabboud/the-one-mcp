//! MCP Resources API support.
//!
//! The MCP protocol supports three primitives: tools, resources, and prompts.
//! This module implements the resources primitive — first-class, client-browsable
//! references to indexed content.
//!
//! # URI scheme
//!
//! All resources use the `the-one://` scheme with the form
//! `the-one://<resource_type>/<identifier>`. Supported types:
//!
//! - `docs` — a file under `<project>/.the-one/docs/`, identifier is the
//!   relative path (no `..` allowed)
//! - `project` — identifier is `profile`; returns the project profile JSON
//! - `catalog` — identifier is `enabled`; returns the enabled tool list for
//!   this project from the SQLite catalog
//!
//! # Example URIs
//!
//! - `the-one://docs/architecture.md`
//! - `the-one://project/profile`
//! - `the-one://catalog/enabled`
//!
//! # Security
//!
//! Path traversal is forbidden: any `docs` identifier containing `..` or an
//! absolute path is rejected before hitting the filesystem.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use the_one_core::error::CoreError;
use the_one_core::tool_catalog::ToolCatalog;

/// The URI scheme prefix for all the-one-mcp resources.
pub const RESOURCE_SCHEME: &str = "the-one://";

/// A resource entry as returned by `resources/list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

/// The response payload for `resources/list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourcesListResponse {
    pub resources: Vec<McpResource>,
}

/// The request payload for `resources/read`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourcesReadRequest {
    pub project_root: String,
    pub project_id: String,
    pub uri: String,
}

/// A single content block returned for a resource read. Currently always
/// text; binary content may be added in a future version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceContent {
    pub uri: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    pub text: String,
}

/// The response payload for `resources/read`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourcesReadResponse {
    pub contents: Vec<ResourceContent>,
}

/// Parse a `the-one://<type>/<identifier>` URI into its components.
///
/// Returns `None` if the scheme is wrong or the structure is malformed.
pub fn parse_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix(RESOURCE_SCHEME)?;
    let (res_type, identifier) = rest.split_once('/')?;
    if res_type.is_empty() || identifier.is_empty() {
        return None;
    }
    Some((res_type.to_string(), identifier.to_string()))
}

/// Reject obviously unsafe `docs` identifiers (path traversal, absolute paths,
/// NUL bytes).
pub fn is_safe_doc_identifier(identifier: &str) -> bool {
    if identifier.is_empty() {
        return false;
    }
    if identifier.contains("..") {
        return false;
    }
    if identifier.contains('\0') {
        return false;
    }
    let p = Path::new(identifier);
    if p.is_absolute() {
        return false;
    }
    // Reject any component starting with a tilde (home expansion attempt) or
    // containing drive letter patterns like C: on Windows paths.
    for component in p.components() {
        if let std::path::Component::Normal(n) = component {
            let s = n.to_string_lossy();
            if s.starts_with('~') {
                return false;
            }
        } else {
            // RootDir / ParentDir / Prefix — all unsafe.
            return false;
        }
    }
    true
}

/// Walk `<project>/.the-one/docs/` recursively and return one `McpResource`
/// entry per file, plus the `project/profile` and `catalog/enabled` defaults.
pub fn list_resources(project_root: &Path) -> Result<Vec<McpResource>, CoreError> {
    let mut resources = Vec::new();

    // Managed docs
    let docs_dir = project_root.join(".the-one").join("docs");
    if docs_dir.exists() {
        walk_docs_dir(&docs_dir, &docs_dir, &mut resources);
    }

    // Project profile
    resources.push(McpResource {
        uri: format!("{RESOURCE_SCHEME}project/profile"),
        name: "Project profile".to_string(),
        description: Some(
            "Profile metadata for this project (languages, frameworks, tests, commands)"
                .to_string(),
        ),
        mime_type: Some("application/json".to_string()),
    });

    // Enabled tool catalog
    resources.push(McpResource {
        uri: format!("{RESOURCE_SCHEME}catalog/enabled"),
        name: "Enabled tools".to_string(),
        description: Some(
            "Tools from the global catalog that are enabled for this project".to_string(),
        ),
        mime_type: Some("application/json".to_string()),
    });

    Ok(resources)
}

fn walk_docs_dir(root: &Path, current: &Path, out: &mut Vec<McpResource>) {
    let Ok(rd) = std::fs::read_dir(current) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_docs_dir(root, &path, out);
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative_str = relative.to_string_lossy().replace('\\', "/");
        // Skip the legacy trash subdir
        if relative_str.starts_with(".trash/") {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let mime = match ext {
            "md" | "markdown" => "text/markdown",
            "json" => "application/json",
            "txt" => "text/plain",
            _ => "application/octet-stream",
        };
        out.push(McpResource {
            uri: format!("{RESOURCE_SCHEME}docs/{relative_str}"),
            name: relative_str.clone(),
            description: Some(format!("Managed doc: {relative_str}")),
            mime_type: Some(mime.to_string()),
        });
    }
}

/// Resolve a resource URI and return its content.
///
/// Returns `CoreError::InvalidRequest` for unknown resource types or unsafe
/// identifiers, and `CoreError::Io` for missing files.
pub fn read_resource(project_root: &Path, uri: &str) -> Result<ResourcesReadResponse, CoreError> {
    let (res_type, identifier) = parse_uri(uri).ok_or_else(|| {
        CoreError::InvalidProjectConfig(format!("unrecognized resource uri: {uri}"))
    })?;

    match res_type.as_str() {
        "docs" => read_docs_resource(project_root, &identifier, uri),
        "project" => read_project_resource(project_root, &identifier, uri),
        "catalog" => read_catalog_resource(project_root, &identifier, uri),
        _ => Err(CoreError::InvalidProjectConfig(format!(
            "unknown resource type: {res_type}"
        ))),
    }
}

fn read_docs_resource(
    project_root: &Path,
    identifier: &str,
    uri: &str,
) -> Result<ResourcesReadResponse, CoreError> {
    if !is_safe_doc_identifier(identifier) {
        return Err(CoreError::InvalidProjectConfig(format!(
            "unsafe or invalid docs identifier: {identifier}"
        )));
    }
    let full_path: PathBuf = project_root.join(".the-one").join("docs").join(identifier);
    let text = std::fs::read_to_string(&full_path)?;
    let mime = if identifier.ends_with(".md") || identifier.ends_with(".markdown") {
        "text/markdown"
    } else if identifier.ends_with(".json") {
        "application/json"
    } else {
        "text/plain"
    };
    Ok(ResourcesReadResponse {
        contents: vec![ResourceContent {
            uri: uri.to_string(),
            mime_type: mime.to_string(),
            text,
        }],
    })
}

fn read_project_resource(
    project_root: &Path,
    identifier: &str,
    uri: &str,
) -> Result<ResourcesReadResponse, CoreError> {
    if identifier != "profile" {
        return Err(CoreError::InvalidProjectConfig(format!(
            "unknown project resource: {identifier}"
        )));
    }
    let profile_path = project_root
        .join(".the-one")
        .join("manifests")
        .join("profile.json");
    let text = if profile_path.exists() {
        std::fs::read_to_string(&profile_path)?
    } else {
        // Return an empty object rather than failing — the profile may not
        // have been generated yet for a freshly-initialized project.
        "{}".to_string()
    };
    Ok(ResourcesReadResponse {
        contents: vec![ResourceContent {
            uri: uri.to_string(),
            mime_type: "application/json".to_string(),
            text,
        }],
    })
}

fn read_catalog_resource(
    project_root: &Path,
    identifier: &str,
    uri: &str,
) -> Result<ResourcesReadResponse, CoreError> {
    if identifier != "enabled" {
        return Err(CoreError::InvalidProjectConfig(format!(
            "unknown catalog resource: {identifier}"
        )));
    }
    let catalog_dir = project_root.join(".the-one").join("catalog");
    let catalog = ToolCatalog::open(&catalog_dir)?;
    let canonical_root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .display()
        .to_string();
    let literal_root = project_root.display().to_string();
    let mut enabled = catalog.list_enabled_tools_for_project(&canonical_root)?;
    if canonical_root != literal_root {
        for tool_id in catalog.list_enabled_tools_for_project(&literal_root)? {
            if !enabled.iter().any(|existing| existing == &tool_id) {
                enabled.push(tool_id);
            }
        }
        enabled.sort();
    }

    Ok(ResourcesReadResponse {
        contents: vec![ResourceContent {
            uri: uri.to_string(),
            mime_type: "application/json".to_string(),
            text: serde_json::to_string(&enabled).map_err(|err| {
                CoreError::InvalidProjectConfig(format!(
                    "failed to serialize enabled tool list: {err}"
                ))
            })?,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_uri_valid() {
        assert_eq!(
            parse_uri("the-one://docs/foo/bar.md"),
            Some(("docs".to_string(), "foo/bar.md".to_string()))
        );
        assert_eq!(
            parse_uri("the-one://project/profile"),
            Some(("project".to_string(), "profile".to_string()))
        );
    }

    #[test]
    fn test_parse_uri_rejects_bad_scheme() {
        assert!(parse_uri("https://example.com/foo").is_none());
        assert!(parse_uri("the-two://docs/foo").is_none());
        assert!(parse_uri("the-one://docs/").is_none());
        assert!(parse_uri("the-one:///foo").is_none());
    }

    #[test]
    fn test_is_safe_doc_identifier() {
        assert!(is_safe_doc_identifier("foo.md"));
        assert!(is_safe_doc_identifier("subdir/foo.md"));
        assert!(is_safe_doc_identifier("a/b/c.md"));

        assert!(!is_safe_doc_identifier(""));
        assert!(!is_safe_doc_identifier("../etc/passwd"));
        assert!(!is_safe_doc_identifier("subdir/../../etc/passwd"));
        assert!(!is_safe_doc_identifier("/etc/passwd"));
        assert!(!is_safe_doc_identifier("foo\0bar"));
        assert!(!is_safe_doc_identifier("~/.ssh/id_rsa"));
    }

    #[test]
    fn test_list_resources_empty_project_returns_defaults() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let resources = list_resources(&root).expect("list");
        // project/profile + catalog/enabled
        assert!(resources.len() >= 2);
        assert!(resources
            .iter()
            .any(|r| r.uri == "the-one://project/profile"));
        assert!(resources
            .iter()
            .any(|r| r.uri == "the-one://catalog/enabled"));
    }

    #[test]
    fn test_list_resources_with_managed_docs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let docs_dir = root.join(".the-one").join("docs");
        fs::create_dir_all(&docs_dir).expect("docs dir");
        fs::write(docs_dir.join("alpha.md"), "# Alpha\n").expect("alpha");
        fs::create_dir_all(docs_dir.join("sub")).expect("sub");
        fs::write(docs_dir.join("sub/beta.md"), "# Beta\n").expect("beta");

        let resources = list_resources(&root).expect("list");
        assert!(resources.iter().any(|r| r.uri == "the-one://docs/alpha.md"));
        assert!(resources
            .iter()
            .any(|r| r.uri == "the-one://docs/sub/beta.md"));
    }

    #[test]
    fn test_read_resource_managed_doc() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let docs_dir = root.join(".the-one").join("docs");
        fs::create_dir_all(&docs_dir).expect("docs dir");
        fs::write(docs_dir.join("notes.md"), "# Notes\nbody").expect("notes");

        let resp = read_resource(&root, "the-one://docs/notes.md").expect("read");
        assert_eq!(resp.contents.len(), 1);
        assert_eq!(resp.contents[0].mime_type, "text/markdown");
        assert!(resp.contents[0].text.contains("Notes"));
    }

    #[test]
    fn test_read_resource_rejects_path_traversal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let result = read_resource(&root, "the-one://docs/../../etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_resource_unknown_type_errors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let result = read_resource(&root, "the-one://unknown/foo");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_resource_project_profile_missing_returns_empty_object() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let resp = read_resource(&root, "the-one://project/profile").expect("read");
        assert_eq!(resp.contents[0].text, "{}");
    }

    #[test]
    fn test_read_resource_catalog_enabled_returns_empty_array() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let resp = read_resource(&root, "the-one://catalog/enabled").expect("read");
        assert_eq!(resp.contents[0].text, "[]");
    }

    #[test]
    fn test_read_resource_catalog_enabled_returns_enabled_tools() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().to_path_buf();
        let catalog_dir = root.join(".the-one").join("catalog");
        let catalog = ToolCatalog::open(&catalog_dir).expect("catalog");
        catalog
            .enable_tool("cargo-audit", "codex", &root.display().to_string())
            .expect("enable tool");

        let resp = read_resource(&root, "the-one://catalog/enabled").expect("read");
        assert!(
            resp.contents[0].text.contains("cargo-audit"),
            "enabled tools response should include persisted tool ids"
        );
    }
}
