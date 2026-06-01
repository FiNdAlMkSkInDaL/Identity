use crate::embedding::{
    cosine_similarity, embed_text, from_le_bytes, to_le_bytes, EMBEDDING_DIM, EMBEDDING_MODEL_ID,
};
use crate::transit::CleanedEvent;
use crate::vector_store::{VectorStore, VectorStoreError};
use crate::workspace::IdentityPaths;
use rusqlite::{params, Connection};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct MemoryNode {
    pub id: i64,
    pub cleaned_event_id: i64,
    pub source: String,
    pub domain_context: String,
    pub entity_type: String,
    pub summary: String,
    pub structured_attributes: String,
    pub raw_text: String,
    pub content_hash: String,
    pub created_at_ms: i64,
}

#[derive(Debug)]
pub struct MemorySearchResult {
    pub node: MemoryNode,
    pub score: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryStats {
    pub node_count: i64,
    pub vectorized_count: i64,
    pub invalid_vector_count: i64,
    pub embedding_model_id: String,
    pub embedding_dim: usize,
    pub vector_store_backend: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRepairSummary {
    pub repaired_vectors: usize,
}

#[derive(Debug)]
pub enum IdentityError {
    ClockBeforeUnixEpoch,
    Sqlite(rusqlite::Error),
    VectorStore(VectorStoreError),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClockBeforeUnixEpoch => write!(f, "system clock is before the Unix epoch"),
            Self::Sqlite(error) => write!(f, "{error}"),
            Self::VectorStore(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for IdentityError {}

impl From<rusqlite::Error> for IdentityError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

impl From<VectorStoreError> for IdentityError {
    fn from(value: VectorStoreError) -> Self {
        Self::VectorStore(value)
    }
}

pub struct IdentityStore {
    backend: SqliteMemoryBackend,
    embedding: EmbeddingEngine,
    vector_store: VectorStore,
}

impl IdentityStore {
    pub fn open(paths: &IdentityPaths) -> Result<Self, IdentityError> {
        let embedding = EmbeddingEngine::new();
        let vector_store = VectorStore::open(paths, embedding.model_id(), embedding.dimension())?;
        let backend = SqliteMemoryBackend::open(paths, &embedding)?;
        backend.sync_vector_store(&vector_store, &embedding)?;
        Ok(Self {
            backend,
            embedding,
            vector_store,
        })
    }

    pub fn insert_memory_from_cleaned(&self, cleaned: &CleanedEvent) -> Result<i64, IdentityError> {
        let created_at_ms = now_ms()?;
        let summary = summarize_capture(&cleaned.source, &cleaned.cleaned_content);
        let structured_attributes = capture_attributes(&cleaned.source, &cleaned.cleaned_content);
        let vector_embedding = self.embedding.encode_bytes(&cleaned.cleaned_content);
        let classification = classify_capture(&cleaned.source);

        let record = MemoryRecord {
            cleaned_event_id: cleaned.id,
            source: cleaned.source.clone(),
            domain_context: classification.domain_context.to_string(),
            entity_type: classification.entity_type.to_string(),
            summary,
            structured_attributes,
            raw_text: cleaned.cleaned_content.clone(),
            content_hash: cleaned.content_hash.clone(),
            vector_embedding,
            created_at_ms,
        };

        let id = self.backend.insert_memory_record(&record)?;
        self.vector_store.upsert(id, &record.vector_embedding)?;
        Ok(id)
    }

    pub fn list_recent(&self, limit: u32) -> Result<Vec<MemoryNode>, IdentityError> {
        self.backend.list_recent(limit)
    }

    pub fn search(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<MemorySearchResult>, IdentityError> {
        let tokens = query_tokens(query);

        if tokens.is_empty() {
            return Ok(Vec::new());
        }

        let query_embedding = self.embedding.embed(query);
        let candidates = self
            .backend
            .list_recent_with_embeddings(500, &self.embedding, &self.vector_store)?;
        let mut results = candidates
            .into_iter()
            .filter_map(|candidate| {
                let token_score = score_node(&candidate.node, &tokens);
                let vector_score = self
                    .embedding
                    .similarity(&query_embedding, &candidate.embedding);

                if token_score == 0 && vector_score < 0.15 {
                    None
                } else {
                    Some(MemorySearchResult {
                        node: candidate.node,
                        score: scaled_score(token_score, vector_score),
                    })
                }
            })
            .collect::<Vec<_>>();

        results.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| right.node.created_at_ms.cmp(&left.node.created_at_ms))
                .then_with(|| right.node.id.cmp(&left.node.id))
        });
        results.truncate(limit as usize);

        Ok(results)
    }

    pub fn stats(&self) -> Result<MemoryStats, IdentityError> {
        self.backend.stats(&self.embedding, &self.vector_store)
    }

    pub fn repair_vectors(&self, limit: u32) -> Result<MemoryRepairSummary, IdentityError> {
        self.backend
            .repair_vectors(limit, &self.embedding, &self.vector_store)
    }
}

struct SqliteMemoryBackend {
    conn: Connection,
}

impl SqliteMemoryBackend {
    fn open(paths: &IdentityPaths, embedding: &EmbeddingEngine) -> Result<Self, IdentityError> {
        let conn = Connection::open(&paths.identity_db)?;
        let backend = Self { conn };
        backend.initialize_schema()?;
        backend.migrate_schema(embedding)?;
        Ok(backend)
    }

    fn insert_memory_record(&self, record: &MemoryRecord) -> Result<i64, IdentityError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO memory_nodes
                (cleaned_event_id, source, domain_context, entity_type, summary, structured_attributes, raw_text, content_hash, vector_embedding, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                record.cleaned_event_id,
                &record.source,
                &record.domain_context,
                &record.entity_type,
                &record.summary,
                &record.structured_attributes,
                &record.raw_text,
                &record.content_hash,
                &record.vector_embedding,
                record.created_at_ms
            ],
        )?;

        if self.conn.changes() == 0 {
            self.conn
                .query_row(
                    "SELECT id FROM memory_nodes WHERE cleaned_event_id = ?1",
                    [record.cleaned_event_id],
                    |row| row.get(0),
                )
                .map_err(IdentityError::from)
        } else {
            Ok(self.conn.last_insert_rowid())
        }
    }

