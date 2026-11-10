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
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, Request as _,
};
use lsp_types::*;

use crate::ast;
use crate::check::index::{DeclKind, Index};
use crate::diag::{Diag, SourceMap, Span};

const KEYWORDS: &[&str] = &[
    "fn", "return", "if", "else", "while", "for", "in", "break", "continue", "mut", "mutex",
    "lock", "struct", "enum", "type", "import", "as", "extern", "pub", "test", "assert", "true",
    "false", "none", "and", "or", "not", "try", "catch", "throw", "async", "await", "fut",
];
const TYPES: &[&str] = &[
    "int", "uint", "byte", "float", "bool", "char", "str", "void", "error", "never",
];
const BUILTINS: &[(&str, &str)] = &[
    ("print", "fn @print(...)"),
    ("println", "fn @println(...)"),
    ("eprint", "fn @eprint(...)"),
    ("eprintln", "fn @eprintln(...)"),
    ("as", "fn @as(type T, value) T"),
    ("args", "fn @args() str[]"),
    ("env", "fn @env() [str]str"),
    ("target", "fn @target() (str, str)"),
    ("embed", "fn @embed(path) byte[]"),
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
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".into(), "@".into()]),
            ..Default::default()
        }),
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into()]),
            retrigger_characters: None,
            work_done_progress_options: Default::default(),
        }),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
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
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16)
        {
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
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                s.push(b as char)
            }
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
    Range::new(
        offset_to_pos(text, span.start as usize),
        offset_to_pos(text, span.end as usize),
    )
}

impl Server {
    fn text_of(&self, path: &Path) -> Option<String> {
        self.docs
            .get(path)
            .cloned()
            .or_else(|| std::fs::read_to_string(path).ok())
    }

    // ------------------------------------------------------------------ notifications

