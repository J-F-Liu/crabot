//! Static HTML export for the currently-viewed session, mirroring the center
//! pane's layout and collapse state with native `<details>` elements.

use std::collections::HashSet;
use std::fmt::Write as _;

use crabot::chat::{Dialog, Turn, TurnBody, markdown_options, streaming_tool_ids, tool_items};
use crabot::session::Session;
use crabot::tools::edit::EditParam;
use crabot::tools::todo::{TodoItem, TodoStatus};
use pulldown_cmark::{Event, Tag, html};
use serde_json::Value;

use super::theme::is_dark;
use super::tool_message::fmt_arg;

/// Write a formatted string into `out`, ignoring the infallible `fmt::Error`.
macro_rules! w {
    ($out:expr, $($arg:tt)*) => {{
        let _ = write!($out, $($arg)*);
    }};
}

/// Content-Security-Policy for the exported page: no scripts, no remote
/// fetches (images included) — the file is opened in a real browser with
/// LLM-derived content, so it must not execute anything or phone home.
const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:";

/// Outcome of an HTML export attempt.
#[derive(Debug, Clone)]
pub(crate) enum ExportOutcome {
    /// The save dialog was cancelled.
    Cancelled,
    /// The file was written and opened in the default browser.
    Saved,
    /// The file was written but could not be opened in a browser.
    SavedButNotOpened(String),
    /// The export failed (e.g. the file could not be written).
    Failed(String),
}

/// Shared rendering context for a single export pass.
struct RenderCtx<'a> {
    expanded_dialogs: &'a HashSet<usize>,
    expanded_turns: &'a HashSet<(usize, usize)>,
}

/// Render the whole session as a standalone HTML document.
pub(crate) fn render_session_html(
    session: &Session,
    title: &str,
    expanded_dialogs: &HashSet<usize>,
    expanded_turns: &HashSet<(usize, usize)>,
) -> String {
    let ctx = RenderCtx {
        expanded_dialogs,
        expanded_turns,
    };
    let mut body = String::new();

    // ── header ────────────────────────────────────────────────────
    w!(body, "<header class=\"session-header\">");
    w!(body, "<h1>{}</h1>", escape_html(title));
    let mut meta = Vec::new();
    if let Some(model) = session.model.as_ref() {
        meta.push(format!("Model: {}", escape_html(&model.model_id)));
    }
    meta.push(format!("Created: {}", escape_html(&session.created_at)));
    if !meta.is_empty() {
        w!(
            body,
            "<div class=\"session-meta\">{}</div>",
            meta.join(" · ")
        );
    }
    w!(body, "</header>");

    // ── dialogs ───────────────────────────────────────────────────
    w!(body, "<div class=\"dialogs\">");
    let mut flat_idx = 0usize;
    for (di, dialog) in session.dialogs.iter().enumerate() {
        render_dialog(dialog, di, &mut flat_idx, &ctx, &mut body);
    }
    w!(body, "</div>");

    let theme = if is_dark() { "dark" } else { "light" };
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\" data-theme=\"{theme}\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">\n<title>{}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n<main class=\"session\">{body}</main>\n</body>\n</html>\n",
        escape_html(title),
    )
}

/// Derive a filesystem-safe default file name from the session title.
pub(crate) fn default_export_filename(title: &str) -> String {
    let mut name: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                c
            }
        })
        .collect();
    name = name.trim().trim_matches('.').to_string();
    name = name.chars().take(120).collect();
    // Truncation can leave a trailing dot, which is invalid on some filesystems.
    name = name.trim_matches('.').to_string();
    if name.is_empty() {
        "session.html".to_string()
    } else {
        format!("{name}.html")
    }
}

// ── dialog rendering ──────────────────────────────────────────────

