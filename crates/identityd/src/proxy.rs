use crate::ingest_safety::MAX_CAPTURE_CONTENT_BYTES;
use crate::transit::TransitBuffer;
use crate::workspace::{IdentityPaths, WorkspaceError};
use lol_html::html_content::TextType;
use lol_html::{doc_text, HtmlRewriter, Settings};
use std::fmt;
use std::io::ErrorKind;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{timeout, Duration};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT_MS: u64 = 3000;

#[derive(Debug)]
pub enum ProxyError {
    Io(std::io::Error),
    Workspace(WorkspaceError),
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Workspace(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProxyError {}

impl From<std::io::Error> for ProxyError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<WorkspaceError> for ProxyError {
    fn from(value: WorkspaceError) -> Self {
        Self::Workspace(value)
    }
}

pub struct LocalCaptureServer {
    addr: SocketAddr,
    paths: IdentityPaths,
    capture_token: String,
}

impl LocalCaptureServer {
    pub fn new(addr: SocketAddr, paths: IdentityPaths) -> Result<Self, ProxyError> {
        let capture_token = paths.ensure_capture_token()?;
        Ok(Self {
            addr,
            paths,
            capture_token,
        })
    }

    pub async fn run(self) -> Result<(), ProxyError> {
        let listener = TcpListener::bind(self.addr).await?;
        println!(
            "identityd capture endpoint listening on http://{}",
            self.addr
        );

        loop {
            let (stream, peer_addr) = listener.accept().await?;
            let paths = self.paths.clone();
            let capture_token = self.capture_token.clone();

            tokio::spawn(async move {
                if let Err(error) = handle_connection(stream, paths, capture_token).await {
                    eprintln!("failed to handle capture request from {peer_addr}: {error}");
                }
            });
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    paths: IdentityPaths,
    capture_token: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request = timeout(
        Duration::from_millis(REQUEST_TIMEOUT_MS),
        read_http_request(&mut stream, &capture_token),
    )
    .await??;

    let response = match request {
        HttpRequest::Health => HttpResponse::ok_json(r#"{"status":"ok"}"#),
        HttpRequest::Capture { content_type, body } => {
            if !supported_capture_content_type(&content_type) {
                HttpResponse::unsupported_media_type(
                    r#"{"error":"unsupported capture content type"}"#,
                )
            } else {
                let cleaned = clean_payload(&content_type, &body);

                if cleaned.trim().is_empty() {
                    HttpResponse::bad_request(r#"{"error":"empty capture payload"}"#)
                } else {
                    let source = capture_source_for_content_type(&content_type);

                    let ingest_result = tokio::task::spawn_blocking(move || {
                        let buffer = TransitBuffer::open(&paths)?;
                        buffer.ingest_text(source, &cleaned)
                    })
                    .await?;

                    match ingest_result {
                        Ok(id) => {
                            HttpResponse::ok_json(&format!(r#"{{"captured":true,"id":{id}}}"#))
                        }
                        Err(error) => {
                            HttpResponse::bad_request(&format!(r#"{{"error":"{error}"}}"#))
                        }
                    }
                }
            }
        }
        HttpRequest::Unauthorized => HttpResponse::unauthorized(r#"{"error":"unauthorized"}"#),
        HttpRequest::PayloadTooLarge => {
            HttpResponse::payload_too_large(r#"{"error":"capture payload too large"}"#)
        }
        HttpRequest::BadRequest => HttpResponse::bad_request(r#"{"error":"bad request"}"#),
        HttpRequest::Unsupported => HttpResponse::not_found(r#"{"error":"not found"}"#),
    };

    write_all(&stream, &response.as_bytes()).await?;
    Ok(())
}

enum HttpRequest {
    Health,
    Capture { content_type: String, body: String },
    Unauthorized,
    PayloadTooLarge,
    BadRequest,
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

    fn unauthorized(body: &str) -> Self {
        Self {
            status: "401 Unauthorized",
            body: body.to_string(),
        }
    }

    fn payload_too_large(body: &str) -> Self {
        Self {
            status: "413 Payload Too Large",
            body: body.to_string(),
        }
    }

    fn unsupported_media_type(body: &str) -> Self {
        Self {
            status: "415 Unsupported Media Type",
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
    capture_token: &str,
) -> Result<HttpRequest, Box<dyn std::error::Error + Send + Sync>> {
    let mut buffer = Vec::new();
    let mut chunk = [0; 4096];

    loop {
        let read = read_chunk(stream, &mut chunk).await?;
        if read == 0 {
            break;
        }

        buffer.extend_from_slice(&chunk[..read]);

        if let Some(header_end) = find_header_end(&buffer) {
            let headers = String::from_utf8_lossy(&buffer[..header_end]);
            let content_length = match content_length(&headers) {
                Ok(value) => value,
                Err(request) => return Ok(request),
            };

            if content_length > MAX_CAPTURE_CONTENT_BYTES {
                return Ok(HttpRequest::PayloadTooLarge);
            }

            let Some(body_start) = header_end.checked_add(4) else {
                return Ok(HttpRequest::PayloadTooLarge);
            };
            let Some(total_needed) = body_start.checked_add(content_length) else {
                return Ok(HttpRequest::PayloadTooLarge);
            };

            while buffer.len() < total_needed {
                let read = read_chunk(stream, &mut chunk).await?;
                if read == 0 {
                    break;
                }

                buffer.extend_from_slice(&chunk[..read]);

                if buffer.len().saturating_sub(body_start) > MAX_CAPTURE_CONTENT_BYTES {
                    return Ok(HttpRequest::PayloadTooLarge);
                }
            }

            return parse_request(&buffer, header_end, total_needed, capture_token);
        }

        if buffer.len() > MAX_HEADER_BYTES {
            return Ok(HttpRequest::PayloadTooLarge);
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
    body_end: usize,
    capture_token: &str,
) -> Result<HttpRequest, Box<dyn std::error::Error + Send + Sync>> {
    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let mut lines = headers.lines();
    let request_line = lines.next().unwrap_or_default();

    if request_line.starts_with("GET /health ") {
        return Ok(HttpRequest::Health);
    }

    if request_line.starts_with("POST /capture ") {
        if !valid_capture_token(&headers, capture_token) {
            return Ok(HttpRequest::Unauthorized);
        }

        let content_type = header_value(&headers, "content-type").unwrap_or_default();
        let body_start = header_end + 4;
        let body =
            String::from_utf8_lossy(&buffer[body_start..body_end.min(buffer.len())]).to_string();
        return Ok(HttpRequest::Capture { content_type, body });
    }

    Ok(HttpRequest::Unsupported)
}

fn valid_capture_token(headers: &str, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }

    header_value(headers, "x-identity-capture-token")
        .map(|actual| constant_time_eq(actual.as_bytes(), expected.as_bytes()))
        .unwrap_or(false)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = left.len() ^ right.len();
    let max_len = left.len().max(right.len());

    for index in 0..max_len {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(left_byte ^ right_byte);
    }

    diff == 0
}

fn clean_payload(content_type: &str, body: &str) -> String {
    if content_type.contains("text/html") || looks_like_html(body) {
        clean_html_to_text(body)
    } else {
        collapse_whitespace(body)
    }
}

fn supported_capture_content_type(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    matches!(
        mime.as_str(),
        "text/plain"
            | "text/html"
            | "text/markdown"
            | "application/json"
            | "application/x-ndjson"
            | "application/xml"
            | "application/xhtml+xml"
    )
}

fn capture_source_for_content_type(content_type: &str) -> &'static str {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    match mime.as_str() {
        "text/html" | "application/xhtml+xml" => "local-proxy:text/html",
        "text/markdown" => "local-proxy:text/markdown",
        "application/json" | "application/x-ndjson" => "local-proxy:application/json",
        "application/xml" => "local-proxy:application/xml",
        _ => "local-proxy:text/plain",
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
    let mut compact = String::with_capacity(input.len());
    let mut last_was_whitespace = true;

    for c in input.chars() {
        if c.is_whitespace() {
            if !last_was_whitespace {
                compact.push(' ');
                last_was_whitespace = true;
            }
        } else {
            compact.push(c);
            last_was_whitespace = false;
        }
    }

    if last_was_whitespace && !compact.is_empty() {
        compact.pop();
    }

    compact
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

fn content_length(headers: &str) -> Result<usize, HttpRequest> {
    match header_value(headers, "content-length") {
        Some(value) => value.parse().map_err(|_| HttpRequest::BadRequest),
        None => Ok(0),
    }
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
        let parsed = parse_request(req, header_end, req.len(), "abc123").unwrap();
        assert!(matches!(parsed, HttpRequest::Health));

        let req =
            b"POST /capture HTTP/1.1\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nrust";
        let header_end = find_header_end(req).unwrap();
        let parsed = parse_request(req, header_end, req.len(), "abc123").unwrap();
        assert!(matches!(parsed, HttpRequest::Unauthorized));

        let req = b"POST /capture HTTP/1.1\r\nContent-Type: text/plain\r\nX-Identity-Capture-Token: abc123\r\nContent-Length: 4\r\n\r\nrust";
        let header_end = find_header_end(req).unwrap();
        let parsed = parse_request(req, header_end, req.len(), "abc123").unwrap();
        if let HttpRequest::Capture { content_type, body } = parsed {
            assert_eq!(content_type, "text/plain");
            assert_eq!(body, "rust");
        } else {
            panic!("expected capture request");
        }

        let req = b"GET /unsupported HTTP/1.1\r\n\r\n";
        let header_end = find_header_end(req).unwrap();
        let parsed = parse_request(req, header_end, req.len(), "abc123").unwrap();
        assert!(matches!(parsed, HttpRequest::Unsupported));
    }

    #[test]
    fn parses_only_declared_capture_body_bytes() {
        use super::find_header_end;

        let req = b"POST /capture HTTP/1.1\r\nContent-Type: text/plain\r\nX-Identity-Capture-Token: abc123\r\nContent-Length: 4\r\n\r\nrustEXTRA";
        let header_end = find_header_end(req).unwrap();
        let parsed = parse_request(req, header_end, header_end + 4 + 4, "abc123").unwrap();

        if let HttpRequest::Capture { body, .. } = parsed {
            assert_eq!(body, "rust");
        } else {
            panic!("expected capture request");
        }
    }

    #[test]
    fn rejects_invalid_content_length() {
        assert!(matches!(
            super::content_length("Content-Length: nope"),
            Err(HttpRequest::BadRequest)
        ));
    }

    #[test]
    fn accepts_only_textual_capture_content_types() {
        assert!(super::supported_capture_content_type(
            "text/html; charset=utf-8"
        ));
        assert!(super::supported_capture_content_type("text/plain"));
        assert!(super::supported_capture_content_type("text/markdown"));
        assert!(super::supported_capture_content_type("application/json"));
        assert!(super::supported_capture_content_type(
            "application/x-ndjson"
        ));
        assert!(super::supported_capture_content_type("application/xml"));
        assert!(super::supported_capture_content_type(
            "application/xhtml+xml"
        ));

        assert!(!super::supported_capture_content_type(""));
        assert!(!super::supported_capture_content_type("image/png"));
        assert!(!super::supported_capture_content_type(
            "application/octet-stream"
        ));
    }

    #[test]
    fn derives_capture_source_from_content_type() {
        assert_eq!(
            super::capture_source_for_content_type("text/html; charset=utf-8"),
            "local-proxy:text/html"
        );
        assert_eq!(
            super::capture_source_for_content_type("application/json"),
            "local-proxy:application/json"
        );
        assert_eq!(
            super::capture_source_for_content_type("text/markdown"),
            "local-proxy:text/markdown"
        );
    }
}
