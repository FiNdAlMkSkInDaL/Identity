use identityd::activity::{
    capture_active_window_once, watch_active_window_until_shutdown, DEFAULT_ACTIVITY_POLL_MS,
};
use identityd::bootstrap::{bootstrap_onnx_artifact, print_bootstrap_guidance};
use identityd::browser_capture::{
    bookmarklet as browser_capture_bookmarklet,
    clipboard_bookmarklet as browser_capture_clipboard_bookmarklet, format_page_capture,
    page_capture_from_clipboard_text, post_page_capture, PageCaptureInput,
};
use identityd::capture::capture_adapter_health;
use identityd::context_builder::build_identity_context;
use identityd::context_snapshot::{capture_context_snapshot, ContextSnapshot};
use identityd::crypto::protection_backend;
use identityd::delta::{agent_delta_from_json, extract_agent_delta, normalize_agent_delta_source};
use identityd::embedding::{
    active_embedding_health, embedding_artifact_health_for_model_path,
    onnx_runtime_health_for_artifact, probe_embedding_latency, run_onnx_embedding_file,
    tokenize_wordpiece_file, tokenizer_health, tokenizer_health_for_vocab_path,
    write_embedding_manifest, ActiveEmbeddingHealth, OnnxRuntimeHealth, TokenizerHealth,
    EMBEDDING_DIM, EMBEDDING_LATENCY_TARGET_MS, EMBEDDING_ONNX_MODEL_PATH_ENV,
    EMBEDDING_RUNTIME_ENV, EMBEDDING_TOKENIZER_DEFAULT_MAX_TOKENS,
    EMBEDDING_TOKENIZER_VOCAB_PATH_ENV, ORT_WAS_LOADED,
};
use identityd::filesystem::{
    ensure_safe_watch_root, FileWatcher, FileWatcherConfig, FileWatcherMode, WATCH_UNSAFE_ROOT_FLAG,
};
use identityd::identity::IdentityStore;
use identityd::processor::{
    pipeline_once_if_idle, process_capture, process_once, process_once_if_idle, promote_capture,
    promote_once,
};
use identityd::project_profile::{find_matching_profile, find_profile_by_name, load_profiles};
use identityd::proxy::LocalCaptureServer;
use identityd::resource::{
    current_process_resources, memory_budget_status, IDLE_MEMORY_TARGET_BYTES,
};
use identityd::slice::{build_prompt_package, generate_meslice};
use identityd::transit::{TransitBuffer, TransitSourceFamilyCounts, DEFAULT_PROCESSING_LEASE_MS};
use identityd::workspace::IdentityPaths;
use std::env;
use std::error::Error;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;
use tokio::signal;
use tokio::time::{sleep, Duration};

