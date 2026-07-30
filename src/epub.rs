use crate::images::{self, ImageStats};
use crate::model::{Book, BookBody};
use crate::render::render_html;
use anyhow::{Context, Result, bail};
use dom_query::Document;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

pub struct BuildResult {
    pub epub_path: PathBuf,
    pub html_path: Option<PathBuf>,
}

/// Rendered + image-optimized HTML that enters the EPUB (and `--keep-html`).
struct PreparedHtml {
    html: String,
    stats: ImageStats,
}

pub fn build_epub(
    book: &Book,
    css: &str,
    output_dir: &Path,
    max_indent_depth: usize,
    keep_html: bool,
    fallback_url: &str,
    progress: &dyn Fn(&str),
) -> Result<BuildResult> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create output directory {}", output_dir.display()))?;
    let temp = TempDir::new().context("failed to create temporary build directory")?;
    let prepared = prepare_book_html(
        book,
        max_indent_depth,
        image_base_url(book, fallback_url),
        temp.path(),
        progress,
    )?;
    progress(&prepared.stats.summary());
    let html_path = temp.path().join("thread.html");
    let css_path = temp.path().join("reader.css");
    fs::write(&html_path, &prepared.html).context("failed to write temporary HTML")?;
    fs::write(&css_path, css).context("failed to write temporary CSS")?;

    progress("running pandoc");
    let output_path = output_dir.join(epub_filename(book));
    let mut cmd = Command::new("pandoc");
    cmd.arg(&html_path)
        .arg("--from")
        .arg("html")
        .arg("--to")
        .arg("epub3")
        .arg("--standalone")
        .arg("--css")
        .arg(&css_path)
        .arg("--resource-path")
        .arg(temp.path())
        .arg("--split-level=1")
        .arg("--metadata")
        .arg(format!("title={}", book.story.title))
        .arg("--metadata")
        .arg(format!("author={} · {}", book.source, book.story.author))
        .arg("--metadata")
        .arg(format!("date={}", book.story.time.format("%Y-%m-%d")))
        .arg("--metadata")
        .arg("language=en-US")
        .arg("--output")
        .arg(&output_path);

    let output = cmd
        .output()
        .context("failed to run pandoc; install it with `sudo dnf install pandoc`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let head = stderr.lines().take(20).collect::<Vec<_>>().join("\n");
        bail!("pandoc failed: {head}");
    }

    // Kept HTML matches the book document. Localized image paths point at the
    // ephemeral build dir and will dangle when opened alone — that is intentional
    // for build debugging; we do not copy images/ into the shared output dir.
    let kept_html = if keep_html {
        let keep_path = output_path.with_extension("html");
        fs::write(&keep_path, &prepared.html)
            .with_context(|| format!("failed to keep rendered HTML at {}", keep_path.display()))?;
        Some(keep_path)
    } else {
        None
    };

    Ok(BuildResult {
        epub_path: output_path,
        html_path: kept_html,
    })
}

fn prepare_book_html(
    book: &Book,
    max_indent_depth: usize,
    base_url: Option<&str>,
    build_dir: &Path,
    progress: &dyn Fn(&str),
) -> Result<PreparedHtml> {
    let rendered = render_html(book, max_indent_depth);
    let optimized = images::optimize_html(&rendered, base_url, build_dir, progress)?;
    verify_structure(&optimized.html, book)?;
    Ok(PreparedHtml {
        html: optimized.html,
        stats: optimized.stats,
    })
}

