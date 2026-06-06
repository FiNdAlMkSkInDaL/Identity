use crate::context_snapshot::ContextSnapshot;
use crate::workspace::IdentityPaths;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectProfile {
    pub name: String,
    pub window_filters: Vec<String>,
    pub guardrails: Vec<String>,
    pub memory_query_terms: Vec<String>,
}

/// Load all project profiles from `projects.json` within the Identity workspace.
/// Seeds the default profile if the file does not exist.
pub fn load_profiles(paths: &IdentityPaths) -> Result<Vec<ProjectProfile>, io::Error> {
    if !paths.projects_json.exists() {
        let _ = paths.ensure_default_projects_json();
    }

    let content = fs::read_to_string(&paths.projects_json)?;
    let profiles: Vec<ProjectProfile> = serde_json::from_str(&content)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    Ok(profiles)
}

/// Perform case-insensitive substring matching of window title and process name
/// against filters declared in each profile. Returns the first matching profile.
pub fn find_matching_profile(
    profiles: &[ProjectProfile],
    snapshot: &ContextSnapshot,
) -> Option<ProjectProfile> {
    let title_lower = snapshot.window_title.to_lowercase();
    let proc_lower = snapshot.process_name.to_lowercase();

    for profile in profiles {
        for filter in &profile.window_filters {
            let filter_lower = filter.to_lowercase();
            if title_lower.contains(&filter_lower) || proc_lower.contains(&filter_lower) {
                return Some(profile.clone());
            }
        }
    }
    None
}

/// Find a profile by explicit user-provided name.
pub fn find_profile_by_name(profiles: &[ProjectProfile], name: &str) -> Option<ProjectProfile> {
    let requested = name.trim();
    if requested.is_empty() {
        return None;
    }

    profiles
        .iter()
        .find(|profile| profile.name.eq_ignore_ascii_case(requested))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_snapshot::ContextSnapshot;
    use crate::workspace::IdentityPaths;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_profile_matching() {
        let profiles = vec![
            ProjectProfile {
                name: "tfl-central".to_string(),
                window_filters: vec!["tfl".to_string(), "oyster".to_string()],
                guardrails: vec!["tfl rules".to_string()],
                memory_query_terms: vec!["tfl".to_string()],
            },
            ProjectProfile {
                name: "rust-dev".to_string(),
                window_filters: vec!["cargo".to_string(), "rust".to_string()],
                guardrails: vec![],
                memory_query_terms: vec![],
            },
        ];

        // Match on window title
        let snap1 = ContextSnapshot {
            process_name: "chrome".to_string(),
            window_title: "TfL Oyster Card Portal".to_string(),
            focused_text: None,
        };
        assert_eq!(
            find_matching_profile(&profiles, &snap1).map(|p| p.name),
            Some("tfl-central".to_string())
        );

        // Match on process name (case insensitivity)
        let snap2 = ContextSnapshot {
            process_name: "CARGO-RUN".to_string(),
            window_title: "random".to_string(),
            focused_text: None,
        };
        assert_eq!(
            find_matching_profile(&profiles, &snap2).map(|p| p.name),
            Some("rust-dev".to_string())
        );

        // No match
        let snap3 = ContextSnapshot {
            process_name: "explorer".to_string(),
            window_title: "my desktop".to_string(),
            focused_text: None,
        };
        assert_eq!(find_matching_profile(&profiles, &snap3), None);
    }

    #[test]
    fn test_profile_lookup_by_explicit_name() {
        let profiles = vec![
            ProjectProfile {
                name: "tfl-central".to_string(),
                window_filters: vec!["tfl".to_string()],
                guardrails: vec![],
                memory_query_terms: vec![],
            },
            ProjectProfile {
                name: "rust-dev".to_string(),
                window_filters: vec!["rust".to_string()],
                guardrails: vec![],
                memory_query_terms: vec![],
            },
        ];

        assert_eq!(
            find_profile_by_name(&profiles, " TFL-CENTRAL ").map(|profile| profile.name),
            Some("tfl-central".to_string())
        );
        assert_eq!(find_profile_by_name(&profiles, "missing"), None);
        assert_eq!(find_profile_by_name(&profiles, "   "), None);
    }

    #[test]
    fn test_load_and_seed_profiles() {
        let temp_dir = std::env::temp_dir().join(format!(
            "identity-profile-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(temp_dir.clone());
        paths.ensure().unwrap();

        // Should seed default and load successfully
        let loaded = load_profiles(&paths).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "tfl-central");
        assert!(loaded[0].window_filters.contains(&"tfl".to_string()));

        let _ = fs::remove_dir_all(temp_dir);
    }
}
