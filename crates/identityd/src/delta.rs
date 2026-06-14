use crate::ingest_safety::{validate_capture, IngestSafetyError};
use crate::slice::security_block;
use crate::transit::CleanedEvent;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub const AGENT_DELTA_SOURCE_PREFIX: &str = "agent-delta:";
pub const AGENT_DELTA_SCHEMA_VERSION: u32 = 1;
const DEFAULT_AGENT_DELTA_SOURCE: &str = "agent-delta:manual";
const MAX_DELTA_SUMMARY_CHARS: usize = 420;
const MAX_DELTA_ENTITY_CHARS: usize = 80;
const MAX_DELTA_ATTRIBUTE_VALUE_CHARS: usize = 240;
const MAX_DELTA_ENTITIES: usize = 6;
const MAX_DELTA_ATTRIBUTES: usize = 8;
const MAX_DELTA_ATTRIBUTE_KEY_CHARS: usize = 64;
const MAX_DELTA_SOURCE_LABEL_CHARS: usize = 80;
const ALLOWED_OUTCOME_STATES: &[&str] = &[
    "FAILED",
    "PAID",
    "CANCELLED",
    "SENT",
    "SCHEDULED",
    "CONFIRMED",
    "UPDATED",
    "CREATED",
    "SUCCESS",
    "OBSERVED",
];
const ALLOWED_REVIEW_CATEGORIES: &[&str] = &[
    "finance",
    "health",
    "legal_identity",
    "private_communications",
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentDelta {
    pub schema_version: u32,
    pub source: String,
    pub outcome_state: String,
    pub summary: String,
    pub entities: Vec<String>,
    pub attributes: Vec<DeltaAttribute>,
    pub requires_review: bool,
    pub review_required_categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeltaAttribute {
    pub key: String,
    pub value: String,
}

#[derive(Debug)]
pub enum AgentDeltaError {
    EmptyInput,
    InvalidSchema(String),
    Safety(IngestSafetyError),
    Json(serde_json::Error),
    ClockBeforeUnixEpoch,
}

impl fmt::Display for AgentDeltaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "agent delta input is empty"),
            Self::InvalidSchema(reason) => write!(f, "invalid agent delta schema: {reason}"),
            Self::Safety(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::ClockBeforeUnixEpoch => write!(f, "system clock is before the Unix epoch"),
        }
    }
}

impl std::error::Error for AgentDeltaError {}

impl From<IngestSafetyError> for AgentDeltaError {
    fn from(value: IngestSafetyError) -> Self {
        Self::Safety(value)
    }
}

impl From<serde_json::Error> for AgentDeltaError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub fn extract_agent_delta(
    input: &str,
    source: Option<&str>,
) -> Result<AgentDelta, AgentDeltaError> {
    let source = normalize_agent_delta_source(source);
    let normalized = normalize_delta_text(input);
    if normalized.is_empty() {
        return Err(AgentDeltaError::EmptyInput);
    }
    validate_capture(&source, &normalized)?;
    if let Some(block) = security_block(&normalized) {
        return Err(AgentDeltaError::Safety(IngestSafetyError::BlockedContent(
            format!(
                "security blacklist marker '{}' in {}",
                block.matched_term, block.category
            ),
        )));
    }

    let review_required_categories = review_required_categories(&normalized);
    let delta = AgentDelta {
        schema_version: AGENT_DELTA_SCHEMA_VERSION,
        source,
        outcome_state: outcome_state(&normalized).to_string(),
        summary: truncate_chars(
            &first_meaningful_sentence(&normalized),
            MAX_DELTA_SUMMARY_CHARS,
        ),
        entities: extract_entities(&normalized),
        attributes: extract_attributes(&normalized),
        requires_review: !review_required_categories.is_empty(),
        review_required_categories,
    };
    delta.validate()?;
    Ok(delta)
}

pub fn agent_delta_from_json(input: &str) -> Result<AgentDelta, AgentDeltaError> {
    let delta: AgentDelta = serde_json::from_str(input)?;
    delta.validate_safety()?;
    Ok(delta)
}

