use crate::model::{Book, BookBody, Comment};
use crate::sites::domain_label;
use chrono::{DateTime, Utc};
use std::fmt::Write;

const SNIPPET_MAX_CHARS: usize = 48;
const SKIP_LINK_MIN_DESCENDANTS: usize = 5;

/// Comment tree annotated with descendant counts in one O(n) bottom-up pass.
/// Skip links and chapter sizing use these counts instead of re-walking each subtree.
struct AnnotatedComment<'a> {
    comment: &'a Comment,
    /// Number of comments in this subtree excluding self (children + deeper).
    descendants: usize,
    /// Replies the budget cut from anywhere in this subtree. Frontier counts
    /// partition omissions, so summing them cannot double-count.
    omitted: usize,
    children: Vec<AnnotatedComment<'a>>,
}

pub fn render_html(book: &Book, max_indent_depth: usize) -> String {
    let mut out = String::new();
    out.push_str("<!doctype html><html><head><meta charset=\"utf-8\"></head><body>\n");
    render_story(&mut out, book);
    if let BookBody::Discussion(discussion) = &book.body
        && !discussion.comments().is_empty()
    {
        render_comments(&mut out, discussion.comments(), max_indent_depth);
    }
    out.push_str("</body></html>\n");
    out
}

fn annotate_comments(comments: &[Comment]) -> Vec<AnnotatedComment<'_>> {
    comments
        .iter()
        .map(|comment| {
            let children = annotate_comments(&comment.children);
            let descendants = children.iter().map(|child| 1 + child.descendants).sum();
            let omitted =
                comment.omitted_replies + children.iter().map(|child| child.omitted).sum::<usize>();
            AnnotatedComment {
                comment,
                descendants,
                omitted,
                children,
            }
        })
        .collect()
}

fn render_story(out: &mut String, book: &Book) {
    writeln!(
        out,
        "<h1 class=\"story-title\">{}</h1>",
        escape_html(&book.story.title)
    )
    .unwrap();
    // Classed block lines must be <div>, not <p>: pandoc's HTML reader drops
    // attributes from <p>, so p-level classes never reach the EPUB.
    match &book.body {
        BookBody::Discussion(discussion) => {
            // When the budget cut the book short, the reader learns it from the
            // book itself rather than from the terminal that built it. Three
            // cases, because round-robin selection usually seats every thread
            // and cuts replies instead — reporting only dropped threads would
            // call such a book complete.
            let extent = match (discussion.is_truncated(), discussion.all_threads_included()) {
                (false, _) => format!("{} comments", discussion.comment_count()),
                (true, true) => format!(
                    "{} of {} comments &middot; all {} threads",
                    discussion.comment_count(),
                    discussion.total_comment_count(),
                    discussion.total_threads()
                ),
                (true, false) => format!(
                    "{} of {} comments &middot; {} of {} threads",
                    discussion.comment_count(),
                    discussion.total_comment_count(),
                    discussion.included_threads(),
                    discussion.total_threads()
                ),
            };
            writeln!(
                out,
                "<div class=\"story-meta\">{}by {} &middot; {} &middot; {}</div>",
                book.story
                    .points
                    .map(|points| format!("{points} points &middot; "))
                    .unwrap_or_default(),
                escape_html(&book.story.author),
                short_date(book.story.time),
                extent
            )
            .unwrap();
        }
        BookBody::Article => {
            writeln!(
                out,
                "<div class=\"story-meta\">by {} &middot; {} &middot; {}</div>",
                escape_html(&book.story.author),
                short_date(book.story.time),
                escape_html(&book.source)
            )
            .unwrap();
        }
    }
    write!(out, "<div class=\"story-link\">").unwrap();
    if let Some(url) = &book.story.url {
        write!(
            out,
            "<a href=\"{}\">{}</a>",
            escape_html(url),
            escape_html(&domain_label(url))
        )
        .unwrap();
    }
    if let Some(discussion_url) = &book.story.discussion_url {
        if book.story.url.is_some() {
            write!(out, " &middot; ").unwrap();
        }
        write!(
            out,
            "<a href=\"{}\">{} discussion</a>",
            escape_html(discussion_url),
            escape_html(&book.source)
        )
        .unwrap();
    }
    writeln!(out, "</div>").unwrap();
    // Heading policy was applied at extraction, where the region was known:
    // discussion selftext demoted `h1`, article bodies kept the author's
    // structure. `render` only places the result.
    if let Some(html) = &book.story.text_html
        && !html.is_empty()
    {
        writeln!(out, "<div class=\"story-text\">{}</div>", html.as_str()).unwrap();
    }
}

