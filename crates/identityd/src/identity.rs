use crate::crypto::{
    is_protected_text, protect_text, unprotect_text, CryptoError, PROTECTED_PREFIX,
};
use crate::embedding::{
    from_le_bytes, EmbeddingArtifactHealth, EmbeddingEngine, EmbeddingRuntimeInfo, EMBEDDING_DIM,
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
    pub node_uid: String,
    pub cleaned_event_id: i64,
    pub source: String,
    pub domain_context: String,
    pub entity_type: String,
    pub summary: String,
    pub structured_attributes: String,
    pub raw_text: String,
    pub content_hash: String,
    pub created_at_ms: i64,
    pub created_at_utc: String,
    pub last_accessed_ms: i64,
    pub last_accessed_utc: String,
}

#[derive(Debug)]
pub struct MemorySearchResult {
    pub node: MemoryNode,
    pub score: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtocolGraphEdge {
    pub target_node_id: String,
    pub relationship_type: String,
    pub edge_weight: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtocolMemoryNode {
    pub node_id: String,
    pub timestamp_created: String,
    pub timestamp_last_accessed: String,
    pub domain_context: String,
    pub entity_type: String,
    pub raw_text: String,
    pub summary_tokens: String,
    pub structured_attributes: String,
    pub vector_embedding: Vec<f32>,
    pub graph_edges: Vec<ProtocolGraphEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolSchemaHealth {
    pub node_count: i64,
    pub valid_node_ids: i64,
    pub valid_timestamps: i64,
    pub valid_structured_attributes: i64,
    pub valid_vector_dimensions: i64,
}

impl ProtocolSchemaHealth {
    pub fn is_ready(&self) -> bool {
        self.node_count == self.valid_node_ids
            && self.node_count == self.valid_timestamps
            && self.node_count == self.valid_structured_attributes
            && self.node_count == self.valid_vector_dimensions
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolSchemaRepairSummary {
    pub repaired_node_ids: usize,
    pub repaired_timestamps: usize,
    pub repaired_structured_attributes: usize,
    pub repaired_vectors: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VectorMirrorHealth {
    pub node_count: i64,
    pub sqlite_vectorized_count: i64,
    pub primary_mirrored_count: i64,
    pub primary_missing_count: i64,
}

impl VectorMirrorHealth {
    pub fn is_ready(&self) -> bool {
        self.sqlite_vectorized_count == self.node_count
            && self.primary_mirrored_count == self.node_count
            && self.primary_missing_count == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryStats {
    pub node_count: i64,
    pub node_uid_count: i64,
    pub timestamp_utc_count: i64,
    pub last_accessed_count: i64,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryProtectionHealth {
    pub unprotected_semantic_fields: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryProtectionSummary {
    pub protected_semantic_fields: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdge {
    pub id: i64,
    pub source_node_id: i64,
    pub target_node_id: i64,
    pub relationship_type: String,
    pub edge_weight: f64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeStats {
    pub edge_count: i64,
    pub decayed_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeDecaySummary {
    pub edges_decayed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphHealth {
    pub node_count: i64,
    pub edge_count: i64,
    pub orphan_count: i64,
    pub decayed_edges: i64,
}

#[derive(Debug)]
pub enum IdentityError {
    ClockBeforeUnixEpoch,
    Crypto(CryptoError),
    EmbeddingModelMismatch(String),
    InvalidGraphEdge(String),
    Random(std::io::Error),
    Sqlite(rusqlite::Error),
    VectorStore(VectorStoreError),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClockBeforeUnixEpoch => write!(f, "system clock is before the Unix epoch"),
            Self::Crypto(error) => write!(f, "{error}"),
            Self::EmbeddingModelMismatch(message) => write!(f, "{message}"),
            Self::InvalidGraphEdge(reason) => write!(f, "invalid graph edge: {reason}"),
            Self::Random(error) => write!(f, "failed to generate local node UUID: {error}"),
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

impl From<CryptoError> for IdentityError {
    fn from(value: CryptoError) -> Self {
        Self::Crypto(value)
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
        let requested_embedding = EmbeddingEngine::new();
        let backend = SqliteMemoryBackend::open(paths)?;
        let embedding = backend.select_embedding_engine(requested_embedding)?;
        backend.migrate_embedding_metadata(&embedding)?;
        let vector_store = VectorStore::open(paths, embedding.model_id(), embedding.dimension())?;
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
        self.backend.link_similar_nodes(
            id,
            &record.source,
            &self.embedding,
            &self.vector_store,
            3,
            0.5,
        )?;
        Ok(id)
    }

    pub fn list_recent(&self, limit: u32) -> Result<Vec<MemoryNode>, IdentityError> {
        self.backend.list_recent(limit)
    }

    pub fn export_recent_protocol_json(&self, limit: u32) -> Result<String, IdentityError> {
        let candidates =
            self.backend
                .list_recent_with_embeddings(limit, &self.embedding, &self.vector_store)?;
        let mut nodes = Vec::with_capacity(candidates.len());

        for candidate in candidates {
            let graph_edges = self.backend.protocol_edges_for_node(candidate.node.id)?;
            nodes.push(ProtocolMemoryNode {
                node_id: candidate.node.node_uid,
                timestamp_created: candidate.node.created_at_utc,
                timestamp_last_accessed: candidate.node.last_accessed_utc,
                domain_context: candidate.node.domain_context,
                entity_type: candidate.node.entity_type,
                raw_text: candidate.node.raw_text,
                summary_tokens: candidate.node.summary,
                structured_attributes: candidate.node.structured_attributes,
                vector_embedding: candidate.embedding.to_vec(),
                graph_edges,
            });
        }

        Ok(protocol_nodes_json(&nodes))
    }

    pub fn protocol_schema_health(&self) -> Result<ProtocolSchemaHealth, IdentityError> {
        self.backend.protocol_schema_health(&self.embedding)
    }

    pub fn repair_protocol_schema(
        &self,
        limit: u32,
    ) -> Result<ProtocolSchemaRepairSummary, IdentityError> {
        self.backend
            .repair_protocol_schema(limit, &self.embedding, &self.vector_store)
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
        let candidates =
            self.backend
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
        self.backend
            .mark_nodes_accessed(results.iter().map(|result| result.node.id))?;

        Ok(results)
    }

    pub fn stats(&self) -> Result<MemoryStats, IdentityError> {
        self.backend.stats(&self.embedding, &self.vector_store)
    }

    pub fn embedding_runtime_info(&self) -> EmbeddingRuntimeInfo {
        self.embedding.runtime_info()
    }

    pub fn embedding_artifact_health(&self) -> EmbeddingArtifactHealth {
        self.embedding.artifact_health()
    }

    pub fn vector_mirror_health(&self) -> Result<VectorMirrorHealth, IdentityError> {
        self.backend
            .vector_mirror_health(&self.embedding, &self.vector_store)
    }

    pub fn repair_vectors(&self, limit: u32) -> Result<MemoryRepairSummary, IdentityError> {
        self.backend
            .repair_vectors(limit, &self.embedding, &self.vector_store)
    }

    pub fn protection_health(&self) -> Result<MemoryProtectionHealth, IdentityError> {
        self.backend.protection_health()
    }

    pub fn protect_legacy_semantic_text(
        &self,
        limit: u32,
    ) -> Result<MemoryProtectionSummary, IdentityError> {
        self.backend.protect_legacy_semantic_text(limit)
    }

    pub fn upsert_edge(
        &self,
        source_node_id: i64,
        target_node_id: i64,
        relationship_type: &str,
        edge_weight: f64,
    ) -> Result<GraphEdge, IdentityError> {
        self.backend.upsert_edge(
            source_node_id,
            target_node_id,
            relationship_type,
            edge_weight,
        )
    }

    pub fn link_nodes(
        &self,
        source_node_id: i64,
        target_node_id: i64,
        relationship_type: &str,
        edge_weight: f64,
    ) -> Result<GraphEdge, IdentityError> {
        self.upsert_edge(
            source_node_id,
            target_node_id,
            relationship_type,
            edge_weight,
        )
    }

    pub fn list_edges(&self, limit: u32) -> Result<Vec<GraphEdge>, IdentityError> {
        self.backend.list_edges(limit)
    }

    pub fn get_edges_for_node(&self, node_id: i64) -> Result<Vec<GraphEdge>, IdentityError> {
        self.backend.get_edges_for_node(node_id)
    }

    pub fn decay_edges(&self, limit: u32) -> Result<EdgeDecaySummary, IdentityError> {
        self.backend.decay_edges(limit)
    }

    pub fn edge_stats(&self) -> Result<EdgeStats, IdentityError> {
        self.backend.edge_stats()
    }

    pub fn graph_health(&self) -> Result<GraphHealth, IdentityError> {
        self.backend.graph_health()
    }
}

struct SqliteMemoryBackend {
    conn: Connection,
}

impl SqliteMemoryBackend {
    fn open(paths: &IdentityPaths) -> Result<Self, IdentityError> {
        let conn = Connection::open(&paths.identity_db)?;
        let backend = Self { conn };
        backend.initialize_schema()?;
        backend.migrate_schema()?;
        Ok(backend)
    }

    fn select_embedding_engine(
        &self,
        requested: EmbeddingEngine,
    ) -> Result<EmbeddingEngine, IdentityError> {
        let Some(stored_model_id) = self.meta_value("embedding_model_id")? else {
            return Ok(requested);
        };

        if stored_model_id == requested.model_id() {
            return Ok(requested);
        }

        let node_count = self.node_count()?;

        if node_count == 0 {
            return Ok(requested);
        }

        if stored_model_id == crate::embedding::EMBEDDING_MODEL_ID {
            return Ok(EmbeddingEngine::hash());
        }

        Err(IdentityError::EmbeddingModelMismatch(format!(
            "identity.me was embedded with model '{stored_model_id}', but active model '{}' was requested; explicit local re-embedding is required before switching runtimes",
            requested.model_id()
        )))
    }

    fn migrate_embedding_metadata(&self, embedding: &EmbeddingEngine) -> Result<(), IdentityError> {
        self.set_meta_value("embedding_model_id", embedding.model_id())?;
        self.set_meta_value("embedding_runtime", embedding.runtime())?;
        self.set_meta_value("embedding_dim", &embedding.dimension().to_string())
    }

    fn insert_memory_record(&self, record: &MemoryRecord) -> Result<i64, IdentityError> {
        let protected_source = protect_text(&record.source)?;
        let protected_summary = protect_text(&record.summary)?;
        let protected_structured_attributes = protect_text(&record.structured_attributes)?;
        let protected_raw_text = protect_text(&record.raw_text)?;
        let created_at_utc = iso8601_utc_from_ms(record.created_at_ms);

        self.conn.execute(
            "INSERT OR IGNORE INTO memory_nodes
                (node_uid, cleaned_event_id, source, domain_context, entity_type, summary, structured_attributes, raw_text, content_hash, vector_embedding, created_at_ms, created_at_utc, last_accessed_ms, last_accessed_utc)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                generate_node_uid()?,
                record.cleaned_event_id,
                &protected_source,
                &record.domain_context,
                &record.entity_type,
                &protected_summary,
                &protected_structured_attributes,
                &protected_raw_text,
                &record.content_hash,
                &record.vector_embedding,
                record.created_at_ms,
                &created_at_utc,
                record.created_at_ms,
                created_at_utc
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
            let primary_ready = vector_store
                .read_primary(id)?
                .map(|primary| primary.len() == bytes.len())
                .unwrap_or(false);
            if !primary_ready {
                vector_store.upsert(id, &bytes)?;
            }
        }

        Ok(())
    }

    fn list_recent(&self, limit: u32) -> Result<Vec<MemoryNode>, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT id, node_uid, cleaned_event_id, source, domain_context, entity_type, summary, structured_attributes, raw_text, content_hash, created_at_ms, created_at_utc, last_accessed_ms, last_accessed_utc
             FROM memory_nodes
             ORDER BY created_at_ms DESC, id DESC
             LIMIT ?1",
        )?;

        let rows = statement.query_map([limit as i64], map_memory_node)?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(decrypt_memory_node)
            .collect()
    }

    fn list_recent_with_embeddings(
        &self,
        limit: u32,
        embedding: &EmbeddingEngine,
        vector_store: &VectorStore,
    ) -> Result<Vec<MemoryCandidate>, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT id, node_uid, cleaned_event_id, source, domain_context, entity_type, summary, structured_attributes, raw_text, content_hash, created_at_ms, created_at_utc, last_accessed_ms, last_accessed_utc
             FROM memory_nodes
             ORDER BY created_at_ms DESC, id DESC
             LIMIT ?1",
        )?;

        let mut rows = statement.query([limit as i64])?;
        let mut candidates = Vec::new();

        while let Some(row) = rows.next()? {
            let node = decrypt_memory_node(map_memory_node(row)?)?;
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
        let node_uid_count = self.conn.query_row(
            "SELECT COUNT(*) FROM memory_nodes WHERE node_uid IS NOT NULL AND node_uid != ''",
            [],
            |row| row.get(0),
        )?;
        let timestamp_utc_count = self.conn.query_row(
            "SELECT COUNT(*) FROM memory_nodes WHERE created_at_utc IS NOT NULL AND created_at_utc != ''",
            [],
            |row| row.get(0),
        )?;
        let last_accessed_count = self.conn.query_row(
            "SELECT COUNT(*) FROM memory_nodes
             WHERE last_accessed_ms IS NOT NULL
               AND last_accessed_utc IS NOT NULL
               AND last_accessed_utc != ''",
            [],
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
            node_uid_count,
            timestamp_utc_count,
            last_accessed_count,
            vectorized_count,
            invalid_vector_count,
            embedding_model_id,
            embedding_dim,
            vector_store_backend: vector_store.backend_name().to_string(),
        })
    }

    fn vector_mirror_health(
        &self,
        embedding: &EmbeddingEngine,
        vector_store: &VectorStore,
    ) -> Result<VectorMirrorHealth, IdentityError> {
        let expected_len = embedding.blob_len() as i64;
        let mut statement = self.conn.prepare(
            "SELECT id, length(vector_embedding)
             FROM memory_nodes
             ORDER BY id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
            ))
        })?;

        let mut health = VectorMirrorHealth {
            node_count: 0,
            sqlite_vectorized_count: 0,
            primary_mirrored_count: 0,
            primary_missing_count: 0,
        };

        for row in rows {
            let (id, sqlite_len) = row?;
            health.node_count += 1;

            if sqlite_len == expected_len {
                health.sqlite_vectorized_count += 1;
            }

            let primary_ready = vector_store
                .read_primary(id)?
                .map(|bytes| bytes.len() as i64 == expected_len)
                .unwrap_or(false);

            if primary_ready {
                health.primary_mirrored_count += 1;
            } else {
                health.primary_missing_count += 1;
            }
        }

        Ok(health)
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
            let plaintext = unprotect_text(raw_text)?;
            let vector_embedding = embedding.encode_bytes(&plaintext);
            self.conn.execute(
                "UPDATE memory_nodes
                 SET vector_embedding = ?1
                 WHERE id = ?2",
                params![&vector_embedding, id],
            )?;
            vector_store.upsert(*id, &vector_embedding)?;
        }

        Ok(MemoryRepairSummary {
            repaired_vectors: repairs.len(),
        })
    }

    fn protocol_schema_health(
        &self,
        embedding: &EmbeddingEngine,
    ) -> Result<ProtocolSchemaHealth, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT node_uid,
                    created_at_utc,
                    last_accessed_utc,
                    structured_attributes,
                    length(vector_embedding)
             FROM memory_nodes
             ORDER BY id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?.unwrap_or(0),
            ))
        })?;

        let mut health = ProtocolSchemaHealth {
            node_count: 0,
            valid_node_ids: 0,
            valid_timestamps: 0,
            valid_structured_attributes: 0,
            valid_vector_dimensions: 0,
        };
        let expected_vector_bytes = embedding.blob_len() as i64;

        for row in rows {
            let (node_uid, created_at, last_accessed, structured_attributes, vector_bytes) = row?;
            let structured_attributes = unprotect_text(&structured_attributes)?;

            health.node_count += 1;
            if is_uuid_v4_like(&node_uid) {
                health.valid_node_ids += 1;
            }
            if is_iso8601_utc_timestamp(&created_at) && is_iso8601_utc_timestamp(&last_accessed) {
                health.valid_timestamps += 1;
            }
            if is_json_object_like(&structured_attributes) {
                health.valid_structured_attributes += 1;
            }
            if vector_bytes == expected_vector_bytes {
                health.valid_vector_dimensions += 1;
            }
        }

        Ok(health)
    }

    fn repair_protocol_schema(
        &self,
        limit: u32,
        embedding: &EmbeddingEngine,
        vector_store: &VectorStore,
    ) -> Result<ProtocolSchemaRepairSummary, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT id,
                    node_uid,
                    created_at_ms,
                    created_at_utc,
                    last_accessed_ms,
                    last_accessed_utc,
                    structured_attributes,
                    raw_text,
                    length(vector_embedding)
             FROM memory_nodes
             ORDER BY id ASC
             LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<i64>>(8)?.unwrap_or(0),
            ))
        })?;
        let rows = rows.collect::<Result<Vec<_>, _>>()?;
        let expected_vector_bytes = embedding.blob_len() as i64;
        let mut summary = ProtocolSchemaRepairSummary {
            repaired_node_ids: 0,
            repaired_timestamps: 0,
            repaired_structured_attributes: 0,
            repaired_vectors: 0,
        };

        for (
            id,
            node_uid,
            created_at_ms,
            created_at_utc,
            last_accessed_ms,
            last_accessed_utc,
            structured_attributes,
            raw_text,
            vector_bytes,
        ) in rows
        {
            if !is_uuid_v4_like(&node_uid) {
                self.conn.execute(
                    "UPDATE memory_nodes SET node_uid = ?1 WHERE id = ?2",
                    params![generate_node_uid()?, id],
                )?;
                summary.repaired_node_ids += 1;
            }

            if !is_iso8601_utc_timestamp(&created_at_utc)
                || !is_iso8601_utc_timestamp(&last_accessed_utc)
            {
                self.conn.execute(
                    "UPDATE memory_nodes
                     SET created_at_utc = ?1,
                         last_accessed_utc = ?2
                     WHERE id = ?3",
                    params![
                        iso8601_utc_from_ms(created_at_ms),
                        iso8601_utc_from_ms(last_accessed_ms),
                        id
                    ],
                )?;
                summary.repaired_timestamps += 1;
            }

            let structured_attributes_plaintext = unprotect_text(&structured_attributes)?;
            if !is_json_object_like(&structured_attributes_plaintext) {
                self.conn.execute(
                    "UPDATE memory_nodes SET structured_attributes = ?1 WHERE id = ?2",
                    params![protect_text("{}")?, id],
                )?;
                summary.repaired_structured_attributes += 1;
            }

            if vector_bytes != expected_vector_bytes {
                let raw_text = unprotect_text(&raw_text)?;
                let vector_embedding = embedding.encode_bytes(&raw_text);
                self.conn.execute(
                    "UPDATE memory_nodes SET vector_embedding = ?1 WHERE id = ?2",
                    params![&vector_embedding, id],
                )?;
                vector_store.upsert(id, &vector_embedding)?;
                summary.repaired_vectors += 1;
            }
        }

        Ok(summary)
    }

    fn protection_health(&self) -> Result<MemoryProtectionHealth, IdentityError> {
        Ok(MemoryProtectionHealth {
            unprotected_semantic_fields: self.count_unprotected_fields(&[
                "source",
                "summary",
                "structured_attributes",
                "raw_text",
            ])?,
        })
    }

    fn protect_legacy_semantic_text(
        &self,
        limit: u32,
    ) -> Result<MemoryProtectionSummary, IdentityError> {
        let tx = self.conn.unchecked_transaction()?;
        let protected = protect_memory_fields(
            &tx,
            &["source", "summary", "structured_attributes", "raw_text"],
            limit,
        )?;
        tx.commit()?;

        Ok(MemoryProtectionSummary {
            protected_semantic_fields: protected,
        })
    }

    fn upsert_edge(
        &self,
        source_node_id: i64,
        target_node_id: i64,
        relationship_type: &str,
        edge_weight: f64,
    ) -> Result<GraphEdge, IdentityError> {
        let now = now_ms()?;
        let relationship = relationship_type.trim();

        if source_node_id <= 0 || target_node_id <= 0 {
            return Err(IdentityError::InvalidGraphEdge(
                "node ids must be positive persisted memory node ids".to_string(),
            ));
        }

        if source_node_id == target_node_id {
            return Err(IdentityError::InvalidGraphEdge(
                "self edges are not allowed".to_string(),
            ));
        }

        if relationship.is_empty() || relationship.len() > 64 {
            return Err(IdentityError::InvalidGraphEdge(
                "relationship type must be 1..=64 bytes".to_string(),
            ));
        }

        if !edge_weight.is_finite() {
            return Err(IdentityError::InvalidGraphEdge(
                "edge weight must be finite".to_string(),
            ));
        }

        let weight = edge_weight.clamp(0.0, 1.0);

        self.conn.execute(
            "INSERT INTO graph_edges
                (source_node_id, target_node_id, relationship_type, edge_weight, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(source_node_id, target_node_id, relationship_type) DO UPDATE SET
                edge_weight = excluded.edge_weight,
                updated_at_ms = excluded.updated_at_ms",
            params![source_node_id, target_node_id, relationship, weight, now, now],
        )?;

        self.conn.query_row(
            "SELECT id, source_node_id, target_node_id, relationship_type, edge_weight, created_at_ms, updated_at_ms
             FROM graph_edges
             WHERE source_node_id = ?1 AND target_node_id = ?2 AND relationship_type = ?3",
            (source_node_id, target_node_id, relationship),
            map_graph_edge,
        )
        .map_err(IdentityError::from)
    }

    fn link_similar_nodes(
        &self,
        node_id: i64,
        source: &str,
        embedding: &EmbeddingEngine,
        vector_store: &VectorStore,
        max_links: usize,
        min_similarity: f64,
    ) -> Result<(), IdentityError> {
        let new_blob = match vector_store.read(node_id)? {
            Some(blob) if blob.len() == embedding.blob_len() => blob,
            _ => return Ok(()),
        };

        let new_embedding = match from_le_bytes(&new_blob) {
            Some(e) => e,
            None => return Ok(()),
        };

        let mut statement = self.conn.prepare(
            "SELECT id, source FROM memory_nodes WHERE id != ?1 ORDER BY id DESC LIMIT 200",
        )?;
        let rows = statement.query_map([node_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut candidates = Vec::new();

        for row in rows {
            let (other_id, other_source) = row?;
            let other_source = unprotect_text(&other_source)?;
            if let Some(other_blob) = vector_store.read(other_id)? {
                if let Some(other_embedding) = from_le_bytes(&other_blob) {
                    let similarity = embedding.similarity(&new_embedding, &other_embedding) as f64;
                    if similarity >= min_similarity {
                        candidates.push((other_id, other_source, similarity));
                    }
                }
            }
        }

        candidates.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        let relationship = edge_relationship_from_source(source);

        for (other_id, other_source, similarity) in candidates.into_iter().take(max_links) {
            let reverse_rel = edge_relationship_from_source(&other_source);
            self.upsert_edge(node_id, other_id, relationship, similarity.clamp(0.0, 1.0))?;
            self.upsert_edge(other_id, node_id, reverse_rel, similarity.clamp(0.0, 1.0))?;
        }

        Ok(())
    }

    fn list_edges(&self, limit: u32) -> Result<Vec<GraphEdge>, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT id, source_node_id, target_node_id, relationship_type, edge_weight, created_at_ms, updated_at_ms
             FROM graph_edges
             ORDER BY updated_at_ms DESC, id DESC
             LIMIT ?1",
        )?;

        let rows = statement.query_map([limit as i64], map_graph_edge)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IdentityError::from)
    }

    fn get_edges_for_node(&self, node_id: i64) -> Result<Vec<GraphEdge>, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT id, source_node_id, target_node_id, relationship_type, edge_weight, created_at_ms, updated_at_ms
             FROM graph_edges
             WHERE source_node_id = ?1 OR target_node_id = ?1
             ORDER BY edge_weight DESC, updated_at_ms DESC",
        )?;

        let rows = statement.query_map([node_id], map_graph_edge)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IdentityError::from)
    }

    fn protocol_edges_for_node(
        &self,
        node_id: i64,
    ) -> Result<Vec<ProtocolGraphEdge>, IdentityError> {
        let mut statement = self.conn.prepare(
            "SELECT target.node_uid, edge.relationship_type, edge.edge_weight
             FROM graph_edges edge
             JOIN memory_nodes target ON target.id = edge.target_node_id
             WHERE edge.source_node_id = ?1
             ORDER BY edge.edge_weight DESC, edge.updated_at_ms DESC
             LIMIT 32",
        )?;

        let rows = statement.query_map([node_id], |row| {
            Ok(ProtocolGraphEdge {
                target_node_id: row.get(0)?,
                relationship_type: row.get(1)?,
                edge_weight: row.get(2)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IdentityError::from)
    }

    fn decay_edges(&self, limit: u32) -> Result<EdgeDecaySummary, IdentityError> {
        let now = now_ms()?;
        let edges = self.list_edges(limit)?;
        let mut decayed = 0;

        for edge in &edges {
            let delta_ms = now.saturating_sub(edge.updated_at_ms);
            let delta_hours = (delta_ms as f64) / (3600.0 * 1000.0);

            let alpha = if delta_hours < 24.0 { 0.1 } else { 0.4 };
            let new_weight = (edge.edge_weight * (1.0 - alpha)).clamp(0.0, 1.0);

            if (new_weight - edge.edge_weight).abs() > 1e-9 {
                self.conn.execute(
                    "UPDATE graph_edges SET edge_weight = ?1, updated_at_ms = ?2 WHERE id = ?3",
                    params![new_weight, now, edge.id],
                )?;
                decayed += 1;
            }
        }

        Ok(EdgeDecaySummary {
            edges_decayed: decayed,
        })
    }

    fn edge_stats(&self) -> Result<EdgeStats, IdentityError> {
        let edge_count = self
            .conn
            .query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get(0))?;
        let decayed_count = self.conn.query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE edge_weight < 0.5",
            [],
            |row| row.get(0),
        )?;

        Ok(EdgeStats {
            edge_count,
            decayed_count,
        })
    }

    fn graph_health(&self) -> Result<GraphHealth, IdentityError> {
        let node_count = self
            .conn
            .query_row("SELECT COUNT(*) FROM memory_nodes", [], |row| row.get(0))?;
        let edge_count = self
            .conn
            .query_row("SELECT COUNT(*) FROM graph_edges", [], |row| row.get(0))?;
        let orphan_count = self.conn.query_row(
            "SELECT COUNT(*) FROM memory_nodes m
             WHERE NOT EXISTS (SELECT 1 FROM graph_edges WHERE source_node_id = m.id OR target_node_id = m.id)",
            [],
            |row| row.get(0),
        )?;
        let decayed_edges = self.conn.query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE edge_weight < 0.5",
            [],
            |row| row.get(0),
        )?;

        Ok(GraphHealth {
            node_count,
            edge_count,
            orphan_count,
            decayed_edges,
        })
    }

    fn initialize_schema(&self) -> Result<(), IdentityError> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;

             CREATE TABLE IF NOT EXISTS memory_nodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                node_uid TEXT NOT NULL UNIQUE,
                cleaned_event_id INTEGER NOT NULL UNIQUE,
                source TEXT NOT NULL,
                domain_context TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                summary TEXT NOT NULL,
                structured_attributes TEXT NOT NULL DEFAULT '{}',
                raw_text TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                vector_embedding BLOB NOT NULL,
                created_at_ms INTEGER NOT NULL,
                created_at_utc TEXT NOT NULL,
                last_accessed_ms INTEGER NOT NULL,
                last_accessed_utc TEXT NOT NULL
             );

             CREATE INDEX IF NOT EXISTS idx_memory_nodes_content_hash
                ON memory_nodes(content_hash);

             CREATE INDEX IF NOT EXISTS idx_memory_nodes_created_at
                ON memory_nodes(created_at_ms);

             CREATE TABLE IF NOT EXISTS graph_edges (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                source_node_id INTEGER NOT NULL,
                target_node_id INTEGER NOT NULL,
                relationship_type TEXT NOT NULL,
                edge_weight REAL NOT NULL DEFAULT 1.0,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                FOREIGN KEY (source_node_id) REFERENCES memory_nodes(id) ON DELETE CASCADE,
                FOREIGN KEY (target_node_id) REFERENCES memory_nodes(id) ON DELETE CASCADE
             );

             CREATE UNIQUE INDEX IF NOT EXISTS idx_graph_edges_source_target_type
                ON graph_edges(source_node_id, target_node_id, relationship_type);

             CREATE INDEX IF NOT EXISTS idx_graph_edges_target
                ON graph_edges(target_node_id);

             CREATE TABLE IF NOT EXISTS store_metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL
             );",
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

        if !self.has_column("memory_nodes", "structured_attributes")? {
            self.conn.execute(
                "ALTER TABLE memory_nodes ADD COLUMN structured_attributes TEXT NOT NULL DEFAULT '{}'",
                [],
            )?;
        }

        if !self.has_column("memory_nodes", "node_uid")? {
            self.conn
                .execute("ALTER TABLE memory_nodes ADD COLUMN node_uid TEXT", [])?;
        }
        self.backfill_missing_node_uids()?;
        self.conn.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_memory_nodes_node_uid
             ON memory_nodes(node_uid)",
            [],
        )?;

        if !self.has_column("memory_nodes", "created_at_utc")? {
            self.conn.execute(
                "ALTER TABLE memory_nodes ADD COLUMN created_at_utc TEXT",
                [],
            )?;
        }
        self.backfill_missing_created_at_utc()?;

        if !self.has_column("memory_nodes", "last_accessed_ms")? {
            self.conn.execute(
                "ALTER TABLE memory_nodes ADD COLUMN last_accessed_ms INTEGER",
                [],
            )?;
        }
        if !self.has_column("memory_nodes", "last_accessed_utc")? {
            self.conn.execute(
                "ALTER TABLE memory_nodes ADD COLUMN last_accessed_utc TEXT",
                [],
            )?;
        }
        self.backfill_missing_last_accessed()?;

        Ok(())
    }

    fn node_count(&self) -> Result<i64, IdentityError> {
        self.conn
            .query_row("SELECT COUNT(*) FROM memory_nodes", [], |row| row.get(0))
            .map_err(IdentityError::from)
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

    fn count_unprotected_fields(&self, fields: &[&str]) -> Result<i64, IdentityError> {
        let mut total = 0;

        for field in fields {
            let sql = format!(
                "SELECT COUNT(*)
                 FROM memory_nodes
                 WHERE {field} != ''
                   AND {field} NOT LIKE ?1"
            );
            total += self
                .conn
                .query_row(&sql, [protected_like_pattern()], |row| row.get::<_, i64>(0))?;
        }

        Ok(total)
    }

    fn backfill_missing_node_uids(&self) -> Result<(), IdentityError> {
        let rows = {
            let mut statement = self.conn.prepare(
                "SELECT id
                 FROM memory_nodes
                 WHERE node_uid IS NULL OR node_uid = ''
                 ORDER BY id ASC",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, i64>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        for id in rows {
            self.conn.execute(
                "UPDATE memory_nodes SET node_uid = ?1 WHERE id = ?2",
                params![generate_node_uid()?, id],
            )?;
        }

        Ok(())
    }

    fn backfill_missing_created_at_utc(&self) -> Result<(), IdentityError> {
        let rows = {
            let mut statement = self.conn.prepare(
                "SELECT id, created_at_ms
                 FROM memory_nodes
                 WHERE created_at_utc IS NULL OR created_at_utc = ''
                 ORDER BY id ASC",
            )?;
            let rows = statement
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        for (id, created_at_ms) in rows {
            self.conn.execute(
                "UPDATE memory_nodes SET created_at_utc = ?1 WHERE id = ?2",
                params![iso8601_utc_from_ms(created_at_ms), id],
            )?;
        }

        Ok(())
    }

    fn backfill_missing_last_accessed(&self) -> Result<(), IdentityError> {
        let rows = {
            let mut statement = self.conn.prepare(
                "SELECT id, created_at_ms
                 FROM memory_nodes
                 WHERE last_accessed_ms IS NULL
                    OR last_accessed_utc IS NULL
                    OR last_accessed_utc = ''
                 ORDER BY id ASC",
            )?;
            let rows = statement
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        for (id, created_at_ms) in rows {
            self.conn.execute(
                "UPDATE memory_nodes
                 SET last_accessed_ms = ?1,
                     last_accessed_utc = ?2
                 WHERE id = ?3",
                params![created_at_ms, iso8601_utc_from_ms(created_at_ms), id],
            )?;
        }

        Ok(())
    }

    fn mark_nodes_accessed<I>(&self, node_ids: I) -> Result<(), IdentityError>
    where
        I: IntoIterator<Item = i64>,
    {
        let node_ids = node_ids.into_iter().collect::<Vec<_>>();
        if node_ids.is_empty() {
            return Ok(());
        }

        let now = now_ms()?;
        let now_utc = iso8601_utc_from_ms(now);
        let tx = self.conn.unchecked_transaction()?;

        for node_id in node_ids {
            tx.execute(
                "UPDATE memory_nodes
                 SET last_accessed_ms = ?1,
                     last_accessed_utc = ?2
                 WHERE id = ?3",
                params![now, &now_utc, node_id],
            )?;
        }

        tx.commit()?;
        Ok(())
    }
}

struct MemoryCandidate {
    node: MemoryNode,
    embedding: [f32; EMBEDDING_DIM],
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
        node_uid: row.get(1)?,
        cleaned_event_id: row.get(2)?,
        source: row.get(3)?,
        domain_context: row.get(4)?,
        entity_type: row.get(5)?,
        summary: row.get(6)?,
        structured_attributes: row.get(7)?,
        raw_text: row.get(8)?,
        content_hash: row.get(9)?,
        created_at_ms: row.get(10)?,
        created_at_utc: row.get(11)?,
        last_accessed_ms: row.get(12)?,
        last_accessed_utc: row.get(13)?,
    })
}

fn decrypt_memory_node(mut node: MemoryNode) -> Result<MemoryNode, IdentityError> {
    node.source = unprotect_text(&node.source)?;
    node.summary = unprotect_text(&node.summary)?;
    node.structured_attributes = unprotect_text(&node.structured_attributes)?;
    node.raw_text = unprotect_text(&node.raw_text)?;
    Ok(node)
}

fn protect_memory_fields(
    tx: &rusqlite::Transaction<'_>,
    fields: &[&str],
    limit: u32,
) -> Result<usize, IdentityError> {
    let select_fields = fields.join(", ");
    let sql = format!("SELECT id, {select_fields} FROM memory_nodes ORDER BY id ASC LIMIT ?1");
    let rows = {
        let mut statement = tx.prepare(&sql)?;
        let rows = statement.query_map([limit as i64], |row| {
            let id = row.get::<_, i64>(0)?;
            let values = (0..fields.len())
                .map(|index| row.get::<_, String>(index + 1))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((id, values))
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut protected_fields = 0;

    for (id, values) in rows {
        for (field, value) in fields.iter().zip(values.iter()) {
            if value.is_empty() || is_protected_text(value) {
                continue;
            }

            let protected = protect_text(value)?;
            let update = format!("UPDATE memory_nodes SET {field} = ?1 WHERE id = ?2");
            tx.execute(&update, params![protected, id])?;
            protected_fields += 1;
        }
    }

    Ok(protected_fields)
}

fn protected_like_pattern() -> String {
    format!("{PROTECTED_PREFIX}%")
}

fn map_graph_edge(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphEdge> {
    Ok(GraphEdge {
        id: row.get(0)?,
        source_node_id: row.get(1)?,
        target_node_id: row.get(2)?,
        relationship_type: row.get(3)?,
        edge_weight: row.get(4)?,
        created_at_ms: row.get(5)?,
        updated_at_ms: row.get(6)?,
    })
}

struct CaptureClassification {
    domain_context: &'static str,
    entity_type: &'static str,
}

fn edge_relationship_from_source(source: &str) -> &'static str {
    if source.starts_with("windows-ui:") {
        "RELATED_TO"
    } else if source.starts_with("filesystem:") {
        "DOCUMENTS"
    } else if source.starts_with("local-proxy:") {
        "REFERENCES"
    } else {
        "RELATED_TO"
    }
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
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0C}' => escaped.push_str("\\f"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            _ => escaped.push(character),
        }
    }

    escaped
}

fn protocol_nodes_json(nodes: &[ProtocolMemoryNode]) -> String {
    let mut output = String::from("[");

    for (index, node) in nodes.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(&protocol_node_json(node));
    }

    output.push(']');
    output
}