const BINARY_SIZE_TARGET_BYTES: u64 = 15 * 1024 * 1024;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        log_error(&format!("identityd error: {error}"));
        std::process::exit(1);
    }
    // If the ONNX runtime was loaded into this process during the command, bypass
    // the normal Rust/C++ cleanup path. The Snapdragon X Elite cpuinfo library has
    // a TLS destructor that fires an access-violation (0xc0000005) during process
    // shutdown after an ORT session is created. All output and DB writes are already
    // complete at this point, so exit(0) is semantically correct.
    if ORT_WAS_LOADED.load(std::sync::atomic::Ordering::SeqCst) {
        std::process::exit(0);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let startup_started = Instant::now();
    let raw_args: Vec<String> = env::args().skip(1).collect();
    let command = command_arg(&raw_args).unwrap_or_else(|| "init".to_string());

    let paths = if let Some(root) = read_flag(&raw_args, "--root") {
        IdentityPaths::from_root(PathBuf::from(root))
    } else {
        IdentityPaths::from_default_home()?
    };
    paths.ensure()?;
    let workspace_ready_ms = startup_started.elapsed().as_millis();

    match command.as_str() {
        "init" => {
            let _buffer = TransitBuffer::open(&paths)?;
            println!("initialized Identity workspace at {}", paths.root.display());
            println!("identity ledger: {}", paths.identity_dir.display());
            println!("vector store root: {}", paths.vector_store_dir.display());
            println!("transit buffer: {}", paths.transit_db.display());
            println!("capture token: {}", paths.capture_token.display());
        }
        "ingest" => {
            let source = read_flag(&raw_args, "--source").unwrap_or_else(|| "manual".to_string());
            let content = read_flag(&raw_args, "--content")
                .ok_or("missing required --content value for ingest command")?;

            let buffer = TransitBuffer::open(&paths)?;
            let id = buffer.ingest_text(&source, &content)?;

            println!("queued captured event #{id} from {source}");
        }
        "capture-active-window" => {
            let id = capture_active_window_once(&paths)?;
            println!("queued active window capture #{id}");
        }
        "capture-page" => {
            let addr = read_flag(&raw_args, "--addr")
                .unwrap_or_else(|| "127.0.0.1:8080".to_string())
                .parse::<SocketAddr>()?;
            ensure_loopback_addr(addr, false)?;
            let clipboard_input = if has_flag(&raw_args, "--from-clipboard") {
                Some(page_capture_from_clipboard_text(
                    &identityd::clipboard::get_clipboard_text()?,
                )?)
            } else {
                None
            };
            let selected_text = if let Some(input) = clipboard_input.as_ref() {
                input.selected_text.clone()
            } else {
                read_page_capture_text(&raw_args)?
            };
            let input = PageCaptureInput {
                title: read_flag(&raw_args, "--title").or_else(|| {
                    clipboard_input
                        .as_ref()
                        .and_then(|input| input.title.clone())
                }),
                url: read_flag(&raw_args, "--url")
                    .or_else(|| clipboard_input.as_ref().and_then(|input| input.url.clone())),
                selected_text,
            };

            if has_flag(&raw_args, "--dry-run") {
                let body = format_page_capture(&input)?;
                println!("{body}");
            } else {
                let result = post_page_capture(&paths, addr, &input).await?;
                let mut promotion = None;
                if has_flag(&raw_args, "--promote-now") {
                    let captured_id = result
                        .captured_id
                        .ok_or("capture endpoint did not return captured id for --promote-now")?;
                    let processed = process_capture(&paths, captured_id)?;
                    let promoted = promote_capture(&paths, captured_id)?;
                    promotion = Some((processed, promoted));
                }
                println!(
                    "queued browser page capture via loopback: status={} bytes_sent={} captured_id={} response={}",
                    result.status_code,
                    result.bytes_sent,
                    result
                        .captured_id
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    result.body
                );
                if let Some((processed, promoted)) = promotion {
                    println!(
                        "immediate page promotion: processed={} promoted={} redacted={} failed={}",
                        processed.processed,
                        promoted.promoted,
                        promoted.redacted,
                        processed.failed + promoted.failed
                    );
                }
            }
        }
        "browser-capture-bookmarklet" => {
            let addr = read_flag(&raw_args, "--addr")
                .unwrap_or_else(|| "127.0.0.1:8080".to_string())
                .parse::<SocketAddr>()?;
            ensure_loopback_addr(addr, false)?;
            println!("{}", browser_capture_bookmarklet(addr)?);
        }
        "browser-capture-clipboard-bookmarklet" => {
            println!("{}", browser_capture_clipboard_bookmarklet());
        }
        "watch-active-window" => {
            let interval_ms = read_flag(&raw_args, "--interval-ms")
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or(DEFAULT_ACTIVITY_POLL_MS);

            println!("press Ctrl+C to stop active window watching");
            run_active_window_watch(paths, interval_ms).await?;
        }
        "list" => {
            let buffer = TransitBuffer::open(&paths)?;
            let events = buffer.list_recent(10)?;

            if events.is_empty() {
                println!("no captured events queued");
            }

            for event in events {
                println!(
                    "#{id} [{status}] retry={retry_count} {source} @ {captured_at_ms}: {content}{error}",
                    id = event.id,
                    status = event.status,
                    retry_count = event.retry_count,
                    source = event.source,
                    captured_at_ms = event.captured_at_ms,
                    content = event.content.replace('\n', " "),
                    error = event
                        .error
                        .as_ref()
                        .map(|value| format!(" error={value}"))
                        .unwrap_or_default()
                );
            }
        }
        "stats" => {
            let buffer = TransitBuffer::open(&paths)?;
            let counts = buffer.status_counts()?;

            if counts.is_empty() {
                println!("no transit events recorded");
            }

            for count in counts {
                println!("{}={}", count.status, count.count);
            }
        }
        "capture-sources" => {
            let buffer = TransitBuffer::open(&paths)?;
            let sources = buffer.source_family_counts()?;
            print_source_family_counts(&sources);
        }
        "doctor" => {
            let lease_ms = read_flag(&raw_args, "--lease-ms")
                .map(|value| value.parse::<i64>())
                .transpose()?
                .unwrap_or(DEFAULT_PROCESSING_LEASE_MS);
            let buffer = TransitBuffer::open(&paths)?;
            let transit = buffer.health(lease_ms)?;
            let transit_sources = buffer.source_family_counts()?;
            let transit_protection = buffer.protection_health()?;
            let transit_probe = buffer.probe_insert_rollback_latency()?;
            let store = IdentityStore::open(&paths)?;
            let memory = store.stats()?;
            let embedding_runtime = store.embedding_runtime_info();
            let embedding_artifact = store.embedding_artifact_health();
            let onnx_runtime = onnx_runtime_health_for_artifact(&embedding_artifact);
            let tokenizer = tokenizer_health();
            let active_embedding = active_embedding_health();
            let protocol = store.protocol_schema_health()?;
            let vector_mirror = store.vector_mirror_health()?;
            let memory_protection = store.protection_health()?;
            let capture_health = capture_adapter_health(&paths);
            let embedding_probe = probe_embedding_latency("Identity local embedding budget probe.");
            let resources = current_process_resources();
            let working_set_bytes = resources.as_ref().map(|probe| probe.working_set_bytes);
            let pagefile_bytes = resources.as_ref().map(|probe| probe.pagefile_bytes);
            let binary_size_bytes = current_binary_size_bytes();
            let content_protection_ready = transit_protection.unprotected_captured_fields == 0
                && transit_protection.unprotected_cleaned_fields == 0
                && memory_protection.unprotected_semantic_fields == 0;
            let transit_ready = transit.stale_processing == 0 && transit.failed == 0;
            let memory_vector_ready = memory.invalid_vector_count == 0 && vector_mirror.is_ready();
            let memory_schema_ready = memory.node_count == memory.node_uid_count
                && memory.node_count == memory.timestamp_utc_count
                && memory.node_count == memory.last_accessed_count;
            let protocol_schema_ready = protocol.is_ready();
            let embedding_artifact_ready = embedding_artifact.is_ready();
            let local_pipeline_status = phase1_local_pipeline_status(
                transit.stale_processing,
                transit.failed,
                memory.invalid_vector_count,
                transit_probe.insert_rollback_ms,
            );
            let resource_budget_ready = embedding_probe.latency_ms <= EMBEDDING_LATENCY_TARGET_MS
                && transit_probe.insert_rollback_ms <= 1
                && memory_budget_status(working_set_bytes) == "within-budget"
                && binary_size_bytes
                    .map(|bytes| bytes <= BINARY_SIZE_TARGET_BYTES)
                    .unwrap_or(false);
            let onnx_session_ready = onnx_runtime.session_status == "ready";
            let vector_store_ready = memory.vector_store_backend == "filesystem+sqlite"
                || memory.vector_store_backend.contains("lancedb");
            let accessibility_ready = capture_health.phase1_status == "ready";

            let phase1_ready_markers = count_ready([
                true, // Workspace initialization
                true, // Local SQLite transit queue
                content_protection_ready,
                transit_ready,
                memory_vector_ready,
                memory_schema_ready,
                protocol_schema_ready,
                embedding_artifact_ready,
                local_pipeline_status == "ready",
                resource_budget_ready,
                onnx_session_ready,
                vector_store_ready,
                accessibility_ready,
            ]);
            let phase1_partial_markers = count_ready([
                !onnx_session_ready,
                !vector_store_ready,
                !accessibility_ready,
                !embedding_artifact_ready,
            ]);
            let phase1_total_markers = 13;
            let phase1_completion_percent = completion_percent(
                phase1_ready_markers,
                phase1_partial_markers,
                phase1_total_markers,
            );

            println!("workspace_root={}", paths.root.display());
            println!("identity_ledger={}", paths.identity_dir.display());
            println!("vector_store_root={}", paths.vector_store_dir.display());
            println!("transit_db={}", paths.transit_db.display());
            println!("transit_queued={}", transit.queued);
            println!("transit_processing={}", transit.processing);
            println!("transit_stale_processing={}", transit.stale_processing);
            println!("transit_processed={}", transit.processed);
            println!("transit_failed={}", transit.failed);
            println!("memory_nodes={}", memory.node_count);
            println!("memory_node_uids={}", memory.node_uid_count);
            println!("memory_created_at_utc={}", memory.timestamp_utc_count);
            println!("memory_last_accessed={}", memory.last_accessed_count);
            println!("memory_vectorized_nodes={}", memory.vectorized_count);
            println!("memory_invalid_vectors={}", memory.invalid_vector_count);
            println!(
                "vector_primary_mirrored={}",
                vector_mirror.primary_mirrored_count
            );
            println!(
                "vector_primary_missing={}",
                vector_mirror.primary_missing_count
            );
            println!("protocol_nodes={}", protocol.node_count);
            println!("protocol_valid_node_ids={}", protocol.valid_node_ids);
            println!("protocol_valid_timestamps={}", protocol.valid_timestamps);
            println!(
                "protocol_valid_structured_attributes={}",
                protocol.valid_structured_attributes
            );
            println!(
                "protocol_valid_vector_dimensions={}",
                protocol.valid_vector_dimensions
            );
            println!("embedding_model_id={}", memory.embedding_model_id);
            println!("embedding_dim={}", memory.embedding_dim);
            println!("embedding_runtime_kind={}", embedding_runtime.runtime_kind);
            println!(
                "embedding_runtime_status={}",
                embedding_runtime.runtime_status
            );
            println!("embedding_acceleration={}", embedding_runtime.acceleration);
            println!("embedding_quantization={}", embedding_runtime.quantization);
            println!(
                "embedding_onnx_model_path_configured={}",
                embedding_runtime.onnx_model_path_configured
            );
            println!("embedding_artifact_status={}", embedding_artifact.status);
            println!(
                "embedding_artifact_path={}",
                optional_string(embedding_artifact.path.as_deref())
            );
            println!("embedding_artifact_exists={}", embedding_artifact.exists);
            println!("embedding_artifact_is_file={}", embedding_artifact.is_file);
            println!(
                "embedding_artifact_has_onnx_extension={}",
                embedding_artifact.has_onnx_extension
            );
            println!(
                "embedding_artifact_size_bytes={}",
                optional_u64(embedding_artifact.size_bytes)
            );
            println!(
                "embedding_artifact_manifest_path={}",
                optional_string(embedding_artifact.manifest_path.as_deref())
            );
            println!(
                "embedding_artifact_manifest_exists={}",
                embedding_artifact.manifest_exists
            );
            println!(
                "embedding_artifact_manifest_size_bytes={}",
                optional_u64(embedding_artifact.manifest_size_bytes)
            );
            println!(
                "embedding_artifact_manifest_model_id={}",
                optional_string(embedding_artifact.manifest_model_id.as_deref())
            );
            println!(
                "embedding_artifact_manifest_embedding_dim={}",
                optional_usize(embedding_artifact.manifest_embedding_dim)
            );
            print_onnx_runtime_health(&onnx_runtime);
            print_tokenizer_health(&tokenizer);
            print_active_embedding_health(&active_embedding);
            println!("embedding_probe_model_id={}", embedding_probe.model_id);
            println!("embedding_probe_dim={}", embedding_probe.dimension);
            println!("embedding_probe_ms={}", embedding_probe.latency_ms);
            println!(
                "embedding_target_ms={EMBEDDING_LATENCY_TARGET_MS} within_budget={}",
                embedding_probe.latency_ms <= EMBEDDING_LATENCY_TARGET_MS
            );
            println!("vector_store_backend={}", memory.vector_store_backend);
            let graph = store.graph_health()?;
            println!("graph_agent_delta_nodes={}", graph.agent_delta_nodes);
            println!("graph_edges={}", graph.edge_count);
            println!("graph_orphans={}", graph.orphan_count);
            println!("graph_outcome_edges={}", graph.outcome_edges);
            println!("graph_conflict_edges={}", graph.conflict_edges);
            println!("graph_supersession_edges={}", graph.supersession_edges);
            println!("graph_decayed_edges={}", graph.decayed_edges);
            println!(
                "content_unprotected_transit_fields={}",
                transit_protection.unprotected_captured_fields
                    + transit_protection.unprotected_cleaned_fields
            );
            println!(
                "content_unprotected_memory_fields={}",
                memory_protection.unprotected_semantic_fields
            );
            println!("startup_workspace_ready_ms={workspace_ready_ms}");
            println!(
                "transit_insert_rollback_probe_ms={}",
                transit_probe.insert_rollback_ms
            );
            println!(
                "transit_insert_target_ms=1 within_budget={}",
                transit_probe.insert_rollback_ms <= 1
            );
            println!(
                "resource_working_set_bytes={}",
                optional_u64(working_set_bytes)
            );
            println!("resource_pagefile_bytes={}", optional_u64(pagefile_bytes));
            println!("resource_idle_memory_target_bytes={IDLE_MEMORY_TARGET_BYTES}");
            println!(
                "resource_idle_memory_status={}",
                memory_budget_status(working_set_bytes)
            );
            println!(
                "resource_binary_size_bytes={}",
                optional_u64(binary_size_bytes)
            );
            println!(
                "resource_binary_target_bytes={BINARY_SIZE_TARGET_BYTES} within_budget={}",
                binary_size_bytes
                    .map(|bytes| bytes <= BINARY_SIZE_TARGET_BYTES)
                    .unwrap_or(false)
            );
            println!("phase1_workspace=ready");
            println!("phase1_transit_buffer=ready");
            println!("capture_manual_adapter={}", capture_health.manual_adapter);
            println!(
                "capture_loopback_adapter={}",
                capture_health.loopback_adapter
            );
            println!(
                "capture_loopback_token_exists={}",
                capture_health.loopback_token_exists
            );
            println!(
                "capture_filesystem_adapter={}",
                capture_health.filesystem_adapter
            );
            println!(
                "capture_active_window_adapter={}",
                capture_health.active_window_adapter
            );
            print_source_family_counts(&transit_sources);
            println!(
                "filesystem_watch_safe_root_enforced={}",
                capture_health.filesystem_policy.safe_root_enforced
            );
            println!(
                "filesystem_watch_unsafe_override_flag={}",
                capture_health.filesystem_policy.unsafe_override_flag
            );
            println!(
                "filesystem_watch_native_watcher={}",
                capture_health.filesystem_policy.native_watcher
            );
            println!(
                "filesystem_watch_poll_fallback={}",
                capture_health.filesystem_policy.poll_fallback
            );
            println!(
                "filesystem_watch_max_text_file_bytes={}",
                capture_health.filesystem_policy.max_text_file_bytes
            );
            println!(
                "filesystem_watch_blocked_segments={}",
                capture_health.filesystem_policy.blocked_segments.join(",")
            );
            println!("phase1_content_protection={}", protection_backend());
            println!(
                "phase1_content_protection_health={}",
                if content_protection_ready {
                    "ready"
                } else {
                    "needs-repair"
                }
            );
            println!("phase1_capture_adapters={}", capture_health.phase1_status);
            println!(
                "phase1_transit_health={}",
                if transit_ready {
                    "ready"
                } else {
                    "needs-repair"
                }
            );
            println!(
                "phase1_memory_vector_health={}",
                if memory_vector_ready {
                    "ready"
                } else {
                    "needs-repair"
                }
            );
            println!(
                "phase1_memory_schema_health={}",
                if memory_schema_ready {
                    "ready"
                } else {
                    "needs-repair"
                }
            );
            println!(
                "phase1_protocol_schema_health={}",
                if protocol_schema_ready {
                    "ready"
                } else {
                    "needs-repair"
                }
            );
            println!(
                "phase1_embedding_runtime={}",
                active_embedding.active_runtime
            );
            println!(
                "phase1_embedding_artifact={}",
                phase1_embedding_artifact_status(
                    embedding_artifact_ready,
                    embedding_artifact.status
                )
            );
            println!(
                "phase1_vector_store_backend={}",
                memory.vector_store_backend
            );
            println!("phase1_local_pipeline={}", local_pipeline_status);
            println!("phase1_ready_markers={phase1_ready_markers}");
            println!("phase1_partial_markers={phase1_partial_markers}");
            println!("phase1_total_markers={phase1_total_markers}");
            println!("phase1_foundation_completion_percent={phase1_completion_percent}");
            println!(
                "phase1_next_milestone={}",
                phase1_next_milestone(embedding_artifact_ready, onnx_session_ready)
            );
            println!(
                "phase1_remaining={}",
                phase1_remaining_summary(
                    embedding_artifact_ready,
                    onnx_session_ready,
                    vector_store_ready,
                    accessibility_ready
                )
            );
            // Bypass tokio runtime shutdown when ONNX is loaded: the cpuinfo TLS destructor
            // (Snapdragon X Elite) fires an access-violation during C++ thread-local cleanup.
            // All output and SQLite writes are complete at this point.
            if ORT_WAS_LOADED.load(std::sync::atomic::Ordering::SeqCst) {
                std::process::exit(0);
            }
        }
        "repair-transit" => {
            let lease_ms = read_flag(&raw_args, "--lease-ms")
                .map(|value| value.parse::<i64>())
                .transpose()?
                .unwrap_or(DEFAULT_PROCESSING_LEASE_MS);
            let buffer = TransitBuffer::open(&paths)?;
            let summary = buffer.repair_stale_processing(lease_ms)?;

            println!(
                "repaired transit buffer: stale_processing_requeued={}",
                summary.stale_processing_requeued
            );
        }
        "protect-at-rest" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(100);
            let buffer = TransitBuffer::open(&paths)?;
            let transit = buffer.protect_legacy_content(limit)?;
            let store = IdentityStore::open(&paths)?;
            let memory = store.protect_legacy_semantic_text(limit)?;

            println!(
                "protected legacy content: captured_fields={} cleaned_fields={} memory_semantic_fields={}",
                transit.protected_captured_fields,
                transit.protected_cleaned_fields,
                memory.protected_semantic_fields
            );
        }
        "redact-transit-content" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(100);
            let buffer = TransitBuffer::open(&paths)?;
            let summary = buffer.redact_promoted_content(limit)?;

            println!(
                "redacted transit content: captured_events={captured} cleaned_events={cleaned}",
                captured = summary.redacted_captured_events,
                cleaned = summary.redacted_cleaned_events
            );
        }
        "cleaned-list" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let buffer = TransitBuffer::open(&paths)?;
            let events = buffer.list_cleaned_recent(limit)?;

            if events.is_empty() {
                println!("no cleaned events staged");
            }

            for event in events {
                println!(
                    "#{id} capture=#{capture_id} {source} hash={hash} @ {cleaned_at_ms}: {content}",
                    id = event.id,
                    capture_id = event.captured_event_id,
                    source = event.source,
                    hash = event.content_hash,
                    cleaned_at_ms = event.cleaned_at_ms,
                    content = event.cleaned_content.replace('\n', " ")
                );
            }
        }
        "memory-list" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let store = IdentityStore::open(&paths)?;
            let memories = store.list_recent(limit)?;

            if memories.is_empty() {
                println!("no memory nodes stored");
            }

            for memory in memories {
                println!(
                    "#{id} uid={node_uid} cleaned=#{cleaned_id} {domain}/{entity} {source} hash={hash} created={created_at_utc} accessed={last_accessed_utc}: {summary}",
                    id = memory.id,
                    node_uid = memory.node_uid,
                    cleaned_id = memory.cleaned_event_id,
                    domain = memory.domain_context,
                    entity = memory.entity_type,
                    source = memory.source,
                    hash = memory.content_hash,
                    created_at_utc = memory.created_at_utc,
                    last_accessed_utc = memory.last_accessed_utc,
                    summary = memory.summary.replace('\n', " ")
                );
            }
        }
        "memory-stats" => {
            let store = IdentityStore::open(&paths)?;
            let stats = store.stats()?;
            let embedding_runtime = store.embedding_runtime_info();
            let embedding_artifact = store.embedding_artifact_health();
            let vector_mirror = store.vector_mirror_health()?;

            println!("memory_nodes={}", stats.node_count);
            println!("memory_node_uids={}", stats.node_uid_count);
            println!("memory_created_at_utc={}", stats.timestamp_utc_count);
            println!("memory_last_accessed={}", stats.last_accessed_count);
            println!("vectorized_nodes={}", stats.vectorized_count);
            println!("invalid_vectors={}", stats.invalid_vector_count);
            println!(
                "vector_primary_mirrored={}",
                vector_mirror.primary_mirrored_count
            );
            println!(
                "vector_primary_missing={}",
                vector_mirror.primary_missing_count
            );
            println!("embedding_model_id={}", stats.embedding_model_id);
            println!("embedding_dim={}", stats.embedding_dim);
            println!("embedding_runtime_kind={}", embedding_runtime.runtime_kind);
            println!(
                "embedding_runtime_status={}",
                embedding_runtime.runtime_status
            );
            println!("embedding_acceleration={}", embedding_runtime.acceleration);
            println!("embedding_quantization={}", embedding_runtime.quantization);
            println!(
                "embedding_onnx_model_path_configured={}",
                embedding_runtime.onnx_model_path_configured
            );
            println!("embedding_artifact_status={}", embedding_artifact.status);
            println!(
                "embedding_artifact_path={}",
                optional_string(embedding_artifact.path.as_deref())
            );
            println!(
                "embedding_artifact_size_bytes={}",
                optional_u64(embedding_artifact.size_bytes)
            );
            println!(
                "embedding_artifact_manifest_model_id={}",
                optional_string(embedding_artifact.manifest_model_id.as_deref())
            );
            println!(
                "embedding_artifact_manifest_embedding_dim={}",
                optional_usize(embedding_artifact.manifest_embedding_dim)
            );
            println!("vector_store_backend={}", stats.vector_store_backend);
        }
        "embedding-runtime-health" => {
            let store = IdentityStore::open(&paths)?;
            let runtime = store.embedding_runtime_info();
            let artifact = store.embedding_artifact_health();
            let onnx_runtime = onnx_runtime_health_for_artifact(&artifact);
            let active_embedding = active_embedding_health();

            println!("embedding_model_id={}", runtime.model_id);
            println!("embedding_dim={}", runtime.dimension);
            println!("embedding_runtime_kind={}", runtime.runtime_kind);
            println!("embedding_runtime_status={}", runtime.runtime_status);
            println!("embedding_acceleration={}", runtime.acceleration);
            println!("embedding_quantization={}", runtime.quantization);
            println!(
                "embedding_onnx_model_path_configured={}",
                runtime.onnx_model_path_configured
            );
            println!("embedding_artifact_env={}", artifact.env_var);
            println!("embedding_artifact_status={}", artifact.status);
            println!(
                "embedding_artifact_path={}",
                optional_string(artifact.path.as_deref())
            );
            println!("embedding_artifact_exists={}", artifact.exists);
            println!("embedding_artifact_is_file={}", artifact.is_file);
            println!(
                "embedding_artifact_has_onnx_extension={}",
                artifact.has_onnx_extension
            );
            println!(
                "embedding_artifact_size_bytes={}",
                optional_u64(artifact.size_bytes)
            );
            println!(
                "embedding_artifact_manifest_path={}",
                optional_string(artifact.manifest_path.as_deref())
            );
            println!(
                "embedding_artifact_manifest_exists={}",
                artifact.manifest_exists
            );
            println!(
                "embedding_artifact_manifest_size_bytes={}",
                optional_u64(artifact.manifest_size_bytes)
            );
            println!(
                "embedding_artifact_manifest_model_id={}",
                optional_string(artifact.manifest_model_id.as_deref())
            );
            println!(
                "embedding_artifact_manifest_embedding_dim={}",
                optional_usize(artifact.manifest_embedding_dim)
            );
            print_onnx_runtime_health(&onnx_runtime);
            print_active_embedding_health(&active_embedding);
        }
        "embedding-active-health" => {
            let health = active_embedding_health();

            print_active_embedding_health(&health);
        }
        "onnx-runtime-health" => {
            let store = IdentityStore::open(&paths)?;
            let artifact = store.embedding_artifact_health();
            let onnx_runtime = onnx_runtime_health_for_artifact(&artifact);

            println!("embedding_artifact_status={}", artifact.status);
            println!(
                "embedding_artifact_path={}",
                optional_string(artifact.path.as_deref())
            );
            print_onnx_runtime_health(&onnx_runtime);
            if ORT_WAS_LOADED.load(std::sync::atomic::Ordering::SeqCst) {
                std::process::exit(0);
            }
        }
        "embedding-tokenizer-health" => {
            let health = if let Some(vocab_path) = read_flag(&raw_args, "--vocab-path") {
                tokenizer_health_for_vocab_path(&PathBuf::from(vocab_path))
            } else {
                tokenizer_health()
            };

            print_tokenizer_health(&health);
        }
        "embedding-tokenize" => {
            let vocab_path = read_flag(&raw_args, "--vocab-path")
                .map(PathBuf::from)
                .or_else(|| env::var_os(EMBEDDING_TOKENIZER_VOCAB_PATH_ENV).map(PathBuf::from))
                .ok_or(
                    "missing --vocab-path or IDENTITY_TOKENIZER_VOCAB_PATH for embedding-tokenize",
                )?;
            let text = read_flag(&raw_args, "--text")
                .ok_or("missing required --text value for embedding-tokenize")?;
            let max_tokens = read_flag(&raw_args, "--max-tokens")
                .map(|value| value.parse::<usize>())
                .transpose()?
                .unwrap_or(EMBEDDING_TOKENIZER_DEFAULT_MAX_TOKENS);
            let tokenized = tokenize_wordpiece_file(&vocab_path, &text, max_tokens)?;

            println!("tokenizer_vocab_path={}", vocab_path.display());
            println!("tokenizer_max_tokens={max_tokens}");
            println!("tokenizer_truncated={}", tokenized.truncated);
            println!("tokenizer_token_count={}", tokenized.tokens.len());
            println!("tokenizer_tokens={}", tokenized.tokens.join("|"));
            println!("tokenizer_input_ids={}", join_i64(&tokenized.input_ids));
            println!(
                "tokenizer_attention_mask={}",
                join_i64(&tokenized.attention_mask)
            );
            println!(
                "tokenizer_token_type_ids={}",
                join_i64(&tokenized.token_type_ids)
            );
        }
        "embedding-onnx-run" => {
            let model_path = read_flag(&raw_args, "--model-path")
                .map(PathBuf::from)
                .or_else(|| env::var_os(EMBEDDING_ONNX_MODEL_PATH_ENV).map(PathBuf::from))
                .ok_or(
                    "missing --model-path or IDENTITY_EMBEDDING_MODEL_PATH for embedding-onnx-run",
                )?;
            let vocab_path = read_flag(&raw_args, "--vocab-path")
                .map(PathBuf::from)
                .or_else(|| env::var_os(EMBEDDING_TOKENIZER_VOCAB_PATH_ENV).map(PathBuf::from))
                .ok_or(
                    "missing --vocab-path or IDENTITY_TOKENIZER_VOCAB_PATH for embedding-onnx-run",
                )?;
            let text = read_flag(&raw_args, "--text")
                .ok_or("missing required --text value for embedding-onnx-run")?;
            let max_tokens = read_flag(&raw_args, "--max-tokens")
                .map(|value| value.parse::<usize>())
                .transpose()?
                .unwrap_or(EMBEDDING_TOKENIZER_DEFAULT_MAX_TOKENS);
            let run = run_onnx_embedding_file(&model_path, &vocab_path, &text, max_tokens)?;

            println!("onnx_embedding_model_path={}", run.model_path);
            println!("onnx_embedding_vocab_path={}", run.vocab_path);
            println!("onnx_embedding_token_count={}", run.token_count);
            println!("onnx_embedding_truncated={}", run.truncated);
            println!("onnx_embedding_output_floats={}", run.output_floats);
            println!("onnx_embedding_pooled_rows={}", run.pooled_rows);
            println!("onnx_embedding_dim={EMBEDDING_DIM}");
            println!(
                "onnx_embedding_prefix={}",
                join_f32_prefix(&run.embedding, 8)
            );
            if ORT_WAS_LOADED.load(std::sync::atomic::Ordering::SeqCst) {
                std::process::exit(0);
            }
        }
        "embedding-manifest-write" => {
            let model_path = read_flag(&raw_args, "--model-path")
                .map(PathBuf::from)
                .ok_or("missing required --model-path value for embedding-manifest-write")?;
            let model_id = read_flag(&raw_args, "--model-id")
                .ok_or("missing required --model-id value for embedding-manifest-write")?;
            let overwrite = has_flag(&raw_args, "--force");
            let manifest_path = write_embedding_manifest(&model_path, &model_id, overwrite)?;
            let artifact = embedding_artifact_health_for_model_path(&model_path);

            println!("embedding_manifest_path={}", manifest_path.display());
            println!("embedding_manifest_model_id={model_id}");
            println!("embedding_manifest_embedding_dim={EMBEDDING_DIM}");
            println!("embedding_artifact_status={}", artifact.status);
            println!("embedding_artifact_exists={}", artifact.exists);
            println!("embedding_artifact_is_file={}", artifact.is_file);
            println!(
                "embedding_artifact_has_onnx_extension={}",
                artifact.has_onnx_extension
            );
            println!(
                "embedding_artifact_size_bytes={}",
                optional_u64(artifact.size_bytes)
            );
            println!(
                "embedding_artifact_manifest_exists={}",
                artifact.manifest_exists
            );
            println!(
                "embedding_artifact_manifest_embedding_dim={}",
                optional_usize(artifact.manifest_embedding_dim)
            );
        }
        "embedding-bootstrap" => {
            let model_dir = read_flag(&raw_args, "--model-dir")
                .map(PathBuf::from)
                .unwrap_or_else(|| paths.identity_dir.join("models"));
            let result = bootstrap_onnx_artifact(&model_dir)?;

            print_bootstrap_guidance(&result);
        }
        "memory-export" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let store = IdentityStore::open(&paths)?;
            println!("{}", store.export_recent_protocol_json(limit)?);
        }
        "memory-protocol-health" => {
            let store = IdentityStore::open(&paths)?;
            let health = store.protocol_schema_health()?;

            println!("protocol_nodes={}", health.node_count);
            println!("protocol_valid_node_ids={}", health.valid_node_ids);
            println!("protocol_valid_timestamps={}", health.valid_timestamps);
            println!(
                "protocol_valid_structured_attributes={}",
                health.valid_structured_attributes
            );
            println!(
                "protocol_valid_vector_dimensions={}",
                health.valid_vector_dimensions
            );
            println!(
                "protocol_schema_health={}",
                if health.is_ready() {
                    "ready"
                } else {
                    "needs-repair"
                }
            );
        }
        "repair-protocol-schema" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(100);
            let store = IdentityStore::open(&paths)?;
            let summary = store.repair_protocol_schema(limit)?;

            println!(
                "repaired protocol schema: node_ids={} timestamps={} structured_attributes={} vectors={}",
                summary.repaired_node_ids,
                summary.repaired_timestamps,
                summary.repaired_structured_attributes,
                summary.repaired_vectors
            );
        }
        "repair-memory-vectors" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(100);
            let store = IdentityStore::open(&paths)?;
            let summary = store.repair_vectors(limit)?;

            println!(
                "repaired memory vectors: repaired_vectors={}",
                summary.repaired_vectors
            );
        }
        "memory-search" => {
            let query = read_flag(&raw_args, "--query")
                .or_else(|| read_flag(&raw_args, "-q"))
                .ok_or("missing required --query value for memory-search command")?;
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(5);
            let store = IdentityStore::open(&paths)?;
            let results = store.search(&query, limit)?;

            if results.is_empty() {
                println!("no memory search results");
            }

            for result in results {
                let memory = result.node;
                println!(
                    "#{id} score={score} cleaned={cleaned_id} {domain}/{entity} {source}: {summary}",
                    id = memory.id,
                    score = result.score,
                    cleaned_id = memory.cleaned_event_id,
                    domain = memory.domain_context,
                    entity = memory.entity_type,
                    source = memory.source,
                    summary = memory.summary.replace('\n', " ")
                );
            }
        }
        "memory-edge-add" => {
            let source_id = read_flag(&raw_args, "--source-id")
                .ok_or("missing required --source-id value for memory-edge-add command")?
                .parse::<i64>()?;
            let target_id = read_flag(&raw_args, "--target-id")
                .ok_or("missing required --target-id value for memory-edge-add command")?
                .parse::<i64>()?;
            let relationship = read_flag(&raw_args, "--relationship")
                .ok_or("missing required --relationship value for memory-edge-add command")?;
            let weight = read_flag(&raw_args, "--weight")
                .map(|value| value.parse::<f64>())
                .transpose()?
                .unwrap_or(1.0);

            let store = IdentityStore::open(&paths)?;
            let edge = store.link_nodes(source_id, target_id, &relationship, weight)?;

            println!(
                "linked nodes: #{id} {source} -> {target} [{relationship}] weight={weight}",
                id = edge.id,
                source = edge.source_node_id,
                target = edge.target_node_id,
                relationship = edge.relationship_type,
                weight = edge.edge_weight
            );
        }
        "memory-edges-list" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);

            let store = IdentityStore::open(&paths)?;
            let edges = store.list_edges(limit)?;

            if edges.is_empty() {
                println!("no graph edges recorded");
            }

            for edge in edges {
                println!(
                    "#{id} {source} -> {target} [{relationship}] weight={weight} updated={updated_at_ms}",
                    id = edge.id,
                    source = edge.source_node_id,
                    target = edge.target_node_id,
                    relationship = edge.relationship_type,
                    weight = edge.edge_weight,
                    updated_at_ms = edge.updated_at_ms
                );
            }
        }
        "memory-edge-decay" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(100);

            let store = IdentityStore::open(&paths)?;
            let summary = store.decay_edges(limit)?;

            println!("decayed edges: edges_decayed={}", summary.edges_decayed);
        }
        "memory-graph-health" => {
            let store = IdentityStore::open(&paths)?;
            let health = store.graph_health()?;

            println!("graph_nodes={}", health.node_count);
            println!("graph_agent_delta_nodes={}", health.agent_delta_nodes);
            println!("graph_edges={}", health.edge_count);
            println!("graph_orphans={}", health.orphan_count);
            println!("graph_outcome_edges={}", health.outcome_edges);
            println!("graph_conflict_edges={}", health.conflict_edges);
            println!("graph_supersession_edges={}", health.supersession_edges);
            println!("graph_decayed_edges={}", health.decayed_edges);
        }
        "context-now" => {
            let is_preview = has_flag(&raw_args, "--preview");
            let is_copy = has_flag(&raw_args, "--copy");
            if !is_preview && !is_copy {
                return Err("context-now command requires --preview and/or --copy flag".into());
            }
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(3);

            let snapshot =
                capture_context_snapshot().unwrap_or_else(|_| ContextSnapshot::default());
            let profiles = load_profiles(&paths)?;
            let matched_profile = if let Some(project_name) = read_flag(&raw_args, "--project") {
                Some(
                    find_profile_by_name(&profiles, &project_name)
                        .ok_or_else(|| format!("project profile not found: {project_name}"))?,
                )
            } else {
                find_matching_profile(&profiles, &snapshot)
            };
            let context =
                build_identity_context(&paths, &snapshot, matched_profile.as_ref(), limit)?;

            let block = context.to_context_block();
            if is_preview {
                println!("{}", block);
            }
            if is_copy {
                identityd::clipboard::set_clipboard_text(&block)?;
                println!(
                    "copied compiled context to clipboard ({} bytes)",
                    block.len()
                );
            }
        }
        "project-profile-list" => {
            let profiles = load_profiles(&paths)?;
            if profiles.is_empty() {
                println!("no project profiles defined");
            } else {
                for profile in profiles {
                    println!(
                        "profile={} filters={} queries={} guardrails={}",
                        profile.name,
                        profile.window_filters.join(","),
                        profile.memory_query_terms.join(","),
                        profile.guardrails.len()
                    );
                }
            }
        }
        "slice-preview" => {
            let intent = read_flag(&raw_args, "--intent")
                .or_else(|| read_flag(&raw_args, "--query"))
                .ok_or("missing required --intent value for slice-preview command")?;
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(3);
            let meslice = generate_meslice(&paths, &intent, limit)?;

            println!("{}", meslice.to_context_block());
        }
        "prompt-package" => {
            let intent = read_flag(&raw_args, "--intent")
                .or_else(|| read_flag(&raw_args, "--query"))
                .ok_or("missing required --intent value for prompt-package command")?;
            let prompt = read_flag(&raw_args, "--prompt")
                .ok_or("missing required --prompt value for prompt-package command")?;
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(3);
            let package = build_prompt_package(&paths, &intent, &prompt, limit)?;

            println!("{package}");
        }
        "agent-delta-list" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let review_only = has_flag(&raw_args, "--review-only");
            let source_filter = read_flag(&raw_args, "--source")
                .map(|source| normalize_agent_delta_source(Some(&source)));
            let entity_filter = read_flag(&raw_args, "--entity");
            let state_filter = read_flag(&raw_args, "--state");
            let store = IdentityStore::open(&paths)?;

            println!(
                "{}",
                store.export_recent_agent_deltas_json_filtered(
                    limit,
                    review_only,
                    source_filter.as_deref(),
                    entity_filter.as_deref(),
                    state_filter.as_deref(),
                )?
            );
        }
        "agent-delta-preview" => {
            let delta = read_agent_delta_candidate(&raw_args)?;

            println!("{}", delta.to_json()?);
        }
        "agent-delta-commit" => {
            let json_output = has_flag(&raw_args, "--json");
            let delta = read_agent_delta_candidate(&raw_args)?;
            if delta.requires_review() && !has_flag(&raw_args, "--allow-sensitive") {
                return Err(format!(
                    "agent delta requires explicit review for categories: {}; rerun with --allow-sensitive after confirming this write-back should be stored locally",
                    delta.review_required_categories.join(",")
                )
                .into());
            }
            let cleaned = delta.to_cleaned_event()?;
            let store = IdentityStore::open(&paths)?;
            let existing_node_id = store.node_uid_for_cleaned_event(cleaned.id)?;
            let memory_id = store.insert_memory_from_cleaned(&cleaned)?;
            let write_status = if existing_node_id.is_some() {
                "existing"
            } else {
                "created"
            };
            let node_id = match existing_node_id {
                Some(node_id) => node_id,
                None => store
                    .node_uid_for_memory_id(memory_id)?
                    .ok_or("committed agent delta is missing a protocol node id")?,
            };

            if json_output {
                let output = serde_json::json!({
                    "node_id": node_id,
                    "write_status": write_status,
                    "source": delta.source,
                    "outcome_state": delta.outcome_state,
                    "requires_review": delta.requires_review(),
                    "review_required_categories": delta.review_required_categories,
                    "summary": delta.summary,
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!(
                    "committed agent delta: node_id={} write_status={} source={} outcome_state={} review_required={} summary={}",
                    node_id,
                    write_status,
                    delta.source,
                    delta.outcome_state,
                    if delta.requires_review() { "yes" } else { "no" },
                    delta.summary
                );
            }
        }
        "process-once" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let summary = process_once(&paths, limit)?;

            println!(
                "processed transit batch: claimed={claimed} processed={processed} failed={failed} skipped_idle_gate={skipped}",
                claimed = summary.claimed,
                processed = summary.processed,
                failed = summary.failed,
                skipped = summary.skipped_idle_gate
            );
        }
        "process-idle-once" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let idle_ms = read_flag(&raw_args, "--idle-ms")
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or(5000);
            let summary = process_once_if_idle(&paths, limit, idle_ms)?;

            println!(
                "idle-gated transit batch: claimed={claimed} processed={processed} failed={failed} skipped_idle_gate={skipped}",
                claimed = summary.claimed,
                processed = summary.processed,
                failed = summary.failed,
                skipped = summary.skipped_idle_gate
            );
        }
        "pipeline-once" => {
            let process_limit = read_flag(&raw_args, "--process-limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let promote_limit = read_flag(&raw_args, "--promote-limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let idle_ms = read_flag(&raw_args, "--idle-ms")
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or(5000);
            let summary = pipeline_once_if_idle(&paths, process_limit, promote_limit, idle_ms)?;

            print_pipeline_summary("pipeline cycle", &summary);
        }
        "pipeline-loop" => {
            let process_limit = read_flag(&raw_args, "--process-limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let promote_limit = read_flag(&raw_args, "--promote-limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let idle_ms = read_flag(&raw_args, "--idle-ms")
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or(5000);
            let interval_ms = read_flag(&raw_args, "--interval-ms")
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or(2000);

            println!("press Ctrl+C to stop pipeline loop");

            run_pipeline_loop(paths, process_limit, promote_limit, idle_ms, interval_ms).await?;
        }
        "promote-once" => {
            let limit = read_flag(&raw_args, "--limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let summary = promote_once(&paths, limit)?;

            println!(
                "promoted cleaned batch: claimed={claimed} promoted={promoted} failed={failed} redacted={redacted}",
                claimed = summary.claimed,
                promoted = summary.promoted,
                failed = summary.failed,
                redacted = summary.redacted
            );
        }
        "serve" => {
            let addr = read_flag(&raw_args, "--addr")
                .unwrap_or_else(|| "127.0.0.1:8080".to_string())
                .parse::<SocketAddr>()?;
            ensure_loopback_addr(addr, has_flag(&raw_args, "--allow-non-loopback"))?;
            let server = LocalCaptureServer::new(addr, paths)?;

            println!("press Ctrl+C to stop identityd");
            server.run().await?;
        }
        "watch" => {
            let watch_path = read_flag(&raw_args, "--path")
                .map(PathBuf::from)
                .ok_or("missing required --path value for watch command")?;
            let recursive = !has_flag(&raw_args, "--non-recursive");
            let mode = if has_flag(&raw_args, "--poll") {
                FileWatcherMode::PollOnly
            } else {
                FileWatcherMode::NativePreferred
            };

            if !watch_path.exists() {
                return Err(format!("watch path does not exist: {}", watch_path.display()).into());
            }
            ensure_safe_watch_root(
                &watch_path,
                &paths.root,
                has_flag(&raw_args, WATCH_UNSAFE_ROOT_FLAG),
            )
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::PermissionDenied, error))?;

            let watcher = FileWatcher::new(
                paths,
                FileWatcherConfig {
                    root: watch_path,
                    recursive,
                    mode,
                },
            );

            println!("press Ctrl+C to stop filesystem watching");
            watcher.run().await?;
        }
        "daemon" | "start" => {
            let start_preset = command == "start";
            let addr = read_flag(&raw_args, "--addr")
                .unwrap_or_else(|| "127.0.0.1:8080".to_string())
                .parse::<SocketAddr>()?;
            ensure_loopback_addr(addr, has_flag(&raw_args, "--allow-non-loopback"))?;

            let process_limit = read_flag(&raw_args, "--process-limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let promote_limit = read_flag(&raw_args, "--promote-limit")
                .map(|value| value.parse::<u32>())
                .transpose()?
                .unwrap_or(10);
            let idle_ms = read_flag(&raw_args, "--idle-ms")
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or(5000);
            let interval_ms = read_flag(&raw_args, "--interval-ms")
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or(2000);
            let watch_path = read_flag(&raw_args, "--watch-path").map(PathBuf::from);
            let watch_active_window = start_preset || has_flag(&raw_args, "--watch-active-window");
            let activity_interval_ms = read_flag(&raw_args, "--activity-interval-ms")
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or(DEFAULT_ACTIVITY_POLL_MS);
            let recursive = !has_flag(&raw_args, "--non-recursive");
            let hotkey = start_preset || has_flag(&raw_args, "--hotkey");
            let hotkey_combo = read_flag(&raw_args, "--hotkey-combo")
                .unwrap_or_else(|| "ctrl+shift+i".to_string());
            let paste_on_hotkey = has_flag(&raw_args, "--paste-on-hotkey");

            if let Some(path) = watch_path.as_ref() {
                if !path.exists() {
                    return Err(format!("watch path does not exist: {}", path.display()).into());
                }
                ensure_safe_watch_root(
                    path,
                    &paths.root,
                    has_flag(&raw_args, WATCH_UNSAFE_ROOT_FLAG),
                )
                .map_err(|error| {
                    std::io::Error::new(std::io::ErrorKind::PermissionDenied, error)
                })?;
            }

            if start_preset {
                log_info(&format!(
                    "starting identityd default local context capture (hotkey={hotkey_combo}, paste={paste_on_hotkey})"
                ));
            }
            log_info("press Ctrl+C to stop identityd daemon");
            run_daemon(
                paths,
                DaemonConfig {
                    addr,
                    process_limit,
                    promote_limit,
                    idle_ms,
                    interval_ms,
                    watch_path,
                    watch_active_window,
                    activity_interval_ms,
                    recursive,
                    hotkey,
                    hotkey_combo,
                    paste_on_hotkey,
                },
            )
            .await?;
        }
        "help" | "--help" | "-h" => print_help(),
        other => {
            return Err(format!("unknown command '{other}'. Run `identityd help`.").into());
        }
    }

    Ok(())
}

