use crate::context_snapshot::ContextSnapshot;
use crate::identity::IdentityStore;
use crate::project_profile::ProjectProfile;
use crate::slice::security_block;
use crate::workspace::IdentityPaths;
use std::time::{SystemTime, UNIX_EPOCH};

const RECENT_PAGE_CONTEXT_MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug)]
pub struct IdentityContext {
    pub session_token: String,
    pub expiry_epoch_ms: i64,
    pub process_name: String,
    pub window_title: String,
    pub focused_text: Option<String>,
    pub profile_name: Option<String>,
    pub guardrails: Vec<String>,
    pub facts: Vec<String>,
}

#[derive(Debug)]
pub enum ContextBuildError {
    Blocked(crate::slice::SecurityBlock),
    ClockBeforeUnixEpoch,
    Identity(crate::identity::IdentityError),
}

impl std::fmt::Display for ContextBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked(block) => write!(
                f,
                "blocked by security policy: {} matched '{}'",
                block.category, block.matched_term
            ),
            Self::ClockBeforeUnixEpoch => write!(f, "system clock is before the Unix epoch"),
            Self::Identity(err) => write!(f, "identity store error: {err}"),
        }
    }
}

impl std::error::Error for ContextBuildError {}

impl From<crate::identity::IdentityError> for ContextBuildError {
    fn from(value: crate::identity::IdentityError) -> Self {
        Self::Identity(value)
    }
}

impl IdentityContext {
    pub fn to_context_block(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!(
            "[IDENTITY-CONTEXT-BLOCK: {}]\n",
            self.session_token
        ));
        output.push_str(&format!(
            "- Authorization expiry epoch ms: {}\n",
            self.expiry_epoch_ms
        ));
        output.push_str(&format!("- Active Application: {}\n", self.process_name));
        output.push_str(&format!("- Active Window: {}\n", self.window_title));

        if let Some(focused) = &self.focused_text {
            output.push_str(&format!("- Focused Control: {}\n", focused));
        }

        if let Some(profile) = &self.profile_name {
            output.push_str(&format!("- Active Profile: {}\n", profile));
        }

        if !self.guardrails.is_empty() {
            output.push_str("- Guardrails:\n");
            for rule in &self.guardrails {
                output.push_str(&format!("  - {}\n", rule));
            }
        }

        output.push_str("- Relevant Local Context:\n");
        if self.facts.is_empty() {
            output.push_str("  - No matching local context found.\n");
        } else {
            for fact in &self.facts {
                output.push_str(&format!("  - {}\n", fact));
            }
        }

        output.push_str(&format!(
            "[IDENTITY-CONTEXT-BLOCK-END: {}]",
            self.session_token
        ));

        output
    }
}

