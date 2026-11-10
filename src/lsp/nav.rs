//! Type definition, call hierarchy, selection ranges and document links.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lsp_types::*;

use super::{
    Analysis, Server, name_span, offset_to_pos, path_to_uri, pos_to_offset, span_range, uri_to_path,
};
use crate::ast::{self, ExprKind as E, StmtKind as S};
use crate::check::index::DeclKind;
use crate::diag::Span;

impl Server {
    pub(super) fn type_definition(
        &self,
        p: GotoDefinitionParams,
    ) -> Option<GotoDefinitionResponse> {
        let pp = p.text_document_position_params;
        let target = match self.lookup(&pp.text_document.uri, pp.position) {
            Some((a, r)) => r.type_def.map(|d| (a, d)),
            None => None,
        };
        let (a, def) = match target {
            Some(t) => t,
            None => {
                let (a, fid, text) = self.fresh(&pp.text_document.uri)?;
                let off = pos_to_offset(text, pp.position) as u32;
                let e = a
                    .index
                    .exprs
                    .iter()
                    .filter(|e| e.span.file == fid && e.span.start <= off && off < e.span.end)
                    .filter(|e| e.type_def.is_some())
                    .min_by_key(|e| e.span.end - e.span.start)?;
                (a, e.type_def?)
            }
        };
        let f = a.sm.file(def.file);
        let span = a
            .decl(def)
            .and_then(|d| name_span(&f.text, def, &d.name, true))
            .unwrap_or(def);
        Some(GotoDefinitionResponse::Scalar(Location::new(
            path_to_uri(&f.path),
            span_range(&f.text, span),
        )))
    }

    // ------------------------------------------------------------------ call hierarchy

    /// The function declared at `def` in an analysis, as a call-hierarchy item.
    fn hierarchy_item(a: &Analysis, def: Span) -> Option<CallHierarchyItem> {
        let d = a.decl(def)?;
        if d.kind != DeclKind::Function {
            return None;
        }
        let f = a.sm.file(def.file);
        let sel = name_span(&f.text, def, &d.name, true).unwrap_or(def);
        Some(CallHierarchyItem {
            name: d.name.clone(),
            kind: SymbolKind::FUNCTION,
            tags: None,
            detail: Some(d.detail.clone()),
            uri: path_to_uri(&f.path),
            range: span_range(&f.text, def),
            selection_range: span_range(&f.text, sel),
            data: Some(serde_json::json!({
                "path": f.path.to_string_lossy(),
                "start": def.start,
            })),
        })
    }

    /// The function a call-hierarchy item stands for: its file and signature offset.
    fn item_key(item: &CallHierarchyItem) -> Option<(PathBuf, u32)> {
        let d = item.data.as_ref()?;
        Some((
            PathBuf::from(d["path"].as_str()?),
            d["start"].as_u64()? as u32,
        ))
    }

    pub(super) fn prepare_call_hierarchy(
        &self,
        p: CallHierarchyPrepareParams,
    ) -> Option<Vec<CallHierarchyItem>> {
        let pp = p.text_document_position_params;
        let (a, r) = self.lookup(&pp.text_document.uri, pp.position)?;
        Some(vec![Self::hierarchy_item(a, r.def?)?])
    }

    pub(super) fn incoming_calls(
        &self,
        p: CallHierarchyIncomingCallsParams,
    ) -> Option<Vec<CallHierarchyIncomingCall>> {
        let key = Self::item_key(&p.item)?;
        // caller (file, start) -> (item, call ranges)
        let mut by_caller: BTreeMap<(PathBuf, u32), (CallHierarchyItem, Vec<Range>)> =
            BTreeMap::new();
        let mut seen = std::collections::HashSet::new();
        for a in self.analyses.values() {
            for c in &a.index.calls {
                let cf = a.sm.file(c.callee.file);
                if (cf.path.clone(), c.callee.start) != key {
                    continue;
                }
                let at_file = a.sm.file(c.at.file);
                if !seen.insert((at_file.path.clone(), c.at.start)) {
                    continue;
                }
                let (ck, item) = match c
                    .caller
                    .and_then(|s| Some((s, Self::hierarchy_item(a, s)?)))
                {
                    Some((s, item)) => ((a.sm.file(s.file).path.clone(), s.start), item),
                    None => {
                        // the top level of a file (or a test)
                        let path = at_file.path.clone();
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        let whole = Range::new(
                            Position::new(0, 0),
                            offset_to_pos(&at_file.text, at_file.text.len()),
                        );
                        let item = CallHierarchyItem {
                            name,
                            kind: SymbolKind::FILE,
                            tags: None,
                            detail: Some("top level".into()),
                            uri: path_to_uri(&path),
                            range: whole,
                            selection_range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                            data: None,
                        };
                        ((path, u32::MAX), item)
                    }
                };
                by_caller
                    .entry(ck)
                    .or_insert_with(|| (item, vec![]))
                    .1
                    .push(span_range(&at_file.text, c.at));
            }
        }
        Some(
            by_caller
                .into_values()
                .map(|(from, from_ranges)| CallHierarchyIncomingCall { from, from_ranges })
                .collect(),
        )
    }

