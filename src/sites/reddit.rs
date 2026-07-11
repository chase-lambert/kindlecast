use crate::model::{Comment, Story, Thread, ThreadKind, comment_stats, rebase_comments};
use crate::sites::{Site, USER_AGENT, fetch_json};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use dom_query::Document;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::sync::OnceLock;

pub struct Reddit;

#[derive(Debug, Deserialize)]
struct Listing {
    data: ListingData,
}

#[derive(Debug, Deserialize)]
struct ListingData {
    children: Vec<Thing>,
}

#[derive(Debug, Deserialize)]
struct Thing {
    kind: String,
    data: Value,
}

#[derive(Debug, Deserialize)]
struct RedditPost {
    id: String,
    title: String,
    author: String,
    selftext_html: Option<String>,
    url: Option<String>,
    permalink: String,
    score: Option<i64>,
    created_utc: f64,
    subreddit: String,
}

#[derive(Debug, Deserialize)]
struct RedditComment {
    author: Option<String>,
    body_html: Option<String>,
    created_utc: Option<f64>,
    replies: Option<Replies>,
}

#[derive(Debug, Deserialize)]
struct MoreComments {
    count: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Replies {
    Listing(Listing),
    Empty(String),
}

struct CommentForest {
    comments: Vec<Comment>,
    count: usize,
    max_depth: usize,
    omitted: usize,
}

#[derive(Clone)]
struct FlatComment {
    author: String,
    time: DateTime<Utc>,
    html: String,
    depth: usize,
    is_deleted_empty: bool,
}

impl Site for Reddit {
    fn name(&self) -> &'static str {
        "reddit"
    }

    fn matches(&self, url: &str) -> bool {
        parse_post_id(url).is_some() || is_share_url(url)
    }

    fn fetch(
        &self,
        url: &str,
        page_html: Option<String>,
        progress: &dyn Fn(&str),
    ) -> Result<Thread> {
        if let Some(ref html) = page_html
            && !html.trim().is_empty()
        {
            progress("using captured Reddit page");
            match parse_captured_page(html, url) {
                Ok((thread, omitted)) => {
                    if omitted > 0 {
                        progress(&format!("{omitted} more comments omitted"));
                    }
                    return Ok(thread);
                }
                Err(capture_err) => {
                    progress("captured page could not be parsed; trying JSON API");
                    match fetch_json_api(url, progress) {
                        Ok(thread) => return Ok(thread),
                        Err(json_err) => {
                            bail!(combined_error(&capture_err, &json_err));
                        }
                    }
                }
            }
        }
        fetch_json_api(url, progress)
    }
}

fn fetch_json_api(url: &str, progress: &dyn Fn(&str)) -> Result<Thread> {
    let id = parse_post_id(url)
        .map(Ok)
        .unwrap_or_else(|| resolve_share_url(url))?;
    let api_url = format!("https://www.reddit.com/comments/{id}.json?raw_json=1&limit=500");
    progress(&format!("fetching post {id}"));
    let listings = fetch_json::<Vec<Listing>>(&api_url)
        .with_context(|| format!("failed to decode Reddit thread {id} (Reddit may be blocking unauthenticated API access; try using the browser extension instead)"))?;
    let (thread, omitted) = build_thread(listings)?;
    if omitted > 0 {
        progress(&format!("{omitted} more comments omitted"));
    }
    Ok(thread)
}

fn combined_error(capture_err: &anyhow::Error, json_err: &anyhow::Error) -> String {
    format!(
        "captured Reddit page could not be parsed: {capture_err}; JSON fallback also failed: {json_err}"
    )
}

// ---------------------------------------------------------------------------
// Captured-page parser
// ---------------------------------------------------------------------------

fn parse_captured_page(html: &str, input_url: &str) -> Result<(Thread, usize)> {
    let doc = Document::from(html);

    if is_blocked_page(&doc) {
        bail!(
            "this Reddit page appears to be a login, consent, or bot-block page and cannot be used for discussion extraction"
        );
    }

    let cs_exists = !doc.select("shreddit-post").is_empty();
    let or_exists = !doc.select(".thing.link").is_empty();

    if cs_exists {
        match extract_current_desktop(&doc, input_url) {
            Ok(result) => Ok(result),
            Err(cs_err) => {
                if or_exists {
                    match extract_old_reddit(&doc, input_url) {
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
        extract_old_reddit(&doc, input_url)
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

fn first_node<'a>(doc: &'a Document, selector: &str) -> Option<dom_query::NodeRef<'a>> {
    doc.select(selector).get(0).copied()
}

/// Find a descendant of `node` by class name.
fn desc_by_class<'a>(node: &dom_query::NodeRef<'a>, class: &str) -> Option<dom_query::NodeRef<'a>> {
    node.descendants_it().find(|d| d.has_class(class))
}

/// Find a descendant of `node` by tag name.
fn desc_by_name<'a>(node: &dom_query::NodeRef<'a>, name: &str) -> Option<dom_query::NodeRef<'a>> {
    node.descendants_it().find(|d| d.has_name(name))
}

