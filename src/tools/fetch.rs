use std::path::Path;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dom_smoothie::{Article, Config, Readability};
use serde_json::{Value, json};

use super::{CANCEL_REASON, Tool, arg_str, tool_limits, truncate_output};

pub struct FetchTool;

impl Tool for FetchTool {
    fn name(&self) -> &str {
        "fetch"
    }

    fn description(&self) -> &str {
        "Fetch the content of a webpage or remote document from an HTTP or HTTPS URL. By default, returns cleaned Markdown optimized for LLM consumption. Use HTML only when the page structure or raw markup is required."
    }

    fn instruction(&self) -> &str {
        "Fetch the content of a webpage or remote document over HTTP/HTTPS. Returns cleaned Markdown by default; pass format \"text\" for extracted plain text or \"html\" for raw markup."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The HTTP or HTTPS URL to fetch."
                },
                "format": {
                    "type": "string",
                    "description": "The format of the returned content.",
                    "enum": ["markdown", "text", "html"]
                }
            },
            "required": ["url"]
        })
    }

    fn execute_inner(
        &self,
        args: &Value,
        _workspace: &Path,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        execute(args, cancel)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Text,
    Html,
}

/// Content classification derived from the response's Content-Type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Html,
    /// JSON, plain text, Markdown, XML — returned verbatim.
    Text,
    Unsupported,
}

pub(super) fn execute(args: &Value, cancel: &AtomicBool) -> Result<String, String> {
    let max_body_bytes = tool_limits().fetch_max_body_bytes;

    let url = arg_str(args, "url").ok_or("Missing 'url' argument")?;
    let format = match arg_str(args, "format").unwrap_or("markdown") {
        "markdown" => Format::Markdown,
        "text" => Format::Text,
        "html" => Format::Html,
        other => {
            return Err(format!(
                "Invalid 'format' argument '{other}' (expected markdown, text, or html)"
            ));
        }
    };

    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Invalid URL '{url}': {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "Unsupported URL scheme '{}' (only http and https are allowed)",
            parsed.scheme()
        ));
    }

    tokio::runtime::Handle::current().block_on(async {
        // Race the HTTP request against user cancellation.
        let resp = tokio::select! {
            r = client()?.get(parsed.clone()).send() => {
                r.map_err(|e| format!("Failed to fetch {url}: {e}"))?
            }
            _ = cancel_signal(cancel) => {
                return Err(CANCEL_REASON.into());
            }
        };

        let status = resp.status();
        if !status.is_success() {
            return Err(format!("Failed to fetch {url}: HTTP {status}"));
        }

        // Refuse known-huge bodies before downloading.
        if let Some(len) = resp.content_length()
            && len > max_body_bytes as u64
        {
            return Err(format!(
                "Response body too large: {len} bytes (max {max_body_bytes})"
            ));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();

        // Download the body, with cancellation support.
        let body = tokio::select! {
            r = resp.text() => {
                r.map_err(|e| format!("Failed to read response body: {e}"))?
            }
            _ = cancel_signal(cancel) => {
                return Err(CANCEL_REASON.into());
            }
        };

        // Cap oversized bodies at a valid UTF-8 boundary.
        let body = truncate_body(body, max_body_bytes);

        let output = match classify(mime_type(&content_type), &body) {
            ContentKind::Html => convert_html(&body, url, format)?,
            ContentKind::Text => body,
            ContentKind::Unsupported => {
                return Err(format!(
                    "Unsupported content type '{content_type}' — only HTML, JSON, and text are supported"
                ));
            }
        };
        Ok(truncate_output(output))
    })
}

// ── async helpers ──────────────────────────────────────────────────

/// Returns a future that completes when `cancel` becomes true.
async fn cancel_signal(cancel: &AtomicBool) {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Shared async client: keeps one connection pool across all fetch calls.
fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent(crate::app_title())
            .timeout(Duration::from_millis(tool_limits().fetch_timeout_ms))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {e}"))
    });
    CLIENT.as_ref().map_err(Clone::clone)
}

// ── body helpers ───────────────────────────────────────────────────

/// Truncate `s` to at most `max_bytes` bytes on a UTF-8 boundary.
pub fn truncate_body(s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = s;
    truncated.truncate(end);
    truncated
}

/// Extract the bare MIME type from a Content-Type header value.
pub fn mime_type(content_type: &str) -> &str {
    content_type.split(';').next().unwrap_or("").trim()
}