    pub(super) fn outgoing_calls(
        &self,
        p: CallHierarchyOutgoingCallsParams,
    ) -> Option<Vec<CallHierarchyOutgoingCall>> {
        let key = Self::item_key(&p.item)?;
        let mut by_callee: BTreeMap<(PathBuf, u32), (CallHierarchyItem, Vec<Range>)> =
            BTreeMap::new();
        let mut seen = std::collections::HashSet::new();
        for a in self.analyses.values() {
            for c in &a.index.calls {
                let Some(caller) = c.caller else { continue };
                if (a.sm.file(caller.file).path.clone(), caller.start) != key {
                    continue;
                }
                let at_file = a.sm.file(c.at.file);
                if !seen.insert((at_file.path.clone(), c.at.start)) {
                    continue;
                }
                let Some(item) = Self::hierarchy_item(a, c.callee) else {
                    continue;
                };
                let ck = (a.sm.file(c.callee.file).path.clone(), c.callee.start);
                by_callee
                    .entry(ck)
                    .or_insert_with(|| (item, vec![]))
                    .1
                    .push(span_range(&at_file.text, c.at));
            }
        }
        Some(
            by_callee
                .into_values()
                .map(|(to, from_ranges)| CallHierarchyOutgoingCall { to, from_ranges })
                .collect(),
        )
    }

    // ------------------------------------------------------------------ selection ranges

    pub(super) fn selection_ranges(&self, p: SelectionRangeParams) -> Option<Vec<SelectionRange>> {
        let path = uri_to_path(&p.text_document.uri)?;
        let text = self.text_of(&path)?;
        let (module, _) = crate::parser::parse_module_recover(&text, 0);
        let mut spans = Vec::new();
        walk(&module, &mut |n| {
            if let Node::Span(s) = n {
                spans.push(s);
            }
        });
        spans.push(Span::new(0, 0, text.len()));
        let mut out = Vec::new();
        for pos in p.positions {
            let off = pos_to_offset(&text, pos) as u32;
            let mut chain: Vec<Span> = spans
                .iter()
                .filter(|s| s.start <= off && off <= s.end && s.end > s.start)
                .copied()
                .collect();
            if let Some((s, e)) = super::word_at(&text, off as usize) {
                chain.push(Span::new(0, s, e));
            }
            // innermost first; equal ranges collapse
            chain.sort_by_key(|s| std::cmp::Reverse(s.end - s.start));
            chain.dedup_by_key(|s| (s.start, s.end));
            let mut parent: Option<Box<SelectionRange>> = None;
            for s in chain {
                parent = Some(Box::new(SelectionRange {
                    range: span_range(&text, s),
                    parent,
                }));
            }
            out.push(parent.map(|b| *b).unwrap_or(SelectionRange {
                range: Range::new(pos, pos),
                parent: None,
            }));
        }
        Some(out)
    }

    // ------------------------------------------------------------------ document links

