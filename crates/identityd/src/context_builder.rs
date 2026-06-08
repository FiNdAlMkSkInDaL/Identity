use crate::context_snapshot::ContextSnapshot;
use crate::identity::IdentityStore;
use crate::project_profile::ProjectProfile;
use crate::slice::security_block;
use crate::workspace::IdentityPaths;
use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

const RECENT_PAGE_CONTEXT_MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;
const CONTEXT_FACT_CHAR_BUDGET: usize = 1200;
const CONTEXT_FACT_MAX_CHARS: usize = 320;
const MAX_ACTIVITY_WINDOW_FACTS: usize = 1;
const MAX_SELECTED_PAGE_FACTS: usize = 3;

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
        for term in &p.memory_query_terms {
            push_query_term(&mut query_terms, term);
        }
    }
    push_query_term(&mut query_terms, &snapshot.window_title);

    if query_terms.is_empty() && !snapshot.process_name.is_empty() {
        push_query_term(&mut query_terms, &snapshot.process_name);
    }

    let mut facts = Vec::new();
    if let Ok(store) = IdentityStore::open(paths) {
        let mut seen_uids = std::collections::HashSet::new();
        let mut candidates = Vec::new();
        let mut sequence = 0usize;

        for term in &query_terms {
            if let Ok(results) = store.search(term, limit) {
                for result in results {
                    if seen_uids.insert(result.node.node_uid.clone()) {
                        candidates.push(ContextFactCandidate {
                            node: result.node,
                            origin: CandidateOrigin::Search(result.score),
                            sequence,
                        });
                        sequence += 1;
                    }
                }
            }
        }

        if should_include_recent_page_context(snapshot) {
            if let Ok(recent_pages) = store.recent_selected_page_captures(limit.min(3)) {
                let recent_pages = recent_pages
                    .into_iter()
                    .filter(|node| is_fresh_recent_page_context(node, now))
                    .filter(|node| seen_uids.insert(node.node_uid.clone()))
                    .collect::<Vec<_>>();
                for node in recent_pages {
                    candidates.push(ContextFactCandidate {
                        node,
                        origin: CandidateOrigin::RecentSelectedPage,
                        sequence,
                    });
                    sequence += 1;
                }
            }
        }

        facts = rank_memory_facts(candidates, limit);
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

#[derive(Debug)]
struct ContextFactCandidate {
    node: crate::identity::MemoryNode,
    origin: CandidateOrigin,
    sequence: usize,
}

#[derive(Debug)]
enum CandidateOrigin {
    Search(u32),
    RecentSelectedPage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextSourceKind {
    SelectedPage,
    WebCapture,
    ActivityWindow,
    Other,
}

#[derive(Debug)]
struct PreparedFact {
    text: String,
    fact_key: String,
    source_key: String,
    domain_key: String,
    kind: ContextSourceKind,
    score: u32,
    created_at_ms: i64,
    sequence: usize,
}

fn rank_memory_facts(candidates: Vec<ContextFactCandidate>, limit: u32) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }

    let mut prepared = candidates
        .into_iter()
        .filter_map(prepare_fact)
        .collect::<Vec<_>>();

    prepared.sort_by(compare_prepared_facts);
    let has_non_selected_page_fact = prepared
        .iter()
        .any(|fact| fact.kind != ContextSourceKind::SelectedPage);
    let selected_page_limit = selected_page_fact_limit(limit, has_non_selected_page_fact);
    let domain_limit = domain_fact_limit(limit, has_diverse_domains(&prepared));

    let mut facts = Vec::new();
    let mut total_chars = 0usize;
    let mut activity_window_facts = 0usize;
    let mut selected_page_facts = 0usize;
    let mut seen_facts = std::collections::HashSet::new();
    let mut seen_activity_sources = std::collections::HashSet::new();
    let mut domain_counts = std::collections::HashMap::<String, usize>::new();
    let mut domain_deferred = Vec::new();

    for fact in prepared {
        if seen_facts.contains(&fact.fact_key) {
            continue;
        }

        if total_chars + fact.text.len() > CONTEXT_FACT_CHAR_BUDGET {
            continue;
        }

        if !source_quotas_allow(
            &fact,
            selected_page_facts,
            selected_page_limit,
            activity_window_facts,
            &seen_activity_sources,
        ) {
            continue;
        }

        if domain_counts
            .get(&fact.domain_key)
            .copied()
            .unwrap_or_default()
            >= domain_limit
        {
            domain_deferred.push(fact);
            continue;
        }

        accept_prepared_fact(
            fact,
            &mut facts,
            &mut total_chars,
            &mut seen_facts,
            &mut seen_activity_sources,
            &mut domain_counts,
            &mut activity_window_facts,
            &mut selected_page_facts,
        );

        if facts.len() >= limit as usize {
            break;
        }
    }

    if facts.len() < limit as usize {
        for fact in domain_deferred {
            if seen_facts.contains(&fact.fact_key) {
                continue;
            }

            if total_chars + fact.text.len() > CONTEXT_FACT_CHAR_BUDGET {
                continue;
            }

            if !source_quotas_allow(
                &fact,
                selected_page_facts,
                selected_page_limit,
                activity_window_facts,
                &seen_activity_sources,
            ) {
                continue;
            }

            accept_prepared_fact(
                fact,
                &mut facts,
                &mut total_chars,
                &mut seen_facts,
                &mut seen_activity_sources,
                &mut domain_counts,
                &mut activity_window_facts,
                &mut selected_page_facts,
            );

            if facts.len() >= limit as usize {
                break;
            }
        }
    }

    facts
}

