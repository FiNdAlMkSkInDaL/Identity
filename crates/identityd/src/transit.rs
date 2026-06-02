use crate::ingest_safety::{validate_capture, IngestSafetyError};
use crate::workspace::IdentityPaths;
use rusqlite::{params, Connection};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
pub struct TransitEvent {
    pub id: i64,
    pub source: String,
    pub content: String,
    pub status: String,
    pub captured_at_ms: i64,
    pub claimed_at_ms: Option<i64>,
    pub processed_at_ms: Option<i64>,
    pub retry_count: i64,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct TransitStatusCount {
    pub status: String,
    pub count: i64,
}

#[derive(Debug)]
pub struct TransitRepairSummary {
    pub stale_processing_requeued: usize,
}

#[derive(Debug)]
pub struct TransitRedactionSummary {
    pub redacted_captured_events: usize,
    pub redacted_cleaned_events: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitHealth {
    pub queued: i64,
    pub processing: i64,
    pub processed: i64,
    pub failed: i64,
    pub stale_processing: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitBudgetProbe {
    pub insert_rollback_ms: u128,
}

#[derive(Debug)]
pub struct CleanedEvent {
    pub id: i64,
    pub captured_event_id: i64,
    pub source: String,
    pub cleaned_content: String,
    pub content_hash: String,
    pub cleaned_at_ms: i64,
    pub promoted_at_ms: Option<i64>,
}

#[derive(Debug)]
pub enum TransitError {
    ClockBeforeUnixEpoch,
    IngestSafety(IngestSafetyError),
    Sqlite(rusqlite::Error),
}

impl fmt::Display for TransitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClockBeforeUnixEpoch => write!(f, "system clock is before the Unix epoch"),
            Self::IngestSafety(error) => write!(f, "{error}"),
            Self::Sqlite(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TransitError {}

impl From<rusqlite::Error> for TransitError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

impl From<IngestSafetyError> for TransitError {
    fn from(value: IngestSafetyError) -> Self {
        Self::IngestSafety(value)
    }
}

pub struct TransitBuffer {
    conn: Connection,
}

pub const DEFAULT_PROCESSING_LEASE_MS: i64 = 5 * 60 * 1000;

impl TransitBuffer {
    pub fn open(paths: &IdentityPaths) -> Result<Self, TransitError> {
        let conn = Connection::open(&paths.transit_db)?;
        let buffer = Self { conn };
        buffer.initialize_schema()?;
        buffer.migrate_schema()?;
        Ok(buffer)
    }

    pub fn ingest_text(&self, source: &str, content: &str) -> Result<i64, TransitError> {
        validate_capture(source, content)?;
        let captured_at_ms = now_ms()?;

        self.conn.execute(
            "INSERT INTO captured_events (source, content, status, captured_at_ms)
             VALUES (?1, ?2, 'queued', ?3)",
            params![source, content, captured_at_ms],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn probe_insert_rollback_latency(&self) -> Result<TransitBudgetProbe, TransitError> {
        let started = std::time::Instant::now();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO captured_events (source, content, status, captured_at_ms)
             VALUES ('doctor:budget-probe', 'local rollback latency probe', 'queued', ?1)",
            [now_ms()?],
        )?;
        tx.rollback()?;

        Ok(TransitBudgetProbe {
            insert_rollback_ms: started.elapsed().as_millis(),
        })
    }

    pub fn list_recent(&self, limit: u32) -> Result<Vec<TransitEvent>, TransitError> {
        let mut statement = self.conn.prepare(
            "SELECT id, source, content, status, captured_at_ms, claimed_at_ms, processed_at_ms, retry_count, error
             FROM captured_events
             ORDER BY captured_at_ms DESC, id DESC
             LIMIT ?1",
        )?;

        let rows = statement.query_map([limit as i64], |row| {
            Ok(TransitEvent {
                id: row.get(0)?,
                source: row.get(1)?,
                content: row.get(2)?,
                status: row.get(3)?,
                captured_at_ms: row.get(4)?,
                claimed_at_ms: row.get(5)?,
                processed_at_ms: row.get(6)?,
                retry_count: row.get(7)?,
                error: row.get(8)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(TransitError::from)
    }

    pub fn claim_queued(&self, limit: u32) -> Result<Vec<TransitEvent>, TransitError> {
        self.repair_stale_processing(DEFAULT_PROCESSING_LEASE_MS)?;

        let claimed_at_ms = now_ms()?;
        let tx = self.conn.unchecked_transaction()?;

        let ids = {
            let mut statement = tx.prepare(
                "SELECT id
                 FROM captured_events
                 WHERE status = 'queued'
                 ORDER BY captured_at_ms ASC, id ASC
                 LIMIT ?1",
            )?;

            let ids = statement
                .query_map([limit as i64], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;

            ids
        };

        for id in &ids {
            tx.execute(
                "UPDATE captured_events
                 SET status = 'processing', claimed_at_ms = ?1, error = NULL
                 WHERE id = ?2 AND status = 'queued'",
                params![claimed_at_ms, id],
            )?;
        }

        tx.commit()?;

        self.get_events_by_ids(&ids)
    }

    pub fn repair_stale_processing(
        &self,
        lease_timeout_ms: i64,
    ) -> Result<TransitRepairSummary, TransitError> {
        let stale_before_ms = now_ms()?.saturating_sub(lease_timeout_ms.max(0));

        let changed = self.conn.execute(
            "UPDATE captured_events
             SET status = 'queued',
                 claimed_at_ms = NULL,
                 retry_count = retry_count + 1,
                 error = 'requeued after stale processing lease'
             WHERE status = 'processing'
               AND claimed_at_ms IS NOT NULL
               AND claimed_at_ms < ?1",
            [stale_before_ms],
        )?;

        Ok(TransitRepairSummary {
            stale_processing_requeued: changed,
        })
    }

    pub fn mark_processed(&self, id: i64) -> Result<(), TransitError> {
        let processed_at_ms = now_ms()?;

        self.conn.execute(
            "UPDATE captured_events
             SET status = 'processed', processed_at_ms = ?1, error = NULL
             WHERE id = ?2",
            params![processed_at_ms, id],
        )?;

        Ok(())
    }

    pub fn store_cleaned_event(
        &self,
        captured_event_id: i64,
        source: &str,
        cleaned_content: &str,
    ) -> Result<i64, TransitError> {
        let cleaned_at_ms = now_ms()?;
        let content_hash = stable_hash_hex(cleaned_content.as_bytes());

        self.conn.execute(
            "INSERT INTO cleaned_events
                (captured_event_id, source, cleaned_content, content_hash, cleaned_at_ms, promoted_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![
                captured_event_id,
                source,
                cleaned_content,
                content_hash,
                cleaned_at_ms
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn complete_processing_with_cleaned(
        &self,
        captured_event_id: i64,
        source: &str,
        cleaned_content: &str,
    ) -> Result<i64, TransitError> {
        let now = now_ms()?;
        let content_hash = stable_hash_hex(cleaned_content.as_bytes());
        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "INSERT INTO cleaned_events
                (captured_event_id, source, cleaned_content, content_hash, cleaned_at_ms, promoted_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)
             ON CONFLICT(captured_event_id) DO UPDATE SET
                source = excluded.source,
                cleaned_content = excluded.cleaned_content,
                content_hash = excluded.content_hash,
                cleaned_at_ms = excluded.cleaned_at_ms,
                promoted_at_ms = cleaned_events.promoted_at_ms",
            params![
                captured_event_id,
                source,
                cleaned_content,
                content_hash,
                now
            ],
        )?;

        let cleaned_id = tx.query_row(
            "SELECT id FROM cleaned_events WHERE captured_event_id = ?1",
            [captured_event_id],
            |row| row.get::<_, i64>(0),
        )?;

        tx.execute(
            "UPDATE captured_events
             SET status = 'processed',
                 processed_at_ms = ?1,
                 error = NULL
             WHERE id = ?2
               AND status = 'processing'",
            params![now, captured_event_id],
        )?;

        tx.commit()?;

        Ok(cleaned_id)
    }

    pub fn mark_failed(&self, id: i64, error: &str) -> Result<(), TransitError> {
        self.conn.execute(
            "UPDATE captured_events
             SET status = 'failed', error = ?1
             WHERE id = ?2",
            params![error, id],
        )?;

        Ok(())
    }

    pub fn status_counts(&self) -> Result<Vec<TransitStatusCount>, TransitError> {
        let mut statement = self.conn.prepare(
            "SELECT status, COUNT(*)
             FROM captured_events
             GROUP BY status
             ORDER BY status ASC",
        )?;

        let rows = statement.query_map([], |row| {
            Ok(TransitStatusCount {
                status: row.get(0)?,
                count: row.get(1)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(TransitError::from)
    }

    pub fn health(&self, lease_timeout_ms: i64) -> Result<TransitHealth, TransitError> {
        let mut health = TransitHealth {
            queued: 0,
            processing: 0,
            processed: 0,
            failed: 0,
            stale_processing: 0,
        };

        for count in self.status_counts()? {
            match count.status.as_str() {
                "queued" => health.queued = count.count,
                "processing" => health.processing = count.count,
                "processed" => health.processed = count.count,
                "failed" => health.failed = count.count,
                _ => {}
            }
        }

        let stale_before_ms = now_ms()?.saturating_sub(lease_timeout_ms.max(0));
        health.stale_processing = self.conn.query_row(
            "SELECT COUNT(*)
             FROM captured_events
             WHERE status = 'processing'
               AND claimed_at_ms IS NOT NULL
               AND claimed_at_ms < ?1",
            [stale_before_ms],
            |row| row.get(0),
        )?;

        Ok(health)
    }

    pub fn list_cleaned_recent(&self, limit: u32) -> Result<Vec<CleanedEvent>, TransitError> {
        let mut statement = self.conn.prepare(
            "SELECT id, captured_event_id, source, cleaned_content, content_hash, cleaned_at_ms, promoted_at_ms
             FROM cleaned_events
             ORDER BY cleaned_at_ms DESC, id DESC
             LIMIT ?1",
        )?;

        let rows = statement.query_map([limit as i64], cleaned_event_from_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(TransitError::from)
    }

    pub fn list_cleaned_pending(&self, limit: u32) -> Result<Vec<CleanedEvent>, TransitError> {
        let mut statement = self.conn.prepare(
            "SELECT id, captured_event_id, source, cleaned_content, content_hash, cleaned_at_ms, promoted_at_ms
             FROM cleaned_events
             WHERE promoted_at_ms IS NULL
             ORDER BY cleaned_at_ms ASC, id ASC
             LIMIT ?1",
        )?;

        let rows = statement.query_map([limit as i64], cleaned_event_from_row)?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(TransitError::from)
    }

    pub fn mark_cleaned_promoted(&self, id: i64) -> Result<(), TransitError> {
        let promoted_at_ms = now_ms()?;

        self.conn.execute(
            "UPDATE cleaned_events
             SET promoted_at_ms = ?1
             WHERE id = ?2",
            params![promoted_at_ms, id],
        )?;

        Ok(())
    }

    pub fn redact_promoted_content(
        &self,
        limit: u32,
    ) -> Result<TransitRedactionSummary, TransitError> {
        let now = now_ms()?;
        let tx = self.conn.unchecked_transaction()?;
        let ids = {
            let mut statement = tx.prepare(
                "SELECT cleaned_events.id, cleaned_events.captured_event_id
                 FROM cleaned_events
                 JOIN captured_events ON captured_events.id = cleaned_events.captured_event_id
                 WHERE cleaned_events.promoted_at_ms IS NOT NULL
                   AND (
                        cleaned_events.cleaned_content != ''
                        OR captured_events.content != ''
                   )
                 ORDER BY cleaned_events.promoted_at_ms ASC, cleaned_events.id ASC
                 LIMIT ?1",
            )?;

            let ids = statement
                .query_map([limit as i64], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;

            ids
        };

        let mut redacted_cleaned_events = 0;
        let mut redacted_captured_events = 0;

        for (cleaned_id, captured_id) in &ids {
            redacted_cleaned_events += tx.execute(
                "UPDATE cleaned_events
                 SET cleaned_content = '',
                     content_redacted_at_ms = ?1
                 WHERE id = ?2
                   AND cleaned_content != ''",
                params![now, cleaned_id],
            )?;
            redacted_captured_events += tx.execute(
                "UPDATE captured_events
                 SET content = '',
                     content_redacted_at_ms = ?1
                 WHERE id = ?2
                   AND content != ''",
                params![now, captured_id],
            )?;
        }

        tx.commit()?;

        Ok(TransitRedactionSummary {
            redacted_captured_events,
            redacted_cleaned_events,
        })
    }

    fn initialize_schema(&self) -> Result<(), TransitError> {
        self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;

             CREATE TABLE IF NOT EXISTS captured_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                source TEXT NOT NULL,
                content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'queued',
                captured_at_ms INTEGER NOT NULL,
                claimed_at_ms INTEGER,
                processed_at_ms INTEGER,
                retry_count INTEGER NOT NULL DEFAULT 0,
                content_redacted_at_ms INTEGER,
                error TEXT
             );

             CREATE INDEX IF NOT EXISTS idx_captured_events_status
                ON captured_events(status);

             CREATE INDEX IF NOT EXISTS idx_captured_events_captured_at
                ON captured_events(captured_at_ms);

             CREATE TABLE IF NOT EXISTS cleaned_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                captured_event_id INTEGER NOT NULL UNIQUE,
                source TEXT NOT NULL,
                cleaned_content TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                cleaned_at_ms INTEGER NOT NULL,
                promoted_at_ms INTEGER,
                content_redacted_at_ms INTEGER,
                FOREIGN KEY(captured_event_id) REFERENCES captured_events(id)
             );

             CREATE INDEX IF NOT EXISTS idx_cleaned_events_content_hash
                ON cleaned_events(content_hash);

             CREATE INDEX IF NOT EXISTS idx_cleaned_events_cleaned_at
                ON cleaned_events(cleaned_at_ms);",
        )?;

        Ok(())
    }

    fn migrate_schema(&self) -> Result<(), TransitError> {
        if !self.has_column("captured_events", "claimed_at_ms")? {
            self.conn.execute(
                "ALTER TABLE captured_events ADD COLUMN claimed_at_ms INTEGER",
                [],
            )?;
        }

        if !self.has_column("captured_events", "retry_count")? {
            self.conn.execute(
                "ALTER TABLE captured_events ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }

        if !self.has_column("captured_events", "content_redacted_at_ms")? {
            self.conn.execute(
                "ALTER TABLE captured_events ADD COLUMN content_redacted_at_ms INTEGER",
                [],
            )?;
        }

        if !self.has_column("cleaned_events", "promoted_at_ms")? {
            self.conn.execute(
                "ALTER TABLE cleaned_events ADD COLUMN promoted_at_ms INTEGER",
                [],
            )?;
        }

        if !self.has_column("cleaned_events", "content_redacted_at_ms")? {
            self.conn.execute(
                "ALTER TABLE cleaned_events ADD COLUMN content_redacted_at_ms INTEGER",
                [],
            )?;
        }

        Ok(())
    }

    fn has_column(&self, table: &str, column: &str) -> Result<bool, TransitError> {
        let mut statement = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let rows = statement.query_map([], |row| row.get::<_, String>(1))?;

        for row in rows {
            if row? == column {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn get_events_by_ids(&self, ids: &[i64]) -> Result<Vec<TransitEvent>, TransitError> {
        let mut events = Vec::with_capacity(ids.len());

        for id in ids {
            let event = self.conn.query_row(
                "SELECT id, source, content, status, captured_at_ms, claimed_at_ms, processed_at_ms, retry_count, error
                 FROM captured_events
                 WHERE id = ?1",
                [id],
                |row| {
                    Ok(TransitEvent {
                        id: row.get(0)?,
                        source: row.get(1)?,
                        content: row.get(2)?,
                        status: row.get(3)?,
                        captured_at_ms: row.get(4)?,
                        claimed_at_ms: row.get(5)?,
                        processed_at_ms: row.get(6)?,
                        retry_count: row.get(7)?,
                        error: row.get(8)?,
                    })
                },
            )?;

            events.push(event);
        }

        Ok(events)
    }
}

fn cleaned_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CleanedEvent> {
    Ok(CleanedEvent {
        id: row.get(0)?,
        captured_event_id: row.get(1)?,
        source: row.get(2)?,
        cleaned_content: row.get(3)?,
        content_hash: row.get(4)?,
        cleaned_at_ms: row.get(5)?,
        promoted_at_ms: row.get(6)?,
    })
}

fn now_ms() -> Result<i64, TransitError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| TransitError::ClockBeforeUnixEpoch)?;

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
    use super::TransitBuffer;
    use crate::workspace::IdentityPaths;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn claims_oldest_queued_events_and_marks_processed() {
        let root = std::env::temp_dir().join(format!(
            "identity-transit-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        let first = buffer.ingest_text("test:first", "first").unwrap();
        let second = buffer.ingest_text("test:second", "second").unwrap();

        let claimed = buffer.claim_queued(1).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, first);
        assert_eq!(claimed[0].status, "processing");

        buffer.mark_processed(first).unwrap();

        let counts = buffer.status_counts().unwrap();
        assert!(counts
            .iter()
            .any(|entry| entry.status == "processed" && entry.count == 1));
        assert!(counts
            .iter()
            .any(|entry| entry.status == "queued" && entry.count == 1));

        let claimed = buffer.claim_queued(1).unwrap();
        assert_eq!(claimed[0].id, second);

        drop(buffer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stores_cleaned_events_as_embedding_stage_handoff() {
        let root = std::env::temp_dir().join(format!(
            "identity-cleaned-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        let captured_id = buffer.ingest_text("test:cleaned", "raw text").unwrap();
        let cleaned_id = buffer
            .store_cleaned_event(captured_id, "test:cleaned", "raw text")
            .unwrap();

        let cleaned = buffer.list_cleaned_recent(10).unwrap();
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0].id, cleaned_id);
        assert_eq!(cleaned[0].captured_event_id, captured_id);
        assert_eq!(cleaned[0].cleaned_content, "raw text");
        assert_eq!(cleaned[0].content_hash.len(), 16);
        assert_eq!(cleaned[0].promoted_at_ms, None);

        let pending = buffer.list_cleaned_pending(10).unwrap();
        assert_eq!(pending.len(), 1);

        buffer.mark_cleaned_promoted(cleaned_id).unwrap();
        let pending = buffer.list_cleaned_pending(10).unwrap();
        assert_eq!(pending.len(), 0);

        drop(buffer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_sensitive_capture_before_persisting_raw_content() {
        let root = std::env::temp_dir().join(format!(
            "identity-sensitive-capture-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        let result = buffer.ingest_text("filesystem:C:/Users/me/.env", "password=secret");
        assert!(result.is_err());

        let events = buffer.list_recent(10).unwrap();
        assert!(events.is_empty());

        drop(buffer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_processing_claims_are_requeued_with_retry_count() {
        let root = std::env::temp_dir().join(format!(
            "identity-stale-claim-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        let captured_id = buffer.ingest_text("test:stale", "stale text").unwrap();
        let claimed = buffer.claim_queued(1).unwrap();
        assert_eq!(claimed[0].id, captured_id);
        assert_eq!(claimed[0].retry_count, 0);

        std::thread::sleep(std::time::Duration::from_millis(2));
        let repair = buffer.repair_stale_processing(0).unwrap();
        assert_eq!(repair.stale_processing_requeued, 1);

        let reclaimed = buffer.claim_queued(1).unwrap();
        assert_eq!(reclaimed.len(), 1);
        assert_eq!(reclaimed[0].id, captured_id);
        assert_eq!(reclaimed[0].retry_count, 1);
        assert_eq!(reclaimed[0].status, "processing");

        drop(buffer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completion_atomically_stores_cleaned_text_and_marks_processed() {
        let root = std::env::temp_dir().join(format!(
            "identity-atomic-complete-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        let captured_id = buffer.ingest_text("test:atomic", "raw text").unwrap();
        let claimed = buffer.claim_queued(1).unwrap();
        assert_eq!(claimed[0].id, captured_id);

        let cleaned_id = buffer
            .complete_processing_with_cleaned(captured_id, "test:atomic", "clean text")
            .unwrap();
        let cleaned = buffer.list_cleaned_recent(10).unwrap();
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0].id, cleaned_id);
        assert_eq!(cleaned[0].cleaned_content, "clean text");

        let events = buffer.list_recent(10).unwrap();
        assert_eq!(events[0].id, captured_id);
        assert_eq!(events[0].status, "processed");
        assert!(events[0].processed_at_ms.is_some());

        drop(buffer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleaned_upsert_preserves_promotion_marker() {
        let root = std::env::temp_dir().join(format!(
            "identity-cleaned-upsert-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        let captured_id = buffer.ingest_text("test:upsert", "raw text").unwrap();
        buffer.claim_queued(1).unwrap();
        let cleaned_id = buffer
            .complete_processing_with_cleaned(captured_id, "test:upsert", "first clean")
            .unwrap();
        buffer.mark_cleaned_promoted(cleaned_id).unwrap();

        buffer
            .complete_processing_with_cleaned(captured_id, "test:upsert", "second clean")
            .unwrap();

        let cleaned = buffer.list_cleaned_recent(10).unwrap();
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0].id, cleaned_id);
        assert_eq!(cleaned[0].cleaned_content, "second clean");
        assert!(cleaned[0].promoted_at_ms.is_some());

        drop(buffer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_transit_health_with_stale_processing_count() {
        let root = std::env::temp_dir().join(format!(
            "identity-transit-health-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        buffer.ingest_text("test:health", "health text").unwrap();
        buffer.claim_queued(1).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));

        let health = buffer.health(0).unwrap();
        assert_eq!(health.processing, 1);
        assert_eq!(health.stale_processing, 1);

        drop(buffer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn redacts_promoted_transit_and_cleaned_content() {
        let root = std::env::temp_dir().join(format!(
            "identity-transit-redact-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        let captured_id = buffer.ingest_text("test:redact", "raw local text").unwrap();
        buffer.claim_queued(1).unwrap();
        let cleaned_id = buffer
            .complete_processing_with_cleaned(captured_id, "test:redact", "clean local text")
            .unwrap();

        buffer.mark_cleaned_promoted(cleaned_id).unwrap();
        let summary = buffer.redact_promoted_content(10).unwrap();
        assert_eq!(summary.redacted_captured_events, 1);
        assert_eq!(summary.redacted_cleaned_events, 1);

        let captured = buffer.list_recent(10).unwrap();
        let cleaned = buffer.list_cleaned_recent(10).unwrap();
        assert_eq!(captured[0].content, "");
        assert_eq!(cleaned[0].cleaned_content, "");
        assert_eq!(cleaned[0].content_hash.len(), 16);

        drop(buffer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn budget_probe_does_not_persist_capture_rows() {
        let root = std::env::temp_dir().join(format!(
            "identity-transit-budget-probe-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        let probe = buffer.probe_insert_rollback_latency().unwrap();
        assert!(probe.insert_rollback_ms < 10_000);
        assert!(buffer.list_recent(10).unwrap().is_empty());

        drop(buffer);
        fs::remove_dir_all(root).unwrap();
    }
}