    pub(super) fn document_links(&self, p: DocumentLinkParams) -> Option<Vec<DocumentLink>> {
        let path = uri_to_path(&p.text_document.uri)?;
        let text = self.text_of(&path)?;
        let (module, _) = crate::parser::parse_module_recover(&text, 0);
        let dir = path.parent()?.to_path_buf();
        let imports = self.analyses.get(&path).map(|a| &a.imports);
        let mut out = Vec::new();
        let mut link = |span: Span, lit: &str, target: Option<PathBuf>| {
            let Some(target) = target.filter(|t| t.is_file()) else {
                return;
            };
            let quoted = format!("\"{lit}\"");
            let seg = &text[span.start as usize..(span.end as usize).min(text.len())];
            if let Some(i) = seg.find(&quoted) {
                let s = span.start as usize + i + 1;
                out.push(DocumentLink {
                    range: Range::new(offset_to_pos(&text, s), offset_to_pos(&text, s + lit.len())),
                    target: Some(path_to_uri(&target)),
                    tooltip: Some(target.display().to_string()),
                    data: None,
                });
            }
        };
        for it in &module.items {
            match it {
                ast::Item::Import(im) => {
                    let target = imports.and_then(|m| m.get(&im.alias)).cloned();
                    link(im.span, &im.path, target);
                }
                ast::Item::Extern(e) => link(e.span, &e.path, Some(dir.join(&e.path))),
                _ => {}
            }
        }
        // `@embed("./file")`
        let mut from = 0;
        while let Some(i) = text[from..].find("@embed(\"") {
            let s = from + i + "@embed(\"".len();
            let Some(len) = text[s..].find('"') else {
                break;
            };
            let lit = &text[s..s + len];
            if let Some(target) = resolve_relative(&dir, lit) {
                out.push(DocumentLink {
                    range: Range::new(offset_to_pos(&text, s), offset_to_pos(&text, s + len)),
                    target: Some(path_to_uri(&target)),
                    tooltip: Some(target.display().to_string()),
                    data: None,
                });
            }
            from = s + len;
        }
        Some(out)
    }
}

/// Resolve `./x`, `../x` or `.../x` (from the project root) like `@embed` does.
pub fn resolve_relative(dir: &Path, path: &str) -> Option<PathBuf> {
    let p = if let Some(rel) = path.strip_prefix(".../") {
        crate::modules::find_root(dir)?.join(rel)
    } else if path.starts_with("./") || path.starts_with("../") {
        dir.join(path.strip_prefix("./").unwrap_or(path))
    } else {
        return None;
    };
    p.is_file().then_some(p)
}

// ------------------------------------------------------------------ syntax tree walk

