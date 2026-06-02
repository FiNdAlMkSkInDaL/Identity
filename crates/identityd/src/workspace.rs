use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct IdentityPaths {
    pub root: PathBuf,
    pub identity_dir: PathBuf,
    pub identity_db: PathBuf,
    pub vector_store_dir: PathBuf,
    pub transit_db: PathBuf,
    pub logs_dir: PathBuf,
    pub capture_token: PathBuf,
}

#[derive(Debug)]
pub enum WorkspaceError {
    MissingHome,
    Io(io::Error),
    ClockBeforeUnixEpoch,
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHome => write!(f, "could not determine the user's home directory"),
            Self::Io(error) => write!(f, "{error}"),
            Self::ClockBeforeUnixEpoch => write!(f, "system clock is before the Unix epoch"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<io::Error> for WorkspaceError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl IdentityPaths {
    pub fn from_default_home() -> Result<Self, WorkspaceError> {
        let home = home_dir()?;
        Ok(Self::from_root(home.join(".identity")))
    }

    pub fn from_root(root: PathBuf) -> Self {
        let identity_dir = root.join("identity.me");
        let identity_db = identity_dir.join("state.db");
        let vector_store_dir = identity_dir.join("vectors");
        let transit_db = root.join("transit.db");
        let logs_dir = root.join("logs");
        let capture_token = root.join("capture.token");

        Self {
            root,
            identity_dir,
            identity_db,
            vector_store_dir,
            transit_db,
            logs_dir,
            capture_token,
        }
    }

    pub fn ensure(&self) -> Result<(), WorkspaceError> {
        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(&self.identity_dir)?;
        fs::create_dir_all(&self.vector_store_dir)?;
        fs::create_dir_all(&self.logs_dir)?;
        self.ensure_capture_token()?;
        Ok(())
    }

    pub fn ensure_capture_token(&self) -> Result<String, WorkspaceError> {
        if self.capture_token.exists() {
            return Ok(fs::read_to_string(&self.capture_token)?.trim().to_string());
        }

        let token = generate_capture_token()?;
        fs::write(&self.capture_token, &token)?;
        Ok(token)
    }
}

fn home_dir() -> Result<PathBuf, WorkspaceError> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or(WorkspaceError::MissingHome)
}

fn generate_capture_token() -> Result<String, WorkspaceError> {
    let mut bytes = [0_u8; 32];

    if fill_random_bytes(&mut bytes).is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| WorkspaceError::ClockBeforeUnixEpoch)?
            .as_nanos();
        let pid = u128::from(std::process::id());

        for (index, byte) in bytes.iter_mut().enumerate() {
            let mixed = nanos.rotate_left(index as u32) ^ pid.rotate_right(index as u32);
            *byte = (mixed >> ((index % 16) * 8)) as u8;
        }
    }

    Ok(hex_encode(&bytes))
}

#[cfg(windows)]
fn fill_random_bytes(bytes: &mut [u8]) -> io::Result<()> {
    #[link(name = "advapi32")]
    extern "system" {
        fn SystemFunction036(RandomBuffer: *mut u8, RandomBufferLength: u32) -> u8;
    }

    let ok = unsafe { SystemFunction036(bytes.as_mut_ptr(), bytes.len() as u32) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn fill_random_bytes(bytes: &mut [u8]) -> io::Result<()> {
    use std::io::Read;

    let mut file = fs::File::open("/dev/urandom")?;
    file.read_exact(bytes)
}

#[cfg(not(any(unix, windows)))]
fn fill_random_bytes(_bytes: &mut [u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "os random source unavailable",
    ))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::IdentityPaths;
    use std::fs;

    #[test]
    fn derives_expected_paths_from_root() {
        let paths = IdentityPaths::from_root("C:/tmp/identity-test".into());

        assert!(paths.root.ends_with(".") || paths.root.ends_with("identity-test"));
        assert!(paths.identity_dir.ends_with("identity.me"));
        assert!(paths.identity_db.ends_with("state.db"));
        assert!(paths.vector_store_dir.ends_with("vectors"));
        assert!(paths.transit_db.ends_with("transit.db"));
        assert!(paths.logs_dir.ends_with("logs"));
        assert!(paths.capture_token.ends_with("capture.token"));
    }

    #[test]
    fn capture_token_is_stable_after_creation() {
        let root = std::env::temp_dir().join(format!(
            "identity-token-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let first = paths.ensure_capture_token().unwrap();
        let second = paths.ensure_capture_token().unwrap();

        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));

        fs::remove_dir_all(root).unwrap();
    }
}
