use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadKind {
    Discussion,
    Article,
}

#[derive(Debug, Clone)]
pub struct Thread {
    pub kind: ThreadKind,
    pub story: Story,
    pub comments: Vec<Comment>,
    pub comment_count: usize,
    pub max_depth: usize,
    pub source: String,
    pub source_slug: String,
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
    pub text_html: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub author: String,
    pub time: DateTime<Utc>,
    pub html: String,
    pub depth: usize,
    pub children: Vec<Comment>,
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
    }
}
