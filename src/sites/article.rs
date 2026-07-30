use crate::model::{Book, BookBody, Story};
use crate::sanitize::{self, Region};
use crate::sites::{Site, agent_with_timeout, domain_label, fetch_html, is_http_url};
use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use dom_query::Document;
use dom_smoothie::{Config, Readability};

const MIN_EXTRACTED_TEXT_CHARS: usize = 200;

pub struct Article;

impl Site for Article {
    fn name(&self) -> &'static str {
        "article"
    }

    fn matches(&self, url: &str) -> bool {
        is_http_url(url)
    }

    fn fetch(&self, url: &str, page_html: Option<String>, progress: &dyn Fn(&str)) -> Result<Book> {
        fetch(url, page_html, progress)
    }
}

pub fn fetch(url: &str, page_html: Option<String>, progress: &dyn Fn(&str)) -> Result<Book> {
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

fn extract_article(document: HtmlDocument) -> Result<Book> {
    let title_fallback = title_tag(&document.html);
    let domain = domain_label(&document.url);
    let url = document.url;
    let article =
        Readability::new(document.html, Some(url.as_str()), Some(Config::default()))?.parse()?;
    // Sanitize before measuring. Readability output can carry 200+ characters
    // inside elements the policy removes (`<select>`, `<script>`), which would
    // otherwise pass this threshold and then vanish from the finished book.
    let content = sanitize::fragment(article.content.trim(), Region::ArticleBody);
    let text_len = crate::util::strip_tags(content.as_str())
        .trim()
        .chars()
        .count();
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

    Ok(Book {
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
        body: BookBody::Article,
        source: domain.clone(),
        source_slug: domain.replace('.', "-"),
    })
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// `<title>` fallback when Readability finds no title.
///
/// Tree-based rather than pattern-based, so `<title>` attributes and stray `>`
/// cannot confuse it. The parser also resolves entities, which the previous
/// regex had to undo by hand.
fn title_tag(html: &str) -> Option<String> {
    let document = Document::from(html);
    let title = document.select("title").text().trim().to_string();
    (!title.is_empty()).then_some(title)
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
        let book = super::extract_article(super::HtmlDocument {
            html,
            url: "https://example.com/story".to_string(),
        })
        .unwrap();

        assert!(matches!(book.body, crate::model::BookBody::Article));
        assert_eq!(book.source_slug, "example-com");
        assert!(
            book.story
                .text_html
                .unwrap()
                .as_str()
                .contains("readable paragraph")
        );
    }
}
