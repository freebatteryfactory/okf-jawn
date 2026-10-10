//! The production `Converter`: docling in a memory-capped child process, one page window per
//! child (`converter-worker-memory-ceiling`).
//!
//! `DoclingConverter` writes a `protocol::Request` into the window's output directory, starts
//! the conversion child under `cap::spawn_capped`, waits for it under the hard time bound with
//! `cap::supervise`, and reads how it ended with `cap::classify`. A child that reported is read
//! back with `read_window`; one that did not is a failed window (the cap, the time bound or a
//! crash), whose pages the import then records as not converted. Dropping the future kills the
//! child. A missing model or native library is a worker fault, found before any child starts.
//!
//! `read_window` is pure over the child's files, so it is tested without models:
//! - the window's coverage lists exactly the pages it converted: a page missing from the
//!   export's `pages` (the time budget ran out before it, `PartialSuccess`, or the converter
//!   dropped it) is `not_converted`, whatever the status, so no unconverted page is shown as
//!   converted (SPEC section 5);
//! - the Markdown has each undecodable-glyph placeholder replaced by U+FFFD
//!   (`glyphs::scrub_placeholders`), so flagged text is never indexed as words, and each page
//!   with one is `partly_extracted`;
//! - items the converter left unlocated go through the location rule (`locate::locate_items`)
//!   against the window's text layer, and the counts become warnings;
//! - each Markdown line run with the converter's own provenance is `Direct` on its page; a run
//!   without one takes the location rule's result for its own item, which the child names by
//!   reference (`Inferred`, or `Unresolved` with the reason), so the rule's inferred locations
//!   reach the record, no run takes another item's location, and a preceding page marker never
//!   locates a run;
//! - tables pair the export's tables with the table line runs, in order, only when both
//!   counts agree;
//! - the outline carries the section-span rule (`outline::outline_of`).
//!
//! Construction limitations, stated rather than hidden: line locations are page-level (a line
//! run is not yet paired with its export item's box), and no page render or picture asset is
//! retained yet (`ConvertedDocument::assets` is empty).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use docling::InputFormat;
use okf_jawn_contract::common::PageRange;
use okf_jawn_contract::error::{ApiError, ErrorCode};
use okf_jawn_contract::extraction::{
    ConverterIdentity, ConverterIssue, ConverterPackage, ModelIdentity,
};
use okf_jawn_contract::source::{SourceLocation, SourceLocator, UnresolvedReason};
use okf_jawn_core::conversion::{
    Conversion, ConversionInput, ConversionStatus, ConvertedCell, ConvertedDocument,
    ConvertedTable, Converter, ConverterLimits, LineLocation, WindowCoverage,
};
use okf_jawn_core::ports::PortFuture;
use okf_jawn_core::storage::LocalSource;
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::cap::{ChildVerdict, classify, spawn_capped, supervise};
use crate::child::format_of;
use crate::export::{provenance, region, resolve};
use crate::glyphs::{scrub_placeholders, undecoded_glyphs};
use crate::locate::{Locations, locate_items, pages_by_item};
use crate::outline::outline_of;
use crate::protocol::{
    Block, EXPORT_FILE, MARKDOWN_FILE, REQUEST_FILE, RESULT_FILE, Reply, Request, TEXT_LAYER_FILE,
    Task, Unlocated,
};

/// How the production converter runs its child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConverterConfig {
    /// The conversion child's executable (`okf-jawn-convert`); configuration that defaults to
    /// beside the server executable (I4).
    pub child: PathBuf,
    /// Window size and memory cap.
    pub limits: ConverterLimits,
}

/// Docling in a capped child process.
#[derive(Debug)]
pub struct DoclingConverter {
    config: ConverterConfig,
    /// The hashed models and native libraries, read once.
    models: OnceCell<Vec<ModelIdentity>>,
}

/// The pinned conversion packages, generated from the lockfile.
const PACKAGES_JSON: &str = include_str!("../../../generated/converter/packages.json");

/// The default pages per window (owner, 2026-10-07), until part (b) measures it.
pub const DEFAULT_WINDOW_PAGES: u32 = 4;

/// How long past the window's time budget the supervisor waits before it kills the child.
/// `document_timeout` is checked only between pages, so the hard bound sits above it.
pub const HARD_BOUND_GRACE: Duration = Duration::from_secs(60);

/// The `model_inventory` stage of the pdfium library, which this build does not compile in.
const PDFIUM_STAGE: &str = "pdfium";

/// The hard time bound of a page count.
pub const PAGE_COUNT_TIME_LIMIT: Duration = Duration::from_secs(120);

