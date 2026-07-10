use crate::model::{Comment, Thread, ThreadKind, comment_stats};
use crate::sites::domain_label;
use chrono::{DateTime, Utc};
use regex::Regex;
use std::fmt::Write;
use std::sync::OnceLock;

const SNIPPET_MAX_CHARS: usize = 48;
const SKIP_LINK_MIN_DESCENDANTS: usize = 5;

pub fn render_html(thread: &Thread, max_indent_depth: usize) -> String {
    let mut out = String::new();
    out.push_str("<!doctype html><html><head><meta charset=\"utf-8\"></head><body>\n");
    render_story(&mut out, thread);
    if thread.kind == ThreadKind::Discussion && thread.comment_count > 0 {
        render_comments(&mut out, thread, max_indent_depth);
    }
    out.push_str("</body></html>\n");
    out
}

fn render_story(out: &mut String, thread: &Thread) {
    writeln!(
        out,
        "<h1 class=\"story-title\">{}</h1>",
        escape_html(&thread.story.title)
    )
    .unwrap();
    // Classed block lines must be <div>, not <p>: pandoc's HTML reader drops
    // attributes from <p>, so p-level classes never reach the EPUB.
    match thread.kind {
        ThreadKind::Discussion => {
            writeln!(
                out,
                "<div class=\"story-meta\">{}by {} &middot; {} &middot; {} comments</div>",
                thread
                    .story
                    .points
                    .map(|points| format!("{points} points &middot; "))
                    .unwrap_or_default(),
                escape_html(&thread.story.author),
                short_date(thread.story.time),
                thread.comment_count
            )
            .unwrap();
        }
        ThreadKind::Article => {
            writeln!(
                out,
                "<div class=\"story-meta\">by {} &middot; {} &middot; {}</div>",
                escape_html(&thread.story.author),
                short_date(thread.story.time),
                escape_html(&thread.source)
            )
            .unwrap();
        }
    }
    write!(out, "<div class=\"story-link\">").unwrap();
    if let Some(url) = &thread.story.url {
        write!(
            out,
            "<a href=\"{}\">{}</a>",
            escape_html(url),
            escape_html(&domain_label(url))
        )
        .unwrap();
    }
    if let Some(discussion_url) = &thread.story.discussion_url {
        if thread.story.url.is_some() {
            write!(out, " &middot; ").unwrap();
        }
        write!(
            out,
            "<a href=\"{}\">{} discussion</a>",
            escape_html(discussion_url),
            escape_html(&thread.source)
        )
        .unwrap();
    }
    writeln!(out, "</div>").unwrap();
    if let Some(html) = &thread.story.text_html
        && !html.trim().is_empty()
    {
        let html = if thread.kind == ThreadKind::Discussion {
            demote_h1(html)
        } else {
            html.to_string()
        };
        writeln!(out, "<div class=\"story-text\">{html}</div>").unwrap();
    }
}

fn render_comments(out: &mut String, thread: &Thread, max_indent_depth: usize) {
    let mut next_comment_id = 1;
    let top_level_count = thread.comments.len();
    for (index, comment) in thread.comments.iter().enumerate() {
        let thread_index = index + 1;
        writeln!(
            out,
            "<h1 class=\"t-head\" id=\"t{thread_index}\">{}</h1>",
            thread_heading(comment)
        )
        .unwrap();
        let subtree_size = 1 + comment_stats(&comment.children).count;
        let thread_end_comment_id = next_comment_id + subtree_size - 1;
        let next_thread_id = (thread_index < top_level_count).then_some(thread_index + 1);
        render_comment(
            out,
            comment,
            max_indent_depth,
            &mut next_comment_id,
            thread_end_comment_id,
            next_thread_id,
            true,
        );
    }
}