fn current_binary_size_bytes() -> Option<u64> {
    let executable = std::env::current_exe().ok()?;
    std::fs::metadata(executable)
        .ok()
        .map(|metadata| metadata.len())
}

fn optional_u64(value: Option<u64>) -> String {
    value
        .map(|number| number.to_string())
        .unwrap_or_else(|| "unavailable".to_string())
}

fn optional_u128(value: Option<u128>) -> String {
    value
        .map(|number| number.to_string())
        .unwrap_or_else(|| "unavailable".to_string())
}

fn optional_usize(value: Option<usize>) -> String {
    value
        .map(|number| number.to_string())
        .unwrap_or_else(|| "unavailable".to_string())
}

fn optional_string(value: Option<&str>) -> String {
    value.unwrap_or("unavailable").to_string()
}

fn read_page_capture_text(args: &[String]) -> Result<String, Box<dyn Error>> {
    if has_flag(args, "--stdin") {
        let mut content = String::new();
        std::io::stdin().read_to_string(&mut content)?;
        return Ok(content);
    }

    read_flag(args, "--text")
        .or_else(|| read_flag(args, "--content"))
        .ok_or_else(|| {
            "capture-page command requires --text <selected text>, --content <selected text>, --stdin, or --from-clipboard"
            .into()
        })
}

