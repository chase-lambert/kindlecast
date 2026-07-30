//! The single boundary where untrusted markup becomes reading content.
//!
//! Every fragment of third-party HTML — comment bodies, discussion selftext,
//! extracted article bodies — enters the book through [`fragment`]. Nothing
//! outside this module can construct a [`SanitizedHtml`], so `render` cannot
//! assemble raw markup even by accident.
//!
//! **Isolation is the point.** Each fragment is parsed in its own document, so
//! markup that would otherwise restructure the whole book is resolved inside
//! that fragment and can never reach its neighbours:
//!
//! - an unbalanced `</div>` that would escape its wrapper and mint a stray
//!   chapter heading with a colliding `id`,
//! - `<plaintext>` or an unclosed `<script>`/`<style>`/`<textarea>`/`<xmp>`,
//!   which swallow every following byte as text,
//! - `<body class="…">`, whose attributes html5ever merges onto the *existing*
//!   body element — one such comment could otherwise re-label every trusted
//!   chapter heading as untrusted content.
//!
//! What comes back out is html5ever's serialization of a parsed tree, which is
//! balanced by construction. That is what makes the later assembled parse in
//! `images` structurally trustworthy: not a promise, a property.
//!
//! Disposition is three buckets. [`KEEP`] survives with filtered attributes.
//! [`REMOVE`] is discarded together with its text, because that text is not
//! reading content. Everything else — including unknown and custom elements —
//! is *unwrapped*: the tag goes, the words stay. Unwrapping is the default so
//! that an unrecognized tag can never cost the reader a sentence.

use dom_query::{Document, NodeRef};

/// Untrusted HTML that has passed [`fragment`]: allowlisted, and balanced by
/// construction. The private field is the whole point — `render` may only
/// assemble values this module produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SanitizedHtml(String);

impl SanitizedHtml {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

/// Which reading region a fragment will occupy. The caller knows this; the
/// sanitizer must never have to guess it from markup the fragment could forge.
///
/// Heading policy is the reason this enum exists. Pandoc splits chapters on
/// `h1` (`--split-level=1`), so a heading's fate depends entirely on whose
/// structure it represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// Comment body: *every* heading becomes a bold lead-in `div.c-hd`. A
    /// surviving `h1` would split a chapter in the middle of a thread, and a
    /// surviving `h2`–`h6` would outrank the thread heading it sits under.
    CommentBody,
    /// Discussion selftext: `h1` demotes to `h2` — still a heading, never a
    /// chapter break. Deeper headings are already harmless.
    DiscussionText,
    /// Extracted article body: headings are the author's own structure, and an
    /// `h1` legitimately starts a new chapter. Left alone.
    ArticleBody,
}

impl Region {
    /// Article footnote and table-of-contents anchors are worth keeping, but
    /// they must not be able to collide with RustyPub's `cN`/`tN` anchors.
    fn keeps_anchors(self) -> bool {
        matches!(self, Region::ArticleBody)
    }
}

/// Prefix applied to `id` and same-document `href` inside article bodies so
/// author anchors can never collide with RustyPub's comment/thread anchors.
const ANCHOR_PREFIX: &str = "rp-";

/// Elements that survive into the book, with attributes filtered to
/// [`keep_attrs`]. Anything not here and not in [`REMOVE`] is unwrapped.
#[rustfmt::skip]
const KEEP: &[&str] = &[
    // Block structure.
    "p", "div", "blockquote", "pre", "hr", "br",
    // Headings. Region policy may already have rewritten these.
    "h1", "h2", "h3", "h4", "h5", "h6",
    // Inline text.
    "span", "a", "em", "strong", "b", "i", "u", "s", "strike", "del", "ins",
    "sub", "sup", "small", "mark", "abbr", "cite", "q", "dfn", "code", "kbd",
    "samp", "var", "time", "wbr",
    // Lists.
    "ul", "ol", "li", "dl", "dt", "dd",
    // Tables.
    "table", "thead", "tbody", "tfoot", "tr", "th", "td", "caption",
    "colgroup", "col",
    // Figures. `img` sources are localized later by `images`.
    "img", "figure", "figcaption",
];