pub fn agent_delta_schema_json() -> Result<String, AgentDeltaError> {
    let candidate_template = AgentDelta {
        schema_version: AGENT_DELTA_SCHEMA_VERSION,
        source: DEFAULT_AGENT_DELTA_SOURCE.to_string(),
        outcome_state: "OBSERVED".to_string(),
        summary: "Brief reviewed outcome summary.".to_string(),
        entities: vec!["Example Entity".to_string()],
        attributes: vec![DeltaAttribute {
            key: "reference".to_string(),
            value: "optional bounded value".to_string(),
        }],
        requires_review: false,
        review_required_categories: Vec::new(),
    };
    let schema = serde_json::json!({
        "schema_version": AGENT_DELTA_SCHEMA_VERSION,
        "source_prefix": AGENT_DELTA_SOURCE_PREFIX,
        "default_source": DEFAULT_AGENT_DELTA_SOURCE,
        "allowed_outcome_states": ALLOWED_OUTCOME_STATES,
        "allowed_review_required_categories": ALLOWED_REVIEW_CATEGORIES,
        "limits": {
            "max_summary_chars": MAX_DELTA_SUMMARY_CHARS,
            "max_entities": MAX_DELTA_ENTITIES,
            "max_entity_chars": MAX_DELTA_ENTITY_CHARS,
            "max_attributes": MAX_DELTA_ATTRIBUTES,
            "max_attribute_key_chars": MAX_DELTA_ATTRIBUTE_KEY_CHARS,
            "max_attribute_value_chars": MAX_DELTA_ATTRIBUTE_VALUE_CHARS,
            "max_source_label_chars": MAX_DELTA_SOURCE_LABEL_CHARS
        },
        "rules": {
            "source": "agent-delta:<bounded lowercase slug>",
            "outcome_state": "one of allowed_outcome_states",
            "attribute_key": "lowercase alphanumeric snake_case",
            "requires_review": "must be true exactly when review_required_categories is non-empty",
            "unknown_fields": "rejected"
        },
        "candidate_template": candidate_template
    });

    Ok(serde_json::to_string_pretty(&schema)?)
}

pub fn agent_delta_validation_json(delta: &AgentDelta) -> Result<String, AgentDeltaError> {
    delta.validate()?;
    let output = serde_json::json!({
        "valid": true,
        "schema_version": delta.schema_version,
        "source": delta.source,
        "outcome_state": delta.outcome_state,
        "requires_review": delta.requires_review(),
        "commit_requires_allow_sensitive": delta.requires_review(),
        "review_required_categories": delta.review_required_categories,
        "entities_count": delta.entities.len(),
        "attributes_count": delta.attributes.len()
    });

    Ok(serde_json::to_string_pretty(&output)?)
}