/// Find a descendant of `node` by tag name AND class.
fn desc_by_name_and_class<'a>(
    node: &dom_query::NodeRef<'a>,
    name: &str,
    class: &str,
) -> Option<dom_query::NodeRef<'a>> {
    node.descendants_it()
        .find(|d| d.has_name(name) && d.has_class(class))
}

/// Find a `.usertext-body` then its `.md` child within a node.
fn find_md_body<'a>(node: &dom_query::NodeRef<'a>) -> Option<dom_query::NodeRef<'a>> {
    let body = desc_by_class(node, "usertext-body")?;
    desc_by_class(&body, "md")
}

/// Get the direct child of `node` that has class `class`.
fn direct_child_by_class<'a>(
    node: &dom_query::NodeRef<'a>,
    class: &str,
) -> Option<dom_query::NodeRef<'a>> {
    node.children().into_iter().find(|c| c.has_class(class))
}

/// Find the `[slot="comment"]` element belonging to this `shreddit-comment`,
/// without accidentally picking up a nested child comment's slot.
/// A candidate belongs to the current comment only if walking its ancestors
/// reaches this `shreddit-comment` before any other `shreddit-comment`.
fn find_own_comment_slot<'a>(
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

fn extract_old_reddit(doc: &Document, input_url: &str) -> Result<(Thread, usize)> {
    let link_el = first_node(doc, ".thing.link")
        .context("no .thing.link element found in old Reddit layout")?;

    let fullname = link_el.attr("data-fullname");
    let id = fullname
        .as_deref()
        .unwrap_or("")
        .strip_prefix("t3_")
        .unwrap_or(fullname.as_deref().unwrap_or(""))
        .to_string();
    let permalink = link_el
        .attr("data-permalink")
        .as_deref()
        .unwrap_or("")
        .to_string();

    let entry = direct_child_by_class(&link_el, "entry")
        .or_else(|| desc_by_class(&link_el, "entry"))
        .context("post entry not found in old Reddit layout")?;

    let title_link = desc_by_name_and_class(&entry, "a", "title");
    let title = title_link
        .as_ref()
        .map(|a| a.text().trim().to_string())
        .unwrap_or_default();
    let title_href = title_link.and_then(|a| a.attr("href").as_deref().map(|s| s.to_string()));

    let external_url = title_href.as_ref().and_then(|href| {
        let is_reddit_self = href.starts_with("/r/") || href.contains("reddit.com/r/");
        if is_reddit_self {
            None
        } else {
            Some(href.clone())
        }
    });

    let author = desc_by_class(&entry, "author")
        .map(|a| a.text().trim().to_string())
        .unwrap_or_default();

    let score = doc
        .select(".thing.link .midcol .score, .thing.link .score.unvoted, .thing.link .score")
        .get(0)
        .and_then(|s| {
            let text = s
                .attr("title")
                .as_deref()
                .map(|v| v.to_string())
                .unwrap_or_else(|| s.text().trim().to_string());
            if text.is_empty() {
                None
            } else {
                text.parse::<i64>().ok()
            }
        });

    let timestamp = desc_by_name(&entry, "time")
        .and_then(|t| t.attr("datetime"))
        .as_deref()
        .and_then(parse_timestamp)
        .unwrap_or_else(Utc::now);

    let subreddit = desc_by_class(&entry, "subreddit")
        .map(|s| {
            s.text()
                .trim()
                .trim_start_matches("/r/")
                .trim_start_matches("r/")
                .to_string()
        })
        .unwrap_or_else(|| "unknown".to_string());

    let selftext_html = link_el
        .descendants_it()
        .find(|d| d.has_class("expando"))
        .and_then(|exp| find_md_body(&exp))
        .or_else(|| find_md_body(&link_el))
        .map(|md| clean_body_html(md.inner_html().as_ref()))
        .filter(|s| !s.trim().is_empty());

    let discussion_url = if !permalink.is_empty() {
        if permalink.starts_with("https://") || permalink.starts_with("http://") {
            permalink
        } else {
            format!("https://www.reddit.com{permalink}")
        }
    } else {
        desc_by_name_and_class(&entry, "a", "comments")
            .and_then(|a| a.attr("href").as_deref().map(|s| s.to_string()))
            .unwrap_or_else(|| input_url.to_string())
    };

    if id.is_empty() {
        bail!("old Reddit post has no usable ID");
    }
    if title.is_empty() {
        bail!("old Reddit post has no title");
    }

    let mut flat_comments: Vec<FlatComment> = Vec::new();
    let mut omitted: usize = 0;

    for comment_el in doc.select(".thing.comment").nodes() {
        let mut depth = count_thing_ancestors(comment_el);
        if depth == 0
            && let Some(cls) = comment_el.attr("class").as_deref()
        {
            for part in cls.split_whitespace() {
                if let Some(d) = part
                    .strip_prefix("depth-")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    depth = d;
                    break;
                }
            }
        }

        let comment_entry = direct_child_by_class(comment_el, "entry");

        let author = comment_entry
            .as_ref()
            .and_then(|e| desc_by_class(e, "author"))
            .map(|a| a.text().trim().to_string())
            .unwrap_or_default();

        let body_html = comment_entry
            .as_ref()
            .and_then(find_md_body)
            .map(|md| clean_body_html(md.inner_html().as_ref()))
            .unwrap_or_default();

        let time = comment_entry
            .as_ref()
            .and_then(|e| desc_by_name(e, "time"))
            .and_then(|t| t.attr("datetime"))
            .as_deref()
            .and_then(parse_timestamp)
            .unwrap_or_else(Utc::now);

        let is_deleted_empty = if comment_entry.is_none() && !comment_el.children().is_empty() {
            true
        } else {
            author == "[deleted]" && body_html.trim().is_empty()
        };

        flat_comments.push(FlatComment {
            author,
            time,
            html: body_html,
            depth,
            is_deleted_empty,
        });
    }

    for more_el in doc
        .select(".morecomments .numbox, .thing.more .numbox")
        .nodes()
    {
        if let Ok(count) = more_el.text().trim().parse::<usize>() {
            omitted += count;
        }
    }
    for more_el in doc.select(".thing.more a, .morecomments a").nodes() {
        if more_el.has_class("numbox") {
            continue;
        }
        let text = more_el.text();
        if let Some(count) = parse_more_count(&text) {
            omitted += count;
        }
    }

    let comments = build_comment_tree(&flat_comments);
    let stats = comment_stats(&comments);

    let thread = Thread {
        kind: ThreadKind::Discussion,
        story: Story {
            id,
            title,
            url: external_url,
            discussion_url: Some(discussion_url),
            author,
            points: score,
            time: timestamp,
            text_html: selftext_html,
        },
        comments,
        comment_count: stats.count,
        max_depth: stats.max_depth,
        source: format!("r/{}", subreddit),
        source_slug: "reddit".to_string(),
    };

    Ok((thread, omitted))
}