fn read_agent_delta_candidate(
    args: &[String],
) -> Result<identityd::delta::AgentDelta, Box<dyn Error>> {
    if let Some(candidate_json) = read_flag(args, "--candidate-json") {
        return Ok(agent_delta_from_json(&candidate_json)?);
    }

    if has_flag(args, "--candidate-json-stdin") {
        let mut candidate_json = String::new();
        std::io::stdin().read_to_string(&mut candidate_json)?;
        return Ok(agent_delta_from_json(&candidate_json)?);
    }

    let source = read_flag(args, "--source");
    let text = read_agent_delta_text(args)?;
    Ok(extract_agent_delta(&text, source.as_deref())?)
}

fn read_agent_delta_text(args: &[String]) -> Result<String, Box<dyn Error>> {
    if has_flag(args, "--stdin") {
        let mut content = String::new();
        std::io::stdin().read_to_string(&mut content)?;
        return Ok(content);
    }

    read_flag(args, "--text")
        .or_else(|| read_flag(args, "--content"))
        .ok_or_else(|| {
            "agent-delta command requires --text <outcome text>, --content <outcome text>, --stdin, --candidate-json <json>, or --candidate-json-stdin"
                .into()
        })
}

fn read_flag(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == flag)
        .map(|window| window[1].clone())
}

