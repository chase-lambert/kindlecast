use crate::sanitize::SanitizedHtml;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct Book {
    pub story: Story,
    pub body: BookBody,
    pub source: String,
    pub source_slug: String,
}

/// Discussion vs article content. Comments are exclusive to Discussion.
#[derive(Debug, Clone)]
pub enum BookBody {
    Discussion(Discussion),
    Article,
}

/// How many comments a book may contain. A hard ceiling: the selected forest
/// always holds `min(MAX_BOOK_COMMENTS, total)` comments.
///
/// Editorial, not defensive. Pandoc's cost is linear in comment count with no
/// cliff — measured 500 → 0.69s, 1,500 → 2.18s, 3,000 → 3.99s, 6,000 → 9.04s,
/// producing a 980 KiB EPUB at the top end. Nothing breaks at any realistic
/// thread size. This is the length at which a discussion stops being a book
/// anyone finishes, so do not "optimize" it as if it were a resource guard.
pub const MAX_BOOK_COMMENTS: usize = 1_500;

/// Comment forest with derived statistics, selected against
/// [`MAX_BOOK_COMMENTS`]. Fields are private so counts cannot drift from the
/// tree; construct only via [`Discussion::new`].
///
/// Truncation happens *here*, at the single boundary every adapter funnels
/// through, which means the truncated forest simply **is** the forest. `render`
/// and `epub::verify_structure` need no notion of a budget, and
/// [`Discussion::comment_count`] can never report comments the book does not
/// contain.
#[derive(Debug, Clone)]
pub struct Discussion {
    comments: Vec<Comment>,
    comment_count: usize,
    max_depth: usize,
    total_comment_count: usize,
    included_threads: usize,
    total_threads: usize,
}

impl Discussion {
    pub fn new(comments: Vec<Comment>) -> Self {
        Self::with_budget(comments, MAX_BOOK_COMMENTS)
    }

    /// Test-only so production cannot route around [`MAX_BOOK_COMMENTS`], and
    /// so `render` tests need not build a 1,500-comment fixture to see a
    /// truncated meta line.
    #[cfg(test)]
    pub(crate) fn with_budget_for_test(comments: Vec<Comment>, budget: usize) -> Self {
        Self::with_budget(comments, budget)
    }

    fn with_budget(comments: Vec<Comment>, budget: usize) -> Self {
        let total_threads = comments.len();
        let sizes = comments
            .iter()
            .map(|thread| comment_stats_one(thread).count)
            .collect::<Vec<_>>();
        let total_comment_count = sizes.iter().sum();

        let allowances = round_robin_allowances(&sizes, budget);
        let comments = comments
            .into_iter()
            .zip(&allowances)
            .filter_map(|(thread, &allowance)| prune_to_breadth_first_prefix(thread, allowance))
            .collect::<Vec<_>>();

        let included_threads = comments.len();
        let stats = comment_stats(&comments);

        Self {
            comments,
            comment_count: stats.count,
            max_depth: stats.max_depth,
            total_comment_count,
            included_threads,
            total_threads,
        }
    }

    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    /// Comments actually in the book.
    pub fn comment_count(&self) -> usize {
        self.comment_count
    }

    pub fn max_depth(&self) -> usize {
        self.max_depth
    }

    /// Comments the discussion had before the budget was applied.
    pub fn total_comment_count(&self) -> usize {
        self.total_comment_count
    }

    pub fn included_threads(&self) -> usize {
        self.included_threads
    }

    pub fn total_threads(&self) -> usize {
        self.total_threads
    }

    /// Whether the budget cut anything at all.
    ///
    /// Deliberately *not* `included_threads < total_threads`. Round-robin
    /// selection seats every thread long before it runs out of budget, so on a
    /// real mega-thread all threads survive while thousands of replies are cut.
    /// Asking about threads would report such a book as complete.
    pub fn is_truncated(&self) -> bool {
        self.comment_count < self.total_comment_count
    }

    pub fn all_threads_included(&self) -> bool {
        self.included_threads == self.total_threads
    }
}

