use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SovereignPaths {
    pub root: PathBuf,
    pub identity_dir: PathBuf,
    pub identity_db: PathBuf,
    pub transit_db: PathBuf,
    pub logs_dir: PathBuf,
}

#[derive(Debug)]
pub enum WorkspaceError {
    MissingHome,
    Io(io::Error),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHome => write!(f, "could not determine the user's home directory"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<io::Error> for WorkspaceError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl SovereignPaths {
    pub fn from_default_home() -> Result<Self, WorkspaceError> {
        let home = home_dir()?;
        Ok(Self::from_root(home.join(".sovereign")))
    }

    pub fn from_root(root: PathBuf) -> Self {
        let identity_dir = root.join("identity.me");
        let identity_db = identity_dir.join("state.db");
        let transit_db = root.join("transit.db");
        let logs_dir = root.join("logs");

        Self {
            root,
            identity_dir,
            identity_db,
            transit_db,
            logs_dir,
        }
    }

    pub fn ensure(&self) -> Result<(), WorkspaceError> {
        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(&self.identity_dir)?;
        fs::create_dir_all(&self.logs_dir)?;
        Ok(())
    }
}

fn home_dir() -> Result<PathBuf, WorkspaceError> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or(WorkspaceError::MissingHome)
}

#[cfg(test)]
mod tests {
    use super::SovereignPaths;

    #[test]
    fn derives_expected_paths_from_root() {
        let paths = SovereignPaths::from_root("C:/tmp/sovereign-test".into());

        assert!(paths.root.ends_with(".") || paths.root.ends_with("sovereign-test"));
        assert!(paths.identity_dir.ends_with("identity.me"));
        assert!(paths.identity_db.ends_with("state.db"));
        assert!(paths.transit_db.ends_with("transit.db"));
        assert!(paths.logs_dir.ends_with("logs"));
    }
}