fn protocol_node_json(node: &ProtocolMemoryNode) -> String {
    let mut output = String::from("{");
    push_json_string_field(&mut output, "node_id", &node.node_id, false);
    push_json_string_field(
        &mut output,
        "timestamp_created",
        &node.timestamp_created,
        true,
    );
    push_json_string_field(
        &mut output,
        "timestamp_last_accessed",
        &node.timestamp_last_accessed,
        true,
    );
    push_json_string_field(&mut output, "domain_context", &node.domain_context, true);
    push_json_string_field(&mut output, "entity_type", &node.entity_type, true);
    output.push_str(",\"semantic_payload\":{");
    push_json_string_field(&mut output, "raw_text", &node.raw_text, false);
    push_json_string_field(&mut output, "summary_tokens", &node.summary_tokens, true);
    output.push_str(",\"structured_attributes\":");
    output.push_str(normalized_json_object(&node.structured_attributes));
    output.push('}');
    output.push_str(",\"vector_embedding\":");
    push_vector_json(&mut output, &node.vector_embedding);
    output.push_str(",\"graph_edges\":");
    push_protocol_edges_json(&mut output, &node.graph_edges);
    output.push('}');
    output
}

fn push_json_string_field(output: &mut String, key: &str, value: &str, needs_comma: bool) {
    if needs_comma {
        output.push(',');
    }
    output.push('"');
    output.push_str(key);
    output.push_str("\":\"");
    output.push_str(&json_escape(value));
    output.push('"');
}