fn render_comments(out: &mut String, comments: &[Comment], max_indent_depth: usize) {
    let annotated = annotate_comments(comments);
    let mut next_comment_id = 1;
    let top_level_count = annotated.len();
    for (index, node) in annotated.iter().enumerate() {
        let thread_index = index + 1;
        writeln!(
            out,
            "<h1 class=\"t-head\" id=\"t{thread_index}\">{}</h1>",
            thread_heading(node.comment)
        )
        .unwrap();
        let subtree_size = 1 + node.descendants;
        // Frontier markers sit where each cut happened, which on a deep thread
        // is at the bottom of a long chain. Without this line a chapter that
        // kept 18 of 436 comments would read as a small thread until it
        // abruptly ends.
        if node.omitted > 0 {
            writeln!(
                out,
                "<div class=\"t-info\">showing {} of {} comments</div>",
                subtree_size,
                subtree_size + node.omitted
            )
            .unwrap();
        }
        let thread_end_comment_id = next_comment_id + subtree_size - 1;
        let next_thread_id = (thread_index < top_level_count).then_some(thread_index + 1);
        render_comment(
            out,
            node,
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
    node: &AnnotatedComment<'_>,
    max_indent_depth: usize,
    next_comment_id: &mut usize,
    thread_end_comment_id: usize,
    next_thread_id: Option<usize>,
    is_top_level: bool,
) {
    let comment = node.comment;
    let comment_id = *next_comment_id;
    *next_comment_id += 1;
    let display_depth = comment.depth.min(max_indent_depth);
    let capped_marker = if comment.depth > max_indent_depth {
        format!(" <span class=\"c-info\">&#8627; {}</span>", comment.depth)
    } else {
        String::new()
    };
    let descendants = node.descendants;
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
    writeln!(out, "<div class=\"c-body\">{}</div>", comment.html.as_str()).unwrap();
    out.push_str("</div>\n");
    for child in &node.children {
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
    // After the included replies, where the cut ones would have been. Not an
    // `a.c-skip`: this is disclosure, not navigation, and keeping it out of that
    // class keeps `epub::verify_structure` checking only real link targets.
    if comment.omitted_replies > 0 {
        let reply_depth = (comment.depth + 1).min(max_indent_depth);
        writeln!(
            out,
            "<div class=\"c-omitted d{reply_depth}\">{} {} omitted</div>",
            comment.omitted_replies,
            if comment.omitted_replies == 1 {
                "reply"
            } else {
                "replies"
            }
        )
        .unwrap();
    }
}

fn thread_heading(comment: &Comment) -> String {
    let author = non_empty(&comment.author).unwrap_or("unknown");
    let snippet = snippet(comment.html.as_str(), SNIPPET_MAX_CHARS);
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
    let stripped = crate::util::strip_tags(html);
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
    use crate::model::Story;
    use crate::sanitize::{self, Region};
    use chrono::TimeZone;

    fn time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap()
    }

    fn comment(author: &str, html: &str, depth: usize, children: Vec<Comment>) -> Comment {
        Comment {
            author: author.to_string(),
            time: time(),
            html: sanitize::fragment(html, Region::CommentBody),
            depth,
            children,
            omitted_replies: 0,
        }
    }

    fn discussion(comments: Vec<Comment>) -> Book {
        Book {
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
            body: BookBody::discussion(comments),
            source: "hn".to_string(),
            source_slug: "hn".to_string(),
        }
    }

    #[test]
    fn untruncated_meta_reports_a_plain_comment_count() {
        let book = discussion(vec![
            comment("a", "<p>one</p>", 0, vec![]),
            comment("b", "<p>two</p>", 0, vec![]),
        ]);

        let html = render_html(&book, 5);
        let meta = html
            .lines()
            .find(|line| line.contains("story-meta"))
            .unwrap();

        assert!(meta.contains("2 comments"));
        assert!(!meta.contains("threads"));
    }

    fn budgeted(threads: Vec<Comment>, budget: usize) -> Book {
        let mut book = discussion(vec![]);
        book.body = BookBody::Discussion(crate::model::Discussion::with_budget_for_test(
            threads, budget,
        ));
        book
    }

    fn meta_line(html: &str) -> &str {
        html.lines()
            .find(|line| line.contains("story-meta"))
            .unwrap()
    }

    #[test]
    fn truncated_meta_reports_partial_comments_when_every_thread_survives() {
        // The shape round-robin actually produces on a mega-thread, and the one
        // a thread-based truncation check would have reported as complete.
        let threads = (0..4)
            .map(|index| {
                comment(
                    &format!("t{index}"),
                    "<p>top</p>",
                    0,
                    vec![comment("child", "<p>child</p>", 1, vec![])],
                )
            })
            .collect::<Vec<_>>();
        let book = budgeted(threads, 6);

        let html = render_html(&book, 5);

        assert!(
            meta_line(&html).contains("6 of 8 comments &middot; all 4 threads"),
            "{}",
            meta_line(&html)
        );
    }

    #[test]
    fn truncated_meta_reports_threads_when_the_budget_cannot_seat_them_all() {
        let threads = (0..4)
            .map(|index| comment(&format!("t{index}"), "<p>top</p>", 0, vec![]))
            .collect::<Vec<_>>();
        let book = budgeted(threads, 2);

        let html = render_html(&book, 5);

        assert!(
            meta_line(&html).contains("2 of 4 comments &middot; 2 of 4 threads"),
            "{}",
            meta_line(&html)
        );
    }

    #[test]
    fn cut_replies_are_disclosed_where_they_were_cut() {
        // Chain of 4, budget 2: the deepest kept comment lost 2 replies.
        let deep = comment(
            "a",
            "<p>a</p>",
            0,
            vec![comment(
                "b",
                "<p>b</p>",
                1,
                vec![comment(
                    "c",
                    "<p>c</p>",
                    2,
                    vec![comment("d", "<p>d</p>", 3, vec![])],
                )],
            )],
        );
        let book = budgeted(vec![deep], 2);

        let html = render_html(&book, 5);

        assert!(
            html.contains("<div class=\"c-omitted d2\">2 replies omitted</div>"),
            "{html}"
        );
        assert_eq!(
            html.matches("c-omitted").count(),
            1,
            "omission restated: {html}"
        );
        // Disclosure must not masquerade as navigation.
        assert!(!html.contains("c-skip"));
    }

    #[test]
    fn a_trimmed_thread_declares_its_full_size_at_the_chapter_head() {
        // Without this the chapter reads as a small thread until it stops.
        let wide = comment(
            "root",
            "<p>root</p>",
            0,
            (0..9)
                .map(|index| comment(&format!("r{index}"), "<p>reply</p>", 1, vec![]))
                .collect(),
        );
        let book = budgeted(vec![wide], 4);

        let html = render_html(&book, 5);

        assert!(
            html.contains("<div class=\"t-info\">showing 4 of 10 comments</div>"),
            "{html}"
        );
    }

    #[test]
    fn a_complete_thread_declares_nothing() {
        let book = budgeted(
            vec![comment(
                "a",
                "<p>a</p>",
                0,
                vec![comment("b", "<p>b</p>", 1, vec![])],
            )],
            crate::model::MAX_BOOK_COMMENTS,
        );

        let html = render_html(&book, 5);

        assert!(!html.contains("t-info"));
        assert!(!html.contains("c-omitted"));
    }

    #[test]
    fn a_single_cut_reply_is_singular() {
        let book = budgeted(
            vec![comment(
                "a",
                "<p>a</p>",
                0,
                vec![comment("b", "<p>b</p>", 1, vec![])],
            )],
            1,
        );

        let html = render_html(&book, 5);

        assert!(html.contains("1 reply omitted"), "{html}");
    }

    #[test]
    fn one_heading_per_top_level() {
        let book = discussion(vec![
            comment("a", "<p>one</p>", 0, vec![]),
            comment("b", "<p>two</p>", 0, vec![]),
            comment("c", "<p>three</p>", 0, vec![]),
        ]);

        let html = render_html(&book, 5);

        assert_eq!(html.matches("class=\"t-head\"").count(), 3);
        assert!(!html.contains("class=\"chunk\""));
        assert!(html.contains("id=\"t1\""));
        assert!(html.contains("id=\"t2\""));
        assert!(html.contains("id=\"t3\""));
    }

    #[test]
    fn heading_has_author_and_escaped_snippet() {
        let book = discussion(vec![comment(
            "a&b",
            "<p>Tom &amp; Jerry <b>quoted</b> < raw > text with enough words to truncate neatly at a boundary</p>",
            0,
            vec![],
        )]);

        let html = render_html(&book, 5);
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
        let mut book = discussion(vec![comment(
            "a",
            "<h1>Big</h1><h2 class=\"x\">Small</h2>",
            0,
            vec![],
        )]);
        book.story.text_html = Some(sanitize::fragment(
            "<h1>Selftext</h1><h2>Already smaller</h2>",
            Region::DiscussionText,
        ));

        let html = render_html(&book, 5);

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
        let book = discussion(vec![first, second]);

        let html = render_html(&book, 5);

        assert!(html.contains("id=\"c2\""));
        assert!(html.contains("href=\"#c8\">skip 5 replies"));
        assert!(html.contains("href=\"#t2\">skip 7 replies"));
    }

    #[test]
    fn small_subtrees_get_no_skip_link() {
        let children = (0..4)
            .map(|index| comment(&format!("child{index}"), "<p>child</p>", 1, vec![]))
            .collect::<Vec<_>>();
        let book = discussion(vec![comment("parent", "<p>parent</p>", 0, children)]);

        let html = render_html(&book, 5);

        assert!(!html.contains("c-skip"));
    }

    #[test]
    fn top_level_head_is_date_only() {
        let book = discussion(vec![comment(
            "top",
            "<p>top</p>",
            0,
            vec![comment("child", "<p>child</p>", 1, vec![])],
        )]);

        let html = render_html(&book, 5);
        let heads = html
            .lines()
            .filter(|line| line.contains("class=\"c-head\""))
            .collect::<Vec<_>>();

        assert!(!heads[0].contains("c-author"));
        assert!(heads[1].contains("c-author"));
    }

    #[test]
    fn annotate_counts_descendants_once_on_a_chain() {
        // Chain of 5: root has 4 descendants, leaf has 0.
        let mut leaf = comment("c4", "<p>4</p>", 4, vec![]);
        for depth in (0..4).rev() {
            leaf = comment(
                &format!("c{depth}"),
                &format!("<p>{depth}</p>"),
                depth,
                vec![leaf],
            );
        }
        let annotated = annotate_comments(std::slice::from_ref(&leaf));
        assert_eq!(annotated.len(), 1);
        assert_eq!(annotated[0].descendants, 4);
        assert_eq!(annotated[0].children[0].descendants, 3);
        let mut node = &annotated[0];
        for expected in [4, 3, 2, 1, 0] {
            assert_eq!(node.descendants, expected);
            if expected > 0 {
                node = &node.children[0];
            }
        }
    }
}