fn command_arg(args: &[String]) -> Option<String> {
    let mut skip_next = false;

    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }

        if arg == "--root" {
            skip_next = true;
            continue;
        }

        if !arg.starts_with('-') {
            return Some(arg.clone());
        }
    }

    None
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn ensure_loopback_addr(addr: SocketAddr, allow_non_loopback: bool) -> Result<(), Box<dyn Error>> {
    if addr.ip().is_loopback() || allow_non_loopback {
        Ok(())
    } else {
        Err(format!(
            "refusing to bind capture endpoint to non-loopback address {addr}; pass --allow-non-loopback only for explicit local development"
        )
        .into())
    }
}

fn print_help() {
    println!(
        concat!(
            "identityd\n\n",
            "Global:\n",
            "  --root <folder>    Use a specific Identity workspace root\n\n",
            "Commands:\n",
            "  init\n",
            "  start [--paste-on-hotkey] [--hotkey-combo ctrl+shift+i]\n",
            "  ingest --source <source> --content <text>\n",
            "  capture-active-window\n",
            "  capture-page --title <title> --url <url> (--text <selected text> | --stdin | --from-clipboard) [--dry-run] [--promote-now] [--addr 127.0.0.1:8080]\n",
            "  browser-capture-bookmarklet [--addr 127.0.0.1:8080]\n",
            "  browser-capture-clipboard-bookmarklet\n",
            "  watch-active-window [--interval-ms 1000]\n",
            "  list\n",
            "  stats\n",
            "  capture-sources\n",
            "  doctor [--lease-ms 300000]\n",
            "  repair-transit [--lease-ms 300000]\n",
            "  protect-at-rest [--limit 100]\n",
            "  redact-transit-content [--limit 100]\n",
            "  cleaned-list [--limit 10]\n",
            "  memory-list [--limit 10]\n",
            "  memory-stats\n",
            "  embedding-runtime-health\n",
            "  embedding-active-health\n",
            "  onnx-runtime-health\n",
            "  embedding-tokenizer-health [--vocab-path <vocab.txt>]\n",
            "  embedding-tokenize --text <text> [--vocab-path <vocab.txt>] [--max-tokens 256]\n",
            "  embedding-onnx-run --text <text> [--model-path <file.onnx>] [--vocab-path <vocab.txt>] [--max-tokens 256]\n",
            "  embedding-manifest-write --model-path <file.onnx> --model-id <id> [--force]\n",
            "  embedding-bootstrap [--model-dir <path>]\n",
            "  memory-export [--limit 10]\n",
            "  memory-protocol-health\n",
            "  repair-protocol-schema [--limit 100]\n",
            "  repair-memory-vectors [--limit 100]\n",
            "  memory-search --query <text> [--limit 5]\n",
            "  memory-edge-add --source-id <id> --target-id <id> --relationship <type> [--weight 1.0]\n",
            "  memory-edges-list [--limit 10]\n",
            "  memory-edge-decay [--limit 100]\n",
            "  memory-graph-health\n",
            "  context-now [--preview] [--copy] [--project <name>] [--limit 3]\n",
            "  project-profile-list\n",
            "  slice-preview --intent <text> [--limit 3]\n",
            "  prompt-package --intent <text> --prompt <text> [--limit 3]\n",
            "  agent-delta-list [--limit 10] [--review-only] [--source <label>] [--entity <name>] [--state <STATE>] (max 100)\n",
            "  agent-delta-preview (--text <outcome text> | --stdin | --candidate-json <json> | --candidate-json-stdin) [--source <label>]\n",
            "  agent-delta-commit (--text <outcome text> | --stdin | --candidate-json <json> | --candidate-json-stdin) [--source <label>] [--allow-sensitive] [--json]\n",
            "  process-once [--limit 10]\n",
            "  process-idle-once [--limit 10] [--idle-ms 5000]\n",
            "  pipeline-once [--process-limit 10] [--promote-limit 10] [--idle-ms 5000]\n",
            "  pipeline-loop [--process-limit 10] [--promote-limit 10] [--idle-ms 5000] [--interval-ms 2000]\n",
            "  promote-once [--limit 10]\n",
            "  serve [--addr 127.0.0.1:8080] [--allow-non-loopback]\n",
            "  watch --path <folder> [--non-recursive] [--poll] [--allow-unsafe-watch-root]\n",
            "  daemon [--addr 127.0.0.1:8080] [--process-limit 10] [--promote-limit 10] [--idle-ms 5000] [--interval-ms 2000] [--watch-path <folder>] [--watch-active-window] [--activity-interval-ms 1000] [--non-recursive] [--allow-non-loopback] [--allow-unsafe-watch-root] [--hotkey] [--hotkey-combo <combo>] [--paste-on-hotkey]"
        )
    );
}