fn render_dialog(
    dialog: &Dialog,
    di: usize,
    flat_idx: &mut usize,
    ctx: &RenderCtx<'_>,
    out: &mut String,
) {
    let collapsed = !ctx.expanded_dialogs.contains(&di);
    let indicator = if collapsed { "⊞" } else { "⊟" };
    let title = dialog.display_title(di);
    let turn_count = dialog.turns.len();
    let turn_label = if turn_count == 1 { "turn" } else { "turns" };

    let open = if collapsed { "" } else { " open" };
    w!(out, "<details class=\"dialog\"{open}>");
    w!(out, "<summary class=\"dialog-header\">");
    w!(out, "<span class=\"indicator\">{indicator}</span>");
    if let Some(mode) = dialog.mode.as_ref() {
        w!(
            out,
            "<span class=\"mode-badge\">{}</span>",
            escape_html(mode.name.as_ref())
        );
    }
    w!(
        out,
        "<span class=\"dialog-title\">{}</span>",
        escape_html(&title)
    );
    w!(
        out,
        "<span class=\"turn-count\">{turn_count} {turn_label}</span>"
    );
    w!(out, "</summary>");

    // Always emit the body so the native `<details>` element can expand a
    // collapsed dialog; the `open` attribute controls initial visibility.
    let streaming_ids = streaming_tool_ids(dialog);
    w!(out, "<div class=\"dialog-body\">");
    for turn in &dialog.turns {
        let i = *flat_idx;
        *flat_idx += 1;
        render_turn(turn, i, &streaming_ids, ctx, out);
    }
    w!(out, "</div>");
    w!(out, "</details>");
}

// ── turn rendering ────────────────────────────────────────────────

fn render_turn(
    turn: &Turn,
    i: usize,
    streaming_ids: &HashSet<&str>,
    ctx: &RenderCtx<'_>,
    out: &mut String,
) {
    match &turn.body {
        TurnBody::Tool(_) | TurnBody::Temp(_) => {
            render_tool_turn(turn, i, streaming_ids, ctx, out);
        }
        TurnBody::Text(_) => render_text_turn(turn, i, ctx, out),
    }
}

fn render_text_turn(turn: &Turn, i: usize, ctx: &RenderCtx<'_>, out: &mut String) {
    let TurnBody::Text(tc) = &turn.body else {
        unreachable!("render_text_turn called on non-Text turn")
    };

    let role_label = turn.role_label();
    // Bubble and badge CSS classes both match the lowercased role label.
    let css_class = role_label.to_lowercase();

    w!(out, "<div class=\"bubble {css_class}\">");
    if let Some(reasoning) = tc.reasoning.as_deref() {
        // Reasoning defaults to expanded, so membership inverts.
        let expanded = !ctx.expanded_turns.contains(&(i, 0));
        let indicator = if expanded { "▼" } else { "⏵" };
        let open = if expanded { " open" } else { "" };
        w!(out, "<details class=\"reasoning\"{open}>");
        w!(out, "<summary class=\"turn-header\">");
        render_turn_header(
            role_label,
            &css_class,
            Some(indicator),
            &turn.timestamp,
            out,
        );
        w!(out, "</summary>");
        w!(
            out,
            "<div class=\"reasoning-body markdown\">{}</div>",
            markdown_to_html(reasoning)
        );
        w!(out, "</details>");
    } else {
        w!(out, "<div class=\"turn-header\">");
        render_turn_header(role_label, &css_class, None, &turn.timestamp, out);
        w!(out, "</div>");
    }
    w!(
        out,
        "<div class=\"content markdown\">{}</div>",
        markdown_to_html(&tc.content)
    );
    w!(out, "</div>");
}

fn render_badge(role_label: &str, badge_class: &str, out: &mut String) {
    w!(
        out,
        "<span class=\"badge badge-{badge_class}\">{}</span>",
        escape_html(role_label)
    );
}

/// Badge + optional indicator + timestamp, without the wrapping header tag.
fn render_turn_header(
    role_label: &str,
    badge_class: &str,
    indicator: Option<&str>,
    timestamp: &str,
    out: &mut String,
) {
    render_badge(role_label, badge_class, out);
    if let Some(indicator) = indicator {
        w!(out, "<span class=\"indicator\">{indicator}</span>");
    }
    w!(
        out,
        "<span class=\"timestamp\">{}</span>",
        escape_html(timestamp)
    );
}