pub fn build_identity_context(
    paths: &IdentityPaths,
    snapshot: &ContextSnapshot,
    profile: Option<&ProjectProfile>,
    limit: u32,
) -> Result<IdentityContext, ContextBuildError> {
    // 1. Check active window metadata for security blocks
    if let Some(block) = security_block(&snapshot.window_title) {
        return Err(ContextBuildError::Blocked(block));
    }
    if let Some(block) = security_block(&snapshot.process_name) {
        return Err(ContextBuildError::Blocked(block));
    }
    if let Some(focused) = &snapshot.focused_text {
        if let Some(block) = security_block(focused) {
            return Err(ContextBuildError::Blocked(block));
        }
    }
    let now = current_epoch_ms()?;

    // 2. Fetch facts from vector memory store
    let mut query_terms = Vec::new();
    let mut guardrails = Vec::new();
    let mut profile_name = None;

    if let Some(p) = profile {
        profile_name = Some(p.name.clone());
        guardrails.extend(p.guardrails.iter().cloned());
        query_terms.extend(p.memory_query_terms.iter().cloned());
    } else if !snapshot.window_title.is_empty() {
        query_terms.push(snapshot.window_title.clone());
    }

    let mut facts = Vec::new();
    if let Ok(store) = IdentityStore::open(paths) {
        let mut seen_uids = std::collections::HashSet::new();
        let mut searched_memories = Vec::new();

        for term in &query_terms {
            if let Ok(results) = store.search(term, limit) {
                for result in results {
                    if seen_uids.insert(result.node.node_uid.clone()) {
                        searched_memories.push(result.node);
                    }
                }
            }
        }

        let mut total_chars = 0;
        let mut seen_facts = std::collections::HashSet::new();
        append_memory_facts(
            searched_memories,
            &mut facts,
            &mut seen_facts,
            &mut total_chars,
        );

        if should_include_recent_page_context(snapshot) {
            if let Ok(recent_pages) = store.recent_selected_page_captures(limit.min(3)) {
                let recent_pages = recent_pages
                    .into_iter()
                    .filter(|node| is_fresh_recent_page_context(node, now))
                    .filter(|node| seen_uids.insert(node.node_uid.clone()))
                    .collect::<Vec<_>>();
                append_memory_facts(recent_pages, &mut facts, &mut seen_facts, &mut total_chars);
            }
        }
    }

    // 3. Generate session token and expiry
    let seed_input = format!(
        "{}:{}:{}:{}",
        snapshot.process_name,
        snapshot.window_title,
        now,
        facts.len()
    );
    let session_token = format!("slice_{}", stable_hash_hex(seed_input.as_bytes()));
    let expiry_epoch_ms = now + 2000;

    Ok(IdentityContext {
        session_token,
        expiry_epoch_ms,
        process_name: snapshot.process_name.clone(),
        window_title: snapshot.window_title.clone(),
        focused_text: snapshot.focused_text.clone(),
        profile_name,
        guardrails,
        facts,
    })
}

fn append_memory_facts(
    nodes: Vec<crate::identity::MemoryNode>,
    facts: &mut Vec<String>,
    seen_facts: &mut std::collections::HashSet<String>,
    total_chars: &mut usize,
) {
    for node in nodes {
        if security_block(&node.summary).is_some() {
            continue;
        }

        let sanitized = node.summary.replace('\n', " ").trim().to_string();
        let truncated = if sanitized.chars().count() > 320 {
            let mut s: String = sanitized.chars().take(320).collect();
            s.push_str("...");
            s
        } else {
            sanitized
        };
        let fact_key = normalize_fact_key(&truncated);
        if fact_key.is_empty() || !seen_facts.insert(fact_key) {
            continue;
        }

        if *total_chars + truncated.len() > 1200 {
            break;
        }
        *total_chars += truncated.len();
        facts.push(truncated);
    }
}

fn current_epoch_ms() -> Result<i64, ContextBuildError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ContextBuildError::ClockBeforeUnixEpoch)?
        .as_millis() as i64)
}

fn is_fresh_recent_page_context(node: &crate::identity::MemoryNode, now_ms: i64) -> bool {
    now_ms.saturating_sub(node.created_at_ms) <= RECENT_PAGE_CONTEXT_MAX_AGE_MS
}

fn should_include_recent_page_context(snapshot: &ContextSnapshot) -> bool {
    let process = snapshot.process_name.to_ascii_lowercase();
    let title = snapshot.window_title.to_ascii_lowercase();

    is_browser_process(&process) || is_agent_surface(&process) || is_agent_surface(&title)
}

fn is_browser_process(process: &str) -> bool {
    let normalized = process
        .trim()
        .trim_end_matches(".exe")
        .trim_end_matches(".app");

    [
        "chrome",
        "msedge",
        "microsoftedge",
        "firefox",
        "brave",
        "bravebrowser",
        "browser",
        "arc",
        "opera",
        "vivaldi",
    ]
    .contains(&normalized)
}

