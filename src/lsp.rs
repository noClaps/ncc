//! `ncc lsp`: a language server over stdio.
//!
//! Features: diagnostics (errors and warnings), hover, go-to-definition, document symbols
//! and completion. Every open document is analysed as an entry file, using the in-memory
//! contents of all open documents.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, Request as _};
use lsp_types::*;

use crate::ast;
use crate::check::index::{DeclKind, Index};
use crate::diag::{Diag, SourceMap, Span};

const KEYWORDS: &[&str] = &[
    "fn", "return", "if", "else", "while", "for", "in", "break", "continue", "mut", "mutex", "lock", "struct",
    "enum", "type", "import", "as", "extern", "pub", "test", "assert", "true", "false", "none", "and", "or", "not",
    "try", "catch", "throw", "async", "await", "fut",
];
const TYPES: &[&str] = &["int", "uint", "byte", "float", "bool", "char", "str", "void", "error", "never"];
const BUILTINS: &[(&str, &str)] = &[
    ("print", "fn @print(...)"),
    ("println", "fn @println(...)"),
    ("eprint", "fn @eprint(...)"),
    ("eprintln", "fn @eprintln(...)"),
    ("as", "fn @as(type T, value) T"),
];

/// Result of analysing one entry document.
struct Analysis {
    sm: SourceMap,
    index: Index,
    /// import alias -> module path, from the last successful load
    imports: HashMap<String, PathBuf>,
}

#[derive(Default)]
struct Server {
    docs: HashMap<PathBuf, String>,
    analyses: HashMap<PathBuf, Analysis>,
    /// files that received diagnostics from each entry, to clear them later
    published: HashMap<PathBuf, HashSet<PathBuf>>,
}

pub fn run() -> ExitCode {
    let (connection, io_threads) = Connection::stdio();
    let caps = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::FULL),
            save: Some(TextDocumentSyncSaveOptions::Supported(true)),
            ..Default::default()
        })),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".into(), "@".into()]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let init = serde_json::json!({
        "capabilities": caps,
        "serverInfo": { "name": "ncc", "version": env!("CARGO_PKG_VERSION") },
    });
    let r = (|| -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
        let (id, _params) = connection.initialize_start()?;
        connection.initialize_finish(id, init)?;
        let mut server = Server::default();
        for msg in &connection.receiver {
            match msg {
                Message::Request(req) => {
                    if connection.handle_shutdown(&req)? {
                        break;
                    }
                    let resp = server.request(req);
                    connection.sender.send(Message::Response(resp))?;
                }
                Message::Notification(n) => {
                    for out in server.notification(n) {
                        connection.sender.send(Message::Notification(out))?;
                    }
                }
                Message::Response(_) => {}
            }
        }
        Ok(())
    })();
    drop(connection);
    let _ = io_threads.join();
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: language server failed: {e}");
            ExitCode::FAILURE
        }
    }
}

// ------------------------------------------------------------------ URIs and positions

fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let s = uri.as_str();
    let rest = s.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        out.push(bytes[i]);
        i += 1;
    }
    let p = PathBuf::from(String::from_utf8(out).ok()?);
    Some(std::fs::canonicalize(&p).unwrap_or(p))
}

fn path_to_uri(p: &Path) -> Uri {
    let mut s = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => s.push(b as char),
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s.parse().expect("valid file URI")
}

/// Byte offset -> LSP position (UTF-16 columns).
fn offset_to_pos(text: &str, offset: usize) -> Position {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count();
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col: usize = text[line_start..offset].encode_utf16().count();
    Position::new(line as u32, col as u32)
}

/// LSP position -> byte offset.
fn pos_to_offset(text: &str, pos: Position) -> usize {
    let mut off = 0;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        if i == pos.line as usize {
            let mut units = 0u32;
            for (bi, ch) in line.char_indices() {
                if units >= pos.character || ch == '\n' {
                    return off + bi;
                }
                units += ch.len_utf16() as u32;
            }
            return off + line.trim_end_matches('\n').len();
        }
        off += line.len();
    }
    text.len()
}

fn span_range(text: &str, span: Span) -> Range {
    Range::new(offset_to_pos(text, span.start as usize), offset_to_pos(text, span.end as usize))
}

impl Server {
    fn text_of(&self, path: &Path) -> Option<String> {
        self.docs.get(path).cloned().or_else(|| std::fs::read_to_string(path).ok())
    }

    // ------------------------------------------------------------------ notifications

