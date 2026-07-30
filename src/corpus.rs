//! Compatibility corpus for the HTML allowlist in [`crate::sanitize`].
//!
//! The unit tests next to the policy check that each rule does what it says.
//! These check the opposite risk: that the policy, applied to the shapes real
//! articles actually ship, does not quietly cost the reader content. An
//! allowlist fails silently — nothing errors when a paragraph disappears — so
//! the guard has to be inventory, asserted per shape.
//!
//! Assertions are explicit (this prose survives, that construct does not, this
//! many tables remain) rather than golden files. A golden file would break on
//! every deliberate policy tweak without ever saying whether the change was an
//! improvement, which trains you to regenerate it unread.
//!
//! Five fixtures are hand-authored, each named for the real-world pattern it
//! stands for. The sixth is a reduced real capture with recorded provenance,
//! because hand-authored markup only ever contains the shapes we already
//! thought of — it cannot produce the malformed nesting and parser repairs that
//! make real pages interesting.

#![cfg(test)]

use crate::sanitize::{self, Region};

const LAZY_IMAGES: &str = include_str!("../fixtures/lazy-images.html");
const EMBEDS: &str = include_str!("../fixtures/embeds.html");
const TABLES_AND_CODE: &str = include_str!("../fixtures/tables-and-code.html");
const PAYWALL_SHELL: &str = include_str!("../fixtures/paywall-shell.html");
const ACTIVE_MARKUP: &str = include_str!("../fixtures/active-markup.html");
const WIKIPEDIA_CAPTURE: &str = include_str!("../fixtures/wikipedia-epub-capture.html");

fn article(fixture: &str) -> String {
    sanitize::fragment(fixture, Region::ArticleBody)
        .as_str()
        .to_string()
}

/// Constructs that must never survive into a book, whatever the fixture.
fn assert_no_active_content(html: &str, fixture: &str) {
    for forbidden in [
        "<script",
        "<style",
        "<iframe",
        "<svg",
        "<template",
        "<form",
        "<input",
        "<select",
        "<textarea",
        "<button",
        "onclick",
        "onload",
        "onmouseover",
        "javascript:",
        "<!--",
    ] {
        assert!(
            !html.contains(forbidden),
            "{fixture}: {forbidden:?} survived sanitize"
        );
    }
}

fn count(html: &str, needle: &str) -> usize {
    html.matches(needle).count()
}

#[test]
fn lazy_images_keep_a_recoverable_source_and_all_prose() {
    let html = article(LAZY_IMAGES);
    assert_no_active_content(&html, "lazy-images");

    for marker in ["PROSE_LEAD", "PROSE_CAPTION", "PROSE_TAIL"] {
        assert!(html.contains(marker), "lazy-images: lost {marker}");
    }
    // `images` recovers the real source from these; it strips them once the
    // asset is localized, so they are inert but load-bearing here.
    assert!(html.contains("data-src=\"/media/diagram-full.png\""));
    assert!(html.contains("data-lazy-src=\"/media/photo.jpg\""));
    // srcset and <source> are the image policy's problem, not the reader's.
    assert!(!html.contains("srcset"));
    assert!(!html.contains("<source"));
    // The <noscript> twin unwraps, so the no-script image is still reachable.
    assert_eq!(
        count(&html, "<img"),
        3,
        "expected placeholder, picture, noscript"
    );
    assert!(html.contains("alt=\"Architecture diagram, no-script fallback\""));
    // Untrusted markup must not carry classes that could impersonate chrome.
    assert!(!html.contains("lazyload"));
    assert!(!html.contains("class="));
}

#[test]
fn embed_shells_are_removed_without_taking_their_prose() {
    let html = article(EMBEDS);
    assert_no_active_content(&html, "embeds");

    for marker in [
        "PROSE_BEFORE_EMBED",
        "PROSE_VIDEO_CAPTION",
        "PROSE_TWEET_BODY",
        "PROSE_CALLOUT",
        "PROSE_CUSTOM_ELEMENT",
        "PROSE_DETAILS",
        "PROSE_AFTER_EMBED",
    ] {
        assert!(html.contains(marker), "embeds: lost {marker}");
    }
    assert!(!html.contains("youtube.com/embed"));
    assert!(html.contains("<blockquote"));
    assert!(html.contains("https://twitter.com/someone/status/1"));
}

