//! `ncc lsp`: a language server over stdio.
//!
//! Every open document is analysed as an entry file, using the in-memory contents of all open
//! documents; the other `.nc` files of the workspace are analysed on demand for workspace-wide
//! requests (symbols, references, rename, call hierarchy).
//!
//! The features are split by area:
//! - `mod.rs`: document sync, diagnostics, hover, definition, symbols, references, rename,
//!   signature help, folding, formatting
//! - `complete.rs`: completion
//! - `calls.rs`: the call at the cursor (signature help) and signature labels
//! - `docs.rs`: doc comments (`///`) and builtin documentation
//! - `hints.rs`: inlay hints and semantic tokens
//! - `nav.rs`: type definition, call hierarchy, selection ranges, document links
//! - `actions.rs`: code actions and the extra diagnostics they fix (unused variables and imports)
//! - `refactor.rs`: more code actions, and exit-point highlights
//! - `ssr.rs`: structural search and replace, join lines and pulled diagnostics

mod actions;
mod calls;
mod complete;
mod docs;
mod hints;
pub(crate) mod nav;
mod refactor;
mod ssr;

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
use ssr::{JoinLinesParams, SsrParams};

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
/// Builtin functions: name, signature and documentation.
const BUILTINS: &[(&str, &str, &str)] = &[
    (
        "print",
        "fn @print(...)",
        "Prints its arguments to standard output, without a trailing newline.",
    ),
    (
        "println",
        "fn @println(...)",
        "Prints its arguments to standard output, followed by a newline.",
    ),
    (
        "eprint",
        "fn @eprint(...)",
        "Prints its arguments to standard error, without a trailing newline.",
    ),
    (
        "eprintln",
        "fn @eprintln(...)",
        "Prints its arguments to standard error, followed by a newline.",
    ),
    (
        "as",
        "fn @as(type T, value) T",
        "Converts `value` to the type `T`. Conversions that can lose information throw at runtime.",
    ),
    (
        "args",
        "fn @args() str[]",
        "The command-line arguments of the program, starting with the program name.",
    ),
    (
        "env",
        "fn @env() [str]str",
        "The environment variables of the process.",
    ),
    (
        "target",
        "fn @target() (str, str)",
        "The `(os, arch)` the program was compiled for, with Rust-style names such as `(\"macos\", \"aarch64\")`. Decided at compile time.",
    ),
    (
        "embed",
        "fn @embed(path) byte[]",
        "The contents of a file, read at compile time. The path is resolved like a relative import: `./`, `../` or `.../` (the project root).",
    ),
];

/// Result of analysing one entry document.
struct Analysis {
    sm: SourceMap,
    index: Index,
    /// import alias -> module path, from the last successful load
    imports: HashMap<String, PathBuf>,
    /// declaration index by span
    decl_at: HashMap<Span, usize>,
    /// the program checked without errors (lints run only then, as in the compiler)
    clean: bool,
}

impl Analysis {
    /// The type of the expression spanning `start..end` in file `fid`.
    fn expr_ty(&self, fid: u32, start: u32, end: u32) -> Option<&crate::check::index::ExprTy> {
        self.index
            .exprs
            .iter()
            .find(|x| x.span.file == fid && x.span.start == start && x.span.end == end)
    }

    /// The innermost reference at offset `off` of file `fid`.
    fn ref_at(&self, fid: u32, off: u32) -> Option<&crate::check::index::SymRef> {
        self.index
            .refs
            .iter()
            .filter(|r| {
                r.span.file == fid
                    && r.span.start <= off
                    && off <= r.span.end
                    && r.span.end > r.span.start
            })
            .min_by_key(|r| r.span.end - r.span.start)
    }

    fn decl(&self, span: Span) -> Option<&crate::check::index::SymDecl> {
        self.decl_at.get(&span).map(|&i| &self.index.decls[i])
    }

    fn file_id(&self, path: &Path) -> Option<u32> {
        self.sm
            .files
            .iter()
            .position(|f| f.path == path)
            .map(|i| i as u32)
    }
}

#[derive(Default)]
struct Server {
    docs: HashMap<PathBuf, String>,
    analyses: HashMap<PathBuf, Analysis>,
    /// the diagnostics found by analysing each entry, by file; a file shows the union of what
    /// every entry found in it
    published: HashMap<PathBuf, Vec<(PathBuf, Diagnostic)>>,
    /// workspace folders, searched for `.nc` files on workspace-wide requests
    roots: Vec<PathBuf>,
    /// whether the workspace's files that are not open have been analysed
    workspace_indexed: bool,
    /// the client accepts snippets in completions
    snippets: bool,
    /// notifications to send after the current message
    outbox: Vec<Notification>,
    /// versions of the open documents
    versions: HashMap<PathBuf, i32>,
    /// the client pulls the diagnostics of open documents (`textDocument/diagnostic`)
    pull: bool,
}