impl AgentDelta {
    pub fn validate(&self) -> Result<(), AgentDeltaError> {
        if self.schema_version != AGENT_DELTA_SCHEMA_VERSION {
            return Err(AgentDeltaError::InvalidSchema(format!(
                "schema_version must be {AGENT_DELTA_SCHEMA_VERSION}"
            )));
        }

        if !self.source.starts_with(AGENT_DELTA_SOURCE_PREFIX) {
            return Err(AgentDeltaError::InvalidSchema(
                "source must use the agent-delta: prefix".to_string(),
            ));
        }
        let source_label = self
            .source
            .strip_prefix(AGENT_DELTA_SOURCE_PREFIX)
            .unwrap_or_default();
        if source_label.is_empty()
            || source_label.chars().count() > MAX_DELTA_SOURCE_LABEL_CHARS
            || !is_valid_source_label(source_label)
        {
            return Err(AgentDeltaError::InvalidSchema(
                "source label must be a bounded lowercase slug".to_string(),
            ));
        }

        if !ALLOWED_OUTCOME_STATES
            .iter()
            .any(|state| *state == self.outcome_state)
        {
            return Err(AgentDeltaError::InvalidSchema(
                "outcome_state is not recognized".to_string(),
            ));
        }

        if self.summary.trim().is_empty()
            || self.summary.chars().count() > MAX_DELTA_SUMMARY_CHARS + 3
        {
            return Err(AgentDeltaError::InvalidSchema(
                "summary must be non-empty and bounded".to_string(),
            ));
        }

        if self.entities.len() > MAX_DELTA_ENTITIES {
            return Err(AgentDeltaError::InvalidSchema(
                "too many entities".to_string(),
            ));
        }
        let mut seen_entities = Vec::new();
        for entity in &self.entities {
            if entity.trim().is_empty() || entity.chars().count() > MAX_DELTA_ENTITY_CHARS + 3 {
                return Err(AgentDeltaError::InvalidSchema(
                    "entities must be non-empty and bounded".to_string(),
                ));
            }
            let key = entity.to_ascii_lowercase();
            if seen_entities.iter().any(|seen| seen == &key) {
                return Err(AgentDeltaError::InvalidSchema(
                    "duplicate entities are not allowed".to_string(),
                ));
            }
            seen_entities.push(key);
        }

        if self.attributes.len() > MAX_DELTA_ATTRIBUTES {
            return Err(AgentDeltaError::InvalidSchema(
                "too many attributes".to_string(),
            ));
        }
        let mut seen_attributes = Vec::new();
        for attribute in &self.attributes {
            if !is_valid_attribute_key(&attribute.key)
                || attribute.key.chars().count() > MAX_DELTA_ATTRIBUTE_KEY_CHARS
            {
                return Err(AgentDeltaError::InvalidSchema(
                    "attribute keys must be lowercase alphanumeric snake_case".to_string(),
                ));
            }
            if attribute.value.trim().is_empty()
                || attribute.value.chars().count() > MAX_DELTA_ATTRIBUTE_VALUE_CHARS + 3
            {
                return Err(AgentDeltaError::InvalidSchema(
                    "attribute values must be non-empty and bounded".to_string(),
                ));
            }
            if seen_attributes.iter().any(|seen| seen == &attribute.key) {
                return Err(AgentDeltaError::InvalidSchema(
                    "duplicate attributes are not allowed".to_string(),
                ));
            }
            seen_attributes.push(attribute.key.clone());
        }

        if self.requires_review != !self.review_required_categories.is_empty() {
            return Err(AgentDeltaError::InvalidSchema(
                "requires_review must match review_required_categories".to_string(),
            ));
        }
        let mut seen_review_categories = Vec::new();
        for category in &self.review_required_categories {
            if !ALLOWED_REVIEW_CATEGORIES
                .iter()
                .any(|allowed| allowed == category)
            {
                return Err(AgentDeltaError::InvalidSchema(
                    "review category is not recognized".to_string(),
                ));
            }
            if seen_review_categories.iter().any(|seen| seen == category) {
                return Err(AgentDeltaError::InvalidSchema(
                    "duplicate review categories are not allowed".to_string(),
                ));
            }
            seen_review_categories.push(category.clone());
        }

        Ok(())
    }

    pub fn to_json(&self) -> Result<String, AgentDeltaError> {
        self.validate()?;
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn to_cleaned_content(&self) -> String {
        let mut output = String::new();
        output.push_str("Agent outcome delta\n");
        output.push_str(&format!("Outcome state: {}\n", self.outcome_state));
        output.push_str(&format!("Delta source: {}\n", self.source));
        output.push_str(&format!("Summary: {}\n", self.summary));

        for entity in &self.entities {
            output.push_str(&format!("Entity: {entity}\n"));
        }
        for attribute in &self.attributes {
            output.push_str(&format!(
                "Attribute {}: {}\n",
                attribute.key, attribute.value
            ));
        }
        if !self.review_required_categories.is_empty() {
            output.push_str(&format!(
                "Review required categories: {}\n",
                self.review_required_categories.join(", ")
            ));
        }

        output.trim_end().to_string()
    }

    pub fn to_cleaned_event(&self) -> Result<CleanedEvent, AgentDeltaError> {
        self.validate_safety()?;
        let cleaned_content = self.to_cleaned_content();
        let now = now_ms()?;
        let content_hash = stable_hash_hex(cleaned_content.as_bytes());

        Ok(CleanedEvent {
            id: stable_negative_id(&content_hash),
            captured_event_id: 0,
            source: self.source.clone(),
            cleaned_content: cleaned_content.clone(),
            content_hash,
            cleaned_at_ms: now,
            promoted_at_ms: None,
        })
    }

    pub fn requires_review(&self) -> bool {
        self.requires_review
    }

    fn validate_safety(&self) -> Result<(), AgentDeltaError> {
        self.validate()?;
        let cleaned_content = self.to_cleaned_content();
        validate_capture(&self.source, &cleaned_content)?;
        if let Some(block) = security_block(&cleaned_content) {
            return Err(AgentDeltaError::Safety(IngestSafetyError::BlockedContent(
                format!(
                    "security blacklist marker '{}' in {}",
                    block.matched_term, block.category
                ),
            )));
        }
        Ok(())
    }
}

pub fn normalize_agent_delta_source(source: Option<&str>) -> String {
    let source = source.unwrap_or(DEFAULT_AGENT_DELTA_SOURCE).trim();
    if source.is_empty() {
        DEFAULT_AGENT_DELTA_SOURCE.to_string()
    } else if source.starts_with(AGENT_DELTA_SOURCE_PREFIX) {
        format!(
            "{AGENT_DELTA_SOURCE_PREFIX}{}",
            normalize_source_label(source.trim_start_matches(AGENT_DELTA_SOURCE_PREFIX))
        )
    } else {
        format!(
            "{AGENT_DELTA_SOURCE_PREFIX}{}",
            normalize_source_label(source)
        )
    }
}

fn normalize_source_label(label: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_separator = false;

    for character in label.trim().chars() {
        let next = if character.is_ascii_alphanumeric() {
            Some(character.to_ascii_lowercase())
        } else if matches!(character, '-' | '_' | '.' | ':') {
            Some(character)
        } else if character.is_whitespace() || character.is_ascii_punctuation() {
            Some('-')
        } else {
            None
        };

        let Some(next) = next else {
            continue;
        };
        let is_separator = matches!(next, '-' | '_' | '.' | ':');
        if is_separator && last_was_separator {
            continue;
        }
        normalized.push(next);
        last_was_separator = is_separator;
        if normalized.chars().count() >= MAX_DELTA_SOURCE_LABEL_CHARS {
            break;
        }
    }

    let normalized =
        normalized.trim_matches(|character| matches!(character, '-' | '_' | '.' | ':'));
    if normalized.is_empty() {
        "manual".to_string()
    } else {
        normalized.to_string()
    }
}

fn is_valid_source_label(label: &str) -> bool {
    label.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '-' | '_' | '.' | ':')
    })
}

