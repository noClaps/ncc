//! More code actions (modelled on Gleam's and rust-analyzer's) and exit-point highlights.
//!
//! Quick fixes: generate a missing type, create a missing module, make an item public, fill the
//! missing fields of a struct pattern.
//! Refactors: convert between `//` and `///` comments, convert an integer literal's base,
//! interpolate a selection into its string, extract a constant, match on a call's result.

use std::collections::HashSet;
use std::fmt::Write;

use lsp_types::*;

use super::nav::{Node, item_spans, walk};
use super::{
    Analysis, Server, edit_action, line_end, line_start, path_to_uri, pos_to_offset, range,
    uri_to_path,
};
use crate::ast::{self, ExprKind as E, StmtKind as S};
use crate::check::index::{DeclKind, SymDecl};
use crate::lexer::Tok;
use crate::lexer::is_ident_byte;

/// The extent (start, end) of the top-level item containing `off`.
pub(super) fn item_extent(module: &ast::Module, off: u32) -> Option<(u32, u32)> {
    for it in &module.items {
        let (mut lo, mut hi) = (u32::MAX, 0);
        item_spans(it, &mut |n| {
            if let Node::Span(sp) = n {
                lo = lo.min(sp.start);
                hi = hi.max(sp.end);
            }
        });
        if lo <= off && off <= hi {
            return Some((lo, hi));
        }
    }
    None
}

/// Format an integer in a base, as an NC literal.
fn int_in_base(v: u128, base: u32, unsigned: bool) -> String {
    let digits = match base {
        16 => format!("0x{v:X}"),
        8 => format!("0o{v:o}"),
        2 => format!("0b{v:b}"),
        _ => v.to_string(),
    };
    if unsigned {
        format!("{digits}u")
    } else {
        digits
    }
}

impl Server {
    /// Actions in addition to those of `code_actions`.
    pub(super) fn more_actions(
        &self,
        p: &CodeActionParams,
        module: &ast::Module,
        text: &str,
    ) -> Vec<CodeActionOrCommand> {
        let uri = &p.text_document.uri;
        let mut out = Vec::new();
        for d in &p.context.diagnostics {
            out.extend(self.diagnostic_fix(uri, module, text, d));
        }
        let (s, e) = (
            pos_to_offset(text, p.range.start),
            pos_to_offset(text, p.range.end),
        );
        out.extend(convert_comment(uri, text, s));
        out.extend(convert_int(uri, text, s, e));
        out.extend(interpolate(uri, text, s, e));
        if let Some((a, fid, ftext)) = self.fresh(uri)
            && ftext == text
        {
            out.extend(extract_constant(a, fid, module, uri, text, s, e));
            out.extend(match_on_call(a, fid, module, uri, text, s));
        }
        out
    }

