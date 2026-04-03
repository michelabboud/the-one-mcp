use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::contracts::{ProjectProfile, RiskProfile};
use crate::error::CoreError;

const SIGNAL_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
    "pyproject.toml",
    "requirements.txt",
    "go.mod",
    "Dockerfile",
    "docker-compose.yml",
];

pub fn detect_profile(project_root: &Path, project_id: &str) -> Result<ProjectProfile, CoreError> {
    let mut languages = BTreeSet::new();
    let mut frameworks = BTreeSet::new();

    if project_root.join("Cargo.toml").exists() {
        languages.insert("rust".to_string());
        let cargo = fs::read_to_string(project_root.join("Cargo.toml"))?;
        if cargo.contains("axum") {
            frameworks.insert("axum".to_string());
        }
        if cargo.contains("tokio") {
            frameworks.insert("tokio".to_string());
        }
    }

    if project_root.join("package.json").exists() {
        languages.insert("javascript".to_string());
    }
    if project_root.join("pyproject.toml").exists()
        || project_root.join("requirements.txt").exists()
    {
        languages.insert("python".to_string());
    }
    if project_root.join("go.mod").exists() {
        languages.insert("go".to_string());
    }

    if project_root.join(".github/workflows").exists() {
        frameworks.insert("github-actions".to_string());
    }
    if project_root.join("Dockerfile").exists() || project_root.join("docker-compose.yml").exists()
    {
        frameworks.insert("docker".to_string());
    }

    let risk_profile = if project_root.join("terraform").exists()
        || project_root.join(".github/workflows/deploy.yml").exists()
    {
        RiskProfile::HighRisk
    } else if project_root.join("Dockerfile").exists() {
        RiskProfile::Caution
    } else {
        RiskProfile::Safe
    };

    Ok(ProjectProfile {
        project_id: project_id.to_string(),
        project_root: project_root.display().to_string(),
        languages: languages.into_iter().collect(),
        frameworks: frameworks.into_iter().collect(),
        risk_profile,
    })
}

pub fn compute_fingerprint(project_root: &Path) -> Result<String, CoreError> {
    let mut hasher = Sha256::new();

    for signal in SIGNAL_FILES {
        let path = project_root.join(signal);
        if path.exists() {
            hasher.update(signal.as_bytes());
            let metadata = fs::metadata(&path)?;
            hasher.update(metadata.len().to_le_bytes());
            if let Ok(content) = fs::read(&path) {
                hasher.update(content);
            }
        }
    }

    if project_root.join(".github/workflows").exists() {
        hasher.update(b".github/workflows");
    }

    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{compute_fingerprint, detect_profile};

    #[test]
    fn test_fingerprint_changes_with_signal_file_updates() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("root should be created");
        fs::write(root.join("Cargo.toml"), "[package]\nname='a'\n")
            .expect("cargo write should succeed");

        let initial = compute_fingerprint(&root).expect("first fingerprint");
        fs::write(root.join("Cargo.toml"), "[package]\nname='b'\n")
            .expect("cargo write should succeed");
        let changed = compute_fingerprint(&root).expect("second fingerprint");

        assert_ne!(initial, changed);
    }

    #[test]
    fn test_profile_detects_rust_and_docker() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path().join("repo");
        fs::create_dir_all(&root).expect("root should be created");
        fs::write(root.join("Cargo.toml"), "[package]\nname='x'\n")
            .expect("cargo write should succeed");
        fs::write(root.join("Dockerfile"), "FROM rust:latest")
            .expect("docker write should succeed");

        let profile = detect_profile(&root, "project-a").expect("profile should be detected");
        assert!(profile.languages.contains(&"rust".to_string()));
        assert!(profile.frameworks.contains(&"docker".to_string()));
    }
}