fn normalize_delta_text(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn outcome_state(text: &str) -> &'static str {
    let lower = text.to_ascii_lowercase();

    if contains_any(&lower, &["failed", "error", "declined", "rejected"]) {
        "FAILED"
    } else if contains_any(&lower, &["paid", "payment", "invoice paid"]) {
        "PAID"
    } else if contains_any(&lower, &["cancelled", "canceled"]) {
        "CANCELLED"
    } else if contains_any(&lower, &["sent", "emailed", "replied", "message delivered"]) {
        "SENT"
    } else if contains_any(&lower, &["scheduled", "calendar", "meeting booked"]) {
        "SCHEDULED"
    } else if contains_any(&lower, &["booked", "reserved", "confirmed", "confirmation"]) {
        "CONFIRMED"
    } else if contains_any(&lower, &["updated", "changed", "modified"]) {
        "UPDATED"
    } else if contains_any(&lower, &["created", "added"]) {
        "CREATED"
    } else if contains_any(&lower, &["completed", "done", "success"]) {
        "SUCCESS"
    } else {
        "OBSERVED"
    }
}

fn review_required_categories(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut categories = Vec::new();

    push_review_category(
        &mut categories,
        "finance",
        &lower,
        &[
            "invoice",
            "payment",
            "paid",
            "wire",
            "bank",
            "refund",
            "receipt",
            "purchase",
            "subscription",
            "expense",
        ],
    );
    push_review_category(
        &mut categories,
        "health",
        &lower,
        &[
            "medical",
            "doctor",
            "patient",
            "prescription",
            "diagnosis",
            "health",
            "therapy",
        ],
    );
    push_review_category(
        &mut categories,
        "legal_identity",
        &lower,
        &[
            "passport",
            "driver license",
            "driving licence",
            "national insurance",
            "social security",
            "visa",
            "tax id",
            "signature",
            "contract",
        ],
    );
    push_review_category(
        &mut categories,
        "private_communications",
        &lower,
        &[
            "email",
            "emailed",
            "message",
            "dm",
            "slack",
            "whatsapp",
            "signal",
            "private thread",
        ],
    );

    categories
}