pub fn run() -> ExitCode {
    let (connection, io_threads) = Connection::stdio();
    let mut caps = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        declaration_provider: Some(DeclarationCapability::Simple(true)),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".into(), "@".into(), "\"".into(), "/".into()]),
            ..Default::default()
        }),
        inlay_hint_provider: Some(OneOf::Left(true)),
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(false),
        }),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: hints::legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: Some(true),
                ..Default::default()
            },
        )),
        call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
        selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
        document_link_provider: Some(DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: Default::default(),
        }),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
            first_trigger_character: "}".into(),
            more_trigger_character: None,
        }),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: None,
            file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                will_rename: Some(FileOperationRegistrationOptions {
                    filters: vec![FileOperationFilter {
                        scheme: Some("file".into()),
                        pattern: FileOperationPattern {
                            glob: "**/*.nc".into(),
                            matches: Some(FileOperationPatternKind::File),
                            options: None,
                        },
                    }],
                }),
                ..Default::default()
            }),
        }),
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into()]),
            ..Default::default()
        }),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::REFACTOR_EXTRACT,
                CodeActionKind::REFACTOR_INLINE,
                CodeActionKind::REFACTOR_REWRITE,
                CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
            ]),
            ..Default::default()
        })),
        document_formatting_provider: Some(OneOf::Left(true)),
        ..Default::default()
    };
    let r = (|| -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
        let (id, params) = connection.initialize_start()?;
        let mut server = Server::default();
        if let Ok(p) = serde_json::from_value::<InitializeParams>(params) {
            server.init(p);
        }
        // clients that pull diagnostics get them on request (and not pushed as well)
        if server.pull {
            caps.diagnostic_provider =
                Some(DiagnosticServerCapabilities::Options(DiagnosticOptions {
                    identifier: Some("ncc".into()),
                    inter_file_dependencies: true,
                    workspace_diagnostics: false,
                    ..Default::default()
                }));
        }
        let init = serde_json::json!({
            "capabilities": caps,
            "serverInfo": { "name": "ncc", "version": env!("CARGO_PKG_VERSION") },
        });
        connection.initialize_finish(id, init)?;
        // (`initialize_finish` has consumed the `initialized` notification)
        server.index_workspace();
        for out in std::mem::take(&mut server.outbox) {
            connection.sender.send(Message::Notification(out))?;
        }
        for msg in &connection.receiver {
            match msg {
                Message::Request(req) => {
                    if connection.handle_shutdown(&req)? {
                        break;
                    }
                    let id = req.id.clone();
                    let method = req.method.clone();
                    // a bug in one feature answers that request with an error instead of
                    // stopping the server
                    let resp = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        server.request(req)
                    }))
                    .unwrap_or_else(|_| {
                        Response::new_err(
                            id,
                            lsp_server::ErrorCode::InternalError as i32,
                            format!("internal error while handling `{method}`"),
                        )
                    });
                    connection.sender.send(Message::Response(resp))?;
                }
                Message::Notification(n) => {
                    let outs = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        server.notification(n)
                    }))
                    .unwrap_or_default();
                    server.outbox.splice(0..0, outs);
                }
                Message::Response(_) => {}
            }
            for out in std::mem::take(&mut server.outbox) {
                connection.sender.send(Message::Notification(out))?;
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
                s.push(b as char);
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
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
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

fn range(text: &str, s: usize, e: usize) -> Range {
    Range::new(offset_to_pos(text, s), offset_to_pos(text, e))
}

fn span_range(text: &str, span: Span) -> Range {
    range(text, span.start as usize, span.end as usize)
}

// `WorkspaceEdit::changes` is keyed by `Uri`, as defined by lsp-types
#[allow(clippy::mutable_key_type)]
fn edit_action(
    title: String,
    kind: CodeActionKind,
    changes: Vec<(Uri, TextEdit)>,
    diag: Option<&Diagnostic>,
) -> CodeActionOrCommand {
    let mut map: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for (u, e) in changes {
        map.entry(u).or_default().push(e);
    }
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(kind),
        diagnostics: diag.map(|d| vec![d.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(map),
            ..Default::default()
        }),
        is_preferred: diag.map(|_| true),
        ..Default::default()
    })
}

/// Offset of the start of the line containing `off`.
fn line_start(text: &str, off: usize) -> usize {
    text[..off.min(text.len())].rfind('\n').map_or(0, |i| i + 1)
}

/// Offset just past the newline ending the line containing `off`.
fn line_end(text: &str, off: usize) -> usize {
    text[off.min(text.len())..]
        .find('\n')
        .map_or(text.len(), |i| off + i + 1)
}

/// The leading whitespace of the line containing `off`.
fn indent_at(text: &str, off: usize) -> &str {
    let l = &text[line_start(text, off)..];
    &l[..l.len() - l.trim_start_matches([' ', '\t']).len()]
}

impl Server {
    #[allow(deprecated)]
    fn init(&mut self, p: InitializeParams) {
        self.pull = p
            .capabilities
            .text_document
            .as_ref()
            .is_some_and(|t| t.diagnostic.is_some());
        self.snippets = p
            .capabilities
            .text_document
            .as_ref()
            .and_then(|t| t.completion.as_ref())
            .and_then(|c| c.completion_item.as_ref())
            .and_then(|c| c.snippet_support)
            .unwrap_or(false);
        for f in p.workspace_folders.unwrap_or_default() {
            if let Some(path) = uri_to_path(&f.uri) {
                self.roots.push(path);
            }
        }
        if self.roots.is_empty()
            && let Some(path) = p.root_uri.as_ref().and_then(uri_to_path)
        {
            self.roots.push(path);
        }
    }