fn count_thing_ancestors(el: &dom_query::NodeRef) -> usize {
    let mut depth: usize = 0;
    let mut current = el.parent();
    while let Some(node) = current {
        if node.is(".thing.comment") {
            depth += 1;
        }
        current = node.parent();
    }
    depth
}

// ---------------------------------------------------------------------------
// Current desktop (shreddit-*) extraction
// ---------------------------------------------------------------------------

fn extract_current_desktop(doc: &Document, input_url: &str) -> Result<(Thread, usize)> {
    let post = first_node(doc, "shreddit-post").context("no shreddit-post element found")?;

    let id = {
        let post_id = post.attr("post-id").as_deref().map(|s| s.to_string());
        let thingid = post.attr("thingid").as_deref().map(|s| s.to_string());
        let elem_id = post.attr("id").as_deref().map(|s| s.to_string());
        post_id
            .or(thingid)
            .or(elem_id)
            .map(|s| s.strip_prefix("t3_").unwrap_or(&s).to_string())
            .unwrap_or_default()
    };

    let permalink = post.attr("permalink").as_deref().unwrap_or("").to_string();

    let title = post
        .attr("post-title")
        .as_deref()
        .map(|s| s.to_string())
        .or_else(|| {
            post.descendants_it()
                .find(|d| d.attr("slot").as_deref() == Some("title"))
                .map(|el| el.text().trim().to_string())
        })
        .or_else(|| desc_by_name(&post, "h1").map(|h1| h1.text().trim().to_string()))
        .unwrap_or_default();

    let author = post.attr("author").as_deref().unwrap_or("").to_string();

    let score = post
        .attr("score")
        .as_deref()
        .and_then(|s| s.parse::<i64>().ok());

    let timestamp = post
        .attr("created-timestamp")
        .as_deref()
        .and_then(parse_timestamp)
        .unwrap_or_else(Utc::now);

    let subreddit = post
        .attr("subreddit-prefixed-name")
        .as_deref()
        .map(|s| s.trim().trim_start_matches("r/").to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let content_href = post.attr("content-href").as_deref().map(|s| s.to_string());
    let external_url = content_href.as_ref().and_then(|href| {
        if href.contains("reddit.com") || href.starts_with('/') {
            None
        } else {
            Some(href.clone())
        }
    });

    let selftext_html = post
        .descendants_it()
        .find(|d| d.attr("slot").as_deref() == Some("text-body"))
        .map(|el| clean_body_html(el.inner_html().as_ref()))
        .filter(|s| !s.trim().is_empty());

    let discussion_url = if !permalink.is_empty() {
        if permalink.starts_with("https://") || permalink.starts_with("http://") {
            permalink
        } else {
            format!("https://www.reddit.com{permalink}")
        }
    } else {
        input_url.to_string()
    };

    if id.is_empty() {
        bail!("shreddit-post has no usable ID (checked post-id, thingid, id)");
    }
    if title.is_empty() {
        bail!("shreddit-post has no title");
    }

    let mut flat_comments: Vec<FlatComment> = Vec::new();
    let mut omitted: usize = 0;

    for comment_el in doc.select("shreddit-comment").nodes() {
        let depth = comment_el
            .attr("depth")
            .as_deref()
            .and_then(|d| d.parse::<usize>().ok())
            .unwrap_or_else(|| {
                let mut d = 0usize;
                let mut cur = comment_el.parent();
                while let Some(node) = cur {
                    if node.has_name("shreddit-comment") {
                        d += 1;
                    }
                    cur = node.parent();
                }
                d
            });

        let author = comment_el
            .attr("author")
            .as_deref()
            .unwrap_or("")
            .to_string();

        let body_html = find_own_comment_slot(comment_el)
            .map(|el| clean_body_html(el.inner_html().as_ref()))
            .unwrap_or_default();

        let time = comment_el
            .attr("created-timestamp")
            .as_deref()
            .and_then(parse_timestamp)
            .unwrap_or_else(Utc::now);

        let is_deleted_empty = author == "[deleted]" && body_html.trim().is_empty();

        flat_comments.push(FlatComment {
            author,
            time,
            html: body_html,
            depth,
            is_deleted_empty,
        });
    }

    for tree_el in doc
        .select("shreddit-comment-tree[more-comments-count]")
        .nodes()
    {
        if let Some(count) = tree_el
            .attr("more-comments-count")
            .as_deref()
            .and_then(|s| s.parse::<usize>().ok())
        {
            omitted += count;
        }
    }
    // Fallback: for shreddit-comment-tree elements without the attribute,
    // look for a recognizable numeric "more replies" control inside.
    for tree_el in doc.select("shreddit-comment-tree").nodes() {
        if tree_el.attr("more-comments-count").as_deref().is_some() {
            continue; // already counted above
        }
        for child in tree_el.descendants_it() {
            if child.has_name("button") || child.has_name("a") {
                let text = child.text();
                if let Some(count) = parse_more_count(&text) {
                    omitted += count;
                    break; // one placeholder per tree
                }
            }
        }
    }

    let comments = build_comment_tree(&flat_comments);
    let stats = comment_stats(&comments);

    let thread = Thread {
        kind: ThreadKind::Discussion,
        story: Story {
            id,
            title,
            url: external_url,
            discussion_url: Some(discussion_url),
            author,
            points: score,
            time: timestamp,
            text_html: selftext_html,
        },
        comments,
        comment_count: stats.count,
        max_depth: stats.max_depth,
        source: format!("r/{}", subreddit),
        source_slug: "reddit".to_string(),
    };

    Ok((thread, omitted))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn build_comment_tree(flat: &[FlatComment]) -> Vec<Comment> {
    let mut normalized = flat.to_vec();
    let mut prev_norm: usize = 0;
    for (i, item) in normalized.iter_mut().enumerate() {
        if i == 0 {
            item.depth = 0;
        } else if item.depth > prev_norm + 1 {
            item.depth = prev_norm + 1;
        }
        prev_norm = item.depth;
    }
    let mut index = 0;
    build_comments(&normalized, &mut index, 0)
}

fn build_comments(flat: &[FlatComment], index: &mut usize, depth: usize) -> Vec<Comment> {
    let mut out = Vec::new();
    while let Some(item) = flat.get(*index) {
        if item.depth < depth {
            break;
        }
        if item.depth > depth {
            let promoted = build_comments(flat, index, item.depth);
            out.extend(promoted);
            continue;
        }
        *index += 1;
        let children = build_comments(flat, index, depth + 1);
        if item.is_deleted_empty {
            out.extend(rebase_comments(children, depth));
        } else {
            out.push(Comment {
                author: item.author.clone(),
                time: item.time,
                html: item.html.clone(),
                depth,
                children,
            });
        }
    }
    out
}

fn clean_body_html(html: &str) -> String {
    static CLEAN_RE: OnceLock<Regex> = OnceLock::new();
    CLEAN_RE
        .get_or_init(|| {
            Regex::new(
                r"(?is)<script\b[^>]*>.*?</script\s*>|<style\b[^>]*>.*?</style\s*>|<template\b[^>]*>.*?</template\s*>",
            )
            .unwrap()
        })
        .replace_all(html, "")
        .trim()
        .to_string()
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
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

fn parse_more_count(text: &str) -> Option<usize> {
    static MORE_RE: OnceLock<Regex> = OnceLock::new();
    MORE_RE
        .get_or_init(|| Regex::new(r"(\d+)\s*(?:more\s*)?(?:repl(?:y|ies))?").unwrap())
        .captures(text)
        .and_then(|caps| caps.get(1))
        .and_then(|m| m.as_str().parse::<usize>().ok())
}

// ---------------------------------------------------------------------------
// Existing JSON path
// ---------------------------------------------------------------------------

fn parse_post_id(url: &str) -> Option<String> {
    static COMMENTS_RE: OnceLock<Regex> = OnceLock::new();
    static SHORT_RE: OnceLock<Regex> = OnceLock::new();
    COMMENTS_RE
        .get_or_init(|| {
            Regex::new(
                r"^https?://(?:www\.|old\.|new\.)?reddit\.com/(?:r/[^/]+/)?comments/([a-z0-9]+)(?:[/?#].*)?$",
            )
            .unwrap()
        })
        .captures(url)
        .and_then(|captures| captures.get(1))
        .or_else(|| {
            SHORT_RE
                .get_or_init(|| {
                    Regex::new(r"^https?://redd\.it/([a-z0-9]+)(?:[/?#].*)?$").unwrap()
                })
                .captures(url)
                .and_then(|captures| captures.get(1))
        })
        .map(|m| m.as_str().to_string())
}

fn is_share_url(url: &str) -> bool {
    static SHARE_RE: OnceLock<Regex> = OnceLock::new();
    SHARE_RE
        .get_or_init(|| {
            Regex::new(
                r"^https?://(?:www\.|old\.|new\.)?reddit\.com/r/[^/]+/s/[A-Za-z0-9]+(?:[/?#].*)?$",
            )
            .unwrap()
        })
        .is_match(url)
}

fn resolve_share_url(url: &str) -> Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .max_redirects_will_error(false)
        .build()
        .into();
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

fn build_thread(listings: Vec<Listing>) -> Result<(Thread, usize)> {
    let mut listings = listings.into_iter();
    let post = listings
        .next()
        .and_then(|listing| {
            listing
                .data
                .children
                .into_iter()
                .find(|thing| thing.kind == "t3")
        })
        .map(|thing| serde_json::from_value::<RedditPost>(thing.data))
        .transpose()?
        .context("Reddit response did not contain a post")?;
    let forest = listings
        .next()
        .map(|listing| build_comment_forest(listing.data.children, 0))
        .unwrap_or_else(empty_forest);
    let comment_count = forest.count;
    let max_depth = forest.max_depth;
    let omitted = forest.omitted;
    let discussion_url = format!("https://www.reddit.com{}", post.permalink);
    Ok((
        Thread {
            kind: ThreadKind::Discussion,
            story: Story {
                id: post.id,
                title: post.title,
                url: post.url.filter(|url| !url.trim().is_empty()),
                discussion_url: Some(discussion_url),
                author: post.author,
                points: post.score,
                time: utc_from_timestamp(post.created_utc),
                text_html: post.selftext_html.and_then(non_empty_html),
            },
            comments: forest.comments,
            comment_count,
            max_depth,
            source: format!("r/{}", post.subreddit),
            source_slug: "reddit".to_string(),
        },
        omitted,
    ))
}

fn build_comment_forest(things: Vec<Thing>, depth: usize) -> CommentForest {
    things.into_iter().fold(empty_forest(), |forest, thing| {
        merge_forest(forest, build_thing(thing, depth))
    })
}

fn build_thing(thing: Thing, depth: usize) -> CommentForest {
    match thing.kind.as_str() {
        "t1" => serde_json::from_value::<RedditComment>(thing.data)
            .map(|comment| build_comment(comment, depth))
            .unwrap_or_else(|_| empty_forest()),
        "more" => more_forest(
            serde_json::from_value::<MoreComments>(thing.data)
                .ok()
                .and_then(|more| more.count)
                .unwrap_or(0),
        ),
        _ => empty_forest(),
    }
}

fn build_comment(raw: RedditComment, depth: usize) -> CommentForest {
    let author = raw.author.unwrap_or_default();
    let html = raw.body_html.and_then(non_empty_html).unwrap_or_default();
    let children = match raw.replies {
        Some(Replies::Listing(listing)) => build_comment_forest(listing.data.children, depth + 1),
        Some(Replies::Empty(value)) => {
            let _ = value.is_empty();
            empty_forest()
        }
        _ => empty_forest(),
    };
    if author == "[deleted]" && html.trim().is_empty() {
        return rebase_forest(children, depth);
    }

    CommentForest {
        count: 1 + children.count,
        max_depth: depth.max(children.max_depth),
        omitted: children.omitted,
        comments: vec![Comment {
            author,
            time: raw
                .created_utc
                .map(utc_from_timestamp)
                .unwrap_or_else(Utc::now),
            html,
            depth,
            children: children.comments,
        }],
    }
}

fn empty_forest() -> CommentForest {
    CommentForest {
        comments: Vec::new(),
        count: 0,
        max_depth: 0,
        omitted: 0,
    }
}

fn more_forest(omitted: usize) -> CommentForest {
    CommentForest {
        omitted,
        ..empty_forest()
    }
}

fn merge_forest(mut left: CommentForest, right: CommentForest) -> CommentForest {
    left.comments.extend(right.comments);
    CommentForest {
        comments: left.comments,
        count: left.count + right.count,
        max_depth: left.max_depth.max(right.max_depth),
        omitted: left.omitted + right.omitted,
    }
}

fn rebase_forest(forest: CommentForest, root_depth: usize) -> CommentForest {
    let comments = rebase_comments(forest.comments, root_depth);
    let stats = comment_stats(&comments);
    CommentForest {
        comments,
        count: stats.count,
        max_depth: stats.max_depth,
        omitted: forest.omitted,
    }
}

fn non_empty_html(value: String) -> Option<String> {
    let html = value.trim().to_string();
    (!html.is_empty()).then_some(html)
}

fn utc_from_timestamp(timestamp: f64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(timestamp as i64, 0).unwrap_or_else(Utc::now)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
        let json = include_str!("fixtures/reddit_post_small.json");
        let listings = serde_json::from_str(json).unwrap();
        let (thread, omitted) = super::build_thread(listings).unwrap();

        assert_eq!(thread.story.id, "abc123");
        assert_eq!(thread.source, "r/rust");
        assert_eq!(thread.comment_count, 2);
        assert_eq!(thread.max_depth, 1);
        assert_eq!(thread.comments[0].children[0].author, "carol");
        assert!(thread.story.text_html.unwrap().contains("<div"));
        assert!(thread.comments[0].html.contains("&lt;"));
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
        let forest = super::build_comment(
            super::RedditComment {
                author: Some("[deleted]".to_string()),
                body_html: Some(String::new()),
                created_utc: Some(1700000000.0),
                replies: Some(super::Replies::Listing(super::Listing {
                    data: super::ListingData {
                        children: vec![super::Thing {
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
        let html = include_str!("fixtures/reddit_old_small.html");
        let (thread, omitted) = parse_captured_page(
            html,
            "https://old.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();

        assert_eq!(thread.story.id, "xyz789");
        assert_eq!(thread.story.title, "Test Reddit Post");
        assert_eq!(
            thread.story.url.as_deref(),
            Some("https://example.com/article")
        );
        assert_eq!(thread.story.author, "testuser");
        assert_eq!(thread.story.points, Some(123));
        assert_eq!(thread.source, "r/programming");
        assert_eq!(thread.story.time.to_rfc3339(), "2024-01-15T10:30:00+00:00");
        assert!(
            thread
                .story
                .discussion_url
                .as_deref()
                .unwrap()
                .contains("xyz789")
        );
        assert!(thread.story.text_html.unwrap().contains("selftext"));
        assert_eq!(omitted, 5);
    }

    #[test]
    fn old_reddit_comment_tree_and_nesting() {
        let html = include_str!("fixtures/reddit_old_small.html");
        let (thread, _) = parse_captured_page(
            html,
            "https://old.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();

        // commenter1: depth 0, has child commenter2: depth 1
        // [deleted] empty: depth 0 (excluded), promotes child replier to depth 0
        // 3 visible comments in tree
        assert_eq!(thread.comment_count, 3);
        assert_eq!(thread.max_depth, 1);
        assert_eq!(thread.comments.len(), 2);

        let top = &thread.comments[0];
        assert_eq!(top.author, "commenter1");
        assert_eq!(top.depth, 0);
        assert_eq!(top.children.len(), 1);
        assert_eq!(top.children[0].author, "commenter2");
        assert_eq!(top.children[0].depth, 1);

        // replier was under a deleted comment — promoted to depth 0
        let promoted = &thread.comments[1];
        assert_eq!(promoted.author, "replier");
        assert_eq!(promoted.depth, 0);
        assert!(promoted.html.contains("Reply to deleted"));
    }

    #[test]
    fn old_reddit_body_excludes_nested_comments_and_controls() {
        let html = include_str!("fixtures/reddit_old_small.html");
        let (thread, _) = parse_captured_page(
            html,
            "https://old.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();

        let top = &thread.comments[0];
        assert_eq!(top.html.trim(), "<p>Top-level reply</p>");
        assert!(!top.html.contains("Nested reply"));
        assert!(!top.html.contains("thing"));
        assert!(!top.html.contains("child"));
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
        let (thread, omitted) = parse_captured_page(
            html,
            "https://old.reddit.com/r/test/comments/zc001/nocomments/",
        )
        .unwrap();

        assert_eq!(thread.story.id, "zc001");
        assert_eq!(thread.comment_count, 0);
        assert!(thread.comments.is_empty());
        assert_eq!(omitted, 0);
    }

    #[test]
    fn current_desktop_metadata_and_self_post() {
        let html = include_str!("fixtures/reddit_current_small.html");
        let (thread, omitted) = parse_captured_page(
            html,
            "https://www.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();

        assert_eq!(thread.story.id, "xyz789");
        assert_eq!(thread.story.title, "Test Reddit Post");
        assert!(thread.story.url.is_none());
        assert_eq!(thread.story.author, "testuser");
        assert_eq!(thread.story.points, Some(123));
        assert_eq!(thread.source, "r/programming");
        assert!(thread.story.text_html.unwrap().contains("Self-post body"));
        assert!(
            thread
                .story
                .discussion_url
                .as_deref()
                .unwrap()
                .contains("xyz789")
        );
        assert_eq!(omitted, 3);
    }

    #[test]
    fn current_desktop_comment_tree() {
        let html = include_str!("fixtures/reddit_current_small.html");
        let (thread, _) = parse_captured_page(
            html,
            "https://www.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();

        assert_eq!(thread.comment_count, 3);
        assert_eq!(thread.max_depth, 1);

        let top = &thread.comments[0];
        assert_eq!(top.author, "commenter1");
        assert_eq!(top.depth, 0);
        assert_eq!(top.children.len(), 1);
        assert_eq!(top.children[0].author, "commenter2");
        assert_eq!(top.children[0].depth, 1);

        let promoted = &thread.comments[1];
        assert_eq!(promoted.author, "replier");
        assert_eq!(promoted.depth, 0);
        assert!(promoted.html.contains("Reply to deleted"));
    }

    #[test]
    fn current_desktop_body_excludes_nested() {
        let html = include_str!("fixtures/reddit_current_small.html");
        let (thread, _) = parse_captured_page(
            html,
            "https://www.reddit.com/r/programming/comments/xyz789/test_post/",
        )
        .unwrap();

        let top = &thread.comments[0];
        assert_eq!(top.html.trim(), "<p>Top-level reply</p>");
        assert!(!top.html.contains("Nested reply"));
    }

    #[test]
    fn blocked_page_rejected() {
        let html = include_str!("fixtures/reddit_blocked.html");
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
        let html = include_str!("fixtures/reddit_old_small.html");
        let (thread, _) =
            parse_captured_page(html, "https://www.reddit.com/r/programming/s/AbCd123").unwrap();

        assert_eq!(thread.story.id, "xyz789");
        let disc = thread.story.discussion_url.as_deref().unwrap();
        assert!(disc.contains("/r/programming/comments/xyz789/"));
    }

    #[test]
    fn clean_body_strips_script_and_style() {
        let html = "<p>Hello</p><script>bad()</script><style>.x{}</style><p>World</p>";
        let cleaned = clean_body_html(html);
        assert_eq!(cleaned, "<p>Hello</p><p>World</p>");
        assert!(!cleaned.contains("script"));
        assert!(!cleaned.contains("style"));
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
        let (thread, omitted) =
            parse_captured_page(html, "https://old.reddit.com/r/test/comments/om001/post/")
                .unwrap();
        assert_eq!(thread.comment_count, 1);
        assert_eq!(omitted, 7);
    }

    #[test]
    fn malformed_depth_jumps_promoted() {
        let now = Utc::now();
        let flat = vec![
            FlatComment {
                author: "a".into(),
                time: now,
                html: "<p>A</p>".into(),
                depth: 0,
                is_deleted_empty: false,
            },
            FlatComment {
                author: "b".into(),
                time: now,
                html: "<p>B</p>".into(),
                depth: 2,
                is_deleted_empty: false,
            },
            FlatComment {
                author: "c".into(),
                time: now,
                html: "<p>C</p>".into(),
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
        let (thread, _) =
            parse_captured_page(html, "https://www.reddit.com/r/test/comments/ext001/post/")
                .unwrap();
        assert_eq!(thread.story.id, "ext001");
        assert_eq!(thread.story.title, "External Link Post");
        assert_eq!(
            thread.story.url.as_deref(),
            Some("https://example.com/article")
        );
        assert!(thread.story.text_html.is_none());
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
        let (thread, _) = parse_captured_page(
            html,
            "https://old.reddit.com/r/test/comments/self01/selfpost/",
        )
        .unwrap();
        assert_eq!(thread.story.id, "self01");
        assert_eq!(thread.story.title, "Self Post Title");
        // Title link points to Reddit itself — no external URL
        assert!(thread.story.url.is_none());
        assert!(thread.story.text_html.unwrap().contains("Self-post text"));
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
        let (thread, _) =
            parse_captured_page(html, "https://www.reddit.com/r/test/comments/good01/post/")
                .unwrap();
        assert_eq!(thread.story.id, "good01");
        assert_eq!(thread.story.title, "Good Old Post");
    }

    #[test]
    fn current_desktop_nested_body_isolation() {
        let html = include_str!("fixtures/reddit_current_nested.html");
        let (thread, _) =
            parse_captured_page(html, "https://www.reddit.com/r/test/comments/nest001/post/")
                .unwrap();
        // parent comment should contain only its own body, not child's
        let parent = &thread.comments[0];
        assert_eq!(parent.author, "parent");
        assert_eq!(parent.html.trim(), "<p>Parent body</p>");
        assert!(!parent.html.contains("Child body"));
        // child should be nested under parent
        assert_eq!(parent.children.len(), 1);
        assert_eq!(parent.children[0].author, "child");
        assert_eq!(parent.children[0].html.trim(), "<p>Child body</p>");
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
        let html = include_str!("fixtures/reddit_current_wrapper.html");
        let (thread, _) =
            parse_captured_page(html, "https://www.reddit.com/r/test/comments/wrap01/post/")
                .unwrap();
        // Parent should have empty body (no own slot), child promoted to top
        // Parent is treated as deleted-empty since it has no entry and has children
        let parent = &thread.comments[0];
        assert!(parent.html.trim().is_empty() || parent.author == "[deleted]");
        assert!(!parent.html.contains("Child body inside wrapper"));
        // Child should appear at depth 0 (promoted from parent)
        assert_eq!(parent.children.len(), 1);
        assert_eq!(parent.children[0].author, "child");
        assert!(
            parent.children[0]
                .html
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
        let (thread, _) =
            parse_captured_page(html, "https://old.reddit.com/r/test/comments/ne001/post/")
                .unwrap();
        // The wrapper (no .entry) is removed, child promoted to depth 0
        assert_eq!(thread.comment_count, 1);
        assert_eq!(thread.comments.len(), 1);
        assert_eq!(thread.comments[0].author, "childauthor");
        assert_eq!(thread.comments[0].depth, 0);
        assert!(thread.comments[0].html.contains("Child comment body"));
        // Child's body should not be duplicated into the removed wrapper
    }
}
