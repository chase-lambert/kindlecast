use super::{parse_post_id, resolve_share_url};
use crate::model::{Book, BookBody, Comment, Story, comment_stats, rebase_comments};
use crate::sites::fetch_json;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub(super) struct Listing {
    pub(super) data: ListingData,
}

#[derive(Debug, Deserialize)]
pub(super) struct ListingData {
    pub(super) children: Vec<Thing>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Thing {
    pub(super) kind: String,
    pub(super) data: Value,
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
pub(super) struct RedditComment {
    pub(super) author: Option<String>,
    pub(super) body_html: Option<String>,
    pub(super) created_utc: Option<f64>,
    pub(super) replies: Option<Replies>,
}

#[derive(Debug, Deserialize)]
struct MoreComments {
    count: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum Replies {
    Listing(Listing),
    Empty(String),
}

pub(super) struct CommentForest {
    pub(super) comments: Vec<Comment>,
    pub(super) count: usize,
    pub(super) max_depth: usize,
    pub(super) omitted: usize,
}

pub(super) fn fetch_json_api(url: &str, progress: &dyn Fn(&str)) -> Result<Book> {
    let id = parse_post_id(url)
        .map(Ok)
        .unwrap_or_else(|| resolve_share_url(url))?;
    let api_url = format!("https://www.reddit.com/comments/{id}.json?raw_json=1&limit=500");
    progress(&format!("fetching post {id}"));
    let listings = fetch_json::<Vec<Listing>>(&api_url)
        .with_context(|| format!("failed to decode Reddit thread {id} (Reddit may be blocking unauthenticated API access; try using the browser extension instead)"))?;
    let (book, omitted) = build_thread(listings)?;
    if omitted > 0 {
        progress(&format!("{omitted} more comments omitted"));
    }
    Ok(book)
}

pub(super) fn build_thread(listings: Vec<Listing>) -> Result<(Book, usize)> {
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
    let omitted = forest.omitted;
    let discussion_url = format!("https://www.reddit.com{}", post.permalink);
    Ok((
        Book {
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
            body: BookBody::discussion(forest.comments),
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

pub(super) fn build_comment(raw: RedditComment, depth: usize) -> CommentForest {
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
