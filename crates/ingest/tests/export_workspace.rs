//! The portable export job (`JobSpec::ExportWorkspace`) over the handler's fakes: one OKF bundle
//! in a zip, holding the revision's committed files as stored (an item with a current review of
//! the whole item written with OKF `verified`), the root index's `okf_version`, the originals and
//! derivatives the cards name under `references/`, and `history.bundle` when asked; and an
//! archive that opens as an OKF bundle without the application (ingest-m2a-design.md
//! section 1).
#![cfg(feature = "runtime")]

#[path = "../../../tests/support/check.rs"]
mod check;
#[path = "support/ports.rs"]
pub mod ports;
#[path = "support/records.rs"]
pub mod records;

mod export_workspace {
    use std::collections::BTreeMap;
    use std::io::{Cursor, Read as _};
    use std::sync::atomic::AtomicUsize;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use okf_core::yaml::{Mapping, Value as YamlValue};
    use okf_core::{Bundle, Document, Frontmatter};
    use okf_jawn_contract::access::AccessRoute;
    use okf_jawn_contract::error::ErrorCode;
    use okf_jawn_contract::error::ErrorDetail;
    use okf_jawn_contract::extraction::{ConversionOutcome, ConversionSettings, ConverterIdentity};
    use okf_jawn_contract::identity::{
        Digest, ItemId, JobId, MutationId, PurgeId, ReviewId, Revision, TenantId, Timestamp,
        WorkspaceId, WorkspacePath,
    };
    use okf_jawn_contract::import::JobState;
    use okf_jawn_contract::item::ItemKind;
    use okf_jawn_contract::read::{AssetRole, Selection};
    use okf_jawn_contract::review::{Review, ReviewCoverage};
    use okf_jawn_contract::source::{SourceLocation, SourceLocator, SourceReference};
    use okf_jawn_contract::workspace::ArtifactKind;
    use okf_jawn_core::conformance::OkfConformance;
    use okf_jawn_core::conversion::{
        ConversionRecord, ConverterLimits, RetainedAsset, WindowExport,
    };
    use okf_jawn_core::items::SourceHeader;
    use okf_jawn_core::jobs::{
        ArtifactRecord, JobHandler, JobScope, JobSpec, NewArtifact, NewJob, RecordStore,
        RevisionMapping,
    };
    use okf_jawn_core::portable::content_digest;
    use okf_jawn_core::storage::{
        CandidateCheck, CommittedFile, CommittedItem, ObjectInfo, Provenance, StorageScope,
    };
    use okf_jawn_ingest::handler::{
        ARCHIVE_MEDIA_TYPE, DRAIN_BOUND, HandlerLimits, HandlerPorts, IngestHandler,
    };
    use okf_jawn_ingest::record::sha256_digest;
    use okf_jawn_ingest::runtime::run_one;
    use serde_json::{Value, json};
    use zip::ZipArchive;

    use crate::check::{TestResult, some};
    use crate::ports::{
        AcceptAll, FakeBackups, FakeBlobs, FakeConverter, FakePurger, FakeSearch, FakeUploads,
        FakeVersions,
    };
    use crate::records::{FakeEvents, FakeRecords, job_id};

    type Built<T> = Result<T, Box<dyn std::error::Error>>;

    /// One entry of an archive as a test reads it: name, bytes, and whether it is stored with
    /// the fixed time and the Unix mode 0644.
    type Entry = (String, Vec<u8>, bool);

    struct World {
        records: Arc<FakeRecords>,
        versions: Arc<FakeVersions>,
        blobs: Arc<FakeBlobs>,
        events: Arc<FakeEvents>,
        handler: IngestHandler,
    }

    /// The workspace tree of most tests, with the objects its card names.
    struct Fixture {
        files: Vec<CommittedFile>,
        original: ObjectInfo,
        record: ObjectInfo,
        exports: Vec<ObjectInfo>,
        asset: ObjectInfo,
        note: ItemId,
        note_digest: Digest,
    }

    const WORKSPACE: &str = "00000000-0000-0000-0000-0000000000aa";
    const NOTE_ID: &str = "7d3c1f0e-8a52-4c1b-9e7a-2b6f0c9d4e11";
    const CARD_ID: &str = "22222222-3333-4444-8555-666666666666";
    const HISTORY_FILE: &str = "history.bundle";
    const ROOT_INDEX_TEXT: &str =
        "# Subdirectories\n\n* [notes](notes/index.md)\n* [reports](reports/index.md)\n";
    const LOG_TEXT: &str = "# Directory Update Log\n\n## 2026-10-09\n* **change**: seed\n";

    fn base() -> Built<Revision> {
        Ok(Revision::try_from("1".repeat(40))?)
    }

    fn tenant() -> Built<TenantId> {
        Ok(TenantId::try_from("tenant-a".to_owned())?)
    }

    fn storage_scope() -> Built<StorageScope> {
        Ok(StorageScope {
            tenant_id: tenant()?,
            workspace_id: serde_json::from_value::<WorkspaceId>(json!(WORKSPACE))?,
        })
    }

