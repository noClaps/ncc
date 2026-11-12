//! Type definition, call hierarchy, selection ranges and document links.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lsp_types::*;

use super::{
    Analysis, Server, offset_to_pos, path_to_uri, pos_to_offset, range, span_range, uri_to_path,
};
use crate::ast;
use crate::ast::visit::{Node, walk};
use crate::check::index::{Call, DeclKind};
use crate::diag::Span;

impl Server {
    pub(super) fn type_definition(
        &self,
        p: GotoDefinitionParams,
    ) -> Option<GotoDefinitionResponse> {
        let pp = p.text_document_position_params;
        let target = self
            .lookup(&pp.text_document.uri, pp.position)
            .and_then(|(a, r)| Some((a, r.type_def?)));
        let (a, def) = if let Some(t) = target {
            t
        } else {
            let (a, fid, text) = self.fresh(&pp.text_document.uri)?;
            let off = pos_to_offset(text, pp.position) as u32;
            let e = a.expr_around(fid, off, |e| e.type_def.is_some())?;
            (a, e.type_def?)
        };
        let f = a.sm.file(def.file);
        let span = a.decl(def).map_or(def, |d| d.name_span);
        Some(GotoDefinitionResponse::Scalar(Location::new(
            path_to_uri(&f.path),
            span_range(&f.text, span),
        )))
    }

    /// An import or extern alias at a position: its name and every occurrence in the file
    /// (the declaration first).
    pub(super) fn alias_at(&self, uri: &Uri, pos: Position) -> Option<(String, Vec<Range>)> {
        let (path, text) = self.source(uri)?;
        let off = pos_to_offset(&text, pos);
        let (s, e) = super::word_at(&text, off)?;
        let name = &text[s..e];
        let module = self.module_of(&path, &text);
        let decl = module.items.iter().find_map(|it| match it {
            ast::Item::Import(im) if im.alias == name => Some(im.alias_span),
            ast::Item::Extern(x) if x.alias == name => Some(x.alias_span),
            _ => None,
        })?;
        let mut out = vec![span_range(&text, decl)];
        let (toks, _) = crate::lexer::lex_all(&text, 0, 0);
        qualifier_uses(&toks, &mut |n, sp| {
            if n == name {
                out.push(span_range(&text, sp));
            }
        });
        // the cursor must be on one of them
        out.iter()
            .any(|r| super::range_contains(&text, *r, off))
            .then(|| (name.to_string(), out))
    }

