//! Source policy checks AST attributes and headers without policing comment vocabulary, and
//! confines the zip crate's reader so that no authored code can extract an archive to disk.
//!
//! The zip rule. `.cargo/audit.toml` and `deny.toml` waive RUSTSEC-2026-0367, -0368 and -0369
//! (zip 8.6: extracting an archive to disk keeps SUID bits, writes symlinks outside the
//! destination and applies permissions through symlinks) only because nothing here extracts.
//! A per-file text scan cannot prove that: one file could re-export `ZipArchive` under another
//! name and another call `.extract(` on it. So the reader is confined, and each step of such a
//! route is refused on its own:
//! - production code names `ZipArchive` in `ZIP_READER_FILE` only; tests may name it, since no
//!   production code can import a test;
//! - nothing imports from the zip crate with a rename or a glob, or re-exports it;
//! - no type alias names `ZipArchive`, and no item visible outside its module (a `pub` or
//!   `pub(..)` function, method, field, constant, static, enum or trait) has it in its signature,
//!   so the reader never leaves the file that holds it;
//! - a file that names `ZipArchive` calls no `extract` and no `extract_unwrapped_root_dir`;
//! - nothing names `ZipStreamReader`, `read_zipfile_from_stream` or the `zip::read::stream`
//!   module.
//!
//! The rule reads syntax, not types: a method named `extract` on another type in a file that
//! names `ZipArchive` is refused too (none exists; rename it), which keeps the rule simple and
//! never too lenient.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::Path;

use syn::visit::Visit;

struct Attributes {
    violations: Vec<String>,
}

/// What one file does with the zip crate's reader, as `zip_violations` judges it: whether it
/// names the reader and calls an extraction (judged together), and each finding that is refused
/// on its own.
#[derive(Default)]
struct ZipUse {
    names_reader: bool,
    extracts: bool,
    findings: BTreeSet<&'static str>,
}

/// Whether a node names `ZipArchive` in any path segment.
#[derive(Default)]
struct NamesReader(bool);

/// The one production file allowed to hold the zip crate's reader.
const ZIP_READER_FILE: &str = "crates/storage/src/backup/archive.rs";
/// The reader's methods that extract an archive to disk.
const EXTRACTING: [&str; 2] = ["extract", "extract_unwrapped_root_dir"];
/// The streaming reader and its entry point, whose `extract` extracts to disk.
const STREAMING: [&str; 2] = ["ZipStreamReader", "read_zipfile_from_stream"];
/// The findings refused on their own, as each is reported.
const STREAMS: &str =
    "names the zip crate's stream reader or its root-folder extraction (the zip rule)";
const RENAMES: &str = "imports from the zip crate with a rename or a glob (the zip rule)";
const RE_EXPORTS: &str = "re-exports the zip crate or its reader (the zip rule)";
const ALIASES: &str = "aliases ZipArchive with a type alias (the zip rule)";
const EXPOSES: &str =
    "has ZipArchive in the signature of an item visible outside its module (the zip rule)";

impl<'ast> Visit<'ast> for Attributes {
    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        let path = attribute.path();
        if path.is_ident("allow") || path.is_ident("expect") {
            self.violations
                .push("lint suppression attribute".to_owned());
        }
        if path.is_ident("cfg_attr")
            && let syn::Meta::List(list) = &attribute.meta
        {
            let tokens = list.tokens.to_string();
            if tokens
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|token| matches!(token, "allow" | "expect"))
            {
                self.violations
                    .push("conditional lint suppression".to_owned());
            }
        }
        syn::visit::visit_attribute(self, attribute);
    }
}

impl<'ast> Visit<'ast> for NamesReader {
    fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
        if segment.ident == "ZipArchive" {
            self.0 = true;
        }
        syn::visit::visit_path_segment(self, segment);
    }
}

