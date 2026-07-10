use crate::model::Thread;
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;

pub mod article;
pub mod hackernews;
pub mod lobsters;
pub mod reddit;

pub trait Site: Sync {
    fn name(&self) -> &'static str;
    fn matches(&self, url: &str) -> bool;
    fn fetch(
        &self,
        url: &str,
        page_html: Option<String>,
        progress: &dyn Fn(&str),
    ) -> Result<Thread>;
}

static HACKER_NEWS: hackernews::HackerNews = hackernews::HackerNews;
static REDDIT: reddit::Reddit = reddit::Reddit;
static LOBSTERS: lobsters::Lobsters = lobsters::Lobsters;
static ARTICLE: article::Article = article::Article;
static SITES: [&dyn Site; 4] = [&HACKER_NEWS, &REDDIT, &LOBSTERS, &ARTICLE];

pub fn adapter_for(url: &str) -> Option<&'static dyn Site> {
    SITES.iter().copied().find(|site| site.matches(url))
}

pub fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

pub fn fetch_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .call()
        .with_context(|| format!("failed to fetch {url}"))?
        .body_mut()
        .read_json::<T>()
        .with_context(|| format!("failed to decode JSON from {url}"))
}

pub fn domain_label(url: &str) -> String {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .trim_start_matches("www.")
        .to_string()
}

pub const USER_AGENT: &str = "linux:kindlecast:0.2 (by /u/chaselambert)";
