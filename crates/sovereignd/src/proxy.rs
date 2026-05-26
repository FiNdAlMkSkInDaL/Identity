use crate::transit::TransitBuffer;
use crate::workspace::SovereignPaths;
use lol_html::html_content::TextType;
use lol_html::{doc_text, HtmlRewriter, Settings};
use std::fmt;
use std::io::ErrorKind;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{timeout, Duration};

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
    paths: SovereignPaths,
}

impl LocalCaptureServer {
    pub fn new(addr: SocketAddr, paths: SovereignPaths) -> Self {
        Self { addr, paths }
    }

    pub async fn run(self) -> Result<(), ProxyError> {
        let listener = TcpListener::bind(self.addr).await?;
        println!(
            "sovereignd capture endpoint listening on http://{}",
            self.addr
        );

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
    paths: SovereignPaths,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request = timeout(
        Duration::from_millis(REQUEST_TIMEOUT_MS),
        read_http_request(&mut stream),
    )
    .await??;

    let response = match request {
        HttpRequest::Health => HttpResponse::ok_json(r#"{"status":"ok"}"#),
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
    Capture { content_type: String, body: String },
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

    if !request_line.starts_with("POST /capture ") {
        return Ok(HttpRequest::Unsupported);
    }

    let content_type = header_value(&headers, "content-type").unwrap_or_default();
    let body = String::from_utf8_lossy(&buffer[header_end + 4..]).to_string();

    Ok(HttpRequest::Capture { content_type, body })
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
    use super::clean_html_to_text;

    #[test]
    fn strips_scripts_styles_tags_and_decodes_entities() {
        let html = r#"
            <html>
              <head><style>.hidden { display: none; }</style></head>
              <body>
                <h1>Hello&nbsp;Sovereign</h1>
                <script>alert("nope")</script>
                <p>Local &amp; private capture.</p>
              </body>
            </html>
        "#;

        let cleaned = clean_html_to_text(html);

        assert_eq!(cleaned, "Hello Sovereign Local & private capture.");
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
}