impl<'ast> Visit<'ast> for ZipUse {
    fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
        if segment.ident == "ZipArchive" {
            self.names_reader = true;
        }
        if STREAMING.iter().any(|name| segment.ident == name) {
            self.findings.insert(STREAMS);
        }
        syn::visit::visit_path_segment(self, segment);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let names: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        if names
            .windows(3)
            .any(|window| matches!(window, [zip, read, stream] if zip == "zip" && read == "read" && stream == "stream"))
        {
            self.findings.insert(STREAMS);
        }
        if names
            .last()
            .is_some_and(|last| EXTRACTING.contains(&last.as_str()))
            && names.len() > 1
        {
            self.extracts = true;
        }
        syn::visit::visit_path(self, path);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        if EXTRACTING.iter().any(|name| call.method == name) {
            self.extracts = true;
        }
        syn::visit::visit_expr_method_call(self, call);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        if roots_at_zip(&item.tree) {
            if !matches!(item.vis, syn::Visibility::Inherited) {
                self.findings.insert(RE_EXPORTS);
            }
            if renames_or_globs(&item.tree) {
                self.findings.insert(RENAMES);
            }
        }
        if use_names(&item.tree, &STREAMING) {
            self.findings.insert(STREAMS);
        }
        if use_names(&item.tree, &["ZipArchive"]) {
            self.names_reader = true;
            if !matches!(item.vis, syn::Visibility::Inherited) {
                self.findings.insert(RE_EXPORTS);
            }
        }
        syn::visit::visit_item_use(self, item);
    }

    fn visit_item_extern_crate(&mut self, item: &'ast syn::ItemExternCrate) {
        if item.ident == "zip"
            && (item.rename.is_some() || !matches!(item.vis, syn::Visibility::Inherited))
        {
            self.findings.insert(RENAMES);
        }
        syn::visit::visit_item_extern_crate(self, item);
    }

    fn visit_item_type(&mut self, item: &'ast syn::ItemType) {
        if names_reader(|visitor| visitor.visit_type(&item.ty)) {
            self.findings.insert(ALIASES);
        }
        syn::visit::visit_item_type(self, item);
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if visible(&item.vis) && names_reader(|visitor| visitor.visit_signature(&item.sig)) {
            self.findings.insert(EXPOSES);
        }
        syn::visit::visit_item_fn(self, item);
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        if visible(&item.vis) && names_reader(|visitor| visitor.visit_signature(&item.sig)) {
            self.findings.insert(EXPOSES);
        }
        syn::visit::visit_impl_item_fn(self, item);
    }

    fn visit_field(&mut self, field: &'ast syn::Field) {
        if visible(&field.vis) && names_reader(|visitor| visitor.visit_type(&field.ty)) {
            self.findings.insert(EXPOSES);
        }
        syn::visit::visit_field(self, field);
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        if visible(&item.vis) && names_reader(|visitor| visitor.visit_type(&item.ty)) {
            self.findings.insert(EXPOSES);
        }
        syn::visit::visit_item_const(self, item);
    }

    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        if visible(&item.vis) && names_reader(|visitor| visitor.visit_type(&item.ty)) {
            self.findings.insert(EXPOSES);
        }
        syn::visit::visit_item_static(self, item);
    }

    fn visit_item_enum(&mut self, item: &'ast syn::ItemEnum) {
        if visible(&item.vis) && names_reader(|visitor| visitor.visit_item_enum(item)) {
            self.findings.insert(EXPOSES);
        }
        syn::visit::visit_item_enum(self, item);
    }

    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        if visible(&item.vis) && names_reader(|visitor| visitor.visit_item_trait(item)) {
            self.findings.insert(EXPOSES);
        }
        syn::visit::visit_item_trait(self, item);
    }
}

