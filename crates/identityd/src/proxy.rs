use crate::identity::IdentityStore;
use crate::slice::{build_prompt_package, generate_meslice};
use crate::transit::{TransitBuffer, DEFAULT_PROCESSING_LEASE_MS};
use crate::workspace::IdentityPaths;
use lol_html::html_content::TextType;
use lol_html::{doc_text, HtmlRewriter, Settings};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::ErrorKind;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{timeout, Duration};

#[derive(Deserialize)]
struct SearchRequest {
    query: String,
    limit: Option<u32>,
}

#[derive(Serialize)]
struct SearchResultJson {
    id: i64,
    cleaned_event_id: i64,
    source: String,
    domain_context: String,
    entity_type: String,
    summary: String,
    structured_attributes: String,
    created_at_ms: i64,
    score: u32,
}

#[derive(Deserialize)]
struct SliceRequest {
    intent: String,
    limit: Option<u32>,
}

#[derive(Serialize)]
struct SliceResponse {
    session_token: String,
    expiry_epoch_ms: i64,
    context_group: String,
    facts: Vec<String>,
    context_block: String,
}

#[derive(Deserialize)]
struct PromptPackageRequest {
    intent: String,
    prompt: String,
    limit: Option<u32>,
}

#[derive(Serialize)]
struct PromptPackageResponse {
    package: String,
}

#[derive(Serialize)]
struct StatsResponse {
    transit_queued: i64,
    transit_processing: i64,
    transit_stale_processing: i64,
    transit_processed: i64,
    transit_failed: i64,
    memory_nodes: i64,
    memory_vectorized_nodes: i64,
    memory_invalid_vectors: i64,
    embedding_model_id: String,
    embedding_dim: usize,
    vector_store_backend: String,
}


const MAX_CAPTURE_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT_MS: u64 = 3000;

#[derive(Debug)]
pub enum ProxyError {
    Io(std::io::Error),
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProxyError {}

impl From<std::io::Error> for ProxyError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub struct LocalCaptureServer {
    addr: SocketAddr,
    paths: IdentityPaths,
}

impl LocalCaptureServer {
    pub fn new(addr: SocketAddr, paths: IdentityPaths) -> Self {
        Self { addr, paths }
    }

