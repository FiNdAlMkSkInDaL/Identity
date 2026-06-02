use crate::transit::{TransitBuffer, TransitError};
use crate::workspace::IdentityPaths;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::SystemTime;
use tokio::task::JoinError;
use tokio::time::{sleep, Duration};

const MAX_TEXT_FILE_BYTES: u64 = 1024 * 1024;
const WATCH_POLL_INTERVAL_MS: u64 = 2000;
#[cfg(windows)]
const WINDOWS_WATCH_SHUTDOWN_POLL_MS: u32 = 250;

#[cfg(windows)]
type WindowsHandle = *mut std::ffi::c_void;

#[cfg(windows)]
#[repr(C)]
struct WindowsOverlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    h_event: WindowsHandle,
}

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
    pub mode: FileWatcherMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileWatcherMode {
    NativePreferred,
    PollOnly,
}

pub struct FileWatcher {
    paths: IdentityPaths,
    config: FileWatcherConfig,
}

impl FileWatcher {
    pub fn new(paths: IdentityPaths, config: FileWatcherConfig) -> Self {
        Self { paths, config }
    }

    pub async fn run(self) -> Result<(), FileWatchError> {
        self.run_until_shutdown(Arc::new(AtomicBool::new(false)))
            .await
    }

    pub async fn run_until_shutdown(self, shutdown: Arc<AtomicBool>) -> Result<(), FileWatchError> {
        #[cfg(windows)]
        {
            if self.config.mode == FileWatcherMode::NativePreferred {
                let paths = self.paths;
                let config = self.config;
                let shutdown = shutdown.clone();

                println!(
                    "watching {} with Windows filesystem events",
                    config.root.display()
                );

                return tokio::task::spawn_blocking(move || {
                    windows_watch_loop(paths, config, shutdown)
                })
                .await?;
            }
        }

        self.run_poll_loop(shutdown).await
    }

    async fn run_poll_loop(self, shutdown: Arc<AtomicBool>) -> Result<(), FileWatchError> {
        println!(
            "polling {} for local text captures",
            self.config.root.display()
        );

        let mut seen = HashMap::new();
        let paths = self.paths;
        let root = self.config.root;
        let recursive = self.config.recursive;

        loop {
            if shutdown.load(Ordering::Relaxed) {
                return Ok(());
            }

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

#[cfg(windows)]
fn windows_watch_loop(
    paths: IdentityPaths,
    config: FileWatcherConfig,
    shutdown: Arc<AtomicBool>,
) -> Result<(), FileWatchError> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;

    type Bool = i32;
    type Dword = u32;
    type Handle = WindowsHandle;

    const FILE_LIST_DIRECTORY: Dword = 0x0001;
    const FILE_SHARE_READ: Dword = 0x0000_0001;
    const FILE_SHARE_WRITE: Dword = 0x0000_0002;
    const FILE_SHARE_DELETE: Dword = 0x0000_0004;
    const OPEN_EXISTING: Dword = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: Dword = 0x0200_0000;
    const FILE_FLAG_OVERLAPPED: Dword = 0x4000_0000;
    const FILE_ACTION_ADDED: Dword = 1;
    const FILE_ACTION_MODIFIED: Dword = 3;
    const FILE_ACTION_RENAMED_NEW_NAME: Dword = 5;
    const INVALID_HANDLE_VALUE: isize = -1;
    const WAIT_OBJECT_0: Dword = 0;
    const WAIT_TIMEOUT: Dword = 258;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            lpFileName: *const u16,
            dwDesiredAccess: Dword,
            dwShareMode: Dword,
            lpSecurityAttributes: *mut c_void,
            dwCreationDisposition: Dword,
            dwFlagsAndAttributes: Dword,
            hTemplateFile: Handle,
        ) -> Handle;
        fn CreateEventW(
            lpEventAttributes: *mut c_void,
            bManualReset: Bool,
            bInitialState: Bool,
            lpName: *const u16,
        ) -> Handle;
        fn WaitForSingleObject(hHandle: Handle, dwMilliseconds: Dword) -> Dword;
        fn ResetEvent(hEvent: Handle) -> Bool;
        fn CancelIoEx(hFile: Handle, lpOverlapped: *mut WindowsOverlapped) -> Bool;
        fn GetOverlappedResult(
            hFile: Handle,
            lpOverlapped: *mut WindowsOverlapped,
            lpNumberOfBytesTransferred: *mut Dword,
            bWait: Bool,
        ) -> Bool;
        fn CloseHandle(hObject: Handle) -> Bool;
    }

    struct DirectoryHandle(Handle);

    impl Drop for DirectoryHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    struct EventHandle(Handle);

    impl Drop for EventHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    let mut root_wide = config
        .root
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();

    let raw_handle = unsafe {
        CreateFileW(
            root_wide.as_mut_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            null_mut(),
        )
    };