    fn diagnostic_fix(
        &self,
        uri: &Uri,
        module: &ast::Module,
        text: &str,
        d: &Diagnostic,
    ) -> Vec<CodeActionOrCommand> {
        let msg = d.message.as_str();
        let off = pos_to_offset(text, d.range.start) as u32;
        let mut out = Vec::new();
        if let Some(name) = msg
            .strip_prefix("unknown type `")
            .and_then(|r| r.strip_suffix('`'))
            .filter(|n| n.bytes().all(is_ident_byte))
        {
            // after the top-level item that uses it
            let at =
                item_extent(module, off).map_or(text.len(), |(_, hi)| line_end(text, hi as usize));
            let lead = if text[..at].ends_with('\n') { "" } else { "\n" };
            for kind in ["struct", "enum"] {
                out.push(edit_action(
                    format!("Generate {kind} `{name}`"),
                    CodeActionKind::QUICKFIX,
                    vec![(
                        uri.clone(),
                        TextEdit::new(
                            range(text, at, at),
                            format!("{lead}\n{kind} {name} {{\n}}\n"),
                        ),
                    )],
                    Some(d),
                ));
            }
        } else if msg.starts_with("cannot find module `")
            && let Some(target) = msg
                .split_once("(looked for `")
                .and_then(|(_, r)| r.strip_suffix("`)"))
        {
            // create the missing file
            let new = path_to_uri(std::path::Path::new(target));
            out.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!(
                    "Create module `{}`",
                    msg.trim_start_matches("cannot find module `")
                        .split('`')
                        .next()
                        .unwrap_or("")
                ),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![d.clone()]),
                edit: Some(WorkspaceEdit {
                    document_changes: Some(DocumentChanges::Operations(vec![
                        DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                            uri: new,
                            options: Some(CreateFileOptions {
                                overwrite: Some(false),
                                ignore_if_exists: Some(true),
                            }),
                            annotation_id: None,
                        })),
                    ])),
                    ..Default::default()
                }),
                is_preferred: Some(true),
                ..Default::default()
            }));
        } else if let Some(rest) = msg.strip_prefix('`')
            && let Some((name, alias)) = rest.split_once("` is not public in module `")
        {
            // `pub` on the declaration in the imported module
            let alias = alias.trim_end_matches('`');
            if let Some(path) = uri_to_path(uri)
                && let Some(mpath) = self.analyses.get(&path).and_then(|a| a.imports.get(alias))
                && let Some(mtext) = self.text_of(mpath)
            {
                let (m, _) = crate::parser::parse_module_recover(&mtext, 0);
                let start = (m.items.iter())
                    .find(|it| it.declared().0.iter().any(|n| *n == name))
                    .map(|it| it.range().0);
                if let Some(start) = start {
                    // the declaration's line, after its indentation
                    let ls = line_start(&mtext, start as usize);
                    let at = ls + mtext[ls..].len() - mtext[ls..].trim_start().len();
                    out.push(edit_action(
                        format!("Make `{name}` public"),
                        CodeActionKind::QUICKFIX,
                        vec![(
                            path_to_uri(mpath),
                            TextEdit::new(range(&mtext, at, at), "pub ".into()),
                        )],
                        Some(d),
                    ));
                }
            }
        } else if msg.starts_with("missing field `") && msg.contains("in pattern") {
            out.extend(self.fill_pattern_fields(uri, module, text, off, d));
        }
        out
    }

    /// `.field = _` for every field a struct pattern leaves out.
    fn fill_pattern_fields(
        &self,
        uri: &Uri,
        module: &ast::Module,
        text: &str,
        off: u32,
        d: &Diagnostic,
    ) -> Option<CodeActionOrCommand> {
        let a = self.analyses.get(&uri_to_path(uri)?)?;
        let edit = super::actions::add_missing_fields(a, module, text, off, |_| "_".into())?;
        Some(edit_action(
            "Fill in the missing fields with `_`".into(),
            CodeActionKind::QUICKFIX,
            vec![(uri.clone(), edit)],
            Some(d),
        ))
    }
}

/// `//` <-> `///` for the run of comment lines at the cursor.
fn convert_comment(uri: &Uri, text: &str, off: usize) -> Option<CodeActionOrCommand> {
    let comments = crate::lexer::comments(text);
    let i = comments.iter().position(|&(s, e)| s <= off && off <= e)?;
    let is_doc = |k: usize| text[comments[k].0..].starts_with("///");
    // only whole-line comments
    let own_line = |k: usize| {
        text[line_start(text, comments[k].0)..comments[k].0]
            .trim()
            .is_empty()
    };
    if !own_line(i) {
        return None;
    }
    let doc = is_doc(i);
    let adjacent = |a: usize, b: usize| {
        own_line(b)
            && is_doc(b) == doc
            && text[comments[a].1..comments[b].0].matches('\n').count() == 1
    };
    let mut lo = i;
    while lo > 0 && adjacent(lo - 1, lo) {
        lo -= 1;
    }
    let mut hi = i;
    while hi + 1 < comments.len() && adjacent(hi, hi + 1) {
        hi += 1;
    }
    let (len, new) = if doc { (3, "//") } else { (2, "///") };
    let edits = (lo..=hi)
        .map(|k| {
            let s = comments[k].0;
            (
                uri.clone(),
                TextEdit::new(range(text, s, s + len), new.into()),
            )
        })
        .collect();
    let title = if doc {
        "Convert to a regular comment"
    } else {
        "Convert to a doc comment"
    };
    Some(edit_action(
        title.into(),
        CodeActionKind::REFACTOR_REWRITE,
        edits,
        None,
    ))
}