/// A node of the syntax tree, visited by `walk`. `Span` is reported for every node.
#[derive(Clone, Copy)]
pub enum Node<'a> {
    Span(Span),
    Stmt(&'a ast::Stmt),
    Expr(&'a ast::Expr),
    Block(&'a ast::Block),
    /// a function (declared or literal): its signature
    Fn(&'a ast::FnSig),
}

/// Visit every node of a module.
pub fn walk<'a>(m: &'a ast::Module, out: &mut dyn FnMut(Node<'a>)) {
    for it in &m.items {
        item_spans(it, out);
    }
}

pub fn item_spans<'a>(it: &'a ast::Item, out: &mut dyn FnMut(Node<'a>)) {
    match it {
        ast::Item::Import(im) => out(Node::Span(im.span)),
        ast::Item::Extern(e) => {
            out(Node::Span(e.span));
            for f in &e.fns {
                sig_spans(&f.sig, out);
            }
        }
        ast::Item::Struct(s) => {
            out(Node::Span(s.span));
            for (t, _, sp) in &s.fields {
                out(Node::Span(*sp));
                type_spans(t, out);
            }
        }
        ast::Item::Enum(e) => {
            out(Node::Span(e.span));
            for (_, ts, sp) in &e.variants {
                out(Node::Span(*sp));
                for t in ts {
                    type_spans(t, out);
                }
            }
        }
        ast::Item::Type(t) => {
            out(Node::Span(t.span));
            type_spans(&t.ty, out);
        }
        ast::Item::Fn(f) => fn_spans(f, out),
        ast::Item::Test(_, b, sp) => {
            out(Node::Span(*sp));
            block_spans(b, out);
        }
        ast::Item::Stmt(s) => stmt_spans(s, out),
    }
}

fn fn_spans<'a>(f: &'a ast::FnDecl, out: &mut dyn FnMut(Node<'a>)) {
    out(Node::Fn(&f.sig));
    out(Node::Span(f.sig.span.to(f.body.span)));
    sig_spans(&f.sig, out);
    block_spans(&f.body, out);
}

fn sig_spans<'a>(s: &'a ast::FnSig, out: &mut dyn FnMut(Node<'a>)) {
    out(Node::Span(s.span));
    for p in &s.params {
        out(Node::Span(p.span));
        type_spans(&p.ty, out);
    }
    if let Some(r) = &s.ret {
        type_spans(r, out);
    }
}

fn type_spans<'a>(t: &'a ast::TypeExpr, out: &mut dyn FnMut(Node<'a>)) {
    out(Node::Span(t.span()));
    match t {
        ast::TypeExpr::Named { args, .. } | ast::TypeExpr::Tuple(args, _) => {
            for a in args {
                type_spans(a, out);
            }
        }
        ast::TypeExpr::Array(x, _, _)
        | ast::TypeExpr::Optional(x, _)
        | ast::TypeExpr::ErrorUnion(x, _)
        | ast::TypeExpr::Fut(x, _) => type_spans(x, out),
        ast::TypeExpr::Map(k, v, _) => {
            type_spans(k, out);
            type_spans(v, out);
        }
        ast::TypeExpr::Fn(ps, r, _) => {
            for p in ps {
                type_spans(p, out);
            }
            if let Some(r) = r {
                type_spans(r, out);
            }
        }
    }
}

pub fn block_spans<'a>(b: &'a ast::Block, out: &mut dyn FnMut(Node<'a>)) {
    out(Node::Span(b.span));
    out(Node::Block(b));
    for s in &b.stmts {
        stmt_spans(s, out);
    }
}

pub fn stmt_spans<'a>(s: &'a ast::Stmt, out: &mut dyn FnMut(Node<'a>)) {
    out(Node::Span(s.span));
    out(Node::Stmt(s));
    match &s.kind {
        S::VarDecl { targets, value, .. } => {
            for t in targets {
                out(Node::Span(t.span));
                type_spans(&t.ty, out);
            }
            expr_spans(value, out);
        }
        S::FnVar { value, .. } | S::Discard(value) | S::Expr(value) | S::Throw(value) => {
            expr_spans(value, out)
        }
        S::Assert(e) => expr_spans(e, out),
        S::Assign { target, value } => {
            expr_spans(target, out);
            expr_spans(value, out);
        }
        S::Return(v) | S::Break { value: v, .. } => {
            if let Some(v) = v {
                expr_spans(v, out);
            }
        }
        S::Continue(_) => {}
        S::While { cond, body, .. } => {
            expr_spans(cond, out);
            block_spans(body, out);
        }
        S::For {
            var_span,
            iter,
            body,
            ..
        } => {
            out(Node::Span(*var_span));
            expr_spans(iter, out);
            block_spans(body, out);
        }
        S::Lock {
            name_span, body, ..
        } => {
            out(Node::Span(*name_span));
            block_spans(body, out);
        }
        S::Block(b) => block_spans(b, out),
        S::Fn(f) => fn_spans(f, out),
    }
}

pub fn expr_spans<'a>(e: &'a ast::Expr, out: &mut dyn FnMut(Node<'a>)) {
    out(Node::Span(e.span));
    out(Node::Expr(e));
    match &e.kind {
        E::Unary(_, x) | E::Try(x) | E::Async(x) | E::Await(x) => expr_spans(x, out),
        E::Binary(_, a, b) | E::Index(a, b) => {
            expr_spans(a, out);
            expr_spans(b, out);
        }
        E::Call(c, args) => {
            expr_spans(c, out);
            for a in args {
                expr_spans(a, out);
            }
        }
        E::Generic(x, ts) => {
            expr_spans(x, out);
            for t in ts {
                type_spans(t, out);
            }
        }
        E::Builtin(_, t, args) => {
            if let Some(t) = t {
                type_spans(t, out);
            }
            for a in args {
                expr_spans(a, out);
            }
        }
        E::Field(x, _) => expr_spans(x, out),
        E::Tuple(xs) | E::Array(xs) => {
            for x in xs {
                expr_spans(x, out);
            }
        }
        E::Map(kvs) => {
            for (k, v) in kvs {
                expr_spans(k, out);
                expr_spans(v, out);
            }
        }
        E::Struct(t, fields) => {
            expr_spans(t, out);
            for (_, v, sp) in fields {
                out(Node::Span(*sp));
                expr_spans(v, out);
            }
        }
        E::FnLit(sig, b) => {
            out(Node::Fn(sig));
            sig_spans(sig, out);
            block_spans(b, out);
        }
        E::If(i) => {
            out(Node::Span(i.span));
            if let Some(s) = &i.scrutinee {
                expr_spans(s, out);
            }
            for arm in &i.arms {
                out(Node::Span(arm.span));
                for p in &arm.patterns {
                    expr_spans(p, out);
                }
                block_spans(&arm.body, out);
            }
        }
        E::Else(x, fb) => {
            expr_spans(x, out);
            match fb {
                ast::ElseFallback::Expr(y) => expr_spans(y, out),
                ast::ElseFallback::Block(b) => block_spans(b, out),
            }
        }
        E::Catch(x, _, b) => {
            expr_spans(x, out);
            block_spans(b, out);
        }
        E::Str(segs) => {
            for s in segs {
                if let ast::StrSeg::Expr(x) = s {
                    expr_spans(x, out);
                }
            }
        }
        _ => {}
    }
}