fn render_tool_turn(
    turn: &Turn,
    i: usize,
    streaming_ids: &HashSet<&str>,
    ctx: &RenderCtx<'_>,
    out: &mut String,
) {
    let items = tool_items(turn, streaming_ids);
    if items.is_empty() {
        return;
    }

    w!(out, "<div class=\"bubble tool\">");
    for (idx, (name, args, result, ts, streaming)) in items.into_iter().enumerate() {
        if idx > 0 {
            w!(out, "<div class=\"tool-spacer\"></div>");
        }

        let badge = format!("Tool - {name}");
        let completed = result.is_some() && !streaming;
        let (status_icon, status_class) = match (result, streaming) {
            (Some(Ok(_)), false) => ("✓", "ok"),
            (Some(Err(_)), false) => ("✗", "err"),
            _ => ("⏳", "pending"),
        };

        // Running tool: header + args + live output, no expand/collapse.
        if streaming {
            w!(out, "<div class=\"tool-header\">");
            render_tool_header(&badge, status_icon, status_class, ts, None, out);
            w!(out, "</div>");
            render_args_rows(name, args, out);
            if let Some(Ok(buffer)) = result {
                render_streaming_result(buffer, out);
            }
            continue;
        }

        // Completed ask tool: question + answer without expand/collapse.
        if name == "ask" && completed {
            w!(out, "<div class=\"tool-header\">");
            render_tool_header(&badge, status_icon, status_class, ts, None, out);
            w!(out, "</div>");
            render_ask_result(args, result.unwrap(), out);
            continue;
        }

        let expanded = completed && ctx.expanded_turns.contains(&(i, idx));
        let indicator = if expanded { "▼" } else { "⏵" };

        if completed {
            let open = if expanded { " open" } else { "" };
            w!(out, "<details class=\"tool-item\"{open}>");
            w!(out, "<summary class=\"tool-header\">");
            render_tool_header(&badge, status_icon, status_class, ts, Some(indicator), out);
            w!(out, "</summary>");
            w!(out, "<div class=\"tool-detail\">");
            render_args_rows(name, args, out);
            render_result(result.unwrap(), out);
            w!(out, "</div>");
            w!(out, "</details>");
            // Shown only while the adjacent `<details>` is collapsed.
            w!(out, "<div class=\"tool-preview\">");
            render_args_preview(name, args, out);
            w!(out, "</div>");
        } else {
            // Pending call: plain header + compact args, no expand control.
            w!(out, "<div class=\"tool-header\">");
            render_tool_header(&badge, status_icon, status_class, ts, None, out);
            w!(out, "</div>");
            render_args_preview(name, args, out);
        }
    }
    w!(out, "</div>");
}

/// Badge + status + optional indicator + timestamp, without the wrapping
/// header tag — callers must wrap the output in a `.tool-header` element.
fn render_tool_header(
    badge: &str,
    status_icon: &str,
    status_class: &str,
    ts: &str,
    indicator: Option<&str>,
    out: &mut String,
) {
    render_badge(badge, "tool", out);
    w!(
        out,
        "<span class=\"status status-{status_class}\">{status_icon}</span>"
    );
    if let Some(indicator) = indicator {
        w!(out, "<span class=\"indicator\">{indicator}</span>");
    }
    w!(out, "<span class=\"timestamp\">{}</span>", escape_html(ts));
}

// ── tool arguments / results ──────────────────────────────────────

fn render_args_preview(name: &str, args: &Value, out: &mut String) {
    if name == "edit" || name == "write" {
        if let Some(path) = args
            .as_object()
            .and_then(|map| map.get("path"))
            .and_then(|v| v.as_str())
        {
            render_arg_row("path", path, out);
        }
    } else {
        render_args_rows(name, args, out);
    }
}

fn render_args_rows(tool_name: &str, args: &Value, out: &mut String) {
    let Some(map) = args.as_object() else {
        return;
    };

    if tool_name == "todo"
        && let Some(items) = map.get("items").and_then(|v| v.as_array())
    {
        render_todo_table(items, out);
        return;
    }

    let has_offset_and_limit = map.contains_key("offset") && map.contains_key("limit");
    if has_offset_and_limit {
        let off = fmt_arg(map, "offset");
        let lim = fmt_arg(map, "limit");
        render_arg_line(&format!("offset: {off}  limit: {lim}"), out);
    }

    for (key, value) in map {
        if has_offset_and_limit && (key == "offset" || key == "limit") {
            continue;
        }
        if key == "edits"
            && let Some(edits) = value.as_array()
        {
            render_edits_table(key, edits, out);
            continue;
        }
        let display = value
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| value.to_string());
        render_arg_row(key, &display, out);
    }
}

