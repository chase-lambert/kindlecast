use crate::model::Book;
use crate::sites::{Site, USER_AGENT, is_alnum, is_lower_alnum, parse_site_url, path_segments};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use dom_query::Document;

mod current;
mod json;
mod old;
mod tree;

pub struct Reddit;

impl Site for Reddit {
    fn name(&self) -> &'static str {
        "reddit"
    }

    fn matches(&self, url: &str) -> bool {
        parse_post_id(url).is_some() || is_share_url(url)
    }

    fn fetch(&self, url: &str, page_html: Option<String>, progress: &dyn Fn(&str)) -> Result<Book> {
        if let Some(ref html) = page_html
            && !html.trim().is_empty()
        {
            progress("using captured Reddit page");
            match parse_captured_page(html, url) {
                Ok((book, omitted)) => {
                    if omitted > 0 {
                        progress(&format!("{omitted} more comments omitted"));
                    }
                    return Ok(book);
                }
                Err(capture_err) => {
                    progress("captured page could not be parsed; trying JSON API");
                    match json::fetch_json_api(url, progress) {
                        Ok(book) => return Ok(book),
                        Err(json_err) => {
                            bail!(combined_error(&capture_err, &json_err));
                        }
                    }
                }
            }
        }
        json::fetch_json_api(url, progress)
    }
}

fn combined_error(capture_err: &anyhow::Error, json_err: &anyhow::Error) -> String {
    format!(
        "captured Reddit page could not be parsed: {capture_err}; JSON fallback also failed: {json_err}"
    )
}

// ---------------------------------------------------------------------------
// Captured-page parser
// ---------------------------------------------------------------------------

fn parse_captured_page(html: &str, input_url: &str) -> Result<(Book, usize)> {
    let doc = Document::from(html);

    if is_blocked_page(&doc) {
        bail!(
            "this Reddit page appears to be a login, consent, or bot-block page and cannot be used for discussion extraction"
        );
    }

    let cs_exists = !doc.select("shreddit-post").is_empty();
    let or_exists = !doc.select(".thing.link").is_empty();

    if cs_exists {
        match current::extract_current_desktop(&doc, input_url) {
            Ok(result) => Ok(result),
            Err(cs_err) => {
                if or_exists {
                    match old::extract_old_reddit(&doc, input_url) {
                        Ok(result) => Ok(result),
                        Err(or_err) => {
                            bail!(
                                "current desktop extraction failed: {cs_err}; old Reddit extraction also failed: {or_err}"
                            )
                        }
                    }
                } else {
                    Err(cs_err).context("shreddit-post found but could not be parsed")
                }
            }
        }
    } else if or_exists {
        old::extract_old_reddit(&doc, input_url)
    } else {
        bail!(
            "captured HTML does not contain a recognizable Reddit discussion (neither shreddit-post nor old Reddit .thing.link layout found); try opening the desktop or old Reddit page"
        );
    }
}

fn is_blocked_page(doc: &Document) -> bool {
    let has_post_marker =
        !doc.select("shreddit-post").is_empty() || !doc.select(".thing.link").is_empty();
    if has_post_marker {
        return false;
    }
    let body_text = doc
        .select("body")
        .get(0)
        .map(|el| el.text().to_lowercase())
        .unwrap_or_default();
    let blocked_markers = [
        "you've been blocked",
        "are you a human",
        "unusual traffic",
        "log in or sign up",
        "log in to reddit",
        "blocked by network",
    ];
    let has_login_form = !doc.select("form[action*=\"login\"]").is_empty()
        || !doc
            .select("input[name=\"username\"], input[name=\"password\"]")
            .is_empty();
    blocked_markers.iter().any(|m| body_text.contains(m)) || has_login_form
}

pub(super) fn first_node<'a>(doc: &'a Document, selector: &str) -> Option<dom_query::NodeRef<'a>> {
    doc.select(selector).get(0).copied()
}

/// Find a descendant of `node` by class name.
pub(super) fn desc_by_class<'a>(
    node: &dom_query::NodeRef<'a>,
    class: &str,
) -> Option<dom_query::NodeRef<'a>> {
    node.descendants_it().find(|d| d.has_class(class))
}

/// Find a descendant of `node` by tag name.
pub(super) fn desc_by_name<'a>(
    node: &dom_query::NodeRef<'a>,
    name: &str,
) -> Option<dom_query::NodeRef<'a>> {
    node.descendants_it().find(|d| d.has_name(name))
}