/// The docling variables the child inherits: exactly those the asset resolution behind
/// `docling::model_inventory` reads (the model files, their int8 or fp32 choice and the
/// execution provider that sways it, the OCR language that picks its model), so the child
/// loads the files whose hashes the converter identity names. Every other `DOCLING_` variable
/// (the renderer, `DOCLING_RS_RENDERER` and its `DOCLING_PARSE_RENDER_LIB` native library,
/// thread counts, debug dumps, network fetches) is removed from the child's environment (review
/// N6): nothing the identity does not record changes what the child does.
pub const CHILD_INHERITED_DOCLING_ENV: [&str; 10] = [
    "DOCLING_RS_MODELS_DIR",
    "DOCLING_LAYOUT_ONNX",
    "DOCLING_TABLEFORMER_ENCODER",
    "DOCLING_TABLEFORMER_DECODER",
    "DOCLING_TABLEFORMER_BBOX",
    "DOCLING_OCR_REC_ONNX",
    "DOCLING_OCR_DICT",
    "DOCLING_RS_OCR_LANG",
    "DOCLING_RS_FP32",
    "DOCLING_RS_EP",
];

/// The prefix of the variables docling reads.
const DOCLING_ENV_PREFIX: &str = "DOCLING_";

impl DoclingConverter {
    /// A converter that runs `config.child` under `config.limits`.
    #[must_use]
    pub const fn new(config: ConverterConfig) -> Self {
        Self {
            config,
            models: OnceCell::const_new(),
        }
    }

    /// The converter identity of one window's conversion.
    async fn identity(&self, input: &ConversionInput) -> Result<ConverterIdentity, ApiError> {
        let packages = packages()?;
        // Only the PDF and image pipeline loads models and pdfium; another format's identity
        // names none, and its conversion does not need them present.
        let models = if matches!(
            format_of(&input.file_name),
            Some(InputFormat::Pdf | InputFormat::Image)
        ) {
            self.models
                .get_or_try_init(|| async {
                    tokio::task::spawn_blocking(hashed_models)
                        .await
                        .map_err(|error| fault(&format!("model hashing stopped: {error}")))?
                })
                .await?
                .clone()
        } else {
            Vec::new()
        };
        let version = packages
            .iter()
            .find(|package| package.name == "docling")
            .map(|package| package.version.clone())
            .ok_or_else(|| fault("the pinned packages do not name docling"))?;
        Ok(ConverterIdentity {
            name: "docling".to_owned(),
            version,
            packages,
            settings: input.settings.clone(),
            models,
            page_window: input
                .window
                .as_ref()
                .map(|_| self.config.limits.window_pages),
        })
    }

    /// Run one request in a capped child and return how it ended and its reply, if any.
    async fn run_child(
        &self,
        directory: &Path,
        request: &Request,
        time_limit: Duration,
    ) -> Result<(ChildVerdict, Option<Reply>, String), ApiError> {
        let request_path = directory.join(REQUEST_FILE);
        // Small files, read and written in place: the workspace tokio has no `fs` feature.
        std::fs::write(&request_path, request.encode()?)
            .map_err(|error| fault(&format!("the request could not be written: {error}")))?;
        let mut command = child_command(&self.config.child, std::env::vars_os());
        let _configured = command.arg(&request_path);
        let limit = self.config.limits.memory_limit_bytes;
        let child = spawn_capped(command, limit)?;
        let mut end = supervise(child, limit, time_limit).await?;
        let result = directory.join(RESULT_FILE);
        end.reported = result.try_exists().unwrap_or(false);
        let seconds = u32::try_from(time_limit.as_secs()).unwrap_or(u32::MAX);
        let verdict = classify(&end, limit, seconds);
        let reply = if verdict == ChildVerdict::Reported {
            let bytes = std::fs::read(&result)
                .map_err(|error| fault(&format!("the reply could not be read: {error}")))?;
            Some(Reply::decode(&bytes)?)
        } else {
            None
        };
        Ok((verdict, reply, end.stderr_tail))
    }