fn normalized_json_object(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        trimmed
    } else {
        "{}"
    }
}

fn push_vector_json(output: &mut String, vector: &[f32]) {
    output.push('[');
    for (index, value) in vector.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(&format!("{value:.6}"));
    }
    output.push(']');
}

fn push_protocol_edges_json(output: &mut String, edges: &[ProtocolGraphEdge]) {
    output.push('[');
    for (index, edge) in edges.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push('{');
        push_json_string_field(output, "target_node_id", &edge.target_node_id, false);
        push_json_string_field(output, "relationship_type", &edge.relationship_type, true);
        output.push_str(",\"edge_weight\":");
        output.push_str(&format!("{:.6}", edge.edge_weight.clamp(0.0, 1.0)));
        output.push('}');
    }
    output.push(']');
}

fn is_uuid_v4_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }

    for (index, byte) in bytes.iter().enumerate() {
        match index {
            8 | 13 | 18 | 23 => {
                if *byte != b'-' {
                    return false;
                }
            }
            14 => {
                if *byte != b'4' {
                    return false;
                }
            }
            19 => {
                if !matches!(*byte, b'8' | b'9' | b'a' | b'b') {
                    return false;
                }
            }
            _ => {
                if !byte.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }

    true
}

fn is_iso8601_utc_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 24
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && bytes[23] == b'Z'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[11..13].iter().all(u8::is_ascii_digit)
        && bytes[14..16].iter().all(u8::is_ascii_digit)
        && bytes[17..19].iter().all(u8::is_ascii_digit)
        && bytes[20..23].iter().all(u8::is_ascii_digit)
}