    /// Analyse every `.nc` file of the workspace that is not analysed yet (once), so that
    /// workspace-wide requests see the files that are not open.
    fn index_workspace(&mut self) {
        if self.workspace_indexed {
            return;
        }
        self.workspace_indexed = true;
        let mut files = Vec::new();
        for root in self.roots.clone() {
            collect_nc_files(&root, false, &mut files, 0);
        }
        let files: Vec<PathBuf> = files
            .into_iter()
            .take(1000)
            .map(|f| std::fs::canonicalize(&f).unwrap_or(f))
            .filter(|f| !self.analyses.contains_key(f))
            .collect();
        let out = self.refresh(files);
        self.outbox.extend(out);
    }

    /// The diagnostics of a file: the union of what every analysis found in it.
    fn diagnostics_of(&self, file: &Path) -> Vec<Diagnostic> {
        let mut diags: Vec<Diagnostic> = self
            .published
            .values()
            .flatten()
            .filter(|(f, _)| f == file)
            .map(|(_, d)| d.clone())
            .collect();
        diags.sort_by(|a, b| {
            (a.range.start.line, a.range.start.character, &a.message).cmp(&(
                b.range.start.line,
                b.range.start.character,
                &b.message,
            ))
        });
        diags.dedup_by(|a, b| a.range == b.range && a.message == b.message);
        diags
    }

    /// Files whose analysis includes `path` (they import it, directly or not).
    fn dependents(&self, path: &Path) -> Vec<PathBuf> {
        self.analyses
            .iter()
            .filter(|(e, a)| e.as_path() != path && a.sm.files.iter().any(|f| f.path == path))
            .map(|(e, _)| e.clone())
            .collect()
    }

