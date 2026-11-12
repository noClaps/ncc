//! The call around the cursor, shared by signature help and completion.

use lsp_types::*;

use super::{BUILTINS, DeclKind, Server, docs, is_ident_byte, markdown};
use std::collections::HashMap;

use super::tyexpr;
use crate::ast::TypeExpr;
use crate::check::index::Sig;

/// The call around a position.
pub struct CallCtx {
    /// the callee's name as its signature shows it (`@as`, `c.read`, `add`)
    pub name: String,
    /// the callee's signature, with explicit generic arguments substituted
    pub sig: Sig,
    /// the generic parameters still to infer (none when the call gives them)
    pub open_tps: Vec<String>,
    /// index of the argument the cursor is in
    pub arg: u32,
    pub documentation: Option<String>,
    pub param_docs: Vec<String>,
}

impl Server {
    /// The unclosed call `callee(` (or `callee<T>(`) around `off`.
    pub(super) fn call_at(&self, uri: &Uri, off: usize) -> Option<CallCtx> {
        let (path, text) = self.source(uri)?;
        // find the unclosed `(` before the cursor and count the commas at its level, skipping
        // string and character literals and comments
        let b = text.as_bytes();
        let mut skip = vec![false; b.len()];
        for (s, e) in super::literals_and_comments(&text) {
            skip[s..e.min(b.len())].fill(true);
        }
        let (mut depth, mut commas, mut i) = (0i32, 0u32, off.min(b.len()));
        let open = loop {
            if i == 0 {
                return None;
            }
            i -= 1;
            if skip[i] {
                continue;
            }
            match b[i] {
                b')' | b']' | b'}' => depth += 1,
                b'(' | b'[' | b'{' if depth > 0 => depth -= 1,
                b'(' => break i,
                b'[' | b'{' => return None,
                b',' if depth == 0 => commas += 1,
                _ => {}
            }
        };
        let mut e = open;
        while e > 0 && b[e - 1] == b' ' {
            e -= 1;
        }
        // explicit generic arguments: `map<int, str>(`
        let generic_args: Vec<TypeExpr> = if e > 0 && b[e - 1] == b'>' {
            let close = e - 1;
            let mut d = 0;
            while e > 0 {
                e -= 1;
                match b[e] {
                    b'>' => d += 1,
                    b'<' => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            crate::parser::parse_generic_args_text(&text[e..=close]).unwrap_or_default()
        } else {
            vec![]
        };
        let mut s = e;
        while s > 0 && (is_ident_byte(b[s - 1]) || b[s - 1] == b'@' || b[s - 1] == b'.') {
            s -= 1;
        }
        let is_builtin = text[s..e].starts_with('@');
        let name = text[s..e].trim_start_matches('@');
        // a module or extern function: `math.sqrt(`, `c.read(`
        let (module, name) = match name.rsplit_once('.') {
            Some((m, n)) => (Some(m), n),
            None => (None, name),
        };
        let mut documentation = None;
        let mut param_docs: Vec<String> = Vec::new();
        let (shown, sig, tps) = if is_builtin {
            let b = BUILTINS.iter().find(|b| b.name == name)?;
            documentation = Some(b.doc.to_string());
            (format!("@{name}"), b.sig(), vec![])
        } else {
            let a = self.analyses.get(&path)?;
            let fid = a.file_id(&path);
            let target_file = module
                .and_then(|m| a.imports.get(m))
                .and_then(|mp| a.file_id(mp));
            let d = a
                .index
                .decls
                .iter()
                .filter(|d| d.name == name && d.kind == DeclKind::Function)
                .filter(|d| match (module, &d.extern_alias) {
                    (Some(m), Some(x)) => m == x,
                    (Some(_), None) => target_file == Some(d.span.file),
                    (None, Some(_)) => false,
                    (None, None) => true,
                })
                .filter(|d| {
                    d.scope.is_none_or(|sc| {
                        Some(sc.file) != fid || (sc.start as usize <= off && off <= sc.end as usize)
                    })
                })
                .max_by_key(|d| (Some(d.span.file) == fid, d.span.start as usize <= off))?;
            let ftext = &a.sm.file(d.span.file).text;
            if let Some(doc) = docs::doc_comment(ftext, d.span) {
                param_docs = docs::param_docs(&doc);
                documentation = Some(doc);
            }
            let shown = match &d.extern_alias {
                Some(x) => format!("{x}.{}", d.name),
                None => d.name.clone(),
            };
            (shown, d.sig.clone()?, d.type_params.clone())
        };
        // `fn map<type I, type O>(I[] arr, ...)` called as `map<int, str>(` shows
        // `fn map<int, str>(int[] arr, ...)`
        let (sig, open_tps) = if !tps.is_empty() && tps.len() == generic_args.len() {
            let binds: HashMap<String, TypeExpr> = tps
                .iter()
                .cloned()
                .zip(generic_args.iter().cloned())
                .collect();
            let sub = |t: &TypeExpr| tyexpr::substitute(t, &binds);
            let sig = Sig {
                params: (sig.params.iter())
                    .map(|(t, n)| (t.as_ref().map(sub), n.clone()))
                    .collect(),
                ret: sig.ret.as_ref().map(sub),
                type_params: generic_args.iter().map(TypeExpr::show).collect(),
            };
            (sig, vec![])
        } else {
            (sig, tps)
        };
        Some(CallCtx {
            name: shown,
            sig,
            open_tps,
            arg: commas,
            documentation,
            param_docs,
        })
    }

    pub(super) fn signature_help(&self, p: SignatureHelpParams) -> Option<SignatureHelp> {
        let pp = p.text_document_position_params;
        let (_, text) = self.source(&pp.text_document.uri)?;
        let off = super::pos_to_offset(&text, pp.position);
        let c = self.call_at(&pp.text_document.uri, off)?;
        let (label, ranges) = c.sig.label(&c.name);
        let parameters = (ranges.into_iter().enumerate())
            .map(|(i, (s, e))| ParameterInformation {
                label: ParameterLabel::LabelOffsets([
                    label[..s].encode_utf16().count() as u32,
                    label[..e].encode_utf16().count() as u32,
                ]),
                documentation: c
                    .param_docs
                    .get(i)
                    .map(|d| Documentation::MarkupContent(markdown(d.clone()))),
            })
            .collect();
        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label,
                documentation: c
                    .documentation
                    .map(|d| Documentation::MarkupContent(markdown(d))),
                parameters: Some(parameters),
                active_parameter: Some(c.arg),
            }],
            active_signature: Some(0),
            active_parameter: Some(c.arg),
        })
    }

    /// A function literal for an argument whose parameter is a function: in
    /// `map<int, str>(xs, |`, `fn(int i) str { ... }`.
    pub(super) fn fn_literal_items(&self, uri: &Uri, off: usize) -> Vec<CompletionItem> {
        let Some(c) = self.call_at(uri, off) else {
            return vec![];
        };
        let Some((ty, _)) = c.sig.params.get(c.arg as usize) else {
            return vec![];
        };
        let Some(fty @ TypeExpr::Fn(ps, ret, _)) = ty else {
            return vec![];
        };
        let ret_part = ret
            .as_ref()
            .map_or(String::new(), |r| format!(" {}", r.show()));
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let name = |i: usize| names.get(i).map_or(format!("a{i}"), |n| n.to_string());
        let args: Vec<(String, String)> = (ps.iter().enumerate())
            .map(|(i, t)| (t.show(), name(i)))
            .collect();
        let plain: Vec<String> = args.iter().map(|(t, n)| format!("{t} {n}")).collect();
        let snip: Vec<String> = args
            .iter()
            .enumerate()
            .map(|(i, (t, n))| format!("{t} ${{{}:{n}}}", i + 1))
            .collect();
        let label = format!("fn({}){ret_part} {{}}", plain.join(", "));
        vec![CompletionItem {
            label,
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(format!("function literal for `{}`", fty.show())),
            filter_text: Some("fn".into()),
            sort_text: Some("0fn".into()),
            insert_text: Some(format!("fn({}){ret_part} {{\n\t$0\n}}", snip.join(", "))),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            preselect: Some(true),
            ..Default::default()
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_labels() {
        let ty = crate::parser::parse_type_text;
        let sig = Sig {
            type_params: vec!["type I".into(), "type O".into()],
            params: vec![(ty("I[]"), "arr".into()), (ty("(fn(I) O)"), "f".into())],
            ret: ty("O[]"),
        };
        let (label, ranges) = sig.label("map");
        assert_eq!(label, "fn map<type I, type O>(I[] arr, (fn(I) O) f) O[]");
        let ps: Vec<&str> = ranges.iter().map(|&(a, b)| &label[a..b]).collect();
        assert_eq!(ps, ["I[] arr", "(fn(I) O) f"]);
        assert_eq!(sig.value_ty().unwrap().show(), "(fn(I[], (fn(I) O)) O[])");
    }
}