    pub async fn run(self) -> Result<(), ProxyError> {
        let listener = TcpListener::bind(self.addr).await?;
        println!("identityd capture endpoint listening on http://{}", self.addr);

        loop {
            let (stream, peer_addr) = listener.accept().await?;
            let paths = self.paths.clone();

            tokio::spawn(async move {
                if let Err(error) = handle_connection(stream, paths).await {
                    eprintln!("failed to handle capture request from {peer_addr}: {error}");
                }
            });
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    paths: IdentityPaths,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request = timeout(
        Duration::from_millis(REQUEST_TIMEOUT_MS),
        read_http_request(&mut stream),
    )
    .await??;

    let response = match request {
        HttpRequest::Health => HttpResponse::ok_json(r#"{"status":"ok"}"#),
        HttpRequest::Stats => {
            let stats_result = tokio::task::spawn_blocking(move || {
                let buffer = TransitBuffer::open(&paths)?;
                let health = buffer.health(DEFAULT_PROCESSING_LEASE_MS)?;
                let store = IdentityStore::open(&paths)?;
                let store_stats = store.stats()?;
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(StatsResponse {
                    transit_queued: health.queued,
                    transit_processing: health.processing,
                    transit_stale_processing: health.stale_processing,
                    transit_processed: health.processed,
                    transit_failed: health.failed,
                    memory_nodes: store_stats.node_count,
                    memory_vectorized_nodes: store_stats.vectorized_count,
                    memory_invalid_vectors: store_stats.invalid_vector_count,
                    embedding_model_id: store_stats.embedding_model_id,
                    embedding_dim: store_stats.embedding_dim,
                    vector_store_backend: store_stats.vector_store_backend,
                })
            })
            .await?;

            match stats_result {
                Ok(stats) => match serde_json::to_string(&stats) {
                    Ok(json) => HttpResponse::ok_json(&json),
                    Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"{error}"}}"#)),
                },
                Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"{error}"}}"#)),
            }
        }
        HttpRequest::Search { body } => {
            let req_result: Result<SearchRequest, _> = serde_json::from_str(&body);
            match req_result {
                Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"invalid json: {error}"}}"#)),
                Ok(req) => {
                    let search_result = tokio::task::spawn_blocking(move || {
                        let store = IdentityStore::open(&paths)?;
                        let limit = req.limit.unwrap_or(5);
                        let results = store.search(&req.query, limit)?;
                        let json_results = results.into_iter().map(|res| SearchResultJson {
                            id: res.node.id,
                            cleaned_event_id: res.node.cleaned_event_id,
                            source: res.node.source,
                            domain_context: res.node.domain_context,
                            entity_type: res.node.entity_type,
                            summary: res.node.summary,
                            structured_attributes: res.node.structured_attributes,
                            created_at_ms: res.node.created_at_ms,
                            score: res.score,
                        }).collect::<Vec<_>>();
                        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(json_results)
                    })
                    .await?;

                    match search_result {
                        Ok(res) => match serde_json::to_string(&res) {
                            Ok(json) => HttpResponse::ok_json(&json),
                            Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"{error}"}}"#)),
                        },
                        Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"{error}"}}"#)),
                    }
                }
            }
        }
        HttpRequest::Slice { body } => {
            let req_result: Result<SliceRequest, _> = serde_json::from_str(&body);
            match req_result {
                Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"invalid json: {error}"}}"#)),
                Ok(req) => {
                    let slice_result = tokio::task::spawn_blocking(move || {
                        let limit = req.limit.unwrap_or(3);
                        let slice = generate_meslice(&paths, &req.intent, limit)?;
                        let context_block = slice.to_context_block();
                        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(SliceResponse {
                            session_token: slice.session_token,
                            expiry_epoch_ms: slice.expiry_epoch_ms,
                            context_group: slice.context_group,
                            facts: slice.facts,
                            context_block,
                        })
                    })
                    .await?;

                    match slice_result {
                        Ok(res) => match serde_json::to_string(&res) {
                            Ok(json) => HttpResponse::ok_json(&json),
                            Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"{error}"}}"#)),
                        },
                        Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"{error}"}}"#)),
                    }
                }
            }
        }
        HttpRequest::PromptPackage { body } => {
            let req_result: Result<PromptPackageRequest, _> = serde_json::from_str(&body);
            match req_result {
                Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"invalid json: {error}"}}"#)),
                Ok(req) => {
                    let pkg_result = tokio::task::spawn_blocking(move || {
                        let limit = req.limit.unwrap_or(3);
                        let package = build_prompt_package(&paths, &req.intent, &req.prompt, limit)?;
                        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(PromptPackageResponse { package })
                    })
                    .await?;

                    match pkg_result {
                        Ok(res) => match serde_json::to_string(&res) {
                            Ok(json) => HttpResponse::ok_json(&json),
                            Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"{error}"}}"#)),
                        },
                        Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"{error}"}}"#)),
                    }
                }
            }
        }
        HttpRequest::Capture { content_type, body } => {
            let cleaned = clean_payload(&content_type, &body);

            if cleaned.trim().is_empty() {
                HttpResponse::bad_request(r#"{"error":"empty capture payload"}"#)
            } else {
                let source = if content_type.contains("text/html") {
                    "local-proxy:text/html"
                } else {
                    "local-proxy:text/plain"
                };

                let ingest_result = tokio::task::spawn_blocking(move || {
                    let buffer = TransitBuffer::open(&paths)?;
                    buffer.ingest_text(source, &cleaned)
                })
                .await?;

                match ingest_result {
                    Ok(id) => HttpResponse::ok_json(&format!(r#"{{"captured":true,"id":{id}}}"#)),
                    Err(error) => HttpResponse::bad_request(&format!(r#"{{"error":"{error}"}}"#)),
                }
            }
        }
        HttpRequest::Unsupported => HttpResponse::not_found(r#"{"error":"not found"}"#),
    };

    write_all(&stream, &response.as_bytes()).await?;
    Ok(())
}

enum HttpRequest {
    Health,
    Stats,
    Capture { content_type: String, body: String },
    Search { body: String },
    Slice { body: String },
    PromptPackage { body: String },
    Unsupported,
}

struct HttpResponse {
    status: &'static str,
    body: String,
}

impl HttpResponse {
    fn ok_json(body: &str) -> Self {
        Self {
            status: "200 OK",
            body: body.to_string(),
        }
    }

    fn bad_request(body: &str) -> Self {
        Self {
            status: "400 Bad Request",
            body: body.to_string(),
        }
    }

    fn not_found(body: &str) -> Self {
        Self {
            status: "404 Not Found",
            body: body.to_string(),
        }
    }

    fn as_bytes(&self) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}",
            status = self.status,
            length = self.body.len(),
            body = self.body
        )
        .into_bytes()
    }
}

async fn read_http_request(
    stream: &mut TcpStream,
) -> Result<HttpRequest, Box<dyn std::error::Error + Send + Sync>> {
    let mut buffer = Vec::new();
    let mut chunk = [0; 4096];

    loop {
        let read = read_chunk(stream, &mut chunk).await?;
        if read == 0 {
            break;
        }

        buffer.extend_from_slice(&chunk[..read]);

        if buffer.len() > MAX_CAPTURE_BYTES {
            return Err("capture request exceeded maximum payload size".into());
        }

        if let Some(header_end) = find_header_end(&buffer) {
            let headers = String::from_utf8_lossy(&buffer[..header_end]);
            let content_length = content_length(&headers).unwrap_or(0);
            let total_needed = header_end + 4 + content_length;

            while buffer.len() < total_needed {
                let read = read_chunk(stream, &mut chunk).await?;
                if read == 0 {
                    break;
                }

                buffer.extend_from_slice(&chunk[..read]);

                if buffer.len() > MAX_CAPTURE_BYTES {
                    return Err("capture request exceeded maximum payload size".into());
                }
            }

            return parse_request(&buffer, header_end);
        }
    }

    Ok(HttpRequest::Unsupported)
}

async fn read_chunk(stream: &TcpStream, chunk: &mut [u8]) -> std::io::Result<usize> {
    loop {
        stream.readable().await?;

        match stream.try_read(chunk) {
            Ok(read) => return Ok(read),
            Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        }
    }
}

async fn write_all(stream: &TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    let mut written = 0;

    while written < bytes.len() {
        stream.writable().await?;

        match stream.try_write(&bytes[written..]) {
            Ok(0) => return Err(std::io::Error::new(ErrorKind::WriteZero, "socket closed")),
            Ok(count) => written += count,
            Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

fn parse_request(
    buffer: &[u8],
    header_end: usize,
) -> Result<HttpRequest, Box<dyn std::error::Error + Send + Sync>> {
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let mut lines = headers.lines();
    let request_line = lines.next().unwrap_or_default();

    if request_line.starts_with("GET /health ") {
        return Ok(HttpRequest::Health);
    }

    if request_line.starts_with("GET /stats ") {
        return Ok(HttpRequest::Stats);
    }

    if request_line.starts_with("POST /capture ") {
        let content_type = header_value(&headers, "content-type").unwrap_or_default();
        let body = String::from_utf8_lossy(&buffer[header_end + 4..]).to_string();
        return Ok(HttpRequest::Capture { content_type, body });
    }

    if request_line.starts_with("POST /search ") {
        let body = String::from_utf8_lossy(&buffer[header_end + 4..]).to_string();
        return Ok(HttpRequest::Search { body });
    }

    if request_line.starts_with("POST /slice ") {
        let body = String::from_utf8_lossy(&buffer[header_end + 4..]).to_string();
        return Ok(HttpRequest::Slice { body });
    }

    if request_line.starts_with("POST /prompt-package ") {
        let body = String::from_utf8_lossy(&buffer[header_end + 4..]).to_string();
        return Ok(HttpRequest::PromptPackage { body });
    }

    Ok(HttpRequest::Unsupported)
}

fn clean_payload(content_type: &str, body: &str) -> String {
    if content_type.contains("text/html") || looks_like_html(body) {
        clean_html_to_text(body)
    } else {
        collapse_whitespace(body)
    }
}

pub fn clean_html_to_text(html: &str) -> String {
    let mut output = String::with_capacity(html.len());

    {
        let mut rewriter = HtmlRewriter::new(
            Settings {
                document_content_handlers: vec![doc_text!(|text| {
                    if matches!(
                        text.text_type(),
                        TextType::Data | TextType::RCData | TextType::PlainText
                    ) {
                        output.push_str(text.as_str());
                        output.push(' ');
                    }

                    Ok(())
                })],
                ..Settings::default()
            },
            |_chunk: &[u8]| {},
        );

        if rewriter.write(html.as_bytes()).is_err() || rewriter.end().is_err() {
            return collapse_whitespace(html);
        }
    }

    decode_entities(&collapse_whitespace(&output))
}

fn decode_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn looks_like_html(input: &str) -> bool {
    let trimmed = input.trim_start().to_lowercase();
    trimmed.starts_with("<!doctype html")
        || trimmed.starts_with("<html")
        || trimmed.contains("<body")
        || trimmed.contains("<div")
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(headers: &str) -> Option<usize> {
    header_value(headers, "content-length")?.parse().ok()
}

fn header_value(headers: &str, name: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim().eq_ignore_ascii_case(name) {
            Some(value.trim().to_lowercase())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{clean_html_to_text, parse_request, HttpRequest};

    #[test]
    fn strips_scripts_styles_tags_and_decodes_entities() {
        let html = r#"
            <html>
              <head><style>.hidden { display: none; }</style></head>
              <body>
                                <h1>Hello&nbsp;Identity</h1>
                <script>alert("nope")</script>
                <p>Local &amp; private capture.</p>
              </body>
            </html>
        "#;

        let cleaned = clean_html_to_text(html);

        assert_eq!(cleaned, "Hello Identity Local & private capture.");
        assert!(!cleaned.contains("alert"));
        assert!(!cleaned.contains("display"));
    }

    #[test]
    fn handles_malformed_html_and_gt_inside_attributes() {
        let html =
            r#"<div data-note="2 > 1"><p>Keep this&nbsp;text<script>drop()</script><span>and this"#;
        let cleaned = clean_html_to_text(html);

        assert_eq!(cleaned, "Keep this text and this");
        assert!(!cleaned.contains("drop"));
        assert!(!cleaned.contains("data-note"));
    }

    #[test]
    fn parses_supported_http_requests() {
        use super::find_header_end;

        let req = b"GET /health HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n";
        let header_end = find_header_end(req).unwrap();
        let parsed = parse_request(req, header_end).unwrap();
        assert!(matches!(parsed, HttpRequest::Health));

        let req = b"GET /stats HTTP/1.1\r\nHost: 127.0.0.1:8080\r\n\r\n";
        let header_end = find_header_end(req).unwrap();
        let parsed = parse_request(req, header_end).unwrap();
        assert!(matches!(parsed, HttpRequest::Stats));

        let req = b"POST /search HTTP/1.1\r\nContent-Length: 16\r\n\r\n{\"query\":\"rust\"}";
        let header_end = find_header_end(req).unwrap();
        let parsed = parse_request(req, header_end).unwrap();
        if let HttpRequest::Search { body } = parsed {
            assert_eq!(body, "{\"query\":\"rust\"}");
        } else {
            panic!("expected search request");
        }

        let req = b"POST /slice HTTP/1.1\r\nContent-Length: 18\r\n\r\n{\"intent\":\"draft\"}";
        let header_end = find_header_end(req).unwrap();
        let parsed = parse_request(req, header_end).unwrap();
        if let HttpRequest::Slice { body } = parsed {
            assert_eq!(body, "{\"intent\":\"draft\"}");
        } else {
            panic!("expected slice request");
        }

        let req = b"POST /prompt-package HTTP/1.1\r\nContent-Length: 26\r\n\r\n{\"intent\":\"t\",\"prompt\":\"p\"}";
        let header_end = find_header_end(req).unwrap();
        let parsed = parse_request(req, header_end).unwrap();
        if let HttpRequest::PromptPackage { body } = parsed {
            assert_eq!(body, "{\"intent\":\"t\",\"prompt\":\"p\"}");
        } else {
            panic!("expected prompt package request");
        }

        let req = b"GET /unsupported HTTP/1.1\r\n\r\n";
        let header_end = find_header_end(req).unwrap();
        let parsed = parse_request(req, header_end).unwrap();
        assert!(matches!(parsed, HttpRequest::Unsupported));
    }
}