fn push_review_category(
    categories: &mut Vec<String>,
    category: &str,
    lower_text: &str,
    markers: &[&str],
) {
    if markers.iter().any(|marker| lower_text.contains(marker))
        && !categories.iter().any(|existing| existing == category)
    {
        categories.push(category.to_string());
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn first_meaningful_sentence(text: &str) -> String {
    let end = text
        .char_indices()
        .find_map(|(index, character)| {
            if matches!(character, '.' | '!' | '?') {
                Some(index + character.len_utf8())
            } else {
                None
            }
        })
        .unwrap_or(text.len());

    text[..end].trim().to_string()
}

fn extract_entities(text: &str) -> Vec<String> {
    let mut entities = Vec::new();
    let mut current = Vec::new();

    for token in text.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '&');
        let sentence_boundary = token.ends_with('.')
            || token.ends_with('!')
            || token.ends_with('?')
            || token.ends_with(';');
        let starts_entity = cleaned
            .chars()
            .next()
            .map(|character| character.is_ascii_uppercase())
            .unwrap_or(false)
            && cleaned
                .chars()
                .any(|character| character.is_ascii_lowercase());

        if starts_entity {
            current.push(cleaned);
            if sentence_boundary {
                push_entity(&mut entities, &mut current);
                if entities.len() >= MAX_DELTA_ENTITIES {
                    break;
                }
            }
            continue;
        }

        push_entity(&mut entities, &mut current);
        if entities.len() >= MAX_DELTA_ENTITIES {
            break;
        }
    }
    push_entity(&mut entities, &mut current);

    entities
}

fn push_entity(entities: &mut Vec<String>, current: &mut Vec<&str>) {
    if current.is_empty() || entities.len() >= MAX_DELTA_ENTITIES {
        current.clear();
        return;
    }

    while current
        .first()
        .map(|word| is_entity_stop_word(word))
        .unwrap_or(false)
    {
        current.remove(0);
    }
    while current
        .last()
        .map(|word| is_entity_stop_word(word))
        .unwrap_or(false)
    {
        current.pop();
    }

    let entity = truncate_chars(&current.join(" "), MAX_DELTA_ENTITY_CHARS);
    let key = entity.to_ascii_lowercase();
    if !entity.is_empty()
        && !is_entity_stop_word(&key)
        && !entities
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&entity))
    {
        entities.push(entity);
    }
    current.clear();
}

fn is_entity_stop_word(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "agent"
            | "outcome"
            | "summary"
            | "sent"
            | "updated"
            | "created"
            | "completed"
            | "confirmed"
            | "scheduled"
            | "booked"
            | "paid"
            | "confirmation"
            | "reference"
            | "receipt"
            | "next"
            | "action"
    )
}

fn extract_attributes(text: &str) -> Vec<DeltaAttribute> {
    let mut attributes = Vec::new();
    for part in text.split([';', '\n']) {
        if attributes.len() >= MAX_DELTA_ATTRIBUTES {
            break;
        }
        let Some((key, value)) = part.split_once(':') else {
            continue;
        };
        let key = normalize_attribute_key(key);
        let value = truncate_chars(value.trim(), MAX_DELTA_ATTRIBUTE_VALUE_CHARS);
        if key.is_empty()
            || value.is_empty()
            || attributes
                .iter()
                .any(|attr: &DeltaAttribute| attr.key == key)
        {
            continue;
        }
        attributes.push(DeltaAttribute { key, value });
    }

    attributes
}

fn normalize_attribute_key(key: &str) -> String {
    let normalized = key
        .trim()
        .rsplit(['.', '!', '?'])
        .next()
        .unwrap_or(key)
        .trim()
        .chars()
        .filter_map(|character| {
            if character.is_ascii_alphanumeric() {
                Some(character.to_ascii_lowercase())
            } else if character.is_whitespace() || matches!(character, '-' | '_') {
                Some('_')
            } else {
                None
            }
        })
        .collect::<String>();

    normalized
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn is_valid_attribute_key(key: &str) -> bool {
    let mut previous_underscore = false;
    let mut saw_character = false;

    for character in key.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            previous_underscore = false;
            saw_character = true;
        } else if character == '_' {
            if previous_underscore || !saw_character {
                return false;
            }
            previous_underscore = true;
        } else {
            return false;
        }
    }

    saw_character && !previous_underscore
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn now_ms() -> Result<i64, AgentDeltaError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AgentDeltaError::ClockBeforeUnixEpoch)?
        .as_millis() as i64)
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn stable_negative_id(hash_hex: &str) -> i64 {
    let parsed = u64::from_str_radix(hash_hex, 16).unwrap_or(1);
    let positive = (parsed & 0x7fff_ffff_ffff_ffff).max(1) as i64;
    -positive
}

#[cfg(test)]
mod tests {
    use super::{
        agent_delta_from_json, agent_delta_schema_json, agent_delta_validation_json,
        extract_agent_delta, DeltaAttribute, AGENT_DELTA_SCHEMA_VERSION, AGENT_DELTA_SOURCE_PREFIX,
    };

