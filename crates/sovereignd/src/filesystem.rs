use crate::transit::{TransitBuffer, TransitError};
use crate::workspace::SovereignPaths;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::task::JoinError;
use tokio::time::{sleep, Duration};

const MAX_TEXT_FILE_BYTES: u64 = 1024 * 1024;
const WATCH_POLL_INTERVAL_MS: u64 = 2000;

#[derive(Debug)]
pub enum FileWatchError {
    Io(std::io::Error),
    Join(JoinError),
    Transit(TransitError),
}

impl fmt::Display for FileWatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Join(error) => write!(f, "{error}"),
            Self::Transit(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for FileWatchError {}

impl From<std::io::Error> for FileWatchError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<TransitError> for FileWatchError {
    fn from(value: TransitError) -> Self {
        Self::Transit(value)
    }
}

impl From<JoinError> for FileWatchError {
    fn from(value: JoinError) -> Self {
        Self::Join(value)
    }
}

#[derive(Debug, Clone)]
pub struct FileWatcherConfig {
    pub root: PathBuf,
    pub recursive: bool,
}

pub struct FileWatcher {
    paths: SovereignPaths,
    config: FileWatcherConfig,
}

impl FileWatcher {
    pub fn new(paths: SovereignPaths, config: FileWatcherConfig) -> Self {
        Self { paths, config }
    }

    pub async fn run(self) -> Result<(), FileWatchError> {
        println!(
            "polling {} for local text captures",
            self.config.root.display()
        );

        let mut seen = HashMap::new();
        let paths = self.paths;
        let root = self.config.root;
        let recursive = self.config.recursive;

        loop {
            let scan_paths = paths.clone();
            let scan_root = root.clone();

            seen = tokio::task::spawn_blocking(move || {
                let mut scan_seen = seen;
                scan_once(&scan_paths, &scan_root, recursive, &mut scan_seen)?;
                Ok::<_, FileWatchError>(scan_seen)
            })
            .await??;

            sleep(Duration::from_millis(WATCH_POLL_INTERVAL_MS)).await;
        }
    }
}

fn scan_once(
    paths: &SovereignPaths,
    root: &Path,
    recursive: bool,
    seen: &mut HashMap<PathBuf, FileFingerprint>,
) -> Result<(), FileWatchError> {
    let buffer = TransitBuffer::open(paths)?;
    scan_path(&buffer, root, recursive, seen)
}

fn scan_path(
    buffer: &TransitBuffer,
    root: &Path,
    recursive: bool,
    seen: &mut HashMap<PathBuf, FileFingerprint>,
) -> Result<(), FileWatchError> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;

        if metadata.is_dir() && recursive {
            scan_path(buffer, &path, recursive, seen)?;
        } else if metadata.is_file() {
            ingest_file_if_text(
                buffer,
                &path,
                metadata.len(),
                metadata.modified().ok(),
                seen,
            )?;
        }
    }

    Ok(())
}

fn ingest_file_if_text(
    buffer: &TransitBuffer,
    path: &Path,
    len: u64,
    modified: Option<SystemTime>,
    seen: &mut HashMap<PathBuf, FileFingerprint>,
) -> Result<(), FileWatchError> {
    if !is_supported_text_path(path) || len > MAX_TEXT_FILE_BYTES {
        return Ok(());
    }

    let fingerprint = FileFingerprint { len, modified };

    if seen.get(path) == Some(&fingerprint) {
        return Ok(());
    }

    let content = fs::read_to_string(path)?;
    let cleaned = collapse_whitespace(&content);

    if cleaned.is_empty() {
        return Ok(());
    }

    let source = format!("filesystem:{}", path.display());
    let id = buffer.ingest_text(&source, &cleaned)?;
    seen.insert(path.to_path_buf(), fingerprint);

    println!("queued filesystem capture #{id} from {}", path.display());
    Ok(())
}

#[inline]
pub fn is_supported_text_path(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };

    matches!(
        extension.to_ascii_lowercase().as_str(),
        "txt"
            | "md"
            | "markdown"
            | "html"
            | "htm"
            | "rs"
            | "toml"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
    )
}

#[inline]
fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
}

#[cfg(test)]
mod tests {
    use super::is_supported_text_path;
    use std::path::Path;

    #[test]
    fn recognizes_supported_text_extensions_case_insensitively() {
        assert!(is_supported_text_path(Path::new("notes.MD")));
        assert!(is_supported_text_path(Path::new("page.HTML")));
        assert!(is_supported_text_path(Path::new("lib.rs")));
        assert!(!is_supported_text_path(Path::new("data.json")));
        assert!(!is_supported_text_path(Path::new("query.sql")));
        assert!(!is_supported_text_path(Path::new("events.log")));
        assert!(!is_supported_text_path(Path::new("photo.png")));
        assert!(!is_supported_text_path(Path::new("no-extension")));
    }
}
