//! Source policy checks AST attributes and headers without policing comment vocabulary.

use std::error::Error;
use std::path::Path;

use syn::visit::Visit;

struct Attributes {
    violations: Vec<String>,
}

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

pub(crate) fn check(root: &Path) -> Result<(), Box<dyn Error>> {
    let mut paths = Vec::new();
    collect(&root.join("crates"), &mut paths)?;
    collect(&root.join("xtask/src"), &mut paths)?;
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
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n").into())
    }
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
