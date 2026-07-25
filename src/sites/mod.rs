use crate::model::Thread;
use crate::util::human_bytes;
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use std::time::Duration;
use ureq::ResponseExt;

pub mod article;
pub mod hackernews;
pub mod lobsters;
pub mod reddit;

/// Match the image pipeline: hang forever is worse than a clear timeout.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Conscious raise above ureq's default 10 MiB so large Algolia/Reddit trees
/// fail with a clear budget message rather than a cryptic decode error.
pub const MAX_JSON_BODY_BYTES: u64 = 32 * 1024 * 1024;

/// Article HTML budget (also above ureq's 10 MiB default).
pub const MAX_HTML_BODY_BYTES: u64 = 16 * 1024 * 1024;

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

/// Agent with the shared timeout preset. Prefer one agent per worker when
/// reusing connections (HN Firebase ordering).
pub fn agent_with_timeout() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .into()
}

/// Shared timeout plus "do not follow redirects" (Reddit share Location capture).
pub fn agent_without_redirects() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .build()
        .into()
}

pub fn fetch_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    fetch_json_with(&agent_with_timeout(), url)
}

pub fn fetch_json_with<T: DeserializeOwned>(agent: &ureq::Agent, url: &str) -> Result<T> {
    let mut response = agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .call()
        .map_err(|err| map_ureq_error(err, url, None))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_JSON_BODY_BYTES)
        .read_to_vec()
        .map_err(|err| map_ureq_error(err, url, Some(MAX_JSON_BODY_BYTES)))?;
    serde_json::from_slice(&bytes).with_context(|| format!("failed to decode JSON from {url}"))
}

/// Fetch HTML, preserving the post-redirect final URL for link/image basing.
pub fn fetch_html(agent: &ureq::Agent, url: &str) -> Result<(String, String)> {
    let mut response = agent
        .get(url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "text/html,application/xhtml+xml")
        .call()
        .map_err(|err| map_ureq_error(err, url, None))?;
    let final_url = response.get_uri().to_string();
    let html = response
        .body_mut()
        .with_config()
        .limit(MAX_HTML_BODY_BYTES)
        .read_to_string()
        .map_err(|err| map_ureq_error(err, &final_url, Some(MAX_HTML_BODY_BYTES)))?;
    Ok((final_url, html))
}

/// Map ureq errors for both `.call()` and body-read phases.
/// `body_limit` is set only when a configured read limit may have been hit.
fn map_ureq_error(err: ureq::Error, url: &str, body_limit: Option<u64>) -> anyhow::Error {
    match err {
        ureq::Error::BodyExceedsLimit(_) => {
            let limit = body_limit.unwrap_or(0);
            anyhow::anyhow!(
                "response from {url} exceeded {} body budget",
                human_bytes(limit)
            )
        }
        ureq::Error::Timeout(_) => {
            anyhow::anyhow!(
                "request to {url} timed out after {}s",
                FETCH_TIMEOUT.as_secs()
            )
        }
        other => {
            let phase = if body_limit.is_some() {
                "failed to read body from"
            } else {
                "failed to fetch"
            };
            anyhow::Error::new(other).context(format!("{phase} {url}"))
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_exceeds_limit_message_names_budget() {
        let err = map_ureq_error(
            ureq::Error::BodyExceedsLimit(MAX_JSON_BODY_BYTES),
            "https://example.com/thread.json",
            Some(MAX_JSON_BODY_BYTES),
        );
        let message = format!("{err:#}");
        assert!(message.contains("exceeded"));
        assert!(message.contains("32.00 MiB"));
        assert!(message.contains("https://example.com/thread.json"));
        assert!(!message.contains("decode"));
    }

    #[test]
    fn timeout_message_names_duration_for_call_and_body() {
        for body_limit in [None, Some(MAX_HTML_BODY_BYTES)] {
            let err = map_ureq_error(
                ureq::Error::Timeout(ureq::Timeout::Global),
                "https://example.com/",
                body_limit,
            );
            let message = format!("{err:#}");
            assert!(
                message.contains("timed out after 30s"),
                "body_limit={body_limit:?}: {message}"
            );
        }
    }
}
