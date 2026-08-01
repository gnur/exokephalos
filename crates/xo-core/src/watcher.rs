//! Recursive, debounced Markdown projection watching.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ProjectionEvent {
    Upsert(PathBuf),
    Remove(PathBuf),
}

impl ProjectionEvent {
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Upsert(path) | Self::Remove(path) => path,
        }
    }
}

#[derive(Debug, Error)]
pub enum WatchError {
    #[error("filesystem watcher error: {0}")]
    Notify(#[from] notify::Error),
    #[error("filesystem watcher stopped unexpectedly")]
    Disconnected,
}

/// A recursive native watcher that collapses all events received within a quiet period.
pub struct DebouncedWatcher {
    root: PathBuf,
    debounce: Duration,
    receiver: Receiver<notify::Result<Event>>,
    _watcher: RecommendedWatcher,
}

impl std::fmt::Debug for DebouncedWatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DebouncedWatcher")
            .field("root", &self.root)
            .field("debounce", &self.debounce)
            .finish_non_exhaustive()
    }
}

impl DebouncedWatcher {
    pub fn new(root: impl AsRef<Path>, debounce: Duration) -> Result<Self, WatchError> {
        let root = root.as_ref().canonicalize().map_err(notify::Error::io)?;
        let (sender, receiver) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(sender)?;
        watcher.watch(&root, RecursiveMode::Recursive)?;
        Ok(Self {
            root,
            debounce,
            receiver,
            _watcher: watcher,
        })
    }

    /// Wait for an initial event, then return Markdown paths once the stream is quiet.
    pub fn recv_timeout(&self, initial_wait: Duration) -> Result<Vec<ProjectionEvent>, WatchError> {
        let first = match self.receiver.recv_timeout(initial_wait) {
            Ok(event) => event?,
            Err(RecvTimeoutError::Timeout) => return Ok(Vec::new()),
            Err(RecvTimeoutError::Disconnected) => return Err(WatchError::Disconnected),
        };
        let mut paths = first.paths;
        loop {
            match self.receiver.recv_timeout(self.debounce) {
                Ok(event) => paths.extend(event?.paths),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return Err(WatchError::Disconnected),
            }
        }
        Ok(normalize_paths(&self.root, paths))
    }
}

fn normalize_paths(root: &Path, paths: Vec<PathBuf>) -> Vec<ProjectionEvent> {
    let candidates = paths
        .into_iter()
        .filter(|path| is_projection_path(root, path))
        .collect::<BTreeSet<_>>();
    let mut upserts = Vec::new();
    let mut removals = Vec::new();
    for path in candidates {
        if path.is_file() {
            upserts.push(ProjectionEvent::Upsert(path));
        } else {
            removals.push(ProjectionEvent::Remove(path));
        }
    }
    upserts.extend(removals);
    upserts
}

fn is_projection_path(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let relative_string = relative.to_string_lossy().replace('\\', "/");
    let content_path = path.extension().is_some_and(|extension| extension == "md")
        || relative_string == "xo.scm"
        || ((relative_string.starts_with("modules/") || relative_string.starts_with("plugins/"))
            && Path::new(&relative_string)
                .extension()
                .is_some_and(|value| value == "scm"));
    content_path
        && relative
            .components()
            .next()
            .is_none_or(|component| component.as_os_str() != "assets")
        && !relative.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| name.starts_with('.'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_filesystem_state_coalesces_rename_and_ignores_hidden_paths() {
        let directory = tempfile::tempdir().unwrap();
        let old = directory.path().join("notes/old.md");
        let new = directory.path().join("notes/new.md");
        let hidden = directory.path().join(".xo/state.md");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::write(&new, "new").unwrap();
        assert_eq!(
            normalize_paths(directory.path(), vec![old.clone(), new.clone(), hidden]),
            vec![ProjectionEvent::Upsert(new), ProjectionEvent::Remove(old)]
        );
    }

    #[test]
    fn watcher_observes_recursive_markdown_write() {
        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("deep");
        std::fs::create_dir_all(&parent).unwrap();
        let watcher = DebouncedWatcher::new(directory.path(), Duration::from_millis(50)).unwrap();
        let path = parent.join("note.md");
        std::fs::write(&path, "body").unwrap();
        let path = path.canonicalize().unwrap();

        let mut observed = Vec::new();
        for _ in 0..5 {
            observed.extend(watcher.recv_timeout(Duration::from_secs(1)).unwrap());
            if observed.contains(&ProjectionEvent::Upsert(path.clone())) {
                break;
            }
        }
        assert!(observed.contains(&ProjectionEvent::Upsert(path)));
    }

    #[test]
    fn watcher_includes_only_supported_steel_configuration_paths() {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("xo.scm");
        let unsupported_main = directory.path().join("unsupported.scm");
        let module = directory.path().join("modules/views/books.scm");
        let plugin = directory.path().join("plugins/hardcover.scm");
        let unrelated = directory.path().join("script.scm");
        std::fs::create_dir_all(module.parent().unwrap()).unwrap();
        std::fs::create_dir_all(plugin.parent().unwrap()).unwrap();
        std::fs::write(&main, "main").unwrap();
        std::fs::write(&unsupported_main, "unsupported main").unwrap();
        std::fs::write(&module, "module").unwrap();
        std::fs::write(&plugin, "plugin").unwrap();
        std::fs::write(&unrelated, "no").unwrap();
        let events = normalize_paths(
            directory.path(),
            vec![
                main.clone(),
                unsupported_main,
                module.clone(),
                plugin.clone(),
                unrelated,
            ],
        );
        assert_eq!(
            events,
            vec![
                ProjectionEvent::Upsert(module),
                ProjectionEvent::Upsert(plugin),
                ProjectionEvent::Upsert(main),
            ]
        );
    }
}