/// Spend `budget` across threads one comment at a time, in site order.
///
/// This single rule gives every property the reading design needs, none of them
/// special-cased: every thread receives its opening comment before any thread
/// receives a second, threads that run out stop consuming so their unspent
/// share flows to threads that remain, and a discussion that fits is returned
/// untouched.
fn round_robin_allowances(sizes: &[usize], budget: usize) -> Vec<usize> {
    let mut allowances = vec![0; sizes.len()];
    let mut used = 0;
    while used < budget {
        let mut progressed = false;
        for (allowance, &size) in allowances.iter_mut().zip(sizes) {
            if used >= budget {
                break;
            }
            if *allowance < size {
                *allowance += 1;
                used += 1;
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    allowances
}

/// Keep the first `allowance` comments of `thread` in breadth-first order,
/// recording what was cut.
///
/// A breadth-first prefix is every node above some depth plus a left-to-right
/// run at that depth, which is why this can be done with one depth-first walk:
/// within a single depth, depth-first and breadth-first visit nodes in the same
/// order.
///
/// The retained set is parent-closed and preserves child order. That is the
/// invariant `render` depends on — it is what keeps subtree ids contiguous
/// under sequential numbering, so every skip target still names a comment that
/// exists.
fn prune_to_breadth_first_prefix(thread: Comment, allowance: usize) -> Option<Comment> {
    if allowance == 0 {
        return None;
    }
    let mut level_sizes = Vec::new();
    measure_levels(&thread, 0, &mut level_sizes);

    // Deepest fully-retained level, and how many of the next level fit.
    let mut cut_depth = 0;
    let mut above = 0;
    for &level in &level_sizes {
        if above + level > allowance {
            break;
        }
        above += level;
        cut_depth += 1;
    }
    let mut remaining_at_cut = allowance - above;
    Some(prune_node(thread, 0, cut_depth, &mut remaining_at_cut))
}

fn measure_levels(comment: &Comment, depth: usize, levels: &mut Vec<usize>) {
    if depth == levels.len() {
        levels.push(0);
    }
    levels[depth] += 1;
    for child in &comment.children {
        measure_levels(child, depth + 1, levels);
    }
}

fn prune_node(
    comment: Comment,
    depth: usize,
    cut_depth: usize,
    remaining_at_cut: &mut usize,
) -> Comment {
    let Comment {
        author,
        time,
        html,
        depth: comment_depth,
        children,
        omitted_replies,
    } = comment;

    let mut kept = Vec::new();
    let mut omitted = omitted_replies;
    for child in children {
        let child_fits = if depth + 1 < cut_depth {
            true
        } else if depth + 1 == cut_depth && *remaining_at_cut > 0 {
            *remaining_at_cut -= 1;
            true
        } else {
            false
        };
        if child_fits {
            kept.push(prune_node(child, depth + 1, cut_depth, remaining_at_cut));
        } else {
            // Frontier accounting: the whole dropped subtree is attributed here,
            // to the comment it hung from, and nowhere else. Counting each kept
            // node's missing descendants instead would report the same omission
            // again at every ancestor.
            omitted += comment_stats_one(&child).count;
        }
    }

    Comment {
        author,
        time,
        html,
        depth: comment_depth,
        children: kept,
        omitted_replies: omitted,
    }
}

impl BookBody {
    /// Construction boundary: stats are derived from `comments`.
    pub fn discussion(comments: Vec<Comment>) -> Self {
        BookBody::Discussion(Discussion::new(comments))
    }
}

#[derive(Debug, Clone)]
pub struct Story {
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    pub discussion_url: Option<String>,
    pub author: String,
    pub points: Option<i64>,
    pub time: DateTime<Utc>,
    /// Selftext or extracted article body. Sanitized at extraction.
    pub text_html: Option<SanitizedHtml>,
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub author: String,
    pub time: DateTime<Utc>,
    /// Comment body. Sanitized at extraction, so `render` can assemble it
    /// without any risk of it restructuring the surrounding book.
    pub html: SanitizedHtml,
    pub depth: usize,
    pub children: Vec<Comment>,
    /// Replies cut from directly beneath this comment, counted as whole
    /// subtrees. Adapters set `0`; the budget fills it in.
    ///
    /// Frontier accounting, not a descendant deficit: each omitted subtree is
    /// attributed to the one comment it hung from. Counting every kept
    /// comment's missing descendants instead would disclose the same omission
    /// again at each ancestor, so the numbers in a book would sum to far more
    /// than was actually cut.
    pub omitted_replies: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommentStats {
    pub count: usize,
    pub max_depth: usize,
}

pub fn comment_stats(comments: &[Comment]) -> CommentStats {
    comments
        .iter()
        .map(comment_stats_one)
        .fold(CommentStats::default(), |acc, stats| CommentStats {
            count: acc.count + stats.count,
            max_depth: acc.max_depth.max(stats.max_depth),
        })
}

pub fn rebase_comments(comments: Vec<Comment>, root_depth: usize) -> Vec<Comment> {
    comments
        .into_iter()
        .map(|comment| rebase_comment(comment, root_depth))
        .collect()
}

fn comment_stats_one(comment: &Comment) -> CommentStats {
    let children = comment_stats(&comment.children);
    CommentStats {
        count: 1 + children.count,
        max_depth: comment.depth.max(children.max_depth),
    }
}

fn rebase_comment(comment: Comment, depth: usize) -> Comment {
    Comment {
        author: comment.author,
        time: comment.time,
        html: comment.html,
        depth,
        children: rebase_comments(comment.children, depth + 1),
        // Rebasing moves a comment, it does not restore anything that was
        // already missing beneath it. Resetting this here would silently
        // undisclose those replies.
        omitted_replies: comment.omitted_replies,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sanitize::{self, Region};
    use chrono::TimeZone;

    /// A top-level thread of `size` comments as a chain, so its depth is also
    /// its size minus one.
    fn thread(size: usize) -> Comment {
        let time = Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap();
        let mut node = Comment {
            author: "a".to_string(),
            time,
            html: sanitize::fragment("<p>x</p>", Region::CommentBody),
            depth: size - 1,
            children: vec![],
            omitted_replies: 0,
        };
        for depth in (0..size - 1).rev() {
            node = Comment {
                author: "a".to_string(),
                time,
                html: sanitize::fragment("<p>x</p>", Region::CommentBody),
                depth,
                children: vec![node],
                omitted_replies: 0,
            };
        }
        node
    }

    /// A thread whose root has `width` direct replies, each with `depth_extra`
    /// descendants in a chain. Lets a test distinguish breadth-first from
    /// depth-first selection.
    fn bushy(width: usize, depth_extra: usize) -> Comment {
        let mut root = leaf(0);
        root.children = (0..width)
            .map(|_| {
                let mut branch = leaf(1);
                let mut tail = &mut branch;
                for d in 0..depth_extra {
                    tail.children = vec![leaf(2 + d)];
                    tail = &mut tail.children[0];
                }
                branch
            })
            .collect();
        root
    }

    fn leaf(depth: usize) -> Comment {
        Comment {
            author: "a".to_string(),
            time: Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap(),
            html: sanitize::fragment("<p>x</p>", Region::CommentBody),
            depth,
            children: vec![],
            omitted_replies: 0,
        }
    }

    fn total_omitted(comments: &[Comment]) -> usize {
        comments
            .iter()
            .map(|c| c.omitted_replies + total_omitted(&c.children))
            .sum()
    }

    #[test]
    fn budget_is_a_hard_ceiling() {
        for (threads, budget) in [(vec![thread(50)], 10), (vec![thread(4), thread(9)], 7)] {
            let total: usize = threads.iter().map(|t| comment_stats_one(t).count).sum();
            let discussion = Discussion::with_budget(threads, budget);
            assert_eq!(discussion.comment_count(), budget.min(total));
        }
    }

    #[test]
    fn every_thread_is_seated_before_any_thread_gets_a_second_comment() {
        // Round 1 selected whole threads, so a giant first thread starved the
        // rest. Round-robin gives each thread its opening comment first.
        let discussion = Discussion::with_budget(vec![thread(50), thread(50), thread(50)], 6);

        assert_eq!(discussion.included_threads(), 3);
        for thread in discussion.comments() {
            assert_eq!(comment_stats_one(thread).count, 2);
        }
    }

    #[test]
    fn exhausted_threads_release_their_share_to_threads_that_remain() {
        // Chase's case: one giant beside many shallow threads. The shallow ones
        // finish and everything they do not need flows to the giant.
        let mut threads = vec![thread(800)];
        threads.extend((0..50).map(|_| thread(2)));
        let discussion = Discussion::with_budget(threads, 1_500);

        assert_eq!(discussion.comment_count(), 900);
        assert_eq!(comment_stats_one(&discussion.comments()[0]).count, 800);
        assert!(!discussion.is_truncated());
    }

    #[test]
    fn a_constrained_refill_splits_the_budget_exactly() {
        // Same shape but over budget, so the refill is actually rationed.
        let mut threads = vec![thread(1_600)];
        threads.extend((0..50).map(|_| thread(2)));
        let discussion = Discussion::with_budget(threads, 1_500);

        assert_eq!(comment_stats_one(&discussion.comments()[0]).count, 1_400);
        let smalls: usize = discussion.comments()[1..]
            .iter()
            .map(|t| comment_stats_one(t).count)
            .sum();
        assert_eq!(smalls, 100);
        assert_eq!(discussion.comment_count(), 1_500);
    }

    #[test]
    fn a_budget_smaller_than_the_thread_count_seats_roots_only() {
        let discussion = Discussion::with_budget((0..50).map(|_| thread(5)).collect(), 10);

        assert_eq!(discussion.included_threads(), 10);
        assert!(!discussion.all_threads_included());
        for thread in discussion.comments() {
            assert_eq!(comment_stats_one(thread).count, 1);
        }
    }

    #[test]
    fn selection_within_a_thread_is_breadth_first_not_depth_first() {
        // Root plus 3 branches of 3. Depth-first would spend the budget down one
        // branch; breadth-first takes the root and all three direct replies.
        let discussion = Discussion::with_budget(vec![bushy(3, 2)], 4);

        let root = &discussion.comments()[0];
        assert_eq!(root.children.len(), 3);
        assert!(root.children.iter().all(|child| child.children.is_empty()));
    }

    #[test]
    fn disclosure_uses_two_channels_and_neither_covers_the_other() {
        // Omission markers account for replies cut from *included* threads.
        // Threads that were never seated have no comment to hang a marker on,
        // and are disclosed by the meta line's `t of T threads` instead.
        //
        // So global conservation holds only when every thread is seated. Anyone
        // "fixing" the shortfall by stamping dropped-thread mass onto a phantom
        // root, or asserting one global equation, would be wrong.
        let seated = Discussion::with_budget((0..5).map(|_| thread(5)).collect(), 15);
        assert!(seated.all_threads_included());
        assert_eq!(
            seated.comment_count() + total_omitted(seated.comments()),
            seated.total_comment_count()
        );

        let unseated = Discussion::with_budget((0..50).map(|_| thread(5)).collect(), 10);
        assert!(!unseated.all_threads_included());
        let included_thread_mass = 10 * 5;
        assert_eq!(
            unseated.comment_count() + total_omitted(unseated.comments()),
            included_thread_mass,
            "markers must account for the included threads exactly"
        );
        assert!(
            unseated.comment_count() + total_omitted(unseated.comments())
                < unseated.total_comment_count(),
            "and must not pretend to cover threads that were never seated"
        );
    }

    #[test]
    fn a_zero_budget_yields_an_empty_book_rather_than_looping() {
        let discussion = Discussion::with_budget(vec![thread(3), thread(4)], 0);

        assert_eq!(discussion.comment_count(), 0);
        assert_eq!(discussion.included_threads(), 0);
        assert!(discussion.is_truncated());
        assert!(!discussion.all_threads_included());
    }

    #[test]
    fn omissions_are_partitioned_not_double_counted() {
        // Conservation: what the book holds plus what it discloses equals what
        // the discussion had. Counting each kept comment's missing descendants
        // would inflate the disclosed total at every ancestor.
        let discussion = Discussion::with_budget(vec![bushy(3, 3), thread(20)], 12);

        assert_eq!(
            discussion.comment_count() + total_omitted(discussion.comments()),
            discussion.total_comment_count()
        );
    }

    #[test]
    fn omission_is_recorded_on_the_comment_it_was_cut_from() {
        // Chain of 5, budget 3: the deepest kept comment owns the 2 it lost, and
        // no ancestor repeats them.
        let discussion = Discussion::with_budget(vec![thread(5)], 3);

        let mut node = &discussion.comments()[0];
        let mut depth = 0;
        while let Some(child) = node.children.first() {
            assert_eq!(
                node.omitted_replies, 0,
                "ancestor at depth {depth} restated an omission"
            );
            node = child;
            depth += 1;
        }
        assert_eq!(depth, 2);
        assert_eq!(node.omitted_replies, 2);
    }

    #[test]
    fn a_discussion_that_fits_is_untouched_and_undisclosed() {
        let discussion = Discussion::with_budget(vec![thread(2), thread(3)], MAX_BOOK_COMMENTS);

        assert!(!discussion.is_truncated());
        assert!(discussion.all_threads_included());
        assert_eq!(total_omitted(discussion.comments()), 0);
        assert_eq!(discussion.comment_count(), discussion.total_comment_count());
    }

    #[test]
    fn an_empty_discussion_is_not_truncated() {
        let discussion = Discussion::with_budget(Vec::new(), MAX_BOOK_COMMENTS);

        assert!(!discussion.is_truncated());
        assert!(discussion.all_threads_included());
        assert_eq!(discussion.comment_count(), 0);
    }

    #[test]
    fn rebasing_preserves_recorded_omissions() {
        let discussion = Discussion::with_budget(vec![thread(5)], 3);
        let before = total_omitted(discussion.comments());

        let rebased = rebase_comments(discussion.comments().to_vec(), 0);

        assert_eq!(total_omitted(&rebased), before);
        assert!(before > 0);
    }

    #[test]
    fn max_depth_reflects_included_comments_only() {
        let discussion = Discussion::with_budget(vec![thread(9)], 3);

        assert_eq!(discussion.max_depth(), 2);
    }
}
