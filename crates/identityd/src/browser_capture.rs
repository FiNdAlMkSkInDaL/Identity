use crate::ingest_safety::{validate_capture, IngestSafetyError, MAX_CAPTURE_CONTENT_BYTES};
use crate::workspace::{IdentityPaths, WorkspaceError};
use std::fmt;
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};

const REQUEST_TIMEOUT_MS: u64 = 3000;
const MAX_TITLE_CHARS: usize = 512;
const MAX_URL_CHARS: usize = 2048;
const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const CLIPBOARD_CAPTURE_START: &str = "[IDENTITY-PAGE-CAPTURE]";
const CLIPBOARD_CAPTURE_END: &str = "[IDENTITY-PAGE-CAPTURE-END]";

#[derive(Debug, Clone)]
pub struct PageCaptureInput {
    pub title: Option<String>,
    pub url: Option<String>,
    pub selected_text: String,
}

#[derive(Debug, Clone)]
pub struct CapturePostResult {
    pub status_code: u16,
    pub body: String,
    pub bytes_sent: usize,
    pub captured_id: Option<i64>,
}

#[derive(Debug)]
pub enum BrowserCaptureError {
    EmptySelection,
    OversizedCapture,
    UnsafeCapture(IngestSafetyError),
    Workspace(WorkspaceError),
    Io(std::io::Error),
    BadEndpointResponse,
    EndpointStatus { status_code: u16, body: String },
}

impl fmt::Display for BrowserCaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySelection => write!(f, "browser page capture requires selected text"),
            Self::OversizedCapture => write!(f, "browser page capture exceeds 1MB transit budget"),
            Self::UnsafeCapture(error) => write!(f, "{error}"),
            Self::Workspace(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::BadEndpointResponse => write!(f, "capture endpoint returned an invalid response"),
            Self::EndpointStatus { status_code, body } => {
                write!(f, "capture endpoint returned HTTP {status_code}: {body}")
            }
        }
    }
}

impl std::error::Error for BrowserCaptureError {}

impl From<WorkspaceError> for BrowserCaptureError {
    fn from(value: WorkspaceError) -> Self {
        Self::Workspace(value)
    }
}

impl From<std::io::Error> for BrowserCaptureError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<IngestSafetyError> for BrowserCaptureError {
    fn from(value: IngestSafetyError) -> Self {
        Self::UnsafeCapture(value)
    }
}

pub fn format_page_capture(input: &PageCaptureInput) -> Result<String, BrowserCaptureError> {
    let title = input
        .title
        .as_deref()
        .map(|value| normalize_field(value, MAX_TITLE_CHARS))
        .filter(|value| !value.is_empty());
    let url = input
        .url
        .as_deref()
        .map(|value| normalize_field(value, MAX_URL_CHARS))
        .filter(|value| !value.is_empty());
    let selected_text = normalize_selected_text(&input.selected_text);

    if selected_text.is_empty() {
        return Err(BrowserCaptureError::EmptySelection);
    }

    let mut body = String::new();
    if let Some(title) = title.as_ref() {
        body.push_str("Page title: ");
        body.push_str(title);
        body.push('\n');
    }
    if let Some(url) = url.as_ref() {
        body.push_str("Page URL: ");
        body.push_str(url);
        body.push('\n');
    }
    body.push_str("Selected page text:\n");
    body.push_str(&selected_text);

    if body.len() > MAX_CAPTURE_CONTENT_BYTES {
        return Err(BrowserCaptureError::OversizedCapture);
    }

    let safety_source = url
        .as_ref()
        .map(|value| format!("browser-page:{value}"))
        .unwrap_or_else(|| "browser-page:selected-text".to_string());
    validate_capture(&safety_source, &body)?;

    Ok(body)
}

pub async fn post_page_capture(
    paths: &IdentityPaths,
    addr: SocketAddr,
    input: &PageCaptureInput,
) -> Result<CapturePostResult, BrowserCaptureError> {
    if !addr.ip().is_loopback() {
        return Err(BrowserCaptureError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser page capture only posts to loopback addresses",
        )));
    }

    let body = format_page_capture(input)?;
    let token = paths.ensure_capture_token()?;
    post_capture_body(addr, &token, &body).await
}

pub fn bookmarklet(addr: SocketAddr) -> Result<String, BrowserCaptureError> {
    if !addr.ip().is_loopback() {
        return Err(BrowserCaptureError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser capture bookmarklet only targets loopback addresses",
        )));
    }

    let endpoint = format!("http://{addr}/capture");
    Ok(format!(
        "javascript:(async()=>{{const k=prompt('Identity capture token from ~/.identity/capture.token');if(!k)return;const s=String(window.getSelection()).trim();if(!s){{alert('Select page text before capturing.');return;}}const c=(v,n)=>String(v||'').replace(/\\s+/g,' ').trim().slice(0,n);const b=`Page title: ${{c(document.title,512)}}\\nPage URL: ${{c(location.href,2048)}}\\nSelected page text:\\n${{s}}`;try{{const r=await fetch('{endpoint}',{{method:'POST',headers:{{'X-Identity-Capture-Token':k.trim(),'Content-Type':'text/markdown'}},body:b}});alert(r.ok?'Identity captured selected page text.':'Identity capture failed: '+await r.text());}}catch(e){{alert('Identity capture failed: '+e);}}}})()"
    ))
}