/// Find a descendant of `node` by tag name AND class.
pub(super) fn desc_by_name_and_class<'a>(
    node: &dom_query::NodeRef<'a>,
    name: &str,
    class: &str,
) -> Option<dom_query::NodeRef<'a>> {
    node.descendants_it()
        .find(|d| d.has_name(name) && d.has_class(class))
}

/// Find a `.usertext-body` then its `.md` child within a node.
pub(super) fn find_md_body<'a>(node: &dom_query::NodeRef<'a>) -> Option<dom_query::NodeRef<'a>> {
    let body = desc_by_class(node, "usertext-body")?;
    desc_by_class(&body, "md")
}

/// Get the direct child of `node` that has class `class`.
pub(super) fn direct_child_by_class<'a>(
    node: &dom_query::NodeRef<'a>,
    class: &str,
) -> Option<dom_query::NodeRef<'a>> {
    node.children().into_iter().find(|c| c.has_class(class))
}

/// Find the `[slot="comment"]` element belonging to this `shreddit-comment`,
/// without accidentally picking up a nested child comment's slot.
/// A candidate belongs to the current comment only if walking its ancestors
/// reaches this `shreddit-comment` before any other `shreddit-comment`.
pub(super) fn find_own_comment_slot<'a>(
    comment_el: &dom_query::NodeRef<'a>,
) -> Option<dom_query::NodeRef<'a>> {
    for candidate in comment_el.descendants_it() {
        if candidate.attr("slot").as_deref() != Some("comment") {
            continue;
        }
        // Verify: the first shreddit-comment ancestor must be this one
        let belongs = {
            let mut cur = candidate.parent();
            let mut ok = false;
            while let Some(node) = cur {
                if node.has_name("shreddit-comment") {
                    ok = node.id == comment_el.id;
                    break;
                }
                cur = node.parent();
            }
            ok
        };
        if belongs {
            return Some(candidate);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Old Reddit extraction
// ---------------------------------------------------------------------------

pub(super) fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(num) = trimmed.parse::<f64>() {
        if num >= 100_000_000_000.0 {
            return DateTime::<Utc>::from_timestamp((num / 1000.0) as i64, 0);
        } else if num >= 1.0 {
            return DateTime::<Utc>::from_timestamp(num as i64, 0);
        }
    }
    None
}

/// First run of digits in a "N more replies" label.
pub(super) fn parse_more_count(text: &str) -> Option<usize> {
    let digits: String = text
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

// ---------------------------------------------------------------------------
// Existing JSON path
// ---------------------------------------------------------------------------

/// Discussion hosts. Exact membership only: `ends_with("reddit.com")` would
/// quietly admit `np.reddit.com`, which the previous pattern rejected.
const DISCUSSION_HOSTS: &[&str] = &[
    "reddit.com",
    "www.reddit.com",
    "old.reddit.com",
    "new.reddit.com",
];
const SHORT_HOST: &[&str] = &["redd.it"];

pub(super) fn parse_post_id(url: &str) -> Option<String> {
    if let Some(parsed) = parse_site_url(url, SHORT_HOST, false)
        && let [id, ..] = path_segments(&parsed).as_slice()
        && is_lower_alnum(id)
    {
        return Some((*id).to_string());
    }

    let parsed = parse_site_url(url, DISCUSSION_HOSTS, false)?;
    let segments = path_segments(&parsed);
    // Either `/comments/<id>` or `/r/<sub>/comments/<id>` — nothing else may
    // precede `comments`, so an arbitrarily deep path cannot smuggle one in.
    let index = segments.iter().position(|segment| *segment == "comments")?;
    if index != 0 && !(index == 2 && segments[0] == "r") {
        return None;
    }
    let id = segments.get(index + 1)?;
    is_lower_alnum(id).then(|| (*id).to_string())
}

pub(super) fn is_share_url(url: &str) -> bool {
    let Some(parsed) = parse_site_url(url, DISCUSSION_HOSTS, false) else {
        return false;
    };
    // Share tokens are mixed case, unlike post ids.
    matches!(
        path_segments(&parsed).as_slice(),
        ["r", _subreddit, "s", token, ..] if is_alnum(token)
    )
}

pub(super) fn resolve_share_url(url: &str) -> Result<String> {
    // Share links only return a Location; do not follow redirects (need the header).
    let agent = crate::sites::agent_without_redirects();
    let response = agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("failed to resolve Reddit share URL {url}"))?;
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .context("Reddit share URL did not return a Location header")?;
    let resolved = location
        .strip_prefix('/')
        .map(|path| format!("https://www.reddit.com/{path}"))
        .unwrap_or_else(|| location.to_string());
    parse_post_id(&resolved).context("Reddit share URL did not resolve to a comments URL")
}

#[cfg(test)]
mod tests {
    use super::tree::{FlatComment, build_comment_tree};
    use super::*;
    use crate::model::{BookBody, comment_stats};
    use crate::sanitize::{self, Region};

    fn discussion(book: &Book) -> &crate::model::Discussion {
        match &book.body {
            BookBody::Discussion(discussion) => discussion,
            BookBody::Article => panic!("expected discussion"),
        }
    }

    // --- URL / JSON tests ---

    #[test]
    fn parses_reddit_url_variants() {
        assert_eq!(
            super::parse_post_id("https://www.reddit.com/r/rust/comments/abc123/title/").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            super::parse_post_id("https://www.reddit.com/r/rust/comments/abc123?context=3")
                .as_deref(),
            Some("abc123")
        );
        assert_eq!(
            super::parse_post_id("https://www.reddit.com/r/rust/comments/abc123#comments")
                .as_deref(),
            Some("abc123")
        );
        assert_eq!(
            super::parse_post_id("https://old.reddit.com/comments/abc123/title/").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            super::parse_post_id("https://redd.it/abc123?share_id=xyz").as_deref(),
            Some("abc123")
        );
        assert!(super::is_share_url(
            "https://www.reddit.com/r/rust/s/AbCd123"
        ));
        assert!(!super::is_share_url("https://www.reddit.com/s/AbCd123"));
    }

    #[test]
    fn builds_reddit_thread() {
        let json = include_str!("../fixtures/reddit_post_small.json");
        let listings = serde_json::from_str(json).unwrap();
        let (book, omitted) = super::json::build_thread(listings).unwrap();
        let d = discussion(&book);

        assert_eq!(book.story.id, "abc123");
        assert_eq!(book.source, "r/rust");
        assert_eq!(d.comment_count(), 2);
        assert_eq!(d.max_depth(), 1);
        assert_eq!(d.comments()[0].children[0].author, "carol");
        assert!(
            book.story
                .text_html
                .as_ref()
                .unwrap()
                .as_str()
                .contains("<div")
        );
        assert!(d.comments()[0].html.as_str().contains("&lt;"));
        assert_eq!(omitted, 4);
    }

    #[test]
    fn promotes_deleted_comment_replies() {
        let child = serde_json::json!({
            "author": "bob",
            "body_html": "<div class=\"md\"><p>child</p></div>",
            "created_utc": 1700000001.0,
            "replies": ""
        });
        let forest = super::json::build_comment(
            super::json::RedditComment {
                author: Some("[deleted]".to_string()),
                body_html: Some(String::new()),
                created_utc: Some(1700000000.0),
                replies: Some(super::json::Replies::Listing(super::json::Listing {
                    data: super::json::ListingData {
                        children: vec![super::json::Thing {
                            kind: "t1".to_string(),
                            data: child,
                        }],
                    },
                })),
            },
            0,
        );

        assert_eq!(forest.count, 1);
        assert_eq!(forest.max_depth, 0);
        assert_eq!(forest.comments[0].author, "bob");
        assert_eq!(forest.comments[0].depth, 0);
    }

    // --- Captured-page tests ---

    #[test]
    fn old_reddit_metadata_and_external_url() {
        let html = include_str!("../fixtures/reddit_old_small.html");
        let (book, omitted) = parse_captured_page(
            html,
            "https://old.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();

        assert_eq!(book.story.id, "xyz789");
        assert_eq!(book.story.title, "Test Reddit Post");
        assert_eq!(
            book.story.url.as_deref(),
            Some("https://example.com/article")
        );
        assert_eq!(book.story.author, "testuser");
        assert_eq!(book.story.points, Some(123));
        assert_eq!(book.source, "r/programming");
        assert_eq!(book.story.time.to_rfc3339(), "2024-01-15T10:30:00+00:00");
        assert!(
            book.story
                .discussion_url
                .as_deref()
                .unwrap()
                .contains("xyz789")
        );
        assert!(
            book.story
                .text_html
                .as_ref()
                .unwrap()
                .as_str()
                .contains("selftext")
        );
        assert_eq!(omitted, 5);
    }

    #[test]
    fn old_reddit_comment_tree_and_nesting() {
        let html = include_str!("../fixtures/reddit_old_small.html");
        let (book, _) = parse_captured_page(
            html,
            "https://old.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();
        let d = discussion(&book);

        // commenter1: depth 0, has child commenter2: depth 1
        // [deleted] empty: depth 0 (excluded), promotes child replier to depth 0
        // 3 visible comments in tree
        assert_eq!(d.comment_count(), 3);
        assert_eq!(d.max_depth(), 1);
        assert_eq!(d.comments().len(), 2);

        let top = &d.comments()[0];
        assert_eq!(top.author, "commenter1");
        assert_eq!(top.depth, 0);
        assert_eq!(top.children.len(), 1);
        assert_eq!(top.children[0].author, "commenter2");
        assert_eq!(top.children[0].depth, 1);

        // replier was under a deleted comment — promoted to depth 0
        let promoted = &d.comments()[1];
        assert_eq!(promoted.author, "replier");
        assert_eq!(promoted.depth, 0);
        assert!(promoted.html.as_str().contains("Reply to deleted"));
    }

    #[test]
    fn old_reddit_body_excludes_nested_comments_and_controls() {
        let html = include_str!("../fixtures/reddit_old_small.html");
        let (book, _) = parse_captured_page(
            html,
            "https://old.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();
        let d = discussion(&book);

        let top = &d.comments()[0];
        assert_eq!(top.html.as_str().trim(), "<p>Top-level reply</p>");
        assert!(!top.html.as_str().contains("Nested reply"));
        assert!(!top.html.as_str().contains("thing"));
        assert!(!top.html.as_str().contains("child"));
    }

    #[test]
    fn old_reddit_zero_comment_post() {
        let html = r#"<html><body>
<div class="thing id-t3_zc001 link" data-fullname="t3_zc001" data-permalink="/r/test/comments/zc001/nocomments/">
  <div class="entry unvoted">
    <p class="title"><a class="title" href="https://example.com">No Comments</a></p>
    <p class="tagline">
      submitted <time datetime="2024-06-01T00:00:00+00:00">1 year ago</time>
      by <a class="author">silent</a>
    </p>
  </div>
  <div class="midcol unvoted"><div class="score unvoted" title="1">1</div></div>
</div>
</body></html>"#;
        let (book, omitted) = parse_captured_page(
            html,
            "https://old.reddit.com/r/test/comments/zc001/nocomments/",
        )
        .unwrap();
        let d = discussion(&book);

        assert_eq!(book.story.id, "zc001");
        assert_eq!(d.comment_count(), 0);
        assert!(d.comments().is_empty());
        assert_eq!(omitted, 0);
    }

    #[test]
    fn current_desktop_metadata_and_self_post() {
        let html = include_str!("../fixtures/reddit_current_small.html");
        let (book, omitted) = parse_captured_page(
            html,
            "https://www.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();

        assert_eq!(book.story.id, "xyz789");
        assert_eq!(book.story.title, "Test Reddit Post");
        assert!(book.story.url.is_none());
        assert_eq!(book.story.author, "testuser");
        assert_eq!(book.story.points, Some(123));
        assert_eq!(book.source, "r/programming");
        assert!(
            book.story
                .text_html
                .as_ref()
                .unwrap()
                .as_str()
                .contains("Self-post body")
        );
        assert!(
            book.story
                .discussion_url
                .as_deref()
                .unwrap()
                .contains("xyz789")
        );
        assert_eq!(omitted, 3);
    }

    #[test]
    fn current_desktop_comment_tree() {
        let html = include_str!("../fixtures/reddit_current_small.html");
        let (book, _) = parse_captured_page(
            html,
            "https://www.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();
        let d = discussion(&book);

        assert_eq!(d.comment_count(), 3);
        assert_eq!(d.max_depth(), 1);

        let top = &d.comments()[0];
        assert_eq!(top.author, "commenter1");
        assert_eq!(top.depth, 0);
        assert_eq!(top.children.len(), 1);
        assert_eq!(top.children[0].author, "commenter2");
        assert_eq!(top.children[0].depth, 1);

        let promoted = &d.comments()[1];
        assert_eq!(promoted.author, "replier");
        assert_eq!(promoted.depth, 0);
        assert!(promoted.html.as_str().contains("Reply to deleted"));
    }

    #[test]
    fn current_desktop_body_excludes_nested() {
        let html = include_str!("../fixtures/reddit_current_small.html");
        let (book, _) = parse_captured_page(
            html,
            "https://www.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();
        let d = discussion(&book);

        let top = &d.comments()[0];
        assert_eq!(top.html.as_str().trim(), "<p>Top-level reply</p>");
        assert!(!top.html.as_str().contains("Nested reply"));
    }

    #[test]
    fn blocked_page_rejected() {
        let html = include_str!("../fixtures/reddit_blocked.html");
        let err = parse_captured_page(
            html,
            "https://www.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("login") || msg.contains("block") || msg.contains("consent"));
    }

    #[test]
    fn no_layout_fails_with_guidance() {
        let html = r#"<html><body><div>Just a normal page, not Reddit</div></body></html>"#;
        let err = parse_captured_page(
            html,
            "https://www.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("neither"));
    }

    #[test]
    fn combined_error_includes_both_causes() {
        let capture_err = anyhow::anyhow!("old Reddit post has no title");
        let json_err = anyhow::anyhow!("failed to decode Reddit thread abc123");
        let msg = super::combined_error(&capture_err, &json_err);
        assert!(msg.contains("old Reddit post has no title"));
        assert!(msg.contains("failed to decode Reddit thread abc123"));
    }

    #[test]
    fn parse_timestamp_rfc3339() {
        let ts = parse_timestamp("2024-01-15T10:30:00Z").unwrap();
        assert_eq!(ts.to_rfc3339(), "2024-01-15T10:30:00+00:00");
    }

    #[test]
    fn parse_timestamp_epoch_seconds() {
        // 2024-01-15T10:30:00Z = 1705314600 epoch seconds
        let ts = parse_timestamp("1705314600").unwrap();
        assert_eq!(ts.to_rfc3339(), "2024-01-15T10:30:00+00:00");
    }

    #[test]
    fn parse_timestamp_epoch_millis() {
        // 2024-01-15T10:30:00Z = 1705314600000 epoch milliseconds
        let ts = parse_timestamp("1705314600000").unwrap();
        assert_eq!(ts.to_rfc3339(), "2024-01-15T10:30:00+00:00");
    }

    #[test]
    fn parse_timestamp_small_number_treated_as_seconds() {
        let ts = parse_timestamp("100").unwrap();
        assert_eq!(ts.timestamp(), 100);
    }

    #[test]
    fn parse_timestamp_boundary_millis() {
        let ts = parse_timestamp("100000000000").unwrap();
        assert_eq!(ts.timestamp(), 100_000_000);
    }

    #[test]
    fn parse_more_count_extracts_number() {
        assert_eq!(parse_more_count("load more comments (5 replies)"), Some(5));
        assert_eq!(parse_more_count("3 more replies"), Some(3));
        assert_eq!(parse_more_count("12"), Some(12));
        assert_eq!(parse_more_count("no number here"), None);
    }

    #[test]
    fn share_url_captured_uses_dom_identity() {
        let html = include_str!("../fixtures/reddit_old_small.html");
        let (book, _) =
            parse_captured_page(html, "https://www.reddit.com/r/programming/s/AbCd123").unwrap();

        assert_eq!(book.story.id, "xyz789");
        let disc = book.story.discussion_url.as_deref().unwrap();
        assert!(disc.contains("/r/programming/comments/xyz789/"));
    }

    #[test]
    fn old_reddit_with_more_numbox_counts_omitted() {
        let html = r#"<html><body>
<div class="thing id-t3_om001 link" data-fullname="t3_om001" data-permalink="/r/test/comments/om001/post/">
  <div class="entry unvoted">
    <p class="title"><a class="title" href="https://example.com">Post</a></p>
    <p class="tagline">
      submitted <time datetime="2024-01-01T00:00:00+00:00">2 years ago</time>
      by <a class="author">poster</a>
    </p>
  </div>
</div>
<div class="sitetable nestedlisting">
  <div class="thing id-t1_c01 comment">
    <div class="entry unvoted">
      <p class="tagline"><a class="author">commenter</a><time datetime="2024-01-01T01:00:00+00:00">2y</time></p>
      <div class="usertext-body"><div class="md"><p>Hello</p></div></div>
    </div>
  </div>
  <div class="morecomments"><a class="numbox">7</a><a>load more</a></div>
</div>
</body></html>"#;
        let (book, omitted) =
            parse_captured_page(html, "https://old.reddit.com/r/test/comments/om001/post/")
                .unwrap();
        let d = discussion(&book);
        assert_eq!(d.comment_count(), 1);
        assert_eq!(omitted, 7);
    }

    #[test]
    fn malformed_depth_jumps_promoted() {
        let now = Utc::now();
        let flat = vec![
            FlatComment {
                author: "a".into(),
                time: now,
                html: sanitize::fragment("<p>A</p>", Region::CommentBody),
                depth: 0,
                is_deleted_empty: false,
            },
            FlatComment {
                author: "b".into(),
                time: now,
                html: sanitize::fragment("<p>B</p>", Region::CommentBody),
                depth: 2,
                is_deleted_empty: false,
            },
            FlatComment {
                author: "c".into(),
                time: now,
                html: sanitize::fragment("<p>C</p>", Region::CommentBody),
                depth: 0,
                is_deleted_empty: false,
            },
        ];
        let tree = build_comment_tree(&flat);
        let stats = comment_stats(&tree);
        assert_eq!(stats.count, 3);
        // b raw depth 2 normalized to 1 (prev 0 + 1)
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].author, "b");
        assert_eq!(tree[0].children[0].depth, 1);
        assert_eq!(tree[1].author, "c");
        assert_eq!(tree[1].depth, 0);
        assert_eq!(stats.max_depth, 1);
    }

    #[test]
    fn current_desktop_external_link_post() {
        let html = r#"<html><body>
<shreddit-post post-id="ext001" permalink="/r/test/comments/ext001/post/"
  post-title="External Link Post" author="linker" score="42"
  created-timestamp="2024-06-01T00:00:00.000Z"
  subreddit-prefixed-name="r/test"
  content-href="https://example.com/article">
</shreddit-post>
</body></html>"#;
        let (book, _) =
            parse_captured_page(html, "https://www.reddit.com/r/test/comments/ext001/post/")
                .unwrap();
        assert_eq!(book.story.id, "ext001");
        assert_eq!(book.story.title, "External Link Post");
        assert_eq!(
            book.story.url.as_deref(),
            Some("https://example.com/article")
        );
        assert!(book.story.text_html.is_none());
    }

    #[test]
    fn old_reddit_self_post() {
        let html = r#"<html><body>
<div class="thing id-t3_self01 link" data-fullname="t3_self01" data-permalink="/r/test/comments/self01/selfpost/">
  <div class="entry unvoted">
    <p class="title"><a class="title" href="/r/test/comments/self01/selfpost/">Self Post Title</a></p>
    <p class="tagline">
      submitted <time datetime="2024-06-01T00:00:00+00:00">1y</time>
      by <a class="author">selfposter</a>
    </p>
    <div class="expando"><div class="usertext-body"><div class="md"><p>Self-post text</p></div></div></div>
  </div>
</div>
</body></html>"#;
        let (book, _) = parse_captured_page(
            html,
            "https://old.reddit.com/r/test/comments/self01/selfpost/",
        )
        .unwrap();
        assert_eq!(book.story.id, "self01");
        assert_eq!(book.story.title, "Self Post Title");
        // Title link points to Reddit itself — no external URL
        assert!(book.story.url.is_none());
        assert!(
            book.story
                .text_html
                .as_ref()
                .unwrap()
                .as_str()
                .contains("Self-post text")
        );
    }

    #[test]
    fn layout_fallback_current_to_old() {
        // Invalid shreddit-post (no title) + valid old Reddit root
        let html = r#"<html><body>
<shreddit-post post-id="bad001"></shreddit-post>
<div class="thing id-t3_good01 link" data-fullname="t3_good01" data-permalink="/r/test/comments/good01/post/">
  <div class="entry unvoted">
    <p class="title"><a class="title" href="https://example.com">Good Old Post</a></p>
    <p class="tagline">
      submitted <time datetime="2024-06-01T00:00:00+00:00">1y</time>
      by <a class="author">olduser</a>
    </p>
  </div>
</div>
</body></html>"#;
        let (book, _) =
            parse_captured_page(html, "https://www.reddit.com/r/test/comments/good01/post/")
                .unwrap();
        assert_eq!(book.story.id, "good01");
        assert_eq!(book.story.title, "Good Old Post");
    }

    #[test]
    fn current_desktop_nested_body_isolation() {
        let html = include_str!("../fixtures/reddit_current_nested.html");
        let (book, _) =
            parse_captured_page(html, "https://www.reddit.com/r/test/comments/nest001/post/")
                .unwrap();
        let d = discussion(&book);
        // parent comment should contain only its own body, not child's
        let parent = &d.comments()[0];
        assert_eq!(parent.author, "parent");
        assert_eq!(parent.html.as_str().trim(), "<p>Parent body</p>");
        assert!(!parent.html.as_str().contains("Child body"));
        // child should be nested under parent
        assert_eq!(parent.children.len(), 1);
        assert_eq!(parent.children[0].author, "child");
        assert_eq!(parent.children[0].html.as_str().trim(), "<p>Child body</p>");
    }

    #[test]
    fn empty_page_gets_unsupported_layout_not_blocked() {
        let html = r#"<html><body></body></html>"#;
        let err = parse_captured_page(
            html,
            "https://www.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap_err();
        let msg = format!("{err}");
        // Empty page without login/consent markers → unsupported layout, not blocked
        assert!(
            msg.contains("neither"),
            "empty page should get unsupported-layout guidance, got: {msg}"
        );
    }

    #[test]
    fn wrapper_parent_does_not_duplicate_child_body() {
        // Parent has no own [slot="comment"]; child is inside a wrapper div.
        // Parent must not pick up the child's slot as its own body.
        let html = include_str!("../fixtures/reddit_current_wrapper.html");
        let (book, _) =
            parse_captured_page(html, "https://www.reddit.com/r/test/comments/wrap01/post/")
                .unwrap();
        let d = discussion(&book);
        // Parent should have empty body (no own slot), child promoted to top
        // Parent is treated as deleted-empty since it has no entry and has children
        let parent = &d.comments()[0];
        assert!(parent.html.as_str().trim().is_empty() || parent.author == "[deleted]");
        assert!(!parent.html.as_str().contains("Child body inside wrapper"));
        // Child should appear at depth 0 (promoted from parent)
        assert_eq!(parent.children.len(), 1);
        assert_eq!(parent.children[0].author, "child");
        assert!(
            parent.children[0]
                .html
                .as_str()
                .contains("Child body inside wrapper")
        );
    }

    #[test]
    fn current_omitted_attribute_button_dedup() {
        // Tree has more-comments-count="3" AND a child button "3 more replies".
        // Should count 3, not 6.
        let html = r#"<html><body>
<shreddit-post post-id="dedup01" permalink="/r/test/comments/dedup01/post/"
  post-title="Dedup Test" author="tester" score="1"
  created-timestamp="2024-06-01T00:00:00.000Z"
  subreddit-prefixed-name="r/test">
</shreddit-post>
<shreddit-comment-tree more-comments-count="3">
  <button aria-label="3 more replies">3 more replies</button>
</shreddit-comment-tree>
</body></html>"#;
        let (_, omitted) =
            parse_captured_page(html, "https://www.reddit.com/r/test/comments/dedup01/post/")
                .unwrap();
        assert_eq!(omitted, 3, "attribute+button should count once, not double");
    }

    #[test]
    fn current_omitted_button_only_fallback() {
        // Tree has no more-comments-count attribute, only a button "5 more replies".
        let html = r#"<html><body>
<shreddit-post post-id="btn001" permalink="/r/test/comments/btn001/post/"
  post-title="Button Test" author="tester" score="1"
  created-timestamp="2024-06-01T00:00:00.000Z"
  subreddit-prefixed-name="r/test">
</shreddit-post>
<shreddit-comment-tree>
  <button>5 more replies</button>
</shreddit-comment-tree>
</body></html>"#;
        let (_, omitted) =
            parse_captured_page(html, "https://www.reddit.com/r/test/comments/btn001/post/")
                .unwrap();
        assert_eq!(omitted, 5, "button-only tree should count fallback");
    }

    #[test]
    fn both_layouts_fail_preserves_both_causes() {
        // Both shreddit-post (invalid: no title) and .thing.link (invalid: no title) exist.
        // Error must contain both concrete causes, not hide old cause.
        let html = r#"<html><body>
<shreddit-post post-id="fail01"></shreddit-post>
<div class="thing id-t3_fail02 link" data-fullname="t3_fail02" data-permalink="/r/test/comments/fail02/">
  <div class="entry unvoted">
    <p class="tagline">no title here</p>
  </div>
</div>
</body></html>"#;
        let err = parse_captured_page(html, "https://www.reddit.com/r/test/comments/fail02/")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("current desktop extraction failed"),
            "should mention current cause, got: {msg}"
        );
        assert!(
            msg.contains("old Reddit extraction also failed"),
            "should mention old cause, got: {msg}"
        );
    }

    #[test]
    fn old_reddit_no_entry_with_child_promotes_without_body_duplication() {
        // Comment has no direct .entry but has a nested .thing.comment child.
        // Treated as empty placeholder; child promoted, no descendant body captured.
        let html = r#"<html><body>
<div class="thing id-t3_ne001 link" data-fullname="t3_ne001" data-permalink="/r/test/comments/ne001/post/">
  <div class="entry unvoted">
    <p class="title"><a class="title" href="https://example.com">No Entry Parent</a></p>
    <p class="tagline">
      submitted <time datetime="2024-06-01T00:00:00+00:00">1y</time>
      by <a class="author">poster</a>
    </p>
  </div>
</div>
<div class="sitetable nestedlisting">
  <div class="thing id-t1_ne002 comment" data-fullname="t1_ne002">
    <!-- no entry here -->
    <div class="child">
      <div class="thing id-t1_ne003 comment" data-fullname="t1_ne003">
        <div class="entry unvoted">
          <p class="tagline">
            <a class="author">childauthor</a>
            <time datetime="2024-06-01T01:00:00+00:00">1y</time>
          </p>
          <div class="usertext-body"><div class="md"><p>Child comment body</p></div></div>
        </div>
      </div>
    </div>
  </div>
</div>
</body></html>"#;
        let (book, _) =
            parse_captured_page(html, "https://old.reddit.com/r/test/comments/ne001/post/")
                .unwrap();
        let d = discussion(&book);
        // The wrapper (no .entry) is removed, child promoted to depth 0
        assert_eq!(d.comment_count(), 1);
        assert_eq!(d.comments().len(), 1);
        assert_eq!(d.comments()[0].author, "childauthor");
        assert_eq!(d.comments()[0].depth, 0);
        assert!(d.comments()[0].html.as_str().contains("Child comment body"));
        // Child's body should not be duplicated into the removed wrapper
    }

    #[test]
    fn parse_post_id_requires_exact_hosts_not_suffixes() {
        // `np.` was rejected by the old pattern; a suffix match would admit it.
        for url in [
            "https://np.reddit.com/r/rust/comments/abc123/",
            "https://reddit.com.evil.test/r/rust/comments/abc123/",
            "https://notreddit.com/r/rust/comments/abc123/",
            "https://www.reddit.com./r/rust/comments/abc123/",
        ] {
            assert_eq!(super::parse_post_id(url), None, "should reject {url}");
        }
        // Apex and the three known subdomains stay accepted.
        for host in [
            "reddit.com",
            "www.reddit.com",
            "old.reddit.com",
            "new.reddit.com",
        ] {
            assert_eq!(
                super::parse_post_id(&format!("https://{host}/r/rust/comments/abc123/")).as_deref(),
                Some("abc123"),
                "should accept {host}"
            );
        }
    }

    #[test]
    fn parse_post_id_rejects_authority_and_encoding_tricks() {
        for url in [
            "https://user:pass@www.reddit.com/r/rust/comments/abc123/",
            "https://www.reddit.com:8443/r/rust/comments/abc123/",
            "ftp://www.reddit.com/r/rust/comments/abc123/",
            "//www.reddit.com/r/rust/comments/abc123/",
            // Percent-encoding must not decode into a valid id.
            "https://redd.it/%61bc123",
            // Uppercase ids were rejected by `[a-z0-9]+`.
            "https://www.reddit.com/r/rust/comments/ABC123/",
            // `comments` may only follow the apex or `/r/<sub>/`.
            "https://www.reddit.com/user/x/r/rust/comments/abc123/",
            "https://www.reddit.com/a/b/comments/abc123/",
        ] {
            assert_eq!(super::parse_post_id(url), None, "should reject {url}");
        }
    }

    #[test]
    fn share_urls_keep_their_mixed_case_token_grammar() {
        assert!(super::is_share_url(
            "https://www.reddit.com/r/rust/s/AbCd123"
        ));
        assert!(super::is_share_url(
            "https://reddit.com/r/rust/s/AbCd123?x=1"
        ));
        for url in [
            "https://np.reddit.com/r/rust/s/AbCd123",
            "https://www.reddit.com/r/rust/s/",
            "https://www.reddit.com/r/rust/s/ab-cd",
            "https://user@www.reddit.com/r/rust/s/AbCd123",
            "https://www.reddit.com:8443/r/rust/s/AbCd123",
        ] {
            assert!(!super::is_share_url(url), "should reject {url}");
        }
    }

    #[test]
    fn more_count_reads_the_first_digit_run() {
        assert_eq!(super::parse_more_count("5 more replies"), Some(5));
        assert_eq!(super::parse_more_count("load more comments (42)"), Some(42));
        assert_eq!(super::parse_more_count("page 2 \u{b7} 5 replies"), Some(2));
        assert_eq!(super::parse_more_count("more replies"), None);
        assert_eq!(super::parse_more_count(""), None);
    }
}
