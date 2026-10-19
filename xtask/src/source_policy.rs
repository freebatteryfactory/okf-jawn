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
//! - no type alias names `ZipArchive`, no item visible outside its module (a `pub` or
//!   `pub(..)` function, method, field, constant or associated constant, associated type, static,
//!   enum or trait) has it in its signature,
//!   and no trait implementation names it (its trait, its self type, or the signature, associated
//!   type or constant of any item: `AsMut<ZipArchive<_>>`, `Deref<Target = ZipArchive<_>>` and
//!   the like are callable from anywhere, though written without `pub`), so the reader never
//!   leaves the file that holds it;
//! - production code names `ZipArchive` in no macro, a definition or an invocation, because the
//!   rule cannot read what a macro expands to (an alias, an impl);
//! - a file that names `ZipArchive` calls no `extract` and no `extract_unwrapped_root_dir`, and
//!   declares no child module in another file and includes no other file (`include!`), so every
//!   module that can reach its private fields (its descendants) is in that same file; the
//!   identifiers inside macros, definitions and invocations alike, are read for the same names
//!   (string literals are not). Tests are exempt from the child-module part: a test target's
//!   support modules (`#[path]` `check.rs`) are read on their own, and tests read archives
//!   they wrote, never one a caller supplied;
//! - nothing names `ZipStreamReader`, `read_zipfile_from_stream` or the `zip::read::stream`
//!   module;
//! - production code includes no file (`include!`), which the rule cannot read, and renames
//!   `ZipArchive` nowhere, wherever the `use` starts (`use self::ZipArchive as Archive`);
//! - production code declares no module by `#[path]`: the rule judges a file production or test
//!   by where it lives, so a production module taken from a `tests` folder would be judged as a
//!   test (none exists, and source code never includes test support).
//!
//! The rule reads syntax, not types: a method named `extract` on another type in a file that
//! names `ZipArchive` is refused too (none exists; rename it), which keeps the rule simple and
//! never too lenient. What it cannot see, stated rather than hidden: code a build script or a
//! procedural macro (a derive or an attribute) generates, and a reader reached through generic
//! code bounded by a trait the reader implements. None of these exists here, the reader file
//! derives nothing that hands its fields out, and authored `unsafe` is forbidden workspace-wide.

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
    reaches_out: bool,
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
const EXPOSES: &str = "has ZipArchive in the signature of an item visible outside its module, or in a trait implementation (the zip rule)";
const PATH_MODULE: &str = "declares a module by #[path] in production code, whose file the rule would judge by another place (the zip rule)";
const INCLUDES: &str = "includes another file (include!) in production code, which the rule cannot read (the zip rule)";
const IN_MACRO: &str =
    "names ZipArchive inside a macro, whose expansion the rule cannot read (the zip rule)";

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
        // Any path ending in an extraction counts, a one-segment one included: the qualified
        // self of `<ZipArchive<File>>::extract` is kept apart from the path's segments.
        if names
            .last()
            .is_some_and(|last| EXTRACTING.contains(&last.as_str()))
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

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if item.content.is_none() {
            self.reaches_out = true;
        }
        if item.attrs.iter().any(names_a_path_attribute) {
            self.findings.insert(PATH_MODULE);
        }
        syn::visit::visit_item_mod(self, item);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if mac.path.is_ident("include") {
            self.reaches_out = true;
            self.findings.insert(INCLUDES);
        }
        let buffer = syn::buffer::TokenBuffer::new2(mac.tokens.clone());
        self.read_tokens(buffer.begin());
        syn::visit::visit_macro(self, mac);
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
        if renames_reader(&item.tree) {
            self.findings.insert(RENAMES);
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

    fn visit_impl_item_const(&mut self, item: &'ast syn::ImplItemConst) {
        if visible(&item.vis) && names_reader(|visitor| visitor.visit_type(&item.ty)) {
            self.findings.insert(EXPOSES);
        }
        syn::visit::visit_impl_item_const(self, item);
    }

    fn visit_impl_item_type(&mut self, item: &'ast syn::ImplItemType) {
        if visible(&item.vis) && names_reader(|visitor| visitor.visit_type(&item.ty)) {
            self.findings.insert(EXPOSES);
        }
        syn::visit::visit_impl_item_type(self, item);
    }

    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        if visible(&item.vis) && names_reader(|visitor| visitor.visit_type(&item.ty)) {
            self.findings.insert(EXPOSES);
        }
        syn::visit::visit_item_static(self, item);
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        if let Some((trait_path, _)) = &item.trait_ {
            let carries = names_reader(|visitor| {
                visitor.visit_path(trait_path);
                visitor.visit_type(&item.self_ty);
                visitor.visit_generics(&item.generics);
                for member in &item.items {
                    match member {
                        syn::ImplItem::Fn(function) => visitor.visit_signature(&function.sig),
                        syn::ImplItem::Type(associated) => visitor.visit_type(&associated.ty),
                        syn::ImplItem::Const(constant) => visitor.visit_type(&constant.ty),
                        _ => {}
                    }
                }
            });
            if carries {
                self.findings.insert(EXPOSES);
            }
        }
        syn::visit::visit_item_impl(self, item);
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

impl ZipUse {
    /// Read a macro's tokens for the rule's names: identifiers only, in every group, so a string
    /// literal that holds one of the words is not taken for code.
    fn read_tokens(&mut self, mut cursor: syn::buffer::Cursor<'_>) {
        while !cursor.eof() {
            if let Some((ident, next)) = cursor.ident() {
                if ident == "ZipArchive" {
                    self.names_reader = true;
                    self.findings.insert(IN_MACRO);
                }
                if EXTRACTING.iter().any(|name| ident == name) {
                    self.extracts = true;
                }
                if STREAMING.iter().any(|name| ident == name) {
                    self.findings.insert(STREAMS);
                }
                cursor = next;
            } else if let Some((inside, _, _, next)) = cursor.any_group() {
                self.read_tokens(inside);
                cursor = next;
            } else if let Some((_, next)) = cursor.token_tree() {
                cursor = next;
            } else {
                break;
            }
        }
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
    if found.names_reader && found.reaches_out && !test_code {
        violations.push(
            "names ZipArchive in a file that declares a child module in another file or includes one; a child module could reach its private fields (the zip rule)"
                .to_owned(),
        );
    }
    violations.extend(
        found
            .findings
            .iter()
            .filter(|finding| {
                !(test_code
                    && (**finding == IN_MACRO || **finding == PATH_MODULE || **finding == INCLUDES))
            })
            .map(|finding| (*finding).to_owned()),
    );
    violations
}

/// Whether an attribute is `#[path = ..]`, or a `#[cfg_attr(..)]` that may apply one.
fn names_a_path_attribute(attribute: &syn::Attribute) -> bool {
    if attribute.path().is_ident("path") {
        return true;
    }
    attribute.path().is_ident("cfg_attr")
        && matches!(&attribute.meta, syn::Meta::List(list) if list
            .tokens
            .to_string()
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .any(|token| token == "path"))
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

/// Whether a `use` tree renames `ZipArchive`, wherever the tree starts (`self::ZipArchive as ..`).
fn renames_reader(tree: &syn::UseTree) -> bool {
    match tree {
        syn::UseTree::Path(path) => renames_reader(&path.tree),
        syn::UseTree::Rename(rename) => rename.ident == "ZipArchive",
        syn::UseTree::Name(_) | syn::UseTree::Glob(_) => false,
        syn::UseTree::Group(group) => group.items.iter().any(renames_reader),
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

    use super::{ZIP_READER_FILE, check, zip_violations};

    type TestResult = Result<(), Box<dyn Error>>;

    /// Run the production policy over a miniature authored tree, without compiling its code.
    fn refused_unsafe(relative: &str, source: &str) -> Result<bool, Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join(relative);
        let parent = path.parent().ok_or("source has no parent")?;
        std::fs::create_dir_all(parent)?;
        std::fs::write(path, format!("//! Policy fixture.\n{source}"))?;
        Ok(check(directory.path()).is_err())
    }

    #[test]
    fn unsafe_policy_allows_only_the_windows_handle_call() -> TestResult {
        let source = "#[cfg(windows)] fn keep_back_standard_handles() { for handle in handles { if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 { return; } } }";
        assert!(!refused_unsafe("crates/cli/src/relay/launch.rs", source)?);
        assert!(refused_unsafe("crates/cli/src/other.rs", source)?);
        assert!(refused_unsafe(
            "crates/cli/src/relay/launch.rs",
            &source.replace("keep_back_standard_handles", "other")
        )?);
        Ok(())
    }

    #[test]
    fn unsafe_policy_refuses_other_operations_and_declarations() -> TestResult {
        for source in [
            "fn other() { unsafe { arbitrary(); } }",
            "unsafe fn other() {}",
            "unsafe trait Other {}",
            "unsafe impl Other for Thing {}",
            "unsafe extern \"C\" { fn other(); }",
            "macro_rules! other { () => { unsafe { arbitrary() } }; }",
            "#[cfg(windows)] fn keep_back_standard_handles() { unsafe { arbitrary(); } }",
            "fn keep_back_standard_handles() { unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) }; }",
            "#[cfg(windows)] fn keep_back_standard_handles() { unsafe { SetHandleInformation(arbitrary(), HANDLE_FLAG_INHERIT, 0) }; }",
            "#[cfg(windows)] fn keep_back_standard_handles() { unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0); arbitrary(); } }",
            "#[cfg(windows)] fn keep_back_standard_handles() { unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) }; unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) }; }",
            "#[cfg(windows)] fn keep_back_standard_handles() { fn nested() { unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) }; } }",
        ] {
            assert!(
                refused_unsafe("crates/cli/src/relay/launch.rs", source)?,
                "{source}"
            );
        }
        Ok(())
    }

    #[test]
    fn unsafe_policy_ignores_documentation_and_string_literals() -> TestResult {
        assert!(!refused_unsafe(
            "crates/cli/src/other.rs",
            "// unsafe { arbitrary() }\nfn safe() { let text = \"unsafe fn fake() {}\"; say!(\"unsafe\"); }"
        )?);
        Ok(())
    }

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
        let chained = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nuse self::ZipArchive as Archive;\n",
        )?;
        assert!(says(&chained, "a rename or a glob"), "{chained:?}");
        let included = zip(
            "crates/storage/src/backup/mod.rs",
            "//! x\ninclude!(\"../../../tests/helper.inc\");\n",
        )?;
        assert!(says(&included, "include!"), "{included:?}");
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
        let qualified_self = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nfn f(a: &mut ZipArchive<File>) { <ZipArchive<File>>::extract(a, dir); }\n",
        )?;
        assert!(says(&qualified_self, "calls extract"), "{qualified_self:?}");
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
        let constant = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nstruct Opened;\nimpl Opened { pub(crate) const OPEN: fn(File) -> ZipResult<ZipArchive<File>> = ZipArchive::new; }\n",
        )?;
        assert!(says(&constant, "visible outside"), "{constant:?}");
        let method = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nstruct O;\nimpl O { pub(super) fn inner(&mut self) -> &mut ZipArchive<File> { todo() } }\n",
        )?;
        assert!(says(&method, "visible outside"), "{method:?}");
        Ok(())
    }

    #[test]
    fn no_child_module_of_the_reader_lives_in_another_file() -> TestResult {
        let child = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nstruct Opened { zip: ZipArchive<File> }\nmod child;\n",
        )?;
        assert!(says(&child, "child module"), "{child:?}");
        let included = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nstruct Opened { zip: ZipArchive<File> }\ninclude!(\"more.rs\");\n",
        )?;
        assert!(says(&included, "child module"), "{included:?}");
        let inline = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nstruct Opened { zip: ZipArchive<File> }\nmod inner { fn f(o: &mut super::Opened) { o.zip.by_index(0); } }\n",
        )?;
        assert_eq!(
            inline,
            Vec::<String>::new(),
            "an inline child is read with its file"
        );
        Ok(())
    }

    #[test]
    fn a_trait_implementation_never_hands_out_the_reader() -> TestResult {
        let as_mut = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nstruct Opened { zip: ZipArchive<File> }\nimpl AsMut<ZipArchive<File>> for Opened { fn as_mut(&mut self) -> &mut ZipArchive<File> { &mut self.zip } }\n",
        )?;
        assert!(says(&as_mut, "trait implementation"), "{as_mut:?}");
        let deref = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nstruct Opened { zip: ZipArchive<File> }\nimpl std::ops::Deref for Opened { type Target = ZipArchive<File>; fn deref(&self) -> &Self::Target { &self.zip } }\n",
        )?;
        assert!(says(&deref, "trait implementation"), "{deref:?}");
        let unrelated = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nstruct Opened { zip: ZipArchive<File> }\nstruct Hashing;\nimpl std::io::Write for Hashing { fn flush(&mut self) -> std::io::Result<()> { Ok(()) } }\n",
        )?;
        assert_eq!(unrelated, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn production_code_names_the_reader_in_no_macro() -> TestResult {
        let alias = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nmacro_rules! alias { () => { pub(crate) type Archive = ZipArchive<File>; }; }\nalias!();\n",
        )?;
        assert!(says(&alias, "inside a macro"), "{alias:?}");
        let in_test = zip(
            "crates/storage/tests/backup.rs",
            "//! x\nfn t() { assert!(zip::ZipArchive::new(file).is_ok()); }\n",
        )?;
        assert_eq!(
            in_test,
            Vec::<String>::new(),
            "a test reads it in an assertion"
        );
        Ok(())
    }

    #[test]
    fn production_code_takes_no_module_by_path() -> TestResult {
        let production = zip(
            "crates/storage/src/backup/mod.rs",
            "//! x\n#[path = \"../../tests/helper.rs\"]\nmod helper;\n",
        )?;
        assert!(says(&production, "#[path]"), "{production:?}");
        let conditional = zip(
            "crates/storage/src/backup/mod.rs",
            "//! x\n#[cfg_attr(not(test), path = \"../../tests/helper.rs\")]\nmod helper;\n",
        )?;
        assert!(says(&conditional, "#[path]"), "{conditional:?}");
        let test = zip(
            "crates/storage/tests/backup.rs",
            "//! x\n#[path = \"support/check.rs\"]\nmod check;\n",
        )?;
        assert_eq!(test, Vec::<String>::new(), "a test target's support module");
        Ok(())
    }

    #[test]
    fn a_string_literal_in_a_macro_is_not_code() -> TestResult {
        let found = zip(
            "xtask/src/report.rs",
            "//! x\nfn f() { let words = format!(\"ZipArchive cannot extract here\"); }\n",
        )?;
        assert_eq!(found, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn a_macro_is_read_for_the_same_names() -> TestResult {
        let defined = zip(
            ZIP_READER_FILE,
            "//! x\nuse zip::ZipArchive;\nmacro_rules! out { ($a:expr) => { $a.extract(dir) }; }\nfn f(a: &mut ZipArchive<File>) { out!(a); }\n",
        )?;
        assert!(says(&defined, "calls extract"), "{defined:?}");
        let invoked = zip(
            "crates/storage/tests/purge.rs",
            "//! x\nfn t() { run!(zip::ZipArchive::new(file).extract(dir)); }\n",
        )?;
        assert!(says(&invoked, "calls extract"), "{invoked:?}");
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
