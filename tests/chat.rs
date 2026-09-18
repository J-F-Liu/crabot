//! Integration tests for the markdown, URL and emoji helpers in `crabot::chat`.

use crabot::chat::{markdown_options, markdown_source, replace_emoji};
use pulldown_cmark::{Event, Parser};

/// Convenience wrapper: true if `text` contains at least one bare URL.
fn has_url(text: &str) -> bool {
    markdown_source(text).1
}

#[test]
fn wraps_bare_urls() {
    assert_eq!(
        markdown_source("Visit https://example.com now").0,
        "Visit <https://example.com> now"
    );
    assert_eq!(
        markdown_source("http://a.org and https://b.io").0,
        "<http://a.org> and <https://b.io>"
    );
}

#[test]
fn strips_trailing_punctuation() {
    assert_eq!(
        markdown_source("See https://x.com.").0,
        "See <https://x.com>."
    );
    assert_eq!(
        markdown_source("a, b, https://x.com,").0,
        "a, b, <https://x.com>,"
    );
}

#[test]
fn keeps_balanced_parens() {
    assert_eq!(
        markdown_source("(https://en.wikipedia.org/wiki/Function_(mathematics))").0,
        "(<https://en.wikipedia.org/wiki/Function_(mathematics)>)"
    );
}

#[test]
fn skips_code_regions() {
    assert_eq!(
        markdown_source("use `https://x.com` inline").0,
        "use `https://x.com` inline"
    );
    assert_eq!(
        markdown_source("```rust\n// https://x.com\n```").0,
        "```rust\n// https://x.com\n```"
    );
}

#[test]
fn skips_existing_link_constructs() {
    assert_eq!(
        markdown_source("[click](https://x.com) here").0,
        "[click](https://x.com) here"
    );
    assert_eq!(
        markdown_source("<https://x.com> autolink").0,
        "<https://x.com> autolink"
    );
    assert_eq!(
        markdown_source("![alt](https://x.com/img.png)").0,
        "![alt](https://x.com/img.png)"
    );
    assert_eq!(
        markdown_source("<a href=\"https://x.com\">raw</a>").0,
        "<a href=\"https://x.com\">raw</a>"
    );
}

#[test]
fn ignores_windows_paths_and_scheme_less() {
    assert_eq!(
        markdown_source(r"See C:\Users\foo\bar").0,
        r"See C:\Users\foo\bar"
    );
    assert_eq!(
        markdown_source("check www.example.com").0,
        "check www.example.com"
    );
}

#[test]
fn reports_whether_urls_were_wrapped() {
    assert!(has_url("go to https://x.com now"));
    assert!(!has_url("no url here"));
    assert!(!has_url("see `https://x.com` in code"));
    assert!(!has_url("[text](https://x.com)"));
    assert!(!has_url(r"C:\path\file.rs"));
    assert!(has_url("http://a.org and https://b.io"));
}

#[test]
fn emoji_replacement_still_works() {
    assert_eq!(replace_emoji("Hello :wave:!"), "Hello 👋!");
    assert_eq!(replace_emoji("`x:wave:`"), "`x:wave:`");
}

#[test]
fn emoji_skipped_inside_link_constructs() {
    // The whole link span (text and destination) is protected, so a `:emoji:`
    // shortcode can never corrupt a link URL.
    assert_eq!(
        replace_emoji("[:wave:](https://x.com)"),
        "[:wave:](https://x.com)"
    );
    assert_eq!(
        replace_emoji("![alt :wave:](https://x.com/img.png)"),
        "![alt :wave:](https://x.com/img.png)"
    );
    // Emoji outside the link span is still replaced.
    assert_eq!(
        replace_emoji(":wave: [text](https://x.com) :wave:"),
        "👋 [text](https://x.com) 👋"
    );
}

#[test]
fn escapes_math_bars_and_keeps_non_ascii() {
    // A `|` inside a formula must not split a GFM table cell.
    let (source, has_url) = markdown_source("| $r = ratio·|n·Ct|$ | 42 |");
    assert_eq!(source, r"| $r = ratio·\|n·Ct\|$ | 42 |");
    assert!(!has_url);
    // Non-ASCII text must pass through untouched.
    assert_eq!(markdown_source("café · 中文 👋").0, "café · 中文 👋");
    // A `$` pair bracketing a table must not escape its separators.
    let table = "Prices: $5 today\n\n| a | b |\n|---|---|\n| x | y$ |";
    assert_eq!(markdown_source(table).0, table);
}

#[test]
fn emoji_then_links() {
    assert_eq!(
        markdown_source(&replace_emoji("See :point_right: https://x.com")).0,
        "See 👉 <https://x.com>"
    );
}

#[test]
fn escapes_code_bars_inside_tables() {
    // A table row is split on every unescaped `|`, even inside a code span, so
    // the bar has to be escaped or the rest of the cell is dropped.
    let (source, has_url) = markdown_source("| a | b |\n|---|---|\n| `x|y` | z |");
    assert_eq!(source, "| a | b |\n|---|---|\n| `x\\|y` | z |");
    assert!(!has_url);
    // Escaping is idempotent.
    assert_eq!(
        markdown_source("| a | b |\n|---|---|\n| `x\\|y` | z |").0,
        "| a | b |\n|---|---|\n| `x\\|y` | z |"
    );
    // Bars inside code spans outside a table stay untouched, an escaped one
    // would render as a literal backslash there.
    assert_eq!(markdown_source("code `x|y` here").0, "code `x|y` here");
    // Blockquote and list tables are covered too, even though the parsed table
    // range starts after the `> ` marker.
    assert_eq!(
        markdown_source("> | a | b |\n> |---|---|\n> | `x|y` | z |").0,
        "> | a | b |\n> |---|---|\n> | `x\\|y` | z |"
    );
}

#[test]
fn unmatched_backtick_run_stays_literal() {
    // An unmatched `` run is literal, so only the `c|d` span bar is escaped;
    // re-reading the run's suffix as a shorter opener would escape the real cell
    // separator instead and drop the `c|d` bar.
    let (source, _) = markdown_source("| a | b |\n|---|---|\n| `` x | `c|d` |");
    assert_eq!(source, "| a | b |\n|---|---|\n| `` x | `c\\|d` |");
}

#[test]
fn code_bar_no_longer_truncates_a_table_row() {
    let row = "| 闭式解正确 | 圆（`r = ratio·|n·Ct|`）反投影后满足原圆锥方程，残差 ~1e-16，说明 `ray`/`normal`/`ratio` 三者自洽 |";
    let (source, _) = markdown_source(&format!("| 项 | 证据 |\n|---|---|\n{row}\n"));
    // The parser now sees the whole row: both code bars survive and no cell text
    // is lost.
    let events: Vec<Event<'_>> = Parser::new_ext(&source, markdown_options()).collect();
    let code: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            Event::Code(text) => Some(text.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(code, ["r = ratio·|n·Ct|", "ray", "normal", "ratio"]);
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            Event::Text(text) => Some(text.to_string()),
            _ => None,
        })
        .collect();
    assert!(
        text.contains("反投影后满足原圆锥方程，残差 ~1e-16"),
        "row tail dropped: {text:?}"
    );
}