pub(crate) fn check(root: &Path) -> Result<(), Box<dyn Error>> {
    let mut paths = Vec::new();
    collect(&root.join("crates"), &mut paths)?;
    collect(&root.join("xtask/src"), &mut paths)?;
    collect(&root.join("qualification"), &mut paths)?;
    let mut failures = Vec::new();
    for path in paths {
        let source = std::fs::read_to_string(&path)?;
        if !source.trim_start().starts_with("//!") {
            failures.push(format!("{}: missing module purpose header", path.display()));
        }
        let syntax = syn::parse_file(&source)?;
        let mut visitor = Attributes {
            violations: Vec::new(),
        };
        visitor.visit_file(&syntax);
        for violation in visitor.violations {
            failures.push(format!("{}: {violation}", path.display()));
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        for violation in zip_violations(&relative, &syntax) {
            failures.push(format!("{}: {violation}", path.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n").into())
    }
}

/// The zip rule (module header) over one parsed file, `relative` to the repository root with
/// forward slashes.
fn zip_violations(relative: &str, file: &syn::File) -> Vec<String> {
    let mut found = ZipUse::default();
    found.visit_file(file);
    let mut violations = Vec::new();
    let test_code = relative.split('/').any(|component| component == "tests");
    if found.names_reader && !test_code && relative != ZIP_READER_FILE {
        violations.push(format!(
            "names the zip crate's ZipArchive outside {ZIP_READER_FILE} (the zip rule)"
        ));
    }
    if found.names_reader && found.extracts {
        violations.push(
            "calls extract in a file that names ZipArchive; read named entries with by_name or by_index (the zip rule)"
                .to_owned(),
        );
    }
    violations.extend(found.findings.iter().map(|finding| (*finding).to_owned()));
    violations
}

/// Whether a `use` tree begins at the zip crate (`zip::…`).
fn roots_at_zip(tree: &syn::UseTree) -> bool {
    match tree {
        syn::UseTree::Path(path) => path.ident == "zip",
        syn::UseTree::Name(name) => name.ident == "zip",
        syn::UseTree::Rename(rename) => rename.ident == "zip",
        syn::UseTree::Group(group) => group.items.iter().any(roots_at_zip),
        syn::UseTree::Glob(_) => false,
    }
}

/// Whether a `use` tree renames anything or imports a glob.
fn renames_or_globs(tree: &syn::UseTree) -> bool {
    match tree {
        syn::UseTree::Path(path) => renames_or_globs(&path.tree),
        syn::UseTree::Name(_) => false,
        syn::UseTree::Rename(_) | syn::UseTree::Glob(_) => true,
        syn::UseTree::Group(group) => group.items.iter().any(renames_or_globs),
    }
}

/// Whether a `use` tree imports one of `names`, under its own name or another.
fn use_names(tree: &syn::UseTree, names: &[&str]) -> bool {
    match tree {
        syn::UseTree::Path(path) => use_names(&path.tree, names),
        syn::UseTree::Name(name) => names.iter().any(|wanted| name.ident == wanted),
        syn::UseTree::Rename(rename) => names.iter().any(|wanted| rename.ident == wanted),
        syn::UseTree::Glob(_) => false,
        syn::UseTree::Group(group) => group.items.iter().any(|item| use_names(item, names)),
    }
}

/// Whether an item is visible outside its module: `pub` or any `pub(..)`.
const fn visible(visibility: &syn::Visibility) -> bool {
    !matches!(visibility, syn::Visibility::Inherited)
}

/// Whether the part of a node that `visit` walks names `ZipArchive`.
fn names_reader(visit: impl FnOnce(&mut NamesReader)) -> bool {
    let mut visitor = NamesReader::default();
    visit(&mut visitor);
    visitor.0
}

fn collect(root: &Path, paths: &mut Vec<std::path::PathBuf>) -> Result<(), std::io::Error> {
    if !root.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect(&path, paths)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::{ZIP_READER_FILE, zip_violations};

    type TestResult = Result<(), Box<dyn Error>>;

    /// The zip rule's findings for `source` at `relative`.
    fn zip(relative: &str, source: &str) -> Result<Vec<String>, Box<dyn Error>> {
        Ok(zip_violations(relative, &syn::parse_file(source)?))
    }

    /// Whether some finding says `words`.
    fn says(found: &[String], words: &str) -> bool {
        found.iter().any(|finding| finding.contains(words))
    }

    #[test]
    fn the_archive_module_reading_named_entries_is_clean() -> TestResult {
        let source = "//! x\nuse zip::{ZipArchive, ZipWriter};\nuse std::fs::File;\npub(crate) struct Opened { zip: ZipArchive<File> }\nimpl Opened {\n    pub(crate) fn extract(&mut self, name: &str) -> usize { let entry = self.zip.by_name(name); 0 }\n}\nfn open(file: File) -> ZipArchive<File> { ZipArchive::new(file) }\n";
        assert_eq!(zip(ZIP_READER_FILE, source)?, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn a_test_may_name_the_reader_and_read_entries() -> TestResult {
        let source = "//! x\nfn t() { let mut archive = zip::ZipArchive::new(file); archive.by_index(0); }\n";
        assert_eq!(
            zip("crates/storage/tests/backup.rs", source)?,
            Vec::<String>::new()
        );
        Ok(())
    }

    #[test]
    fn a_re_export_under_another_name_is_refused() -> TestResult {
        let found = zip(
            ZIP_READER_FILE,
            "//! x\npub use zip::ZipArchive as Archive;\n",
        )?;
        assert!(says(&found, "re-exports"), "{found:?}");
        assert!(says(&found, "a rename or a glob"), "{found:?}");
        let glob = zip(ZIP_READER_FILE, "//! x\nuse zip::*;\n")?;
        assert!(says(&glob, "a rename or a glob"), "{glob:?}");
        let crate_alias = zip(ZIP_READER_FILE, "//! x\nextern crate zip as archive;\n")?;
        assert!(says(&crate_alias, "a rename or a glob"), "{crate_alias:?}");
        Ok(())
    }

    #[test]
    fn production_code_outside_the_archive_module_never_names_the_reader() -> TestResult {
        let found = zip(
            "crates/ingest/src/import.rs",
            "//! x\nfn f() { let archive = zip::ZipArchive::new(file); }\n",
        )?;
        assert!(says(&found, "outside"), "{found:?}");
        Ok(())
    }

    #[test]
    fn extracting_beside_the_reader_is_refused_by_method_and_by_path() -> TestResult {
        let by_method = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nfn f(a: &mut ZipArchive<File>) { a.extract(dir); }\n",
        )?;
        assert!(says(&by_method, "calls extract"), "{by_method:?}");
        let by_path = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nfn f(a: &mut ZipArchive<File>) { ZipArchive::extract_unwrapped_root_dir(a, dir); }\n",
        )?;
        assert!(says(&by_path, "calls extract"), "{by_path:?}");
        let in_test = zip(
            "crates/storage/tests/purge.rs",
            "//! x\nfn t() { zip::ZipArchive::new(file).extract(dir); }\n",
        )?;
        assert!(says(&in_test, "calls extract"), "{in_test:?}");
        Ok(())
    }

    #[test]
    fn the_reader_never_leaves_its_file() -> TestResult {
        let alias = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\ntype Reader = ZipArchive<File>;\n",
        )?;
        assert!(says(&alias, "type alias"), "{alias:?}");
        let returned = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\npub(crate) fn open(f: File) -> ZipArchive<File> { ZipArchive::new(f) }\n",
        )?;
        assert!(says(&returned, "visible outside"), "{returned:?}");
        let field = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\npub struct Opened { pub zip: ZipArchive<File> }\n",
        )?;
        assert!(says(&field, "visible outside"), "{field:?}");
        let method = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nstruct O;\nimpl O { pub(super) fn inner(&mut self) -> &mut ZipArchive<File> { todo() } }\n",
        )?;
        assert!(says(&method, "visible outside"), "{method:?}");
        Ok(())
    }

    #[test]
    fn the_stream_reader_is_refused_anywhere() -> TestResult {
        let named = zip(
            "crates/storage/tests/backup.rs",
            "//! x\nuse zip::read::stream::ZipStreamReader;\n",
        )?;
        assert!(says(&named, "stream reader"), "{named:?}");
        let entry = zip(
            "crates/storage/tests/backup.rs",
            "//! x\nfn t() { zip::read::read_zipfile_from_stream(&mut input); }\n",
        )?;
        assert!(says(&entry, "stream reader"), "{entry:?}");
        Ok(())
    }
}
