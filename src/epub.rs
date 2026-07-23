use crate::images;
use crate::model::{Thread, ThreadKind};
use crate::render::render_html;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

pub struct BuildResult {
    pub epub_path: PathBuf,
    pub html_path: Option<PathBuf>,
}

pub fn build_epub(
    thread: &Thread,
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
    let html = render_html(thread, max_indent_depth);
    let optimized = images::optimize_html(
        &html,
        image_base_url(thread, fallback_url),
        temp.path(),
        progress,
    )?;
    progress(&optimized.stats.summary());
    let html_path = temp.path().join("thread.html");
    let css_path = temp.path().join("kindle.css");
    fs::write(&html_path, optimized.html).context("failed to write temporary HTML")?;
    fs::write(&css_path, css).context("failed to write temporary CSS")?;

    progress("running pandoc");
    let output_path = output_dir.join(epub_filename(thread));
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
        .arg(format!("title={}", thread.story.title))
        .arg("--metadata")
        .arg(format!(
            "author={} · {}",
            thread.source, thread.story.author
        ))
        .arg("--metadata")
        .arg(format!("date={}", thread.story.time.format("%Y-%m-%d")))
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

    let kept_html = if keep_html {
        let keep_path = output_path.with_extension("html");
        fs::write(&keep_path, html)
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

fn image_base_url<'a>(thread: &'a Thread, fallback_url: &'a str) -> Option<&'a str> {
    match thread.kind {
        ThreadKind::Article => thread.story.url.as_deref(),
        ThreadKind::Discussion => thread.story.discussion_url.as_deref(),
    }
    .or(Some(fallback_url))
}

fn epub_filename(thread: &Thread) -> String {
    match thread.kind {
        ThreadKind::Discussion => {
            format!(
                "{}-{}-{}.epub",
                thread.source_slug,
                slug(&thread.story.id),
                slug(&thread.story.title)
            )
        }
        ThreadKind::Article => format!("{}-{}.epub", thread.source_slug, slug(&thread.story.title)),
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
