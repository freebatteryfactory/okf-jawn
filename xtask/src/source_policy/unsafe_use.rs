//! Confine the owner's C1 exception to one Windows handle call in the CLI launcher.
//! This checks authored syntax, including macro tokens; string literals and comments are data.
//! Compiler lints still forbid unsafe in every other crate. Generated procedural-macro output
//! is outside this syntax check, just as it is outside the neighboring zip policy.

use std::collections::BTreeSet;

use syn::visit::Visit;

#[derive(Default)]
struct UnsafeUse {
    allowed_function: bool,
    blocks: usize,
    findings: BTreeSet<&'static str>,
}

impl<'ast> Visit<'ast> for UnsafeUse {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        // A nested declaration cannot borrow its containing function's exception.
        let outer = self.allowed_function;
        self.allowed_function = false;
        syn::visit::visit_item(self, item);
        self.allowed_function = outer;
    }

    fn visit_signature(&mut self, signature: &'ast syn::Signature) {
        if matches!(signature.safety, syn::Safety::Unsafe(_)) {
            self.findings
                .insert("authored unsafe function is forbidden");
        }
        syn::visit::visit_signature(self, signature);
    }

    fn visit_expr_unsafe(&mut self, expression: &'ast syn::ExprUnsafe) {
        if self.allowed_function && exact_handle_call(&expression.block) {
            self.blocks = self.blocks.saturating_add(1);
        } else {
            self.findings
                .insert("unsafe block is outside the one C1 handle-call exception");
        }
        syn::visit::visit_expr_unsafe(self, expression);
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        if item.unsafety.is_some() {
            self.findings.insert("authored unsafe impl is forbidden");
        }
        syn::visit::visit_item_impl(self, item);
    }

    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        if item.unsafety.is_some() {
            self.findings.insert("authored unsafe trait is forbidden");
        }
        syn::visit::visit_item_trait(self, item);
    }

    fn visit_item_foreign_mod(&mut self, item: &'ast syn::ItemForeignMod) {
        if item.unsafety.is_some() {
            self.findings.insert("authored unsafe extern is forbidden");
        }
        syn::visit::visit_item_foreign_mod(self, item);
    }

    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        if attribute.path().is_ident("unsafe") {
            self.findings
                .insert("authored unsafe attribute is forbidden");
        }
        if let syn::Meta::List(list) = &attribute.meta
            && list.path.is_ident("cfg_attr")
            && has_unsafe(syn::buffer::TokenBuffer::new2(list.tokens.clone()).begin())
        {
            self.findings
                .insert("conditional unsafe attribute is forbidden");
        }
        syn::visit::visit_attribute(self, attribute);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let tokens = syn::buffer::TokenBuffer::new2(mac.tokens.clone());
        if has_unsafe(tokens.begin()) {
            self.findings
                .insert("unsafe in a macro cannot use the C1 exception");
        }
        syn::visit::visit_macro(self, mac);
    }
}

pub(super) fn violations(relative: &str, file: &syn::File) -> Vec<String> {
    let mut found = UnsafeUse::default();
    let mut functions = 0_u32;
    for item in &file.items {
        if let syn::Item::Fn(function) = item
            && relative == "crates/cli/src/relay/launch.rs"
            && function.sig.ident == "keep_back_standard_handles"
            && function.attrs.iter().any(|attribute| {
                matches!(&attribute.meta, syn::Meta::List(list)
                    if list.path.is_ident("cfg") && list.tokens.to_string() == "windows")
            })
        {
            functions = functions.saturating_add(1);
            found.allowed_function = true;
            syn::visit::visit_item_fn(&mut found, function);
            found.allowed_function = false;
        } else {
            found.visit_item(item);
        }
    }
    if functions > 1 || found.blocks > 1 {
        found
            .findings
            .insert("C1 permits one function and one unsafe call site only");
    }
    found.findings.into_iter().map(str::to_owned).collect()
}

fn exact_handle_call(block: &syn::Block) -> bool {
    let [syn::Stmt::Expr(syn::Expr::Call(call), _)] = block.stmts.as_slice() else {
        return false;
    };
    if !named(&call.func, "SetHandleInformation") {
        return false;
    }
    let arguments: Vec<_> = call.args.iter().collect();
    matches!(arguments.as_slice(), [handle, mask, syn::Expr::Lit(value)]
        if named(handle, "handle") && named(mask, "HANDLE_FLAG_INHERIT")
        && matches!(&value.lit, syn::Lit::Int(integer) if integer.to_string() == "0"))
}

fn named(expression: &syn::Expr, name: &str) -> bool {
    matches!(expression, syn::Expr::Path(path) if path.qself.is_none() && path.path.is_ident(name))
}

fn has_unsafe(mut cursor: syn::buffer::Cursor<'_>) -> bool {
    while !cursor.eof() {
        if let Some((identifier, next)) = cursor.ident() {
            if identifier == "unsafe" {
                return true;
            }
            cursor = next;
        } else if let Some((inside, _, _, next)) = cursor.any_group() {
            if has_unsafe(inside) {
                return true;
            }
            cursor = next;
        } else if let Some((_, next)) = cursor.token_tree() {
            cursor = next;
        } else {
            break;
        }
    }
    false
}
