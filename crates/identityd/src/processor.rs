use crate::identity::{IdentityError, IdentityStore};
use crate::idle::{is_idle_for, IdleError};
use crate::transit::{TransitBuffer, TransitError};
use crate::workspace::IdentityPaths;
use std::fmt;
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug)]
pub struct ProcessSummary {
    pub claimed: usize,
    pub processed: usize,
    pub failed: usize,
    pub skipped_idle_gate: bool,
}

#[derive(Debug)]
pub struct PromoteSummary {
    pub claimed: usize,
    pub promoted: usize,
    pub failed: usize,
    pub redacted: usize,
}

#[derive(Debug)]
pub struct PipelineSummary {
    pub processed: ProcessSummary,
    pub promoted: PromoteSummary,
}

#[derive(Debug)]
pub enum ProcessError {
    Idle(IdleError),
    Identity(IdentityError),
    Transit(TransitError),
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle(error) => write!(f, "{error}"),
            Self::Identity(error) => write!(f, "{error}"),
            Self::Transit(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProcessError {}

impl From<TransitError> for ProcessError {
    fn from(value: TransitError) -> Self {
        Self::Transit(value)
    }
}

impl From<IdleError> for ProcessError {
    fn from(value: IdleError) -> Self {
        Self::Idle(value)
    }
}

impl From<IdentityError> for ProcessError {
    fn from(value: IdentityError) -> Self {
        Self::Identity(value)
    }
}

pub fn process_once(paths: &IdentityPaths, limit: u32) -> Result<ProcessSummary, ProcessError> {
    let buffer = TransitBuffer::open(paths)?;
    let events = buffer.claim_queued(limit)?;
    let mut processed = 0;
    let mut failed = 0;

    for event in &events {
        match clean_for_next_stage(&event.content) {
            Some(cleaned) => {
                buffer.complete_processing_with_cleaned(event.id, &event.source, &cleaned)?;
                processed += 1;
            }
            None => {
                buffer.mark_failed(event.id, "empty content after local cleaning")?;
                failed += 1;
            }
        }
    }

    Ok(ProcessSummary {
        claimed: events.len(),
        processed,
        failed,
        skipped_idle_gate: false,
    })
}

pub fn process_once_if_idle(
    paths: &IdentityPaths,
    limit: u32,
    min_idle_ms: u64,
) -> Result<ProcessSummary, ProcessError> {
    if !is_idle_for(Duration::from_millis(min_idle_ms))? {
        return Ok(ProcessSummary {
            claimed: 0,
            processed: 0,
            failed: 0,
            skipped_idle_gate: true,
        });
    }

    process_once(paths, limit)
}

pub fn promote_once(paths: &IdentityPaths, limit: u32) -> Result<PromoteSummary, ProcessError> {
    let transit = TransitBuffer::open(paths)?;
    let identity = IdentityStore::open(paths)?;
    let cleaned_events = transit.list_cleaned_pending(limit)?;
    let mut promoted = 0;
    let mut failed = 0;

    for cleaned in &cleaned_events {
        match identity.insert_memory_from_cleaned(cleaned) {
            Ok(_) => {
                transit.mark_cleaned_promoted(cleaned.id)?;
                promoted += 1;
            }
            Err(error) => {
                eprintln!("failed to promote cleaned event #{}: {error}", cleaned.id);
                failed += 1;
            }
        }
    }

    let redaction = transit.redact_promoted_content(limit)?;

    Ok(PromoteSummary {
        claimed: cleaned_events.len(),
        promoted,
        failed,
        redacted: redaction
            .redacted_captured_events
            .max(redaction.redacted_cleaned_events),
    })
}

pub fn pipeline_once_if_idle(
    paths: &IdentityPaths,
    process_limit: u32,
    promote_limit: u32,
    min_idle_ms: u64,
) -> Result<PipelineSummary, ProcessError> {
    let processed = process_once_if_idle(paths, process_limit, min_idle_ms)?;
    let promoted = if processed.skipped_idle_gate {
        PromoteSummary {
            claimed: 0,
            promoted: 0,
            failed: 0,
            redacted: 0,
        }
    } else {
        promote_once(paths, promote_limit)?
    };

    Ok(PipelineSummary {
        processed,
        promoted,
    })
}

#[inline]
fn clean_for_next_stage(content: &str) -> Option<String> {
    let normalized: String = content
        .chars()
        .map(|c| if c.is_control() && c != '\n' && c != '\t' { ' ' } else { c })
        .collect();
    let cleaned: String = normalized.nfkc().collect::<String>();
    let compact = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");

    if compact.is_empty() {
        None
    } else {
        Some(compact)
    }
}

#[cfg(test)]
mod tests {
    use super::{clean_for_next_stage, pipeline_once_if_idle, process_once, promote_once};
    use crate::identity::IdentityStore;
    use crate::transit::TransitBuffer;
    use crate::workspace::IdentityPaths;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn cleaner_discards_empty_content() {
        assert_eq!(clean_for_next_stage(" \n\t "), None);
        assert_eq!(
            clean_for_next_stage("Identity\nkeeps\tlocal context"),
            Some("Identity keeps local context".to_string())
        );
    }

    #[test]
    fn process_and_promote_create_identity_memory() {
        let root = std::env::temp_dir().join(format!(
            "identity-promote-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let transit = TransitBuffer::open(&paths).unwrap();
        transit
            .ingest_text("test:promote", "Local memory promotion works.")
            .unwrap();

        let process = process_once(&paths, 1).unwrap();
        assert_eq!(process.processed, 1);

        let promote = promote_once(&paths, 1).unwrap();
        assert_eq!(promote.promoted, 1);
        assert_eq!(promote.redacted, 1);

        let identity = IdentityStore::open(&paths).unwrap();
        let memories = identity.list_recent(10).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].summary, "Local memory promotion works.");
        let captured = transit.list_recent(10).unwrap();
        let cleaned = transit.list_cleaned_recent(10).unwrap();
        assert_eq!(captured[0].content, "");
        assert_eq!(cleaned[0].cleaned_content, "");

        drop(identity);
        drop(transit);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pipeline_once_processes_and_promotes_when_idle_gate_passes() {
        let root = std::env::temp_dir().join(format!(
            "identity-pipeline-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = IdentityPaths::from_root(root.clone());
        paths.ensure().unwrap();

        let transit = TransitBuffer::open(&paths).unwrap();
        transit
            .ingest_text("test:pipeline", "Pipeline writes local memory.")
            .unwrap();

        let summary = pipeline_once_if_idle(&paths, 10, 10, 0).unwrap();
        assert_eq!(summary.processed.processed, 1);
        assert_eq!(summary.promoted.promoted, 1);
        assert_eq!(summary.promoted.redacted, 1);

        let identity = IdentityStore::open(&paths).unwrap();
        let memories = identity.list_recent(10).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].summary, "Pipeline writes local memory.");

        drop(identity);
        drop(transit);
        fs::remove_dir_all(root).unwrap();
    }
}