/// Elements discarded together with their text, because the text is markup
/// payload, interface furniture, or a value — never prose.
///
/// Note what is deliberately *absent*: `object`, `canvas`, `dialog`, `audio`,
/// `video`, `details`, `noscript`, `form`. Those are shells whose children are
/// often real fallback prose, so they unwrap instead. Unwrapping `noscript` also
/// *improves* fidelity on lazy-loading sites, where the true `<img>` lives
/// there; tracking pixels are the image policy's problem, not a reason to delete
/// a paragraph.
#[rustfmt::skip]
const REMOVE: &[&str] = &[
    // Script and style payloads.
    "script", "style", "template",
    // Pandoc externalizes surviving inline SVG as a `.svgz` asset, and
    // malformed source can invalidate the entire EPUB.
    "svg",
    // Document metadata a fragment should never contribute.
    "title", "base", "link", "meta", "param",
    // Form controls and their values.
    "input", "textarea", "button", "select", "option", "optgroup", "datalist",
    "output", "progress", "meter",
    // Embedded and framed content.
    "iframe", "frame", "frameset", "applet", "embed", "source", "track",
];

/// Sanitize one untrusted fragment for `region`.
pub fn fragment(html: &str, region: Region) -> SanitizedHtml {
    if html.trim().is_empty() {
        return SanitizedHtml(String::new());
    }

    // Isolated parse. Anything structural the fragment tries to do is confined
    // to this document, and `body`'s own attributes are discarded with it.
    let document = Document::from(html);
    let Some(body) = document.select("body").nodes().first().cloned() else {
        return SanitizedHtml(String::new());
    };

    remove_comment_nodes(&body);
    document.select(&REMOVE.join(", ")).remove();
    filter_attributes(&document, region);
    apply_heading_policy(&document, region);
    unwrap_unknown(&document);

    SanitizedHtml(body.inner_html().to_string())
}

/// Drop comment nodes. They survive parse and serialization, are invisible to
/// selectors, and would otherwise ride into the EPUB source (and `--keep-html`).
fn remove_comment_nodes(node: &NodeRef<'_>) {
    for child in node.children() {
        if child.is_comment() {
            child.remove_from_parent();
        } else {
            remove_comment_nodes(&child);
        }
    }
}

/// Attributes a given element may keep. `class` and `id` are never among them:
/// untrusted content must not be able to impersonate RustyPub's chrome
/// (`t-head`, `c-skip`, `d0`…`d5`) or collide with its `cN`/`tN` anchors.
/// The sanitizer applies its own `c-hd` class afterwards.
fn keep_attrs(tag: &str) -> &'static [&'static str] {
    match tag {
        "a" => &["href", "title", "lang", "dir"],
        // `data-src` / `data-lazy-src` are inert, and `images` reads them to
        // recover the real source of a lazy-loaded image. It strips them again
        // once localized, so they never reach the book.
        "img" => &[
            "src",
            "data-src",
            "data-lazy-src",
            "alt",
            "width",
            "height",
            "title",
        ],
        "td" | "th" => &["colspan", "rowspan", "headers", "scope", "lang", "dir"],
        "ol" => &["start", "type", "reversed", "lang", "dir"],
        "li" => &["value", "lang", "dir"],
        "col" | "colgroup" => &["span"],
        "time" => &["datetime", "lang", "dir"],
        "blockquote" | "q" => &["cite", "title", "lang", "dir"],
        _ => &["title", "lang", "dir"],
    }
}

fn filter_attributes(document: &Document, region: Region) {
    for node in document.select("*").nodes() {
        let Some(tag) = node.node_name().map(|name| name.to_string()) else {
            continue;
        };
        let mut allowed: Vec<&str> = keep_attrs(&tag).to_vec();

        // Article anchors are preserved but namespaced, so footnotes keep
        // working without ever colliding with RustyPub's own ids.
        let anchors = region.keeps_anchors();
        if anchors {
            allowed.push("id");
        }
        node.retain_attrs(&allowed);

        if anchors && let Some(id) = node.attr("id") {
            node.set_attr("id", &format!("{ANCHOR_PREFIX}{}", id.trim()));
        }

        rewrite_url_attr(node, &tag, "href", anchors);
        rewrite_url_attr(node, &tag, "src", anchors);
    }
}