fn render_arg_row(key: &str, value: &str, out: &mut String) {
    w!(
        out,
        "<div class=\"arg-row\"><span class=\"arg-key\">{}:</span><span class=\"arg-value\">{}</span></div>",
        escape_html(key),
        escape_html(value)
    );
}

fn render_arg_line(value: &str, out: &mut String) {
    w!(
        out,
        "<div class=\"arg-row\"><span class=\"arg-value\">{}</span></div>",
        escape_html(value)
    );
}

fn render_edits_table(key: &str, edits: &[Value], out: &mut String) {
    w!(out, "<div class=\"edits\">");
    w!(
        out,
        "<div class=\"edits-header\"><span class=\"arg-key\">{}:</span><span class=\"arg-value muted\">{} edit(s)</span></div>",
        escape_html(key),
        edits.len()
    );
    for (idx, edit) in edits.iter().enumerate() {
        w!(
            out,
            "<div class=\"edit-block\"><span class=\"edit-index\">Edit #{}</span>",
            idx + 1
        );
        match serde_json::from_value::<EditParam>(edit.clone()) {
            Ok(param) => {
                render_diff_row("−", "del", &param.old_text, out);
                render_diff_row("+", "add", &param.new_text, out);
            }
            Err(_) => render_diff_row("⚠", "del", &edit.to_string(), out),
        }
        w!(out, "</div>");
    }
    w!(out, "</div>");
}

fn render_diff_row(marker: &str, class: &str, content: &str, out: &mut String) {
    w!(
        out,
        "<div class=\"diff-row diff-{class}\"><span class=\"diff-marker\">{marker}</span><span class=\"diff-content\">{}</span></div>",
        escape_html(content)
    );
}

fn render_todo_table(items: &[Value], out: &mut String) {
    w!(out, "<div class=\"todo-table\">");
    w!(
        out,
        "<div class=\"todo-header\"><span class=\"todo-text\">Text</span><span class=\"todo-status\">Status</span></div>"
    );
    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            w!(out, "<div class=\"todo-divider\"></div>");
        }
        match serde_json::from_value::<TodoItem>(item.clone()) {
            Ok(todo) => {
                let (status, class) = match todo.status {
                    TodoStatus::Pending => ("pending", "todo-pending"),
                    TodoStatus::InProgress => ("in progress", "todo-in-progress"),
                    TodoStatus::Completed => ("completed", "todo-completed"),
                };
                let content = format!("{}{}", "  ".repeat(todo.depth as usize), todo.text);
                w!(
                    out,
                    "<div class=\"todo-row\"><span class=\"todo-text\">{}</span><span class=\"todo-status {class}\">{status}</span></div>",
                    escape_html(&content)
                );
            }
            Err(_) => w!(
                out,
                "<div class=\"todo-row\"><span class=\"todo-text\">{}</span><span class=\"todo-status todo-invalid\">⚠ invalid</span></div>",
                escape_html(&item.to_string())
            ),
        }
    }
    w!(out, "</div>");
}

fn render_result(result: &Result<String, String>, out: &mut String) {
    let (display, is_ok) = match result {
        Ok(s) => (s.as_str(), true),
        Err(e) => (e.as_str(), false),
    };
    let (label, class) = if is_ok {
        ("Result", "ok")
    } else {
        ("Error", "err")
    };
    render_result_box(label, class, display, out);
}

fn render_streaming_result(buffer: &str, out: &mut String) {
    render_result_box("Running…", "ok", buffer, out);
}

fn render_result_box(label: &str, class: &str, text: &str, out: &mut String) {
    w!(
        out,
        "<div class=\"result-box {class}\"><div class=\"result-label\">{label}</div><pre class=\"result-text\">{}</pre></div>",
        escape_html(text)
    );
}

