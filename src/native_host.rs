use crate::{JobOptions, run_job};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::io::{self, Read, Write};
use std::path::PathBuf;

const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct Request {
    action: String,
    url: String,
    #[serde(default)]
    page_html: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum Response {
    Progress {
        stage: String,
        detail: String,
    },
    Ok {
        title: String,
        comments: usize,
        file: String,
        emailed: bool,
    },
    Error {
        message: String,
    },
}

pub fn run() -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();

    while let Some(request) = read_request(&mut input)? {
        let result = handle_request(request, &mut output);
        if let Err(err) = result {
            let _ = write_response(
                &mut output,
                &Response::Error {
                    message: format!("{err:#}"),
                },
            );
        }
    }
    Ok(())
}

fn handle_request(request: Request, output: &mut impl Write) -> Result<()> {
    let action = request.action.as_str();
    if action != "send" && action != "download" {
        bail!("unsupported action `{}`", request.action);
    }
    let output_cell = RefCell::new(output);
    let result = run_job(
        JobOptions {
            url: request.url,
            page_html: request.page_html,
            no_email: action == "download",
            email_only: false,
            output_dir: None::<PathBuf>,
            max_depth: None,
            keep_html: false,
        },
        &|stage, detail| {
            let mut output = output_cell.borrow_mut();
            let _ = write_response(
                &mut **output,
                &Response::Progress {
                    stage: stage.to_string(),
                    detail: detail.to_string(),
                },
            );
        },
    )?;
    let mut output = output_cell.borrow_mut();
    write_response(
        &mut **output,
        &Response::Ok {
            title: result.title,
            comments: result.comments,
            file: result.file.display().to_string(),
            emailed: result.emailed,
        },
    )
}

fn read_request(input: &mut impl Read) -> Result<Option<Request>> {
    let mut length_bytes = [0u8; 4];
    match input.read_exact(&mut length_bytes) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err).context("failed to read native message length"),
    }
    let length = u32::from_le_bytes(length_bytes) as usize;
    if length > MAX_REQUEST_BYTES {
        bail!("native message too large ({length} bytes; max {MAX_REQUEST_BYTES})");
    }
    let mut body = vec![0u8; length];
    input
        .read_exact(&mut body)
        .context("failed to read native message body")?;
    serde_json::from_slice(&body)
        .context("failed to parse native message")
        .map(Some)
}

fn write_response(output: &mut impl Write, response: &Response) -> Result<()> {
    let body = serde_json::to_vec(response).context("failed to encode native response")?;
    let length = u32::try_from(body.len()).context("native response too large")?;
    output.write_all(&length.to_le_bytes())?;
    output.write_all(&body)?;
    output.flush()?;
    Ok(())
}