fn source_quotas_allow(
    fact: &PreparedFact,
    selected_page_facts: usize,
    selected_page_limit: usize,
    activity_window_facts: usize,
    seen_activity_sources: &std::collections::HashSet<String>,
) -> bool {
    if fact.kind == ContextSourceKind::SelectedPage && selected_page_facts >= selected_page_limit {
        return false;
    }

    if fact.kind == ContextSourceKind::ActivityWindow {
        if activity_window_facts >= MAX_ACTIVITY_WINDOW_FACTS {
            return false;
        }
        if seen_activity_sources.contains(&fact.source_key) {
            return false;
        }
    }

    true
}

fn accept_prepared_fact(
    fact: PreparedFact,
    facts: &mut Vec<String>,
    total_chars: &mut usize,
    seen_facts: &mut std::collections::HashSet<String>,
    seen_activity_sources: &mut std::collections::HashSet<String>,
    domain_counts: &mut std::collections::HashMap<String, usize>,
    activity_window_facts: &mut usize,
    selected_page_facts: &mut usize,
) {
    if fact.kind == ContextSourceKind::SelectedPage {
        *selected_page_facts += 1;
    }
    if fact.kind == ContextSourceKind::ActivityWindow {
        *activity_window_facts += 1;
        seen_activity_sources.insert(fact.source_key);
    }

    *total_chars += fact.text.len();
    *domain_counts.entry(fact.domain_key).or_default() += 1;
    seen_facts.insert(fact.fact_key);
    facts.push(fact.text);
}

fn has_diverse_domains(facts: &[PreparedFact]) -> bool {
    let mut domains = std::collections::HashSet::new();
    facts
        .iter()
        .any(|fact| domains.insert(fact.domain_key.as_str()) && domains.len() > 1)
}

fn domain_fact_limit(limit: u32, has_diverse_domains: bool) -> usize {
    let requested = limit as usize;
    if requested == 0 {
        return 0;
    }
    if has_diverse_domains {
        requested.saturating_sub(1).max(1)
    } else {
        requested
    }
}

fn selected_page_fact_limit(limit: u32, has_non_selected_page_fact: bool) -> usize {
    let requested = limit as usize;
    if requested == 0 {
        return 0;
    }
    if has_non_selected_page_fact {
        requested
            .saturating_sub(1)
            .clamp(1, MAX_SELECTED_PAGE_FACTS)
    } else {
        requested.min(MAX_SELECTED_PAGE_FACTS)
    }
}

fn push_query_term(terms: &mut Vec<String>, term: &str) {
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return;
    }
    let key = normalize_fact_key(trimmed);
    if terms
        .iter()
        .any(|existing| normalize_fact_key(existing) == key)
    {
        return;
    }
    terms.push(trimmed.to_string());
}