fn render_ask_result(args: &Value, result: &Result<String, String>, out: &mut String) {
    let question = args
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let options: Vec<&str> = args
        .get("options")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let (answer, is_ok) = match result {
        Ok(s) => (s.as_str(), true),
        Err(e) => (e.as_str(), false),
    };
    let label = if is_ok { "Answer" } else { "Error" };
    let class = if is_ok { "ok" } else { "err" };

    w!(out, "<div class=\"ask-result\">");
    w!(
        out,
        "<div class=\"ask-question\">{}</div>",
        escape_html(question)
    );
    if options.is_empty() {
        render_ask_answer(label, class, answer, out);
    } else {
        let matched = options.contains(&answer);
        let header = if matched {
            format!("{label}:")
        } else {
            "Options:".to_string()
        };
        w!(out, "<div class=\"result-label {class}\">{header}</div>");
        w!(out, "<div class=\"ask-options\">");
        for &opt in &options {
            let check = if opt == answer { "✓" } else { " " };
            w!(
                out,
                "<div class=\"ask-option\"><span class=\"ask-check\">{check}</span><span class=\"ask-option-text\">{}</span></div>",
                escape_html(opt)
            );
        }
        w!(out, "</div>");
        if !matched {
            render_ask_answer(label, class, answer, out);
        }
    }
    w!(out, "</div>");
}

fn render_ask_answer(label: &str, class: &str, answer: &str, out: &mut String) {
    w!(
        out,
        "<div class=\"ask-answer\"><span class=\"result-label {class}\">{label}: </span><span>{}</span></div>",
        escape_html(answer)
    );
}

// ── markdown / escaping ───────────────────────────────────────────

/// Linkify bare URLs (as the center pane does) then render markdown to HTML.
/// Raw HTML is escaped and link/image destinations are restricted to safe
/// schemes, so LLM output can't execute markup or navigate the exported page
/// (opened in a real browser) to `javascript:`/`file:` URLs.
fn markdown_to_html(text: &str) -> String {
    let (linked, _) = crabot::chat::linkify_urls(text);
    let parser =
        pulldown_cmark::Parser::new_ext(&linked, markdown_options()).map(|event| match event {
            Event::Html(text) => Event::Html(escape_html(&text).into()),
            Event::InlineHtml(text) => Event::InlineHtml(escape_html(&text).into()),
            // Neutralize unsafe destinations; CSP can't restrict link navigation.
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => Event::Start(Tag::Link {
                link_type,
                dest_url: if is_safe_link_dest(&dest_url) {
                    dest_url
                } else {
                    "#".into()
                },
                title,
                id,
            }),
            // Images are never fetched by the app's markdown viewer either;
            // remote http(s) images are additionally blocked by the page CSP.
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => Event::Start(Tag::Image {
                link_type,
                dest_url: if is_safe_image_dest(&dest_url) {
                    dest_url
                } else {
                    "".into()
                },
                title,
                id,
            }),
            other => other,
        });
    let mut html = String::new();
    html::push_html(&mut html, parser);
    html
}

/// Only `http(s)` link destinations are kept; everything else (`javascript:`,
/// `file:`, `data:`, …) is rewritten to a no-op fragment.
fn is_safe_link_dest(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Image destinations may additionally be `data:` URIs (no network fetch).
fn is_safe_image_dest(url: &str) -> bool {
    is_safe_link_dest(url) || url.starts_with("data:")
}

/// Escape `& < > " '` for HTML text and quoted attributes.
fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    let _ = pulldown_cmark_escape::escape_html(&mut escaped, text);
    escaped
}

// ── styles ────────────────────────────────────────────────────────