fn phase1_local_pipeline_status(
    stale_processing: i64,
    failed_transit: i64,
    invalid_vectors: i64,
    transit_insert_ms: u128,
) -> &'static str {
    if stale_processing > 0 || failed_transit > 0 || invalid_vectors > 0 {
        "needs-repair"
    } else if transit_insert_ms <= 1 {
        "ready"
    } else {
        "slow"
    }
}

fn phase1_embedding_artifact_status(
    artifact_ready: bool,
    artifact_status: &'static str,
) -> &'static str {
    if artifact_ready {
        "ready"
    } else {
        artifact_status
    }
}

fn phase1_remaining_summary(
    embedding_artifact_ready: bool,
    onnx_session_ready: bool,
    vector_store_ready: bool,
    accessibility_ready: bool,
) -> String {
    let mut remaining = Vec::new();
    if !embedding_artifact_ready {
        remaining.push("valid local ONNX embedding artifact (run embedding-bootstrap)");
    }
    if !onnx_session_ready {
        remaining.push("final ONNX/ort embedding runtime (dll + feature flag)");
    }
    if !vector_store_ready {
        remaining.push("default local vector store backend");
    }
    if !accessibility_ready {
        remaining.push("fuller OS accessibility coverage");
    }
    remaining.push("cross-platform OS content-protection backends beyond Windows");

    remaining.join("; ")
}

