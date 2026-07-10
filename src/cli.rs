use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    author,
    version,
    about = "Send articles and discussion threads to Kindle as EPUB"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(value_name = "URL_OR_ID")]
    url: Option<String>,

    #[arg(long, conflicts_with = "email_only")]
    no_email: bool,

    #[arg(long, conflicts_with = "no_email")]
    email_only: bool,

    #[arg(long, value_name = "DIR")]
    output_dir: Option<PathBuf>,

    #[arg(long, value_name = "N")]
    max_depth: Option<usize>,

    #[arg(long)]
    keep_html: bool,
}

#[derive(Subcommand)]
enum Command {
    Run(RunArgs),
    Init,
    Install(InstallArgs),
}

#[derive(Args, Clone)]
pub struct RunArgs {
    pub url: String,

    #[arg(long, conflicts_with = "email_only")]
    pub no_email: bool,

    #[arg(long, conflicts_with = "no_email")]
    pub email_only: bool,

    #[arg(long, value_name = "DIR")]
    pub output_dir: Option<PathBuf>,

    #[arg(long, value_name = "N")]
    pub max_depth: Option<usize>,

    #[arg(long)]
    pub keep_html: bool,
}

#[derive(Args, Clone)]
pub struct InstallArgs {
    #[arg(long)]
    pub extension_id: String,

    #[arg(long)]
    pub firefox_id: Option<String>,

    #[arg(long)]
    pub dry_run: bool,
}

pub enum Commands {
    Run(RunArgs),
    Init,
    Install(InstallArgs),
}

impl Cli {
    pub fn into_command(self) -> Result<Commands> {
        let has_run_flags = self.url.is_some()
            || self.no_email
            || self.email_only
            || self.output_dir.is_some()
            || self.max_depth.is_some()
            || self.keep_html;
        match self.command {
            Some(Command::Run(args)) => Ok(Commands::Run(args)),
            Some(Command::Init) => {
                reject_run_flags(has_run_flags)?;
                Ok(Commands::Init)
            }
            Some(Command::Install(args)) => {
                reject_run_flags(has_run_flags)?;
                Ok(Commands::Install(args))
            }
            None => {
                let url = self.url.ok_or_else(|| {
                    anyhow::anyhow!("usage: kindlecast <url-or-hn-id> [--no-email]")
                })?;
                Ok(Commands::Run(RunArgs {
                    url,
                    no_email: self.no_email,
                    email_only: self.email_only,
                    output_dir: self.output_dir,
                    max_depth: self.max_depth,
                    keep_html: self.keep_html,
                }))
            }
        }
    }
}

fn reject_run_flags(has_run_flags: bool) -> Result<()> {
    if has_run_flags {
        bail!("run options can only be used with a thread URL or `run`");
    }
    Ok(())
}