    fn world() -> Built<World> {
        let records = Arc::new(FakeRecords::default());
        let events = Arc::new(FakeEvents::default());
        let versions = Arc::new(FakeVersions::at(base()?));
        let blobs = Arc::new(FakeBlobs::new()?);
        let handler = IngestHandler::new(HandlerPorts {
            records: Arc::clone(&records) as Arc<dyn RecordStore>,
            versions: Arc::clone(&versions) as Arc<dyn okf_jawn_core::storage::VersionStore>,
            search: Arc::new(FakeSearch::default()),
            events: Arc::clone(&events) as Arc<dyn okf_jawn_core::events::EventLog>,
            backups: Arc::new(FakeBackups {
                archive: ObjectInfo {
                    digest: Digest::try_from("ab".repeat(32))?,
                    size: 2048,
                },
                writes: AtomicUsize::new(0),
                restored: Mutex::new(Vec::new()),
                versions: Arc::clone(&versions),
                restore_hold: Mutex::new(None),
                write_hold: Mutex::new(None),
            }),
            purger: Arc::new(FakePurger {
                report: serde_json::from_value(json!({
                    "objects_removed": "0", "objects_kept_shared": "0", "backups_removed": 0,
                    "backups_rewritten": 0, "exports_removed": 0, "proposals_removed": 0,
                    "drafts_removed": 0, "revisions_invalidated": 0, "citations_remapped": 0,
                    "citations_invalidated": 0, "views_invalidated": 0,
                }))?,
                calls: AtomicUsize::new(0),
                hold: Mutex::new(None),
                failure: Mutex::new(None),
            }),
            uploads: Arc::new(FakeUploads::default()),
            blobs: Arc::clone(&blobs) as Arc<dyn okf_jawn_core::storage::BlobStore>,
            converter: Arc::new(FakeConverter {
                pages: None,
                capped: Vec::new(),
                limits: ConverterLimits {
                    window_pages: 4,
                    memory_limit_bytes: 1 << 30,
                },
                windows: Mutex::new(Vec::new()),
                on_window: Mutex::new(None),
                on_page_count: Mutex::new(None),
                dropped: Arc::new(AtomicUsize::new(0)),
                count_failed: Mutex::new(None),
                runs: AtomicUsize::new(0),
                pictures: std::sync::atomic::AtomicBool::new(false),
            }),
            check: Arc::new(AcceptAll) as Arc<dyn CandidateCheck>,
            limits: HandlerLimits {
                heartbeat: Duration::from_secs(60),
                window_timeout: Duration::from_secs(600),
                drain: DRAIN_BOUND,
            },
        });
        Ok(World {
            records,
            versions,
            blobs,
            events,
            handler,
        })
    }

    fn initiator() -> Provenance {
        Provenance {
            subject: "owner".to_owned(),
            route: AccessRoute::LocalOwner,
            client_id: None,
        }
    }

    fn mutation(number: u128) -> Built<MutationId> {
        Ok(serde_json::from_value(json!(format!(
            "00000000-0000-0000-0000-{number:012}"
        )))?)
    }

    /// An export job of the revision `base`, numbered `number`.
    fn export_job(world: &World, number: u128, include_history: bool) -> Built<JobId> {
        let id = job_id(number)?;
        world.records.insert(
            id,
            JobScope::Workspace(storage_scope()?),
            NewJob {
                mutation_id: mutation(number)?,
                initiator: initiator(),
                spec: JobSpec::ExportWorkspace {
                    revision: base()?,
                    include_history,
                },
            },
        )?;
        Ok(id)
    }

    async fn run(world: &World, id: JobId) -> Built<bool> {
        Ok(run_one(
            world.records.as_ref(),
            world.events.as_ref(),
            &world.handler as &dyn JobHandler,
            id,
        )
        .await?)
    }

    /// A JSON value as okf-core's YAML value, integers kept as integers.
    fn yaml(value: &Value) -> YamlValue {
        match value {
            Value::Null => YamlValue::Null,
            Value::Bool(flag) => YamlValue::Bool(*flag),
            Value::Number(number) => number
                .as_i64()
                .map_or_else(|| YamlValue::String(number.to_string()), YamlValue::Int),
            Value::String(text) => YamlValue::String(text.clone()),
            Value::Array(items) => YamlValue::Sequence(items.iter().map(yaml).collect()),
            Value::Object(map) => {
                let mut mapping = Mapping::new();
                for (key, child) in map {
                    mapping.insert(key.clone(), yaml(child));
                }
                YamlValue::Mapping(mapping)
            }
        }
    }

    /// An item file as storage writes one: the frontmatter in okf-core's preferred key order.
    fn item_text(pairs: &[(&str, Value)], body: &str) -> String {
        let mut frontmatter = Frontmatter::new();
        for (key, value) in pairs {
            frontmatter.set(*key, yaml(value));
        }
        frontmatter.reorder_preferred();
        Document::new(frontmatter, body).serialize()
    }

    /// The content digest `show` would answer for an item file: over its body and its
    /// frontmatter read as storage reads it.
    fn digest_of(text: &str) -> Built<Digest> {
        let document = Document::parse(text)?;
        let properties: BTreeMap<String, Value> = document
            .frontmatter
            .iter()
            .filter_map(|(key, value)| {
                key.as_str()
                    .map(|key| (key.to_owned(), fixture_json(value)))
            })
            .collect();
        Ok(content_digest(&document.body, &properties)?)
    }

    /// Decode only fixture inputs as storage's YAML projection does; no export-writer result
    /// is computed here. C3 deliberately does not import the not-yet-built C2 `json_of` API.
    fn fixture_json(value: &YamlValue) -> Value {
        match value {
            YamlValue::Null => Value::Null,
            YamlValue::Bool(value) => Value::Bool(*value),
            YamlValue::Int(value) => Value::from(*value),
            YamlValue::Float(value) => {
                serde_json::Number::from_f64(*value).map_or(Value::Null, Value::Number)
            }
            YamlValue::String(value) => Value::String(value.clone()),
            YamlValue::Sequence(values) => Value::Array(values.iter().map(fixture_json).collect()),
            YamlValue::Mapping(values) => Value::Object(
                values
                    .iter()
                    .filter_map(|(key, value)| {
                        key.as_display_string()
                            .map(|key| (key, fixture_json(value)))
                    })
                    .collect(),
            ),
        }
    }

