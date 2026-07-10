use crate::model::{Comment, Story, Thread, ThreadKind};
use crate::sites::{Site, USER_AGENT, fetch_json};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

const ORDER_FETCH_WORKERS: usize = 16;

pub struct HackerNews;

#[derive(Debug, Deserialize)]
struct AlgoliaItem {
    id: u64,
    author: Option<String>,
    children: Option<Vec<AlgoliaItem>>,
    created_at: Option<DateTime<Utc>>,
    parent_id: Option<u64>,
    points: Option<i64>,
    story_id: Option<u64>,
    text: Option<String>,
    title: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    url: Option<String>,
}

impl Site for HackerNews {
    fn name(&self) -> &'static str {
        "hackernews"
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
        let id = parse_id(url).context("expected an HN item URL or bare item ID")?;
        progress(&format!("fetching item {id}"));
        let item = fetch_item(id)?;
        let mut story = if is_thread_root(&item) {
            item
        } else if let Some(story_id) = item.story_id.or(item.parent_id) {
            progress(&format!(
                "item {id} is a comment; fetching story {story_id}"
            ));
            fetch_item(story_id)?
        } else {
            bail!("HN item {id} is not a story and has no story_id");
        };
        apply_display_order(&mut story, progress);
        build_thread(story)
    }
}

// Algolia returns children chronologically; HN's ranked display order only
// exists in the official Firebase API's `kids` arrays, so fetch those for
// every branch where order matters (2+ children) and reorder to match.
fn apply_display_order(story: &mut AlgoliaItem, progress: &dyn Fn(&str)) {
    let mut branch_ids = Vec::new();
    collect_branch_ids(story, &mut branch_ids);
    if branch_ids.is_empty() {
        return;
    }
    progress(&format!("ordering {} branches", branch_ids.len()));
    let orders = fetch_display_orders(&branch_ids);
    let failed = branch_ids.len() - orders.len();
    if failed > 0 {
        progress(&format!(
            "{failed} branches kept chronological order (lookup failed)"
        ));
    }
    reorder_children(story, &orders);
}

fn collect_branch_ids(item: &AlgoliaItem, out: &mut Vec<u64>) {
    if let Some(children) = &item.children {
        if children.len() > 1 {
            out.push(item.id);
        }
        for child in children {
            collect_branch_ids(child, out);
        }
    }
}

fn fetch_display_orders(ids: &[u64]) -> HashMap<u64, Vec<u64>> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(HashMap::new());
    let workers = ids.len().min(ORDER_FETCH_WORKERS);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let agent = ureq::Agent::new_with_defaults();
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&id) = ids.get(index) else { break };
                    if let Ok(kids) = fetch_firebase_kids(&agent, id) {
                        results.lock().unwrap().insert(id, kids);
                    }
                }
            });
        }
    });
    results.into_inner().unwrap()
}

fn fetch_firebase_kids(agent: &ureq::Agent, id: u64) -> Result<Vec<u64>> {
    #[derive(Deserialize)]
    struct FirebaseItem {
        kids: Option<Vec<u64>>,
    }
    let url = format!("https://hacker-news.firebaseio.com/v0/item/{id}.json");
    let item = agent
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("failed to fetch {url}"))?
        .body_mut()
        .read_json::<FirebaseItem>()
        .with_context(|| format!("failed to decode Firebase response for item {id}"))?;
    Ok(item.kids.unwrap_or_default())
}

fn reorder_children(item: &mut AlgoliaItem, orders: &HashMap<u64, Vec<u64>>) {
    if let Some(children) = &mut item.children {
        if let Some(kids) = orders.get(&item.id) {
            let position: HashMap<u64, usize> = kids
                .iter()
                .enumerate()
                .map(|(index, id)| (*id, index))
                .collect();
            // Stable sort: ids missing from `kids` stay chronological, after ranked ones.
            children.sort_by_key(|child| position.get(&child.id).copied().unwrap_or(usize::MAX));
        }
        for child in children.iter_mut() {
            reorder_children(child, orders);
        }
    }
}

fn parse_id(url: &str) -> Option<u64> {
    static ITEM_RE: OnceLock<Regex> = OnceLock::new();
    if url.chars().all(|ch| ch.is_ascii_digit()) {
        return url.parse().ok();
    }
    let re = ITEM_RE.get_or_init(|| {
        Regex::new(r"^(?:https?://)?news\.ycombinator\.com/item\?id=(\d+)(?:[&#].*)?$").unwrap()
    });
    re.captures(url)
        .and_then(|captures| captures.get(1))
        .and_then(|m| m.as_str().parse().ok())
}