/// Drop active-scheme URLs, and keep same-document links honest.
///
/// A bare `#target` inside untrusted content is either namespaced (articles) or
/// dropped (comments), so it can never aim at a `cN`/`tN` anchor.
fn rewrite_url_attr(node: &NodeRef<'_>, tag: &str, attr: &str, keeps_anchors: bool) {
    let Some(value) = node.attr(attr) else {
        return;
    };
    let value = value.trim().to_string();

    if let Some(target) = value.strip_prefix('#') {
        if keeps_anchors && !target.is_empty() {
            node.set_attr(attr, &format!("#{ANCHOR_PREFIX}{target}"));
        } else {
            node.remove_attr(attr);
        }
        return;
    }

    if is_active_scheme(&value) {
        node.remove_attr(attr);
        // An `img` with no source is furniture; leave alt text behind instead.
        if tag == "img" && attr == "src" {
            node.remove_attr("src");
        }
    }
}

/// `javascript:`, `vbscript:` and `data:` URLs never belong in a passive book.
/// Checked on the raw prefix rather than via a URL parser so that entity- and
/// whitespace-obfuscated schemes cannot slip past a stricter grammar.
fn is_active_scheme(value: &str) -> bool {
    let mut scheme = String::new();
    for ch in value.chars() {
        match ch {
            ':' => break,
            // Tabs, newlines and NULs inside a scheme are ignored by browsers.
            c if c.is_whitespace() || c == '\0' => continue,
            c => scheme.push(c.to_ascii_lowercase()),
        }
    }
    matches!(scheme.as_str(), "javascript" | "vbscript" | "data")
}

fn apply_heading_policy(document: &Document, region: Region) {
    match region {
        Region::CommentBody => {
            for node in document.select("h1, h2, h3, h4, h5, h6").nodes() {
                // The result is RustyPub's own lead-in element, not the
                // author's heading, so it carries only our class.
                node.rename("div");
                node.remove_all_attrs();
                node.set_attr("class", "c-hd");
            }
        }
        Region::DiscussionText => {
            for node in document.select("h1").nodes() {
                node.rename("h2");
            }
        }
        Region::ArticleBody => {}
    }
}