pub fn clipboard_bookmarklet() -> String {
    format!(
        "javascript:(()=>{{const s=String(window.getSelection()).trim();if(!s){{alert('Select page text before copying.');return;}}const c=(v,n)=>String(v||'').replace(/\\s+/g,' ').trim().slice(0,n);const b=`{start}\\nPage title: ${{c(document.title,512)}}\\nPage URL: ${{c(location.href,2048)}}\\nSelected page text:\\n${{s}}\\n{end}`;const t=document.createElement('textarea');t.value=b;t.setAttribute('readonly','');t.style.position='fixed';t.style.left='-9999px';document.body.appendChild(t);t.select();let ok=false;try{{ok=document.execCommand('copy');}}catch(_e){{ok=false;}}document.body.removeChild(t);alert(ok?'Identity page capture copied. Run identityd capture-page --from-clipboard.':'Copy failed. Use the network bookmarklet or manual capture-page command.');}})()",
        start = CLIPBOARD_CAPTURE_START,
        end = CLIPBOARD_CAPTURE_END
    )
}

pub fn page_capture_from_clipboard_text(text: &str) -> PageCaptureInput {
    let trimmed = text.trim();
    if !trimmed.starts_with(CLIPBOARD_CAPTURE_START) {
        return PageCaptureInput {
            title: None,
            url: None,
            selected_text: trimmed.to_string(),
        };
    }

    let without_start = trimmed
        .strip_prefix(CLIPBOARD_CAPTURE_START)
        .unwrap_or(trimmed)
        .trim_start();
    let body = without_start
        .split(CLIPBOARD_CAPTURE_END)
        .next()
        .unwrap_or(without_start)
        .trim();

    PageCaptureInput {
        title: labelled_value(body, "Page title:"),
        url: labelled_value(body, "Page URL:"),
        selected_text: labelled_value(body, "Selected page text:").unwrap_or_default(),
    }
}

async fn post_capture_body(
    addr: SocketAddr,
    token: &str,
    body: &str,
) -> Result<CapturePostResult, BrowserCaptureError> {
    let body_bytes = body.as_bytes();
    let request = format!(
        "POST /capture HTTP/1.1\r\nHost: {addr}\r\nContent-Type: text/markdown; charset=utf-8\r\nX-Identity-Capture-Token: {token}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n",
        length = body_bytes.len()
    );

    let result = timeout(Duration::from_millis(REQUEST_TIMEOUT_MS), async {
        let stream = TcpStream::connect(addr).await?;
        write_all(&stream, request.as_bytes()).await?;
        write_all(&stream, body_bytes).await?;
        read_response(&stream).await
    })
    .await
    .map_err(|_| {
        BrowserCaptureError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "capture endpoint timed out",
        ))
    })??;

    let (status_code, response_body) = parse_http_response(&result)?;
    if !(200..300).contains(&status_code) {
        return Err(BrowserCaptureError::EndpointStatus {
            status_code,
            body: response_body,
        });
    }

    let captured_id = captured_id_from_response(&response_body);

    Ok(CapturePostResult {
        status_code,
        body: response_body,
        bytes_sent: body_bytes.len(),
        captured_id,
    })
}

async fn write_all(stream: &TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    let mut written = 0;

    while written < bytes.len() {
        stream.writable().await?;

        match stream.try_write(&bytes[written..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "socket closed",
                ))
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

async fn read_response(stream: &TcpStream) -> Result<Vec<u8>, BrowserCaptureError> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];

    loop {
        stream.readable().await?;

        match stream.try_read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > MAX_RESPONSE_BYTES {
                    return Err(BrowserCaptureError::BadEndpointResponse);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(BrowserCaptureError::Io(error)),
        }
    }

    Ok(buffer)
}

fn parse_http_response(bytes: &[u8]) -> Result<(u16, String), BrowserCaptureError> {
    let response = String::from_utf8_lossy(bytes);
    let mut lines = response.lines();
    let status_line = lines
        .next()
        .ok_or(BrowserCaptureError::BadEndpointResponse)?;
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .ok_or(BrowserCaptureError::BadEndpointResponse)?
        .parse::<u16>()
        .map_err(|_| BrowserCaptureError::BadEndpointResponse)?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.trim().to_string())
        .unwrap_or_default();

    Ok((status_code, body))
}

