//! File watcher for incremental indexing.
//!
//! When `auto_index_enabled: true`, the broker spawns a background tokio task
//! per project that watches `.the-one/docs/` and `.the-one/images/` for
//! changes and queues reindex operations.

use notify_debouncer_mini::{
    new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
    Debouncer,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// An event emitted when a watched file changes.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// File was created or modified.
    Upserted(PathBuf),
    /// File was removed.
    Removed(PathBuf),
}

/// Default debounce window.
pub const DEFAULT_DEBOUNCE_MS: u64 = 2000;

/// Default file extensions to watch (markdown + images).
pub const DEFAULT_WATCHED_EXTENSIONS: &[&str] = &["md", "png", "jpg", "jpeg", "webp"];

/// Check whether a path has one of the watched extensions (case-insensitive).
pub fn is_watched(path: &Path, extensions: &[&str]) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext_lower = ext.to_ascii_lowercase();
    extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext_lower))
}

/// Spawn a file watcher that sends events to an async mpsc channel.
///
/// Returns:
/// - the receiver side of the async channel for [`WatchEvent`]s
/// - a [`CancellationToken`] to signal shutdown
/// - the [`Debouncer`] handle — the caller **must** keep this alive for as
///   long as watching is desired; dropping it stops the underlying watcher
///
/// # Errors
///
/// Returns an error string if notify fails to initialize or watch a path.
pub fn spawn_watcher(
    paths: Vec<PathBuf>,
    extensions: Vec<String>,
    debounce_ms: u64,
) -> Result<
    (
        mpsc::UnboundedReceiver<WatchEvent>,
        CancellationToken,
        Debouncer<RecommendedWatcher>,
    ),
    String,
> {
    let (async_tx, async_rx) = mpsc::unbounded_channel::<WatchEvent>();
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Synchronous mpsc channel to bridge notify callback → our thread
    let (sync_tx, sync_rx) = channel::<notify_debouncer_mini::DebounceEventResult>();

    let mut debouncer = new_debouncer(Duration::from_millis(debounce_ms), move |res| {
        let _ = sync_tx.send(res);
    })
    .map_err(|e| format!("notify init failed: {e}"))?;

    for path in &paths {
        if path.exists() {
            debouncer
                .watcher()
                .watch(path, RecursiveMode::Recursive)
                .map_err(|e| format!("notify watch failed for {}: {e}", path.display()))?;
        }
    }

    // Bridge sync → async channel on a dedicated blocking thread
    std::thread::spawn(move || {
        for result in sync_rx {
            if cancel_clone.is_cancelled() {
                break;
            }
            match result {
                Ok(events) => {
                    for event in events {
                        let path = event.path;
                        let ext_refs: Vec<&str> =
                            extensions.iter().map(String::as_str).collect();
                        if !is_watched(&path, &ext_refs) {
                            continue;
                        }
                        // Heuristic: path still exists → upsert; otherwise → removal
                        let ev = if path.exists() {
                            WatchEvent::Upserted(path)
                        } else {
                            WatchEvent::Removed(path)
                        };
                        if async_tx.send(ev).is_err() {
                            // Receiver dropped — shut down
                            return;
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!("watcher error: {err}");
                }
            }
        }
    });

    Ok((async_rx, cancel, debouncer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_is_watched_matches_extensions() {
        let exts = &["md", "png"];
        assert!(is_watched(Path::new("a.md"), exts));
        assert!(is_watched(Path::new("a.PNG"), exts));
        assert!(!is_watched(Path::new("a.txt"), exts));
        assert!(!is_watched(Path::new("a"), exts));
    }

    #[tokio::test]
    async fn test_watcher_detects_file_create() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut rx, cancel, _debouncer) = spawn_watcher(
            vec![tmp.path().to_path_buf()],
            vec!["md".to_string()],
            100, // fast debounce for tests
        )
        .expect("spawn watcher");

        // Small delay to let inotify set up before creating the file
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Create a markdown file
        let test_file = tmp.path().join("test.md");
        fs::write(&test_file, "hello").expect("write");

        // Wait up to 3 seconds for the event
        let result = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;

        cancel.cancel();

        match result {
            Ok(Some(WatchEvent::Upserted(path))) => {
                assert_eq!(
                    path.file_name().and_then(|n| n.to_str()),
                    Some("test.md")
                );
            }
            other => panic!("expected upsert event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_watcher_ignores_unwatched_extensions() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut rx, cancel, _debouncer) = spawn_watcher(
            vec![tmp.path().to_path_buf()],
            vec!["md".to_string()],
            100,
        )
        .expect("spawn watcher");

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Create a .txt file (not watched)
        fs::write(tmp.path().join("test.txt"), "hello").expect("write");

        let result = tokio::time::timeout(Duration::from_millis(600), rx.recv()).await;

        cancel.cancel();

        // Expect timeout (no event)
        assert!(result.is_err(), "expected no event for unwatched extension");
    }

    #[tokio::test]
    async fn test_watcher_skips_nonexistent_path() {
        // A path that doesn't exist should be skipped (no error, no watch)
        let tmp = tempfile::tempdir().expect("tempdir");
        let nonexistent = tmp.path().join("does-not-exist");
        let result = spawn_watcher(
            vec![nonexistent],
            vec!["md".to_string()],
            100,
        );
        // Should succeed (nonexistent paths are silently skipped)
        assert!(result.is_ok(), "should not error on missing path");
    }
}