fn prepare_fact(candidate: ContextFactCandidate) -> Option<PreparedFact> {
    if security_block(&candidate.node.summary).is_some() {
        return None;
    }

    let sanitized = candidate.node.summary.replace('\n', " ").trim().to_string();
    let text = if sanitized.chars().count() > CONTEXT_FACT_MAX_CHARS {
        let mut truncated: String = sanitized.chars().take(CONTEXT_FACT_MAX_CHARS).collect();
        truncated.push_str("...");
        truncated
    } else {
        sanitized
    };
    let fact_key = normalize_fact_key(&text);
    if fact_key.is_empty() {
        return None;
    }

    let kind = context_source_kind(&candidate.node);
    let score = context_fact_score(&candidate.origin, kind);
    let source_key = source_diversity_key(&candidate.node, &text, kind);
    let domain_key = normalize_fact_key(&candidate.node.domain_context);

    Some(PreparedFact {
        text,
        fact_key,
        source_key,
        domain_key,
        kind,
        score,
        created_at_ms: candidate.node.created_at_ms,
        sequence: candidate.sequence,
    })
}

fn compare_prepared_facts(left: &PreparedFact, right: &PreparedFact) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| right.created_at_ms.cmp(&left.created_at_ms))
        .then_with(|| left.sequence.cmp(&right.sequence))
}

fn context_fact_score(origin: &CandidateOrigin, kind: ContextSourceKind) -> u32 {
    let base = match origin {
        CandidateOrigin::Search(score) => score.saturating_add(200),
        CandidateOrigin::RecentSelectedPage => 700,
    };
    let diversity_bonus = match kind {
        ContextSourceKind::SelectedPage => 80,
        ContextSourceKind::Other => 40,
        ContextSourceKind::WebCapture => 20,
        ContextSourceKind::ActivityWindow => 0,
    };

    base.saturating_add(diversity_bonus)
}

fn context_source_kind(node: &crate::identity::MemoryNode) -> ContextSourceKind {
    if node.domain_context == "local.web.capture" && node.summary.contains("selected text") {
        ContextSourceKind::SelectedPage
    } else if node.domain_context == "local.web.capture" {
        ContextSourceKind::WebCapture
    } else if node.domain_context == "local.activity.window" {
        ContextSourceKind::ActivityWindow
    } else {
        ContextSourceKind::Other
    }
}

fn source_diversity_key(
    node: &crate::identity::MemoryNode,
    text: &str,
    kind: ContextSourceKind,
) -> String {
    if kind == ContextSourceKind::ActivityWindow {
        if let Some(key) = activity_window_key(&node.structured_attributes) {
            return key;
        }
    }

    format!("{}:{}", node.domain_context, normalize_fact_key(text))
}