    /// A committed item file at `path` holding `text`.
    fn item_file(
        path: &str,
        text: &str,
        id: &str,
        kind: ItemKind,
        source: Option<SourceHeader>,
        record: Option<Digest>,
    ) -> Built<CommittedFile> {
        Ok(CommittedFile {
            path: path.to_owned(),
            bytes: text.as_bytes().to_vec(),
            item: Some(CommittedItem {
                item_id: serde_json::from_value(json!(id))?,
                kind,
                content_digest: digest_of(text)?,
                source,
                record,
            }),
        })
    }

    /// A committed file that holds no item.
    fn plain_file(path: &str, bytes: &[u8]) -> CommittedFile {
        CommittedFile {
            path: path.to_owned(),
            bytes: bytes.to_vec(),
            item: None,
        }
    }

    /// A conversion record naming two window exports and one picture.
    fn record_of(exports: &[ObjectInfo], asset: &ObjectInfo) -> Built<ConversionRecord> {
        Ok(ConversionRecord {
            converter: ConverterIdentity {
                name: "docling".to_owned(),
                version: "2.3.0".to_owned(),
                packages: Vec::new(),
                settings: ConversionSettings::default(),
                models: Vec::new(),
                page_window: Some(4),
            },
            outcome: ConversionOutcome::Completed,
            page_count: Some(8),
            structured: exports
                .iter()
                .map(|export| WindowExport {
                    window: None,
                    digest: export.digest.clone(),
                    lines: None,
                })
                .collect(),
            markdown: sha256_digest(b"Converted text.\n")?,
            locations: Vec::new(),
            outline: Vec::new(),
            tables: Vec::new(),
            assets: vec![RetainedAsset {
                digest: asset.digest.clone(),
                media_type: "image/png".to_owned(),
                role: AssetRole::Picture,
                location: SourceLocation::Direct {
                    locator: SourceLocator::Page { page_no: 1 },
                },
                pixel_size: None,
                caption: None,
            }],
            warnings: Vec::new(),
        })
    }

    /// A note (archived, with an extension), a source card naming an original and a record,
    /// the folder indexes, the change log and the card's appearance under `.okf/`.
    fn fixture(world: &World) -> Built<Fixture> {
        fixture_named(world, "report.pdf")
    }

    fn fixture_named(world: &World, original_name: &str) -> Built<Fixture> {
        let original = world.blobs.insert(b"%PDF-1.7 the original report")?;
        let exports = vec![
            world.blobs.insert(b"{\"window\": 1}")?,
            world.blobs.insert(b"{\"window\": 2}")?,
        ];
        let asset = world.blobs.insert(b"\x89PNG a picture")?;
        let record = world
            .blobs
            .insert(&serde_json::to_vec(&record_of(&exports, &asset)?)?)?;
        let note_text = item_text(
            &[
                ("type", json!("Note")),
                ("title", json!("Plan")),
                ("x_team", json!(7)),
                (
                    "okf_jawn",
                    json!({ "item_id": NOTE_ID, "kind": "note", "archived": true }),
                ),
            ],
            "# Plan\n\nShip it.\n",
        );
        let card_text = item_text(
            &[
                ("type", json!("source")),
                ("title", json!(original_name)),
                (
                    "okf_jawn",
                    json!({
                        "item_id": CARD_ID,
                        "kind": "source",
                        "source": { "original_name": original_name, "digest": original.digest },
                    }),
                ),
            ],
            "Converted text.\n",
        );
        let files = vec![
            // Given out of order: the store answers in path order (the fake sorts).
            item_file(
                "reports/report-pdf.md",
                &card_text,
                CARD_ID,
                ItemKind::Source,
                Some(SourceHeader {
                    original_name: original_name.to_owned(),
                    digest: original.digest.clone(),
                }),
                Some(record.digest.clone()),
            )?,
            plain_file("index.md", ROOT_INDEX_TEXT.as_bytes()),
            plain_file("log.md", LOG_TEXT.as_bytes()),
            item_file(
                "notes/a.md",
                &note_text,
                NOTE_ID,
                ItemKind::Note,
                None,
                None,
            )?,
            plain_file("notes/index.md", b"# Note\n\n* [Plan](a.md)\n"),
            plain_file(
                "reports/index.md",
                b"# source\n\n* [report.pdf](report-pdf.md)\n",
            ),
            plain_file(
                &format!(".okf/sources/{CARD_ID}.json"),
                format!("{{\"object\": \"{}\"}}", original.digest.as_str()).as_bytes(),
            ),
        ];
        Ok(Fixture {
            files,
            original,
            record,
            exports,
            asset,
            note: serde_json::from_value(json!(NOTE_ID))?,
            note_digest: digest_of(&note_text)?,
        })
    }

    /// The archive the job's artifact names, and the artifact's digest.
    fn archive_of(world: &World, id: JobId) -> Built<(Vec<u8>, Digest)> {
        let entry = world.records.entry(id)?;
        let completion = some(entry.completions.first(), "the completion")?;
        let artifacts = world.records.artifacts()?;
        let artifact = some(artifacts.first(), "the artifact")?;
        assert_eq!(completion.artifact, Some(artifact.id));
        assert_eq!(
            completion.outputs,
            std::slice::from_ref(&artifact.object.digest)
        );
        assert_eq!(
            (
                artifact.kind,
                artifact.media_type.as_str(),
                artifact.created_by_job
            ),
            (ArtifactKind::Export, ARCHIVE_MEDIA_TYPE, id)
        );
        Ok((
            world.blobs.bytes(&artifact.object.digest)?,
            artifact.object.digest.clone(),
        ))
    }

