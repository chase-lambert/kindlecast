use crate::model::{Story, Thread, ThreadKind};
use crate::sites::{Site, agent_with_timeout, domain_label, fetch_html, is_http_url};
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use dom_smoothie::{Config, Readability};
use regex::Regex;
use std::sync::OnceLock;

const MIN_EXTRACTED_TEXT_CHARS: usize = 200;

pub struct Article;

impl Site for Article {
    fn name(&self) -> &'static str {
        "article"
    }

    fn matches(&self, url: &str) -> bool {
        is_http_url(url)
    }

    fn fetch(
        &self,
        url: &str,
        page_html: Option<String>,
        progress: &dyn Fn(&str),
    ) -> Result<Thread> {
        fetch(url, page_html, progress)
    }
}

pub fn fetch(url: &str, page_html: Option<String>, progress: &dyn Fn(&str)) -> Result<Thread> {
    let document = match page_html {
        Some(html) if !html.trim().is_empty() => {
            progress("using captured page DOM");
            HtmlDocument {
                html,
                url: url.to_string(),
            }
        }
        _ => {
            progress("fetching article HTML");
            fetch_document(url)?
        }
    };
    extract_article(document)
}

struct HtmlDocument {
    html: String,
    url: String,
}

fn fetch_document(url: &str) -> Result<HtmlDocument> {
    let (final_url, html) = fetch_html(&agent_with_timeout(), url)?;
    Ok(HtmlDocument {
        html,
        url: final_url,
    })
}

fn extract_article(document: HtmlDocument) -> Result<Thread> {
    let title_fallback = title_tag(&document.html);
    let domain = domain_label(&document.url);
    let url = document.url;
    let article =
        Readability::new(document.html, Some(url.as_str()), Some(Config::default()))?.parse()?;
    let content = article.content.trim().to_string();
    let text_len = crate::util::strip_tags(&content).trim().chars().count();
    if text_len < MIN_EXTRACTED_TEXT_CHARS {
        bail!("article extraction found only {text_len} text chars; refusing to send a husk");
    }

    let title = non_empty(article.title)
        .or(title_fallback)
        .unwrap_or_else(|| domain.clone());
    let author = article
        .byline
        .and_then(non_empty)
        .unwrap_or_else(|| domain.clone());
    let time = article
        .published_time
        .and_then(|time| DateTime::parse_from_rfc3339(&time).ok())
        .map(|time| time.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    Ok(Thread {
        kind: ThreadKind::Article,
        story: Story {
            id: String::new(),
            title,
            url: Some(url),
            discussion_url: None,
            author,
            points: None,
            time,
            text_html: Some(content),
        },
        comments: Vec::new(),
        comment_count: 0,
        max_depth: 0,
        source: domain.clone(),
        source_slug: domain.replace('.', "-"),
    })
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn title_tag(html: &str) -> Option<String> {
    static TITLE_RE: OnceLock<Regex> = OnceLock::new();
    TITLE_RE
        .get_or_init(|| Regex::new("(?is)<title[^>]*>(.*?)</title>").unwrap())
        .captures(html)
        .and_then(|captures| captures.get(1))
        .map(|title| {
            html_escape::decode_html_entities(title.as_str())
                .trim()
                .to_string()
        })
        .filter(|title| !title.is_empty())
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracts_basic_article() {
        let body = (0..40)
            .map(|_| "This is a readable paragraph with enough text to pass extraction.")
            .collect::<Vec<_>>()
            .join(" ");
        let html = format!(
            "<html><head><title>Readable</title></head><body><nav>Nav junk</nav><article><h1>Readable</h1><p>{body}</p></article></body></html>"
        );
        let thread = super::extract_article(super::HtmlDocument {
            html,
            url: "https://example.com/story".to_string(),
        })
        .unwrap();

        assert_eq!(thread.kind, crate::model::ThreadKind::Article);
        assert_eq!(thread.source_slug, "example-com");
        assert!(
            thread
                .story
                .text_html
                .unwrap()
                .contains("readable paragraph")
        );
    }
}