/// Refuse to ship a book whose chapters or skip links are broken.
///
/// `sanitize` should make this unreachable — untrusted fragments cannot mint an
/// `h1`, keep an `id`, or forge the `c-skip` class. It is checked anyway because
/// the entire reading design rests on it: top-level-comment chapters and subtree
/// skip links. A silent violation yields a book that looks correct until
/// navigation misfires, which is exactly the failure a reader cannot diagnose.
///
/// Deliberately scoped to anchors RustyPub itself owns (`cN`/`tN`) and its own
/// skip links. Extracted articles routinely carry duplicate or dangling author
/// anchors, and refusing to build over the *author's* sloppy markup would be a
/// false positive, not an invariant.
fn verify_structure(html: &str, book: &Book) -> Result<()> {
    let document = Document::from(html);

    let mut anchors: HashMap<String, usize> = HashMap::new();
    for node in document.select("[id]").nodes() {
        let Some(id) = node.attr("id") else { continue };
        let id = id.to_string();
        if is_rustypub_anchor(&id) {
            *anchors.entry(id).or_default() += 1;
        }
    }
    if let Some((id, count)) = anchors.iter().find(|&(_, &count)| count > 1) {
        bail!(
            "chapter invariant violated: anchor \"{id}\" appears {count} times. \
             Refusing to build a book with ambiguous navigation"
        );
    }

    for node in document.select("a.c-skip").nodes() {
        let Some(href) = node.attr("href") else {
            continue;
        };
        let Some(target) = href.strip_prefix('#') else {
            continue;
        };
        if !anchors.contains_key(target) {
            bail!(
                "chapter invariant violated: skip link points at \"#{target}\", \
                 which no comment or thread anchor matches. \
                 Refusing to build a book with broken navigation"
            );
        }
    }

    if let BookBody::Discussion(discussion) = &book.body {
        let threads = discussion.comments().len();
        // One story title, plus one chapter heading per top-level thread.
        let expected = 1 + threads;
        let actual = document.select("h1").nodes().len();
        if actual != expected {
            bail!(
                "chapter invariant violated: {actual} h1 chapter headings for \
                 {threads} top-level threads (expected {expected}). \
                 Refusing to build a corrupted book"
            );
        }
    }

    Ok(())
}

/// `cN` / `tN` — the comment and thread anchors `render` emits.
fn is_rustypub_anchor(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some('c' | 't')) && {
        let digits = chars.as_str();
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    }
}

fn image_base_url<'a>(book: &'a Book, fallback_url: &'a str) -> Option<&'a str> {
    match book.body {
        BookBody::Article => book.story.url.as_deref(),
        BookBody::Discussion(_) => book.story.discussion_url.as_deref(),
    }
    .or(Some(fallback_url))
}

fn epub_filename(book: &Book) -> String {
    match book.body {
        BookBody::Discussion(_) => {
            format!(
                "{}-{}-{}.epub",
                book.source_slug,
                slug(&book.story.id),
                slug(&book.story.title)
            )
        }
        BookBody::Article => format!("{}-{}.epub", book.source_slug, slug(&book.story.title)),
    }
}