/// Classify the response content; sniff the body when the header is missing.
pub fn classify(mime: &str, body: &str) -> ContentKind {
    if mime.is_empty() {
        return if looks_like_html(body) {
            ContentKind::Html
        } else {
            ContentKind::Text
        };
    }
    let mime = mime.to_ascii_lowercase();
    if mime == "text/html" || mime == "application/xhtml+xml" {
        ContentKind::Html
    } else if is_textual_mime(&mime) {
        ContentKind::Text
    } else {
        ContentKind::Unsupported
    }
}

fn is_textual_mime(mime: &str) -> bool {
    mime.starts_with("text/")
        || mime == "application/json"
        || mime.ends_with("+json")
        || mime == "application/xml"
        || mime.ends_with("+xml")
        || mime == "application/javascript"
}

/// Cheap sniff for HTML when the server omits Content-Type.
pub fn looks_like_html(body: &str) -> bool {
    let mut rest = body.trim_start();
    // Skip leading HTML comments like `<!-- license banner -->`.
    while let Some(after) = rest.strip_prefix("<!--") {
        match after.find("-->") {
            Some(end) => rest = after[end + 3..].trim_start(),
            None => return false,
        }
    }
    starts_with_ignore_ascii_case(rest, "<!doctype html")
        || starts_with_ignore_ascii_case(rest, "<html")
}

fn starts_with_ignore_ascii_case(s: &str, prefix: &str) -> bool {
    s.as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

// ── rendering ──────────────────────────────────────────────────────

/// Render an HTML page in the requested format: raw markup, extracted plain
/// text, or cleaned Markdown (readability extraction with a whole-page
/// fallback).
pub fn convert_html(html: &str, url: &str, format: Format) -> Result<String, String> {
    if format == Format::Html {
        return Ok(html.to_string());
    }

    if let Some(article) = extract_article(html, url) {
        return match format {
            Format::Text => Ok(article.text_content.trim().to_string()),
            Format::Markdown => article_markdown(&article),
            Format::Html => unreachable!("raw HTML handled above"),
        };
    }

    // Readability failed or the page is not article-like: convert the whole
    // page, dropping boilerplate tags.
    let markdown = full_page_markdown(html)?;
    match format {
        Format::Text => Ok(markdown_to_text(&markdown)),
        Format::Markdown => Ok(markdown),
        Format::Html => unreachable!("raw HTML handled above"),
    }
}

/// Extract the main article content via readability. Returns `None` when the
/// page is not article-like or extraction fails.
fn extract_article(html: &str, url: &str) -> Option<Article> {
    let cfg = Config {
        max_elements_to_parse: 9000,
        ..Default::default()
    };
    let mut readability = Readability::new(html, Some(url), Some(cfg)).ok()?;
    if !readability.is_probably_readable() {
        return None;
    }
    let article = readability.parse().ok()?;
    if article.content.trim().is_empty() {
        return None;
    }
    Some(article)
}

/// Convert extracted article HTML to Markdown, prefixed with the title.
fn article_markdown(article: &Article) -> Result<String, String> {
    let body = htmd::HtmlToMarkdown::new()
        .convert(&article.content)
        .map_err(|e| format!("Failed to convert article HTML to Markdown: {e}"))?;
    let title = article.title.trim();
    if title.is_empty() {
        Ok(body)
    } else {
        Ok(format!("# {title}\n\n{body}"))
    }
}

/// Convert a full page to Markdown, skipping boilerplate tags.
fn full_page_markdown(html: &str) -> Result<String, String> {
    htmd::HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "nav", "header", "footer"])
        .build()
        .convert(html)
        .map_err(|e| format!("Failed to convert page HTML to Markdown: {e}"))
}

/// Strip Markdown syntax, keeping readable plain text: inline formatting
/// markers and link URLs are dropped, block boundaries become newlines, and
/// list items keep a `-` bullet.
fn markdown_to_text(markdown: &str) -> String {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    let mut text = String::with_capacity(markdown.len());
    for event in Parser::new(markdown) {
        match event {
            Event::Text(t) | Event::Code(t) => text.push_str(&t),
            Event::SoftBreak | Event::HardBreak => text.push('\n'),
            Event::Start(Tag::Item) => text.push_str("- "),
            // Code block contents already end with a newline.
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::CodeBlock,
            ) if !text.ends_with('\n') => text.push('\n'),
            _ => {}
        }
    }
    text.trim().to_string()
}