/// Rewrite the integer literal at the cursor in the other bases.
fn convert_int(uri: &Uri, text: &str, s: usize, e: usize) -> Vec<CodeActionOrCommand> {
    let (toks, _) = crate::lexer::lex_all(text, 0, 0);
    let Some(t) = toks.iter().find(|t| {
        matches!(t.tok, Tok::Int(..)) && t.span.start as usize <= s && e <= t.span.end as usize
    }) else {
        return vec![];
    };
    let Tok::Int(v, unsigned) = t.tok else {
        return vec![];
    };
    let (ts, te) = (t.span.start as usize, t.span.end as usize);
    let cur = &text[ts..te];
    [
        (10, "decimal"),
        (16, "hexadecimal"),
        (8, "octal"),
        (2, "binary"),
    ]
    .into_iter()
    .map(|(b, name)| (int_in_base(v, b, unsigned), name))
    .filter(|(lit, _)| lit != cur)
    .map(|(lit, name)| {
        edit_action(
            format!("Convert to {name} (`{lit}`)"),
            CodeActionKind::REFACTOR_REWRITE,
            vec![(uri.clone(), TextEdit::new(range(text, ts, te), lit))],
            None,
        )
    })
    .collect()
}

/// Turn the selected text inside a string literal into an interpolation: `"a b"` -> `"a {b}"`.
fn interpolate(uri: &Uri, text: &str, s: usize, e: usize) -> Option<CodeActionOrCommand> {
    if s >= e {
        return None;
    }
    let sel = &text[s..e];
    if sel.contains(['"', '{', '}', '\n']) || sel.trim().is_empty() {
        return None;
    }
    let (toks, _) = crate::lexer::lex_all(text, 0, 0);
    let t = toks.iter().find(|t| {
        matches!(t.tok, Tok::Str(_)) && (t.span.start as usize) < s && e < t.span.end as usize
    })?;
    // not inside an existing interpolation
    if let Tok::Str(parts) = &t.tok
        && parts.iter().any(|p| {
            matches!(p, crate::lexer::StrPart::Expr(src, off) if *off <= e && s <= off + src.len())
        })
    {
        return None;
    }
    Some(edit_action(
        format!("Interpolate `{}`", sel.trim()),
        CodeActionKind::REFACTOR_REWRITE,
        vec![(
            uri.clone(),
            TextEdit::new(range(text, s, e), format!("{{{}}}", sel.trim())),
        )],
        None,
    ))
}