    /// Analyse entry files and publish the diagnostics of every file they touch.
    fn refresh(&mut self, entries: Vec<PathBuf>) -> Vec<Notification> {
        let mut touched: HashSet<PathBuf> = HashSet::new();
        let mut done = HashSet::new();
        for entry in entries {
            if !done.insert(entry.clone()) {
                continue;
            }
            let old = self.published.remove(&entry).unwrap_or_default();
            touched.extend(old.into_iter().map(|(f, _)| f));
            touched.insert(entry.clone());
            if !self.docs.contains_key(&entry) && !entry.is_file() {
                // deleted: forget it
                self.analyses.remove(&entry);
                continue;
            }
            let diags = self.analyze(&entry);
            touched.extend(diags.iter().map(|(f, _)| f.clone()));
            self.published.insert(entry, diags);
        }
        let mut out = Vec::new();
        for file in touched {
            let diags = self.diagnostics_of(&file);
            if self.pull && self.docs.contains_key(&file) {
                continue;
            }
            let version = self.versions.get(&file).copied();
            out.push(publish(&file, diags, version));
        }
        out
    }

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
                self.versions.insert(path.clone(), p.text_document.version);
                path
            }
            DidChangeTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(n.params) else {
                    return vec![];
                };
                let Some(path) = uri_to_path(&p.text_document.uri) else {
                    return vec![];
                };
                let mut text = self.docs.remove(&path).unwrap_or_default();
                for change in p.content_changes {
                    match change.range {
                        Some(r) => {
                            let (s, e) =
                                (pos_to_offset(&text, r.start), pos_to_offset(&text, r.end));
                            text.replace_range(s.min(e)..e, &change.text);
                        }
                        None => text = change.text,
                    }
                }
                self.docs.insert(path.clone(), text);
                self.versions.insert(path.clone(), p.text_document.version);
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
            "workspace/didChangeWatchedFiles" => {
                // files changed outside the editor: analyse them and what imports them again
                let Ok(p) = serde_json::from_value::<DidChangeWatchedFilesParams>(n.params) else {
                    return vec![];
                };
                let mut entries = Vec::new();
                for c in p.changes {
                    let Some(path) = uri_to_path(&c.uri) else {
                        continue;
                    };
                    if self.docs.contains_key(&path) {
                        continue;
                    }
                    entries.extend(self.dependents(&path));
                    entries.push(path);
                }
                return self.refresh(entries);
            }
            DidCloseTextDocument::METHOD => {
                let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(n.params) else {
                    return vec![];
                };
                let Some(path) = uri_to_path(&p.text_document.uri) else {
                    return vec![];
                };
                self.docs.remove(&path);
                self.versions.remove(&path);
                // the file is now analysed as it is on disk (or forgotten when it is not)
                path
            }
            _ => return vec![],
        };
        // re-analyse the document, what imports it, and every open document
        let mut entries = vec![path.clone()];
        entries.extend(self.dependents(&path));
        entries.extend(self.docs.keys().filter(|p| **p != path).cloned());
        self.refresh(entries)
    }

    /// Analyse an entry file; returns diagnostics per file.
    fn analyze(&mut self, entry: &Path) -> Vec<(PathBuf, Diagnostic)> {
        let mut sm = SourceMap::default();
        let mut diags: Vec<(Diag, DiagnosticSeverity)> = Vec::new();
        let mut index = None;
        let mut imports = None;
        let mut clean = false;
        match crate::modules::load_with(&mut sm, entry, &self.docs) {
            Err(errs) => diags.extend(errs.into_iter().map(|d| (d, DiagnosticSeverity::ERROR))),
            Ok((graph, load_errors)) => {
                let loaded = load_errors.is_empty();
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
                let (mut p, errs, ix) = crate::check::check_for_editor(graph);
                clean = loaded && errs.is_empty();
                diags.extend(errs.into_iter().map(|d| (d, DiagnosticSeverity::ERROR)));
                if let Err(errs) = crate::eval::Evaluator::run(&mut p, &sm, false) {
                    diags.extend(errs.into_iter().map(|d| (d, DiagnosticSeverity::ERROR)));
                }
                if clean {
                    for w in crate::lint::race_warnings(&p) {
                        diags.push((w, DiagnosticSeverity::WARNING));
                    }
                }
                index = Some(ix);
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
        // keep the previous index when the file did not load
        if let Some(index) = index {
            let imports = imports.unwrap_or_default();
            let decl_at = index
                .decls
                .iter()
                .enumerate()
                .map(|(i, d)| (d.span, i))
                .collect();
            let a = Analysis {
                clean,
                sm,
                index,
                imports,
                decl_at,
            };
            out.extend(actions::warnings(&a, entry));
            self.analyses.insert(entry.to_path_buf(), a);
        }
        out
    }

    // ------------------------------------------------------------------ requests

    fn request(&mut self, req: Request) -> Response {
        let id = req.id.clone();
        if matches!(
            req.method.as_str(),
            "workspace/symbol"
                | "textDocument/codeLens"
                | "workspace/willRenameFiles"
                | "experimental/ssr"
                | "textDocument/references"
                | "textDocument/rename"
                | "callHierarchy/incomingCalls"
        ) {
            self.index_workspace();
        }
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
            "textDocument/formatting" => {
                serde_json::from_value::<DocumentFormattingParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.formatting(p)).unwrap())
            }
            "textDocument/codeAction" => serde_json::from_value::<CodeActionParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.code_actions(p)).unwrap()),
            "textDocument/declaration" => {
                serde_json::from_value::<GotoDefinitionParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.definition(p)).unwrap())
            }
            "textDocument/typeDefinition" => {
                serde_json::from_value::<GotoDefinitionParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.type_definition(p)).unwrap())
            }
            "textDocument/inlayHint" => serde_json::from_value::<InlayHintParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.inlay_hints(p)).unwrap()),
            "textDocument/semanticTokens/full" => {
                serde_json::from_value::<SemanticTokensParams>(req.params)
                    .ok()
                    .map(|p| {
                        serde_json::to_value(self.semantic_tokens(&p.text_document.uri, None))
                            .unwrap()
                    })
            }
            "textDocument/semanticTokens/range" => serde_json::from_value::<
                SemanticTokensRangeParams,
            >(req.params)
            .ok()
            .map(|p| {
                serde_json::to_value(self.semantic_tokens(&p.text_document.uri, Some(p.range)))
                    .unwrap()
            }),
            "textDocument/prepareCallHierarchy" => {
                serde_json::from_value::<CallHierarchyPrepareParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.prepare_call_hierarchy(p)).unwrap())
            }
            "callHierarchy/incomingCalls" => {
                serde_json::from_value::<CallHierarchyIncomingCallsParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.incoming_calls(p)).unwrap())
            }
            "callHierarchy/outgoingCalls" => {
                serde_json::from_value::<CallHierarchyOutgoingCallsParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.outgoing_calls(p)).unwrap())
            }
            "textDocument/selectionRange" => {
                serde_json::from_value::<SelectionRangeParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.selection_ranges(p)).unwrap())
            }
            "textDocument/documentLink" => serde_json::from_value::<DocumentLinkParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.document_links(p)).unwrap()),
            "workspace/willRenameFiles" => serde_json::from_value::<RenameFilesParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.will_rename_files(p)).unwrap()),
            "textDocument/onTypeFormatting" => {
                serde_json::from_value::<DocumentOnTypeFormattingParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.on_type_formatting(p)).unwrap())
            }
            // the parsed syntax tree, for debugging (like rust-analyzer's "view syntax tree")
            "ncc/syntaxTree" => serde_json::from_value::<TextDocumentIdentifier>(
                req.params.get("textDocument").cloned().unwrap_or_default(),
            )
            .ok()
            .and_then(|d| uri_to_path(&d.uri))
            .and_then(|path| self.text_of(&path))
            .map(|text| {
                let (module, _) = crate::parser::parse_module_recover(&text, 0);
                serde_json::Value::String(format!("{module:#?}"))
            }),
            "textDocument/codeLens" => serde_json::from_value::<CodeLensParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.code_lenses(p)).unwrap()),
            "textDocument/diagnostic" => {
                serde_json::from_value::<DocumentDiagnosticParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.pull_diagnostics(p)).unwrap())
            }
            "experimental/joinLines" => serde_json::from_value::<JoinLinesParams>(req.params)
                .ok()
                .map(|p| serde_json::to_value(self.join_lines(p)).unwrap()),
            "experimental/ssr" => match serde_json::from_value::<SsrParams>(req.params) {
                Ok(p) => match self.ssr(p) {
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
            "textDocument/rangeFormatting" => {
                serde_json::from_value::<DocumentRangeFormattingParams>(req.params)
                    .ok()
                    .map(|p| serde_json::to_value(self.range_formatting(p)).unwrap())
            }
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
        let fid = a.file_id(&path)?;
        let text = &a.sm.file(fid).text;
        let off = pos_to_offset(text, pos) as u32;
        a.ref_at(fid, off).map(|r| (a, r))
    }

    fn hover(&self, p: HoverParams) -> Option<Hover> {
        let pp = p.text_document_position_params;
        if let Some(h) = self.builtin_hover(&pp) {
            return Some(h);
        }
        if let Some((target, range)) = self.alias_target(&pp) {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("```nc\nmodule {}\n```\n\n`{}`", range.0, target.display()),
                }),
                range: Some(range.1),
            });
        }
        if let Some((a, r)) = self.lookup(&pp.text_document.uri, pp.position) {
            let text = &a.sm.file(r.span.file).text;
            let mut value = format!("```nc\n{}\n```", r.hover);
            if let Some(def) = r.def {
                let f = a.sm.file(def.file);
                if let Some(doc) = docs::doc_comment(&f.text, def) {
                    value.push_str("\n\n---\n\n");
                    value.push_str(&doc);
                }
                if def.file != r.span.file {
                    let name = f.path.file_name().unwrap_or_default().to_string_lossy();
                    value.push_str(&format!("\n\n*Defined in `{name}`*"));
                }
            }
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                }),
                range: Some(span_range(text, r.span)),
            });
        }
        // keywords
        if let Some(path) = uri_to_path(&pp.text_document.uri)
            && let Some(text) = self.text_of(&path)
            && let Some((s, e)) = word_at(&text, pos_to_offset(&text, pp.position))
            && let Some((_, doc)) = docs::KEYWORD_DOCS.iter().find(|(k, _)| *k == &text[s..e])
            && !in_string_or_comment(&text, s)
        {
            return Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("```nc\n{}\n```\n\n---\n\n{doc}", &text[s..e]),
                }),
                range: Some(range(&text, s, e)),
            });
        }
        // the type of the innermost expression under the cursor
        let (a, fid, text) = self.fresh(&pp.text_document.uri)?;
        let off = pos_to_offset(text, pp.position) as u32;
        let e = a
            .index
            .exprs
            .iter()
            .filter(|e| e.span.file == fid && e.span.start <= off && off < e.span.end)
            .min_by_key(|e| e.span.end - e.span.start)?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("```nc\n{}\n```", e.ty),
            }),
            range: Some(span_range(text, e.span)),
        })
    }

    /// The module file of an import or extern alias under the cursor, with the alias's path text
    /// and range.
    fn alias_target(&self, pp: &TextDocumentPositionParams) -> Option<(PathBuf, (String, Range))> {
        let (name, ranges) = self.alias_at(&pp.text_document.uri, pp.position)?;
        let path = uri_to_path(&pp.text_document.uri)?;
        let text = self.text_of(&path)?;
        let off = pos_to_offset(&text, pp.position);
        let range = ranges
            .into_iter()
            .find(|r| pos_to_offset(&text, r.start) <= off && off <= pos_to_offset(&text, r.end))?;
        let (module, _) = crate::parser::parse_module_recover(&text, 0);
        for it in &module.items {
            match it {
                ast::Item::Import(im) if im.alias == name => {
                    let target = self.analyses.get(&path)?.imports.get(&name)?.clone();
                    return Some((target, (im.path.clone(), range)));
                }
                ast::Item::Extern(x) if x.alias == name => {
                    let target = path.parent()?.join(&x.path);
                    return Some((target, (x.path.clone(), range)));
                }
                _ => {}
            }
        }
        None
    }

    /// The analysis of a document, when it was made from the document's current text: its
    /// file id and that text.
    fn fresh(&self, uri: &Uri) -> Option<(&Analysis, u32, &str)> {
        let path = uri_to_path(uri)?;
        let a = self.analyses.get(&path)?;
        let fid = a.file_id(&path)?;
        let text = &a.sm.file(fid).text;
        if let Some(cur) = self.docs.get(&path)
            && cur != text
        {
            return None;
        }
        Some((a, fid, text))
    }

    fn definition(&self, p: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let pp = p.text_document_position_params;
        if let Some((target, _)) = self.alias_target(&pp) {
            let start = Range::new(Position::new(0, 0), Position::new(0, 0));
            return Some(GotoDefinitionResponse::Scalar(Location::new(
                path_to_uri(&target),
                start,
            )));
        }
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
            // the name comes last in `int x` (fields, variables, imports) and first elsewhere
            let first = !matches!(
                kind,
                SymbolKind::FIELD
                    | SymbolKind::VARIABLE
                    | SymbolKind::CONSTANT
                    | SymbolKind::MODULE
            );
            let selection_range =
                name_span(&text, span, name, first).map_or(range, |s| span_range(&text, s));
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
                selection_range,
                children,
            }
        };
        let src = |sp: Span| -> String {
            text.get(sp.start as usize..sp.end as usize)
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        // `(int a, int b) int` from a signature (its span covers the name; the parameters
        // and return type run up to the body or `=`)
        let sig_detail = |sig: &ast::FnSig| -> Option<String> {
            let start = sig.span.end as usize;
            let rest = text.get(start..)?;
            let end = rest.find(['{', '=', '\n']).unwrap_or(rest.len());
            let t = src(Span::new(sig.span.file, start, start + end));
            t.starts_with('(').then_some(t)
        };
        let fn_symbol = |f: &ast::FnDecl| -> DocumentSymbol {
            // functions declared directly in the body
            let nested: Vec<DocumentSymbol> = f
                .body
                .stmts
                .iter()
                .filter_map(|st| match &st.kind {
                    ast::StmtKind::Fn(g) => Some(sym(
                        &g.sig.name,
                        SymbolKind::FUNCTION,
                        g.sig.span.to(g.body.span),
                        sig_detail(&g.sig),
                        None,
                    )),
                    _ => None,
                })
                .collect();
            sym(
                &f.sig.name,
                SymbolKind::FUNCTION,
                f.sig.span.to(f.body.span),
                sig_detail(&f.sig),
                (!nested.is_empty()).then_some(nested),
            )
        };
        let mut out = Vec::new();
        for it in &module.items {
            match it {
                ast::Item::Fn(f) => out.push(fn_symbol(f)),
                ast::Item::Struct(s) => {
                    let fields = s
                        .fields
                        .iter()
                        .map(|(t, n, sp)| sym(n, SymbolKind::FIELD, *sp, Some(src(t.span())), None))
                        .collect();
                    out.push(sym(&s.name, SymbolKind::STRUCT, s.span, None, Some(fields)));
                }
                ast::Item::Enum(e) => {
                    let vs = e
                        .variants
                        .iter()
                        .map(|(n, ts, sp)| {
                            let detail = (!ts.is_empty()).then(|| {
                                let ts: Vec<String> = ts.iter().map(|t| src(t.span())).collect();
                                format!("({})", ts.join(", "))
                            });
                            sym(n, SymbolKind::ENUM_MEMBER, *sp, detail, None)
                        })
                        .collect();
                    out.push(sym(&e.name, SymbolKind::ENUM, e.span, None, Some(vs)));
                }
                ast::Item::Type(t) => out.push(sym(
                    &t.name,
                    SymbolKind::TYPE_PARAMETER,
                    t.span,
                    Some(src(t.ty.span())),
                    None,
                )),
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
                                sig_detail(&f.sig),
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
                ast::Item::Stmt(s) => match &s.kind {
                    ast::StmtKind::VarDecl {
                        targets, mutable, ..
                    } => {
                        for t in targets {
                            let kind = if *mutable {
                                SymbolKind::VARIABLE
                            } else {
                                SymbolKind::CONSTANT
                            };
                            out.push(sym(&t.name, kind, t.span, Some(src(t.ty.span())), None));
                        }
                    }
                    ast::StmtKind::FnVar { name, .. } => {
                        out.push(sym(name, SymbolKind::FUNCTION, s.span, None, None));
                    }
                    ast::StmtKind::Fn(f) => out.push(fn_symbol(f)),
                    _ => {}
                },
            }
        }
        Some(DocumentSymbolResponse::Nested(out))
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
        if let Some((_, ranges)) = self.alias_at(&pp.text_document.uri, pp.position) {
            let skip = usize::from(!p.context.include_declaration);
            return Some(
                ranges
                    .into_iter()
                    .skip(skip)
                    .map(|r| Location::new(pp.text_document.uri.clone(), r))
                    .collect(),
            );
        }
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
        let text_highlights = |ranges: Vec<Range>| {
            let kind = Some(DocumentHighlightKind::TEXT);
            Some(
                ranges
                    .into_iter()
                    .map(|range| DocumentHighlight { range, kind })
                    .collect(),
            )
        };
        // `fn`/`return`/`throw`/`try`: the function and its exit points
        if let Some(path) = uri_to_path(&pp.text_document.uri)
            && let Some(text) = self.text_of(&path)
            && let off = pos_to_offset(&text, pp.position)
            && !in_string_or_comment(&text, off)
        {
            let (module, _) = crate::parser::parse_module_recover(&text, 0);
            if let Some(ranges) = refactor::exit_points(&module, &text, off) {
                return text_highlights(ranges);
            }
        }
        if let Some((_, ranges)) = self.alias_at(&pp.text_document.uri, pp.position) {
            return text_highlights(ranges);
        }
        let path = uri_to_path(&pp.text_document.uri)?;
        let (key, name, first) = self.symbol_at(&pp.text_document.uri, pp.position)?;
        text_highlights(
            (self.occurrences(&key, &name, first).into_iter())
                .filter(|(p, _)| *p == path)
                .map(|(_, range)| range)
                .collect(),
        )
    }

    fn prepare_rename(&self, p: TextDocumentPositionParams) -> Option<PrepareRenameResponse> {
        if let Some((name, ranges)) = self.alias_at(&p.text_document.uri, p.position) {
            let path = uri_to_path(&p.text_document.uri)?;
            let text = self.text_of(&path)?;
            let off = pos_to_offset(&text, p.position);
            return ranges
                .into_iter()
                .find(|r| {
                    pos_to_offset(&text, r.start) <= off && off <= pos_to_offset(&text, r.end)
                })
                .map(|range| PrepareRenameResponse::RangeWithPlaceholder {
                    range,
                    placeholder: name,
                });
        }
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
        if let Some((_, ranges)) = self.alias_at(&pp.text_document.uri, pp.position) {
            let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
            changes.insert(
                pp.text_document.uri.clone(),
                ranges
                    .into_iter()
                    .map(|r| TextEdit::new(r, new.to_string()))
                    .collect(),
            );
            return Ok(Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }));
        }
        let Some((key, name, first)) = self.symbol_at(&pp.text_document.uri, pp.position) else {
            return Ok(None);
        };
        // a name that is already declared where the symbol is declared would clash
        if let Some(a) = self.analyses.get(&key.0)
            && let Some(fid) = a.file_id(&key.0)
            && let Some(d) = a.decl_at.keys().find(|s| s.file == fid && s.start == key.1)
            && let Some(decl) = a.decl(*d)
        {
            let clash = a.index.decls.iter().any(|o| {
                o.name == new
                    && o.span.file == fid
                    && o.scope == decl.scope
                    && !matches!(o.kind, DeclKind::Field | DeclKind::Variant)
                    && !matches!(decl.kind, DeclKind::Field | DeclKind::Variant)
            }) || a.index.decls.iter().any(|o| {
                // fields and variants clash only with their siblings
                o.name == new
                    && matches!(decl.kind, DeclKind::Field | DeclKind::Variant)
                    && o.kind == decl.kind
                    && o.span.file == fid
                    && a.index.decls.iter().any(|p| {
                        matches!(p.kind, DeclKind::Struct | DeclKind::Enum)
                            && p.span.start <= o.span.start
                            && o.span.end <= p.span.end
                            && p.span.start <= decl.span.start
                            && decl.span.end <= p.span.end
                    })
            });
            if clash {
                return Err(format!("`{new}` is already declared here"));
            }
        }
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
        let (_, sig, doc) = BUILTINS.iter().find(|(n, ..)| *n == word)?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("```nc\n{sig}\n```\n\n---\n\n{doc}"),
            }),
            range: Some(range(&text, s, e)),
        })
    }

    fn folding(&self, p: FoldingRangeParams) -> Option<Vec<FoldingRange>> {
        let path = uri_to_path(&p.text_document.uri)?;
        let text = self.text_of(&path)?;
        let (toks, _) = crate::lexer::lex_all(&text, 0, 0);
        let line = |off: u32| offset_to_pos(&text, off as usize).line;
        let mut out = Vec::new();
        let mut stack: Vec<u32> = Vec::new();
        let mut import_braces = HashSet::new();
        for (i, t) in toks.iter().enumerate() {
            if t.tok == crate::lexer::Tok::LBrace
                && i > 0
                && matches!(&toks[i - 1].tok, crate::lexer::Tok::Ident(k) if k == "import")
            {
                import_braces.insert(t.span.start);
            }
        }
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
                                kind: import_braces
                                    .contains(&open)
                                    .then_some(FoldingRangeKind::Imports),
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
        // (a trailing non-comment line ends the last run)
        for (i, l) in text.lines().chain([""]).enumerate() {
            let i = i as u32;
            if l.trim_start().starts_with("//") {
                run = Some((run.map_or(i, |r| r.0), i));
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
        Some(out)
    }

    #[allow(deprecated)]
    fn workspace_symbols(&self, p: WorkspaceSymbolParams) -> WorkspaceSymbolResponse {
        let q = p.query.to_lowercase();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for a in self.analyses.values() {
            for d in &a.index.decls {
                let kind = match d.kind {
                    DeclKind::Function if d.scope.is_none() => SymbolKind::FUNCTION,
                    DeclKind::Struct => SymbolKind::STRUCT,
                    DeclKind::Enum => SymbolKind::ENUM,
                    DeclKind::Type => SymbolKind::TYPE_PARAMETER,
                    DeclKind::Variable if d.scope.is_none() && d.mutable => SymbolKind::VARIABLE,
                    DeclKind::Variable if d.scope.is_none() => SymbolKind::CONSTANT,
                    _ => continue,
                };
                if !d.name.to_lowercase().contains(&q) {
                    continue;
                }
                let f = a.sm.file(d.span.file);
                if !seen.insert((f.path.clone(), d.span.start)) {
                    continue;
                }
                let first = matches!(
                    d.kind,
                    DeclKind::Struct | DeclKind::Enum | DeclKind::Type | DeclKind::Function
                );
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
        WorkspaceSymbolResponse::Flat(out)
    }
}

impl Server {
    /// Whole-document formatting with `ncc fmt`'s rules (nothing when it has syntax errors).
    fn formatting(&self, p: DocumentFormattingParams) -> Option<Vec<TextEdit>> {
        let path = uri_to_path(&p.text_document.uri)?;
        let text = self.text_of(&path)?;
        let formatted = crate::fmt::format(&text).ok()?;
        Some(minimal_edits(&text, &formatted, None))
    }

    /// After a `}` that starts its line, indent it like the line of its `{`.
    fn on_type_formatting(&self, p: DocumentOnTypeFormattingParams) -> Option<Vec<TextEdit>> {
        let path = uri_to_path(&p.text_document_position.text_document.uri)?;
        let text = self.text_of(&path)?;
        let off = pos_to_offset(&text, p.text_document_position.position);
        let (toks, _) = crate::lexer::lex_all(&text, 0, 0);
        let close = toks
            .iter()
            .position(|t| t.tok == crate::lexer::Tok::RBrace && t.span.end as usize == off)?;
        let ls = line_start(&text, off - 1);
        if !text[ls..off - 1].trim().is_empty() {
            return Some(vec![]);
        }
        let mut depth = 0;
        let open = toks[..close].iter().rev().find(|t| match t.tok {
            crate::lexer::Tok::RBrace => {
                depth += 1;
                false
            }
            crate::lexer::Tok::LBrace if depth > 0 => {
                depth -= 1;
                false
            }
            crate::lexer::Tok::LBrace => true,
            _ => false,
        })?;
        let indent = indent_at(&text, open.span.start as usize);
        if &text[ls..off - 1] == indent {
            return Some(vec![]);
        }
        Some(vec![TextEdit::new(
            range(&text, ls, off - 1),
            indent.to_string(),
        )])
    }

    /// Formats the whole document, keeping only the changes to the lines in the range.
    fn range_formatting(&self, p: DocumentRangeFormattingParams) -> Option<Vec<TextEdit>> {
        let path = uri_to_path(&p.text_document.uri)?;
        let text = self.text_of(&path)?;
        let formatted = crate::fmt::format(&text).ok()?;
        Some(minimal_edits(&text, &formatted, Some(p.range)))
    }
}

/// Whether an offset is inside a string literal or a comment.
fn in_string_or_comment(text: &str, off: usize) -> bool {
    if crate::lexer::comments(text)
        .iter()
        .any(|&(s, e)| s <= off && off < e)
    {
        return true;
    }
    let (toks, _) = crate::lexer::lex_all(text, 0, 0);
    toks.iter().any(|t| {
        matches!(
            t.tok,
            crate::lexer::Tok::Str(_) | crate::lexer::Tok::Char(_)
        ) && (t.span.start as usize) <= off
            && off < t.span.end as usize
    })
}

/// Line-based edits turning `old` into `new`: the changed region between the common prefix and
/// suffix of lines, split at lines that match. With a range, only edits touching it are kept.
fn minimal_edits(old: &str, new: &str, range: Option<Range>) -> Vec<TextEdit> {
    if old == new {
        return vec![];
    }
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    // longest common subsequence of lines (the files are small enough for O(n*m))
    let (n, m) = (a.len(), b.len());
    if n * m > 4_000_000 {
        let end = offset_to_pos(old, old.len());
        return vec![TextEdit::new(
            Range::new(Position::new(0, 0), end),
            new.to_string(),
        )];
    }
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * (m + 1) + j] = if a[i] == b[j] {
                dp[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                dp[(i + 1) * (m + 1) + j].max(dp[i * (m + 1) + j + 1])
            };
        }
    }
    let mut edits = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut pending: Option<(usize, usize, usize)> = None; // (old line start, old end, new start)
    let mut new_end = 0;
    let flush =
        |pending: &mut Option<(usize, usize, usize)>, new_end: usize, edits: &mut Vec<TextEdit>| {
            if let Some((os, oe, ns)) = pending.take() {
                edits.push(TextEdit::new(
                    Range::new(Position::new(os as u32, 0), Position::new(oe as u32, 0)),
                    b[ns..new_end].concat(),
                ));
            }
        };
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            flush(&mut pending, new_end, &mut edits);
            i += 1;
            j += 1;
        } else if j < m && (i == n || dp[i * (m + 1) + j + 1] >= dp[(i + 1) * (m + 1) + j]) {
            let p = pending.get_or_insert((i, i, j));
            p.1 = i;
            j += 1;
            new_end = j;
        } else {
            let p = pending.get_or_insert((i, i, j));
            i += 1;
            p.1 = i;
            new_end = j;
        }
    }
    flush(&mut pending, new_end, &mut edits);
    // an old text without a final newline: positions past its last line are clamped by clients
    if let Some(r) = range {
        edits.retain(|e| e.range.start.line <= r.end.line && e.range.end.line >= r.start.line);
    }
    edits
}

/// `.nc` files under a directory, skipping hidden directories and build output unless `all`
/// is set (dependencies live under `.nc/`).
fn collect_nc_files(dir: &Path, all: bool, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 12 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !all && (name.starts_with('.') || name == "target" || name == "node_modules") {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() => collect_nc_files(&p, all, out, depth + 1),
            Ok(t) if t.is_file() && name.ends_with(".nc") => out.push(p),
            _ => {}
        }
    }
}

/// Hover text of a reference to a type (whose spans start with the name).
fn is_type_ref(hover: &str) -> bool {
    hover.starts_with("struct ") || hover.starts_with("enum ") || hover.starts_with("type ")
}

fn publish(path: &Path, diagnostics: Vec<Diagnostic>, version: Option<i32>) -> Notification {
    Notification::new(
        PublishDiagnostics::METHOD.to_string(),
        PublishDiagnosticsParams {
            uri: path_to_uri(path),
            diagnostics,
            version,
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