    fn sync_vector_store(
        &self,
        vector_store: &VectorStore,
        embedding: &EmbeddingEngine,
    ) -> Result<(), IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT id, vector_embedding
             FROM memory_nodes
             WHERE length(vector_embedding) = ?1
             ORDER BY created_at_ms ASC, id ASC",
        )?;
        let rows = statement.query_map([embedding.blob_len() as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let vectors = rows.collect::<Result<Vec<_>, _>>()?;

        for (id, bytes) in vectors {
            if vector_store.read(id)?.is_none() {
                vector_store.upsert(id, &bytes)?;
            }
        }

        Ok(())
    }

    fn list_recent(&self, limit: u32) -> Result<Vec<MemoryNode>, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT id, cleaned_event_id, source, domain_context, entity_type, summary, structured_attributes, raw_text, content_hash, created_at_ms
             FROM memory_nodes
             ORDER BY created_at_ms DESC, id DESC
             LIMIT ?1",
        )?;

        let rows = statement.query_map([limit as i64], map_memory_node)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IdentityError::from)
    }

    fn list_recent_with_embeddings(
        &self,
        limit: u32,
        embedding: &EmbeddingEngine,
        vector_store: &VectorStore,
    ) -> Result<Vec<MemoryCandidate>, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT id, cleaned_event_id, source, domain_context, entity_type, summary, structured_attributes, raw_text, content_hash, created_at_ms
             FROM memory_nodes
             ORDER BY created_at_ms DESC, id DESC
             LIMIT ?1",
        )?;

        let mut rows = statement.query([limit as i64])?;
        let mut candidates = Vec::new();

        while let Some(row) = rows.next()? {
            let node = map_memory_node(row)?;
            let stored_blob = vector_store.read(node.id)?;
            let embedding = embedding.resolve_bytes(stored_blob.as_deref(), None, &node.raw_text);

            candidates.push(MemoryCandidate { node, embedding });
        }

        Ok(candidates)
    }

    fn stats(
        &self,
        embedding: &EmbeddingEngine,
        vector_store: &VectorStore,
    ) -> Result<MemoryStats, IdentityError> {
        let node_count = self
            .conn
            .query_row("SELECT COUNT(*) FROM memory_nodes", [], |row| row.get(0))?;
        let vectorized_count = self.conn.query_row(
            "SELECT COUNT(*) FROM memory_nodes WHERE length(vector_embedding) = ?1",
            [embedding.blob_len() as i64],
            |row| row.get(0),
        )?;
        let invalid_vector_count = node_count - vectorized_count;
        let embedding_model_id = self.meta_value("embedding_model_id")?.unwrap_or_default();
        let embedding_dim = self
            .meta_value("embedding_dim")?
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(embedding.dimension());

        Ok(MemoryStats {
            node_count,
            vectorized_count,
            invalid_vector_count,
            embedding_model_id,
            embedding_dim,
            vector_store_backend: vector_store.backend_name().to_string(),
        })
    }

    fn repair_vectors(
        &self,
        limit: u32,
        embedding: &EmbeddingEngine,
        vector_store: &VectorStore,
    ) -> Result<MemoryRepairSummary, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT id, raw_text
             FROM memory_nodes
             WHERE vector_embedding IS NULL
                OR length(vector_embedding) != ?1
             ORDER BY created_at_ms ASC, id ASC
             LIMIT ?2",
        )?;
        let rows = statement.query_map([embedding.blob_len() as i64, limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let repairs = rows.collect::<Result<Vec<_>, _>>()?;

        for (id, raw_text) in &repairs {
            let vector_embedding = embedding.encode_bytes(raw_text);
            self.conn.execute(
                "UPDATE memory_nodes
                 SET vector_embedding = ?1
                 WHERE id = ?2",
                params![vector_embedding, id],
            )?;
            vector_store.upsert(*id, &embedding.encode_bytes(raw_text))?;
        }

        Ok(MemoryRepairSummary {
            repaired_vectors: repairs.len(),
        })
    }

    fn initialize_schema(&self) -> Result<(), IdentityError> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;

             CREATE TABLE IF NOT EXISTS memory_nodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                cleaned_event_id INTEGER NOT NULL UNIQUE,
                source TEXT NOT NULL,
                domain_context TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                summary TEXT NOT NULL,
                structured_attributes TEXT NOT NULL DEFAULT '{}',
                raw_text TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                vector_embedding BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL
             );

             CREATE INDEX IF NOT EXISTS idx_memory_nodes_content_hash
                ON memory_nodes(content_hash);

             CREATE INDEX IF NOT EXISTS idx_memory_nodes_created_at
                ON memory_nodes(created_at_ms);

             CREATE TABLE IF NOT EXISTS store_metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL
             );",
        )?;

        Ok(())
    }

    fn migrate_schema(&self, embedding: &EmbeddingEngine) -> Result<(), IdentityError> {
        if !self.has_column("memory_nodes", "vector_embedding")? {
            self.conn.execute(
                "ALTER TABLE memory_nodes ADD COLUMN vector_embedding BLOB",
                [],
            )?;
        }

        if !self.has_column("memory_nodes", "structured_attributes")? {
            self.conn.execute(
                "ALTER TABLE memory_nodes ADD COLUMN structured_attributes TEXT NOT NULL DEFAULT '{}'",
                [],
            )?;
        }

        self.set_meta_value("embedding_model_id", embedding.model_id())?;
        self.set_meta_value("embedding_dim", &embedding.dimension().to_string())?;

        Ok(())
    }

    fn has_column(&self, table: &str, column: &str) -> Result<bool, IdentityError> {
        let mut statement = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let rows = statement.query_map([], |row| row.get::<_, String>(1))?;

        for row in rows {
            if row? == column {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn meta_value(&self, key: &str) -> Result<Option<String>, IdentityError> {
        let mut statement = self
            .conn
            .prepare("SELECT value FROM store_metadata WHERE key = ?1")?;
        let mut rows = statement.query([key])?;

        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    fn set_meta_value(&self, key: &str, value: &str) -> Result<(), IdentityError> {
        let updated_at_ms = now_ms()?;

        self.conn.execute(
            "INSERT INTO store_metadata (key, value, updated_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_at_ms = excluded.updated_at_ms",
            params![key, value, updated_at_ms],
        )?;

        Ok(())
    }
}

struct MemoryCandidate {
    node: MemoryNode,
    embedding: [f32; EMBEDDING_DIM],
}

#[derive(Clone, Copy)]
struct EmbeddingEngine;

impl EmbeddingEngine {
    fn new() -> Self {
        Self
    }

    fn model_id(&self) -> &'static str {
        EMBEDDING_MODEL_ID
    }

    fn dimension(&self) -> usize {
        EMBEDDING_DIM
    }

    fn blob_len(&self) -> usize {
        self.dimension() * std::mem::size_of::<f32>()
    }

    fn embed(&self, text: &str) -> [f32; EMBEDDING_DIM] {
        embed_text(text)
    }

    fn encode_bytes(&self, text: &str) -> Vec<u8> {
        to_le_bytes(&self.embed(text))
    }

    fn resolve_bytes(
        &self,
        primary: Option<&[u8]>,
        secondary: Option<&[u8]>,
        text: &str,
    ) -> [f32; EMBEDDING_DIM] {
        primary
            .and_then(from_le_bytes)
            .or_else(|| secondary.and_then(from_le_bytes))
            .unwrap_or_else(|| self.embed(text))
    }

    fn similarity(&self, left: &[f32; EMBEDDING_DIM], right: &[f32; EMBEDDING_DIM]) -> f32 {
        cosine_similarity(left, right)
    }
}

struct MemoryRecord {
    cleaned_event_id: i64,
    source: String,
    domain_context: String,
    entity_type: String,
    summary: String,
    structured_attributes: String,
    raw_text: String,
    content_hash: String,
    vector_embedding: Vec<u8>,
    created_at_ms: i64,
}

fn map_memory_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryNode> {
    Ok(MemoryNode {
        id: row.get(0)?,
        cleaned_event_id: row.get(1)?,
        source: row.get(2)?,
        domain_context: row.get(3)?,
        entity_type: row.get(4)?,
        summary: row.get(5)?,
        structured_attributes: row.get(6)?,
        raw_text: row.get(7)?,
        content_hash: row.get(8)?,
        created_at_ms: row.get(9)?,
    })
}

struct CaptureClassification {
    domain_context: &'static str,
    entity_type: &'static str,
}

fn classify_capture(source: &str) -> CaptureClassification {
    if source.starts_with("windows-ui:") {
        CaptureClassification {
            domain_context: "local.activity.window",
            entity_type: "USER_INTERFACE",
        }
    } else if source.starts_with("filesystem:") {
        CaptureClassification {
            domain_context: "local.filesystem",
            entity_type: "DOCUMENT",
        }
    } else if source.starts_with("local-proxy:") {
        CaptureClassification {
            domain_context: "local.web.capture",
            entity_type: "WEB_CONTENT",
        }
    } else {
        CaptureClassification {
            domain_context: "local.capture",
            entity_type: "DOCUMENT",
        }
    }
}

fn capture_attributes(source: &str, content: &str) -> String {
    if source.starts_with("windows-ui:") {
        windows_activity_attributes(content)
    } else {
        "{}".to_string()
    }
}

fn windows_activity_attributes(content: &str) -> String {
    let mut fields = Vec::new();

    if let Some(application) = labelled_value(content, "Active application:") {
        fields.push(json_field("application", &application));
    }
    if let Some(title) = labelled_value(content, "Active window title:") {
        fields.push(json_field("window_title", &title));
    }
    if let Some(focus) = labelled_value(content, "Focused control text:") {
        fields.push(json_field("focused_text", &focus));
    }
    if let Some(visible) = first_visible_line(content) {
        fields.push(json_field("first_visible_text", &visible));
    }

    if fields.is_empty() {
        "{}".to_string()
    } else {
        format!("{{{}}}", fields.join(","))
    }
}

fn json_field(key: &str, value: &str) -> String {
    format!("\"{}\":\"{}\"", key, json_escape(value))
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());

    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(character),
        }
    }

    escaped
}