/// Extract a selected expression that uses no local variables into a module-level variable.
fn extract_constant(
    a: &Analysis,
    fid: u32,
    module: &ast::Module,
    uri: &Uri,
    text: &str,
    s: usize,
    e: usize,
) -> Option<CodeActionOrCommand> {
    if s >= e {
        return None;
    }
    let (s32, e32) = (s as u32, e as u32);
    let x = a.expr_ty(fid, s32, e32)?;
    if matches!(x.ty.as_str(), "void" | "never")
        || x.ty.starts_with("fn")
        || x.ty.starts_with("fut")
    {
        return None;
    }
    // only inside a function, test or block (at the top level it is "extract variable")
    let (lo, _) = item_extent(module, s32)?;
    let top_stmt = module.items.iter().any(|it| {
        matches!(it, ast::Item::Stmt(st) if st.span.start == lo && !matches!(st.kind, S::Fn(_)))
    });
    if top_stmt {
        return None;
    }
    // every name used must be module-level (or a function or type)
    for r in &a.index.refs {
        if r.span.file != fid || r.span.start < s32 || r.span.end > e32 {
            continue;
        }
        let d = r.def.and_then(|def| a.decl(def));
        if d.is_some_and(|d| d.scope.is_some()) {
            return None;
        }
    }
    let taken: HashSet<String> = a.index.decls.iter().map(|d| d.name.clone()).collect();
    let mut name = "CONSTANT".to_string();
    let mut k = 2;
    while taken.contains(&name) {
        name = format!("CONSTANT_{k}");
        k += 1;
    }
    let at = line_start(text, lo as usize);
    let decl = format!("{} {name} = {}\n\n", x.ty, &text[s..e]);
    Some(edit_action(
        "Extract into a module-level constant".into(),
        CodeActionKind::REFACTOR_EXTRACT,
        vec![
            (uri.clone(), TextEdit::new(range(text, at, at), decl)),
            (uri.clone(), TextEdit::new(range(text, s, e), name)),
        ],
        None,
    ))
}

/// "Match on the result": an expression statement of an enum type becomes an `if` over its
/// variants.
fn match_on_call(
    a: &Analysis,
    fid: u32,
    module: &ast::Module,
    uri: &Uri,
    text: &str,
    off: usize,
) -> Option<CodeActionOrCommand> {
    let off = off as u32;
    let mut target: Option<&ast::Expr> = None;
    walk(module, &mut |n| {
        if let Node::Stmt(st) = n
            && let S::Expr(x) = &st.kind
            && matches!(x.kind, E::Call(..))
            && x.span.start <= off
            && off <= x.span.end
        {
            target = Some(x);
        }
    });
    let x = target?;
    let ty = a.expr_ty(fid, x.span.start, x.span.end)?;
    let edecl = a.decl(ty.type_def?)?;
    if edecl.kind != DeclKind::Enum || ty.ty != edecl.name {
        return None;
    }
    let ls = line_start(text, x.span.start as usize);
    let indent = &text[ls..x.span.start as usize];
    if !indent.trim().is_empty() {
        return None;
    }
    let subject = &text[x.span.start as usize..x.span.end as usize];
    let body = enum_match(a, fid, edecl, subject, indent)?;
    let body = body.trim_start().trim_end_matches('\n').to_string();
    Some(edit_action(
        "Match on the result".into(),
        CodeActionKind::REFACTOR_REWRITE,
        vec![(
            uri.clone(),
            TextEdit::new(
                range(text, x.span.start as usize, x.span.end as usize),
                body,
            ),
        )],
        None,
    ))
}

/// `if <subject> {` with an empty branch per variant of the enum `edecl`, each line (the first
/// included) indented by `indent`.
pub(super) fn enum_match(
    a: &Analysis,
    fid: u32,
    edecl: &SymDecl,
    subject: &str,
    indent: &str,
) -> Option<String> {
    // the enum as written from this file
    let prefix = if edecl.span.file == fid {
        edecl.name.clone()
    } else {
        let epath = &a.sm.file(edecl.span.file).path;
        let alias = a.imports.iter().find(|(_, p)| *p == epath)?.0;
        format!("{alias}.{}", edecl.name)
    };
    let mut body = format!("{indent}if {subject} {{\n");
    for (m, detail) in &edecl.members {
        let pat = variant_pattern(&prefix, m, detail);
        writeln!(body, "{indent}  {pat} -> {{}}").unwrap();
    }
    writeln!(body, "{indent}}}").unwrap();
    Some(body)
}