    /// Convert one window: identity, child, and the window read back.
    async fn convert_window(&self, input: ConversionInput) -> Result<Conversion, ApiError> {
        let converter = self.identity(&input).await?;
        let request = Request {
            task: Task::Convert,
            source: input.source.path.clone(),
            file_name: input.file_name.clone(),
            settings: input.settings.clone(),
            window: input.window.clone(),
            timeout: input.timeout,
        };
        let time_limit = input.timeout.saturating_add(HARD_BOUND_GRACE);
        let (verdict, reply, stderr) = self
            .run_child(&input.output_directory, &request, time_limit)
            .await?;
        let source_digest = input.source.object.digest.clone();
        let Some(reply) = reply else {
            let ChildVerdict::Failed(reason) = verdict else {
                return Err(fault("a child that reported left no reply"));
            };
            return Ok(Conversion {
                source_digest,
                converter,
                status: ConversionStatus::Failure(reason),
                document: None,
                coverage: None,
                issues: vec![ConverterIssue {
                    component_type: "worker".to_owned(),
                    module_name: "okf-jawn-convert".to_owned(),
                    error_message: last_line(&stderr),
                }],
            });
        };
        let produced = matches!(
            reply.status,
            ConversionStatus::Success | ConversionStatus::PartialSuccess
        );
        let (document, coverage) = if produced {
            let (document, coverage) =
                read_window(&input.output_directory, input.window.as_ref(), &reply)?;
            (Some(document), coverage)
        } else {
            (None, None)
        };
        Ok(Conversion {
            source_digest,
            converter,
            status: reply.status,
            document,
            coverage,
            issues: reply.issues,
        })
    }

    /// Count pages in a capped child.
    async fn count_pages(
        &self,
        source: &LocalSource,
        file_name: &str,
    ) -> Result<Option<u32>, ApiError> {
        let directory = tempfile::tempdir()
            .map_err(|error| fault(&format!("a page-count directory was refused: {error}")))?;
        let request = Request {
            task: Task::PageCount,
            source: source.path.clone(),
            file_name: file_name.to_owned(),
            settings: okf_jawn_contract::extraction::ConversionSettings::default(),
            window: None,
            timeout: PAGE_COUNT_TIME_LIMIT,
        };
        let (verdict, reply, stderr) = self
            .run_child(directory.path(), &request, PAGE_COUNT_TIME_LIMIT)
            .await?;
        match (verdict, reply) {
            // A count that failed (a damaged PDF) converts the document whole, where the
            // converter records why it cannot be read.
            (_, Some(reply)) => Ok(reply.page_count),
            (ChildVerdict::Reported | ChildVerdict::Failed(_), None) => Err(fault(&format!(
                "the page count child ended without a reply: {}",
                last_line(&stderr)
            ))),
        }
    }
}

impl Converter for DoclingConverter {
    fn page_count<'a>(
        &'a self,
        source: &'a LocalSource,
        file_name: &'a str,
    ) -> PortFuture<'a, Option<u32>> {
        Box::pin(self.count_pages(source, file_name))
    }

    fn convert(&self, input: ConversionInput) -> PortFuture<'_, Conversion> {
        Box::pin(self.convert_window(input))
    }

    fn limits(&self) -> ConverterLimits {
        self.config.limits
    }
}

/// The conversion child's command: `program`, with every `DOCLING_` variable of `environment`
/// (the supervisor's own) removed except those in `CHILD_INHERITED_DOCLING_ENV`.
#[must_use]
pub fn child_command(
    program: &Path,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    for (name, _value) in environment {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if upper.starts_with(DOCLING_ENV_PREFIX)
            && !CHILD_INHERITED_DOCLING_ENV.contains(&upper.as_str())
        {
            let _configured = command.env_remove(&name);
        }
    }
    command
}

/// Read back a window the child converted: the document and, for a paginated window, its
/// coverage.
///
/// # Errors
/// Returns `Internal` when a file the reply names is missing or not what it should be: a
/// child that reports success without its outputs is a worker fault.
pub fn read_window(
    directory: &Path,
    window: Option<&PageRange>,
    reply: &Reply,
) -> Result<(ConvertedDocument, Option<WindowCoverage>), ApiError> {
    let shown = read_text(&directory.join(MARKDOWN_FILE))?;
    let structured = directory.join(EXPORT_FILE);
    let export = read_json(&structured)?;
    let (markdown, _scrubbed) = scrub_placeholders(&shown);
    let mut warnings = Vec::new();
    let located = if window.is_some() {
        let text_layer = if reply.text_layer {
            Ok(read_json(&directory.join(TEXT_LAYER_FILE))?)
        } else {
            Err("the window's text layer could not be read")
        };
        let located = locate_items(&export, text_layer.as_ref().map_err(|error| *error));
        warnings.extend(located.warnings());
        Some(located)
    } else {
        None
    };
    let page_of = located.as_ref().map(pages_by_item).unwrap_or_default();
    let glyphs = undecoded_glyphs(&export, |item| page_of.get(item).copied());
    warnings.extend(glyphs.warning());
    let coverage = window.map(|window| {
        let present = exported_pages(&export);
        let partly: BTreeSet<u32> = glyphs
            .pages
            .keys()
            .copied()
            .filter(|page| *page >= window.start && *page <= window.end && present.contains(page))
            .collect();
        WindowCoverage {
            window: window.clone(),
            converted: ranges(
                (window.start..=window.end)
                    .filter(|page| present.contains(page) && !partly.contains(page)),
            ),
            partly_extracted: ranges(partly.into_iter()),
            not_converted: ranges(
                (window.start..=window.end).filter(|page| !present.contains(page)),
            ),
        }
    });
    let document = ConvertedDocument {
        outline: outline_of(&markdown),
        locations: line_locations(&reply.blocks, &export, located.as_ref()),
        tables: tables(&export, &reply.blocks),
        markdown,
        structured,
        assets: Vec::new(),
        warnings,
    };
    Ok((document, coverage))
}

