//! Standalone iii qualification worker.
//!
//! Registers durable:subscriber handlers with gate-file crash points.
//! Returning `Ok` from a handler is the queue acknowledgement. Does not
//! implement product `RecordStore` / `JobQueue` ports.

use iii_sdk::channel::ChannelReader;
use iii_sdk::helpers::{ChannelDirection, create_channel, extract_channel_refs};
use iii_sdk::protocol::{RegisterTriggerInput, TriggerRequest};
use iii_sdk::{Error, InitOptions, RegisterFunction, register_worker};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::Duration;

#[derive(Debug, Deserialize, JsonSchema)]
struct ImportJob {
    job_id: String,
    #[serde(default)]
    blob_path: Option<String>,
    #[serde(default)]
    blob_sha256: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ImportResult {
    completed: bool,
    first_effect: bool,
    job_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChannelJob {
    job_id: String,
    #[serde(default)]
    reader: Option<Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ChannelResult {
    bytes: usize,
    job_id: String,
    sha256: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChannelSendRequest {
    job_id: String,
    payload: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ChannelSendResult {
    published: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FailJob {
    job_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct FailResult {
    completed: bool,
    job_id: String,
}

const FAIL_GATE: &str = "FAIL_GATE";
const GATE_AFTER_EFFECT: &str = "GATE_AFTER_EFFECT";
const GATE_BEFORE_WORK: &str = "GATE_BEFORE_WORK";
const READY_FILE: &str = "READY";

fn control_dir() -> Result<PathBuf, Error> {
    env::var("OKF_III_CONTROL_DIR")
        .map(PathBuf::from)
        .map_err(|_| Error::Handler("OKF_III_CONTROL_DIR is required".into()))
}

fn marker_dir() -> Result<PathBuf, Error> {
    env::var("OKF_III_MARKER_DIR")
        .map(PathBuf::from)
        .map_err(|_| Error::Handler("OKF_III_MARKER_DIR is required".into()))
}

fn ensure_dir(path: &Path) -> Result<(), Error> {
    fs::create_dir_all(path)
        .map_err(|error| Error::Handler(format!("mkdir {}: {error}", path.display())))
}

fn touch_status(name: &str, content: &str) -> Result<(), Error> {
    let path = control_dir()?.join(name);
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let mut file = File::create(&path)
        .map_err(|error| Error::Handler(format!("create {}: {error}", path.display())))?;
    file.write_all(content.as_bytes())
        .map_err(|error| Error::Handler(format!("write {}: {error}", path.display())))?;
    Ok(())
}

fn gate_present(name: &str) -> Result<bool, Error> {
    Ok(control_dir()?.join(name).exists())
}

fn wait_while_gate(name: &str, job_id: &str) -> Result<(), Error> {
    if !gate_present(name)? {
        return Ok(());
    }
    touch_status(&format!("BLOCKED_{name}_{job_id}"), "waiting")?;
    while gate_present(name)? {
        thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_file(control_dir()?.join(format!("BLOCKED_{name}_{job_id}")));
    Ok(())
}

fn append_delivery(job_id: &str) -> Result<u64, Error> {
    let dir = marker_dir()?;
    ensure_dir(&dir)?;
    let path = dir.join(format!("deliveries_{job_id}"));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| Error::Handler(format!("open {}: {error}", path.display())))?;
    file.write_all(b"1\n")
        .map_err(|error| Error::Handler(format!("append {}: {error}", path.display())))?;
    let count = fs::read_to_string(&path)
        .map_err(|error| Error::Handler(format!("read {}: {error}", path.display())))?
        .lines()
        .count();
    u64::try_from(count).map_err(|_| Error::Handler("delivery count overflow".into()))
}

fn create_effect_marker(job_id: &str) -> Result<bool, Error> {
    let dir = marker_dir()?;
    ensure_dir(&dir)?;
    let path = dir.join(format!("effect_{job_id}"));
    match File::create_new(&path) {
        Ok(mut file) => {
            file.write_all(b"effect\n")
                .map_err(|error| Error::Handler(format!("write {}: {error}", path.display())))?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(Error::Handler(format!(
            "create_new {}: {error}",
            path.display()
        ))),
    }
}

fn mark_completed(job_id: &str) -> Result<(), Error> {
    let dir = marker_dir()?;
    ensure_dir(&dir)?;
    let path = dir.join(format!("completed_{job_id}"));
    fs::write(&path, b"ok\n")
        .map_err(|error| Error::Handler(format!("write {}: {error}", path.display())))
}

fn append_effect_ledger(job_id: &str, first_effect: bool) -> Result<(), Error> {
    let dir = marker_dir()?;
    ensure_dir(&dir)?;
    let path = dir.join(format!("effect_ledger_{job_id}"));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| Error::Handler(format!("open {}: {error}", path.display())))?;
    let line = if first_effect { "true\n" } else { "false\n" };
    file.write_all(line.as_bytes())
        .map_err(|error| Error::Handler(format!("append {}: {error}", path.display())))?;
    Ok(())
}

fn handle_import(job: &ImportJob) -> Result<ImportResult, Error> {
    let job_id = job.job_id.clone();
    touch_status(&format!("ENTERED_{job_id}"), "import")?;
    if let Some(blob_path) = job.blob_path.as_ref() {
        let expected = job
            .blob_sha256
            .as_ref()
            .ok_or_else(|| Error::Handler("blob_path requires blob_sha256".into()))?;
        let bytes = fs::read(blob_path)
            .map_err(|error| Error::Handler(format!("read blob {blob_path}: {error}")))?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if digest != *expected {
            return Err(Error::Handler(format!(
                "blob sha mismatch for {job_id}: got {digest} expected {expected}"
            )));
        }
        let dir = marker_dir()?;
        ensure_dir(&dir)?;
        fs::write(dir.join(format!("blob_ok_{job_id}")), format!("{digest}\n"))
            .map_err(|error| Error::Handler(format!("write blob_ok: {error}")))?;
    }
    let _deliveries = append_delivery(&job_id)?;
    wait_while_gate(GATE_BEFORE_WORK, &job_id)?;
    let first_effect = create_effect_marker(&job_id)?;
    append_effect_ledger(&job_id, first_effect)?;
    wait_while_gate(GATE_AFTER_EFFECT, &job_id)?;
    mark_completed(&job_id)?;
    Ok(ImportResult {
        completed: true,
        first_effect,
        job_id,
    })
}

fn handle_fail(job: FailJob) -> Result<FailResult, Error> {
    let job_id = job.job_id;
    touch_status(&format!("ENTERED_FAIL_{job_id}"), "fail")?;
    let _ = append_delivery(&format!("fail_{job_id}"))?;
    if gate_present(FAIL_GATE)? {
        return Err(Error::Handler(format!("forced failure for {job_id}")));
    }
    let _ = create_effect_marker(&format!("fail_{job_id}"))?;
    mark_completed(&format!("fail_{job_id}"))?;
    Ok(FailResult {
        completed: true,
        job_id,
    })
}

async fn handle_channel_recv(job: ChannelJob) -> Result<ChannelResult, Error> {
    let job_id = job.job_id;
    touch_status(&format!("ENTERED_CHANNEL_{job_id}"), "channel")?;
    let payload = json!({ "reader": job.reader });
    let refs = extract_channel_refs(&payload);
    let (_, reader_ref) = refs
        .iter()
        .find(|(key, channel_ref)| {
            key == "reader" && matches!(channel_ref.direction, ChannelDirection::Read)
        })
        .ok_or_else(|| Error::Handler("missing reader channel ref".into()))?;
    let engine = env::var("III_URL").unwrap_or_else(|_| iii_sdk::DEFAULT_ENGINE_URL.to_owned());
    let reader = ChannelReader::new(&engine, reader_ref);
    let bytes = reader.read_all().await?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let dir = marker_dir()?;
    ensure_dir(&dir)?;
    let path = dir.join(format!("channel_{job_id}.sha256"));
    fs::write(&path, format!("{digest}\n"))
        .map_err(|error| Error::Handler(format!("write {}: {error}", path.display())))?;
    mark_completed(&format!("channel_{job_id}"))?;
    Ok(ChannelResult {
        bytes: bytes.len(),
        job_id,
        sha256: digest,
    })
}

fn register_qualify_functions(
    worker: &iii_sdk::IIIClient,
    namespace: &str,
) -> Result<(), Error> {
    worker.register_function(
        "qualify::import",
        RegisterFunction::new(|job: ImportJob| handle_import(&job)),
    );
    worker
        .register_trigger(RegisterTriggerInput::new(
            "durable:subscriber",
            "qualify::import",
            json!({
                "topic": "okf.qualify.import",
                "max_retries": 5,
                "backoff_ms": 200
            }),
        ))
        .map_err(|error| Error::Handler(format!("register import trigger: {error}")))?;

    worker.register_function(
        "qualify::fail",
        RegisterFunction::new(|job: FailJob| handle_fail(job)),
    );
    worker
        .register_trigger(RegisterTriggerInput::new(
            "durable:subscriber",
            "qualify::fail",
            json!({
                "topic": "okf.qualify.fail",
                "max_retries": 1,
                "backoff_ms": 100
            }),
        ))
        .map_err(|error| Error::Handler(format!("register fail trigger: {error}")))?;

    worker.register_function(
        "qualify::channel_recv",
        RegisterFunction::new_async(
            |job: ChannelJob| async move { handle_channel_recv(job).await },
        ),
    );
    worker
        .register_trigger(RegisterTriggerInput::new(
            "durable:subscriber",
            "qualify::channel_recv",
            json!({
                "topic": "okf.qualify.channel",
                "max_retries": 3,
                "backoff_ms": 200
            }),
        ))
        .map_err(|error| Error::Handler(format!("register channel trigger: {error}")))?;

    let publisher = worker.clone();
    let publish_namespace = namespace.to_owned();
    worker.register_function(
        "qualify::channel_send",
        RegisterFunction::new_async(move |request: ChannelSendRequest| {
            let publisher = publisher.clone();
            let publish_namespace = publish_namespace.clone();
            async move {
                let channel = create_channel(&publisher, None).await?;
                let reader_value = serde_json::to_value(&channel.reader_ref)
                    .map_err(|error| Error::Serde(error.to_string()))?;
                publisher
                    .trigger(
                        TriggerRequest {
                            function_id: "iii::durable::publish".into(),
                            payload: json!({
                                "topic": "okf.qualify.channel",
                                "data": {
                                    "job_id": request.job_id,
                                    "reader": reader_value
                                }
                            }),
                            action: None,
                            timeout_ms: Some(30_000),
                        }
                        .namespace(publish_namespace),
                    )
                    .await?;
                channel.writer.write(request.payload.as_bytes()).await?;
                channel.writer.close().await?;
                Ok(ChannelSendResult { published: true })
            }
        }),
    );
    Ok(())
}

fn main() -> Result<(), Error> {
    let url = env::var("III_URL").unwrap_or_else(|_| iii_sdk::DEFAULT_ENGINE_URL.to_owned());
    let namespace = env::var("III_NAMESPACE").unwrap_or_else(|_| "okf-qualify".to_owned());
    let control = control_dir()?;
    let markers = marker_dir()?;
    ensure_dir(&control)?;
    ensure_dir(&markers)?;

    let worker = register_worker(
        &url,
        InitOptions {
            namespace: Some(namespace.clone()),
            ..InitOptions::default()
        },
    );
    register_qualify_functions(&worker, &namespace)?;
    touch_status(READY_FILE, &format!("pid={}\n", process::id()))?;

    let stop = control.join("STOP");
    while !stop.exists() {
        thread::sleep(Duration::from_millis(100));
    }
    worker.shutdown();
    Ok(())
}