fn summarize_capture(source: &str, content: &str) -> String {
    if source.starts_with("windows-ui:") {
        summarize_windows_activity(content)
    } else {
        summarize(content)
    }
}

fn summarize_windows_activity(content: &str) -> String {
    let application = labelled_value(content, "Active application:");
    let title = labelled_value(content, "Active window title:");
    let focus = labelled_value(content, "Focused control text:");
    let first_visible = first_visible_line(content);

    let mut parts = Vec::new();

    if let Some(application) = application {
        parts.push(format!("UI activity in {application}"));
    }
    if let Some(title) = title {
        parts.push(format!("window {title}"));
    }
    if let Some(focus) = focus {
        parts.push(format!("focus {focus}"));
    }
    if let Some(visible) = first_visible {
        parts.push(format!("visible {visible}"));
    }

    if parts.is_empty() {
        summarize(content)
    } else {
        summarize(&parts.join("; "))
    }
}

fn labelled_value(content: &str, label: &str) -> Option<String> {
    let start = content.find(label)? + label.len();
    let remainder = content[start..].trim_start();
    let next_label_offset = [
        "Active application:",
        "Active window title:",
        "Focused control text:",
        "Visible window text:",
        "\n- ",
    ]
    .into_iter()
    .filter(|candidate| *candidate != label)
    .filter_map(|candidate| remainder.find(candidate))
    .min()
    .unwrap_or(remainder.len());

    let value = remainder[..next_label_offset].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn first_visible_line(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("- "))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
}