    fn notification(&mut self, n: Notification) -> Vec<Notification> {
        let path = match n.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(n.params) else { return vec![] };
                let Some(path) = uri_to_path(&p.text_document.uri) else { return vec![] };
                self.docs.insert(path.clone(), p.text_document.text);
                path
            }
            DidChangeTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(n.params) else { return vec![] };
                let Some(path) = uri_to_path(&p.text_document.uri) else { return vec![] };
                if let Some(change) = p.content_changes.into_iter().last() {
                    self.docs.insert(path.clone(), change.text);
                }
                path
            }
            DidSaveTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidSaveTextDocumentParams>(n.params) else { return vec![] };
                let Some(path) = uri_to_path(&p.text_document.uri) else { return vec![] };
                path
            }
            DidCloseTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(n.params) else { return vec![] };
                let Some(path) = uri_to_path(&p.text_document.uri) else { return vec![] };
                self.docs.remove(&path);
                self.analyses.remove(&path);
                // clear diagnostics owned by this entry
                let mut out = Vec::new();
                for f in self.published.remove(&path).unwrap_or_default() {
                    out.push(publish(&f, vec![]));
                }
                return out;
            }
            _ => return vec![],
        };
        // re-analyse every open document (imports may have changed)
        let open: Vec<PathBuf> = self.docs.keys().cloned().collect();
        let mut out = Vec::new();
        let mut order = vec![path.clone()];
        order.extend(open.into_iter().filter(|p| *p != path));
        let mut by_file: HashMap<PathBuf, Vec<Diagnostic>> = HashMap::new();
        for entry in order {
            let diags = self.analyze(&entry);
            let mut files: HashSet<PathBuf> = HashSet::new();
            for (file, d) in diags {
                files.insert(file.clone());
                by_file.entry(file).or_default().push(d);
            }
            files.insert(entry.clone());
            let old = self.published.insert(entry, files.clone()).unwrap_or_default();
            for f in old.difference(&files) {
                by_file.entry(f.clone()).or_default();
            }
            for f in files {
                by_file.entry(f).or_default();
            }
        }
        for (file, mut diags) in by_file {
            diags.dedup_by(|a, b| a.range == b.range && a.message == b.message);
            out.push(publish(&file, diags));
        }
        out
    }

    /// Analyse an entry file; returns diagnostics per file.
    fn analyze(&mut self, entry: &Path) -> Vec<(PathBuf, Diagnostic)> {
        let mut sm = SourceMap::default();
        let mut diags: Vec<(Diag, DiagnosticSeverity)> = Vec::new();
        let mut index = None;
        let mut imports = None;
        match crate::modules::load_with(&mut sm, entry, &self.docs) {
            Err(errs) => diags.extend(errs.into_iter().map(|d| (d, DiagnosticSeverity::ERROR))),
            Ok((graph, load_errors)) => {
                diags.extend(load_errors.into_iter().map(|d| (d, DiagnosticSeverity::ERROR)));
                let mut im = HashMap::new();
                for (alias, &m) in &graph.modules[0].imports {
                    im.insert(alias.clone(), graph.modules[m].path.clone());
                }
                imports = Some(im);
                let (mut p, errs, ix) = crate::check::check_partial(graph);
                index = Some(ix);
                let clean = errs.is_empty();
                diags.extend(errs.into_iter().map(|d| (d, DiagnosticSeverity::ERROR)));
                if let Err(errs) = crate::eval::Evaluator::run(&mut p, &sm, false) {
                    diags.extend(errs.into_iter().map(|d| (d, DiagnosticSeverity::ERROR)));
                }
                if clean {
                    for w in crate::lint::race_warnings(&p) {
                        diags.push((w, DiagnosticSeverity::WARNING));
                    }
                }
            }
        }
        let mut out = Vec::new();
        for (d, sev) in diags {
            if sm.files.is_empty() {
                continue;
            }
            let f = sm.file(d.span.file);
            out.push((
                f.path.clone(),
                Diagnostic {
                    range: span_range(&f.text, d.span),
                    severity: Some(sev),
                    source: Some("ncc".into()),
                    message: d.msg.clone(),
                    ..Default::default()
                },
            ));
        }
        // keep the previous index when the file did not parse
        match (index, self.analyses.get_mut(entry)) {
            (Some(index), _) => {
                let imports = imports.unwrap_or_default();
                self.analyses.insert(entry.to_path_buf(), Analysis { sm, index, imports });
            }
            (None, Some(a)) => {
                // refresh the text used for position mapping of the current file
                let _ = a;
            }
            (None, None) => {}
        }
        out
    }

    // ------------------------------------------------------------------ requests

    fn request(&mut self, req: Request) -> Response {
        let id = req.id.clone();
        let result = match req.method.as_str() {
            HoverRequest::METHOD => serde_json::from_value::<HoverParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.hover(p)).unwrap()),
            GotoDefinition::METHOD => serde_json::from_value::<GotoDefinitionParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.definition(p)).unwrap()),
            DocumentSymbolRequest::METHOD => serde_json::from_value::<DocumentSymbolParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.symbols(p)).unwrap()),
            Completion::METHOD => serde_json::from_value::<CompletionParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.completion(p)).unwrap()),
            _ => return method_not_found(id, &req.method),
        };
        match result {
            Some(v) => Response::new_ok(id, v),
            None => Response::new_err(id, lsp_server::ErrorCode::InvalidParams as i32, "invalid parameters".into()),
        }
    }

    /// Find the innermost indexed reference at a position.
    fn lookup(&self, uri: &Uri, pos: Position) -> Option<(&Analysis, &crate::check::index::SymRef)> {
        let path = uri_to_path(uri)?;
        let a = self.analyses.get(&path)?;
        let fid = a.sm.files.iter().position(|f| f.path == path)? as u32;
        let text = &a.sm.file(fid).text;
        let off = pos_to_offset(text, pos) as u32;
        a.index
            .refs
            .iter()
            .filter(|r| r.span.file == fid && r.span.start <= off && off <= r.span.end && r.span.end > r.span.start)
            .min_by_key(|r| r.span.end - r.span.start)
            .map(|r| (a, r))
    }

    fn hover(&self, p: HoverParams) -> Option<Hover> {
        let pp = p.text_document_position_params;
        let (a, r) = self.lookup(&pp.text_document.uri, pp.position)?;
        let text = &a.sm.file(r.span.file).text;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("```nc\n{}\n```", r.hover),
            }),
            range: Some(span_range(text, r.span)),
        })
    }

    fn definition(&self, p: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let pp = p.text_document_position_params;
        let (a, r) = self.lookup(&pp.text_document.uri, pp.position)?;
        let def = r.def?;
        let f = a.sm.file(def.file);
        Some(GotoDefinitionResponse::Scalar(Location::new(path_to_uri(&f.path), span_range(&f.text, def))))
    }

    #[allow(deprecated)]
    fn symbols(&self, p: DocumentSymbolParams) -> Option<DocumentSymbolResponse> {
        let path = uri_to_path(&p.text_document.uri)?;
        let text = self.text_of(&path)?;
        let (module, _) = crate::parser::parse_module_recover(&text, 0);
        let sym = |name: &str, kind: SymbolKind, span: Span, detail: Option<String>, children: Option<Vec<DocumentSymbol>>| {
            let range = span_range(&text, span);
            DocumentSymbol {
                name: if name.is_empty() { "_".into() } else { name.to_string() },
                detail,
                kind,
                tags: None,
                deprecated: None,
                range,
                selection_range: range,
                children,
            }
        };
        let mut out = Vec::new();
        for it in &module.items {
            match it {
                ast::Item::Fn(f) => out.push(sym(&f.sig.name, SymbolKind::FUNCTION, f.sig.span.to(f.body.span), None, None)),
                ast::Item::Struct(s) => {
                    let fields = s.fields.iter().map(|(_, n, sp)| sym(n, SymbolKind::FIELD, *sp, None, None)).collect();
                    out.push(sym(&s.name, SymbolKind::STRUCT, s.span, None, Some(fields)));
                }
                ast::Item::Enum(e) => {
                    let vs = e.variants.iter().map(|(n, _, sp)| sym(n, SymbolKind::ENUM_MEMBER, *sp, None, None)).collect();
                    out.push(sym(&e.name, SymbolKind::ENUM, e.span, None, Some(vs)));
                }
                ast::Item::Type(t) => out.push(sym(&t.name, SymbolKind::TYPE_PARAMETER, t.span, None, None)),
                ast::Item::Test(name, _, span) => out.push(sym(name, SymbolKind::METHOD, *span, Some("test".into()), None)),
                ast::Item::Extern(e) => {
                    let fs = e.fns.iter().map(|f| sym(&f.sig.name, SymbolKind::FUNCTION, f.sig.span, Some(f.symbol.clone()), None)).collect();
                    out.push(sym(&e.alias, SymbolKind::MODULE, e.span, Some(e.path.clone()), Some(fs)));
                }
                ast::Item::Import(im) => out.push(sym(&im.alias, SymbolKind::MODULE, im.span, Some(im.path.clone()), None)),
                ast::Item::Stmt(s) => {
                    if let ast::StmtKind::VarDecl { targets, mutable, .. } = &s.kind {
                        for t in targets {
                            let kind = if *mutable { SymbolKind::VARIABLE } else { SymbolKind::CONSTANT };
                            out.push(sym(&t.name, kind, t.span, None, None));
                        }
                    } else if let ast::StmtKind::FnVar { name, .. } = &s.kind {
                        out.push(sym(name, SymbolKind::FUNCTION, s.span, None, None));
                    }
                }
            }
        }
        Some(DocumentSymbolResponse::Nested(out))
    }

    fn completion(&self, p: CompletionParams) -> Option<CompletionResponse> {
        let pp = p.text_document_position;
        let path = uri_to_path(&pp.text_document.uri)?;
        let text = self.text_of(&path)?;
        let off = pos_to_offset(&text, pp.position);
        let line_start = text[..off].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let before = &text[line_start..off];
        let item = |label: &str, kind: CompletionItemKind, detail: Option<String>| CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            detail,
            ..Default::default()
        };
        let mut items = Vec::new();
        let analysis = self.analyses.get(&path);
        let fid = analysis.and_then(|a| a.sm.files.iter().position(|f| f.path == path)).map(|i| i as u32);

        // `@builtin`
        let word_start = before
            .char_indices()
            .rev()
            .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        let prefix_char = before[..word_start].chars().last();
        if prefix_char == Some('@') {
            for (n, d) in BUILTINS {
                items.push(item(n, CompletionItemKind::FUNCTION, Some(d.to_string())));
            }
            return Some(CompletionResponse::Array(items));
        }

        // `name.` member completion
        if prefix_char == Some('.') {
            let head = &before[..word_start - 1];
            let name_start = head
                .char_indices()
                .rev()
                .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            let name = &head[name_start..];
            if let Some(a) = analysis {
                if let Some(mpath) = a.imports.get(name) {
                    if let Some(mtext) = self.text_of(mpath)
                        && let Ok(m) = crate::parser::parse_module(&mtext, 0) {
                            for it in &m.items {
                                match it {
                                    ast::Item::Fn(f) if f.public => {
                                        items.push(item(&f.sig.name, CompletionItemKind::FUNCTION, None))
                                    }
                                    ast::Item::Struct(s) if s.public => {
                                        items.push(item(&s.name, CompletionItemKind::STRUCT, None))
                                    }
                                    ast::Item::Enum(e) if e.public => items.push(item(&e.name, CompletionItemKind::ENUM, None)),
                                    ast::Item::Type(t) if t.public => {
                                        items.push(item(&t.name, CompletionItemKind::TYPE_PARAMETER, None))
                                    }
                                    ast::Item::Stmt(ast::Stmt { kind: ast::StmtKind::VarDecl { public: true, targets, .. }, .. }) => {
                                        for t in targets {
                                            items.push(item(&t.name, CompletionItemKind::VARIABLE, None));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    return Some(CompletionResponse::Array(items));
                }
                // most recent declaration of `name` before the cursor in this file
                let decl = a
                    .index
                    .decls
                    .iter()
                    .filter(|d| d.name == name && Some(d.span.file) == fid)
                    .filter(|d| (d.span.start as usize) <= off || matches!(d.kind, DeclKind::Enum | DeclKind::Struct))
                    .max_by_key(|d| d.span.start);
                if let Some(d) = decl {
                    for (m, detail) in &d.members {
                        let kind = if d.kind == DeclKind::Enum { CompletionItemKind::ENUM_MEMBER } else { CompletionItemKind::FIELD };
                        items.push(item(m, kind, Some(detail.clone())));
                    }
                }
            }
            return Some(CompletionResponse::Array(items));
        }

        // general completion
        for k in KEYWORDS {
            items.push(item(k, CompletionItemKind::KEYWORD, None));
        }
        for t in TYPES {
            items.push(item(t, CompletionItemKind::KEYWORD, Some("builtin type".into())));
        }
        if let Some(a) = analysis {
            let mut seen = HashSet::new();
            for d in a.index.decls.iter().rev() {
                if Some(d.span.file) != fid || matches!(d.kind, DeclKind::Field | DeclKind::Variant) {
                    continue;
                }
                let visible = matches!(d.kind, DeclKind::Function | DeclKind::Struct | DeclKind::Enum | DeclKind::Type)
                    || (d.span.start as usize) <= off;
                if !visible || !seen.insert(d.name.clone()) {
                    continue;
                }
                let kind = match d.kind {
                    DeclKind::Function => CompletionItemKind::FUNCTION,
                    DeclKind::Struct => CompletionItemKind::STRUCT,
                    DeclKind::Enum => CompletionItemKind::ENUM,
                    DeclKind::Type => CompletionItemKind::TYPE_PARAMETER,
                    _ => CompletionItemKind::VARIABLE,
                };
                items.push(item(&d.name, kind, Some(d.detail.clone())));
            }
            for alias in a.imports.keys() {
                items.push(item(alias, CompletionItemKind::MODULE, None));
            }
        }
        Some(CompletionResponse::Array(items))
    }
}

fn publish(path: &Path, diagnostics: Vec<Diagnostic>) -> Notification {
    Notification::new(
        PublishDiagnostics::METHOD.to_string(),
        PublishDiagnosticsParams { uri: path_to_uri(path), diagnostics, version: None },
    )
}

fn method_not_found(id: RequestId, method: &str) -> Response {
    Response::new_err(id, lsp_server::ErrorCode::MethodNotFound as i32, format!("unsupported request `{method}`"))
}