fn activity_window_key(structured_attributes: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(structured_attributes).ok()?;
    let application = value
        .get("application")
        .and_then(|field| field.as_str())
        .unwrap_or("")
        .trim();
    let title = value
        .get("window_title")
        .and_then(|field| field.as_str())
        .unwrap_or("")
        .trim();

    if application.is_empty() && title.is_empty() {
        None
    } else {
        Some(format!(
            "activity:{}:{}",
            normalize_fact_key(application),
            normalize_fact_key(title)
        ))
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
        let _ = fs::remove_dir_all(&root);
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
        let _ = fs::remove_dir_all(&root);
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
        let _ = fs::remove_dir_all(&root);
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
    fn context_with_profile_also_searches_active_window_title() {
        let root = std::env::temp_dir().join("identity-context-profile-window-query-test");
        let _ = fs::remove_dir_all(&root);
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 1,
                captured_event_id: 1,
                source: "manual".to_string(),
                cleaned_content: "Project profile memory should remain available.".to_string(),
                content_hash: "hash-profile-query".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();
        store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 2,
                captured_event_id: 2,
                source: "manual".to_string(),
                cleaned_content: "Oyster refund active-window memory should also appear."
                    .to_string(),
                content_hash: "hash-window-query".to_string(),
                cleaned_at_ms: 2,
                promoted_at_ms: None,
            })
            .unwrap();

        let snap = ContextSnapshot {
            process_name: "code.exe".to_string(),
            window_title: "Oyster refund workspace".to_string(),
            focused_text: None,
        };
        let profile = ProjectProfile {
            name: "identity".to_string(),
            window_filters: Vec::new(),
            guardrails: Vec::new(),
            memory_query_terms: vec!["Project profile memory".to_string()],
        };

        let ctx = build_identity_context(&paths, &snap, Some(&profile), 3).unwrap();
        assert!(ctx
            .facts
            .iter()
            .any(|fact| fact.contains("Project profile memory")));
        assert!(ctx
            .facts
            .iter()
            .any(|fact| fact.contains("Oyster refund active-window memory")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn test_context_deduplicates_repeated_fact_text() {
        let root = std::env::temp_dir().join("identity-context-dedup-test");
        let _ = fs::remove_dir_all(&root);
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
    fn context_ranking_collapses_repeated_window_memories_without_starving_diverse_facts() {
        let root = std::env::temp_dir().join("identity-context-diversity-test");
        let _ = fs::remove_dir_all(&root);
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        for id in 1..=3 {
            store
                .insert_memory_from_cleaned(&CleanedEvent {
                    id,
                    captured_event_id: id,
                    source: "windows-ui:active-window".to_string(),
                    cleaned_content: format!(
                        "Active application: msedge.exe\nActive window title: Google Gemini\nFocused control text: repeated focus {id}"
                    ),
                    content_hash: format!("hash-window-{id}"),
                    cleaned_at_ms: id,
                    promoted_at_ms: None,
                })
                .unwrap();
        }
        store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 10,
                captured_event_id: 10,
                source: "manual".to_string(),
                cleaned_content:
                    "Identity diversity project fact should stay in the context block.".to_string(),
                content_hash: "hash-project-fact".to_string(),
                cleaned_at_ms: 10,
                promoted_at_ms: None,
            })
            .unwrap();
        store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 11,
                captured_event_id: 11,
                source: "local-proxy:text/markdown".to_string(),
                cleaned_content: "Page title: Identity diversity notes\nPage URL: https://example.test/diversity\nSelected page text:\nSelected page context should survive the diversity ranking.".to_string(),
                content_hash: "hash-selected-diverse".to_string(),
                cleaned_at_ms: 11,
                promoted_at_ms: None,
            })
            .unwrap();

        let snap = ContextSnapshot {
            process_name: "msedge.exe".to_string(),
            window_title: "Google Gemini".to_string(),
            focused_text: None,
        };
        let profile = ProjectProfile {
            name: "identity".to_string(),
            window_filters: vec!["identity".to_string()],
            guardrails: vec!["Keep the context local and scoped.".to_string()],
            memory_query_terms: vec![
                "Google Gemini".to_string(),
                "Identity diversity".to_string(),
            ],
        };

        let ctx = build_identity_context(&paths, &snap, Some(&profile), 3).unwrap();
        let window_fact_count = ctx
            .facts
            .iter()
            .filter(|fact| fact.contains("UI activity in msedge.exe"))
            .count();

        assert_eq!(window_fact_count, 1);
        assert!(ctx
            .facts
            .iter()
            .any(|fact| fact.contains("Identity diversity project fact")));
        assert!(ctx
            .facts
            .iter()
            .any(|fact| fact.contains("Selected page context should survive")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn context_ranking_keeps_selected_page_fallback_from_monopolizing_fact_slots() {
        let root = std::env::temp_dir().join("identity-context-selected-page-quota-test");
        let _ = fs::remove_dir_all(&root);
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        for id in 1..=3 {
            store
                .insert_memory_from_cleaned(&CleanedEvent {
                    id,
                    captured_event_id: id,
                    source: "local-proxy:text/markdown".to_string(),
                    cleaned_content: format!(
                        "Page title: Page {id}\nPage URL: https://example.test/page-{id}\nSelected page text:\nFresh selected page fallback {id}."
                    ),
                    content_hash: format!("hash-page-quota-{id}"),
                    cleaned_at_ms: id,
                    promoted_at_ms: None,
                })
                .unwrap();
        }
        store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 10,
                captured_event_id: 10,
                source: "manual".to_string(),
                cleaned_content: "Profile-specific quota memory must not be crowded out."
                    .to_string(),
                content_hash: "hash-profile-quota".to_string(),
                cleaned_at_ms: 10,
                promoted_at_ms: None,
            })
            .unwrap();

        let snap = ContextSnapshot {
            process_name: "chrome.exe".to_string(),
            window_title: "Google Gemini".to_string(),
            focused_text: None,
        };
        let profile = ProjectProfile {
            name: "identity".to_string(),
            window_filters: Vec::new(),
            guardrails: Vec::new(),
            memory_query_terms: vec!["Profile-specific quota memory".to_string()],
        };

        let ctx = build_identity_context(&paths, &snap, Some(&profile), 3).unwrap();
        let selected_page_count = ctx
            .facts
            .iter()
            .filter(|fact| fact.contains("Fresh selected page fallback"))
            .count();

        assert!(selected_page_count <= 2);
        assert!(ctx
            .facts
            .iter()
            .any(|fact| fact.contains("Profile-specific quota memory")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn context_ranking_keeps_one_domain_from_monopolizing_when_another_domain_is_available() {
        let mut candidates = Vec::new();
        for id in 1..=3 {
            candidates.push(test_context_candidate(
                id,
                &format!("Manual profile memory fact {id}."),
                "local.capture",
                300,
            ));
        }
        candidates.push(test_context_candidate(
            4,
            "Filesystem project document fact.",
            "local.filesystem",
            100,
        ));

        let facts = rank_memory_facts(candidates, 3);
        let manual_count = facts
            .iter()
            .filter(|fact| fact.contains("Manual profile memory fact"))
            .count();

        assert_eq!(facts.len(), 3);
        assert_eq!(manual_count, 2);
        assert!(facts
            .iter()
            .any(|fact| fact == "Filesystem project document fact."));
    }

    #[test]
    fn context_ranking_relaxes_domain_cap_to_fill_slots_when_other_domain_does_not_fit() {
        let long_tail = "budget filler ".repeat(40);
        let mut candidates = Vec::new();

        for id in 1..=3 {
            candidates.push(test_context_candidate(
                id,
                &format!("Manual long budget fact {id}: {long_tail}"),
                "local.capture",
                300,
            ));
        }
        candidates.push(test_context_candidate(
            4,
            "Manual short fallback fact.",
            "local.capture",
            250,
        ));
        candidates.push(test_context_candidate(
            5,
            &format!("Filesystem oversized alternate fact: {long_tail}"),
            "local.filesystem",
            100,
        ));

        let facts = rank_memory_facts(candidates, 4);

        assert_eq!(facts.len(), 4);
        assert!(facts
            .iter()
            .any(|fact| fact == "Manual short fallback fact."));
        assert!(!facts
            .iter()
            .any(|fact| fact.contains("Filesystem oversized alternate fact")));
    }

    #[test]
    fn context_ranking_skips_oversized_fact_to_pack_shorter_later_fact() {
        let long_tail = "budget filler ".repeat(40);
        let mut candidates = Vec::new();

        for id in 1..=4 {
            candidates.push(test_context_candidate(
                id,
                &format!("Budget packing long fact {id}: {long_tail}"),
                "local.capture",
                100,
            ));
        }
        candidates.push(test_context_candidate(
            0,
            "Tiny budget packing fact.",
            "local.capture",
            100,
        ));

        let facts = rank_memory_facts(candidates, 5);

        assert_eq!(facts.len(), 4);
        assert!(facts.iter().any(|fact| fact == "Tiny budget packing fact."));
    }

    #[test]
    fn browser_context_includes_recent_selected_page_capture() {
        let root = std::env::temp_dir().join("identity-context-recent-page-test");
        let _ = fs::remove_dir_all(&root);
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
        let _ = fs::remove_dir_all(&root);
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

    fn test_context_candidate(
        id: i64,
        summary: &str,
        domain_context: &str,
        score: u32,
    ) -> ContextFactCandidate {
        ContextFactCandidate {
            node: crate::identity::MemoryNode {
                id,
                node_uid: format!("00000000-0000-4000-8000-{id:012}"),
                cleaned_event_id: id,
                source: "test".to_string(),
                domain_context: domain_context.to_string(),
                entity_type: "DOCUMENT".to_string(),
                summary: summary.to_string(),
                structured_attributes: "{}".to_string(),
                raw_text: summary.to_string(),
                content_hash: format!("hash-{id}"),
                created_at_ms: id,
                created_at_utc: "1970-01-01T00:00:00.000Z".to_string(),
                last_accessed_ms: id,
                last_accessed_utc: "1970-01-01T00:00:00.000Z".to_string(),
            },
            origin: CandidateOrigin::Search(score),
            sequence: id as usize,
        }
    }
}