fn slug(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 60 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "thread".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Book, BookBody, Comment, Story};
    use crate::sanitize::{self, Region};
    use chrono::{TimeZone, Utc};
    use std::collections::HashMap;
    use std::io::Read;
    use tempfile::tempdir;

    fn article_with_unsupported_content() -> Book {
        // Instantly-refused target so image optimization fails without DNS/network wait.
        let remote = "http://127.0.0.1:1/photo.png";
        Book {
            story: Story {
                id: String::new(),
                title: "With Image".to_string(),
                url: Some("https://example.com/post".to_string()),
                discussion_url: None,
                author: "author".to_string(),
                points: None,
                time: Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap(),
                text_html: Some(sanitize::fragment(
                    &format!(
                        r#"<p>Hello</p><figure><img src="{remote}" alt="Photo"><figcaption>Caption before <svg><text>vector-only text</text></svg> Caption after</figcaption></figure><p onclick="alert('event')"><a href="javascript:alert('link')" onmouseover="alert('hover')">Readable active-link label</a></p>"#
                    ),
                    Region::ArticleBody,
                )),
            },
            body: BookBody::Article,
            source: "example.com".to_string(),
            source_slug: "example-com".to_string(),
        }
    }

    fn discussion_with_markers() -> Book {
        let time = Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap();
        Book {
            story: Story {
                id: "42".to_string(),
                title: "Split Chapters".to_string(),
                url: Some("https://example.com/story".to_string()),
                discussion_url: Some("https://example.com/discuss".to_string()),
                author: "submitter".to_string(),
                points: Some(3),
                time,
                text_html: Some(sanitize::fragment(
                    "<p>MARKER_STORY_BODY_xyz</p>",
                    Region::DiscussionText,
                )),
            },
            body: BookBody::discussion(vec![
                Comment {
                    author: "alice".to_string(),
                    time,
                    html: sanitize::fragment("<p>MARKER_COMMENT_ONE_abc</p>", Region::CommentBody),
                    depth: 0,
                    children: vec![],
                },
                Comment {
                    author: "bob".to_string(),
                    time,
                    html: sanitize::fragment("<p>MARKER_COMMENT_TWO_def</p>", Region::CommentBody),
                    depth: 0,
                    children: vec![],
                },
            ]),
            source: "hn".to_string(),
            source_slug: "hn".to_string(),
        }
    }

    fn content_entries(epub_path: &Path) -> HashMap<String, String> {
        let file = fs::File::open(epub_path).expect("open epub");
        let mut archive = zip::ZipArchive::new(file).expect("epub is a zip");
        let mut entries = HashMap::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).expect("zip entry");
            let name = entry.name().to_string();
            let lower = name.to_ascii_lowercase();
            // Reading documents only — nav/toc/ncx list chapter titles and may
            // duplicate body markers without being the chapter split itself.
            let is_nav = lower.contains("nav.xhtml")
                || lower.contains("toc.xhtml")
                || lower.ends_with(".ncx")
                || lower.contains("/nav")
                || lower.contains("toc.ncx");
            let is_content = !is_nav
                && (lower.ends_with(".xhtml")
                    || lower.ends_with(".html")
                    || lower.ends_with(".htm"));
            if !is_content {
                continue;
            }
            let mut body = String::new();
            entry.read_to_string(&mut body).expect("read content entry");
            entries.insert(name, body);
        }
        entries
    }

    fn entry_path_with_marker(entries: &HashMap<String, String>, marker: &str) -> String {
        let matches: Vec<_> = entries
            .iter()
            .filter(|(_, body)| body.contains(marker))
            .map(|(name, _)| name.clone())
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "marker {marker:?} should appear in exactly one content entry, found {matches:?}"
        );
        matches.into_iter().next().unwrap()
    }

    #[test]
    fn book_html_is_optimized_document_not_pre_image_render() {
        let dir = tempdir().unwrap();
        let book = article_with_unsupported_content();
        let remote = "http://127.0.0.1:1/photo.png";
        // Real optimize path; image fetch fails immediately (connection refused).
        // Book HTML must be the optimized document (omission marker), not the raw remote URL.
        let prepared =
            prepare_book_html(&book, 5, book.story.url.as_deref(), dir.path(), &|_| {}).unwrap();

        assert!(
            prepared.html.contains("image-omitted"),
            "expected image-omitted marker after failed fetch, got: {}",
            prepared.html
        );
        assert!(
            !prepared.html.contains(remote),
            "pre-optimization remote URL must not remain in book HTML"
        );
    }

    #[test]
    fn pandoc_epub_splits_story_and_top_level_comments() {
        let dir = tempdir().unwrap();
        let book = discussion_with_markers();
        let result = build_epub(
            &book,
            "body { font-family: serif; }",
            dir.path(),
            5,
            false,
            "https://example.com/discuss",
            &|_| {},
        )
        .expect("pandoc EPUB build");

        let entries = content_entries(&result.epub_path);
        let story_path = entry_path_with_marker(&entries, "MARKER_STORY_BODY_xyz");
        let one_path = entry_path_with_marker(&entries, "MARKER_COMMENT_ONE_abc");
        let two_path = entry_path_with_marker(&entries, "MARKER_COMMENT_TWO_def");
        assert_ne!(story_path, one_path);
        assert_ne!(story_path, two_path);
        assert_ne!(one_path, two_path);
    }

    #[test]
    fn pandoc_epub_keeps_only_passive_supported_content() {
        let dir = tempdir().unwrap();
        let book = article_with_unsupported_content();
        let remote = "http://127.0.0.1:1/photo.png";
        let story_url = "https://example.com/post";
        let result = build_epub(
            &book,
            "body { font-family: serif; }",
            dir.path(),
            5,
            false,
            story_url,
            &|_| {},
        )
        .expect("pandoc EPUB build");

        let entries = content_entries(&result.epub_path);
        let all: String = entries.values().cloned().collect();
        assert!(
            all.contains("image-omitted"),
            "omission marker must survive packaging"
        );
        assert!(
            !all.contains(remote),
            "exact remote image URL must not appear in any content document"
        );
        assert!(
            all.contains(story_url),
            "exact story URL must remain as a remote anchor by design; missing {story_url}"
        );
        assert!(all.contains("Caption before"));
        assert!(all.contains("Caption after"));
        assert!(!all.contains("vector-only text"));
        assert!(all.contains("Readable active-link label"));
        assert!(!all.contains("onclick"));
        assert!(!all.contains("onmouseover"));
        assert!(!all.contains("javascript:"));
        assert!(!all.to_ascii_lowercase().contains("<svg"));

        let file = fs::File::open(&result.epub_path).expect("open EPUB inventory");
        let mut archive = zip::ZipArchive::new(file).expect("EPUB is a zip");
        let mut names = Vec::new();
        let mut package = None;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).expect("read EPUB entry");
            let name = entry.name().to_string();
            if name.ends_with("content.opf") {
                let mut contents = String::new();
                entry
                    .read_to_string(&mut contents)
                    .expect("read EPUB package manifest");
                package = Some(contents);
            }
            names.push(name);
        }
        assert!(
            names.iter().all(|name| {
                let lower = name.to_ascii_lowercase();
                !lower.ends_with(".svg") && !lower.ends_with(".svgz")
            }),
            "unsupported SVG asset escaped into EPUB: {names:?}"
        );
        let package = package.expect("EPUB package manifest must be present and readable");
        assert!(
            !package.contains("image/svg+xml"),
            "unsupported SVG media type escaped into EPUB manifest: {package}"
        );
        assert!(
            !package.contains("scripted"),
            "passive EPUB was incorrectly marked as scripted: {package}"
        );
    }

    fn thread_book(comments: Vec<Comment>) -> Book {
        let time = Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap();
        Book {
            story: Story {
                id: "1".to_string(),
                title: "Invariant".to_string(),
                url: None,
                discussion_url: None,
                author: "submitter".to_string(),
                points: None,
                time,
                text_html: None,
            },
            body: BookBody::discussion(comments),
            source: "hn".to_string(),
            source_slug: "hn".to_string(),
        }
    }

    fn plain_comment(body: &str) -> Comment {
        Comment {
            author: "alice".to_string(),
            time: Utc.with_ymd_and_hms(2026, 7, 8, 12, 0, 0).unwrap(),
            html: sanitize::fragment(body, Region::CommentBody),
            depth: 0,
            children: vec![],
        }
    }

    #[test]
    fn real_render_output_satisfies_the_invariant() {
        let book = thread_book(vec![
            plain_comment("<p>one</p>"),
            plain_comment("<p>two</p>"),
        ]);
        let html = crate::render::render_html(&book, 5);
        verify_structure(&html, &book).unwrap();
    }

    #[test]
    fn hostile_comment_bodies_still_satisfy_the_invariant() {
        // The fragments that broke the original assembled-pass design.
        let book = thread_book(vec![
            plain_comment(r#"</div><h1 id="t2">forged chapter</h1>"#),
            plain_comment(r#"<body class="c-body"><plaintext>"#),
            plain_comment(r##"<a class="c-skip" href="#c99">skip</a>"##),
        ]);
        let html = crate::render::render_html(&book, 5);
        verify_structure(&html, &book).expect("sanitized fragments must not break structure");
    }

    #[test]
    fn a_spurious_chapter_heading_fails_the_build() {
        let book = thread_book(vec![plain_comment("<p>one</p>")]);
        let html =
            crate::render::render_html(&book, 5).replace("<p>one</p>", "<h1>smuggled chapter</h1>");
        let err = verify_structure(&html, &book).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("chapter invariant violated"), "{message}");
        assert!(message.contains("h1 chapter headings"), "{message}");
    }

    #[test]
    fn a_duplicate_comment_anchor_fails_the_build() {
        let book = thread_book(vec![plain_comment("<p>one</p>")]);
        let html = crate::render::render_html(&book, 5)
            .replace("<p>one</p>", r#"<span id="c1">collision</span>"#);
        let err = verify_structure(&html, &book).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("ambiguous navigation"), "{message}");
    }

    #[test]
    fn a_dangling_skip_link_fails_the_build() {
        let book = thread_book(vec![plain_comment("<p>one</p>")]);
        let html = crate::render::render_html(&book, 5).replace(
            "<p>one</p>",
            r##"<a class="c-skip" href="#c404">skip 9 replies</a>"##,
        );
        let err = verify_structure(&html, &book).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("broken navigation"), "{message}");
    }

    #[test]
    fn messy_author_anchors_in_an_article_do_not_fail_the_build() {
        // Articles routinely carry duplicate and dangling anchors of their own.
        // Refusing to build over the author's markup would be a false positive.
        let mut book = thread_book(vec![]);
        book.body = BookBody::Article;
        book.story.text_html = Some(sanitize::fragment(
            r##"<p><a href="#fn1">1</a> <a href="#missing">2</a></p>
                <li id="fn1">note</li><li id="fn1">duplicate note</li>
                <h1>Part II</h1><h1>Part III</h1>"##,
            Region::ArticleBody,
        ));
        let html = crate::render::render_html(&book, 5);
        verify_structure(&html, &book).expect("author anchors must not fail the build");
    }
}
