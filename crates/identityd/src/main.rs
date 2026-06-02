use identityd::activity::{
    capture_active_window_once, watch_active_window_until_shutdown, DEFAULT_ACTIVITY_POLL_MS,
};
use identityd::embedding::{probe_embedding_latency, EMBEDDING_LATENCY_TARGET_MS};
use identityd::filesystem::{FileWatcher, FileWatcherConfig, FileWatcherMode};
use identityd::identity::IdentityStore;
use identityd::processor::{
    pipeline_once_if_idle, process_once, process_once_if_idle, promote_once,
};
use identityd::proxy::LocalCaptureServer;
use identityd::resource::{
    current_process_resources, memory_budget_status, IDLE_MEMORY_TARGET_BYTES,
};
use identityd::slice::{build_prompt_package, generate_meslice};
use identityd::transit::{TransitBuffer, DEFAULT_PROCESSING_LEASE_MS};
use identityd::workspace::IdentityPaths;
use std::env;
use std::error::Error;
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
        eprintln!("identityd error: {error}");
        std::process::exit(1);
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
        "doctor" => {
            let lease_ms = read_flag(&raw_args, "--lease-ms")
                .map(|value| value.parse::<i64>())
                .transpose()?
                .unwrap_or(DEFAULT_PROCESSING_LEASE_MS);
            let buffer = TransitBuffer::open(&paths)?;
            let transit = buffer.health(lease_ms)?;
            let transit_probe = buffer.probe_insert_rollback_latency()?;
            let store = IdentityStore::open(&paths)?;
            let memory = store.stats()?;
            let embedding_probe = probe_embedding_latency("Identity local embedding budget probe.");
            let resources = current_process_resources();
            let working_set_bytes = resources.as_ref().map(|probe| probe.working_set_bytes);
            let pagefile_bytes = resources.as_ref().map(|probe| probe.pagefile_bytes);
            let binary_size_bytes = current_binary_size_bytes();

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
            println!("memory_vectorized_nodes={}", memory.vectorized_count);
            println!("memory_invalid_vectors={}", memory.invalid_vector_count);
            println!("embedding_model_id={}", memory.embedding_model_id);
            println!("embedding_dim={}", memory.embedding_dim);
            println!("embedding_probe_model_id={}", embedding_probe.model_id);
            println!("embedding_probe_dim={}", embedding_probe.dimension);
            println!("embedding_probe_ms={}", embedding_probe.latency_ms);
                        println!("embedding_target_ms={EMBEDDING_LATENCY_TARGET_MS} within_budget={}",
                embedding_probe.latency_ms <= EMBEDDING_LATENCY_TARGET_MS
            );
            println!("vector_store_backend={}", memory.vector_store_backend);
            let graph = store.graph_health()?;
            println!("graph_edges={}", graph.edge_count);
            println!("graph_orphans={}", graph.orphan_count);
            println!("graph_decayed_edges={}", graph.decayed_edges);
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
            println!("phase1_capture_adapters=partial");
            println!(
                "phase1_transit_health={}",
                if transit.stale_processing == 0 && transit.failed == 0 {
                    "ready"
                } else {
                    "needs-repair"
                }
            );
            println!(
                "phase1_memory_vector_health={}",
                if memory.invalid_vector_count == 0 {
                    "ready"
                } else {
                    "needs-repair"
                }
            );
            println!("phase1_embedding_runtime=prototype");
            println!(
                "phase1_vector_store_backend={}",
                memory.vector_store_backend
            );
            println!(
                "phase1_local_pipeline={}",
                phase1_local_pipeline_status(
                    transit.stale_processing,
                    transit.failed,
                    memory.invalid_vector_count,
                    transit_probe.insert_rollback_ms,
                )
            );
            println!(
                "phase1_remaining=final ONNX/ort embedding runtime; default embedded LanceDB or equivalent hybrid graph; local encryption for real captured content; fuller OS accessibility coverage"
            );
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
                    "#{id} cleaned=#{cleaned_id} {domain}/{entity} {source} hash={hash} @ {created_at_ms}: {summary}",
                    id = memory.id,
                    cleaned_id = memory.cleaned_event_id,
                    domain = memory.domain_context,
                    entity = memory.entity_type,
                    source = memory.source,
                    hash = memory.content_hash,
                    created_at_ms = memory.created_at_ms,
                    summary = memory.summary.replace('\n', " ")
                );
            }
        }
        "memory-stats" => {
            let store = IdentityStore::open(&paths)?;
            let stats = store.stats()?;

            println!("memory_nodes={}", stats.node_count);
            println!("vectorized_nodes={}", stats.vectorized_count);
            println!("invalid_vectors={}", stats.invalid_vector_count);
            println!("embedding_model_id={}", stats.embedding_model_id);
            println!("embedding_dim={}", stats.embedding_dim);
            println!("vector_store_backend={}", stats.vector_store_backend);
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

            println!(
                "decayed edges: edges_decayed={}",
                summary.edges_decayed
            );
        }
        "memory-graph-health" => {
            let store = IdentityStore::open(&paths)?;
            let health = store.graph_health()?;

            println!("graph_nodes={}", health.node_count);
            println!("graph_edges={}", health.edge_count);
            println!("graph_orphans={}", health.orphan_count);
            println!("graph_decayed_edges={}", health.decayed_edges);
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
        "daemon" => {
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
            let watch_active_window = has_flag(&raw_args, "--watch-active-window");
            let activity_interval_ms = read_flag(&raw_args, "--activity-interval-ms")
                .map(|value| value.parse::<u64>())
                .transpose()?
                .unwrap_or(DEFAULT_ACTIVITY_POLL_MS);
            let recursive = !has_flag(&raw_args, "--non-recursive");

            if let Some(path) = watch_path.as_ref() {
                if !path.exists() {
                    return Err(format!("watch path does not exist: {}", path.display()).into());
                }
            }

            println!("press Ctrl+C to stop identityd daemon");
            run_daemon(
                paths,
                addr,
                process_limit,
                promote_limit,
                idle_ms,
                interval_ms,
                watch_path,
                watch_active_window,
                activity_interval_ms,
                recursive,
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
        "identityd\n\nGlobal:\n  --root <folder>    Use a specific Identity workspace root\n\nCommands:\n  init\n  ingest --source <source> --content <text>\n  capture-active-window\n  watch-active-window [--interval-ms 1000]\n  list\n  stats\n  doctor [--lease-ms 300000]\n  repair-transit [--lease-ms 300000]\n  redact-transit-content [--limit 100]\n  cleaned-list [--limit 10]\n  memory-list [--limit 10]\n  memory-stats\n  repair-memory-vectors [--limit 100]\n  memory-search --query <text> [--limit 5]\n  memory-edge-add --source-id <id> --target-id <id> --relationship <type> [--weight 1.0]\n  memory-edges-list [--limit 10]\n  memory-edge-decay [--limit 100]\n  memory-graph-health\n  slice-preview --intent <text> [--limit 3]\n  prompt-package --intent <text> --prompt <text> [--limit 3]\n  process-once [--limit 10]\n  process-idle-once [--limit 10] [--idle-ms 5000]\n  pipeline-once [--process-limit 10] [--promote-limit 10] [--idle-ms 5000]\n  pipeline-loop [--process-limit 10] [--promote-limit 10] [--idle-ms 5000] [--interval-ms 2000]\n  promote-once [--limit 10]\n  serve [--addr 127.0.0.1:8080] [--allow-non-loopback]\n  watch --path <folder> [--non-recursive] [--poll]\n  daemon [--addr 127.0.0.1:8080] [--process-limit 10] [--promote-limit 10] [--idle-ms 5000] [--interval-ms 2000] [--watch-path <folder>] [--watch-active-window] [--activity-interval-ms 1000] [--non-recursive] [--allow-non-loopback]"
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

async fn run_pipeline_loop(
    paths: IdentityPaths,
    process_limit: u32,
    promote_limit: u32,
    idle_ms: u64,
    interval_ms: u64,
) -> Result<(), Box<dyn Error>> {
    loop {
        let summary = pipeline_once_if_idle(&paths, process_limit, promote_limit, idle_ms)?;
        print_pipeline_summary("pipeline cycle", &summary);
        sleep(Duration::from_millis(interval_ms)).await;
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

async fn run_daemon(
    paths: IdentityPaths,
    addr: SocketAddr,
    process_limit: u32,
    promote_limit: u32,
    idle_ms: u64,
    interval_ms: u64,
    watch_path: Option<PathBuf>,
    watch_active_window: bool,
    activity_interval_ms: u64,
    recursive: bool,
) -> Result<(), Box<dyn Error>> {
    let server = LocalCaptureServer::new(addr, paths.clone())?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let pipeline = run_pipeline_loop(
        paths.clone(),
        process_limit,
        promote_limit,
        idle_ms,
        interval_ms,
    );
    let activity_watch = watch_active_window.then(|| {
        watch_active_window_until_shutdown(paths.clone(), activity_interval_ms, shutdown.clone())
    });

    if let Some(watch_root) = watch_path {
        let watcher = FileWatcher::new(
            paths,
            FileWatcherConfig {
                root: watch_root,
                recursive,
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
                result = &mut server => result.map_err(|error| Box::new(error) as Box<dyn Error>),
                result = &mut pipeline => result,
                result = &mut watcher => result.map_err(|error| Box::new(error) as Box<dyn Error>),
                result = &mut activity_watch => result.map_err(|error| Box::new(error) as Box<dyn Error>),
                _ = signal::ctrl_c() => {
                    shutdown.store(true, Ordering::Relaxed);
                    println!("shutdown signal received");
                    Ok(())
                }
            }
        } else {
            tokio::select! {
                result = &mut server => result.map_err(|error| Box::new(error) as Box<dyn Error>),
                result = &mut pipeline => result,
                result = &mut watcher => result.map_err(|error| Box::new(error) as Box<dyn Error>),
                _ = signal::ctrl_c() => {
                    shutdown.store(true, Ordering::Relaxed);
                    println!("shutdown signal received");
                    Ok(())
                }
            }
        }
    } else {
        let server = server.run();
        tokio::pin!(server);
        tokio::pin!(pipeline);

        if let Some(activity_watch) = activity_watch {
            tokio::pin!(activity_watch);

            tokio::select! {
                result = &mut server => result.map_err(|error| Box::new(error) as Box<dyn Error>),
                result = &mut pipeline => result,
                result = &mut activity_watch => result.map_err(|error| Box::new(error) as Box<dyn Error>),
                _ = signal::ctrl_c() => {
                    shutdown.store(true, Ordering::Relaxed);
                    println!("shutdown signal received");
                    Ok(())
                }
            }
        } else {
            tokio::select! {
                result = &mut server => result.map_err(|error| Box::new(error) as Box<dyn Error>),
                result = &mut pipeline => result,
                _ = signal::ctrl_c() => {
                    shutdown.store(true, Ordering::Relaxed);
                    println!("shutdown signal received");
                    Ok(())
                }
            }
        }
    }
}

fn print_pipeline_summary(label: &str, summary: &identityd::processor::PipelineSummary) {
    println!(
        "{label}: process_claimed={process_claimed} processed={processed} process_failed={process_failed} skipped_idle_gate={skipped} promote_claimed={promote_claimed} promoted={promoted} promote_failed={promote_failed} redacted={redacted}",
        process_claimed = summary.processed.claimed,
        processed = summary.processed.processed,
        process_failed = summary.processed.failed,
        skipped = summary.processed.skipped_idle_gate,
        promote_claimed = summary.promoted.claimed,
        promoted = summary.promoted.promoted,
        promote_failed = summary.promoted.failed,
        redacted = summary.promoted.redacted
    );
}

#[cfg(test)]
mod tests {
    use super::{command_arg, ensure_loopback_addr, optional_u64, phase1_local_pipeline_status};
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
    fn optional_u64_formats_unavailable_values() {
        assert_eq!(optional_u64(Some(42)), "42");
        assert_eq!(optional_u64(None), "unavailable");
    }
}