    fn notification(&mut self, n: Notification) -> Vec<Notification> {
        let path = match n.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(n.params) else {
                    return vec![];
                };
                let Some(path) = uri_to_path(&p.text_document.uri) else {
                    return vec![];
                };
                self.docs.insert(path.clone(), p.text_document.text);
                path
            }
            DidChangeTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(n.params) else {
                    return vec![];
                };
                let Some(path) = uri_to_path(&p.text_document.uri) else {
                    return vec![];
                };
                if let Some(change) = p.content_changes.into_iter().last() {
                    self.docs.insert(path.clone(), change.text);
                }
                path
            }
            DidSaveTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidSaveTextDocumentParams>(n.params) else {
                    return vec![];
                };
                let Some(path) = uri_to_path(&p.text_document.uri) else {
                    return vec![];
                };
                path
            }
            DidCloseTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(n.params) else {
                    return vec![];
                };
                let Some(path) = uri_to_path(&p.text_document.uri) else {
                    return vec![];
                };
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
            let old = self
                .published
                .insert(entry, files.clone())
                .unwrap_or_default();
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
                diags.extend(
                    load_errors
                        .into_iter()
                        .map(|d| (d, DiagnosticSeverity::ERROR)),
                );
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
                self.analyses
                    .insert(entry.to_path_buf(), Analysis { sm, index, imports });
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
            DocumentSymbolRequest::METHOD => {
                serde_json::from_value::<DocumentSymbolParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.symbols(p)).unwrap())
            }
            Completion::METHOD => serde_json::from_value::<CompletionParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.completion(p)).unwrap()),
            "textDocument/references" => serde_json::from_value::<ReferenceParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.references(p)).unwrap()),
            "textDocument/documentHighlight" => {
                serde_json::from_value::<DocumentHighlightParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.highlights(p)).unwrap())
            }
            "textDocument/prepareRename" => {
                serde_json::from_value::<TextDocumentPositionParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.prepare_rename(p)).unwrap())
            }
            "textDocument/rename" => match serde_json::from_value::<RenameParams>(req.params) {
                Ok(p) => match self.rename(p) {
                    Ok(edit) => Some(serde_json::to_value(edit).unwrap()),
                    Err(msg) => {
                        return Response::new_err(
                            id,
                            lsp_server::ErrorCode::InvalidParams as i32,
                            msg,
                        );
                    }
                },
                Err(_) => None,
            },
            "textDocument/signatureHelp" => {
                serde_json::from_value::<SignatureHelpParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.signature_help(p)).unwrap())
            }
            "textDocument/foldingRange" => serde_json::from_value::<FoldingRangeParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.folding(p)).unwrap()),
            "workspace/symbol" => serde_json::from_value::<WorkspaceSymbolParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.workspace_symbols(p)).unwrap()),
            "textDocument/codeAction" => serde_json::from_value::<CodeActionParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.code_actions(p)).unwrap()),
            _ => return method_not_found(id, &req.method),
        };
        match result {
            Some(v) => Response::new_ok(id, v),
            None => Response::new_err(
                id,
                lsp_server::ErrorCode::InvalidParams as i32,
                "invalid parameters".into(),
            ),
        }
    }

    /// Find the innermost indexed reference at a position.
    fn lookup(
        &self,
        uri: &Uri,
        pos: Position,
    ) -> Option<(&Analysis, &crate::check::index::SymRef)> {
        let path = uri_to_path(uri)?;
        let a = self.analyses.get(&path)?;
        let fid = a.sm.files.iter().position(|f| f.path == path)? as u32;
        let text = &a.sm.file(fid).text;
        let off = pos_to_offset(text, pos) as u32;
        a.index
            .refs
            .iter()
            .filter(|r| {
                r.span.file == fid
                    && r.span.start <= off
                    && off <= r.span.end
                    && r.span.end > r.span.start
            })
            .min_by_key(|r| r.span.end - r.span.start)
            .map(|r| (a, r))
    }

    fn hover(&self, p: HoverParams) -> Option<Hover> {
        let pp = p.text_document_position_params;
        if let Some(h) = self.builtin_hover(&pp) {
            return Some(h);
        }
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
        Some(GotoDefinitionResponse::Scalar(Location::new(
            path_to_uri(&f.path),
            span_range(&f.text, def),
        )))
    }

    #[allow(deprecated)]
    fn symbols(&self, p: DocumentSymbolParams) -> Option<DocumentSymbolResponse> {
        let path = uri_to_path(&p.text_document.uri)?;
        let text = self.text_of(&path)?;
        let (module, _) = crate::parser::parse_module_recover(&text, 0);
        let sym = |name: &str,
                   kind: SymbolKind,
                   span: Span,
                   detail: Option<String>,
                   children: Option<Vec<DocumentSymbol>>| {
            let range = span_range(&text, span);
            DocumentSymbol {
                name: if name.is_empty() {
                    "_".into()
                } else {
                    name.to_string()
                },
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
                ast::Item::Fn(f) => out.push(sym(
                    &f.sig.name,
                    SymbolKind::FUNCTION,
                    f.sig.span.to(f.body.span),
                    None,
                    None,
                )),
                ast::Item::Struct(s) => {
                    let fields = s
                        .fields
                        .iter()
                        .map(|(_, n, sp)| sym(n, SymbolKind::FIELD, *sp, None, None))
                        .collect();
                    out.push(sym(&s.name, SymbolKind::STRUCT, s.span, None, Some(fields)));
                }
                ast::Item::Enum(e) => {
                    let vs = e
                        .variants
                        .iter()
                        .map(|(n, _, sp)| sym(n, SymbolKind::ENUM_MEMBER, *sp, None, None))
                        .collect();
                    out.push(sym(&e.name, SymbolKind::ENUM, e.span, None, Some(vs)));
                }
                ast::Item::Type(t) => {
                    out.push(sym(&t.name, SymbolKind::TYPE_PARAMETER, t.span, None, None))
                }
                ast::Item::Test(name, _, span) => out.push(sym(
                    name,
                    SymbolKind::METHOD,
                    *span,
                    Some("test".into()),
                    None,
                )),
                ast::Item::Extern(e) => {
                    let fs = e
                        .fns
                        .iter()
                        .map(|f| {
                            sym(
                                &f.sig.name,
                                SymbolKind::FUNCTION,
                                f.sig.span,
                                Some(f.symbol.clone()),
                                None,
                            )
                        })
                        .collect();
                    out.push(sym(
                        &e.alias,
                        SymbolKind::MODULE,
                        e.span,
                        Some(e.path.clone()),
                        Some(fs),
                    ));
                }
                ast::Item::Import(im) => out.push(sym(
                    &im.alias,
                    SymbolKind::MODULE,
                    im.span,
                    Some(im.path.clone()),
                    None,
                )),
                ast::Item::Stmt(s) => {
                    if let ast::StmtKind::VarDecl {
                        targets, mutable, ..
                    } = &s.kind
                    {
                        for t in targets {
                            let kind = if *mutable {
                                SymbolKind::VARIABLE
                            } else {
                                SymbolKind::CONSTANT
                            };
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
        let fid = analysis
            .and_then(|a| a.sm.files.iter().position(|f| f.path == path))
            .map(|i| i as u32);

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
                        && let Ok(m) = crate::parser::parse_module(&mtext, 0)
                    {
                        for it in &m.items {
                            match it {
                                ast::Item::Fn(f) if f.public => items.push(item(
                                    &f.sig.name,
                                    CompletionItemKind::FUNCTION,
                                    None,
                                )),
                                ast::Item::Struct(s) if s.public => {
                                    items.push(item(&s.name, CompletionItemKind::STRUCT, None))
                                }
                                ast::Item::Enum(e) if e.public => {
                                    items.push(item(&e.name, CompletionItemKind::ENUM, None))
                                }
                                ast::Item::Type(t) if t.public => items.push(item(
                                    &t.name,
                                    CompletionItemKind::TYPE_PARAMETER,
                                    None,
                                )),
                                ast::Item::Stmt(ast::Stmt {
                                    kind:
                                        ast::StmtKind::VarDecl {
                                            public: true,
                                            targets,
                                            ..
                                        },
                                    ..
                                }) => {
                                    for t in targets {
                                        items.push(item(
                                            &t.name,
                                            CompletionItemKind::VARIABLE,
                                            None,
                                        ));
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
                    .filter(|d| {
                        (d.span.start as usize) <= off
                            || matches!(d.kind, DeclKind::Enum | DeclKind::Struct)
                    })
                    .max_by_key(|d| d.span.start);
                if let Some(d) = decl {
                    for (m, detail) in &d.members {
                        let kind = if d.kind == DeclKind::Enum {
                            CompletionItemKind::ENUM_MEMBER
                        } else {
                            CompletionItemKind::FIELD
                        };
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
            items.push(item(
                t,
                CompletionItemKind::KEYWORD,
                Some("builtin type".into()),
            ));
        }
        if let Some(a) = analysis {
            let mut seen = HashSet::new();
            for d in a.index.decls.iter().rev() {
                if Some(d.span.file) != fid || matches!(d.kind, DeclKind::Field | DeclKind::Variant)
                {
                    continue;
                }
                let visible = matches!(
                    d.kind,
                    DeclKind::Function | DeclKind::Struct | DeclKind::Enum | DeclKind::Type
                ) || (d.span.start as usize) <= off;
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

// ------------------------------------------------------------------ navigation and editing

/// A symbol's identity across analyses: its definition's file and offset.
type SymKey = (PathBuf, u32);

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Byte ranges of whole-word occurrences of `name` in `text[start..end]`.
fn word_occurrences(text: &str, start: usize, end: usize, name: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let end = end.min(text.len());
    let mut from = start;
    while let Some(i) = text[from..end].find(name) {
        let s = from + i;
        let e = s + name.len();
        let before_ok = s == 0 || !is_ident_byte(bytes[s - 1]);
        let after_ok = e >= bytes.len() || !is_ident_byte(bytes[e]);
        if before_ok && after_ok {
            out.push((s, e));
        }
        from = s + 1;
    }
    out
}

/// The identifier itself within a reference or declaration span. Types and `struct`/`enum`/`type`
/// declarations put the name first (`Matrix<int>`); everything else ends with it (`p.x`, `int a`).
fn name_span(text: &str, span: Span, name: &str, name_first: bool) -> Option<Span> {
    let occ = word_occurrences(text, span.start as usize, span.end as usize, name);
    let (s, e) = if name_first {
        *occ.first()?
    } else {
        *occ.last()?
    };
    Some(Span::new(span.file, s, e))
}

fn word_at(text: &str, off: usize) -> Option<(usize, usize)> {
    let b = text.as_bytes();
    let mut s = off.min(b.len());
    while s > 0 && (is_ident_byte(b[s - 1]) || b[s - 1] == b'@') {
        s -= 1;
    }
    let mut e = off.min(b.len());
    while e < b.len() && is_ident_byte(b[e]) {
        e += 1;
    }
    (s < e).then_some((s, e))
}

impl Server {
    fn key_of(a: &Analysis, def: Span) -> SymKey {
        (a.sm.file(def.file).path.clone(), def.start)
    }

    /// The symbol at a position: its key, name and whether its name comes first in spans.
    fn symbol_at(&self, uri: &Uri, pos: Position) -> Option<(SymKey, String, bool)> {
        let (a, r) = self.lookup(uri, pos)?;
        let def = r.def?;
        let decl = a.index.decls.iter().find(|d| d.span == def)?;
        let name_first = matches!(
            decl.kind,
            DeclKind::Struct | DeclKind::Enum | DeclKind::Type
        );
        Some((Self::key_of(a, def), decl.name.clone(), name_first))
    }

    /// Every occurrence of a symbol (declaration and uses) in all analysed files.
    fn occurrences(&self, key: &SymKey, name: &str, name_first: bool) -> Vec<(PathBuf, Range)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for a in self.analyses.values() {
            for r in &a.index.refs {
                let Some(def) = r.def else { continue };
                if &Self::key_of(a, def) != key {
                    continue;
                }
                let f = a.sm.file(r.span.file);
                // a reference to a type shows the type's declaration: its name comes first
                let first = name_first || r.span == def && name_first;
                let Some(ns) = name_span(&f.text, r.span, name, first || is_type_ref(&r.hover))
                else {
                    continue;
                };
                if seen.insert((f.path.clone(), ns.start)) {
                    out.push((f.path.clone(), span_range(&f.text, ns)));
                }
            }
        }
        out.sort_by(|a, b| {
            (&a.0, a.1.start.line, a.1.start.character).cmp(&(
                &b.0,
                b.1.start.line,
                b.1.start.character,
            ))
        });
        out
    }

    fn references(&self, p: ReferenceParams) -> Option<Vec<Location>> {
        let pp = p.text_document_position;
        let (key, name, first) = self.symbol_at(&pp.text_document.uri, pp.position)?;
        let mut locs: Vec<Location> = self
            .occurrences(&key, &name, first)
            .into_iter()
            .map(|(path, range)| Location::new(path_to_uri(&path), range))
            .collect();
        if !p.context.include_declaration {
            let decl_line = self
                .occurrences(&key, &name, first)
                .into_iter()
                .find(|(path, _)| *path == key.0);
            if let Some((_, r)) = decl_line {
                locs.retain(|l| l.range != r || uri_to_path(&l.uri).as_ref() != Some(&key.0));
            }
        }
        Some(locs)
    }

    fn highlights(&self, p: DocumentHighlightParams) -> Option<Vec<DocumentHighlight>> {
        let pp = p.text_document_position_params;
        let path = uri_to_path(&pp.text_document.uri)?;
        let (key, name, first) = self.symbol_at(&pp.text_document.uri, pp.position)?;
        Some(
            self.occurrences(&key, &name, first)
                .into_iter()
                .filter(|(p, _)| *p == path)
                .map(|(_, range)| DocumentHighlight {
                    range,
                    kind: Some(DocumentHighlightKind::TEXT),
                })
                .collect(),
        )
    }

    fn prepare_rename(&self, p: TextDocumentPositionParams) -> Option<PrepareRenameResponse> {
        let path = uri_to_path(&p.text_document.uri)?;
        let (key, name, first) = self.symbol_at(&p.text_document.uri, p.position)?;
        let text = self.text_of(&path)?;
        let off = pos_to_offset(&text, p.position);
        // the cursor must be on the name itself
        self.occurrences(&key, &name, first)
            .into_iter()
            .filter(|(p, _)| *p == path)
            .map(|(_, r)| r)
            .find(|r| pos_to_offset(&text, r.start) <= off && off <= pos_to_offset(&text, r.end))
            .map(|range| PrepareRenameResponse::RangeWithPlaceholder {
                range,
                placeholder: name,
            })
    }

    // `WorkspaceEdit::changes` is keyed by `Uri`, as defined by lsp-types
    #[allow(clippy::mutable_key_type)]
    fn rename(&self, p: RenameParams) -> Result<Option<WorkspaceEdit>, String> {
        let pp = p.text_document_position;
        let new = p.new_name.trim();
        let valid = new
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && new.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && new != "_";
        if !valid || crate::parser::is_keyword(new) {
            return Err(format!("`{new}` is not a valid name"));
        }
        let Some((key, name, first)) = self.symbol_at(&pp.text_document.uri, pp.position) else {
            return Ok(None);
        };
        let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
        for (path, range) in self.occurrences(&key, &name, first) {
            changes
                .entry(path_to_uri(&path))
                .or_default()
                .push(TextEdit::new(range, new.to_string()));
        }
        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }))
    }

    fn builtin_hover(&self, pp: &TextDocumentPositionParams) -> Option<Hover> {
        let path = uri_to_path(&pp.text_document.uri)?;
        let text = self.text_of(&path)?;
        let off = pos_to_offset(&text, pp.position);
        let (s, e) = word_at(&text, off)?;
        let word = text[s..e].strip_prefix('@')?;
        let (_, sig) = BUILTINS.iter().find(|(n, _)| *n == word)?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("```nc\n{sig}\n```"),
            }),
            range: Some(Range::new(offset_to_pos(&text, s), offset_to_pos(&text, e))),
        })
    }

    fn signature_help(&self, p: SignatureHelpParams) -> Option<SignatureHelp> {
        let pp = p.text_document_position_params;
        let path = uri_to_path(&pp.text_document.uri)?;
        let text = self.text_of(&path)?;
        let off = pos_to_offset(&text, pp.position);
        // find the unclosed `(` before the cursor and count the commas at its level
        let b = text.as_bytes();
        let (mut depth, mut commas, mut i) = (0i32, 0u32, off);
        let open = loop {
            if i == 0 {
                return None;
            }
            i -= 1;
            match b[i] {
                b')' | b']' | b'}' => depth += 1,
                b'(' | b'[' | b'{' if depth > 0 => depth -= 1,
                b'(' => break i,
                b'[' | b'{' => return None,
                b',' if depth == 0 => commas += 1,
                b'\n' if depth == 0 && i + 1 < off && text[i + 1..off].trim().is_empty() => {}
                _ => {}
            }
        };
        let mut e = open;
        while e > 0 && b[e - 1] == b' ' {
            e -= 1;
        }
        // skip explicit generic arguments: `map<int, str>(`
        if e > 0 && b[e - 1] == b'>' {
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
        }
        let mut s = e;
        while s > 0 && (is_ident_byte(b[s - 1]) || b[s - 1] == b'@') {
            s -= 1;
        }
        let name = &text[s..e];
        let label = if let Some(bi) = name.strip_prefix('@') {
            BUILTINS.iter().find(|(n, _)| *n == bi)?.1.to_string()
        } else {
            let a = self.analyses.get(&path)?;
            let fid =
                a.sm.files
                    .iter()
                    .position(|f| f.path == path)
                    .map(|i| i as u32);
            a.index
                .decls
                .iter()
                .filter(|d| d.name == name && d.kind == DeclKind::Function)
                .max_by_key(|d| Some(d.span.file) == fid)?
                .detail
                .clone()
        };
        // parameter label offsets within `fn name(a, b) ret`
        let lp = label.find('(')?;
        let rp = label.rfind(')')?;
        let mut params = Vec::new();
        let mut start = lp + 1;
        let mut depth = 0;
        for (k, c) in label[lp + 1..rp].char_indices() {
            let k = lp + 1 + k;
            match c {
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                ',' if depth == 0 => {
                    params.push((start, k));
                    start = k + 2;
                }
                _ => {}
            }
        }
        if start < rp {
            params.push((start, rp));
        }
        let parameters = params
            .iter()
            .map(|(s, e)| ParameterInformation {
                label: ParameterLabel::LabelOffsets([
                    label[..*s].encode_utf16().count() as u32,
                    label[..*e].encode_utf16().count() as u32,
                ]),
                documentation: None,
            })
            .collect();
        Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label,
                documentation: None,
                parameters: Some(parameters),
                active_parameter: Some(commas),
            }],
            active_signature: Some(0),
            active_parameter: Some(commas),
        })
    }

    fn folding(&self, p: FoldingRangeParams) -> Option<Vec<FoldingRange>> {
        let path = uri_to_path(&p.text_document.uri)?;
        let text = self.text_of(&path)?;
        let (toks, _) = crate::lexer::lex_all(&text, 0, 0);
        let line = |off: u32| offset_to_pos(&text, off as usize).line;
        let mut out = Vec::new();
        let mut stack: Vec<u32> = Vec::new();
        for t in &toks {
            match t.tok {
                crate::lexer::Tok::LBrace
                | crate::lexer::Tok::LBracket
                | crate::lexer::Tok::LParen => stack.push(t.span.start),
                crate::lexer::Tok::RBrace
                | crate::lexer::Tok::RBracket
                | crate::lexer::Tok::RParen => {
                    if let Some(open) = stack.pop() {
                        let (a, b) = (line(open), line(t.span.start));
                        if b > a {
                            out.push(FoldingRange {
                                start_line: a,
                                end_line: b.saturating_sub(1).max(a),
                                ..Default::default()
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        // runs of comment lines
        let mut run: Option<(u32, u32)> = None;
        for (i, l) in text.lines().enumerate() {
            let i = i as u32;
            if l.trim_start().starts_with("//") {
                run = Some(match run {
                    Some((s, _)) => (s, i),
                    None => (i, i),
                });
            } else if let Some((s, e)) = run.take()
                && e > s
            {
                out.push(FoldingRange {
                    start_line: s,
                    end_line: e,
                    kind: Some(FoldingRangeKind::Comment),
                    ..Default::default()
                });
            }
        }
        if let Some((s, e)) = run
            && e > s
        {
            out.push(FoldingRange {
                start_line: s,
                end_line: e,
                kind: Some(FoldingRangeKind::Comment),
                ..Default::default()
            });
        }
        Some(out)
    }

    #[allow(deprecated)]
    fn workspace_symbols(&self, p: WorkspaceSymbolParams) -> Option<WorkspaceSymbolResponse> {
        let q = p.query.to_lowercase();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for a in self.analyses.values() {
            for d in &a.index.decls {
                let kind = match d.kind {
                    DeclKind::Function => SymbolKind::FUNCTION,
                    DeclKind::Struct => SymbolKind::STRUCT,
                    DeclKind::Enum => SymbolKind::ENUM,
                    DeclKind::Type => SymbolKind::TYPE_PARAMETER,
                    _ => continue,
                };
                if !d.name.to_lowercase().contains(&q) {
                    continue;
                }
                let f = a.sm.file(d.span.file);
                if !seen.insert((f.path.clone(), d.span.start)) {
                    continue;
                }
                let first = matches!(d.kind, DeclKind::Struct | DeclKind::Enum | DeclKind::Type);
                let span = name_span(&f.text, d.span, &d.name, first).unwrap_or(d.span);
                out.push(SymbolInformation {
                    name: d.name.clone(),
                    kind,
                    tags: None,
                    deprecated: None,
                    location: Location::new(path_to_uri(&f.path), span_range(&f.text, span)),
                    container_name: f.path.file_name().map(|n| n.to_string_lossy().into_owned()),
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Some(WorkspaceSymbolResponse::Flat(out))
    }

    // `WorkspaceEdit::changes` is keyed by `Uri`, as defined by lsp-types
    #[allow(clippy::mutable_key_type)]
    fn code_actions(&self, p: CodeActionParams) -> Option<CodeActionResponse> {
        let path = uri_to_path(&p.text_document.uri)?;
        let text = self.text_of(&path)?;
        let mut out = Vec::new();
        for d in &p.context.diagnostics {
            let fix = |title: String, at: Position, insert: &str| {
                let mut changes = HashMap::new();
                changes.insert(
                    p.text_document.uri.clone(),
                    vec![TextEdit::new(Range::new(at, at), insert.to_string())],
                );
                CodeActionOrCommand::CodeAction(CodeAction {
                    title,
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![d.clone()]),
                    edit: Some(WorkspaceEdit {
                        changes: Some(changes),
                        ..Default::default()
                    }),
                    is_preferred: Some(true),
                    ..Default::default()
                })
            };
            if d.message.contains("declare it with `mut`")
                && d.message.contains("immutable variable")
            {
                // insert `mut` at the variable's declaration
                if let Some((a, r)) = self.lookup(&p.text_document.uri, d.range.start)
                    && let Some(def) = r.def
                    && a.sm.file(def.file).path == path
                {
                    let at = offset_to_pos(&text, def.start as usize);
                    out.push(fix("Make the variable mutable".into(), at, "mut "));
                }
            } else if d.message.contains("not used") || d.message.starts_with("unused value") {
                out.push(fix(
                    "Discard the value with `_ =`".into(),
                    d.range.start,
                    "_ = ",
                ));
            }
        }
        Some(out)
    }
}

/// Hover text of a reference to a type (whose spans start with the name).
fn is_type_ref(hover: &str) -> bool {
    hover.starts_with("struct ") || hover.starts_with("enum ") || hover.starts_with("type ")
}

fn publish(path: &Path, diagnostics: Vec<Diagnostic>) -> Notification {
    Notification::new(
        PublishDiagnostics::METHOD.to_string(),
        PublishDiagnosticsParams {
            uri: path_to_uri(path),
            diagnostics,
            version: None,
        },
    )
}

fn method_not_found(id: RequestId, method: &str) -> Response {
    Response::new_err(
        id,
        lsp_server::ErrorCode::MethodNotFound as i32,
        format!("unsupported request `{method}`"),
    )
}
