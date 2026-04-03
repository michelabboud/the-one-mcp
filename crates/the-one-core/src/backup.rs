use std::fs;
use std::path::{Path, PathBuf};

use crate::error::CoreError;

#[derive(Debug, Clone)]
pub struct BackupResult {
    pub sqlite_backup_path: PathBuf,
    pub qdrant_backup_path: PathBuf,
}

pub fn backup_project_state(
    project_root: &Path,
    backup_root: &Path,
) -> Result<BackupResult, CoreError> {
    let state_dir = project_root.join(".the-one");
    if !state_dir.exists() {
        return Err(CoreError::InvalidProjectConfig(format!(
            "project state directory missing: {}",
            state_dir.display()
        )));
    }

    fs::create_dir_all(backup_root)?;

    let sqlite_src = state_dir.join("state.db");
    let sqlite_dst = backup_root.join("state.db.bak");
    if sqlite_src.exists() {
        fs::copy(&sqlite_src, &sqlite_dst)?;
    } else {
        fs::write(&sqlite_dst, b"")?;
    }

    let qdrant_src = state_dir.join("qdrant");
    let qdrant_dst = backup_root.join("qdrant.bak");
    if qdrant_src.exists() {
        copy_dir_recursive(&qdrant_src, &qdrant_dst)?;
    } else {
        fs::create_dir_all(&qdrant_dst)?;
    }

    Ok(BackupResult {
        sqlite_backup_path: sqlite_dst,
        qdrant_backup_path: qdrant_dst,
    })
}

pub fn restore_project_state(project_root: &Path, backup_root: &Path) -> Result<(), CoreError> {
    let state_dir = project_root.join(".the-one");
    fs::create_dir_all(&state_dir)?;

    let sqlite_src = backup_root.join("state.db.bak");
    let sqlite_dst = state_dir.join("state.db");
    let qdrant_src = backup_root.join("qdrant.bak");
    if !sqlite_src.exists() && !qdrant_src.exists() {
        return Err(CoreError::InvalidProjectConfig(format!(
            "backup artifacts missing in {}",
            backup_root.display()
        )));
    }

    if sqlite_src.exists() {
        fs::copy(&sqlite_src, &sqlite_dst)?;
    }

    let qdrant_dst = state_dir.join("qdrant");
    if qdrant_src.exists() {
        if qdrant_dst.exists() {
            fs::remove_dir_all(&qdrant_dst)?;
        }
        copy_dir_recursive(&qdrant_src, &qdrant_dst)?;
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), CoreError> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dst_path)?;
        } else {
            fs::copy(&path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{backup_project_state, restore_project_state};

    #[test]
    fn test_manual_backup_copies_sqlite_and_qdrant_tree() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        let state = project.join(".the-one");
        let qdrant = state.join("qdrant");
        let backup = temp.path().join("backup");

        fs::create_dir_all(&qdrant).expect("qdrant dir should exist");
        fs::write(state.join("state.db"), "db").expect("db write should succeed");
        fs::write(qdrant.join("segment.bin"), "qdrant-data").expect("qdrant write should succeed");

        let result = backup_project_state(&project, &backup).expect("backup should succeed");
        assert!(result.sqlite_backup_path.exists());
        assert!(result.qdrant_backup_path.join("segment.bin").exists());
    }

    #[test]
    fn test_manual_restore_recovers_sqlite_and_qdrant_tree() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        let state = project.join(".the-one");
        let qdrant = state.join("qdrant");
        let backup = temp.path().join("backup");

        fs::create_dir_all(&qdrant).expect("qdrant dir should exist");
        fs::write(state.join("state.db"), "db-v1").expect("db write should succeed");
        fs::write(qdrant.join("segment.bin"), "qdrant-v1").expect("qdrant write should succeed");
        backup_project_state(&project, &backup).expect("backup should succeed");

        fs::write(state.join("state.db"), "db-v2").expect("db write should succeed");
        fs::write(qdrant.join("segment.bin"), "qdrant-v2").expect("qdrant write should succeed");

        restore_project_state(&project, &backup).expect("restore should succeed");
        let restored_db = fs::read_to_string(state.join("state.db")).expect("db should exist");
        let restored_qdrant =
            fs::read_to_string(qdrant.join("segment.bin")).expect("segment should exist");
        assert_eq!(restored_db, "db-v1");
        assert_eq!(restored_qdrant, "qdrant-v1");
    }

    #[test]
    fn test_restore_fails_when_backup_artifacts_are_missing() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let project = temp.path().join("repo");
        let backup = temp.path().join("backup-empty");
        fs::create_dir_all(&backup).expect("backup dir should exist");

        let err = restore_project_state(&project, &backup).expect_err("restore should fail");
        assert!(err.to_string().contains("backup artifacts missing"));
    }
}