    #[test]
    fn extracts_bounded_outcome_delta() {
        let delta = extract_agent_delta(
            "Sent follow-up message to Acme Capital. Confirmation reference: MSG-42; Next action: wait for reply.",
            Some("follow-up"),
        )
        .unwrap();

        assert_eq!(delta.source, "agent-delta:follow-up");
        assert_eq!(delta.schema_version, AGENT_DELTA_SCHEMA_VERSION);
        assert_eq!(delta.outcome_state, "SENT");
        assert!(delta.summary.contains("Sent follow-up"));
        assert!(delta.entities.iter().any(|entity| entity == "Acme Capital"));
        assert!(delta
            .attributes
            .iter()
            .any(|attr| attr.key == "confirmation_reference" && attr.value == "MSG-42"));
        assert!(delta.to_cleaned_content().contains("Outcome state: SENT"));
        assert_eq!(
            delta.review_required_categories,
            vec!["private_communications"]
        );
        assert!(delta.requires_review);
    }

    #[test]
    fn defaults_empty_source_to_manual_agent_delta() {
        let delta = extract_agent_delta("Completed the local test run.", Some("")).unwrap();

        assert_eq!(delta.source, "agent-delta:manual");
        assert!(delta.source.starts_with(AGENT_DELTA_SOURCE_PREFIX));
    }

    #[test]
    fn normalizes_source_labels_to_bounded_slugs() {
        let delta = extract_agent_delta(
            "Updated Acme Capital follow-up status.",
            Some(" Follow Up / CRM "),
        )
        .unwrap();
        let prefixed = extract_agent_delta(
            "Updated Acme Capital follow-up status.",
            Some("agent-delta:Billing Review!"),
        )
        .unwrap();
        let long = extract_agent_delta(
            "Updated Acme Capital follow-up status.",
            Some("abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz"),
        )
        .unwrap();

        assert_eq!(delta.source, "agent-delta:follow-up-crm");
        assert_eq!(prefixed.source, "agent-delta:billing-review");
        assert!(long.source.starts_with(AGENT_DELTA_SOURCE_PREFIX));
        assert!(
            long.source
                .strip_prefix(AGENT_DELTA_SOURCE_PREFIX)
                .unwrap()
                .chars()
                .count()
                <= 80
        );
    }

    #[test]
    fn rejects_sensitive_delta_content_before_memory_commit() {
        let blocked = extract_agent_delta("The .env password is secret.", None).unwrap_err();

        assert!(blocked.to_string().contains("blocked capture content"));
    }

    #[test]
    fn flags_sensitive_delta_categories_for_explicit_review() {
        let delta = extract_agent_delta(
            "Booked payment for medical invoice. Contract signature is pending.",
            Some("test"),
        )
        .unwrap();

        assert_eq!(
            delta.review_required_categories,
            vec!["finance", "health", "legal_identity"]
        );
        assert!(delta.requires_review());
    }

    #[test]
    fn derives_stable_negative_cleaned_event_id_for_dedupe() {
        let first = extract_agent_delta("Updated Acme Capital follow-up status.", Some("test"))
            .unwrap()
            .to_cleaned_event()
            .unwrap();
        let second = extract_agent_delta("Updated Acme Capital follow-up status.", Some("test"))
            .unwrap()
            .to_cleaned_event()
            .unwrap();

        assert!(first.id < 0);
        assert_eq!(first.id, second.id);
        assert_eq!(first.content_hash, second.content_hash);
    }

    #[test]
    fn parses_validated_agent_delta_json_candidate() {
        let delta = extract_agent_delta(
            "Sent follow-up message to Acme Capital. Confirmation reference: MSG-42",
            Some("follow-up"),
        )
        .unwrap();
        let json = delta.to_json().unwrap();

        let parsed = agent_delta_from_json(&json).unwrap();

        assert_eq!(parsed, delta);
        assert_eq!(
            parsed.to_cleaned_event().unwrap().content_hash,
            delta.to_cleaned_event().unwrap().content_hash
        );
    }

