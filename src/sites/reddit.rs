use crate::model::{Comment, Story, Thread, ThreadKind, comment_stats, rebase_comments};
use crate::sites::{Site, USER_AGENT, fetch_json};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
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
        _page_html: Option<String>,
        progress: &dyn Fn(&str),
    ) -> Result<Thread> {
        let id = parse_post_id(url)
            .map(Ok)
            .unwrap_or_else(|| resolve_share_url(url))?;
        let api_url = format!("https://www.reddit.com/comments/{id}.json?raw_json=1&limit=500");
        progress(&format!("fetching post {id}"));
        let listings = fetch_json::<Vec<Listing>>(&api_url)
            .with_context(|| format!("failed to decode Reddit thread {id}"))?;
        let (thread, omitted) = build_thread(listings)?;
        if omitted > 0 {
            progress(&format!("{omitted} more comments omitted"));
        }
        Ok(thread)
    }
}

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

#[cfg(test)]
mod tests {
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
}
