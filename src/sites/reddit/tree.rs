use crate::model::{Comment, rebase_comments};
use crate::sanitize::SanitizedHtml;
use chrono::{DateTime, Utc};

#[derive(Clone)]
pub(super) struct FlatComment {
    pub(super) author: String,
    pub(super) time: DateTime<Utc>,
    pub(super) html: SanitizedHtml,
    pub(super) depth: usize,
    pub(super) is_deleted_empty: bool,
}

pub(super) fn build_comment_tree(flat: &[FlatComment]) -> Vec<Comment> {
    let mut normalized = flat.to_vec();
    let mut prev_norm: usize = 0;
    for (i, item) in normalized.iter_mut().enumerate() {
        if i == 0 {
            item.depth = 0;
        } else if item.depth > prev_norm + 1 {
            item.depth = prev_norm + 1;
        }
        prev_norm = item.depth;
    }
    let mut index = 0;
    build_comments(&normalized, &mut index, 0)
}

fn build_comments(flat: &[FlatComment], index: &mut usize, depth: usize) -> Vec<Comment> {
    let mut out = Vec::new();
    while let Some(item) = flat.get(*index) {
        if item.depth < depth {
            break;
        }
        if item.depth > depth {
            let promoted = build_comments(flat, index, item.depth);
            out.extend(promoted);
            continue;
        }
        *index += 1;
        let children = build_comments(flat, index, depth + 1);
        if item.is_deleted_empty {
            out.extend(rebase_comments(children, depth));
        } else {
            out.push(Comment {
                author: item.author.clone(),
                time: item.time,
                html: item.html.clone(),
                depth,
                children,
            });
        }
    }
    out
}
