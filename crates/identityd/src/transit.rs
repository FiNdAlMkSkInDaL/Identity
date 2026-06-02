use crate::crypto::{
    is_protected_text, protect_text, unprotect_text, CryptoError, PROTECTED_PREFIX,
};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitProtectionHealth {
    pub unprotected_captured_fields: i64,
    pub unprotected_cleaned_fields: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitProtectionSummary {
    pub protected_captured_fields: usize,
    pub protected_cleaned_fields: usize,
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
    Crypto(CryptoError),
    IngestSafety(IngestSafetyError),
    InvalidState(String),
    Sqlite(rusqlite::Error),
}

impl fmt::Display for TransitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClockBeforeUnixEpoch => write!(f, "system clock is before the Unix epoch"),
            Self::Crypto(error) => write!(f, "{error}"),
            Self::IngestSafety(error) => write!(f, "{error}"),
            Self::InvalidState(reason) => write!(f, "invalid transit state: {reason}"),
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

impl From<CryptoError> for TransitError {
    fn from(value: CryptoError) -> Self {
        Self::Crypto(value)
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
        let protected_source = protect_text(source)?;
        let protected_content = protect_text(content)?;

        self.conn.execute(
            "INSERT INTO captured_events (source, content, status, captured_at_ms)
             VALUES (?1, ?2, 'queued', ?3)",
            params![protected_source, protected_content, captured_at_ms],
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

        let rows = statement.query_map([limit as i64], raw_transit_event_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(decrypt_transit_event)
            .collect()
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
        let protected_source = protect_text(source)?;
        let protected_cleaned_content = protect_text(cleaned_content)?;

        self.conn.execute(
            "INSERT INTO cleaned_events
                (captured_event_id, source, cleaned_content, content_hash, cleaned_at_ms, promoted_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![
                captured_event_id,
                protected_source,
                protected_cleaned_content,
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
        let protected_source = protect_text(source)?;
        let protected_cleaned_content = protect_text(cleaned_content)?;
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
                protected_source,
                protected_cleaned_content,
                content_hash,
                now
            ],
        )?;

        let cleaned_id = tx.query_row(
            "SELECT id FROM cleaned_events WHERE captured_event_id = ?1",
            [captured_event_id],
            |row| row.get::<_, i64>(0),
        )?;

        let changed = tx.execute(
            "UPDATE captured_events
             SET status = 'processed',
                 processed_at_ms = ?1,
                 error = NULL
             WHERE id = ?2
               AND status = 'processing'",
            params![now, captured_event_id],
        )?;

        if changed != 1 {
            return Err(TransitError::InvalidState(format!(
                "capture #{captured_event_id} was not in processing state"
            )));
        }

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

    pub fn protection_health(&self) -> Result<TransitProtectionHealth, TransitError> {
        Ok(TransitProtectionHealth {
            unprotected_captured_fields: self
                .count_unprotected_fields("captured_events", &["source", "content"])?,
            unprotected_cleaned_fields: self
                .count_unprotected_fields("cleaned_events", &["source", "cleaned_content"])?,
        })
    }

    pub fn protect_legacy_content(
        &self,
        limit: u32,
    ) -> Result<TransitProtectionSummary, TransitError> {
        let tx = self.conn.unchecked_transaction()?;
        let captured =
            protect_transit_table_fields(&tx, "captured_events", &["source", "content"], limit)?;
        let cleaned = protect_transit_table_fields(
            &tx,
            "cleaned_events",
            &["source", "cleaned_content"],
            limit,
        )?;
        tx.commit()?;

        Ok(TransitProtectionSummary {
            protected_captured_fields: captured,
            protected_cleaned_fields: cleaned,
        })
    }

    pub fn list_cleaned_recent(&self, limit: u32) -> Result<Vec<CleanedEvent>, TransitError> {
        let mut statement = self.conn.prepare(
            "SELECT id, captured_event_id, source, cleaned_content, content_hash, cleaned_at_ms, promoted_at_ms
             FROM cleaned_events
             ORDER BY cleaned_at_ms DESC, id DESC
             LIMIT ?1",
        )?;

        let rows = statement.query_map([limit as i64], cleaned_event_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(decrypt_cleaned_event)
            .collect()
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
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(decrypt_cleaned_event)
            .collect()
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
             PRAGMA foreign_keys = ON;

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

    fn count_unprotected_fields(&self, table: &str, fields: &[&str]) -> Result<i64, TransitError> {
        let mut total = 0;

        for field in fields {
            let sql = format!(
                "SELECT COUNT(*)
                 FROM {table}
                 WHERE {field} != ''
                   AND {field} NOT LIKE ?1"
            );
            total += self
                .conn
                .query_row(&sql, [protected_like_pattern()], |row| row.get::<_, i64>(0))?;
        }

        Ok(total)
    }

    fn get_events_by_ids(&self, ids: &[i64]) -> Result<Vec<TransitEvent>, TransitError> {
        let mut events = Vec::with_capacity(ids.len());

        for id in ids {
            let event = self.conn.query_row(
                "SELECT id, source, content, status, captured_at_ms, claimed_at_ms, processed_at_ms, retry_count, error
                 FROM captured_events
                 WHERE id = ?1",
                [id],
                raw_transit_event_from_row,
            )?;

            events.push(decrypt_transit_event(event)?);
        }

        Ok(events)
    }
}

fn raw_transit_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TransitEvent> {
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
}

fn decrypt_transit_event(mut event: TransitEvent) -> Result<TransitEvent, TransitError> {
    event.source = unprotect_text(&event.source)?;
    event.content = unprotect_text(&event.content)?;
    Ok(event)
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

fn decrypt_cleaned_event(mut event: CleanedEvent) -> Result<CleanedEvent, TransitError> {
    event.source = unprotect_text(&event.source)?;
    event.cleaned_content = unprotect_text(&event.cleaned_content)?;
    Ok(event)
}

fn protect_transit_table_fields(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    fields: &[&str],
    limit: u32,
) -> Result<usize, TransitError> {
    let select_fields = fields.join(", ");
    let sql = format!("SELECT id, {select_fields} FROM {table} ORDER BY id ASC LIMIT ?1");
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
            let update = format!("UPDATE {table} SET {field} = ?1 WHERE id = ?2");
            tx.execute(&update, params![protected, id])?;
            protected_fields += 1;
        }
    }

    Ok(protected_fields)
}

fn protected_like_pattern() -> String {
    format!("{PROTECTED_PREFIX}%")
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
    use crate::crypto::is_protected_text;
    use crate::workspace::IdentityPaths;
    use rusqlite::Connection;
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
    fn protects_transit_content_at_rest() {
        let root = std::env::temp_dir().join(format!(
            "identity-transit-protection-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        let captured_id = buffer
            .ingest_text("test:protected", "plain local capture")
            .unwrap();
        let visible = buffer.list_recent(10).unwrap();
        assert_eq!(visible[0].content, "plain local capture");

        let conn = Connection::open(&paths.transit_db).unwrap();
        let (stored_source, stored_content): (String, String) = conn
            .query_row(
                "SELECT source, content FROM captured_events WHERE id = ?1",
                [captured_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_ne!(stored_source, "test:protected");
        assert_ne!(stored_content, "plain local capture");
        assert!(is_protected_text(&stored_source));
        assert!(is_protected_text(&stored_content));

        drop(conn);
        drop(buffer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_and_protects_legacy_transit_plaintext() {
        let root = std::env::temp_dir().join(format!(
            "identity-transit-protection-migration-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        let conn = Connection::open(&paths.transit_db).unwrap();
        conn.execute(
            "INSERT INTO captured_events (source, content, status, captured_at_ms)
             VALUES ('legacy:source', 'legacy capture text', 'queued', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cleaned_events (captured_event_id, source, cleaned_content, content_hash, cleaned_at_ms)
             VALUES (1, 'legacy:source', 'legacy cleaned text', 'hash', 2)",
            [],
        )
        .unwrap();

        let before = buffer.protection_health().unwrap();
        assert_eq!(before.unprotected_captured_fields, 2);
        assert_eq!(before.unprotected_cleaned_fields, 2);

        let summary = buffer.protect_legacy_content(100).unwrap();
        assert_eq!(summary.protected_captured_fields, 2);
        assert_eq!(summary.protected_cleaned_fields, 2);

        let after = buffer.protection_health().unwrap();
        assert_eq!(after.unprotected_captured_fields, 0);
        assert_eq!(after.unprotected_cleaned_fields, 0);
        assert_eq!(buffer.list_recent(10).unwrap()[0].source, "legacy:source");
        assert_eq!(
            buffer.list_cleaned_recent(10).unwrap()[0].cleaned_content,
            "legacy cleaned text"
        );

        drop(conn);
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
    fn completion_requires_a_processing_capture() {
        let root = std::env::temp_dir().join(format!(
            "identity-processing-state-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let buffer = TransitBuffer::open(&paths).unwrap();
        let captured_id = buffer.ingest_text("test:state", "raw text").unwrap();
        let result = buffer.complete_processing_with_cleaned(captured_id, "test:state", "clean");

        assert!(result.is_err());
        assert!(buffer.list_cleaned_recent(10).unwrap().is_empty());
        assert_eq!(buffer.list_recent(10).unwrap()[0].status, "queued");

        drop(buffer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_completion_rejects_without_touching_promoted_cleaned_row() {
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

        let duplicate =
            buffer.complete_processing_with_cleaned(captured_id, "test:upsert", "second clean");
        assert!(duplicate.is_err());

        let cleaned = buffer.list_cleaned_recent(10).unwrap();
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0].id, cleaned_id);
        assert_eq!(cleaned[0].cleaned_content, "first clean");
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