fn fetch_item(id: u64) -> Result<AlgoliaItem> {
    let url = format!("https://hn.algolia.com/api/v1/items/{id}");
    fetch_json::<AlgoliaItem>(&url)
        .with_context(|| format!("failed to decode Algolia response for item {id}"))
}

fn build_thread(item: AlgoliaItem) -> Result<Thread> {
    if !is_thread_root(&item) {
        bail!("Algolia item {} is not a story or poll", item.id);
    }
    let comments_raw = item.children.unwrap_or_default();
    let mut comment_count = 0;
    let mut max_depth = 0;
    let comments = comments_raw
        .into_iter()
        .filter_map(|child| build_comment(child, 0, &mut comment_count, &mut max_depth).transpose())
        .collect::<Result<Vec<_>>>()?;
    let story_id = item.id;
    Ok(Thread {
        kind: ThreadKind::Discussion,
        story: Story {
            id: story_id.to_string(),
            title: item.title.unwrap_or_else(|| format!("HN item {story_id}")),
            url: item.url,
            discussion_url: Some(format!("https://news.ycombinator.com/item?id={story_id}")),
            author: item.author.unwrap_or_else(|| "unknown".to_string()),
            points: item.points,
            time: item.created_at.unwrap_or_else(Utc::now),
            text_html: item.text,
        },
        comments,
        comment_count,
        max_depth,
        source: "Hacker News".to_string(),
        source_slug: "hn".to_string(),
    })
}

fn is_thread_root(item: &AlgoliaItem) -> bool {
    matches!(item.kind.as_deref(), Some("story" | "poll"))
}

fn build_comment(
    item: AlgoliaItem,
    depth: usize,
    comment_count: &mut usize,
    max_depth: &mut usize,
) -> Result<Option<Comment>> {
    let author = item.author.unwrap_or_default();
    let html = item.text.unwrap_or_default();
    let children_raw = item.children.unwrap_or_default();
    if author.is_empty() && html.is_empty() {
        return Ok(None);
    }

    *comment_count += 1;
    *max_depth = (*max_depth).max(depth);
    let children = children_raw
        .into_iter()
        .filter_map(|child| build_comment(child, depth + 1, comment_count, max_depth).transpose())
        .collect::<Result<Vec<_>>>()?;

    Ok(Some(Comment {
        author,
        time: item.created_at.unwrap_or_else(Utc::now),
        html,
        depth,
        children,
    }))
}

#[cfg(test)]
mod tests {
    use super::parse_id;

    #[test]
    fn parses_hn_url_variants() {
        assert_eq!(parse_id("126809"), Some(126809));
        assert_eq!(
            parse_id("https://news.ycombinator.com/item?id=126809"),
            Some(126809)
        );
        assert_eq!(
            parse_id("https://news.ycombinator.com/item?id=126809&p=2"),
            Some(126809)
        );
        assert_eq!(
            parse_id("https://news.ycombinator.com/item?id=126809#comments"),
            Some(126809)
        );
        assert_eq!(parse_id("https://anyforum.com/item?id=126809"), None);
        assert_eq!(parse_id("https://example.com/"), None);
    }

    #[test]
    fn converts_algolia_tree_to_model() {
        let item: super::AlgoliaItem =
            serde_json::from_str(include_str!("fixtures/hn_item_small.json")).unwrap();
        let thread = super::build_thread(item).unwrap();

        assert_eq!(thread.story.id, "126809");
        assert_eq!(thread.comment_count, 4);
        assert_eq!(thread.max_depth, 2);
        assert_eq!(thread.comments.len(), 2);
        assert_eq!(thread.comments[0].children[0].children[0].author, "carol");
    }

    #[test]
    fn reorders_children_to_display_order() {
        let mut item: super::AlgoliaItem =
            serde_json::from_str(include_str!("fixtures/hn_item_small.json")).unwrap();

        let mut branch_ids = Vec::new();
        super::collect_branch_ids(&item, &mut branch_ids);
        assert_eq!(branch_ids, vec![126809]);

        // Ranked order puts 5 first; 4 is absent (e.g. dead) and stays last.
        let orders = std::collections::HashMap::from([(126809, vec![5, 1])]);
        super::reorder_children(&mut item, &orders);

        let ids: Vec<u64> = item
            .children
            .as_ref()
            .unwrap()
            .iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, vec![5, 1, 4]);
    }
}
