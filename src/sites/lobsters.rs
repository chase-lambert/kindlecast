use crate::model::{Comment, Story, Thread, ThreadKind, comment_stats, rebase_comments};
use crate::sites::{Site, fetch_json};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::Deserialize;
use std::sync::OnceLock;

pub struct Lobsters;

#[derive(Debug, Deserialize)]
struct LobstersStory {
    short_id: String,
    title: String,
    url: Option<String>,
    created_at: DateTime<Utc>,
    score: i64,
    description: Option<String>,
    submitter_user: String,
    comments: Vec<LobstersComment>,
}

#[derive(Debug, Deserialize)]
struct LobstersComment {
    comment: Option<String>,
    commenting_user: Option<String>,
    created_at: DateTime<Utc>,
    depth: usize,
    is_deleted: Option<bool>,
}

impl Site for Lobsters {
    fn name(&self) -> &'static str {
        "lobsters"
    }

    fn matches(&self, url: &str) -> bool {
        parse_id(url).is_some()
    }

    fn fetch(
        &self,
        url: &str,
        _page_html: Option<String>,
        progress: &dyn Fn(&str),
    ) -> Result<Thread> {
        let id = parse_id(url).context("expected a lobste.rs story URL")?;
        let api_url = format!("https://lobste.rs/s/{id}.json");
        progress(&format!("fetching story {id}"));
        let story = fetch_json::<LobstersStory>(&api_url)
            .with_context(|| format!("failed to decode Lobsters story {id}"))?;
        build_thread(story)
    }
}

fn parse_id(url: &str) -> Option<String> {
    static STORY_RE: OnceLock<Regex> = OnceLock::new();
    STORY_RE
        .get_or_init(|| Regex::new(r"^https?://lobste\.rs/s/([a-z0-9]+)(?:[/?#].*)?$").unwrap())
        .captures(url)
        .and_then(|captures| captures.get(1))
        .map(|m| m.as_str().to_string())
}

fn build_thread(story: LobstersStory) -> Result<Thread> {
    let mut index = 0;
    let comments = build_comments(&story.comments, &mut index, 0);
    let stats = comment_stats(&comments);
    let story_url = format!("https://lobste.rs/s/{}", story.short_id);
    Ok(Thread {
        kind: ThreadKind::Discussion,
        story: Story {
            id: story.short_id,
            title: story.title,
            url: story.url.filter(|url| !url.trim().is_empty()),
            discussion_url: Some(story_url),
            author: story.submitter_user,
            points: Some(story.score),
            time: story.created_at,
            text_html: story.description.filter(|html| !html.trim().is_empty()),
        },
        comments,
        comment_count: stats.count,
        max_depth: stats.max_depth,
        source: "Lobsters".to_string(),
        source_slug: "lobsters".to_string(),
    })
}

fn build_comments(raw: &[LobstersComment], index: &mut usize, depth: usize) -> Vec<Comment> {
    let mut out = Vec::new();
    while let Some(item) = raw.get(*index) {
        if item.depth < depth {
            break;
        }
        if item.depth > depth {
            let promoted = build_comments(raw, index, item.depth);
            out.extend(promoted);
            continue;
        }
        *index += 1;
        let children = build_comments(raw, index, depth + 1);
        if !item.is_deleted.unwrap_or(false) {
            out.push(Comment {
                author: item
                    .commenting_user
                    .as_deref()
                    .filter(|author| !author.trim().is_empty())
                    .unwrap_or("unknown")
                    .to_string(),
                time: item.created_at,
                html: item.comment.clone().unwrap_or_default(),
                depth,
                children,
            });
        } else {
            out.extend(rebase_comments(children, depth));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{LobstersComment, LobstersStory};
    use chrono::Utc;

    #[test]
    fn parses_lobsters_url() {
        assert_eq!(
            super::parse_id("https://lobste.rs/s/abc123/title").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            super::parse_id("https://lobste.rs/s/abc123?foo=bar").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            super::parse_id("https://lobste.rs/s/abc123#comments").as_deref(),
            Some("abc123")
        );
        assert_eq!(super::parse_id("https://example.com/s/abc123"), None);
    }

    #[test]
    fn rebuilds_flat_comment_tree() {
        let now = Utc::now();
        let thread = super::build_thread(LobstersStory {
            short_id: "abc123".to_string(),
            title: "story".to_string(),
            url: Some("https://example.com".to_string()),
            created_at: now,
            score: 10,
            description: None,
            submitter_user: "alice".to_string(),
            comments: vec![
                LobstersComment {
                    comment: Some("one".to_string()),
                    commenting_user: Some("bob".to_string()),
                    created_at: now,
                    depth: 0,
                    is_deleted: None,
                },
                LobstersComment {
                    comment: Some("two".to_string()),
                    commenting_user: Some("carol".to_string()),
                    created_at: now,
                    depth: 1,
                    is_deleted: None,
                },
                LobstersComment {
                    comment: Some("three".to_string()),
                    commenting_user: Some("dave".to_string()),
                    created_at: now,
                    depth: 0,
                    is_deleted: None,
                },
            ],
        })
        .unwrap();

        assert_eq!(thread.comment_count, 3);
        assert_eq!(thread.max_depth, 1);
        assert_eq!(thread.comments[0].children[0].author, "carol");
    }

    #[test]
    fn promotes_deleted_children_at_parent_depth() {
        let now = Utc::now();
        let thread = super::build_thread(LobstersStory {
            short_id: "abc123".to_string(),
            title: "story".to_string(),
            url: None,
            created_at: now,
            score: 0,
            description: None,
            submitter_user: "alice".to_string(),
            comments: vec![
                LobstersComment {
                    comment: None,
                    commenting_user: None,
                    created_at: now,
                    depth: 0,
                    is_deleted: Some(true),
                },
                LobstersComment {
                    comment: Some("promoted".to_string()),
                    commenting_user: Some("bob".to_string()),
                    created_at: now,
                    depth: 1,
                    is_deleted: None,
                },
            ],
        })
        .unwrap();

        assert_eq!(thread.comment_count, 1);
        assert_eq!(thread.max_depth, 0);
        assert_eq!(thread.comments[0].author, "bob");
        assert_eq!(thread.comments[0].depth, 0);
    }
}
