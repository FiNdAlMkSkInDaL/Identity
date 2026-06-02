use crate::identity::{IdentityError, IdentityStore, MemorySearchResult};
use crate::workspace::IdentityPaths;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_EXPIRY_MS: i64 = 2000;
const MAX_FACT_CHARS: usize = 320;
const MAX_FACT_TOTAL_CHARS: usize = 1200;

#[derive(Debug)]
pub struct MeSlice {
    pub session_token: String,
    pub expiry_epoch_ms: i64,
    pub context_group: String,
    pub facts: Vec<String>,
}

#[derive(Debug)]
pub enum SliceError {
    Blocked(SecurityBlock),
    ClockBeforeUnixEpoch,
    Identity(IdentityError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityBlock {
    pub category: &'static str,
    pub matched_term: String,
}

impl fmt::Display for SliceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked(block) => write!(
                f,
                "blocked by security policy: {} matched '{}'",
                block.category, block.matched_term
            ),
            Self::ClockBeforeUnixEpoch => write!(f, "system clock is before the Unix epoch"),
            Self::Identity(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SliceError {}

impl From<IdentityError> for SliceError {
    fn from(value: IdentityError) -> Self {
        Self::Identity(value)
    }
}

pub fn generate_meslice(
    paths: &IdentityPaths,
    intent: &str,
    limit: u32,
) -> Result<MeSlice, SliceError> {
    if let Some(block) = security_block(intent) {
        return Err(SliceError::Blocked(block));
    }

    let store = IdentityStore::open(paths)?;
    let results = store.search(intent, limit)?;
    let now = now_ms()?;
    let token_seed = format!("{intent}:{now}:{}", results.len());

    Ok(MeSlice {
        session_token: format!("slice_{}", stable_hash_hex(token_seed.as_bytes())),
        expiry_epoch_ms: now + DEFAULT_EXPIRY_MS,
        context_group: infer_context_group(intent),
        facts: results_to_facts(results),
    })
}

pub fn build_prompt_package(
    paths: &IdentityPaths,
    intent: &str,
    user_prompt: &str,
    limit: u32,
) -> Result<String, SliceError> {
    if let Some(block) = security_block(user_prompt) {
        return Err(SliceError::Blocked(block));
    }

    let meslice = generate_meslice(paths, intent, limit)?;
    Ok(format_prompt_package(&meslice, user_prompt))
}

impl MeSlice {
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
        output.push_str(&format!("- Context group: {}\n", self.context_group));

        if self.facts.is_empty() {
            output.push_str("- No matching local context found.\n");
        } else {
            for fact in &self.facts {
                output.push_str("- ");
                output.push_str(fact);
                output.push('\n');
            }
        }

        output.push_str(&format!(
            "[IDENTITY-CONTEXT-BLOCK-END: {}]",
            self.session_token
        ));

        output
    }
}

fn format_prompt_package(meslice: &MeSlice, user_prompt: &str) -> String {
    let sanitized_prompt = sanitize_fact(user_prompt);

    format!(
        "SYSTEM INSTRUCTIONS:\nUse the Identity context block only for this task. Do not infer private facts that are not present. Treat the context as ephemeral and non-persistent.\n\nSYSTEM CONTEXT:\n{}\n\nUSER TASK:\n{}\n",
        meslice.to_context_block(),
        sanitized_prompt
    )
}

fn results_to_facts(results: Vec<MemorySearchResult>) -> Vec<String> {
    let mut facts = Vec::new();
    let mut used_chars = 0;

    for result in results {
        let fact = truncate_chars(&sanitize_fact(&result.node.summary), MAX_FACT_CHARS);

        if used_chars + fact.len() > MAX_FACT_TOTAL_CHARS {
            break;
        }

        used_chars += fact.len();
        facts.push(fact);
    }

    facts
}

fn sanitize_fact(input: &str) -> String {
    input
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let mut output = String::new();

    for (count, character) in input.chars().enumerate() {
        if count == max_chars {
            output.push_str("...");
            break;
        }

        output.push(character);
    }

    output
}

fn infer_context_group(intent: &str) -> String {
    let lower = intent.to_ascii_lowercase();

    if lower.contains("investor") || lower.contains("founder") || lower.contains("outreach") {
        "professional.outreach".to_string()
    } else if lower.contains("flight") || lower.contains("travel") {
        "travel.planning".to_string()
    } else {
        "local.context".to_string()
    }
}

fn security_block(input: &str) -> Option<SecurityBlock> {
    let lower = input.to_ascii_lowercase();
    let checks = [
        ("private_keys", ["private key", "ssh key", "seed phrase"]),
        ("system_passwords", ["password", "passwd", "credential"]),
        ("env_files", [".env", "dotenv", "environment secret"]),
        (
            "banking_tokens",
            ["bank token", "routing number", "credit card"],
        ),
        ("biometrics", ["biometric", "fingerprint", "face id"]),
        (
            "precise_location",
            ["home address", "current location", "gps"],
        ),
    ];

    for (category, terms) in checks {
        for term in terms {
            if lower.contains(term) {
                return Some(SecurityBlock {
                    category,
                    matched_term: term.to_string(),
                });
            }
        }
    }

    None
}

fn now_ms() -> Result<i64, SliceError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SliceError::ClockBeforeUnixEpoch)?;