/// The pinned packages, from the generated `packages.json`.
fn packages() -> Result<Vec<ConverterPackage>, ApiError> {
    serde_json::from_str(PACKAGES_JSON)
        .map_err(|error| fault(&format!("the pinned packages are not readable: {error}")))
}

/// Every model and native library docling resolves, hashed. A missing one is a worker fault.
fn hashed_models() -> Result<Vec<ModelIdentity>, ApiError> {
    docling::model_inventory()
        .into_iter()
        // This build enables docling's `pdf` feature without `pdfium`: pages are rendered and
        // counted by the pure-Rust path, so the pdfium entry names a library nothing loads.
        .filter(|entry| entry.stage != PDFIUM_STAGE)
        .map(|entry| {
            if !entry.found {
                return Err(fault(&format!(
                    "the conversion asset for {} is missing at {}",
                    entry.stage, entry.path
                )));
            }
            Ok(ModelIdentity {
                stage: entry.stage.to_owned(),
                file: Path::new(&entry.path)
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                size: entry.bytes.to_string(),
                sha256: file_digest(Path::new(&entry.path))?,
            })
        })
        .collect()
}

/// SHA-256 of a file, read in pieces.
fn file_digest(path: &Path) -> Result<okf_jawn_contract::identity::Digest, ApiError> {
    use sha2::Digest as _;
    let mut file = std::fs::File::open(path)
        .map_err(|error| fault(&format!("{} could not be opened: {error}", path.display())))?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| fault(&format!("{} could not be read: {error}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(buffer.get(..read).unwrap_or_default());
    }
    let hash = hasher.finalize();
    okf_jawn_contract::identity::Digest::try_from(format!("{hash:x}"))
        .map_err(|error| fault(&format!("a SHA-256 was not a digest: {error}")))
}

/// Where each line run is, and which source said so.
///
/// A run with the converter's own provenance is `Direct` on its page. A run on a page without
/// one takes the location rule's result for its own item, the one the child named by
/// reference: `Inferred` when the text layer found it, `Unresolved` with the rule's reason
/// when not. Another item never lends a run its location, however many items share the run's
/// text. A run whose own item cannot be confirmed is `Unresolved` as not located by the
/// converter. A run in a document with no pages has none.
fn line_locations(
    blocks: &[Block],
    export: &Value,
    located: Option<&Locations>,
) -> Vec<LineLocation> {
    blocks
        .iter()
        .filter_map(|block| {
            let location = match (block.page, block.unlocated.as_ref()) {
                (Some(page_no), _) => SourceLocation::Direct {
                    locator: SourceLocator::Page { page_no },
                },
                (None, Some(unlocated)) => {
                    own_location(unlocated, export, located).unwrap_or(SourceLocation::Unresolved {
                        reason: UnresolvedReason::NotLocatedByConverter,
                    })
                }
                (None, None) => return None,
            };
            Some(LineLocation {
                lines: block.lines.clone(),
                location,
            })
        })
        .collect()
}

/// The location rule's result for a run's own item, when the export confirms the item the
/// child named: it exists, hangs directly off the body (never a caption or another item's
/// child), has no provenance of its own, and has the text the child saw.
fn own_location(
    unlocated: &Unlocated,
    export: &Value,
    located: Option<&Locations>,
) -> Option<SourceLocation> {
    let reference = unlocated.item.as_deref()?;
    let item = resolve(export, reference)?;
    let parent = item.get("parent")?.get("$ref")?.as_str()?;
    let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
    let confirmed = parent == "#/body" && provenance(item).is_empty() && text == unlocated.text;
    confirmed
        .then(|| located?.location_of(reference).cloned())
        .flatten()
}

/// The export's tables, paired in order with the table line runs; none when the counts differ.
fn tables(export: &Value, blocks: &[Block]) -> Vec<ConvertedTable> {
    let items: Vec<&Value> = export
        .get("tables")
        .and_then(Value::as_array)
        .map(|tables| tables.iter().collect())
        .unwrap_or_default();
    let runs: Vec<&Block> = blocks.iter().filter(|block| block.table).collect();
    if items.len() != runs.len() {
        return Vec::new();
    }
    items
        .into_iter()
        .zip(runs)
        .map(|(item, run)| table(export, item, run))
        .collect()
}

/// One table of the export, on the lines of its run.
fn table(export: &Value, item: &Value, run: &Block) -> ConvertedTable {
    let data = item.get("data");
    let number = |value: Option<&Value>| {
        value
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
            .unwrap_or(0)
    };
    let location = item
        .get("prov")
        .and_then(Value::as_array)
        .and_then(|prov| prov.first())
        .and_then(|first| region(export, first))
        .map(|region| SourceLocation::Direct {
            locator: SourceLocator::Region { region },
        })
        .or_else(|| {
            run.page.map(|page_no| SourceLocation::Direct {
                locator: SourceLocator::Page { page_no },
            })
        })
        .unwrap_or(SourceLocation::Unresolved {
            reason: UnresolvedReason::NotLocatedByConverter,
        });
    let cells = data
        .and_then(|data| data.get("table_cells"))
        .and_then(Value::as_array)
        .map(|cells| {
            cells
                .iter()
                .map(|cell| {
                    let start_row = number(cell.get("start_row_offset_idx"));
                    let start_column = number(cell.get("start_col_offset_idx"));
                    ConvertedCell {
                        row: start_row,
                        column: start_column,
                        row_span: number(cell.get("end_row_offset_idx"))
                            .saturating_sub(start_row)
                            .max(1),
                        column_span: number(cell.get("end_col_offset_idx"))
                            .saturating_sub(start_column)
                            .max(1),
                        text: cell
                            .get("text")
                            .and_then(Value::as_str)
                            .map(|text| scrub_placeholders(text).0)
                            .unwrap_or_default(),
                        column_header: flag(cell.get("column_header")),
                        row_header: flag(cell.get("row_header")),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    ConvertedTable {
        lines: run.lines.clone(),
        location,
        num_rows: number(data.and_then(|data| data.get("num_rows"))),
        num_cols: number(data.and_then(|data| data.get("num_cols"))),
        cells,
    }
}

/// The pages the export holds (its `pages` map): the pages the conversion finished. In docling
/// 2.3.0 a `PartialSuccess` is a document whose budget ran out, and only the pages already
/// finished become the document, so a page of the window missing here was not converted.
fn exported_pages(export: &Value) -> BTreeSet<u32> {
    export
        .get("pages")
        .and_then(Value::as_object)
        .map(|pages| {
            pages
                .values()
                .filter_map(|page| page.get("page_no").and_then(Value::as_u64))
                .filter_map(|page| u32::try_from(page).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Ascending pages as ranges.
fn ranges(pages: impl Iterator<Item = u32>) -> Vec<PageRange> {
    let mut ranges: Vec<PageRange> = Vec::new();
    for page in pages {
        match ranges.last_mut() {
            Some(last) if last.end.checked_add(1) == Some(page) => last.end = page,
            Some(_) | None => ranges.push(PageRange {
                start: page,
                end: page,
            }),
        }
    }
    ranges
}

/// A JSON boolean, `false` when absent.
fn flag(value: Option<&Value>) -> bool {
    value.and_then(Value::as_bool).unwrap_or(false)
}

/// The last non-empty line of a child's stderr, or a note that it said nothing.
fn last_line(stderr: &str) -> String {
    stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map_or_else(
            || "the conversion child ended without a result and said nothing".to_owned(),
            str::to_owned,
        )
}

/// A text file the child wrote.
fn read_text(path: &Path) -> Result<String, ApiError> {
    std::fs::read_to_string(path)
        .map_err(|error| fault(&format!("{} could not be read: {error}", path.display())))
}

/// A JSON file the child wrote.
fn read_json(path: &Path) -> Result<Value, ApiError> {
    serde_json::from_str(&read_text(path)?)
        .map_err(|error| fault(&format!("{} is not JSON: {error}", path.display())))
}

/// A worker fault.
fn fault(message: &str) -> ApiError {
    ApiError::new(ErrorCode::Internal, message)
}