/// A pattern matching variant `m` of the enum written `prefix`, with `_` for each value of its
/// payload (`detail` is the variant's signature, `Shape.Circle(float)`).
pub(super) fn variant_pattern(prefix: &str, m: &str, detail: &str) -> String {
    let arity = detail
        .split_once('(')
        .map_or(0, |(_, ps)| ps.trim_end_matches(')').split(',').count());
    if arity == 0 {
        format!("{prefix}.{m}")
    } else {
        format!("{prefix}.{m}({})", vec!["_"; arity].join(", "))
    }
}

// ------------------------------------------------------------------ exit points

/// Ranges to highlight for a `fn`, `return`, `throw` or `try` keyword at `off`: the function's
/// keyword and every way out of it (returns, throws, `try`s), as rust-analyzer does.
pub fn exit_points(module: &ast::Module, text: &str, off: usize) -> Option<Vec<Range>> {
    let (s, e) = super::word_at(text, off)?;
    let word = &text[s..e];
    if !matches!(word, "fn" | "return" | "throw" | "try") {
        return None;
    }
    // every function: (keyword offset, body)
    let mut fns: Vec<(usize, &ast::Block)> = Vec::new();
    let fn_kw = |sig_start: u32| text[..sig_start as usize].rfind("fn").unwrap_or(0);
    for it in &module.items {
        if let ast::Item::Fn(f) = it {
            fns.push((fn_kw(f.sig.span.start), &f.body));
        }
    }
    walk(module, &mut |n| match n {
        Node::Stmt(st) => {
            if let S::Fn(f) = &st.kind {
                fns.push((fn_kw(f.sig.span.start), &f.body));
            }
        }
        Node::Expr(x) => {
            if let E::FnLit(_, b) = &x.kind {
                fns.push((x.span.start as usize, b));
            }
        }
        _ => {}
    });
    // the function the keyword belongs to: the one it opens, or the innermost around it
    let (kw, body) = if word == "fn" {
        *fns.iter().find(|(k, _)| *k == s)?
    } else {
        *fns.iter()
            .filter(|(_, b)| b.span.start as usize <= s && s < b.span.end as usize)
            .max_by_key(|(_, b)| b.span.start)?
    };
    let nested: Vec<(u32, u32)> = fns
        .iter()
        .filter(|(k, b)| *k != kw && body.span.start < b.span.start && b.span.end <= body.span.end)
        .map(|(_, b)| (b.span.start, b.span.end))
        .collect();
    let inside_nested = |p: u32| nested.iter().any(|&(a, b)| a <= p && p < b);
    let mut out = vec![range(text, kw, kw + 2)];
    let mut push_kw = |p: u32, k: &str| {
        if !inside_nested(p) && text[p as usize..].starts_with(k) {
            out.push(range(text, p as usize, p as usize + k.len()));
        }
    };
    let mut visit = |n: Node| match n {
        Node::Stmt(st) => match &st.kind {
            S::Return(_) => push_kw(st.span.start, "return"),
            S::Throw(_) => push_kw(st.span.start, "throw"),
            _ => {}
        },
        Node::Expr(x) => {
            if let E::Try(_) = &x.kind {
                push_kw(x.span.start, "try");
            }
        }
        _ => {}
    };
    super::nav::block_spans(body, &mut visit);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_bases() {
        assert_eq!(int_in_base(255, 16, false), "0xFF");
        assert_eq!(int_in_base(5, 2, true), "0b101u");
        assert_eq!(int_in_base(8, 8, false), "0o10");
    }

    #[test]
    fn exit_point_highlights() {
        let src = "fn f(int x) int! {\n  fn g() int {\n    return 1\n  }\n  if {\n    x > 1 -> { throw \"no\" }\n    _ -> {}\n  }\n  return try h()\n}\n";
        let (m, _) = crate::parser::parse_module_recover(src, 0);
        let r = exit_points(&m, src, 0).unwrap();
        let lines: Vec<u32> = r.iter().map(|r| r.start.line).collect();
        // `fn`, `throw`, `return`, `try` -- not the nested function's `return`
        assert_eq!(lines, [0, 5, 8, 8]);
    }
}