    /// Import paths to update when `.nc` files are renamed.
    pub(super) fn will_rename_files(&self, p: RenameFilesParams) -> Option<WorkspaceEdit> {
        let mut changes: std::collections::HashMap<Uri, Vec<TextEdit>> = Default::default();
        for f in &p.files {
            let (Ok(old), Ok(new)) = (f.old_uri.parse::<Uri>(), f.new_uri.parse::<Uri>()) else {
                continue;
            };
            let Some(old) = uri_to_path(&old) else {
                continue;
            };
            // the new file does not exist yet: canonicalize its directory
            let Some(new) = uri_to_path(&new) else {
                continue;
            };
            let new = match (
                new.parent().and_then(|d| std::fs::canonicalize(d).ok()),
                new.file_name(),
            ) {
                (Some(d), Some(n)) => d.join(n),
                _ => new,
            };
            for (importer, a) in &self.analyses {
                let Some(text) = self.text_of(importer) else {
                    continue;
                };
                let module = self.module_of(importer, &text);
                for it in &module.items {
                    let ast::Item::Import(im) = it else { continue };
                    let Some(target) = a.imports.get(&im.alias) else {
                        continue;
                    };
                    // imports of the renamed file, and the renamed file's own imports when it
                    // moves to another directory
                    let moved = importer == &old && old.parent() != new.parent();
                    if target != &old && !moved {
                        continue;
                    }
                    let target = if target == &old { &new } else { target };
                    let dir = if importer == &old {
                        new.parent()
                    } else {
                        importer.parent()
                    };
                    let Some(dir) = dir else { continue };
                    let Some(path) = import_path(dir, target, &im.path) else {
                        continue;
                    };
                    if path == im.path {
                        continue;
                    }
                    if let Some(s) = quoted_at(&text, im.span, &im.path) {
                        changes
                            .entry(path_to_uri(importer))
                            .or_default()
                            .push(TextEdit::new(
                                Range::new(
                                    offset_to_pos(&text, s),
                                    offset_to_pos(&text, s + im.path.len()),
                                ),
                                path,
                            ));
                    }
                }
            }
        }
        (!changes.is_empty()).then(|| WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
    }

    /// "N references" above each module-level function, struct, enum and type, using the
    /// client's `editor.action.showReferences` command (as rust-analyzer does).
    pub(super) fn code_lenses(&self, p: CodeLensParams) -> Option<Vec<CodeLens>> {
        let path = uri_to_path(&p.text_document.uri)?;
        let (a, fid, text) = self.analysis(&p.text_document.uri)?;
        let mut out = Vec::new();
        for d in &a.index.decls {
            if d.span.file != fid
                || d.scope.is_some()
                || !matches!(
                    d.kind,
                    DeclKind::Function | DeclKind::Struct | DeclKind::Enum | DeclKind::Type
                )
            {
                continue;
            }
            let key = (path.clone(), d.span.start);
            let occ = self.occurrences(&key, &d.name);
            let decl_range = span_range(text, d.name_span);
            let locations: Vec<Location> = occ
                .into_iter()
                .filter(|(f, r)| !(f == &path && *r == decl_range))
                .map(|(f, r)| Location::new(path_to_uri(&f), r))
                .collect();
            let n = locations.len();
            out.push(CodeLens {
                range: decl_range,
                command: Some(Command::new(
                    format!("{n} reference{}", if n == 1 { "" } else { "s" }),
                    "editor.action.showReferences".into(),
                    Some(vec![
                        serde_json::to_value(&p.text_document.uri).ok()?,
                        serde_json::to_value(decl_range.start).ok()?,
                        serde_json::to_value(&locations).ok()?,
                    ]),
                )),
                data: None,
            });
        }
        out.sort_by_key(|l| l.range.start);
        out.dedup_by_key(|l| l.range);
        Some(out)
    }

    // ------------------------------------------------------------------ call hierarchy

    /// The function declared at `def` in an analysis, as a call-hierarchy item.
    fn hierarchy_item(a: &Analysis, def: Span) -> Option<CallHierarchyItem> {
        let d = a.decl(def)?;
        if d.kind != DeclKind::Function {
            return None;
        }
        let f = a.sm.file(def.file);
        let sel = d.name_span;
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
        let calls = self.grouped_calls(|a, c| {
            if (a.sm.file(c.callee.file).path.clone(), c.callee.start) != key {
                return None;
            }
            if let Some(s) = c.caller
                && let Some(item) = Self::hierarchy_item(a, s)
            {
                return Some(((a.sm.file(s.file).path.clone(), s.start), item));
            }
            // the top level of a file (or a test)
            let at_file = a.sm.file(c.at.file);
            let path = at_file.path.clone();
            let item = CallHierarchyItem {
                name: (path.file_name().unwrap_or_default().to_string_lossy()).into_owned(),
                kind: SymbolKind::FILE,
                tags: None,
                detail: Some("top level".into()),
                uri: path_to_uri(&path),
                range: Range::new(
                    Position::new(0, 0),
                    offset_to_pos(&at_file.text, at_file.text.len()),
                ),
                selection_range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                data: None,
            };
            Some(((path, u32::MAX), item))
        });
        Some(
            (calls.into_iter())
                .map(|(from, from_ranges)| CallHierarchyIncomingCall { from, from_ranges })
                .collect(),
        )
    }

    pub(super) fn outgoing_calls(
        &self,
        p: CallHierarchyOutgoingCallsParams,
    ) -> Option<Vec<CallHierarchyOutgoingCall>> {
        let key = Self::item_key(&p.item)?;
        let calls = self.grouped_calls(|a, c| {
            let caller = c.caller?;
            if (a.sm.file(caller.file).path.clone(), caller.start) != key {
                return None;
            }
            let ck = (a.sm.file(c.callee.file).path.clone(), c.callee.start);
            Some((ck, Self::hierarchy_item(a, c.callee)?))
        });
        Some(
            (calls.into_iter())
                .map(|(to, from_ranges)| CallHierarchyOutgoingCall { to, from_ranges })
                .collect(),
        )
    }

    /// The call sites of every analysis, each once, grouped by the item (with its sort key)
    /// that `group` gives them; `None` leaves a call out.
    fn grouped_calls(
        &self,
        group: impl Fn(&Analysis, &Call) -> Option<((PathBuf, u32), CallHierarchyItem)>,
    ) -> Vec<(CallHierarchyItem, Vec<Range>)> {
        let mut groups: BTreeMap<(PathBuf, u32), (CallHierarchyItem, Vec<Range>)> = BTreeMap::new();
        let mut seen = std::collections::HashSet::new();
        for a in self.analyses.values() {
            for c in &a.index.calls {
                let Some((key, item)) = group(a, c) else {
                    continue;
                };
                let at_file = a.sm.file(c.at.file);
                if seen.insert((at_file.path.clone(), c.at.start)) {
                    let ranges = &mut groups.entry(key).or_insert_with(|| (item, vec![])).1;
                    ranges.push(span_range(&at_file.text, c.at));
                }
            }
        }
        groups.into_values().collect()
    }

    // ------------------------------------------------------------------ selection ranges

    pub(super) fn selection_ranges(&self, p: SelectionRangeParams) -> Option<Vec<SelectionRange>> {
        let (path, text) = self.source(&p.text_document.uri)?;
        let module = self.module_of(&path, &text);
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
        let (path, text) = self.source(&p.text_document.uri)?;
        let module = self.module_of(&path, &text);
        let dir = path.parent()?.to_path_buf();
        let imports = self.analyses.get(&path).map(|a| &a.imports);
        let mut out = Vec::new();
        for it in &module.items {
            let (span, lit, target) = match it {
                ast::Item::Import(im) => (
                    im.span,
                    &im.path,
                    imports.and_then(|m| m.get(&im.alias)).cloned(),
                ),
                ast::Item::Extern(e) => (e.span, &e.path, Some(dir.join(&e.path))),
                _ => continue,
            };
            if let Some(target) = target.filter(|t| t.is_file())
                && let Some(s) = quoted_at(&text, span, lit)
            {
                out.push(link(&text, s, s + lit.len(), &target));
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
                out.push(link(&text, s, s + len, &target));
            }
            from = s + len;
        }
        Some(out)
    }
}

/// The import path of `target` from a file in `dir`, in the style of the old path: relative
/// (`./x`, `../x`) or from the project root (`.../x`). Dependency imports are left alone.
fn import_path(dir: &Path, target: &Path, old: &str) -> Option<String> {
    let stem = target.with_extension("");
    if old.starts_with(".../") {
        let root = crate::modules::find_root(dir)?;
        let rel = stem.strip_prefix(&root).ok()?;
        return Some(format!(".../{}", rel.to_string_lossy()));
    }
    if !(old.starts_with("./") || old.starts_with("../")) {
        return None;
    }
    let from: Vec<_> = dir.components().collect();
    let to: Vec<_> = stem.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".into(); from.len() - common];
    parts.extend(
        to[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    let rel = parts.join("/");
    Some(if rel.starts_with("..") {
        rel
    } else {
        format!("./{rel}")
    })
}

/// Every name used as a qualifier (`name.`) in tokens, including in string interpolations:
/// the name and its span.
pub(super) fn qualifier_uses(toks: &[crate::lexer::Token], out: &mut dyn FnMut(&str, Span)) {
    use crate::lexer::{StrPart, Tok};
    for (i, t) in toks.iter().enumerate() {
        match &t.tok {
            Tok::Ident(n) if matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Dot)) => {
                out(n, t.span);
            }
            Tok::Str(parts) => {
                for p in parts {
                    if let StrPart::Expr(src, off) = p {
                        qualifier_uses(&crate::lexer::lex_all(src, 0, *off).0, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The offset of `lit` written as a quoted string inside `span`.
fn quoted_at(text: &str, span: Span, lit: &str) -> Option<usize> {
    let seg = &text[span.start as usize..(span.end as usize).min(text.len())];
    seg.find(&format!("\"{lit}\""))
        .map(|i| span.start as usize + i + 1)
}

fn link(text: &str, s: usize, e: usize, target: &Path) -> DocumentLink {
    DocumentLink {
        range: range(text, s, e),
        target: Some(path_to_uri(target)),
        tooltip: Some(target.display().to_string()),
        data: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_paths() {
        let p = |d: &str, t: &str, o: &str| import_path(Path::new(d), Path::new(t), o);
        assert_eq!(p("/a/b", "/a/b/c.nc", "./x").as_deref(), Some("./c"));
        assert_eq!(p("/a/b", "/a/lib/c.nc", "./x").as_deref(), Some("../lib/c"));
        assert_eq!(
            p("/a/b", "/a/b/sub/c.nc", "../x").as_deref(),
            Some("./sub/c")
        );
        assert_eq!(p("/a/b", "/a/b/c.nc", "std/x"), None);
    }
}
