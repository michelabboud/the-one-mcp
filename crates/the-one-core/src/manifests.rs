use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

pub const MANIFEST_SCHEMA_VERSION: &str = "1";

#[derive(Debug, Clone)]
pub struct ProjectStatePaths {
    pub state_dir: PathBuf,
    pub project_json: PathBuf,
    pub overrides_json: PathBuf,
    pub fingerprint_json: PathBuf,
    pub pointers_json: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectManifest {
    pub schema_version: String,
    pub project_id: String,
    pub initialized_at_epoch_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OverridesManifest {
    pub schema_version: String,
    pub enabled_families: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FingerprintManifest {
    pub schema_version: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PointersManifest {
    pub schema_version: String,
    pub sqlite_db: String,
    pub rag_location: String,
}

pub fn initialize_project_state(
    project_root: &Path,
    project_id: &str,
) -> Result<ProjectStatePaths, CoreError> {
    if !project_root.exists() || !project_root.is_dir() {
        return Err(CoreError::InvalidProjectConfig(format!(
            "project root is invalid: {}",
            project_root.display()
        )));
    }

    let state_dir = project_root.join(".the-one");
    fs::create_dir_all(&state_dir)?;

    let paths = project_state_paths(project_root);

    write_json_if_missing(
        &paths.project_json,
        &ProjectManifest {
            schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
            project_id: project_id.to_string(),
            initialized_at_epoch_ms: epoch_ms(),
        },
    )?;
    write_json_if_missing(
        &paths.overrides_json,
        &OverridesManifest {
            schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
            enabled_families: Vec::new(),
        },
    )?;
    write_json_if_missing(
        &paths.fingerprint_json,
        &FingerprintManifest {
            schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
            fingerprint: "unset".to_string(),
        },
    )?;
    write_json_if_missing(
        &paths.pointers_json,
        &PointersManifest {
            schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
            sqlite_db: ".the-one/state.db".to_string(),
            rag_location: ".the-one/qdrant".to_string(),
        },
    )?;

    Ok(paths)
}

pub fn project_state_paths(project_root: &Path) -> ProjectStatePaths {
    let state_dir = project_root.join(".the-one");
    ProjectStatePaths {
        state_dir: state_dir.clone(),
        project_json: state_dir.join("project.json"),
        overrides_json: state_dir.join("overrides.json"),
        fingerprint_json: state_dir.join("fingerprint.json"),
        pointers_json: state_dir.join("pointers.json"),
    }
}

pub fn load_project_manifest(path: &Path) -> Result<ProjectManifest, CoreError> {
    let manifest: ProjectManifest = read_json(path)?;
    ensure_schema_version(&manifest.schema_version)?;
    Ok(manifest)
}

pub fn save_project_manifest(path: &Path, manifest: &ProjectManifest) -> Result<(), CoreError> {
    ensure_schema_version(&manifest.schema_version)?;
    write_json_atomic(path, manifest)
}

pub fn load_overrides_manifest(path: &Path) -> Result<OverridesManifest, CoreError> {
    let manifest: OverridesManifest = read_json(path)?;
    ensure_schema_version(&manifest.schema_version)?;
    Ok(manifest)
}

pub fn save_overrides_manifest(path: &Path, manifest: &OverridesManifest) -> Result<(), CoreError> {
    ensure_schema_version(&manifest.schema_version)?;
    write_json_atomic(path, manifest)
}

pub fn load_fingerprint_manifest(path: &Path) -> Result<FingerprintManifest, CoreError> {
    let manifest: FingerprintManifest = read_json(path)?;
    ensure_schema_version(&manifest.schema_version)?;
    Ok(manifest)
}

pub fn save_fingerprint_manifest(
    path: &Path,
    manifest: &FingerprintManifest,
) -> Result<(), CoreError> {
    ensure_schema_version(&manifest.schema_version)?;
    write_json_atomic(path, manifest)
}

pub fn load_pointers_manifest(path: &Path) -> Result<PointersManifest, CoreError> {
    let manifest: PointersManifest = read_json(path)?;
    ensure_schema_version(&manifest.schema_version)?;
    Ok(manifest)
}

pub fn save_pointers_manifest(path: &Path, manifest: &PointersManifest) -> Result<(), CoreError> {
    ensure_schema_version(&manifest.schema_version)?;
    write_json_atomic(path, manifest)
}

fn ensure_schema_version(version: &str) -> Result<(), CoreError> {
    if version == MANIFEST_SCHEMA_VERSION {
        return Ok(());
    }

    Err(CoreError::UnsupportedSchemaVersion(version.to_string()))
}

fn write_json_if_missing<T: Serialize>(path: &Path, value: &T) -> Result<(), CoreError> {
    if path.exists() {
        return Ok(());
    }

    write_json_atomic(path, value)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, CoreError> {
    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content)?)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), CoreError> {
    let parent = path.parent().ok_or_else(|| {
        CoreError::InvalidProjectConfig(format!("manifest has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent)?;

    let tmp_name = format!(
        "{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("manifest"),
        std::process::id(),
        epoch_ms(),
    );
    let tmp_path = parent.join(tmp_name);

    let payload = serde_json::to_vec_pretty(value)?;
    fs::write(&tmp_path, payload)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        initialize_project_state, load_project_manifest, project_state_paths,
        save_project_manifest, ProjectManifest, MANIFEST_SCHEMA_VERSION,
    };

    #[test]
    fn test_isolation_between_projects() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_a = temp.path().join("repo-a");
        let project_b = temp.path().join("repo-b");

        fs::create_dir_all(&project_a).expect("project a dir should exist");
        fs::create_dir_all(&project_b).expect("project b dir should exist");

        let paths_a = initialize_project_state(&project_a, "project-a").expect("init a");
        let paths_b = initialize_project_state(&project_b, "project-b").expect("init b");

        let manifest_a = load_project_manifest(&paths_a.project_json).expect("load a");
        let manifest_b = load_project_manifest(&paths_b.project_json).expect("load b");

        assert_eq!(manifest_a.project_id, "project-a");
        assert_eq!(manifest_b.project_id, "project-b");
        assert_ne!(paths_a.project_json, paths_b.project_json);
    }

    #[test]
    fn test_atomic_manifest_write_replaces_payload() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");
        initialize_project_state(&project_root, "project-1").expect("init state");

        let paths = project_state_paths(&project_root);
        let updated = ProjectManifest {
            schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
            project_id: "project-1-updated".to_string(),
            initialized_at_epoch_ms: 42,
        };

        save_project_manifest(&paths.project_json, &updated).expect("save should succeed");
        let loaded = load_project_manifest(&paths.project_json).expect("load should succeed");
        assert_eq!(loaded.project_id, "project-1-updated");
        assert_eq!(loaded.initialized_at_epoch_ms, 42);
    }

    #[test]
    fn test_schema_version_mismatch_is_rejected() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");
        let paths = initialize_project_state(&project_root, "project-1").expect("init state");

        fs::write(
            &paths.project_json,
            r#"{"schema_version":"999","project_id":"x","initialized_at_epoch_ms":1}"#,
        )
        .expect("write should succeed");

        let err = load_project_manifest(&paths.project_json).expect_err("must fail");
        assert!(err.to_string().contains("unsupported schema version"));
    }
}
