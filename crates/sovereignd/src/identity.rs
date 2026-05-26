use crate::embedding::{cosine_similarity, embed_text, from_le_bytes, to_le_bytes, EMBEDDING_DIM};
use crate::transit::CleanedEvent;
use crate::workspace::SovereignPaths;
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
    pub raw_text: String,
    pub content_hash: String,
    pub created_at_ms: i64,
}

#[derive(Debug)]
pub struct MemorySearchResult {
    pub node: MemoryNode,
    pub score: u32,
}

#[derive(Debug)]
pub enum IdentityError {
    ClockBeforeUnixEpoch,
    Sqlite(rusqlite::Error),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClockBeforeUnixEpoch => write!(f, "system clock is before the Unix epoch"),
            Self::Sqlite(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for IdentityError {}

impl From<rusqlite::Error> for IdentityError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

pub struct IdentityStore {
    conn: Connection,
}

impl IdentityStore {
    pub fn open(paths: &SovereignPaths) -> Result<Self, IdentityError> {
        let conn = Connection::open(&paths.identity_db)?;
        let store = Self { conn };
        store.initialize_schema()?;
        store.migrate_schema()?;
        Ok(store)
    }

    pub fn insert_memory_from_cleaned(&self, cleaned: &CleanedEvent) -> Result<i64, IdentityError> {
        let created_at_ms = now_ms()?;
        let summary = summarize(&cleaned.cleaned_content);
        let vector_embedding = to_le_bytes(&embed_text(&cleaned.cleaned_content));

        self.conn.execute(
            "INSERT OR IGNORE INTO memory_nodes
                (cleaned_event_id, source, domain_context, entity_type, summary, raw_text, content_hash, vector_embedding, created_at_ms)
             VALUES (?1, ?2, 'local.capture', 'DOCUMENT', ?3, ?4, ?5, ?6, ?7)",
            params![
                cleaned.id,
                cleaned.source,
                summary,
                cleaned.cleaned_content,
                cleaned.content_hash,
                vector_embedding,
                created_at_ms
            ],
        )?;

        if self.conn.changes() == 0 {
            self.conn
                .query_row(
                    "SELECT id FROM memory_nodes WHERE cleaned_event_id = ?1",
                    [cleaned.id],
                    |row| row.get(0),
                )
                .map_err(IdentityError::from)
        } else {
            Ok(self.conn.last_insert_rowid())
        }
    }

    pub fn list_recent(&self, limit: u32) -> Result<Vec<MemoryNode>, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT id, cleaned_event_id, source, domain_context, entity_type, summary, raw_text, content_hash, created_at_ms
             FROM memory_nodes
             ORDER BY created_at_ms DESC, id DESC
             LIMIT ?1",
        )?;

        let rows = statement.query_map([limit as i64], |row| {
            Ok(MemoryNode {
                id: row.get(0)?,
                cleaned_event_id: row.get(1)?,
                source: row.get(2)?,
                domain_context: row.get(3)?,
                entity_type: row.get(4)?,
                summary: row.get(5)?,
                raw_text: row.get(6)?,
                content_hash: row.get(7)?,
                created_at_ms: row.get(8)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IdentityError::from)
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

        let query_embedding = embed_text(query);
        let candidates = self.list_recent_with_embeddings(500)?;
        let mut results = candidates
            .into_iter()
            .filter_map(|candidate| {
                let token_score = score_node(&candidate.node, &tokens);
                let vector_score = cosine_similarity(&query_embedding, &candidate.embedding);

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
                raw_text TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                vector_embedding BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL
             );

             CREATE INDEX IF NOT EXISTS idx_memory_nodes_content_hash
                ON memory_nodes(content_hash);

             CREATE INDEX IF NOT EXISTS idx_memory_nodes_created_at
                ON memory_nodes(created_at_ms);",
        )?;

        Ok(())
    }

    fn migrate_schema(&self) -> Result<(), IdentityError> {
        if !self.has_column("memory_nodes", "vector_embedding")? {
            self.conn.execute(
                "ALTER TABLE memory_nodes ADD COLUMN vector_embedding BLOB",
                [],
            )?;
        }

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

    fn list_recent_with_embeddings(
        &self,
        limit: u32,
    ) -> Result<Vec<MemoryCandidate>, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT id, cleaned_event_id, source, domain_context, entity_type, summary, raw_text, content_hash, created_at_ms, vector_embedding
             FROM memory_nodes
             ORDER BY created_at_ms DESC, id DESC
             LIMIT ?1",
        )?;

        let rows = statement.query_map([limit as i64], |row| {
            let node = MemoryNode {
                id: row.get(0)?,
                cleaned_event_id: row.get(1)?,
                source: row.get(2)?,
                domain_context: row.get(3)?,
                entity_type: row.get(4)?,
                summary: row.get(5)?,
                raw_text: row.get(6)?,
                content_hash: row.get(7)?,
                created_at_ms: row.get(8)?,
            };
            let embedding_blob: Option<Vec<u8>> = row.get(9)?;
            let embedding = embedding_blob
                .as_deref()
                .and_then(from_le_bytes)
                .unwrap_or_else(|| embed_text(&node.raw_text));

            Ok(MemoryCandidate { node, embedding })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IdentityError::from)
    }
}

struct MemoryCandidate {
    node: MemoryNode,
    embedding: [f32; EMBEDDING_DIM],
}

fn score_node(node: &MemoryNode, tokens: &[String]) -> u32 {
    let haystack = format!(
        "{} {} {} {} {}",
        node.summary, node.raw_text, node.source, node.domain_context, node.entity_type
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
    use super::{query_tokens, summarize, IdentityStore};
    use crate::transit::CleanedEvent;
    use crate::workspace::SovereignPaths;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn inserts_memory_node_from_cleaned_event_idempotently() {
        let root = std::env::temp_dir().join(format!(
            "sovereign-identity-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = SovereignPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 42,
            captured_event_id: 7,
            source: "test".to_string(),
            cleaned_content: "Sovereign stores local memory.".to_string(),
            content_hash: "hash".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        let first = store.insert_memory_from_cleaned(&cleaned).unwrap();
        let second = store.insert_memory_from_cleaned(&cleaned).unwrap();
        let memories = store.list_recent(10).unwrap();

        assert_eq!(first, second);
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].summary, "Sovereign stores local memory.");

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn summary_is_bounded() {
        let long = "a".repeat(300);
        assert_eq!(summarize(&long).len(), 240);
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
    fn searches_memory_nodes_by_token_overlap() {
        let root = std::env::temp_dir().join(format!(
            "sovereign-search-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = SovereignPaths::from_root(root.clone());
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
    fn stores_fixed_width_vector_embedding_on_promotion() {
        let root = std::env::temp_dir().join(format!(
            "sovereign-vector-store-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = SovereignPaths::from_root(root.clone());
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
}