const STYLE: &str = r#"
:root {
  --bg: #F0F0F0;
  --card: #FFFFFF;
  --header-bg: #F5F5F5;
  --surface: #E8E8E8;
  --text: #333333;
  --muted: #666666;
  --border: #E0E0E0;
  --primary: #1A9A8C;
  --success: #2EB67F;
  --danger: #E54D4D;
  --tool-accent: #D9A558;
  --user-bg: #EFF5FD;
  --assistant-bg: #F3F7F6;
  --tool-bg: #FBFBF8;
  --tool-content-bg: #FFF8F2;
  --tool-content-border: #F4F0EC;
  --diff-del-bg: #FFF0F0;
  --diff-add-bg: #F0FAF4;
  --reasoning-bg: rgba(0, 0, 0, 0.035);
  --reasoning-border: rgba(0, 0, 0, 0.06);
  --user-badge: #4A90D9;
  --tool-badge: #D08F33;
}
[data-theme="dark"] {
  --bg: #14161A;
  --card: #232730;
  --header-bg: #24282F;
  --surface: #2A2F38;
  --text: #E2E5EA;
  --muted: #9BA1AB;
  --border: #343945;
  --user-bg: #1E2938;
  --assistant-bg: #21252C;
  --tool-bg: #282520;
  --tool-content-bg: #2B2721;
  --tool-content-border: #3D382E;
  --diff-del-bg: #3D2025;
  --diff-add-bg: #1E2D25;
  --reasoning-bg: rgba(255, 255, 255, 0.05);
  --reasoning-border: rgba(255, 255, 255, 0.09);
}

* { box-sizing: border-box; }
body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
  font: 14px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
}
.session { max-width: 860px; margin: 0 auto; padding: 24px 16px 48px; }

.session-header {
  background: var(--header-bg);
  border: 1px solid var(--border);
  padding: 6px 14px;
  margin-bottom: 12px;
}
.session-header h1 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  white-space: pre-wrap;
  word-break: break-word;
}
.session-meta { margin-top: 4px; font-size: 12px; color: var(--muted); }

.dialogs { display: flex; flex-direction: column; gap: 8px; }

.dialog { background: var(--card); border-radius: 10px; overflow: hidden; }
.dialog > summary { list-style: none; }
.dialog > summary::-webkit-details-marker { display: none; }
.dialog-header { display: flex; align-items: center; gap: 8px; padding: 8px 12px; cursor: pointer; }
.dialog-header .indicator { color: var(--primary); font-size: 10px; }
.dialog-title { font-weight: 700; font-size: 13px; word-break: break-word; }
.turn-count {
  margin-left: auto;
  background: var(--surface);
  color: var(--muted);
  border-radius: 10px;
  padding: 2px 8px;
  font-size: 10px;
  white-space: nowrap;
}
.mode-badge {
  background: rgba(26, 154, 140, 0.12);
  color: var(--primary);
  font-size: 11px;
  font-weight: 600;
  border-radius: 8px;
  padding: 2px 8px;
  white-space: nowrap;
}
.dialog-body { display: flex; flex-direction: column; gap: 8px; padding: 8px 10px 10px; }

.bubble { border-radius: 12px; padding: 8px 12px; }
.bubble.user { background: var(--user-bg); }
.bubble.assistant, .bubble.system { background: var(--assistant-bg); }
.bubble.tool { background: var(--tool-bg); border-radius: 8px; }

.turn-header { display: flex; align-items: center; gap: 6px; }
.turn-header .indicator { color: var(--primary); font-size: 10px; }
.timestamp { margin-left: auto; font-size: 11px; color: var(--muted); white-space: nowrap; }

.badge { font-size: 12px; font-weight: 700; white-space: nowrap; }
.badge-user { color: var(--user-badge); }
.badge-assistant { color: var(--primary); }
.badge-system { color: var(--muted); }
.badge-tool { color: var(--tool-badge); }

.content { margin-top: 8px; }

.reasoning > summary { list-style: none; }
.reasoning > summary::-webkit-details-marker { display: none; }
.reasoning-body {
  background: var(--reasoning-bg);
  border: 1px solid var(--reasoning-border);
  border-radius: 6px;
  padding: 6px 10px;
  margin-top: 8px;
  font-size: 13px;
}

.markdown > :first-child { margin-top: 0; }
.markdown > :last-child { margin-bottom: 0; }
.markdown p { margin: 0 0 8px; }
.markdown pre {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 8px 10px;
  overflow-x: auto;
  margin: 0 0 8px;
}
.markdown code {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
  font-size: 13px;
}
.markdown pre code { background: none; padding: 0; }
.markdown :not(pre) > code { background: transparent; padding: 0; }
.markdown table { border-collapse: collapse; margin: 0 0 8px; }
.markdown th, .markdown td { border: 1px solid var(--border); padding: 4px 8px; }
.markdown a { color: var(--primary); }
.markdown h1, .markdown h2, .markdown h3, .markdown h4, .markdown h5, .markdown h6 {
  font-size: 1em;
  margin: 8px 0;
}