fn is_agent_surface(value: &str) -> bool {
    [
        "google gemini",
        "gemini",
        "chatgpt",
        "chat.openai",
        "codex",
        "claude",
        "antigravity",
        "perplexity",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn normalize_fact_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_snapshot::ContextSnapshot;
    use crate::identity::IdentityStore;
    use crate::project_profile::ProjectProfile;
    use crate::transit::CleanedEvent;
    use crate::workspace::IdentityPaths;
    use rusqlite::params;
    use std::fs;

    #[test]
    fn test_security_block_active_window() {
        let root = std::env::temp_dir().join("identity-context-security-test");
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let snap = ContextSnapshot {
            process_name: "chrome".to_string(),
            window_title: "private key leaking in browser".to_string(),
            focused_text: None,
        };

        let err = build_identity_context(&paths, &snap, None, 3).unwrap_err();
        assert!(err.to_string().contains("private_keys"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn test_context_building_without_profile() {
        let root = std::env::temp_dir().join("identity-context-build-test");
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 1,
            captured_event_id: 1,
            source: "test".to_string(),
            cleaned_content: "TfL Oyster cards are Oyster card details.".to_string(),
            content_hash: "hash".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };
        store.insert_memory_from_cleaned(&cleaned).unwrap();

        let snap = ContextSnapshot {
            process_name: "chrome".to_string(),
            window_title: "Oyster card details".to_string(),
            focused_text: Some("card number field".to_string()),
        };

        let ctx = build_identity_context(&paths, &snap, None, 3).unwrap();
        let block = ctx.to_context_block();

        assert!(block.contains("IDENTITY-CONTEXT-BLOCK"));
        assert!(block.contains("Active Application: chrome"));
        assert!(block.contains("Active Window: Oyster card details"));
        assert!(block.contains("Focused Control: card number field"));
        assert!(block.contains("TfL Oyster cards are Oyster card details."));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn test_context_building_with_profile() {
        let root = std::env::temp_dir().join("identity-context-profile-build-test");
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 1,
            captured_event_id: 1,
            source: "test".to_string(),
            cleaned_content: "London Underground stations map".to_string(),
            content_hash: "hash".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };
        store.insert_memory_from_cleaned(&cleaned).unwrap();

        let snap = ContextSnapshot {
            process_name: "code".to_string(),
            window_title: "tfl-central-repo".to_string(),
            focused_text: None,
        };

        let profile = ProjectProfile {
            name: "tfl-central".to_string(),
            window_filters: vec!["tfl".to_string()],
            guardrails: vec!["Follow TfL API regulations".to_string()],
            memory_query_terms: vec!["London Underground stations".to_string()],
        };

        let ctx = build_identity_context(&paths, &snap, Some(&profile), 3).unwrap();
        let block = ctx.to_context_block();

        assert!(block.contains("Active Profile: tfl-central"));
        assert!(block.contains("Follow TfL API regulations"));
        assert!(block.contains("London Underground stations map"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn test_context_deduplicates_repeated_fact_text() {
        let root = std::env::temp_dir().join("identity-context-dedup-test");
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let content = "Active application: msedge.exe\nActive window title: Google Gemini";

        for id in 1..=2 {
            let cleaned = CleanedEvent {
                id,
                captured_event_id: id,
                source: "windows-ui:active-window".to_string(),
                cleaned_content: content.to_string(),
                content_hash: format!("hash-{id}"),
                cleaned_at_ms: id,
                promoted_at_ms: None,
            };
            store.insert_memory_from_cleaned(&cleaned).unwrap();
        }

        let snap = ContextSnapshot {
            process_name: "msedge".to_string(),
            window_title: "Google Gemini".to_string(),
            focused_text: None,
        };

        let ctx = build_identity_context(&paths, &snap, None, 5).unwrap();
        assert_eq!(ctx.facts.len(), 1);
        assert_eq!(
            ctx.facts[0],
            "UI activity in msedge.exe; window Google Gemini"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn browser_context_includes_recent_selected_page_capture() {
        let root = std::env::temp_dir().join("identity-context-recent-page-test");
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 9,
            captured_event_id: 9,
            source: "local-proxy:text/markdown".to_string(),
            cleaned_content: "Page title: Identity manifesto Page URL: https://example.test/manifesto Selected page text: Identity must stay local-first, lean, and user-owned.".to_string(),
            content_hash: "hash-page".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };
        store.insert_memory_from_cleaned(&cleaned).unwrap();
        let generic_web = CleanedEvent {
            id: 10,
            captured_event_id: 10,
            source: "local-proxy:text/plain".to_string(),
            cleaned_content: "Generic loopback capture should stay out of selected page fallback."
                .to_string(),
            content_hash: "hash-generic-web".to_string(),
            cleaned_at_ms: 2,
            promoted_at_ms: None,
        };
        store.insert_memory_from_cleaned(&generic_web).unwrap();

        let snap = ContextSnapshot {
            process_name: "msedge.exe".to_string(),
            window_title: "Google Gemini".to_string(),
            focused_text: None,
        };

        let ctx = build_identity_context(&paths, &snap, None, 3).unwrap();
        assert!(ctx.facts.iter().any(|fact| {
            fact.contains("web page Identity manifesto")
                && fact.contains("local-first, lean, and user-owned")
        }));
        assert!(!ctx
            .facts
            .iter()
            .any(|fact| fact.contains("Generic loopback capture")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn browser_context_ignores_stale_recent_selected_page_capture() {
        let root = std::env::temp_dir().join("identity-context-stale-page-test");
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let node_id = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 31,
                captured_event_id: 31,
                source: "local-proxy:text/markdown".to_string(),
                cleaned_content: "Page title: Old page\nPage URL: https://example.test/old\nSelected page text:\nStale selected page context should remain searchable but not automatic.".to_string(),
                content_hash: "hash-stale-page".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();
        drop(store);

        let stale_ms = current_epoch_ms().unwrap() - RECENT_PAGE_CONTEXT_MAX_AGE_MS - 1000;
        let conn = rusqlite::Connection::open(&paths.identity_db).unwrap();
        conn.execute(
            "UPDATE memory_nodes SET created_at_ms = ?1 WHERE id = ?2",
            params![stale_ms, node_id],
        )
        .unwrap();
        drop(conn);

        let snap = ContextSnapshot {
            process_name: "msedge.exe".to_string(),
            window_title: "Plain browser notes".to_string(),
            focused_text: None,
        };
        let profile = ProjectProfile {
            name: "empty-query".to_string(),
            window_filters: Vec::new(),
            guardrails: Vec::new(),
            memory_query_terms: Vec::new(),
        };

        let ctx = build_identity_context(&paths, &snap, Some(&profile), 3).unwrap();
        assert!(!ctx
            .facts
            .iter()
            .any(|fact| fact.contains("Stale selected page context")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_profile_alone_does_not_include_recent_selected_page_capture() {
        let project_surface = ContextSnapshot {
            process_name: "code.exe".to_string(),
            window_title: "identity workspace".to_string(),
            focused_text: None,
        };
        let browser_surface = ContextSnapshot {
            process_name: "code.exe".to_string(),
            window_title: "Google Gemini".to_string(),
            focused_text: None,
        };

        assert!(!should_include_recent_page_context(&project_surface));
        assert!(should_include_recent_page_context(&browser_surface));
    }

    #[test]
    fn page_context_surface_classifier_avoids_short_substring_false_positives() {
        for title in [
            "knowledge base notes",
            "architecture notes",
            "project edge cases",
        ] {
            let snap = ContextSnapshot {
                process_name: "code.exe".to_string(),
                window_title: title.to_string(),
                focused_text: None,
            };
            assert!(
                !should_include_recent_page_context(&snap),
                "unexpected page-context fallback for title {title:?}"
            );
        }
    }

    #[test]
    fn page_context_surface_classifier_allows_known_browser_processes_and_agent_titles() {
        for process in ["msedge.exe", "chrome", "firefox.exe", "arc.exe"] {
            let snap = ContextSnapshot {
                process_name: process.to_string(),
                window_title: "Plain project notes".to_string(),
                focused_text: None,
            };
            assert!(
                should_include_recent_page_context(&snap),
                "expected browser process {process:?} to allow recent page context"
            );
        }

        let agent_title = ContextSnapshot {
            process_name: "powershell.exe".to_string(),
            window_title: "Google Gemini - Identity page capture smoke test".to_string(),
            focused_text: None,
        };
        assert!(should_include_recent_page_context(&agent_title));
    }
}