fn score_node(node: &MemoryNode, tokens: &[String]) -> u32 {
    let haystack = format!(
        "{} {} {} {} {} {}",
        node.summary,
        node.structured_attributes,
        node.raw_text,
        node.source,
        node.domain_context,
        node.entity_type
    )
    .to_ascii_lowercase();

    tokens
        .iter()
        .map(|token| haystack.matches(token.as_str()).count() as u32)
        .sum()
}

fn scaled_score(token_score: u32, vector_score: f32) -> u32 {
    token_score.saturating_mul(100) + (vector_score * 1000.0) as u32
}

fn query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();

    for raw in query.split(|character: char| !character.is_ascii_alphanumeric()) {
        let token = raw.trim().to_ascii_lowercase();

        if token.len() >= 3 && !is_stopword(&token) && !tokens.contains(&token) {
            tokens.push(token);
        }
    }

    tokens
}

#[inline]
fn is_stopword(token: &str) -> bool {
    matches!(
        token,
        "and"
            | "are"
            | "but"
            | "for"
            | "not"
            | "the"
            | "this"
            | "that"
            | "with"
            | "from"
            | "into"
            | "real"
            | "user"
    )
}

#[inline]
fn summarize(content: &str) -> String {
    const MAX_SUMMARY_CHARS: usize = 240;
    let mut summary = String::new();

    for character in content.chars().take(MAX_SUMMARY_CHARS) {
        summary.push(character);
    }

    summary
}