.tool-spacer { height: 8px; }
.tool-header { display: flex; align-items: center; gap: 6px; }
.tool-item > summary { list-style: none; cursor: pointer; }
.tool-item > summary::-webkit-details-marker { display: none; }
.tool-header .indicator { color: var(--tool-accent); font-size: 10px; }
.status { font-size: 12px; }
.status.ok { color: var(--success); font-weight: 700; }
.status.err { color: var(--danger); font-weight: 700; }
.status.pending { color: var(--muted); }

.tool-detail { display: flex; flex-direction: column; gap: 6px; margin-top: 8px; }
.tool-preview { display: none; flex-direction: column; gap: 4px; margin-top: 8px; }
details.tool-item:not([open]) + .tool-preview { display: flex; }

.arg-row { display: flex; gap: 8px; align-items: baseline; }
.arg-key { color: var(--tool-accent); font-weight: 700; font-size: 12px; white-space: nowrap; }
.arg-value {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
  font-size: 12px;
  white-space: pre-wrap;
  word-break: break-word;
}
.arg-value.muted { color: var(--muted); }

.edits { display: flex; flex-direction: column; gap: 6px; }
.edits-header { display: flex; gap: 8px; }
.edits-header .arg-key { color: var(--muted); }
.edit-block { display: flex; flex-direction: column; gap: 4px; }
.edit-index { font-size: 11px; color: var(--muted); }
.diff-row { display: flex; gap: 6px; align-items: baseline; border-radius: 4px; padding: 2px 8px; }
.diff-row.diff-del { background: var(--diff-del-bg); }
.diff-row.diff-add { background: var(--diff-add-bg); }
.diff-marker { font-weight: 700; font-size: 13px; }
.diff-del .diff-marker { color: var(--danger); }
.diff-add .diff-marker { color: var(--success); }
.diff-content {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
  font-size: 12px;
  white-space: pre-wrap;
  word-break: break-word;
}

.todo-table { border: 1px solid var(--tool-content-border); border-radius: 4px; padding: 4px; }
.todo-header, .todo-row { display: flex; gap: 8px; align-items: baseline; padding: 2px; }
.todo-header { font-size: 11px; color: var(--muted); font-weight: 700; }
.todo-text {
  flex: 1;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
  font-size: 12px;
  white-space: pre-wrap;
  word-break: break-word;
}
.todo-status { width: 96px; flex: none; font-size: 12px; font-weight: 700; }
.todo-pending { color: #999999; }
.todo-in-progress { color: #2976FF; }
.todo-completed { color: var(--success); }
.todo-invalid { color: var(--danger); }
.todo-divider { height: 1px; background: var(--tool-content-border); }

.result-box { border: 1px solid var(--tool-content-border); border-radius: 6px; padding: 8px 10px; }
.result-box.ok { background: var(--tool-content-bg); }
.result-box.err { background: var(--diff-del-bg); border-color: rgba(229, 77, 77, 0.4); }
.result-label { font-size: 11px; font-weight: 700; }
.result-box.ok .result-label { color: var(--tool-accent); }
.result-box.err .result-label { color: var(--danger); }
.result-text {
  margin: 4px 0 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
  font-size: 13px;
  white-space: pre-wrap;
  word-break: break-word;
}

.ask-result {
  background: var(--tool-content-bg);
  border: 1px solid var(--tool-content-border);
  border-radius: 4px;
  padding: 10px 14px;
  margin-top: 8px;
}
.ask-question { margin-bottom: 8px; white-space: pre-wrap; }
.ask-options { display: flex; flex-direction: column; gap: 2px; }
.ask-option { display: flex; gap: 6px; }
.ask-check { width: 16px; flex: none; }
.ask-answer { margin-top: 4px; }
.ask-result .result-label.ok { color: var(--success); }
.ask-result .result-label.err { color: var(--danger); }
"#;
