//! HTTP CLI for canonical operations; bearer tokens are read only from the environment.

use std::error::Error;
use std::io::{Read, Write};
use std::process::ExitCode;

use okf_jawn_contract::metadata::operations;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let matches = match okf_jawn_cli::command().try_get_matches() {
        Ok(matches) => matches,
        Err(error) if error.use_stderr() => return Err(error.into()),
        Err(error) => { write!(std::io::stdout().lock(), "{error}")?; return Ok(()); }
    };
    let (name, _) = matches.subcommand().ok_or("an operation is required")?;
    let operation = operations().into_iter().find(|op| op.id == name).ok_or("unknown operation")?;
    let server = matches.get_one::<String>("server").ok_or("server URL is required")?;
    let mut url = reqwest::Url::parse(server)?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]" | "::1"));
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err("remote endpoints require HTTPS".into());
    }
    if !url.username().is_empty() || url.password().is_some() { return Err("credentials in URLs are forbidden".into()); }
    url.set_path(operation.path); url.set_query(None); url.set_fragment(None);
    let argument = matches.get_one::<String>("json").ok_or("JSON input is required")?;
    let text = if argument == "-" {
        let mut text = String::new(); std::io::stdin().read_to_string(&mut text)?; text
    } else if let Some(path) = argument.strip_prefix('@') { std::fs::read_to_string(path)? }
    else { argument.clone() };
    let input: serde_json::Value = serde_json::from_str(&text)?;
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(120)).build()?;
    let mut request = client.post(url).json(&input);
    if let Ok(token) = std::env::var("OKF_JAWN_TOKEN") { request = request.bearer_auth(token); }
    let response = request.send().await?;
    let status = response.status(); let body = response.text().await?;
    if !status.is_success() { return Err(format!("HTTP {status}: {body}").into()); }
    writeln!(std::io::stdout().lock(), "{body}")?;
    Ok(())
}