fn now_ms() -> Result<i64, IdentityError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| IdentityError::ClockBeforeUnixEpoch)?;

    Ok(duration.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::{
        capture_attributes, classify_capture, labelled_value, query_tokens, summarize,
        summarize_capture, summarize_windows_activity, windows_activity_attributes, IdentityStore,
    };
    use crate::transit::CleanedEvent;
    use crate::workspace::IdentityPaths;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn inserts_memory_node_from_cleaned_event_idempotently() {
        let root = std::env::temp_dir().join(format!(
            "identity-identity-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 42,
            captured_event_id: 7,
            source: "test".to_string(),
            cleaned_content: "Identity stores local memory.".to_string(),
            content_hash: "hash".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        let first = store.insert_memory_from_cleaned(&cleaned).unwrap();
        let second = store.insert_memory_from_cleaned(&cleaned).unwrap();
        let memories = store.list_recent(10).unwrap();

        assert_eq!(first, second);
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].summary, "Identity stores local memory.");

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn summary_is_bounded() {
        let long = "a".repeat(300);
        assert_eq!(summarize(&long).len(), 240);
    }

    #[test]
    fn windows_activity_summary_prioritizes_structured_context() {
        let summary = summarize_windows_activity(
            "Active application: Code.exe\nActive window title: Identity - README.md\nFocused control text: Search files\nVisible window text:\n- Identity local-first notes\n- Another line",
        );

        assert_eq!(
            summary,
            "UI activity in Code.exe; window Identity - README.md; focus Search files; visible Identity local-first notes"
        );
    }

    #[test]
    fn extracts_labelled_values_from_activity_payload() {
        let content = "Active application: Code.exe\nFocused control text: Search files\nVisible window text:\n- Notes";

        assert_eq!(
            labelled_value(content, "Active application:"),
            Some("Code.exe".to_string())
        );
        assert_eq!(
            labelled_value(content, "Focused control text:"),
            Some("Search files".to_string())
        );
    }

    #[test]
    fn windows_activity_attributes_extract_structured_fields() {
        let attributes = windows_activity_attributes(
            "Active application: Code.exe\nActive window title: Identity\nFocused control text: Search files\nVisible window text:\n- Identity note",
        );

        assert_eq!(
            attributes,
            "{\"application\":\"Code.exe\",\"window_title\":\"Identity\",\"focused_text\":\"Search files\",\"first_visible_text\":\"Identity note\"}"
        );
    }

    #[test]
    fn tokenizes_queries_without_duplicates() {
        assert_eq!(
            query_tokens("Local, local-first memory!"),
            vec![
                "local".to_string(),
                "first".to_string(),
                "memory".to_string()
            ]
        );
        assert_eq!(
            query_tokens("not-a-real-memory-token"),
            vec!["memory".to_string(), "token".to_string()]
        );
    }

    #[test]
    fn classifies_windows_activity_captures_as_ui_context() {
        let classification = classify_capture("windows-ui:foreground-window");
        assert_eq!(classification.domain_context, "local.activity.window");
        assert_eq!(classification.entity_type, "USER_INTERFACE");
    }

    #[test]
    fn searches_memory_nodes_by_token_overlap() {
        let root = std::env::temp_dir().join(format!(
            "identity-search-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let first = CleanedEvent {
            id: 1,
            captured_event_id: 1,
            source: "test".to_string(),
            cleaned_content: "User prefers local-first private memory.".to_string(),
            content_hash: "hash1".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };
        let second = CleanedEvent {
            id: 2,
            captured_event_id: 2,
            source: "test".to_string(),
            cleaned_content: "Unrelated weather note.".to_string(),
            content_hash: "hash2".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&first).unwrap();
        store.insert_memory_from_cleaned(&second).unwrap();

        let results = store.search("private memory", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].node.summary.contains("private memory"));
        assert!(results[0].score > 0);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_source_specific_memory_classification() {
        let root = std::env::temp_dir().join(format!(
            "identity-classification-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 21,
            captured_event_id: 21,
            source: "windows-ui:foreground-window".to_string(),
            cleaned_content: "Active application: Code.exe Active window title: Identity".to_string(),
            content_hash: "hash21".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&cleaned).unwrap();
        let memories = store.list_recent(10).unwrap();

        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].domain_context, "local.activity.window");
        assert_eq!(memories[0].entity_type, "USER_INTERFACE");
        assert_eq!(memories[0].summary, "UI activity in Code.exe; window Identity");
        assert_eq!(
            memories[0].structured_attributes,
            "{\"application\":\"Code.exe\",\"window_title\":\"Identity\"}"
        );

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn summarize_capture_uses_source_specific_windows_path() {
        let summary = summarize_capture(
            "windows-ui:foreground-window",
            "Active application: Code.exe\nActive window title: Identity\nFocused control text: Search files",
        );

        assert_eq!(summary, "UI activity in Code.exe; window Identity; focus Search files");
    }

    #[test]
    fn capture_attributes_defaults_to_empty_object_for_other_sources() {
        assert_eq!(capture_attributes("filesystem:note", "plain text"), "{}");
    }

    #[test]
    fn stores_fixed_width_vector_embedding_on_promotion() {
        let root = std::env::temp_dir().join(format!(
            "identity-vector-store-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 3,
            captured_event_id: 3,
            source: "test".to_string(),
            cleaned_content: "Local vectors are computed on device.".to_string(),
            content_hash: "hash3".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&cleaned).unwrap();
        let blob_len: i64 = store
            .backend
            .conn
            .query_row(
                "SELECT length(vector_embedding) FROM memory_nodes WHERE cleaned_event_id = ?1",
                [cleaned.id],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(blob_len, (crate::embedding::EMBEDDING_DIM * 4) as i64);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn promotion_syncs_vector_blob_into_vector_store_root() {
        let root = std::env::temp_dir().join(format!(
            "identity-vector-store-sync-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 31,
            captured_event_id: 31,
            source: "test".to_string(),
            cleaned_content: "Vectors should also land in the reserved vector-store root.".to_string(),
            content_hash: "hash31".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        let node_id = store.insert_memory_from_cleaned(&cleaned).unwrap();
        let stored = store.vector_store.read(node_id).unwrap().unwrap();

        assert_eq!(stored.len(), crate::embedding::EMBEDDING_DIM * 4);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reopening_identity_store_backfills_missing_vector_store_files() {
        let root = std::env::temp_dir().join(format!(
            "identity-vector-store-backfill-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 32,
            captured_event_id: 32,
            source: "test".to_string(),
            cleaned_content: "Existing SQLite vectors should backfill the mirror on reopen.".to_string(),
            content_hash: "hash32".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        let node_id = store.insert_memory_from_cleaned(&cleaned).unwrap();
        drop(store);

        for entry in fs::read_dir(&paths.vector_store_dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name().to_string_lossy().starts_with("node-") {
                fs::remove_file(entry.path()).unwrap();
            }
        }

        let reopened = IdentityStore::open(&paths).unwrap();
        let restored = reopened.vector_store.read(node_id).unwrap();

        assert!(restored.is_some());
        assert_eq!(restored.unwrap().len(), crate::embedding::EMBEDDING_DIM * 4);

        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_vector_health_and_embedding_metadata() {
        let root = std::env::temp_dir().join(format!(
            "identity-memory-stats-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 4,
            captured_event_id: 4,
            source: "test".to_string(),
            cleaned_content: "Vector metadata should be inspectable.".to_string(),
            content_hash: "hash4".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&cleaned).unwrap();
        let stats = store.stats().unwrap();

        assert_eq!(stats.node_count, 1);
        assert_eq!(stats.vectorized_count, 1);
        assert_eq!(stats.invalid_vector_count, 0);
        assert_eq!(
            stats.embedding_model_id,
            crate::embedding::EMBEDDING_MODEL_ID
        );
        assert_eq!(stats.embedding_dim, crate::embedding::EMBEDDING_DIM);
        assert_eq!(stats.vector_store_backend, "lancedb+filesystem+sqlite");

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repairs_missing_or_corrupt_vector_blobs() {
        let root = std::env::temp_dir().join(format!(
            "identity-memory-repair-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 5,
            captured_event_id: 5,
            source: "test".to_string(),
            cleaned_content: "Corrupt vectors can be rebuilt locally.".to_string(),
            content_hash: "hash5".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&cleaned).unwrap();
        store
            .backend
            .conn
            .execute(
                "UPDATE memory_nodes SET vector_embedding = ?1 WHERE cleaned_event_id = ?2",
                (&vec![1_u8, 2, 3], cleaned.id),
            )
            .unwrap();

        let before = store.stats().unwrap();
        assert_eq!(before.invalid_vector_count, 1);

        let repair = store.repair_vectors(10).unwrap();
        assert_eq!(repair.repaired_vectors, 1);

        let after = store.stats().unwrap();
        assert_eq!(after.invalid_vector_count, 0);
        assert_eq!(after.vectorized_count, 1);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
