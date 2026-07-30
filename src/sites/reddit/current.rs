use super::tree::{FlatComment, build_comment_tree};
use super::{desc_by_name, find_own_comment_slot, first_node, parse_more_count, parse_timestamp};
use crate::model::{Book, BookBody, Story};
use crate::sanitize::{self, Region};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use dom_query::Document;

pub(super) fn extract_current_desktop(doc: &Document, input_url: &str) -> Result<(Book, usize)> {
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
        .map(|el| sanitize::fragment(el.inner_html().as_ref(), Region::DiscussionText))
        .filter(|html| !html.is_empty());

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
            .map(|el| sanitize::fragment(el.inner_html().as_ref(), Region::CommentBody))
            .unwrap_or_default();

        let time = comment_el
            .attr("created-timestamp")
            .as_deref()
            .and_then(parse_timestamp)
            .unwrap_or_else(Utc::now);

        let is_deleted_empty = author == "[deleted]" && body_html.is_empty();

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

    let book = Book {
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
        body: BookBody::discussion(comments),
        source: format!("r/{}", subreddit),
        source_slug: "reddit".to_string(),
    };

    Ok((book, omitted))
}