fn phase1_next_milestone(embedding_artifact_ready: bool, onnx_session_ready: bool) -> &'static str {
    if !embedding_artifact_ready {
        "run `identityd embedding-bootstrap` to download the local ONNX model and vocabulary"
    } else if !onnx_session_ready {
        "download onnxruntime.dll, set ORT_DYLIB_PATH, and build with --features onnx-runtime"
    } else {
        "Phase 1 core ingestion is complete. Proceed to Phase 2 hotkey command bar overlay."
    }
}

fn count_ready<const N: usize>(markers: [bool; N]) -> u32 {
    markers.into_iter().filter(|marker| *marker).count() as u32
}

fn completion_percent(ready_markers: u32, partial_markers: u32, total_markers: u32) -> u32 {
    if total_markers == 0 {
        return 0;
    }

    let numerator = ready_markers.saturating_mul(100) + partial_markers.saturating_mul(50);
    ((numerator + (total_markers / 2)) / total_markers).min(100)
}

async fn run_pipeline_loop(
    paths: IdentityPaths,
    process_limit: u32,
    promote_limit: u32,
    idle_ms: u64,
    interval_ms: u64,
) -> Result<(), Box<dyn Error>> {
    loop {
        match pipeline_once_if_idle(&paths, process_limit, promote_limit, idle_ms) {
            Ok(summary) => {
                if !summary.processed.skipped_idle_gate {
                    print_pipeline_summary("pipeline cycle", &summary);
                }
            }
            Err(error) => {
                log_error(&format!(
                    "pipeline cycle error (will retry after {interval_ms}ms): {error}"
                ));
            }
        }
        sleep(Duration::from_millis(interval_ms)).await;
    }
}

async fn run_daemon_active_window_watch(
    paths: IdentityPaths,
    interval_ms: u64,
    shutdown: Arc<AtomicBool>,
) -> Result<(), Box<dyn Error>> {
    let retry_ms = interval_ms.max(1000);

    loop {
        if shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }

        match watch_active_window_until_shutdown(paths.clone(), interval_ms, shutdown.clone()).await
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                log_error(&format!(
                    "active-window watcher error (will retry after {retry_ms}ms): {error}"
                ));
                sleep(Duration::from_millis(retry_ms)).await;
            }
        }
    }
}

async fn run_active_window_watch(
    paths: IdentityPaths,
    interval_ms: u64,
) -> Result<(), Box<dyn Error>> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let watcher = watch_active_window_until_shutdown(paths, interval_ms, shutdown.clone());
    tokio::pin!(watcher);

    tokio::select! {
        result = &mut watcher => result.map_err(|error| Box::new(error) as Box<dyn Error>),
        _ = signal::ctrl_c() => {
            shutdown.store(true, Ordering::Relaxed);
            println!("shutdown signal received");
            Ok(())
        }
    }
}

struct DaemonConfig {
    addr: SocketAddr,
    process_limit: u32,
    promote_limit: u32,
    idle_ms: u64,
    interval_ms: u64,
    watch_path: Option<PathBuf>,
    watch_active_window: bool,
    activity_interval_ms: u64,
    recursive: bool,
    hotkey: bool,
    hotkey_combo: String,
    paste_on_hotkey: bool,
}

async fn run_daemon(paths: IdentityPaths, config: DaemonConfig) -> Result<(), Box<dyn Error>> {
    loop {
        match run_daemon_once(paths.clone(), &config).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                log_error(&format!(
                    "identityd daemon stopped unexpectedly: {error}; restarting in 1000ms"
                ));
                sleep(Duration::from_millis(1000)).await;
            }
        }
    }
}

