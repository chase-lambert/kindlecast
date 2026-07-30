mod cli;
mod config;
mod corpus;
mod email;
mod epub;
mod images;
mod install;
mod model;
mod native_host;
mod net;
mod render;
mod sanitize;
mod sites;
mod util;

use anyhow::{Context, Result, bail};
use clap::Parser;
use cli::{Cli, Commands, RunArgs};
use config::Config;
use model::BookBody;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;

pub struct JobOptions {
    pub url: String,
    pub page_html: Option<String>,
    pub no_email: bool,
    pub email_only: bool,
    pub output_dir: Option<PathBuf>,
    pub max_depth: Option<usize>,
    pub keep_html: bool,
}

pub struct JobResult {
    pub title: String,
    pub comments: usize,
    pub file: PathBuf,
    pub emailed: bool,
}

fn is_native_host_invocation(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg.starts_with("chrome-extension://")
            || arg.starts_with("moz-extension://")
            || arg.starts_with("safari-web-extension://")
    }) || args
        .get(1)
        .is_some_and(|arg| arg.ends_with(".json") && Path::new(arg).is_file())
}

pub fn run_job(options: JobOptions, progress: &dyn Fn(&str, &str)) -> Result<JobResult> {
    if options.no_email && options.email_only {
        bail!("--no-email and --email-only cannot be used together");
    }

    let should_email = !options.no_email;
    let config = Config::load_optional()?;
    if should_email {
        let cfg = config
            .as_ref()
            .context("email config missing; run rustypub init")?;
        cfg.ensure_email_configured()?;
    }

    progress("fetching", "matching URL");
    let site = sites::adapter_for(&options.url)
        .with_context(|| "unsupported URL (HN/Reddit/Lobsters thread, or any http(s) article)")?;
    progress("fetching", site.name());
    let book = site.fetch(&options.url, options.page_html, &|detail| {
        progress("fetching", detail)
    })?;
    progress("fetching", &fetch_summary(&book));

    progress("rendering", "rendering book HTML");
    let output_dir = if options.email_only {
        None
    } else {
        Some(
            options
                .output_dir
                .clone()
                .or_else(|| config.as_ref().map(|cfg| cfg.output_dir()))
                .unwrap_or_else(config::default_output_dir),
        )
    };
    let max_depth = options
        .max_depth
        .or_else(|| config.as_ref().map(|cfg| cfg.max_indent_depth))
        .unwrap_or(config::DEFAULT_MAX_INDENT_DEPTH);
    let css = config
        .as_ref()
        .and_then(|cfg| cfg.css_override().transpose())
        .transpose()?
        .unwrap_or_else(|| include_str!("../assets/reader.css").to_string());

    let _email_temp_dir;
    let epub_target_dir = match output_dir {
        Some(dir) => dir,
        None => {
            _email_temp_dir =
                TempDir::new().context("failed to create temporary output directory")?;
            _email_temp_dir.path().to_path_buf()
        }
    };

    let build = epub::build_epub(
        &book,
        &css,
        &epub_target_dir,
        max_depth,
        options.keep_html && !options.email_only,
        &options.url,
        &|detail| progress("building", detail),
    )?;
    if let Some(path) = &build.html_path {
        progress("building", &format!("kept HTML at {}", path.display()));
    }

    let emailed = if should_email {
        let cfg = config.context("email config missing; run rustypub init")?;
        let epub_size = email::epub_size(&build.epub_path)?;
        if epub_size > email::MAX_EMAIL_EPUB_BYTES {
            let diagnosis = email::oversized_epub_diagnosis(epub_size);
            if options.email_only {
                let recovery = preserve_email_only_epub(&build.epub_path, &cfg.output_dir());
                match recovery {
                    Ok(path) => bail!(
                        "{diagnosis}. Saved the completed book to {}. Import it through your reader's library or desktop app",
                        path.display()
                    ),
                    Err(error) => bail!(
                        "{diagnosis}. The temporary book could not be preserved: {error:#}. Build again with --no-email, then import the EPUB through your reader's library or desktop app"
                    ),
                }
            }
            bail!(
                "{diagnosis}. The completed book remains at {}. Import it through your reader's library or desktop app",
                build.epub_path.display()
            );
        }
        progress("emailing", "sending EPUB attachment");
        email::send_epub(&cfg, &book.story.title, &book.source, &build.epub_path)?;
        true
    } else {
        false
    };

    let comments = match &book.body {
        BookBody::Discussion(discussion) => discussion.comment_count(),
        BookBody::Article => 0,
    };

    Ok(JobResult {
        title: book.story.title,
        comments,
        file: build.epub_path,
        emailed,
    })
}