fn captured_id_from_response(body: &str) -> Option<i64> {
    let id_index = body.find("\"id\"")?;
    let after_key = &body[id_index + 4..];
    let colon_index = after_key.find(':')?;
    let after_colon = after_key[colon_index + 1..].trim_start();
    let digits = after_colon
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();

    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn normalize_field(input: &str, max_chars: usize) -> String {
    collapse_whitespace(input)
        .chars()
        .take(max_chars)
        .collect::<String>()
}

fn normalize_selected_text(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut blank_lines = 0_u8;
    let mut last_was_space = true;

    for character in input.chars() {
        match character {
            '\r' => {}
            '\n' => {
                if !output.ends_with('\n') {
                    output.push('\n');
                }
                blank_lines = blank_lines.saturating_add(1).min(2);
                last_was_space = true;
            }
            character if character.is_control() => {}
            character if character.is_whitespace() => {
                if !last_was_space {
                    output.push(' ');
                    last_was_space = true;
                }
            }
            character => {
                if blank_lines > 1 && !output.ends_with("\n\n") {
                    output.push('\n');
                }
                output.push(character);
                blank_lines = 0;
                last_was_space = false;
            }
        }
    }

    output.trim().to_string()
}

fn collapse_whitespace(input: &str) -> String {
    let mut compact = String::with_capacity(input.len());
    let mut last_was_whitespace = true;

    for character in input.chars() {
        if character.is_control() {
            continue;
        }
        if character.is_whitespace() {
            if !last_was_whitespace {
                compact.push(' ');
                last_was_whitespace = true;
            }
        } else {
            compact.push(character);
            last_was_whitespace = false;
        }
    }

    if last_was_whitespace && !compact.is_empty() {
        compact.pop();
    }

    compact
}

fn labelled_value(content: &str, label: &str) -> Option<String> {
    let start = content.find(label)? + label.len();
    let remainder = content[start..].trim_start();
    let next_label_offset = ["Page title:", "Page URL:", "Selected page text:"]
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

#[cfg(test)]
mod tests {
    use super::{
        bookmarklet, captured_id_from_response, clipboard_bookmarklet, format_page_capture,
        page_capture_from_clipboard_text, PageCaptureInput,
    };
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn formats_selected_page_text_with_bounded_metadata() {
        let body = format_page_capture(&PageCaptureInput {
            title: Some(" Identity   design notes ".to_string()),
            url: Some("https://example.test/page ".to_string()),
            selected_text: "  Use selected text only.\n\nNo ambient DOM capture.  ".to_string(),
        })
        .unwrap();

        assert!(body.contains("Page title: Identity design notes"));
        assert!(body.contains("Page URL: https://example.test/page"));
        assert!(body.contains("Selected page text:\nUse selected text only."));
        assert!(!body.contains("  "));
    }

    #[test]
    fn rejects_empty_or_sensitive_page_capture() {
        assert!(format_page_capture(&PageCaptureInput {
            title: None,
            url: Some("https://example.test/page".to_string()),
            selected_text: "   ".to_string(),
        })
        .is_err());

        assert!(format_page_capture(&PageCaptureInput {
            title: Some("secrets".to_string()),
            url: Some("https://example.test/.env".to_string()),
            selected_text: "plain text".to_string(),
        })
        .is_err());

        assert!(format_page_capture(&PageCaptureInput {
            title: Some("payment".to_string()),
            url: Some("https://example.test/page".to_string()),
            selected_text: "card 4111 1111 1111 1111".to_string(),
        })
        .is_err());
    }

    #[test]
    fn bookmarklet_is_user_triggered_selection_only_and_does_not_embed_token() {
        let script = bookmarklet(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)).unwrap();

        assert!(script.starts_with("javascript:"));
        assert!(script.contains("window.getSelection()"));
        assert!(script.contains("X-Identity-Capture-Token"));
        assert!(script.contains("prompt("));
        assert!(script.contains("k.trim()"));
        assert!(!script.contains("document.body.innerText"));
    }

    #[test]
    fn clipboard_bookmarklet_copies_selection_without_network_or_token() {
        let script = clipboard_bookmarklet();

        assert!(script.starts_with("javascript:"));
        assert!(script.contains("window.getSelection()"));
        assert!(script.contains("[IDENTITY-PAGE-CAPTURE]"));
        assert!(script.contains("document.execCommand('copy')"));
        assert!(!script.contains("fetch("));
        assert!(!script.contains("X-Identity-Capture-Token"));
    }

    #[test]
    fn parses_page_capture_clipboard_envelope() {
        let input = page_capture_from_clipboard_text(
            "[IDENTITY-PAGE-CAPTURE]\nPage title: Identity notes\nPage URL: https://example.test/notes\nSelected page text:\nLocal-first selected text.\n[IDENTITY-PAGE-CAPTURE-END]",
        );

        assert_eq!(input.title.as_deref(), Some("Identity notes"));
        assert_eq!(input.url.as_deref(), Some("https://example.test/notes"));
        assert_eq!(input.selected_text, "Local-first selected text.");

        let plain = page_capture_from_clipboard_text("plain selected text");
        assert_eq!(plain.title, None);
        assert_eq!(plain.url, None);
        assert_eq!(plain.selected_text, "plain selected text");
    }

    #[test]
    fn parses_capture_id_from_endpoint_response() {
        assert_eq!(
            captured_id_from_response(r#"{"captured":true,"id":42}"#),
            Some(42)
        );
        assert_eq!(captured_id_from_response(r#"{"error":"bad"}"#), None);
    }
}