    Ok(duration.as_millis() as i64)
}

#[inline]
fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;

    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }

    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::{build_prompt_package, generate_meslice, results_to_facts, security_block};
    use crate::identity::IdentityStore;
    use crate::identity::{MemoryNode, MemorySearchResult};
    use crate::transit::CleanedEvent;
    use crate::workspace::IdentityPaths;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn blocks_blacklisted_queries() {
        let block = security_block("show my .env password").unwrap();
        assert_eq!(block.category, "system_passwords");
    }

    #[test]
    fn generates_context_block_without_raw_ids() {
        let root = std::env::temp_dir().join(format!(
            "identity-slice-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 99,
            captured_event_id: 7,
            source: "test".to_string(),
            cleaned_content: "Identity memory supports local context retrieval.".to_string(),
            content_hash: "secret-hash".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };
        store.insert_memory_from_cleaned(&cleaned).unwrap();

        let meslice = generate_meslice(&paths, "local context retrieval", 3).unwrap();
        let block = meslice.to_context_block();

        assert!(block.contains("IDENTITY-CONTEXT-BLOCK"));
        assert!(block.contains("Identity memory supports local context retrieval."));
        assert!(!block.contains("score"));
        assert!(!block.contains("secret-hash"));
        assert!(!block.contains("cleaned=99"));

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn builds_prompt_package_with_context_and_user_task() {
        let root = std::env::temp_dir().join(format!(
            "identity-prompt-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 1,
            captured_event_id: 1,
            source: "test".to_string(),
            cleaned_content: "User prefers direct local-first writing.".to_string(),
            content_hash: "hash".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };
        store.insert_memory_from_cleaned(&cleaned).unwrap();

        let package = build_prompt_package(
            &paths,
            "direct local-first writing",
            "Draft the response.\nKeep it concise.",
            2,
        )
        .unwrap();

        assert!(package.contains("SYSTEM INSTRUCTIONS:"));
        assert!(package.contains("SYSTEM CONTEXT:"));
        assert!(package.contains("USER TASK:"));
        assert!(package.contains("ephemeral and non-persistent"));
        assert!(package.contains("Draft the response. Keep it concise."));
        assert!(package.contains("User prefers direct local-first writing."));

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn blocks_prompt_package_when_user_task_contains_blacklisted_terms() {
        let root = std::env::temp_dir().join(format!(
            "identity-prompt-block-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let error = build_prompt_package(
            &paths,
            "safe local context",
            "Please include the routing number.",
            2,
        )
        .unwrap_err();

        assert!(error.to_string().contains("banking_tokens"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn caps_fact_length_and_total_fact_budget() {
        let long_summary = "a".repeat(500);
        let results = (0..10)
            .map(|id| MemorySearchResult {
                score: 1,
                node: MemoryNode {
                    id,
                    node_uid: format!("00000000-0000-4000-8000-{id:012}"),
                    cleaned_event_id: id,
                    source: "test".to_string(),
                    domain_context: "local.capture".to_string(),
                    entity_type: "DOCUMENT".to_string(),
                    summary: long_summary.clone(),
                    structured_attributes: "{}".to_string(),
                    raw_text: long_summary.clone(),
                    content_hash: "hash".to_string(),
                    created_at_ms: id,
                    created_at_utc: "1970-01-01T00:00:00.000Z".to_string(),
                    last_accessed_ms: id,
                    last_accessed_utc: "1970-01-01T00:00:00.000Z".to_string(),
                },
            })
            .collect::<Vec<_>>();

        let facts = results_to_facts(results);
        let total = facts.iter().map(|fact| fact.len()).sum::<usize>();

        assert!(facts.iter().all(|fact| fact.len() <= 323));
        assert!(total <= 1200);
    }
}