/// Unwrap every element that is neither kept nor removed: the tag goes, its
/// children stay in place.
///
/// Uses `strip_elements`, never `NodeRef::unwrap_node` — the latter is inverted
/// and lossy in dom_query 0.28. Unwrapping `<em>` in
/// `<div id=keep><span>A</span><em>B</em><i>C</i></div>` destroys `div#keep` and
/// silently deletes `<span>A</span>`, which in a sanitizer would eat prose out
/// of the middle of an article.
fn unwrap_unknown(document: &Document) {
    let unknown: Vec<String> = document
        .select("*")
        .nodes()
        .iter()
        .filter_map(|node| node.node_name().map(|name| name.to_string()))
        .filter(|tag| {
            !matches!(tag.as_str(), "html" | "head" | "body")
                && !KEEP.contains(&tag.as_str())
                && !REMOVE.contains(&tag.as_str())
        })
        .collect();
    if unknown.is_empty() {
        return;
    }
    let names: Vec<&str> = unknown.iter().map(String::as_str).collect();
    document.select("body").strip_elements(&names);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(html: &str) -> String {
        fragment(html, Region::CommentBody).as_str().to_string()
    }

    #[test]
    fn script_and_style_payloads_are_removed_with_their_text() {
        let out = comment("<p>Hello</p><script>bad()</script><style>.x{}</style><p>World</p>");
        assert_eq!(out, "<p>Hello</p><p>World</p>");
    }

    #[test]
    fn unknown_and_custom_elements_unwrap_and_keep_their_words() {
        let out = comment("<section><my-widget>kept words</my-widget></section>");
        assert_eq!(out, "kept words");
    }

    #[test]
    fn fallback_shells_unwrap_so_their_prose_survives() {
        for shell in ["noscript", "object", "canvas", "dialog", "details", "form"] {
            let out = comment(&format!("<{shell}><p>fallback prose</p></{shell}>"));
            assert!(
                out.contains("fallback prose"),
                "{shell} lost its fallback: {out}"
            );
        }
    }

    #[test]
    fn form_controls_leave_nothing_behind() {
        let out = comment(
            "<form><input value=\"v\"><textarea>typed</textarea><button>go</button></form>",
        );
        assert_eq!(out, "");
    }

    #[test]
    fn active_content_and_presentation_attributes_are_dropped() {
        let out = comment(
            "<p onclick=\"bad()\" style=\"color:red\" data-track=\"1\" class=\"x\" id=\"y\">text</p>",
        );
        assert_eq!(out, "<p>text</p>");
    }

    #[test]
    fn javascript_href_is_neutralized_but_link_text_survives() {
        let out = comment("<a href=\"javascript:bad()\">click me</a>");
        assert_eq!(out, "<a>click me</a>");
    }

    #[test]
    fn obfuscated_active_scheme_is_still_caught() {
        for href in [
            "JaVaScRiPt:bad()",
            "java\tscript:bad()",
            "  javascript:bad()",
            "vbscript:bad()",
            "data:text/html;base64,PHNjcmlwdD4=",
        ] {
            let out = comment(&format!("<a href=\"{href}\">t</a>"));
            assert_eq!(out, "<a>t</a>", "not neutralized: {href}");
        }
    }

    #[test]
    fn ordinary_links_and_images_keep_their_useful_attributes() {
        let out = comment(
            "<a href=\"https://example.com/x\">link</a><img src=\"https://example.com/i.png\" alt=\"pic\">",
        );
        assert!(out.contains("href=\"https://example.com/x\""));
        assert!(out.contains("src=\"https://example.com/i.png\""));
        assert!(out.contains("alt=\"pic\""));
    }

    #[test]
    fn svg_is_removed_but_table_structure_survives() {
        let out = comment("<svg><text>vector</text></svg><table><tr><td>cell</td></tr></table>");
        assert!(!out.contains("svg"));
        assert!(!out.contains("vector"));
        assert!(out.contains("<td>cell</td>"));
    }

    #[test]
    fn comment_nodes_do_not_reach_the_book() {
        let out = comment("<p>a</p><!-- secret --><div><!-- deep --><b>x</b></div>");
        assert!(!out.contains("secret"));
        assert!(!out.contains("deep"));
        assert!(!out.contains("<!--"));
        assert_eq!(out, "<p>a</p><div><b>x</b></div>");
    }

    #[test]
    fn template_contents_cannot_hide_a_script() {
        // `select("*")` never enumerates inside `<template>`, yet the contents
        // do serialize — so only removing the element itself closes this.
        let out = comment("<p>a</p><template><script>bad()</script><p>hidden</p></template>");
        assert_eq!(out, "<p>a</p>");
    }

    #[test]
    fn noscript_image_survives_for_lazy_loading_sites() {
        let out = comment("<noscript><img src=\"https://example.com/real.png\"></noscript>");
        assert!(out.contains("src=\"https://example.com/real.png\""));
    }

    // ---- Region heading policy: today's three behaviours, preserved exactly ----

    #[test]
    fn comment_headings_all_become_lead_ins() {
        let out = comment("<h1>Big</h1><h2 class=\"x\">Small</h2><h6>Tiny</h6>");
        assert_eq!(
            out,
            "<div class=\"c-hd\">Big</div><div class=\"c-hd\">Small</div><div class=\"c-hd\">Tiny</div>"
        );
    }

    #[test]
    fn discussion_text_demotes_h1_only() {
        let out = fragment(
            "<h1>Selftext</h1><h2>Already smaller</h2>",
            Region::DiscussionText,
        );
        assert_eq!(out.as_str(), "<h2>Selftext</h2><h2>Already smaller</h2>");
    }

    #[test]
    fn article_headings_keep_the_authors_structure() {
        let out = fragment("<h1>Part II</h1><h2>Section</h2>", Region::ArticleBody);
        assert_eq!(out.as_str(), "<h1>Part II</h1><h2>Section</h2>");
    }

    #[test]
    fn heading_with_angle_bracket_in_attribute_is_handled_correctly() {
        // The regex this replaced (`<h[1-6]\b[^>]*>`) stopped at the first `>`
        // inside the attribute value and leaked ` b">` into the book as text.
        let out = comment("<h3 title=\"a > b\">Odd</h3>");
        assert_eq!(out, "<div class=\"c-hd\">Odd</div>");
    }

    // ---- Containment: the findings that overturned the assembled-pass design ----

    #[test]
    fn unbalanced_close_tag_cannot_escape_the_fragment() {
        let out = comment("</div><h1 id=\"t2\">forged chapter</h1>");
        assert!(
            !out.contains("<h1"),
            "forged chapter heading survived: {out}"
        );
        assert!(!out.contains("id="), "forged anchor survived: {out}");
        assert!(out.contains("forged chapter"));
    }

    #[test]
    fn body_attributes_cannot_be_smuggled_out_of_a_fragment() {
        let out = comment("<body class=\"c-body\"><p>text</p>");
        assert!(!out.contains("c-body"), "body class escaped: {out}");
        assert_eq!(out, "<p>text</p>");
    }

    #[test]
    fn plaintext_cannot_swallow_the_rest_of_the_book() {
        // In an assembled parse this consumed every following chapter.
        let out = comment("<plaintext>swallowed");
        assert!(!out.contains("<plaintext"));
        assert!(out.contains("swallowed"));
    }

    #[test]
    fn unclosed_raw_text_elements_are_contained() {
        for probe in ["<script>", "<style>", "<textarea>", "<xmp>", "<title>"] {
            let out = comment(&format!("{probe}rest of document"));
            assert!(
                !out.contains("<script") && !out.contains("<style") && !out.contains("<textarea"),
                "{probe} leaked markup: {out}"
            );
        }
    }

    #[test]
    fn chrome_classes_cannot_be_forged() {
        let out = comment("<a class=\"c-skip\" href=\"#t2\">skip 900 replies</a>");
        assert!(
            !out.contains("c-skip"),
            "forged skip link class survived: {out}"
        );
        assert!(!out.contains("href"), "forged skip target survived: {out}");
        assert!(out.contains("skip 900 replies"));
    }

    #[test]
    fn comment_fragment_links_cannot_aim_at_rustypub_anchors() {
        let out = comment("<a href=\"#c8\">jump</a>");
        assert_eq!(out, "<a>jump</a>");
    }

    #[test]
    fn article_footnote_anchors_survive_but_are_namespaced() {
        let out = fragment(
            "<p><a href=\"#fn1\">1</a></p><li id=\"fn1\">note</li>",
            Region::ArticleBody,
        );
        assert!(
            out.as_str().contains("href=\"#rp-fn1\""),
            "{}",
            out.as_str()
        );
        assert!(out.as_str().contains("id=\"rp-fn1\""), "{}", out.as_str());
        assert!(!out.as_str().contains("\"fn1\""));
    }

    #[test]
    fn output_is_balanced_so_assembly_cannot_be_broken() {
        // Every hostile fragment must serialize to markup that reparses with
        // its own wrapper intact — the property assembly depends on.
        for hostile in [
            "</div></div><h1>x</h1>",
            "<plaintext>",
            "<script>",
            "<body class=\"c-body\">",
            "<div><div><div>unclosed",
            "<table><tr><td>a",
        ] {
            let sanitized = comment(hostile);
            let assembled =
                format!("<div class=\"wrap\">{sanitized}</div><h1 id=\"after\">after</h1>");
            let doc = Document::from(assembled.as_str());
            assert_eq!(
                doc.select("h1#after").nodes().len(),
                1,
                "fragment {hostile:?} broke the following chapter: {sanitized}"
            );
            assert_eq!(
                doc.select(".wrap").nodes().len(),
                1,
                "fragment {hostile:?} escaped its wrapper: {sanitized}"
            );
        }
    }

    #[test]
    fn empty_and_whitespace_fragments_are_empty() {
        assert!(fragment("", Region::CommentBody).is_empty());
        assert!(fragment("   \n ", Region::CommentBody).is_empty());
    }
}