fn is_json_object_like(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with('{') && trimmed.ends_with('}')
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

fn generate_node_uid() -> Result<String, IdentityError> {
    let mut bytes = [0_u8; 16];
    fill_random_bytes(&mut bytes).map_err(IdentityError::Random)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format_uuid_bytes(&bytes))
}

fn format_uuid_bytes(bytes: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}

fn iso8601_utc_from_ms(timestamp_ms: i64) -> String {
    let seconds = timestamp_ms.div_euclid(1000);
    let millis = timestamp_ms.rem_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }

    (year, month, day)
}

#[cfg(windows)]
fn fill_random_bytes(bytes: &mut [u8]) -> std::io::Result<()> {
    #[link(name = "advapi32")]
    extern "system" {
        fn SystemFunction036(random_buffer: *mut u8, random_buffer_length: u32) -> u8;
    }

    let ok = unsafe { SystemFunction036(bytes.as_mut_ptr(), bytes.len() as u32) };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn fill_random_bytes(bytes: &mut [u8]) -> std::io::Result<()> {
    use std::io::Read;

    let mut file = std::fs::File::open("/dev/urandom")?;
    file.read_exact(bytes)
}

#[cfg(not(any(unix, windows)))]
fn fill_random_bytes(_bytes: &mut [u8]) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "os random source unavailable",
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        capture_attributes, classify_capture, format_uuid_bytes, is_iso8601_utc_timestamp,
        is_json_object_like, is_uuid_v4_like, iso8601_utc_from_ms, labelled_value,
        protocol_nodes_json, query_tokens, summarize, summarize_capture,
        summarize_windows_activity, windows_activity_attributes, IdentityStore, ProtocolGraphEdge,
        ProtocolMemoryNode, SqliteMemoryBackend,
    };
    use crate::crypto::is_protected_text;
    use crate::embedding::{
        ActiveEmbeddingHealth, EmbeddingArtifactHealth, EmbeddingEngine, EMBEDDING_DIM,
        EMBEDDING_MODEL_ID, EMBEDDING_ONNX_MODEL_PATH_ENV, EMBEDDING_RUNTIME_ENV,
        EMBEDDING_RUNTIME_HASH, EMBEDDING_RUNTIME_ONNX,
    };
    use crate::transit::CleanedEvent;
    use crate::workspace::IdentityPaths;
    use rusqlite::params;
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
        assert_eq!(memories[0].node_uid.len(), 36);
        assert_eq!(memories[0].node_uid.as_bytes()[14], b'4');
        assert!(matches!(
            memories[0].node_uid.as_bytes()[19],
            b'8' | b'9' | b'a' | b'b'
        ));
        assert!(memories[0].created_at_utc.ends_with('Z'));
        assert_eq!(memories[0].summary, "Identity stores local memory.");

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uuid_formatter_uses_canonical_layout() {
        let uuid = format_uuid_bytes(&[
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0x4d, 0xef, 0x80, 0x12, 0x34, 0x56, 0x78, 0x9a,
            0xbc, 0xde,
        ]);

        assert_eq!(uuid, "12345678-9abc-4def-8012-3456789abcde");
    }

    #[test]
    fn utc_timestamp_formatter_handles_epoch_and_milliseconds() {
        assert_eq!(iso8601_utc_from_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            iso8601_utc_from_ms(1_735_689_599_123),
            "2024-12-31T23:59:59.123Z"
        );
    }

    #[test]
    fn protocol_schema_validators_are_strict_but_lightweight() {
        assert!(is_uuid_v4_like("12345678-9abc-4def-8012-3456789abcde"));
        assert!(!is_uuid_v4_like("12345678-9abc-5def-8012-3456789abcde"));
        assert!(is_iso8601_utc_timestamp("2024-12-31T23:59:59.123Z"));
        assert!(!is_iso8601_utc_timestamp("2024-12-31 23:59:59"));
        assert!(is_json_object_like("{\"ok\":\"yes\"}"));
        assert!(!is_json_object_like("[\"not-object\"]"));
    }

    #[test]
    fn protocol_json_escapes_text_and_falls_back_from_malformed_attributes() {
        let json = protocol_nodes_json(&[ProtocolMemoryNode {
            node_id: "00000000-0000-4000-8000-000000000001".to_string(),
            timestamp_created: "1970-01-01T00:00:00.000Z".to_string(),
            timestamp_last_accessed: "1970-01-01T00:00:00.000Z".to_string(),
            domain_context: "local.capture".to_string(),
            entity_type: "DOCUMENT".to_string(),
            raw_text: "Line one\n\"line two\"".to_string(),
            summary_tokens: "Summary with tab\tmarker".to_string(),
            structured_attributes: "not-json".to_string(),
            vector_embedding: vec![0.25, -0.5],
            graph_edges: vec![ProtocolGraphEdge {
                target_node_id: "00000000-0000-4000-8000-000000000002".to_string(),
                relationship_type: "RELATED_TO".to_string(),
                edge_weight: 1.5,
            }],
        }]);

        assert!(json.contains("\"raw_text\":\"Line one\\n\\\"line two\\\"\""));
        assert!(json.contains("\"summary_tokens\":\"Summary with tab\\tmarker\""));
        assert!(json.contains("\"structured_attributes\":{}"));
        assert!(json.contains("\"vector_embedding\":[0.250000,-0.500000]"));
        assert!(json.contains("\"edge_weight\":1.000000"));
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
    fn search_marks_returned_memory_nodes_accessed() {
        let root = std::env::temp_dir().join(format!(
            "identity-last-access-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 14,
            captured_event_id: 14,
            source: "test".to_string(),
            cleaned_content: "Search access should advance dynamic memory timestamps.".to_string(),
            content_hash: "hash14".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&cleaned).unwrap();
        let before = store.list_recent(1).unwrap()[0].last_accessed_ms;
        std::thread::sleep(std::time::Duration::from_millis(2));

        let results = store.search("dynamic memory", 1).unwrap();
        assert_eq!(results.len(), 1);

        let after = store.list_recent(1).unwrap()[0].last_accessed_ms;
        assert!(after > before);
        assert!(store.list_recent(1).unwrap()[0]
            .last_accessed_utc
            .ends_with('Z'));

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exports_recent_memory_in_protocol_shape_without_local_row_ids() {
        let root = std::env::temp_dir().join(format!(
            "identity-protocol-export-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let first = CleanedEvent {
            id: 21,
            captured_event_id: 21,
            source: "test".to_string(),
            cleaned_content: "Protocol export keeps local ids private.".to_string(),
            content_hash: "hash21".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };
        let second = CleanedEvent {
            id: 22,
            captured_event_id: 22,
            source: "test".to_string(),
            cleaned_content: "Related protocol export target.".to_string(),
            content_hash: "hash22".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        let first_id = store.insert_memory_from_cleaned(&first).unwrap();
        let second_id = store.insert_memory_from_cleaned(&second).unwrap();
        store
            .link_nodes(first_id, second_id, "RELATED_TO", 0.75)
            .unwrap();

        let exported = store.export_recent_protocol_json(10).unwrap();

        assert!(exported.starts_with('['));
        assert!(exported.contains("\"node_id\":\""));
        assert!(exported.contains("\"timestamp_created\":\""));
        assert!(exported.contains("\"timestamp_last_accessed\":\""));
        assert!(exported.contains("\"semantic_payload\":{"));
        assert!(exported.contains("\"vector_embedding\":["));
        assert!(exported.contains("\"graph_edges\":["));
        assert!(exported.contains("\"target_node_id\":\""));
        assert!(exported.contains("\"edge_weight\":0.750000"));
        assert!(!exported.contains("cleaned_event_id"));
        assert!(!exported.contains("\"id\":"));

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protocol_schema_health_reports_ready_for_valid_memory() {
        let root = std::env::temp_dir().join(format!(
            "identity-protocol-health-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 23,
            captured_event_id: 23,
            source: "test".to_string(),
            cleaned_content: "Protocol health validates local memory shape.".to_string(),
            content_hash: "hash23".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&cleaned).unwrap();
        let health = store.protocol_schema_health().unwrap();

        assert_eq!(health.node_count, 1);
        assert_eq!(health.valid_node_ids, 1);
        assert_eq!(health.valid_timestamps, 1);
        assert_eq!(health.valid_structured_attributes, 1);
        assert_eq!(health.valid_vector_dimensions, 1);
        assert!(health.is_ready());

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protocol_schema_health_flags_malformed_protocol_fields() {
        let root = std::env::temp_dir().join(format!(
            "identity-protocol-health-corrupt-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 24,
            captured_event_id: 24,
            source: "test".to_string(),
            cleaned_content: "Protocol health should flag malformed rows.".to_string(),
            content_hash: "hash24".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&cleaned).unwrap();
        store
            .backend
            .conn
            .execute(
                "UPDATE memory_nodes
                 SET node_uid = 'bad',
                     created_at_utc = 'bad',
                     last_accessed_utc = 'bad',
                     structured_attributes = 'bad',
                     vector_embedding = ?1
                 WHERE cleaned_event_id = ?2",
                params![vec![1_u8, 2, 3], cleaned.id],
            )
            .unwrap();

        let health = store.protocol_schema_health().unwrap();

        assert_eq!(health.node_count, 1);
        assert_eq!(health.valid_node_ids, 0);
        assert_eq!(health.valid_timestamps, 0);
        assert_eq!(health.valid_structured_attributes, 0);
        assert_eq!(health.valid_vector_dimensions, 0);
        assert!(!health.is_ready());

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repair_protocol_schema_restores_malformed_protocol_fields() {
        let root = std::env::temp_dir().join(format!(
            "identity-protocol-repair-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 25,
            captured_event_id: 25,
            source: "test".to_string(),
            cleaned_content: "Protocol repair should restore local state shape.".to_string(),
            content_hash: "hash25".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&cleaned).unwrap();
        store
            .backend
            .conn
            .execute(
                "UPDATE memory_nodes
                 SET node_uid = 'bad',
                     created_at_utc = 'bad',
                     last_accessed_utc = 'bad',
                     structured_attributes = 'bad',
                     vector_embedding = ?1
                 WHERE cleaned_event_id = ?2",
                params![vec![1_u8, 2, 3], cleaned.id],
            )
            .unwrap();

        assert!(!store.protocol_schema_health().unwrap().is_ready());

        let summary = store.repair_protocol_schema(100).unwrap();
        assert_eq!(summary.repaired_node_ids, 1);
        assert_eq!(summary.repaired_timestamps, 1);
        assert_eq!(summary.repaired_structured_attributes, 1);
        assert_eq!(summary.repaired_vectors, 1);

        let health = store.protocol_schema_health().unwrap();
        assert!(health.is_ready());
        let exported = store.export_recent_protocol_json(1).unwrap();
        assert!(exported.contains("\"structured_attributes\":{}"));
        assert!(!exported.contains("\"node_id\":\"bad\""));

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
            cleaned_content: "Active application: Code.exe Active window title: Identity"
                .to_string(),
            content_hash: "hash21".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&cleaned).unwrap();
        let memories = store.list_recent(10).unwrap();

        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].domain_context, "local.activity.window");
        assert_eq!(memories[0].entity_type, "USER_INTERFACE");
        assert_eq!(
            memories[0].summary,
            "UI activity in Code.exe; window Identity"
        );
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

        assert_eq!(
            summary,
            "UI activity in Code.exe; window Identity; focus Search files"
        );
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
    fn protects_memory_semantic_text_at_rest() {
        let root = std::env::temp_dir().join(format!(
            "identity-memory-protection-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 33,
            captured_event_id: 33,
            source: "test".to_string(),
            cleaned_content: "Private memory raw text stays local and protected.".to_string(),
            content_hash: "hash33".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&cleaned).unwrap();
        let memories = store.list_recent(10).unwrap();
        assert_eq!(
            memories[0].raw_text,
            "Private memory raw text stays local and protected."
        );

        let stored: (String, String, String, String) = store
            .backend
            .conn
            .query_row(
                "SELECT source, summary, structured_attributes, raw_text FROM memory_nodes WHERE cleaned_event_id = ?1",
                [cleaned.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();

        assert_ne!(stored.0, cleaned.source);
        assert_ne!(stored.1, memories[0].summary);
        assert_ne!(stored.2, memories[0].structured_attributes);
        assert_ne!(stored.3, cleaned.cleaned_content);
        assert!(is_protected_text(&stored.0));
        assert!(is_protected_text(&stored.1));
        assert!(is_protected_text(&stored.2));
        assert!(is_protected_text(&stored.3));

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_and_protects_legacy_memory_plaintext() {
        let root = std::env::temp_dir().join(format!(
            "identity-memory-protection-migration-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        store
            .backend
            .conn
            .execute(
                "INSERT INTO memory_nodes
                    (node_uid, cleaned_event_id, source, domain_context, entity_type, summary, structured_attributes, raw_text, content_hash, vector_embedding, created_at_ms, created_at_utc, last_accessed_ms, last_accessed_utc)
                 VALUES (?1, ?2, 'legacy:source', 'local.capture', 'DOCUMENT', 'legacy summary', '{\"legacy\":\"yes\"}', 'legacy raw text', 'hash', ?3, 1, '1970-01-01T00:00:00.001Z', 1, '1970-01-01T00:00:00.001Z')",
                params![
                    "00000000-0000-4000-8000-000000000077",
                    77_i64,
                    vec![0_u8; crate::embedding::EMBEDDING_DIM * 4]
                ],
            )
            .unwrap();

        let before = store.protection_health().unwrap();
        assert_eq!(before.unprotected_semantic_fields, 4);

        let summary = store.protect_legacy_semantic_text(100).unwrap();
        assert_eq!(summary.protected_semantic_fields, 4);

        let after = store.protection_health().unwrap();
        assert_eq!(after.unprotected_semantic_fields, 0);

        let memories = store.list_recent(10).unwrap();
        assert_eq!(memories[0].source, "legacy:source");
        assert_eq!(memories[0].summary, "legacy summary");
        assert_eq!(memories[0].structured_attributes, "{\"legacy\":\"yes\"}");
        assert_eq!(memories[0].raw_text, "legacy raw text");

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backfills_missing_node_uids_on_open() {
        let root = std::env::temp_dir().join(format!(
            "identity-node-uid-backfill-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let cleaned = CleanedEvent {
            id: 34,
            captured_event_id: 34,
            source: "test".to_string(),
            cleaned_content: "Node uid can be repaired locally.".to_string(),
            content_hash: "hash34".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        store.insert_memory_from_cleaned(&cleaned).unwrap();
        store
            .backend
            .conn
            .execute(
                "UPDATE memory_nodes
                 SET node_uid = '',
                     created_at_utc = '',
                     last_accessed_ms = 0,
                     last_accessed_utc = ''
                 WHERE cleaned_event_id = ?1",
                [cleaned.id],
            )
            .unwrap();
        drop(store);

        let reopened = IdentityStore::open(&paths).unwrap();
        let stats = reopened.stats().unwrap();
        let memories = reopened.list_recent(10).unwrap();

        assert_eq!(stats.node_count, 1);
        assert_eq!(stats.node_uid_count, 1);
        assert_eq!(stats.timestamp_utc_count, 1);
        assert_eq!(stats.last_accessed_count, 1);
        assert_eq!(memories[0].node_uid.len(), 36);
        assert!(memories[0].created_at_utc.ends_with('Z'));
        assert!(memories[0].last_accessed_utc.ends_with('Z'));

        drop(reopened);
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
            cleaned_content: "Vectors should also land in the reserved vector-store root."
                .to_string(),
            content_hash: "hash31".to_string(),
            cleaned_at_ms: 1,
            promoted_at_ms: None,
        };

        let node_id = store.insert_memory_from_cleaned(&cleaned).unwrap();
        let stored = store.vector_store.read(node_id).unwrap().unwrap();
        let mirror = store.vector_mirror_health().unwrap();

        assert_eq!(stored.len(), crate::embedding::EMBEDDING_DIM * 4);
        assert_eq!(mirror.node_count, 1);
        assert_eq!(mirror.sqlite_vectorized_count, 1);
        assert_eq!(mirror.primary_mirrored_count, 1);
        assert_eq!(mirror.primary_missing_count, 0);
        assert!(mirror.is_ready());

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
            cleaned_content: "Existing SQLite vectors should backfill the mirror on reopen."
                .to_string(),
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
        let mirror_path = paths
            .vector_store_dir
            .join(format!("node-{node_id:020}.f32le"));
        let mirror = reopened.vector_mirror_health().unwrap();

        assert!(restored.is_some());
        assert_eq!(restored.unwrap().len(), crate::embedding::EMBEDDING_DIM * 4);
        assert!(mirror_path.exists());
        assert_eq!(
            fs::metadata(mirror_path).unwrap().len(),
            (crate::embedding::EMBEDDING_DIM * 4) as u64
        );
        assert_eq!(mirror.primary_mirrored_count, 1);
        assert_eq!(mirror.primary_missing_count, 0);
        assert!(mirror.is_ready());

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
        assert_eq!(stats.node_uid_count, 1);
        assert_eq!(stats.timestamp_utc_count, 1);
        assert_eq!(stats.last_accessed_count, 1);
        assert_eq!(stats.vectorized_count, 1);
        assert_eq!(stats.invalid_vector_count, 0);
        assert_eq!(
            stats.embedding_model_id,
            crate::embedding::EMBEDDING_MODEL_ID
        );
        assert_eq!(stats.embedding_dim, crate::embedding::EMBEDDING_DIM);
        #[cfg(feature = "lancedb-backend")]
        assert_eq!(stats.vector_store_backend, "lancedb+filesystem+sqlite");
        #[cfg(not(feature = "lancedb-backend"))]
        assert_eq!(stats.vector_store_backend, "filesystem+sqlite");

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_hash_store_keeps_hash_engine_when_onnx_is_requested() {
        let root = std::env::temp_dir().join(format!(
            "identity-memory-engine-selection-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 400,
                captured_event_id: 400,
                source: "test".to_string(),
                cleaned_content: "Existing local vectors must keep their model family.".to_string(),
                content_hash: "hash400".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();
        drop(store);

        let backend = SqliteMemoryBackend::open(&paths).unwrap();
        let requested = EmbeddingEngine::from_active_health(
            &ActiveEmbeddingHealth {
                env_var: EMBEDDING_RUNTIME_ENV,
                requested_runtime: EMBEDDING_RUNTIME_ONNX.to_string(),
                active_runtime: EMBEDDING_RUNTIME_ONNX,
                fallback_reason: None,
            },
            &EmbeddingArtifactHealth {
                env_var: EMBEDDING_ONNX_MODEL_PATH_ENV,
                configured: true,
                path: Some("local-model.onnx".to_string()),
                exists: true,
                is_file: true,
                has_onnx_extension: true,
                size_bytes: Some(1),
                manifest_path: Some("local-model.onnx.identity.json".to_string()),
                manifest_exists: true,
                manifest_size_bytes: Some(1),
                manifest_model_id: Some("identity-test-onnx".to_string()),
                manifest_embedding_dim: Some(EMBEDDING_DIM),
                status: "ready",
            },
        );
        let selected = backend.select_embedding_engine(requested).unwrap();

        assert_eq!(selected.model_id(), EMBEDDING_MODEL_ID);
        assert_eq!(selected.runtime(), EMBEDDING_RUNTIME_HASH);

        drop(backend);
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

    #[test]
    fn creates_graph_edges_and_queries_them() {
        let root = std::env::temp_dir().join(format!(
            "identity-edge-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();

        let first = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 101,
                captured_event_id: 101,
                source: "test".to_string(),
                cleaned_content: "First memory node about local-first development.".to_string(),
                content_hash: "hash101".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();

        let second = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 102,
                captured_event_id: 102,
                source: "test".to_string(),
                cleaned_content: "Second memory node about local-first coding.".to_string(),
                content_hash: "hash102".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();

        let edge = store.link_nodes(first, second, "REFERENCES", 0.95).unwrap();

        assert_eq!(edge.source_node_id, first);
        assert_eq!(edge.target_node_id, second);
        assert_eq!(edge.relationship_type, "REFERENCES");
        assert!((edge.edge_weight - 0.95).abs() < 0.001);

        let edges = store.list_edges(10).unwrap();
        assert!(
            edges.len() >= 2,
            "auto-linked similar nodes produce bidirectional edges; got {}",
            edges.len()
        );

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn edge_upsert_replaces_weight_for_same_relationship() {
        let root = std::env::temp_dir().join(format!(
            "identity-edge-upsert-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();

        let first = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 201,
                captured_event_id: 201,
                source: "test".to_string(),
                cleaned_content: "First node.".to_string(),
                content_hash: "hash201".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();

        let second = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 202,
                captured_event_id: 202,
                source: "test".to_string(),
                cleaned_content: "Second node.".to_string(),
                content_hash: "hash202".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();

        store.link_nodes(first, second, "RELATED", 1.0).unwrap();
        store.link_nodes(first, second, "RELATED", 0.5).unwrap();

        let edges = store.get_edges_for_node(first).unwrap();
        assert_eq!(edges.len(), 1);
        assert!((edges[0].edge_weight - 0.5).abs() < 0.001);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn graph_health_reports_node_edge_and_orphan_counts() {
        let root = std::env::temp_dir().join(format!(
            "identity-graph-health-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();

        let first = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 301,
                captured_event_id: 301,
                source: "test".to_string(),
                cleaned_content: "Alpha node.".to_string(),
                content_hash: "hash301".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();

        let second = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 302,
                captured_event_id: 302,
                source: "test".to_string(),
                cleaned_content: "Beta node opposed.".to_string(),
                content_hash: "hash302".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();

        let health_before = store.graph_health().unwrap();
        assert_eq!(health_before.node_count, 2);
        assert!(
            health_before.edge_count >= 0,
            "auto-linking may produce edges; got {}",
            health_before.edge_count
        );
        assert!(
            health_before.orphan_count <= 2,
            "nodes may be linked after insert; orphans={}",
            health_before.orphan_count
        );

        store
            .link_nodes(first, second, "MANUAL_RELATED_TO", 1.0)
            .unwrap();

        let health_after = store.graph_health().unwrap();
        assert_eq!(health_after.node_count, 2);
        assert_eq!(health_after.edge_count, health_before.edge_count + 1);
        assert_eq!(health_after.orphan_count, 0);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn edge_stats_flags_decayed_edges_below_half() {
        let root = std::env::temp_dir().join(format!(
            "identity-edge-stats-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();

        let first = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 401,
                captured_event_id: 401,
                source: "test".to_string(),
                cleaned_content: "Node A.".to_string(),
                content_hash: "hash401".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();

        let second = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 402,
                captured_event_id: 402,
                source: "test".to_string(),
                cleaned_content: "Node B.".to_string(),
                content_hash: "hash402".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();

        store.link_nodes(first, second, "RELATED_TO", 0.49).unwrap();
        let stats = store.edge_stats().unwrap();
        assert!(stats.edge_count >= 1);
        assert!(stats.decayed_count >= 1);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn decay_lowers_weight_using_alpha_short_delta() {
        let root = std::env::temp_dir().join(format!(
            "identity-decay-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();

        let first = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 501,
                captured_event_id: 501,
                source: "test".to_string(),
                cleaned_content: "Node X.".to_string(),
                content_hash: "hash501".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();

        let second = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 502,
                captured_event_id: 502,
                source: "test".to_string(),
                cleaned_content: "Node Y.".to_string(),
                content_hash: "hash502".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();

        store.link_nodes(first, second, "RELATED_TO", 1.0).unwrap();

        let decay_summary = store.decay_edges(100).unwrap();
        let edges = store.list_edges(10).unwrap();

        assert!(decay_summary.edges_decayed >= 1);
        let forward_edge = edges
            .iter()
            .find(|edge| edge.source_node_id == first && edge.target_node_id == second)
            .expect("forward edge exists");
        assert!((forward_edge.edge_weight - 0.9).abs() < 0.001);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_graph_edges() {
        let root = std::env::temp_dir().join(format!(
            "identity-invalid-edge-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let store = IdentityStore::open(&paths).unwrap();
        let node = store
            .insert_memory_from_cleaned(&CleanedEvent {
                id: 601,
                captured_event_id: 601,
                source: "test".to_string(),
                cleaned_content: "Graph validation node.".to_string(),
                content_hash: "hash601".to_string(),
                cleaned_at_ms: 1,
                promoted_at_ms: None,
            })
            .unwrap();

        assert!(store.link_nodes(node, node, "RELATED_TO", 1.0).is_err());
        assert!(store.link_nodes(node, 9999, "RELATED_TO", 1.0).is_err());
        assert!(store.link_nodes(node, 9999, "", 1.0).is_err());
        assert!(store
            .link_nodes(node, 9999, "RELATED_TO", f64::NAN)
            .is_err());
        assert!(store
            .link_nodes(node, 9999, "RELATED_TO", f64::INFINITY)
            .is_err());

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
