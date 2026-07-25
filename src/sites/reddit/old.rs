use super::tree::{FlatComment, build_comment_tree};
use super::{
    clean_body_html, desc_by_class, desc_by_name, desc_by_name_and_class, direct_child_by_class,
    find_md_body, first_node, parse_more_count, parse_timestamp,
};
use crate::model::{Book, BookBody, Story};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use dom_query::Document;

pub(super) fn extract_old_reddit(doc: &Document, input_url: &str) -> Result<(Book, usize)> {
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
