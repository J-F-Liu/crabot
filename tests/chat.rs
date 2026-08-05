//! Integration tests for the URL linkification and emoji replacement helpers
//! in `crabot::chat`.

use crabot::chat::{linkify_urls, replace_emoji};

/// Convenience wrapper: true if `text` contains at least one bare URL.
fn has_url(text: &str) -> bool {
    linkify_urls(text).1
}

#[test]
fn wraps_bare_urls() {
    assert_eq!(
        linkify_urls("Visit https://example.com now").0,
        "Visit <https://example.com> now"
    );
    assert_eq!(
        linkify_urls("http://a.org and https://b.io").0,
        "<http://a.org> and <https://b.io>"
    );
}

#[test]
fn strips_trailing_punctuation() {
    assert_eq!(linkify_urls("See https://x.com.").0, "See <https://x.com>.");
    assert_eq!(
        linkify_urls("a, b, https://x.com,").0,
        "a, b, <https://x.com>,"
    );
}

#[test]
fn keeps_balanced_parens() {
    assert_eq!(
        linkify_urls("(https://en.wikipedia.org/wiki/Function_(mathematics))").0,
        "(<https://en.wikipedia.org/wiki/Function_(mathematics)>)"
    );
}

#[test]
fn skips_code_regions() {
    assert_eq!(
        linkify_urls("use `https://x.com` inline").0,
        "use `https://x.com` inline"
    );
    assert_eq!(
        linkify_urls("```rust\n// https://x.com\n```").0,
        "```rust\n// https://x.com\n```"
    );
}

#[test]
fn skips_existing_link_constructs() {
    assert_eq!(
        linkify_urls("[click](https://x.com) here").0,
        "[click](https://x.com) here"
    );
    assert_eq!(
        linkify_urls("<https://x.com> autolink").0,
        "<https://x.com> autolink"
    );
    assert_eq!(
        linkify_urls("![alt](https://x.com/img.png)").0,
        "![alt](https://x.com/img.png)"
    );
    assert_eq!(
        linkify_urls("<a href=\"https://x.com\">raw</a>").0,
        "<a href=\"https://x.com\">raw</a>"
    );
}

#[test]
fn ignores_windows_paths_and_scheme_less() {
    assert_eq!(
        linkify_urls(r"See C:\Users\foo\bar").0,
        r"See C:\Users\foo\bar"
    );
    assert_eq!(
        linkify_urls("check www.example.com").0,
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
fn emoji_then_links() {
    assert_eq!(
        linkify_urls(&replace_emoji("See :point_right: https://x.com")).0,
        "See 👉 <https://x.com>"
    );
}
