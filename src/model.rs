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

/// Comment forest with derived statistics. Fields are private so counts cannot
/// drift from the tree; construct only via [`Discussion::new`].
#[derive(Debug, Clone)]
pub struct Discussion {
    comments: Vec<Comment>,
    comment_count: usize,
    max_depth: usize,
}

impl Discussion {
    pub fn new(comments: Vec<Comment>) -> Self {
        let stats = comment_stats(&comments);
        Self {
            comments,
            comment_count: stats.count,
            max_depth: stats.max_depth,
        }
    }

    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    pub fn comment_count(&self) -> usize {
        self.comment_count
    }

    pub fn max_depth(&self) -> usize {
        self.max_depth
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
