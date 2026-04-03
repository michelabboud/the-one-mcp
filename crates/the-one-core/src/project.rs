use std::fs;
use std::path::Path;

use crate::config::{AppConfig, RuntimeOverrides};
use crate::contracts::{ProjectProfile, RiskLevel};
use crate::error::CoreError;
use crate::manifests::{
    initialize_project_state, load_fingerprint_manifest, load_overrides_manifest,
    load_project_manifest, save_fingerprint_manifest, FingerprintManifest, OverridesManifest,
    MANIFEST_SCHEMA_VERSION,
};
use crate::profiler::{compute_fingerprint, detect_profile};
use crate::storage::sqlite::ProjectDatabase;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshMode {
    ReusedCachedProfile,
    RecomputedProfile,
}

#[derive(Debug, Clone)]
pub struct InitResult {
    pub project_id: String,
    pub profile: ProjectProfile,
    pub fingerprint: String,
    pub db_path: String,
}

#[derive(Debug, Clone)]
pub struct RefreshResult {
    pub project_id: String,
    pub mode: RefreshMode,
    pub profile: ProjectProfile,
    pub fingerprint: String,
}

pub fn project_init(project_root: &Path, project_id: &str) -> Result<InitResult, CoreError> {
    let _config = AppConfig::load(project_root, RuntimeOverrides::default())?;
    let state_paths = initialize_project_state(project_root, project_id)?;
    let project_manifest = load_project_manifest(&state_paths.project_json)?;
    if project_manifest.project_id != project_id {
        return Err(CoreError::InvalidProjectConfig(format!(
            "project id mismatch for root {}: expected '{}', found '{}'",
            project_root.display(),
            project_id,
            project_manifest.project_id
        )));
    }
    let db = ProjectDatabase::open(project_root, project_id)?;

    let profile = detect_profile(project_root, project_id)?;
    let profile_json = serde_json::to_string(&profile)?;
    db.upsert_project_profile(&profile_json)?;

    let fingerprint = compute_fingerprint(project_root)?;
    save_fingerprint_manifest(
        &state_paths.fingerprint_json,
        &FingerprintManifest {
            schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
            fingerprint: fingerprint.clone(),
        },
    )?;

    if !state_paths.overrides_json.exists() {
        let overrides = OverridesManifest {
            schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
            enabled_families: Vec::new(),
        };
        let content = serde_json::to_vec_pretty(&overrides)?;
        fs::write(&state_paths.overrides_json, content)?;
    }

    Ok(InitResult {
        project_id: project_id.to_string(),
        profile,
        fingerprint,
        db_path: db.db_path().display().to_string(),
    })
}

pub fn project_refresh(project_root: &Path, project_id: &str) -> Result<RefreshResult, CoreError> {
    let state_paths = initialize_project_state(project_root, project_id)?;
    let project_manifest = load_project_manifest(&state_paths.project_json)?;
    if project_manifest.project_id != project_id {
        return Err(CoreError::InvalidProjectConfig(format!(
            "project id mismatch for root {}: expected '{}', found '{}'",
            project_root.display(),
            project_id,
            project_manifest.project_id
        )));
    }
    let db = ProjectDatabase::open(project_root, project_id)?;

    let previous = load_fingerprint_manifest(&state_paths.fingerprint_json)?.fingerprint;
    let current = compute_fingerprint(project_root)?;

    let profile = if previous == current {
        let cached = db
            .latest_project_profile()?
            .ok_or_else(|| CoreError::InvalidProjectConfig("missing cached profile".to_string()))?;

        RefreshResult {
            project_id: project_id.to_string(),
            mode: RefreshMode::ReusedCachedProfile,
            profile: serde_json::from_str::<ProjectProfile>(&cached)?,
            fingerprint: current,
        }
    } else {
        let profile = detect_profile(project_root, project_id)?;
        db.upsert_project_profile(&serde_json::to_string(&profile)?)?;
        save_fingerprint_manifest(
            &state_paths.fingerprint_json,
            &FingerprintManifest {
                schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
                fingerprint: current.clone(),
            },
        )?;

        RefreshResult {
            project_id: project_id.to_string(),
            mode: RefreshMode::RecomputedProfile,
            profile,
            fingerprint: current,
        }
    };

    Ok(profile)
}

pub fn effective_risk_level(project_root: &Path) -> Result<RiskLevel, CoreError> {
    let paths = crate::manifests::project_state_paths(project_root);
    let overrides = load_overrides_manifest(&paths.overrides_json)?;
    if overrides
        .enabled_families
        .iter()
        .any(|family| family == "unsafe-tools")
    {
        return Ok(RiskLevel::High);
    }
    Ok(RiskLevel::Medium)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{project_init, project_refresh, RefreshMode};

    #[test]
    fn test_refresh_reuses_cached_profile_when_fingerprint_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");
        fs::write(project_root.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo file write should succeed");

        project_init(&project_root, "project-x").expect("init should succeed");
        let refresh = project_refresh(&project_root, "project-x").expect("refresh should succeed");
        assert_eq!(refresh.mode, RefreshMode::ReusedCachedProfile);
    }

    #[test]
    fn test_refresh_recomputes_after_signal_change() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");
        fs::write(project_root.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo file write should succeed");

        project_init(&project_root, "project-x").expect("init should succeed");
        fs::write(project_root.join("Cargo.toml"), "[package]\nname='y'\n")
            .expect("cargo file write should succeed");
        let refresh = project_refresh(&project_root, "project-x").expect("refresh should succeed");
        assert_eq!(refresh.mode, RefreshMode::RecomputedProfile);
    }

    #[test]
    fn test_project_id_mismatch_is_rejected_for_existing_state() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project_root = temp.path().join("repo");
        fs::create_dir_all(&project_root).expect("project dir should exist");
        fs::write(project_root.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo file write should succeed");

        project_init(&project_root, "project-a").expect("init should succeed");
        let err = project_refresh(&project_root, "project-b").expect_err("must fail");
        assert!(err.to_string().contains("project id mismatch"));
    }
}