    #[test]
    fn schema_json_exposes_review_candidate_contract() {
        let json = agent_delta_schema_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["schema_version"], AGENT_DELTA_SCHEMA_VERSION);
        assert_eq!(parsed["source_prefix"], AGENT_DELTA_SOURCE_PREFIX);
        assert!(parsed["allowed_outcome_states"]
            .as_array()
            .unwrap()
            .iter()
            .any(|state| state == "SENT"));
        assert!(parsed["allowed_review_required_categories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|category| category == "finance"));
        assert_eq!(parsed["rules"]["unknown_fields"], "rejected");

        let template_json =
            serde_json::to_string(&parsed["candidate_template"]).expect("template serializes");
        let template = agent_delta_from_json(&template_json).unwrap();

        assert_eq!(template.schema_version, AGENT_DELTA_SCHEMA_VERSION);
        assert_eq!(template.source, "agent-delta:manual");
        assert_eq!(template.outcome_state, "OBSERVED");
        assert!(!template.requires_review());
    }

    #[test]
    fn validation_json_reports_safe_bounded_status() {
        let delta = extract_agent_delta(
            "Paid invoice for Acme Capital. Receipt reference: INV-42",
            Some("billing"),
        )
        .unwrap();

        let json = agent_delta_validation_json(&delta).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["valid"], true);
        assert_eq!(parsed["schema_version"], AGENT_DELTA_SCHEMA_VERSION);
        assert_eq!(parsed["source"], "agent-delta:billing");
        assert_eq!(parsed["outcome_state"], "PAID");
        assert_eq!(parsed["requires_review"], true);
        assert_eq!(parsed["commit_requires_allow_sensitive"], true);
        assert_eq!(
            parsed["review_required_categories"],
            serde_json::json!(["finance"])
        );
        assert_eq!(parsed["entities_count"], 1);
        assert_eq!(parsed["attributes_count"], 1);
        assert!(parsed.get("summary").is_none());
        assert!(parsed.get("entities").is_none());
        assert!(parsed.get("attributes").is_none());
        assert!(!json.contains("Acme Capital"));
        assert!(!json.contains("INV-42"));
    }

    #[test]
    fn rejects_unknown_agent_delta_json_fields() {
        let rejected = agent_delta_from_json(
            r#"{
                "schema_version": 1,
                "source": "agent-delta:test",
                "outcome_state": "UPDATED",
                "summary": "Updated Acme Capital follow-up status.",
                "entities": ["Acme Capital"],
                "attributes": [],
                "requires_review": false,
                "review_required_categories": [],
                "raw_session_log": "hidden"
            }"#,
        )
        .unwrap_err();

        assert!(rejected.to_string().contains("unknown field"));
    }

    #[test]
    fn rejects_unsafe_agent_delta_json_candidate() {
        let rejected = agent_delta_from_json(
            r#"{
                "schema_version": 1,
                "source": "agent-delta:test",
                "outcome_state": "UPDATED",
                "summary": "The .env password is secret.",
                "entities": [],
                "attributes": [],
                "requires_review": false,
                "review_required_categories": []
            }"#,
        )
        .unwrap_err();

        assert!(rejected.to_string().contains("blocked capture content"));
    }

    #[test]
    fn validates_delta_schema_before_json_or_commit() {
        let mut delta =
            extract_agent_delta("Updated Acme Capital follow-up status.", Some("test")).unwrap();

        delta.schema_version = 2;
        assert!(delta
            .to_json()
            .unwrap_err()
            .to_string()
            .contains("schema_version"));

        delta.schema_version = AGENT_DELTA_SCHEMA_VERSION;
        delta.outcome_state = "MAYBE".to_string();
        assert!(delta
            .to_cleaned_event()
            .unwrap_err()
            .to_string()
            .contains("outcome_state"));

        delta.outcome_state = "UPDATED".to_string();
        delta.attributes.push(DeltaAttribute {
            key: "Bad Key".to_string(),
            value: "value".to_string(),
        });
        assert!(delta
            .validate()
            .unwrap_err()
            .to_string()
            .contains("attribute keys"));

        delta.attributes.clear();
        delta.source = "agent-delta:Bad Label".to_string();
        assert!(delta
            .validate()
            .unwrap_err()
            .to_string()
            .contains("source label"));
    }

    #[test]
    fn schema_requires_review_flag_to_match_categories() {
        let mut delta = extract_agent_delta(
            "Booked payment for medical invoice. Contract signature is pending.",
            Some("test"),
        )
        .unwrap();

        assert!(delta.requires_review());
        delta.requires_review = false;

        assert!(delta
            .validate()
            .unwrap_err()
            .to_string()
            .contains("requires_review"));
    }
}