#[test]
fn tables_code_and_footnotes_survive_with_structure_intact() {
    let html = article(TABLES_AND_CODE);
    assert_no_active_content(&html, "tables-and-code");

    for marker in [
        "PROSE_TABLE_CAPTION",
        "PROSE_TABLE_CELL",
        "PROSE_TABLE_SPAN",
        "PROSE_CODE_MARKER",
        "PROSE_DEFINITION",
        "PROSE_LIST_ITEM",
        "PROSE_FOOTNOTE",
    ] {
        assert!(html.contains(marker), "tables-and-code: lost {marker}");
    }
    assert_eq!(count(&html, "<table"), 1);
    // Trailing space on purpose: `<th` also matches `<thead`.
    assert_eq!(count(&html, "<th "), 5);
    assert!(html.contains("rowspan=\"2\""));
    assert!(html.contains("colspan=\"3\""));
    assert!(html.contains("scope=\"row\""));
    assert!(html.contains("<caption"));
    assert!(html.contains("<colgroup"));
    // Code must stay verbatim: escaped angle brackets, indentation, ampersands.
    assert!(html.contains("Vec&lt;u8&gt;"));
    assert!(html.contains("&amp;&amp;"));
    assert!(html.contains("<pre>"));
    assert!(html.contains("start=\"3\""));
    assert!(html.contains("value=\"7\""));
    // Author anchors are namespaced, never dropped, so they cannot collide with
    // RustyPub's own cN/tN targets.
    assert!(html.contains("id=\"rp-cite_ref-1\""));
    assert!(html.contains("href=\"#rp-cite_note-1\""));
    assert!(!html.contains("href=\"#cite_note-1\""));
}

#[test]
fn paywall_furniture_contributes_no_text_but_prose_remains() {
    let html = article(PAYWALL_SHELL);
    assert_no_active_content(&html, "paywall-shell");

    for marker in ["PROSE_TEASER", "PROSE_SECOND", "PROSE_RELATED_LINK"] {
        assert!(html.contains(marker), "paywall-shell: lost {marker}");
    }
    // The point of the fixture: none of the form's long strings can pad the
    // husk-length check in sites::article.
    for furniture in [
        "billed at nine dollars",
        "Enter a very long placeholder",
        "Textarea default values",
        "Subscribe now for full unlimited",
        "70 percent",
        "40 percent",
    ] {
        assert!(
            !html.contains(furniture),
            "paywall-shell: form value {furniture:?} leaked into reading text"
        );
    }
    // The property `sites::article` leans on: sanitize runs *before* the
    // husk-length check, so a page that is only paywall furniture cannot reach
    // the 200-character threshold on the strength of option labels, placeholder
    // text, and button captions.
    let start = PAYWALL_SHELL.find("<!-- FURNITURE_START -->").unwrap();
    let end = PAYWALL_SHELL.find("<!-- FURNITURE_END -->").unwrap();
    let furniture = article(&PAYWALL_SHELL[start..end]);
    let furniture_text = crate::util::strip_tags(&furniture);
    assert!(
        furniture_text.trim().chars().count() < 200,
        "furniture alone would pass the husk check: {furniture_text:?}"
    );
}

#[test]
fn hostile_markup_cannot_restructure_or_impersonate() {
    let html = article(ACTIVE_MARKUP);
    assert_no_active_content(&html, "active-markup");

    for marker in [
        "PROSE_OPENING",
        "PROSE_HANDLER_TEXT",
        "PROSE_ACTIVE_LINK_LABEL",
        "PROSE_DATA_LINK_LABEL",
        "PROSE_SAFE_LINK",
        "PROSE_IMPERSONATION_TEXT",
        "PROSE_BEFORE_UNBALANCED",
        "PROSE_AFTER_BODY_TAG",
    ] {
        assert!(html.contains(marker), "active-markup: lost {marker}");
    }
    for payload in [
        "SHOULD_NOT_APPEAR_SCRIPT",
        "SHOULD_NOT_APPEAR_STYLE",
        "SHOULD_NOT_APPEAR_TEMPLATE",
        "SHOULD_NOT_APPEAR_SVG",
        "SHOULD_NOT_APPEAR_COMMENT",
    ] {
        assert!(!html.contains(payload), "active-markup: {payload} survived");
    }
    // Active hrefs lose the attribute but keep the label — the reader still
    // sees the words, just not a live trap.
    assert!(!html.contains("href=\"javascript:"));
    assert!(!html.contains("href=\"data:"));
    assert!(html.contains("href=\"https://example.com/ok\""));
    // Chrome impersonation is the attack the class/id ban exists for.
    assert!(!html.contains("t-head"));
    assert!(!html.contains("c-skip"));
    assert!(!html.contains("id=\"t1\""));
    assert!(!html.contains("id=\"c1\""));
    assert!(!html.contains("untrusted-body-class"));
    // Isolation guarantee: whatever the fragment did structurally stayed inside
    // it. html5ever hands back a balanced tree, so the assembled book cannot
    // inherit a stray open element.
    assert_eq!(count(&html, "<div"), count(&html, "</div>"));
    assert_eq!(count(&html, "<p>"), count(&html, "</p>"));
    assert!(!html.contains("<body"));
    assert!(!html.contains("<plaintext"));
}