fn render_comment(
    out: &mut String,
    comment: &Comment,
    max_indent_depth: usize,
    next_comment_id: &mut usize,
    thread_end_comment_id: usize,
    next_thread_id: Option<usize>,
    is_top_level: bool,
) {
    let comment_id = *next_comment_id;
    *next_comment_id += 1;
    let display_depth = comment.depth.min(max_indent_depth);
    let capped_marker = if comment.depth > max_indent_depth {
        format!(" <span class=\"c-info\">&#8627; {}</span>", comment.depth)
    } else {
        String::new()
    };
    let descendants = comment_stats(&comment.children).count;
    let skip_target = skip_target(
        comment_id,
        descendants,
        thread_end_comment_id,
        next_thread_id,
    );
    writeln!(
        out,
        "<div class=\"c d{display_depth}\" id=\"c{comment_id}\">"
    )
    .unwrap();
    if is_top_level {
        writeln!(
            out,
            "<div class=\"c-head\"><span class=\"c-info\">{}</span>{}</div>",
            short_date(comment.time),
            skip_link(descendants, skip_target)
        )
        .unwrap();
    } else {
        writeln!(
            out,
            "<div class=\"c-head\"><b class=\"c-author\">{}</b> <span class=\"c-info\">&middot; {}</span>{}{}</div>",
            escape_html(&comment.author),
            short_date(comment.time),
            capped_marker,
            skip_link(descendants, skip_target)
        )
        .unwrap();
    }
    writeln!(
        out,
        "<div class=\"c-body\">{}</div>",
        neutralize_headings(&comment.html)
    )
    .unwrap();
    out.push_str("</div>\n");
    for child in &comment.children {
        render_comment(
            out,
            child,
            max_indent_depth,
            next_comment_id,
            thread_end_comment_id,
            next_thread_id,
            false,
        );
    }
}

fn thread_heading(comment: &Comment) -> String {
    let author = non_empty(&comment.author).unwrap_or("unknown");
    let snippet = snippet(&comment.html, SNIPPET_MAX_CHARS);
    if snippet.is_empty() {
        escape_html(author)
    } else {
        format!("{} &middot; {}", escape_html(author), escape_html(&snippet))
    }
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn skip_target(
    comment_id: usize,
    descendants: usize,
    thread_end_comment_id: usize,
    next_thread_id: Option<usize>,
) -> Option<String> {
    if descendants < SKIP_LINK_MIN_DESCENDANTS {
        return None;
    }
    let subtree_end_id = comment_id + descendants;
    if subtree_end_id < thread_end_comment_id {
        Some(format!("#c{}", subtree_end_id + 1))
    } else {
        next_thread_id.map(|thread_id| format!("#t{thread_id}"))
    }
}

fn skip_link(descendants: usize, target: Option<String>) -> String {
    let Some(target) = target else {
        return String::new();
    };
    let label = if descendants == 1 { "reply" } else { "replies" };
    format!(
        " <a class=\"c-skip\" href=\"{}\">skip {} {} &#8595;</a>",
        escape_html(&target),
        descendants,
        label
    )
}

fn snippet(html: &str, max_chars: usize) -> String {
    let stripped = strip_tags(html);
    let text = html_escape::decode_html_entities(&stripped);
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }

    let mut truncated = collapsed.chars().take(max_chars).collect::<String>();
    let min_word_boundary = max_chars * 3 / 5;
    if let Some((idx, _)) = truncated.char_indices().rfind(|(_, ch)| ch.is_whitespace())
        && truncated[..idx].chars().count() >= min_word_boundary
    {
        truncated.truncate(idx);
    }
    truncated = truncated.trim_end().to_string();
    truncated.push('…');
    truncated
}

fn strip_tags(html: &str) -> String {
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    TAG_RE
        .get_or_init(|| Regex::new("(?is)<[^>]+>").unwrap())
        .replace_all(html, " ")
        .to_string()
}

fn neutralize_headings(html: &str) -> String {
    static OPEN_RE: OnceLock<Regex> = OnceLock::new();
    static CLOSE_RE: OnceLock<Regex> = OnceLock::new();
    let html = OPEN_RE
        .get_or_init(|| Regex::new("(?is)<h[1-6]\\b[^>]*>").unwrap())
        .replace_all(html, "<div class=\"c-hd\">");
    CLOSE_RE
        .get_or_init(|| Regex::new("(?is)</h[1-6]\\s*>").unwrap())
        .replace_all(&html, "</div>")
        .to_string()
}

fn demote_h1(html: &str) -> String {
    static OPEN_RE: OnceLock<Regex> = OnceLock::new();
    static CLOSE_RE: OnceLock<Regex> = OnceLock::new();
    let html = OPEN_RE
        .get_or_init(|| Regex::new("(?is)<h1(\\s[^>]*)?>").unwrap())
        .replace_all(html, "<h2$1>");
    CLOSE_RE
        .get_or_init(|| Regex::new("(?is)</h1\\s*>").unwrap())
        .replace_all(&html, "</h2>")
        .to_string()
}