    /// Every entry of an archive, in its order.
    fn entries(bytes: &[u8]) -> Built<Vec<Entry>> {
        let mut archive = ZipArchive::new(Cursor::new(bytes.to_vec()))?;
        let mut found = Vec::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index)?;
            let fixed = file.compression() == zip::CompressionMethod::Stored
                && file.last_modified() == Some(zip::DateTime::DEFAULT)
                && file.unix_mode().map(|mode| mode & 0o777) == Some(0o644);
            let mut content = Vec::new();
            file.read_to_end(&mut content)?;
            found.push((file.name().to_owned(), content, fixed));
        }
        Ok(found)
    }

    fn names(entries: &[Entry]) -> Vec<String> {
        entries.iter().map(|(name, _, _)| name.clone()).collect()
    }

    fn bytes_at<'e>(entries: &'e [Entry], name: &str) -> Built<&'e [u8]> {
        Ok(
            some(entries.iter().find(|(entry, _, _)| entry == name), name)?
                .1
                .as_slice(),
        )
    }

    /// The export-owned name of a derivative.
    fn derived(digest: &Digest, extension: &str) -> String {
        format!("references/derived/{}.{extension}", digest.as_str())
    }

    #[tokio::test]
    async fn e1_e10_an_export_holds_the_tree_and_references_in_one_global_path_order() -> TestResult
    {
        let world = world()?;
        let fixture = fixture(&world)?;
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, fixture.files.clone())?;
        let id = export_job(&world, 1, false)?;
        assert!(run(&world, id).await?);
        assert_eq!(world.records.entry(id)?.job.state, JobState::Succeeded);
        let (archive, _) = archive_of(&world, id)?;
        let found = entries(&archive)?;

        let mut derivatives = vec![
            (fixture.record.digest.clone(), "json"),
            (fixture.asset.digest.clone(), "png"),
        ];
        derivatives.extend(
            fixture
                .exports
                .iter()
                .map(|export| (export.digest.clone(), "json")),
        );
        derivatives.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
        let mut expected = vec![
            format!(".okf/sources/{CARD_ID}.json"),
            "index.md".to_owned(),
            "log.md".to_owned(),
            "notes/a.md".to_owned(),
            "notes/index.md".to_owned(),
            "reports/index.md".to_owned(),
            "reports/report-pdf.md".to_owned(),
            format!(
                "references/originals/{}/report.pdf",
                fixture.original.digest.as_str()
            ),
        ];
        expected.extend(
            derivatives
                .iter()
                .map(|(digest, extension)| derived(digest, extension)),
        );
        // E1 is one bytewise order across committed files and export-owned paths. In
        // particular references precede reports; the old draft's grouped order is not ruled.
        expected.sort();
        assert_eq!(names(&found), expected);

        // Every committed file but the root index as stored: no review, so no `verified`; the
        // archived note's header kept as committed.
        for file in &fixture.files {
            if file.path != "index.md" {
                assert_eq!(
                    bytes_at(&found, &file.path)?,
                    file.bytes.as_slice(),
                    "{}",
                    file.path
                );
            }
        }
        // The root index declares the OKF version and keeps its listing.
        assert_eq!(
            String::from_utf8(bytes_at(&found, "index.md")?.to_vec())?,
            format!("---\nokf_version: \"0.2\"\n---\n\n{ROOT_INDEX_TEXT}")
        );
        // Each object as the store holds it.
        assert_eq!(
            bytes_at(
                &found,
                &format!(
                    "references/originals/{}/report.pdf",
                    fixture.original.digest.as_str()
                )
            )?,
            world.blobs.bytes(&fixture.original.digest)?.as_slice()
        );
        for (digest, extension) in &derivatives {
            assert_eq!(
                bytes_at(&found, &derived(digest, extension))?,
                world.blobs.bytes(digest)?.as_slice()
            );
        }
        assert_eq!(
            world.versions.histories()?,
            [] as [(Revision, MutationId); 0]
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_original_named_by_two_cards_is_written_once_under_the_first_cards_name()
    -> TestResult {
        let world = world()?;
        let fixture = fixture(&world)?;
        let mut files = fixture.files.clone();
        let second = item_text(
            &[
                ("type", json!("source")),
                ("title", json!("Report (1).pdf")),
            ],
            "Converted text.\n",
        );
        files.push(item_file(
            "z/report-1-pdf.md",
            &second,
            "33333333-3333-4444-8555-666666666666",
            ItemKind::Source,
            Some(SourceHeader {
                original_name: "Report (1).pdf".to_owned(),
                digest: fixture.original.digest.clone(),
            }),
            None,
        )?);
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, files)?;
        let id = export_job(&world, 2, false)?;
        assert!(run(&world, id).await?);
        let found = entries(&archive_of(&world, id)?.0)?;
        let originals: Vec<String> = names(&found)
            .into_iter()
            .filter(|name| name.starts_with("references/originals/"))
            .collect();
        assert_eq!(
            originals,
            [format!(
                "references/originals/{}/report.pdf",
                fixture.original.digest.as_str()
            )]
        );
        Ok(())
    }

    #[tokio::test]
    async fn e3_e4_original_names_are_portable_and_not_accidental_concepts() -> TestResult {
        let long = format!("{}.pdf", "q".repeat(300));
        let long_stem = "x".repeat(300);
        for (original, written) in [
            ("report.pdf", "report.pdf"),
            ("Report (1).pdf", "Report (1).pdf"),
            ("notes.md", "notes.markdown"),
            ("README", "README"),
            ("a.b.MD", "a.b.MD"),
            ("con.pdf", "original.pdf"),
            ("report.", "original"),
            ("a:b.pdf", "original.pdf"),
            ("x/notes.md", "original.markdown"),
            ("bad:name.TIFF", "original.tiff"),
            ("bad:name.tar-gz", "original"),
            (long.as_str(), "original.pdf"),
            (long_stem.as_str(), "original"),
            ("cafe\u{301}.pdf", "caf\u{e9}.pdf"),
        ] {
            let world = world()?;
            let fixture = fixture_named(&world, original)?;
            world
                .versions
                .set_tree(&storage_scope()?, &base()?, fixture.files)?;
            let id = export_job(&world, 4, false)?;
            assert!(run(&world, id).await?);
            let found = entries(&archive_of(&world, id)?.0)?;
            let originals: Vec<_> = names(&found)
                .into_iter()
                .filter(|name| name.starts_with("references/originals/"))
                .collect();
            assert_eq!(
                originals,
                [format!(
                    "references/originals/{}/{written}",
                    fixture.original.digest.as_str()
                )],
                "{original}"
            );
            assert_eq!(
                bytes_at(&found, some(originals.first(), "the original entry")?)?,
                b"%PDF-1.7 the original report"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn history_is_written_only_when_the_job_asks_for_it() -> TestResult {
        // Each job in a world of its own, so its artifact is the first one recorded there.
        for include_history in [false, true] {
            let world = world()?;
            let fixture = fixture(&world)?;
            world
                .versions
                .set_tree(&storage_scope()?, &base()?, fixture.files)?;
            let history = world.blobs.insert(b"# v2 git bundle\nthe history")?;
            world.versions.set_history(history)?;
            let id = export_job(&world, 3, include_history)?;
            assert!(run(&world, id).await?);
            let found = entries(&archive_of(&world, id)?.0)?;
            if include_history {
                assert_eq!(names(&found).get(1).map(String::as_str), Some(HISTORY_FILE));
                assert_eq!(
                    bytes_at(&found, HISTORY_FILE)?,
                    b"# v2 git bundle\nthe history"
                );
                assert_eq!(world.versions.histories()?, [(base()?, mutation(3)?)]);
            } else {
                assert!(!names(&found).contains(&HISTORY_FILE.to_owned()));
                assert_eq!(
                    world.versions.histories()?,
                    [] as [(Revision, MutationId); 0]
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn the_root_index_declares_the_okf_version_once() -> TestResult {
        // A root index that declares a version already is kept as stored.
        let world = world()?;
        let fixture = fixture(&world)?;
        let declared = "---\nokf_version: \"0.1\"\n---\n\n# Subdirectories\n";
        let mut files: Vec<CommittedFile> = fixture
            .files
            .iter()
            .filter(|file| file.path != "index.md")
            .cloned()
            .collect();
        files.push(plain_file("index.md", declared.as_bytes()));
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, files)?;
        let id = export_job(&world, 5, false)?;
        assert!(run(&world, id).await?);
        assert_eq!(
            bytes_at(&entries(&archive_of(&world, id)?.0)?, "index.md")?,
            declared.as_bytes()
        );

        // A revision with no root index gets one that declares the version and lists nothing,
        // in global path order with the committed files.
        let world = self::world()?;
        let fixture = self::fixture(&world)?;
        let files: Vec<CommittedFile> = fixture
            .files
            .iter()
            .filter(|file| file.path != "index.md")
            .cloned()
            .collect();
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, files)?;
        let id = export_job(&world, 6, false)?;
        assert!(run(&world, id).await?);
        let found = entries(&archive_of(&world, id)?.0)?;
        assert_eq!(
            bytes_at(&found, "index.md")?,
            b"---\nokf_version: \"0.2\"\n---\n\n\n"
        );
        assert_eq!(
            names(&found).get(1).map(String::as_str),
            Some("index.md"),
            "after .okf and before log.md, not after the committed tree"
        );
        Ok(())
    }

    #[tokio::test]
    async fn nothing_but_the_revisions_tree_and_what_it_names_reaches_the_archive() -> TestResult {
        let world = world()?;
        let fixture = fixture(&world)?;
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, fixture.files.clone())?;
        let marker = "MARKER-7f3a9c";
        // A proposal's candidate tree, another revision, and an object no card names.
        let candidate = Revision::try_from("2".repeat(40))?;
        world.versions.set_candidate_tree(
            &storage_scope()?,
            &candidate,
            vec![plain_file("notes/proposed.md", marker.as_bytes())],
        )?;
        let other_scope = StorageScope {
            tenant_id: tenant()?,
            workspace_id: serde_json::from_value(json!("00000000-0000-0000-0000-0000000000bb"))?,
        };
        world.versions.set_tree(
            &other_scope,
            &base()?,
            vec![plain_file("notes/foreign.md", marker.as_bytes())],
        )?;
        let _loose = world
            .blobs
            .insert(format!("an object of {marker}").as_bytes())?;
        let id = export_job(&world, 7, false)?;
        assert!(run(&world, id).await?);
        let (archive, _) = archive_of(&world, id)?;
        assert!(
            !archive
                .windows(marker.len())
                .any(|window| window == marker.as_bytes())
        );
        let expected_scope = storage_scope()?;
        assert!(
            world
                .versions
                .file_scopes
                .lock()
                .map_err(|_| "file scopes lock poisoned")?
                .iter()
                .all(|scope| scope == &expected_scope)
        );
        // HandlerPorts contains neither DraftStore nor ProposalStore/comment access. This
        // proves its candidate/foreign/unreferenced-object boundaries; real private-store
        // exclusion is still checked by the independent real-binary Wave3 journey.
        Ok(())
    }

    #[tokio::test]
    async fn an_invalidated_revision_fails_the_export_for_good_and_puts_nothing() -> TestResult {
        let world = world()?;
        let fixture = fixture(&world)?;
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, fixture.files)?;
        let purge =
            serde_json::from_value::<PurgeId>(json!("00000000-0000-0000-0000-0000000000cc"))?;
        world.records.map_revision(
            &base()?,
            RevisionMapping {
                purge_id: purge,
                replacement: Some(Revision::try_from("3".repeat(40))?),
            },
        )?;
        let id = export_job(&world, 8, false)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert_eq!(entry.job.retryable, Some(false));
        let error = some(entry.job.error.as_ref(), "the failure")?;
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(matches!(
            error.detail.as_deref(),
            Some(ErrorDetail::Invalidated { purge_id, .. }) if *purge_id == purge
        ));
        assert!(
            world.versions.file_pages()?.is_empty(),
            "nothing read at it"
        );
        assert_eq!(world.records.artifacts()?, [] as [ArtifactRecord; 0]);
        assert_eq!(world.blobs.puts()?, [] as [ObjectInfo; 0]);
        Ok(())
    }

    #[tokio::test]
    async fn a_revision_off_the_accepted_line_fails_the_export_for_good() -> TestResult {
        for retained_candidate in [false, true] {
            let world = world()?;
            if retained_candidate {
                world.versions.set_candidate_tree(
                    &storage_scope()?,
                    &base()?,
                    vec![plain_file("notes/proposed.md", b"candidate bytes")],
                )?;
            }
            let id = export_job(&world, 9, false)?;
            assert!(run(&world, id).await?);
            let entry = world.records.entry(id)?;
            assert_eq!(entry.job.state, JobState::Failed);
            assert_eq!(entry.job.retryable, Some(false));
            assert_eq!(
                some(entry.job.error.as_ref(), "the failure")?.code,
                ErrorCode::NotFound
            );
            assert_eq!(world.records.artifacts()?, [] as [ArtifactRecord; 0]);
            assert_eq!(world.blobs.puts()?, [] as [ObjectInfo; 0]);
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_retried_export_writes_the_same_bytes_and_records_one_artifact() -> TestResult {
        let world = world()?;
        let fixture = fixture(&world)?;
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, fixture.files.clone())?;
        // The first attempt puts its archive and then cannot record it.
        world
            .records
            .fail_next_artifact(okf_jawn_contract::error::ApiError::new(
                ErrorCode::Unavailable,
                "the records store is busy",
            ))?;
        let id = export_job(&world, 10, false)?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        assert_eq!(entry.job.retryable, Some(true));
        assert_eq!(world.records.artifacts()?, [] as [ArtifactRecord; 0]);

        // Queued again under the same identity, it writes the same bytes, and one artifact.
        world
            .records
            .insert(id, entry.scope.clone(), entry.created.clone())?;
        assert!(run(&world, id).await?);
        let (_, digest) = archive_of(&world, id)?;
        let puts = world.blobs.puts()?;
        assert_eq!(puts.len(), 2);
        assert!(puts.iter().all(|put| put.digest == digest));
        assert_eq!(world.records.artifacts()?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn e11_completion_names_the_previously_recorded_artifact_not_newly_rebuilt_bytes()
    -> TestResult {
        let world = world()?;
        let fixture = fixture(&world)?;
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, fixture.files)?;
        let id = export_job(&world, 19, false)?;
        let previous = world
            .blobs
            .insert(b"previous attempt's retained archive bytes")?;
        let record = world
            .records
            .record_artifact(
                &JobScope::Workspace(storage_scope()?),
                mutation(19)?,
                NewArtifact {
                    kind: ArtifactKind::Export,
                    object: previous.clone(),
                    media_type: ARCHIVE_MEDIA_TYPE.to_owned(),
                    created_by_job: id,
                },
            )
            .await?;
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Succeeded);
        let completed = some(entry.completions.first(), "the replay's completion")?;
        assert_eq!(completed.artifact, Some(record.id));
        assert_eq!(
            completed.outputs.as_slice(),
            std::slice::from_ref(&previous.digest)
        );
        assert_eq!(world.records.artifacts()?.len(), 1);
        let puts = world.blobs.puts()?;
        assert_eq!(puts.len(), 1);
        assert_ne!(
            some(puts.first(), "the rebuilt archive")?.digest,
            previous.digest
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_export_takes_the_revisions_spelling_of_references() -> TestResult {
        let world = world()?;
        let fixture = fixture(&world)?;
        let mut files = fixture.files.clone();
        let kept = item_text(&[("type", json!("Note"))], "Kept.\n");
        files.push(item_file(
            "References/kept.md",
            &kept,
            "44444444-3333-4444-8555-666666666666",
            ItemKind::Note,
            None,
            None,
        )?);
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, files)?;
        let id = export_job(&world, 11, false)?;
        assert!(run(&world, id).await?);
        let found = names(&entries(&archive_of(&world, id)?.0)?);
        assert!(found.contains(&format!(
            "References/originals/{}/report.pdf",
            fixture.original.digest.as_str()
        )));
        assert!(!found.iter().any(|name| name.starts_with("references/")));
        Ok(())
    }

    /// These are ordinary, portable committed folder indexes, not synthetic invalid paths.
    fn add_committed_folder(files: &mut Vec<CommittedFile>, folder: &str) -> TestResult {
        let mut prefix = String::new();
        for segment in folder.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(segment);
            let path = format!("{prefix}/index.md");
            let checked = WorkspacePath::try_from(path.clone())?;
            assert_eq!(checked.as_str(), path);
            if !files.iter().any(|file| file.path == path) {
                files.push(plain_file(&path, b"# Subdirectories\n"));
            }
        }
        Ok(())
    }

    async fn assert_generated_path_fault(
        world: &World,
        files: Vec<CommittedFile>,
        include_history: bool,
    ) -> TestResult {
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, files)?;
        if include_history {
            world
                .versions
                .set_history(world.blobs.insert(b"# v2 git bundle\nthe history")?)?;
        }
        let id = export_job(world, 30, include_history)?;
        assert!(run(world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Failed);
        let error = some(entry.job.error.as_ref(), "the layout refusal")?;
        assert_eq!(error.code, ErrorCode::Internal);
        assert_eq!(
            error.message,
            "the portable archive holds conflicting paths"
        );
        assert!(entry.completions.is_empty());
        assert_eq!(world.records.artifacts()?, [] as [ArtifactRecord; 0]);
        assert_eq!(world.blobs.puts()?, [] as [ObjectInfo; 0]);
        Ok(())
    }

    /// SPEC3: generated files cannot occupy a committed directory's portable identity.
    #[tokio::test]
    async fn generated_path_refuses_a_committed_history_bundle_directory() -> TestResult {
        let world = world()?;
        let mut fixture = fixture(&world)?;
        add_committed_folder(&mut fixture.files, "history.bundle")?;
        assert_generated_path_fault(&world, fixture.files, true).await
    }

    #[tokio::test]
    async fn generated_path_refuses_a_committed_directory_at_a_derivative_file() -> TestResult {
        let world = world()?;
        let mut fixture = fixture(&world)?;
        let folder = format!("references/derived/{}.png", fixture.asset.digest.as_str());
        add_committed_folder(&mut fixture.files, &folder)?;
        assert_generated_path_fault(&world, fixture.files, false).await
    }

    #[tokio::test]
    async fn generated_path_refuses_case_aliases_of_nested_directories() -> TestResult {
        let world = world()?;
        let mut fixture = fixture(&world)?;
        add_committed_folder(&mut fixture.files, "references/Derived")?;
        assert_generated_path_fault(&world, fixture.files, false).await
    }

    #[tokio::test]
    async fn generated_path_refuses_unicode_case_aliases_at_an_original_file_prefix() -> TestResult
    {
        let world = world()?;
        let mut fixture = fixture_named(&world, "café.pdf")?;
        let folder = format!(
            "references/originals/{}/CAFÉ.PDF",
            fixture.original.digest.as_str()
        );
        add_committed_folder(&mut fixture.files, &folder)?;
        assert_generated_path_fault(&world, fixture.files, false).await
    }

    /// A textual prefix that ends inside a segment is not a file/directory collision.
    #[tokio::test]
    async fn generated_path_keeps_a_sibling_with_a_longer_segment() -> TestResult {
        let world = world()?;
        let mut fixture = fixture(&world)?;
        add_committed_folder(&mut fixture.files, "history.bundle-extra")?;
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, fixture.files)?;
        world
            .versions
            .set_history(world.blobs.insert(b"# v2 git bundle\nthe history")?)?;
        let id = export_job(&world, 30, true)?;
        assert!(run(&world, id).await?);
        let found = names(&entries(&archive_of(&world, id)?.0)?);
        assert!(found.contains(&"history.bundle".to_owned()));
        assert!(found.contains(&"history.bundle-extra/index.md".to_owned()));
        Ok(())
    }

    #[tokio::test]
    async fn e13_cancellation_after_page_one_prevents_the_next_page_put_and_completion()
    -> TestResult {
        let world = world()?;
        let files = (0..501)
            .map(|n| plain_file(&format!(".okf/f/{n:04}.json"), b"{}"))
            .collect();
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, files)?;
        let id = export_job(&world, 12, false)?;
        let records = Arc::clone(&world.records);
        *world
            .versions
            .after_file_page
            .lock()
            .map_err(|_| "page hook lock poisoned")? = Some(Box::new(move |page| {
            if page == 1 {
                records.cancel(id)?;
            }
            Ok(())
        }));
        assert!(run(&world, id).await?);
        let entry = world.records.entry(id)?;
        assert_eq!(entry.job.state, JobState::Cancelled);
        assert!(entry.completions.is_empty());
        assert_eq!(entry.failures, [] as [(String, bool); 0]);
        assert_eq!(world.records.artifacts()?, [] as [ArtifactRecord; 0]);
        assert_eq!(world.blobs.puts()?, [] as [ObjectInfo; 0]);
        assert_eq!(
            world.versions.file_pages()?.len(),
            1,
            "the next page must not be read"
        );
        Ok(())
    }

    #[tokio::test]
    async fn every_entry_is_stored_with_a_fixed_time_and_unix_mode() -> TestResult {
        let world = world()?;
        let fixture = fixture(&world)?;
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, fixture.files)?;
        let id = export_job(&world, 13, false)?;
        assert!(run(&world, id).await?);
        let found = entries(&archive_of(&world, id)?.0)?;
        assert_ne!(found, [] as [Entry; 0]);
        for (name, _, fixed) in &found {
            assert!(fixed, "{name} is stored with the fixed time and mode");
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_currently_reviewed_item_is_exported_with_verified_and_the_rest_as_stored()
    -> TestResult {
        let world = world()?;
        let fixture = fixture(&world)?;
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, fixture.files.clone())?;
        let review = |subject: &str, digest: Digest, selection: Selection| -> Built<Review> {
            Ok(Review {
                id: serde_json::from_value::<ReviewId>(json!(format!(
                    "00000000-0000-0000-0000-{:012}",
                    subject.len()
                )))?,
                source: SourceReference {
                    workspace_id: storage_scope()?.workspace_id,
                    item_id: fixture.note,
                    path: WorkspacePath::try_from("notes/a.md".to_owned())?,
                    revision: base()?,
                    digest: None,
                    selection,
                    locations: Vec::new(),
                },
                content_digest: digest,
                reviewer_subject: subject.to_owned(),
                reviewed_at: Timestamp::try_from("2026-10-08T09:00:00.000Z".to_owned())?,
                coverage: ReviewCoverage::Current,
            })
        };
        world.records.add_review(
            fixture.note,
            review("alice", fixture.note_digest.clone(), Selection::All)?,
        )?;
        // A review of other content and a review of some lines are not written.
        world.records.add_review(
            fixture.note,
            review("bob", Digest::try_from("e".repeat(64))?, Selection::All)?,
        )?;
        world.records.add_review(
            fixture.note,
            review(
                "carol",
                fixture.note_digest.clone(),
                Selection::Lines {
                    range: okf_jawn_contract::common::TextRange { start: 1, end: 1 },
                },
            )?,
        )?;
        let id = export_job(&world, 14, false)?;
        assert!(run(&world, id).await?);
        let found = entries(&archive_of(&world, id)?.0)?;
        let note = Document::parse(std::str::from_utf8(bytes_at(&found, "notes/a.md")?)?)?;
        let by: Vec<String> = note
            .frontmatter
            .verified()
            .into_iter()
            .filter_map(|event| event.by.map(|by| by.as_str().to_owned()))
            .collect();
        assert_eq!(by, ["human:alice"]);
        // The card has no review: its bytes are the stored ones.
        let card = some(
            fixture
                .files
                .iter()
                .find(|file| file.path == "reports/report-pdf.md"),
            "the card",
        )?;
        assert_eq!(bytes_at(&found, &card.path)?, card.bytes.as_slice());
        Ok(())
    }

    #[tokio::test]
    async fn the_archive_opens_as_an_okf_bundle_without_the_application() -> TestResult {
        let world = world()?;
        let fixture = fixture(&world)?;
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, fixture.files)?;
        let id = export_job(&world, 15, false)?;
        assert!(run(&world, id).await?);
        let found = entries(&archive_of(&world, id)?.0)?;
        // Extracted by hand, entry by entry, as a file manager does.
        let folder = tempfile::tempdir()?;
        for (name, bytes, _) in &found {
            let relative = std::path::Path::new(name);
            assert!(
                !name.is_empty()
                    && !name.contains('\\')
                    && relative
                        .components()
                        .all(|component| matches!(component, std::path::Component::Normal(_))),
                "unsafe archive entry {name}"
            );
            let target = folder.path().join(name);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, bytes)?;
        }
        let bundle = Bundle::load(folder.path())?;
        assert!(
            bundle.parse_errors().is_empty(),
            "{:?}",
            bundle.parse_errors()
        );
        let mut concepts: Vec<String> = bundle
            .concepts()
            .iter()
            .map(|concept| concept.id.to_string())
            .collect();
        concepts.sort();
        assert_eq!(concepts, ["notes/a", "reports/report-pdf"]);
        assert_eq!(bundle.okf_version(), Some("0.2"));
        // The application's own conformance check, okf-validator's, refuses nothing.
        let _warnings = OkfConformance.check(folder.path(), &[])?;
        Ok(())
    }

    #[tokio::test]
    async fn the_export_reads_the_tree_a_page_at_a_time() -> TestResult {
        let world = world()?;
        let mut files: Vec<CommittedFile> = (0..501)
            .map(|n| plain_file(&format!(".okf/f/{n:04}.json"), b"{}"))
            .collect();
        files.push(plain_file("index.md", ROOT_INDEX_TEXT.as_bytes()));
        world
            .versions
            .set_tree(&storage_scope()?, &base()?, files)?;
        let id = export_job(&world, 16, false)?;
        assert!(run(&world, id).await?);
        let pages = world.versions.file_pages()?;
        assert_eq!(pages.len(), 2);
        assert!(
            pages
                .iter()
                .all(|page| page.limit == 500 && page.max_bytes == 32 * 1024 * 1024)
        );
        assert_eq!(
            pages.get(1).and_then(|page| page.after.clone()).as_deref(),
            Some(".okf/f/0499.json")
        );
        assert_eq!(entries(&archive_of(&world, id)?.0)?.len(), 502);
        Ok(())
    }

    #[tokio::test]
    async fn a_committed_path_that_is_not_plain_fails_the_export_as_a_store_fault() -> TestResult {
        for path in [
            "",
            "notes/../escape.md",
            "/abs.md",
            "notes//a.md",
            "a\\b.md",
            "notes/./a.md",
            "a\0.md",
        ] {
            let world = world()?;
            world.versions.set_tree(
                &storage_scope()?,
                &base()?,
                vec![plain_file(path, b"# x\n")],
            )?;
            let id = export_job(&world, 17, false)?;
            assert!(run(&world, id).await?);
            let entry = world.records.entry(id)?;
            assert_eq!(entry.job.state, JobState::Failed, "{path}");
            assert_eq!(
                some(entry.job.error.as_ref(), "the failure")?.code,
                ErrorCode::Internal,
                "{path}"
            );
            assert_eq!(world.records.artifacts()?, [] as [ArtifactRecord; 0]);
        }
        Ok(())
    }

    #[tokio::test]
    async fn an_object_the_store_does_not_hold_fails_the_export_and_records_nothing() -> TestResult
    {
        for missing in ["original", "record", "window", "asset"] {
            let world = world()?;
            let fixture = fixture(&world)?;
            let digest = match missing {
                "original" => &fixture.original.digest,
                "record" => &fixture.record.digest,
                "window" => &some(fixture.exports.first(), "the retained window")?.digest,
                _ => &fixture.asset.digest,
            };
            world.blobs.forget(digest)?;
            world
                .versions
                .set_tree(&storage_scope()?, &base()?, fixture.files)?;
            let id = export_job(&world, 18, false)?;
            assert!(run(&world, id).await?);
            let entry = world.records.entry(id)?;
            assert_eq!(entry.job.state, JobState::Failed, "{missing}");
            assert_eq!(
                some(entry.job.error.as_ref(), "the failure")?.code,
                ErrorCode::NotFound,
                "{missing}"
            );
            assert_eq!(world.records.artifacts()?, [] as [ArtifactRecord; 0]);
            assert_eq!(world.blobs.puts()?, [] as [ObjectInfo; 0]);
        }
        Ok(())
    }
}