    if raw_handle as isize == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }

    let _handle_guard = DirectoryHandle(raw_handle);
    let raw_event = unsafe { CreateEventW(null_mut(), 1, 0, null_mut()) };

    if raw_event.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }

    let _event_guard = EventHandle(raw_event);
    let buffer = TransitBuffer::open(&paths)?;
    let mut seen = HashMap::new();
    let mut events = [0_u8; 16 * 1024];
    let mut overlapped = WindowsOverlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        h_event: raw_event,
    };

    loop {
        issue_directory_read(raw_handle, &mut events, config.recursive, &mut overlapped)?;

        loop {
            let wait = unsafe { WaitForSingleObject(raw_event, WINDOWS_WATCH_SHUTDOWN_POLL_MS) };

            if wait == WAIT_OBJECT_0 {
                break;
            }

            if wait != WAIT_TIMEOUT {
                return Err(std::io::Error::last_os_error().into());
            }

            if shutdown.load(Ordering::Relaxed) {
                unsafe {
                    CancelIoEx(raw_handle, &mut overlapped);
                }
                return Ok(());
            }
        }

        let mut bytes_returned = 0;
        let ok =
            unsafe { GetOverlappedResult(raw_handle, &mut overlapped, &mut bytes_returned, 0) };

        if ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }

        unsafe {
            ResetEvent(raw_event);
        }

        process_windows_events(
            &buffer,
            &config.root,
            &events[..bytes_returned as usize],
            &mut seen,
            FILE_ACTION_ADDED,
            FILE_ACTION_MODIFIED,
            FILE_ACTION_RENAMED_NEW_NAME,
        )?;
    }
}

#[cfg(windows)]
fn issue_directory_read(
    raw_handle: WindowsHandle,
    events: &mut [u8],
    recursive: bool,
    overlapped: &mut WindowsOverlapped,
) -> Result<(), FileWatchError> {
    type Dword = u32;

    const FILE_NOTIFY_CHANGE_FILE_NAME: Dword = 0x0000_0001;
    const FILE_NOTIFY_CHANGE_LAST_WRITE: Dword = 0x0000_0010;
    const FILE_NOTIFY_CHANGE_SIZE: Dword = 0x0000_0008;
    const ERROR_IO_PENDING: i32 = 997;

    #[link(name = "kernel32")]
    extern "system" {
        fn ReadDirectoryChangesW(
            hDirectory: WindowsHandle,
            lpBuffer: *mut std::ffi::c_void,
            nBufferLength: Dword,
            bWatchSubtree: i32,
            dwNotifyFilter: Dword,
            lpBytesReturned: *mut Dword,
            lpOverlapped: *mut WindowsOverlapped,
            lpCompletionRoutine: *mut std::ffi::c_void,
        ) -> i32;
        fn GetLastError() -> Dword;
    }

    let ok = unsafe {
        ReadDirectoryChangesW(
            raw_handle,
            events.as_mut_ptr().cast::<std::ffi::c_void>(),
            events.len() as Dword,
            if recursive { 1 } else { 0 },
            FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE,
            std::ptr::null_mut(),
            overlapped,
            std::ptr::null_mut(),
        )
    };

    if ok == 0 {
        let error = unsafe { GetLastError() } as i32;
        if error != ERROR_IO_PENDING {
            return Err(std::io::Error::last_os_error().into());
        }
    }

    Ok(())
}

#[cfg(windows)]
fn process_windows_events(
    buffer: &TransitBuffer,
    root: &Path,
    events: &[u8],
    seen: &mut HashMap<PathBuf, FileFingerprint>,
    added: u32,
    modified: u32,
    renamed_new: u32,
) -> Result<(), FileWatchError> {
    let mut offset = 0_usize;

    while offset + 12 <= events.len() {
        let base = unsafe { events.as_ptr().add(offset) };
        let next_entry_offset = unsafe { std::ptr::read_unaligned(base.cast::<u32>()) };
        let action = unsafe { std::ptr::read_unaligned(base.add(4).cast::<u32>()) };
        let file_name_len = unsafe { std::ptr::read_unaligned(base.add(8).cast::<u32>()) } as usize;

        if action == added || action == modified || action == renamed_new {
            let name_start = offset + 12;
            let name_end = name_start.saturating_add(file_name_len);

            if name_end <= events.len() && file_name_len % 2 == 0 {
                let name_slice = unsafe {
                    std::slice::from_raw_parts(
                        events.as_ptr().add(name_start).cast::<u16>(),
                        file_name_len / 2,
                    )
                };
                let relative = String::from_utf16_lossy(name_slice);
                let path = root.join(relative);

                if let Ok(metadata) = fs::metadata(&path) {
                    if metadata.is_file() {
                        ingest_file_if_text(
                            buffer,
                            &path,
                            metadata.len(),
                            metadata.modified().ok(),
                            seen,
                        )?;
                    }
                }
            }
        }

        if next_entry_offset == 0 {
            break;
        }

        offset += next_entry_offset as usize;
    }

    Ok(())
}

fn scan_once(
    paths: &IdentityPaths,
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
    _modified: Option<SystemTime>,
    seen: &mut HashMap<PathBuf, FileFingerprint>,
) -> Result<(), FileWatchError> {
    if !is_supported_text_path(path) || len > MAX_TEXT_FILE_BYTES {
        return Ok(());
    }

    let Some(content) = read_text_with_short_retry(path)? else {
        return Ok(());
    };
    let fingerprint = FileFingerprint {
        content_hash: stable_hash(content.as_bytes()),
    };

    if seen.get(path) == Some(&fingerprint) {
        return Ok(());
    }

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

fn read_text_with_short_retry(path: &Path) -> Result<Option<String>, FileWatchError> {
    for attempt in 0..3 {
        match fs::read_to_string(path) {
            Ok(content) => return Ok(Some(content)),
            Err(error) if is_transient_file_lock(&error) && attempt < 2 => {
                std::thread::sleep(std::time::Duration::from_millis(75));
            }
            Err(error) if is_transient_file_lock(&error) => {
                return Ok(None);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
    }

    Ok(None)
}

#[inline]
fn is_transient_file_lock(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::PermissionDenied | ErrorKind::WouldBlock
    ) || error.raw_os_error() == Some(32)
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
    content_hash: u64,
}

#[inline]
fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;

    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }

    hash
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