pub fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn short_date(time: DateTime<Utc>) -> String {
    time.format("%b %-d, %Y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Story, Thread};
    use chrono::TimeZone;

    fn time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap()
    }

    fn comment(author: &str, html: &str, depth: usize, children: Vec<Comment>) -> Comment {
        Comment {
            author: author.to_string(),
            time: time(),
            html: html.to_string(),
            depth,
            children,
        }
    }

    fn thread(kind: ThreadKind, comments: Vec<Comment>) -> Thread {
        let comment_count = comment_stats(&comments).count;
        Thread {
            kind,
            story: Story {
                id: "42".to_string(),
                title: "Story".to_string(),
                url: Some("https://example.com/story".to_string()),
                discussion_url: Some("https://example.com/discuss".to_string()),
                author: "submitter".to_string(),
                points: Some(10),
                time: time(),
                text_html: None,
            },
            comments,
            comment_count,
            max_depth: 0,
            source: "hn".to_string(),
            source_slug: "hn".to_string(),
        }
    }

    #[test]
    fn one_heading_per_top_level() {
        let thread = thread(
            ThreadKind::Discussion,
            vec![
                comment("a", "<p>one</p>", 0, vec![]),
                comment("b", "<p>two</p>", 0, vec![]),
                comment("c", "<p>three</p>", 0, vec![]),
            ],
        );

        let html = render_html(&thread, 5);

        assert_eq!(html.matches("class=\"t-head\"").count(), 3);
        assert!(!html.contains("class=\"chunk\""));
        assert!(html.contains("id=\"t1\""));
        assert!(html.contains("id=\"t2\""));
        assert!(html.contains("id=\"t3\""));
    }

    #[test]
    fn heading_has_author_and_escaped_snippet() {
        let thread = thread(
            ThreadKind::Discussion,
            vec![comment(
                "a&b",
                "<p>Tom &amp; Jerry <b>quoted</b> < raw > text with enough words to truncate neatly at a boundary</p>",
                0,
                vec![],
            )],
        );

        let html = render_html(&thread, 5);
        let heading = html.lines().find(|line| line.contains("t-head")).unwrap();

        assert!(heading.contains("a&amp;b &middot; Tom &amp; Jerry quoted"));
        assert!(heading.contains('…'));
        assert!(!heading.contains("< raw >"));
    }

    #[test]
    fn snippet_truncates_on_char_boundary() {
        let value = snippet("ééééé ééééé ééééé ééééé", 9);

        assert!(value.ends_with('…'));
        assert!(value.is_char_boundary(value.len()));
    }

    #[test]
    fn comment_body_headings_neutralized() {
        let mut thread = thread(
            ThreadKind::Discussion,
            vec![comment(
                "a",
                "<h1>Big</h1><h2 class=\"x\">Small</h2>",
                0,
                vec![],
            )],
        );
        thread.story.text_html = Some("<h1>Selftext</h1><h2>Already smaller</h2>".to_string());

        let html = render_html(&thread, 5);

        assert!(html.contains("<div class=\"c-hd\">Big</div><div class=\"c-hd\">Small</div>"));
        assert!(
            html.contains(
                "<div class=\"story-text\"><h2>Selftext</h2><h2>Already smaller</h2></div>"
            )
        );
    }

    #[test]
    fn skip_link_targets_next_sibling() {
        let children = (0..5)
            .map(|index| comment(&format!("child{index}"), "<p>child</p>", 1, vec![]))
            .collect::<Vec<_>>();
        let first = comment(
            "first",
            "<p>first</p>",
            0,
            vec![
                comment("large", "<p>large</p>", 1, children),
                comment("after", "<p>after</p>", 1, vec![]),
            ],
        );
        let second = comment("second", "<p>second</p>", 0, vec![]);
        let thread = thread(ThreadKind::Discussion, vec![first, second]);

        let html = render_html(&thread, 5);

        assert!(html.contains("id=\"c2\""));
        assert!(html.contains("href=\"#c8\">skip 5 replies"));
        assert!(html.contains("href=\"#t2\">skip 7 replies"));
    }

    #[test]
    fn small_subtrees_get_no_skip_link() {
        let children = (0..4)
            .map(|index| comment(&format!("child{index}"), "<p>child</p>", 1, vec![]))
            .collect::<Vec<_>>();
        let thread = thread(
            ThreadKind::Discussion,
            vec![comment("parent", "<p>parent</p>", 0, children)],
        );

        let html = render_html(&thread, 5);

        assert!(!html.contains("c-skip"));
    }

    #[test]
    fn top_level_head_is_date_only() {
        let thread = thread(
            ThreadKind::Discussion,
            vec![comment(
                "top",
                "<p>top</p>",
                0,
                vec![comment("child", "<p>child</p>", 1, vec![])],
            )],
        );

        let html = render_html(&thread, 5);
        let heads = html
            .lines()
            .filter(|line| line.contains("class=\"c-head\""))
            .collect::<Vec<_>>();

        assert!(!heads[0].contains("c-author"));
        assert!(heads[1].contains("c-author"));
    }
}