#[test]
fn a_real_capture_survives_the_allowlist() {
    // Reduced Wikipedia article; see the provenance header in the fixture.
    let html = article(WIKIPEDIA_CAPTURE);
    assert_no_active_content(&html, "wikipedia-epub-capture");

    // Real prose from the capture, not a marker we planted.
    assert!(html.contains("EPUB"));
    let text = crate::util::strip_tags(&html);
    assert!(
        text.split_whitespace().count() > 400,
        "capture reduced to a husk: {} words",
        text.split_whitespace().count()
    );
    // The structures the capture was chosen for.
    assert_eq!(count(&html, "<table"), 1);
    assert!(count(&html, "<sup") > 20, "reference superscripts lost");
    assert!(html.contains("<img"));
    assert!(!html.contains("srcset"));
    assert!(!html.contains("<link"));
    // Parsoid ships data-mw payloads and typeof attributes on ordinary
    // elements; they are metadata, not reading content.
    assert!(!html.contains("data-mw"));
    assert!(!html.contains("typeof="));
    assert!(!html.contains("class="));
    assert_eq!(count(&html, "<div"), count(&html, "</div>"));
}

#[test]
fn a_corpus_article_reaches_the_epub_intact() {
    use crate::model::{Book, BookBody, Story};
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let book = Book {
        story: Story {
            id: String::new(),
            title: "Corpus".to_string(),
            // Non-routable base, matching epub::tests: image fetches must fail
            // instantly rather than reaching the network or waiting out
            // FETCH_TIMEOUT. With the address policy in place these fail at
            // resolve rather than connect — still offline, different path.
            url: Some("http://127.0.0.1:1/article".to_string()),
            discussion_url: None,
            author: "author".to_string(),
            points: None,
            time: Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap(),
            // The fixture plus a loopback image, so this test actually walks the
            // omission path it claims to: the address policy refuses to resolve
            // 127.0.0.1 and the reader is left an alt-text marker.
            text_html: Some(sanitize::fragment(
                &format!(
                    "{TABLES_AND_CODE}<p><img src=\"http://127.0.0.1:1/photo.png\" alt=\"Bench rig\"></p>"
                ),
                Region::ArticleBody,
            )),
        },
        body: BookBody::Article,
        source: "example.com".to_string(),
        source_slug: "example-com".to_string(),
    };

    let result = crate::epub::build_epub(
        &book,
        "body { font-family: serif; }",
        dir.path(),
        5,
        false,
        "http://127.0.0.1:1/article",
        &|_| {},
    )
    .expect("pandoc EPUB build");

    let entries = crate::epub::tests::content_entries(&result.epub_path);
    let all = entries.values().cloned().collect::<String>();

    for marker in ["PROSE_TABLE_CAPTION", "PROSE_CODE_MARKER", "PROSE_FOOTNOTE"] {
        assert!(all.contains(marker), "EPUB lost {marker}");
    }
    assert!(all.contains("<table"), "table did not reach the EPUB");
    assert!(
        all.contains("Vec&lt;u8&gt;"),
        "code was not preserved verbatim"
    );
    // The unreachable image degrades to its alt text rather than a broken
    // reference, and its URL does not survive into the book.
    assert!(
        all.contains("Image omitted: Bench rig"),
        "unreachable image did not degrade to alt text"
    );
    // The story link legitimately carries the article URL, so check the image
    // path specifically rather than the host.
    assert!(
        !all.contains("photo.png"),
        "unfetched image URL leaked into the EPUB"
    );
    for forbidden in ["<script", "<svg", "onclick", "javascript:"] {
        assert!(!all.contains(forbidden), "EPUB contains {forbidden}");
    }
}