fn preserve_email_only_epub(epub_path: &Path, output_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let filename = epub_path
        .file_name()
        .context("temporary EPUB has no filename")?;
    let mut destination = output_dir.join(filename);
    if destination.exists() {
        let stem = epub_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("rustypub");
        let extension = epub_path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("epub");
        destination = (1..=999)
            .map(|index| output_dir.join(format!("{stem}-email-recovery-{index}.{extension}")))
            .find(|candidate| !candidate.exists())
            .context("no available recovery filename")?;
    }
    fs::copy(epub_path, &destination).with_context(|| {
        format!(
            "failed to copy {} to {}",
            epub_path.display(),
            destination.display()
        )
    })?;
    Ok(destination)
}

fn run_cli(args: RunArgs) -> Result<()> {
    let email_only = args.email_only;
    let result = run_job(
        JobOptions {
            url: args.url,
            page_html: None,
            no_email: args.no_email,
            email_only: args.email_only,
            output_dir: args.output_dir,
            max_depth: args.max_depth,
            keep_html: args.keep_html,
        },
        &|stage, detail| eprintln!("{stage}: {detail}"),
    )?;

    if result.emailed {
        eprintln!("emailed EPUB");
    }
    if !email_only && !result.file.as_os_str().is_empty() {
        println!("{}", result.file.display());
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = if is_native_host_invocation(&args) {
        native_host::run()
    } else {
        let cli = Cli::parse();
        match cli.into_command() {
            Ok(Commands::Run(args)) => run_cli(args),
            Ok(Commands::Init) => config::init_config(),
            Ok(Commands::Install(args)) => install::install(args),
            Err(err) => Err(err),
        }
    };

    if let Err(err) = result {
        eprintln!("{err:#}");
        std::process::exit(1);
    }
}

fn fetch_summary(book: &model::Book) -> String {
    match &book.body {
        BookBody::Discussion(discussion) => {
            // The book itself is the reader's source of truth; this line exists
            // so the operator is not surprised by a smaller count than the
            // thread showed.
            let budget = match (discussion.is_truncated(), discussion.all_threads_included()) {
                (false, _) => String::new(),
                (true, true) => format!(
                    " (of {}; all {} threads)",
                    discussion.total_comment_count(),
                    discussion.total_threads()
                ),
                (true, false) => format!(
                    " (of {}; {} of {} threads)",
                    discussion.total_comment_count(),
                    discussion.included_threads(),
                    discussion.total_threads()
                ),
            };
            format!(
                "{} comments{}; max depth {}",
                discussion.comment_count(),
                budget,
                discussion.max_depth()
            )
        }
        BookBody::Article => book
            .story
            .text_html
            .as_ref()
            .map(|html| format!("extracted article ({} chars)", html.as_str().len()))
            .unwrap_or_else(|| "extracted article".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    #[test]
    fn json_url_with_cli_flag_is_not_native_host_invocation() {
        let args = vec![
            "rustypub".to_string(),
            "https://example.com/feed.json".to_string(),
            "--no-email".to_string(),
        ];

        assert!(!super::is_native_host_invocation(&args));
    }

    #[test]
    fn extension_origin_is_native_host_invocation() {
        let args = vec![
            "rustypub".to_string(),
            "chrome-extension://abc/".to_string(),
        ];

        assert!(super::is_native_host_invocation(&args));
    }

    #[test]
    fn firefox_manifest_path_argument_is_native_host_invocation() {
        // Firefox launches the host with the path to the extension manifest as
        // argv[1]. Entry-point routing only looks at that path; the add-on ID
        // (later argv) is not re-checked here — browser allowlists own trust.
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("extension/manifest.firefox.json");
        let manifest = std::fs::canonicalize(manifest).unwrap();
        let args = vec![
            "rustypub".to_string(),
            manifest.display().to_string(),
            crate::install::FIREFOX_EXTENSION_ID.to_string(),
        ];

        assert!(super::is_native_host_invocation(&args));
    }

    #[test]
    fn email_only_recovery_preserves_book_without_overwriting() {
        let temp = tempdir().unwrap();
        let build_dir = temp.path().join("build");
        let output_dir = temp.path().join("downloads");
        fs::create_dir_all(&build_dir).unwrap();
        fs::create_dir_all(&output_dir).unwrap();
        let epub_path = build_dir.join("article.epub");
        fs::write(&epub_path, b"new book").unwrap();
        fs::write(output_dir.join("article.epub"), b"existing book").unwrap();

        let recovered = super::preserve_email_only_epub(&epub_path, &output_dir).unwrap();

        assert_eq!(
            recovered.file_name().unwrap(),
            "article-email-recovery-1.epub"
        );
        assert_eq!(fs::read(recovered).unwrap(), b"new book");
        assert_eq!(
            fs::read(output_dir.join("article.epub")).unwrap(),
            b"existing book"
        );
    }
}
