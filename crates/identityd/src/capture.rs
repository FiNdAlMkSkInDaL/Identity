use crate::filesystem::{filesystem_watch_policy_health, FileWatchPolicyHealth};
use crate::workspace::IdentityPaths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureAdapterHealth {
    pub manual_adapter: &'static str,
    pub loopback_adapter: &'static str,
    pub loopback_token_exists: bool,
    pub filesystem_adapter: &'static str,
    pub active_window_adapter: &'static str,
    pub phase1_status: &'static str,
    pub filesystem_policy: FileWatchPolicyHealth,
}

pub fn capture_adapter_health(paths: &IdentityPaths) -> CaptureAdapterHealth {
    let loopback_token_exists = paths.capture_token.exists();

    CaptureAdapterHealth {
        manual_adapter: "ready",
        loopback_adapter: loopback_capture_status(loopback_token_exists),
        loopback_token_exists,
        filesystem_adapter: "safe-root-policy",
        active_window_adapter: active_window_capture_status(),
        phase1_status: phase1_capture_adapters_status(
            loopback_token_exists,
            active_window_capture_status(),
        ),
        filesystem_policy: filesystem_watch_policy_health(),
    }
}

fn loopback_capture_status(capture_token_exists: bool) -> &'static str {
    if capture_token_exists {
        "token-protected"
    } else {
        "missing-token"
    }
}

fn active_window_capture_status() -> &'static str {
    if cfg!(windows) {
        "windows-minimal"
    } else if cfg!(target_os = "macos") {
        "macos-accessibility"
    } else if cfg!(target_os = "linux") {
        "linux-accessibility"
    } else {
        "unsupported-platform"
    }
}

fn phase1_capture_adapters_status(capture_token_exists: bool, active_window: &str) -> &'static str {
    if !capture_token_exists {
        "needs-repair"
    } else if active_window == "unsupported-platform" {
        "partial"
    } else {
        "ready"
    }
}

#[cfg(test)]
mod tests {
    use super::capture_adapter_health;
    use crate::workspace::IdentityPaths;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn capture_adapter_health_reports_ready_local_adapters_conservatively() {
        let root = temp_root();
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let health = capture_adapter_health(&paths);

        assert_eq!(health.manual_adapter, "ready");
        assert_eq!(health.loopback_adapter, "token-protected");
        assert!(health.loopback_token_exists);
        assert_eq!(health.filesystem_adapter, "safe-root-policy");
        assert_eq!(health.phase1_status, "ready");
        assert!(health.filesystem_policy.safe_root_enforced);
        assert!(!health.active_window_adapter.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_adapter_health_flags_missing_loopback_token() {
        let root = temp_root();
        let paths = IdentityPaths::from_root(root.clone());

        let health = capture_adapter_health(&paths);

        assert_eq!(health.loopback_adapter, "missing-token");
        assert!(!health.loopback_token_exists);
        assert_eq!(health.phase1_status, "needs-repair");

        fs::remove_dir_all(root).unwrap();
    }

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "identity-capture-health-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
