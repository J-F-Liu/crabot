//! Integration tests for the `fetch` tool in `crabot::tools::fetch`.

use crabot::tools::fetch::{
    ContentKind, Format, classify, convert_html, looks_like_html, mime_type, truncate_body,
};
use crabot::tools::tool_limits;

const ARTICLE_URL: &str = "https://example.com/post";

/// Article-like page: readability should extract only the main content.
const ARTICLE_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><title>Test Article - Example</title></head>
<body>
<nav><a href="/">Home</a> | <a href="/about">About</a></nav>
<article>
<h1>Test Article</h1>
<p>This is the first paragraph of a sufficiently long article body, written to
resemble real prose, with several clauses, commas, and enough words to convince
the readability scorer that this page contains genuine article content worth
extracting and keeping for the reader.</p>
<p>This is the second paragraph, equally verbose, continuing the discussion with
more detail, more commas, and more filler text, so that the extracted content
clearly stands out from the surrounding boilerplate navigation and footer.</p>
</article>
<footer>Copyright 2026 Example Corp</footer>
</body>
</html>"#;

/// Tiny non-article page: readability declines, whole-page fallback runs.
const FALLBACK_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><script>let tracker = 1;</script></head>
<body>
<nav>Menu</nav>
<h2>Status</h2>
<p>Hello <b>fallback</b>, see <a href="https://example.com/docs">docs</a>.</p>
<footer>Legal</footer>
</body>
</html>"#;

#[test]
fn mime_type_strips_parameters() {
    assert_eq!(mime_type("text/html; charset=utf-8"), "text/html");
    assert_eq!(mime_type("application/json"), "application/json");
    assert_eq!(mime_type(""), "");
}

#[test]
fn classify_routes_known_types() {
    assert_eq!(classify("text/html", ""), ContentKind::Html);
    assert_eq!(classify("application/xhtml+xml", ""), ContentKind::Html);
    assert_eq!(classify("application/json", ""), ContentKind::Text);
    assert_eq!(classify("application/vnd.api+json", ""), ContentKind::Text);
    assert_eq!(classify("text/markdown", ""), ContentKind::Text);
    assert_eq!(classify("application/rss+xml", ""), ContentKind::Text);
    assert_eq!(classify("application/pdf", ""), ContentKind::Unsupported);
    assert_eq!(classify("image/png", ""), ContentKind::Unsupported);
}

#[test]
fn classify_sniffs_missing_content_type() {
    assert_eq!(classify("", "  <!DOCTYPE html><html>"), ContentKind::Html);
    assert_eq!(
        classify("", "\n<HTML><body>x</body></HTML>"),
        ContentKind::Html
    );
    assert_eq!(classify("", "just plain words"), ContentKind::Text);
}

#[test]
fn looks_like_html_skips_leading_comments() {
    assert!(looks_like_html("<!-- banner --><HTML>"));
    assert!(looks_like_html(" <!--a--> <!--b-->\n<!doctype HTML>"));
    assert!(!looks_like_html("<!-- unterminated comment"));
    assert!(!looks_like_html("<!-- c --> plain text"));
}

#[test]
fn markdown_format_extracts_article() {
    let md = convert_html(ARTICLE_HTML, ARTICLE_URL, Format::Markdown).unwrap();
    assert!(md.contains("# Test Article"), "missing title: {md}");
    assert!(md.contains("first paragraph"));
    assert!(!md.contains("Copyright"), "boilerplate leaked: {md}");
}

#[test]
fn text_format_extracts_article_text() {
    let text = convert_html(ARTICLE_HTML, ARTICLE_URL, Format::Text).unwrap();
    assert!(text.contains("first paragraph"));
    assert!(!text.contains("Copyright"), "boilerplate leaked: {text}");
}

#[test]
fn html_format_returns_raw_markup() {
    let raw = convert_html(ARTICLE_HTML, ARTICLE_URL, Format::Html).unwrap();
    assert_eq!(raw, ARTICLE_HTML);
}

#[test]
fn non_article_page_uses_whole_page_fallback() {
    let md = convert_html(FALLBACK_HTML, ARTICLE_URL, Format::Markdown).unwrap();
    assert!(md.contains("## Status"), "heading lost: {md}");
    assert!(md.contains("**fallback**"), "bold lost: {md}");
    assert!(md.contains("[docs](https://example.com/docs)"));
    assert!(!md.contains("tracker"), "script leaked: {md}");
    assert!(!md.contains("Menu"), "nav leaked: {md}");
    assert!(!md.contains("Legal"), "footer leaked: {md}");
}

#[test]
fn text_format_fallback_strips_markdown() {
    let text = convert_html(FALLBACK_HTML, ARTICLE_URL, Format::Text).unwrap();
    assert!(text.contains("Status"));
    assert!(text.contains("Hello fallback, see docs."));
    for syntax in ["#", "**", "]("] {
        assert!(
            !text.contains(syntax),
            "markdown syntax '{syntax}' leaked: {text}"
        );
    }
    assert!(!text.contains("tracker"), "script leaked: {text}");
}

#[test]
fn truncate_body_respects_char_boundaries() {
    let max = tool_limits().fetch_max_body_bytes;
    // "é" is 2 bytes in UTF-8
    let s = "a".repeat(max - 1) + "é";
    assert!(s.len() > max);
    let t = truncate_body(s, max);
    // Should not panic and should be valid UTF-8
    assert!(t.len() <= max);
    assert!(t.ends_with('a'));
}