async fn run_daemon_once(
    paths: IdentityPaths,
    config: &DaemonConfig,
) -> Result<(), Box<dyn Error>> {
    let server = LocalCaptureServer::bind(config.addr, paths.clone()).await?;
    let shutdown = Arc::new(AtomicBool::new(false));

    let _hotkey_handle = if config.hotkey {
        match identityd::hotkey::start_hotkey_listener(
            paths.clone(),
            &config.hotkey_combo,
            config.paste_on_hotkey,
            shutdown.clone(),
        ) {
            Ok(handle) => {
                log_info(&format!(
                    "global hotkey listener started on combo '{}' (paste={})",
                    config.hotkey_combo, config.paste_on_hotkey
                ));
                Some(handle)
            }
            Err(error) => {
                log_error(&format!("failed to start global hotkey listener: {error}"));
                None
            }
        }
    } else {
        None
    };

    let pipeline = run_pipeline_loop(
        paths.clone(),
        config.process_limit,
        config.promote_limit,
        config.idle_ms,
        config.interval_ms,
    );
    let activity_watch = config.watch_active_window.then(|| {
        run_daemon_active_window_watch(paths.clone(), config.activity_interval_ms, shutdown.clone())
    });

    if let Some(watch_root) = config.watch_path.clone() {
        let watcher = FileWatcher::new(
            paths,
            FileWatcherConfig {
                root: watch_root,
                recursive: config.recursive,
                mode: FileWatcherMode::NativePreferred,
            },
        );

        let server = server.run();
        let watcher = watcher.run_until_shutdown(shutdown.clone());
        tokio::pin!(server);
        tokio::pin!(pipeline);
        tokio::pin!(watcher);

        if let Some(activity_watch) = activity_watch {
            tokio::pin!(activity_watch);

            tokio::select! {
                result = &mut server => match result {
                    Ok(()) => Err("capture endpoint stopped unexpectedly".into()),
                    Err(error) => Err(Box::new(error) as Box<dyn Error>),
                },
                result = &mut pipeline => match result {
                    Ok(()) => Err("pipeline loop stopped unexpectedly".into()),
                    Err(error) => Err(error),
                },
                result = &mut watcher => match result {
                    Ok(()) => Err("filesystem watcher stopped unexpectedly".into()),
                    Err(error) => Err(Box::new(error) as Box<dyn Error>),
                },
                result = &mut activity_watch => match result {
                    Ok(()) => Err("active-window watcher stopped unexpectedly".into()),
                    Err(error) => Err(error),
                },
            }
        } else {
            tokio::select! {
                result = &mut server => match result {
                    Ok(()) => Err("capture endpoint stopped unexpectedly".into()),
                    Err(error) => Err(Box::new(error) as Box<dyn Error>),
                },
                result = &mut pipeline => match result {
                    Ok(()) => Err("pipeline loop stopped unexpectedly".into()),
                    Err(error) => Err(error),
                },
                result = &mut watcher => match result {
                    Ok(()) => Err("filesystem watcher stopped unexpectedly".into()),
                    Err(error) => Err(Box::new(error) as Box<dyn Error>),
                },
            }
        }
    } else {
        let server = server.run();
        tokio::pin!(server);
        tokio::pin!(pipeline);

        if let Some(activity_watch) = activity_watch {
            tokio::pin!(activity_watch);

            tokio::select! {
                result = &mut server => match result {
                    Ok(()) => Err("capture endpoint stopped unexpectedly".into()),
                    Err(error) => Err(Box::new(error) as Box<dyn Error>),
                },
                result = &mut pipeline => match result {
                    Ok(()) => Err("pipeline loop stopped unexpectedly".into()),
                    Err(error) => Err(error),
                },
                result = &mut activity_watch => match result {
                    Ok(()) => Err("active-window watcher stopped unexpectedly".into()),
                    Err(error) => Err(error),
                },
            }
        } else {
            tokio::select! {
                result = &mut server => match result {
                    Ok(()) => Err("capture endpoint stopped unexpectedly".into()),
                    Err(error) => Err(Box::new(error) as Box<dyn Error>),
                },
                result = &mut pipeline => match result {
                    Ok(()) => Err("pipeline loop stopped unexpectedly".into()),
                    Err(error) => Err(error),
                },
            }
        }
    }
}

fn print_pipeline_summary(label: &str, summary: &identityd::processor::PipelineSummary) {
    log_info(&format!(
        "{label}: process_claimed={process_claimed} processed={processed} process_failed={process_failed} skipped_idle_gate={skipped} promote_claimed={promote_claimed} promoted={promoted} promote_failed={promote_failed} redacted={redacted}",
        process_claimed = summary.processed.claimed,
        processed = summary.processed.processed,
        process_failed = summary.processed.failed,
        skipped = summary.processed.skipped_idle_gate,
        promote_claimed = summary.promoted.claimed,
        promoted = summary.promoted.promoted,
        promote_failed = summary.promoted.failed,
        redacted = summary.promoted.redacted
    ));
}

fn log_info(message: &str) {
    let _ = writeln!(std::io::stdout(), "{message}");
}

fn log_error(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

fn print_source_family_counts(sources: &TransitSourceFamilyCounts) {
    println!("capture_source_manual_count={}", sources.manual);
    println!("capture_source_loopback_count={}", sources.loopback);
    println!("capture_source_filesystem_count={}", sources.filesystem);
    println!(
        "capture_source_active_window_count={}",
        sources.active_window
    );
    println!("capture_source_other_count={}", sources.other);
}

fn print_onnx_runtime_health(health: &OnnxRuntimeHealth) {
    println!("onnx_runtime_feature_enabled={}", health.feature_enabled);
    println!("onnx_runtime_dylib_env={}", health.dylib_env_var);
    println!(
        "onnx_runtime_dylib_path_configured={}",
        health.dylib_path_configured
    );
    println!("onnx_runtime_artifact_status={}", health.artifact_status);
    println!("onnx_runtime_session_status={}", health.session_status);
    println!("onnx_runtime_load_ms={}", optional_u128(health.load_ms));
    println!(
        "onnx_runtime_input_count={}",
        optional_usize(health.input_count)
    );
    println!(
        "onnx_runtime_output_count={}",
        optional_usize(health.output_count)
    );
    println!(
        "onnx_runtime_first_input={}",
        optional_string(health.first_input.as_deref())
    );
    println!(
        "onnx_runtime_first_output={}",
        optional_string(health.first_output.as_deref())
    );
}

fn print_tokenizer_health(health: &TokenizerHealth) {
    println!("tokenizer_vocab_env={}", health.env_var);
    println!("tokenizer_vocab_configured={}", health.configured);
    println!(
        "tokenizer_vocab_path={}",
        optional_string(health.path.as_deref())
    );
    println!("tokenizer_vocab_exists={}", health.exists);
    println!("tokenizer_vocab_is_file={}", health.is_file);
    println!(
        "tokenizer_vocab_size_bytes={}",
        optional_u64(health.size_bytes)
    );
    println!(
        "tokenizer_vocab_token_count={}",
        optional_usize(health.token_count)
    );
    println!("tokenizer_vocab_status={}", health.status);
}

fn print_active_embedding_health(health: &ActiveEmbeddingHealth) {
    println!("embedding_runtime_env={}", EMBEDDING_RUNTIME_ENV);
    println!("embedding_requested_runtime={}", health.requested_runtime);
    println!("embedding_active_runtime={}", health.active_runtime);
    println!(
        "embedding_fallback_reason={}",
        optional_string(health.fallback_reason.as_deref())
    );
}

fn join_i64(values: &[i64]) -> String {
    values
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn join_f32_prefix(values: &[f32], limit: usize) -> String {
    values
        .iter()
        .take(limit)
        .map(|value| format!("{value:.6}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::{
        command_arg, completion_percent, count_ready, ensure_loopback_addr, optional_string,
        optional_u64, optional_usize, phase1_embedding_artifact_status,
        phase1_local_pipeline_status, phase1_next_milestone, phase1_remaining_summary,
    };
    use std::net::SocketAddr;

    #[test]
    fn serve_rejects_non_loopback_addresses_by_default() {
        let addr: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        assert!(ensure_loopback_addr(addr, false).is_err());
        assert!(ensure_loopback_addr(addr, true).is_ok());
    }

    #[test]
    fn serve_allows_loopback_addresses() {
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        assert!(ensure_loopback_addr(addr, false).is_ok());
    }

    #[test]
    fn command_arg_skips_global_root_flag() {
        let args = vec![
            "--root".to_string(),
            "C:/tmp/identity-test".to_string(),
            "doctor".to_string(),
        ];

        assert_eq!(command_arg(&args), Some("doctor".to_string()));
    }

    #[test]
    fn phase1_pipeline_status_prioritizes_repair_then_latency() {
        assert_eq!(phase1_local_pipeline_status(0, 0, 0, 0), "ready");
        assert_eq!(phase1_local_pipeline_status(0, 0, 0, 3), "slow");
        assert_eq!(phase1_local_pipeline_status(1, 0, 0, 0), "needs-repair");
        assert_eq!(phase1_local_pipeline_status(0, 1, 0, 0), "needs-repair");
        assert_eq!(phase1_local_pipeline_status(0, 0, 1, 0), "needs-repair");
    }

    #[test]
    fn phase1_completion_score_counts_ready_and_partial_markers() {
        assert_eq!(count_ready([true, false, true, true]), 3);
        assert_eq!(completion_percent(9, 3, 12), 88);
        assert_eq!(completion_percent(9, 4, 13), 85);
        assert_eq!(completion_percent(10, 3, 13), 88);
        assert_eq!(completion_percent(0, 0, 0), 0);
        assert_eq!(completion_percent(12, 12, 12), 100);
    }

    #[test]
    fn phase1_embedding_artifact_status_tracks_model_artifact_readiness() {
        assert_eq!(
            phase1_embedding_artifact_status(false, "not-configured"),
            "not-configured"
        );
        assert_eq!(phase1_embedding_artifact_status(false, "empty"), "empty");
        assert_eq!(phase1_embedding_artifact_status(true, "ready"), "ready");
        assert!(phase1_remaining_summary(false, true, true, true).starts_with("valid local ONNX"));
        assert!(phase1_remaining_summary(true, false, true, true).starts_with("final ONNX"));
        assert!(phase1_next_milestone(false, false).starts_with("run "));
        assert!(phase1_next_milestone(true, false).starts_with("download"));
    }

    #[test]
    fn optional_u64_formats_unavailable_values() {
        assert_eq!(optional_u64(Some(42)), "42");
        assert_eq!(optional_u64(None), "unavailable");
        assert_eq!(optional_usize(Some(384)), "384");
        assert_eq!(optional_usize(None), "unavailable");
        assert_eq!(optional_string(Some("ready")), "ready");
        assert_eq!(optional_string(None), "unavailable");
    }
}
