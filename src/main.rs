mod cli;
mod config;
mod email;
mod epub;
mod install;
mod model;
mod native_host;
mod render;
mod sites;

use anyhow::{Context, Result, bail};
use clap::Parser;
use cli::{Cli, Commands, RunArgs};
use config::Config;
use model::ThreadKind;
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
    if should_email && config.is_none() {
        bail!("email config missing; run kindlecast init");
    }

    progress("fetching", "matching URL");
    let site = sites::adapter_for(&options.url)
        .with_context(|| "unsupported URL (HN/Reddit/Lobsters thread, or any http(s) article)")?;
    progress("fetching", site.name());
    let thread = site.fetch(&options.url, options.page_html, &|detail| {
        progress("fetching", detail)
    })?;
    progress("fetching", &fetch_summary(&thread));

    progress("rendering", "rendering Kindle HTML");
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
        .unwrap_or_else(|| include_str!("../assets/kindle.css").to_string());

    let _email_temp_dir;
    let epub_target_dir = match output_dir {
        Some(dir) => dir,
        None => {
            _email_temp_dir =
                TempDir::new().context("failed to create temporary output directory")?;
            _email_temp_dir.path().to_path_buf()
        }
    };

    progress("building", "running pandoc");
    let build = epub::build_epub(
        &thread,
        &css,
        &epub_target_dir,
        max_depth,
        options.keep_html && !options.email_only,
    )?;
    if let Some(path) = &build.html_path {
        progress("building", &format!("kept HTML at {}", path.display()));
    }

    let emailed = if should_email {
        let cfg = config.context("email config missing; run kindlecast init")?;
        progress("emailing", "sending EPUB to Kindle");
        email::send_epub(&cfg, &thread.story.title, &thread.source, &build.epub_path)?;
        true
    } else {
        false
    };

    Ok(JobResult {
        title: thread.story.title,
        comments: thread.comment_count,
        file: build.epub_path,
        emailed,
    })
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
        eprintln!("emailed to Kindle");
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

fn fetch_summary(thread: &model::Thread) -> String {
    match thread.kind {
        ThreadKind::Discussion => {
            format!(
                "{} comments; max depth {}",
                thread.comment_count, thread.max_depth
            )
        }
        ThreadKind::Article => thread
            .story
            .text_html
            .as_deref()
            .map(|html| format!("extracted article ({} chars)", html.len()))
            .unwrap_or_else(|| "extracted article".to_string()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn json_url_with_cli_flag_is_not_native_host_invocation() {
        let args = vec![
            "kindlecast".to_string(),
            "https://example.com/feed.json".to_string(),
            "--no-email".to_string(),
        ];

        assert!(!super::is_native_host_invocation(&args));
    }

    #[test]
    fn extension_origin_is_native_host_invocation() {
        let args = vec![
            "kindlecast".to_string(),
            "chrome-extension://abc/".to_string(),
        ];

        assert!(super::is_native_host_invocation(&args));
    }
}
