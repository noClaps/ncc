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

// `WorkspaceEdit::changes` is keyed by `Uri`, as defined by lsp-types
#![allow(clippy::mutable_key_type)]

mod actions;
mod calls;
mod complete;
mod docs;
mod hints;
pub mod nav;
mod refactor;
mod ssr;
mod tyexpr;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lsp_server::{Connection, Message, Notification, Request, Response};
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
use crate::diag::{SourceMap, Span, codes};
use crate::lexer::{Tok, is_ident_byte};
use crate::parser::KEYWORDS;

/// A builtin function (`@name`).
struct Builtin {
    name: &'static str,
    params: &'static [&'static str],
    /// the return type, empty for none
    ret: &'static str,
    doc: &'static str,
}

impl Builtin {
    fn sig(&self) -> crate::check::index::Sig {
        crate::check::index::Sig {
            type_params: vec![],
            params: (self.params.iter())
                .map(|p| (None, p.to_string()))
                .collect(),
            ret: crate::parser::parse_type_text(self.ret),
        }
    }

    /// `fn @as(type T, value) T`
    fn label(&self) -> String {
        self.sig().label(&format!("@{}", self.name)).0
    }
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "print",
        params: &["..."],
        ret: "",
        doc: "Prints its arguments to standard output, without a trailing newline.",
    },
    Builtin {
        name: "println",
        params: &["..."],
        ret: "",
        doc: "Prints its arguments to standard output, followed by a newline.",
    },
    Builtin {
        name: "eprint",
        params: &["..."],
        ret: "",
        doc: "Prints its arguments to standard error, without a trailing newline.",
    },
    Builtin {
        name: "eprintln",
        params: &["..."],
        ret: "",
        doc: "Prints its arguments to standard error, followed by a newline.",
    },
    Builtin {
        name: "as",
        params: &["type T", "value"],
        ret: "T",
        doc: "Converts `value` to the type `T`. Conversions that can lose information throw at runtime.",
    },
    Builtin {
        name: "args",
        params: &[],
        ret: "str[]",
        doc: "The command-line arguments of the program, starting with the program name.",
    },
    Builtin {
        name: "env",
        params: &[],
        ret: "[str]str",
        doc: "The environment variables of the process.",
    },
    Builtin {
        name: "target",
        params: &[],
        ret: "(str, str)",
        doc: "The `(os, arch)` the program was compiled for, with Rust-style names such as `(\"macos\", \"aarch64\")`. Decided at compile time.",
    },
    Builtin {
        name: "embed",
        params: &["path"],
        ret: "byte[]",
        doc: "The contents of a file, read at compile time. The path is resolved like a relative import: `./`, `../` or `.../` (the project root).",
    },
];

/// A document symbol, serialized like `DocumentSymbol` but without its deprecated
/// `deprecated` field (which `DocumentSymbol` cannot be built without).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DocSymbol {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    kind: SymbolKind,
    range: Range,
    selection_range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<Self>>,
}

/// Result of analysing one entry document.
struct Analysis {
    sm: SourceMap,
    index: Index,
    /// the types the index refers to
    types: crate::types::Types,
    /// import alias -> module path, from the last successful load
    imports: HashMap<String, PathBuf>,
    /// declaration index by span
    decl_at: HashMap<Span, usize>,
    /// the program checked without errors (lints run only then, as in the compiler)
    clean: bool,
}

impl Analysis {
    fn show(&self, t: crate::types::TyId) -> String {
        self.types.show(t)
    }

    /// The variants of the enum declared by `d`: name and number of payload values.
    fn variants(&self, d: &crate::check::index::SymDecl) -> Vec<(String, usize)> {
        (d.ty.and_then(|t| self.types.enum_def(t)).into_iter())
            .flat_map(|e| e.variants.iter().map(|(n, ps)| (n.clone(), ps.len())))
            .collect()
    }

    /// The members reachable with `.` on values of type `t`.
    fn members(&self, t: crate::types::TyId) -> Vec<crate::check::index::Member> {
        self.index.members(&self.types, t)
    }

    /// The type of the expression spanning `start..end` in file `fid`.
    fn expr_ty(&self, fid: u32, start: u32, end: u32) -> Option<&crate::check::index::ExprTy> {
        self.index
            .exprs
            .iter()
            .find(|x| x.span.file == fid && x.span.start == start && x.span.end == end)
    }

    /// The innermost expression around offset `off` of file `fid` that satisfies `want`.
    fn expr_around(
        &self,
        fid: u32,
        off: u32,
        want: impl Fn(&crate::check::index::ExprTy) -> bool,
    ) -> Option<&crate::check::index::ExprTy> {
        self.index
            .exprs
            .iter()
            .filter(|e| e.span.file == fid && e.span.start <= off && off < e.span.end && want(e))
            .min_by_key(|e| e.span.end - e.span.start)
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
    /// for each file, the entries whose analysis found diagnostics in it (keys of `published`)
    reported_by: HashMap<PathBuf, HashSet<PathBuf>>,
    /// workspace folders, searched for `.nc` files on workspace-wide requests
    roots: Vec<PathBuf>,
    /// whether the workspace's files have been listed for analysis
    workspace_queued: bool,
    /// workspace files still to analyse, in the background
    unindexed: Vec<PathBuf>,
    /// the client accepts snippets in completions
    snippets: bool,
    /// notifications to send after the current message
    outbox: Vec<Notification>,
    /// versions of the open documents
    versions: HashMap<PathBuf, i32>,
    /// the client pulls the diagnostics of open documents (`textDocument/diagnostic`)
    pull: bool,
    /// modules parsed by earlier analyses
    parses: crate::modules::ParseCache,
    /// the syntax tree of each document, for the version (or, when not open, the text) it had
    asts: std::cell::RefCell<HashMap<PathBuf, DocAst>>,
    /// modules each directory's files could import, until files change
    importable: std::cell::RefCell<HashMap<PathBuf, ImportableScan>>,
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
        // diagnostics for the whole workspace, not only open files: its files are analysed a
        // few at a time while no message is waiting
        server.queue_workspace();
        let mut waiting = std::collections::VecDeque::new();
        loop {
            // take what has arrived, so that a burst of notifications is handled at once
            while let Ok(m) = connection.receiver.try_recv() {
                waiting.push_back(m);
            }
            // workspace files nothing asked for yet are analysed a few at a time, and only once
            // the client has been quiet for a moment
            if waiting.is_empty() && !server.unindexed.is_empty() {
                match connection.receiver.recv_timeout(DEBOUNCE) {
                    Ok(m) => waiting.push_back(m),
                    Err(e) if e.is_disconnected() => break,
                    Err(_) => {
                        server.index_some(4);
                        for out in std::mem::take(&mut server.outbox) {
                            connection.sender.send(Message::Notification(out))?;
                        }
                        continue;
                    }
                }
            }
            let msg = match waiting.pop_front() {
                Some(m) => m,
                None => match connection.receiver.recv() {
                    Ok(m) => m,
                    Err(_) => break,
                },
            };
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
                    // edits arrive in bursts: take the notifications already waiting too
                    let mut batch = vec![n];
                    while let Some(Message::Notification(_)) = waiting.front() {
                        let Some(Message::Notification(n)) = waiting.pop_front() else {
                            unreachable!()
                        };
                        batch.push(n);
                    }
                    // typing sends an edit per key: wait a moment for the next one, unless a
                    // request is waiting for the analysis
                    while waiting.is_empty()
                        && batch
                            .last()
                            .is_some_and(|n| n.method == DidChangeTextDocument::METHOD)
                    {
                        match connection.receiver.recv_timeout(DEBOUNCE) {
                            Ok(Message::Notification(n)) => batch.push(n),
                            Ok(m) => waiting.push_back(m),
                            Err(_) => break,
                        }
                    }
                    let outs = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        server.notifications(batch)
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
    if !uri.as_str().starts_with("file://") {
        return None;
    }
    let p = PathBuf::from(&*uri.path().as_estr().decode().into_string().ok()?);
    // a request resolves its document several times: remember the paths of files that exist
    thread_local! {
        static RESOLVED: std::cell::RefCell<HashMap<PathBuf, PathBuf>> =
            std::cell::RefCell::new(HashMap::new());
    }
    if let Some(c) = RESOLVED.with_borrow(|r| r.get(&p).cloned()) {
        return Some(c);
    }
    match std::fs::canonicalize(&p) {
        Ok(c) => {
            RESOLVED.with_borrow_mut(|r| r.insert(p, c.clone()));
            Some(c)
        }
        Err(_) => Some(p),
    }
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
    let col: usize = text[line_start(text, offset)..offset]
        .encode_utf16()
        .count();
    Position::new(line as u32, col as u32)
}

/// Converts mostly increasing byte offsets to LSP positions without rescanning the text from
/// the start each time.
struct Positions<'a> {
    text: &'a str,
    off: usize,
    line: u32,
}

impl<'a> Positions<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            off: 0,
            line: 0,
        }
    }

    fn at(&mut self, offset: usize) -> Position {
        let offset = offset.min(self.text.len());
        if offset < self.off {
            (self.off, self.line) = (0, 0);
        }
        self.line += self.text[self.off..offset].matches('\n').count() as u32;
        self.off = offset;
        let col = self.text[line_start(self.text, offset)..offset]
            .encode_utf16()
            .count();
        Position::new(self.line, col as u32)
    }
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

fn markdown(value: String) -> MarkupContent {
    MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    }
}

fn md_hover(value: String, range: Range) -> Option<Hover> {
    Some(Hover {
        contents: HoverContents::Markup(markdown(value)),
        range: Some(range),
    })
}

fn range(text: &str, s: usize, e: usize) -> Range {
    Range::new(offset_to_pos(text, s), offset_to_pos(text, e))
}

fn span_range(text: &str, span: Span) -> Range {
    range(text, span.start as usize, span.end as usize)
}

/// A code action editing one document; one fixing a diagnostic is preferred.
fn edit_action(
    title: String,
    kind: CodeActionKind,
    uri: &Uri,
    edits: Vec<TextEdit>,
    diag: Option<&Diagnostic>,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(kind),
        diagnostics: diag.map(|d| vec![d.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(HashMap::from([(uri.clone(), edits)])),
            ..Default::default()
        }),
        is_preferred: diag.map(|_| true),
        ..Default::default()
    })
}

/// An edit inserting `s` at offset `at`.
fn insert_at(text: &str, at: usize, s: impl Into<String>) -> TextEdit {
    TextEdit::new(range(text, at, at), s.into())
}

/// What `pick` gives for the last node of `module` (in source order) it gives something for:
/// of nested nodes, the innermost.
fn last_node<'a, T>(
    module: &'a ast::Module,
    mut pick: impl FnMut(ast::visit::Node<'a>) -> Option<T>,
) -> Option<T> {
    let mut found = None;
    ast::visit::walk(module, &mut |n| found = pick(n).or(found.take()));
    found
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
        self.roots = (p.workspace_folders.unwrap_or_default().iter())
            .filter_map(|f| uri_to_path(&f.uri))
            .collect();
    }

    /// List the workspace's `.nc` files (once), to analyse so that workspace-wide requests and
    /// diagnostics cover the files that are not open.
    fn queue_workspace(&mut self) {
        if self.workspace_queued {
            return;
        }
        self.workspace_queued = true;
        let mut files = Vec::new();
        for root in self.roots.clone() {
            collect_nc_files(&root, false, &mut files, &mut vec![], 0);
        }
        self.unindexed = (files.into_iter().take(1000))
            .map(|f| std::fs::canonicalize(&f).unwrap_or(f))
            .collect();
    }

    /// Analyse up to `n` of the workspace files not analysed yet.
    fn index_some(&mut self, n: usize) {
        self.queue_workspace();
        let mut files = Vec::new();
        while files.len() < n
            && let Some(f) = self.unindexed.pop()
        {
            if !self.analyses.contains_key(&f) {
                files.push(f);
            }
        }
        let out = self.refresh(files);
        self.outbox.extend(out);
    }

    /// The diagnostics of a file: the union of what every analysis found in it.
    fn diagnostics_of(&self, file: &Path) -> Vec<Diagnostic> {
        let diags = (self.reported_by.get(file).into_iter().flatten())
            .flat_map(|entry| self.published.get(entry).into_iter().flatten())
            .filter(|(f, _)| f == file)
            .map(|(_, d)| d.clone())
            .collect();
        sort_diagnostics(diags)
    }

    /// Modules `file` could import, as (name, import path), best first.
    fn importable_modules(&self, file: &Path) -> Importable {
        let Some(dir) = file.parent() else {
            return std::rc::Rc::new([]);
        };
        let mut cache = self.importable.borrow_mut();
        // files appearing, going or being renamed change their directory
        let fresh = cache
            .get(dir)
            .is_some_and(|c| c.dirs.iter().all(|(d, t)| modified(d) == *t));
        if !fresh {
            let (modules, dirs) = actions::importable_modules(dir);
            let dirs = dirs.into_iter().map(|d| {
                let t = modified(&d);
                (d, t)
            });
            let scan = ImportableScan {
                modules: modules.into(),
                dirs: dirs.collect(),
            };
            cache.insert(dir.to_path_buf(), scan);
        }
        let all = std::rc::Rc::clone(&cache[dir].modules);
        drop(cache);
        // a file does not import itself
        let me = file
            .file_stem()
            .map(|n| format!("./{}", n.to_string_lossy()));
        if all.iter().any(|(_, p)| Some(p) == me.as_ref()) {
            all.iter()
                .filter(|(_, p)| Some(p) != me.as_ref())
                .cloned()
                .collect()
        } else {
            all
        }
    }

    /// Import paths of modules named `name` that `file` could import.
    fn find_modules(&self, file: &Path, name: &str) -> Vec<String> {
        (self.importable_modules(file).iter())
            .filter(|(n, _)| n == name)
            .map(|(_, p)| p.clone())
            .collect()
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
            for (f, _) in self.published.remove(&entry).unwrap_or_default() {
                if let Some(entries) = self.reported_by.get_mut(&f) {
                    entries.remove(&entry);
                    if entries.is_empty() {
                        self.reported_by.remove(&f);
                    }
                }
                touched.insert(f);
            }
            touched.insert(entry.clone());
            if !self.docs.contains_key(&entry) && !entry.is_file() {
                // deleted: forget it
                self.analyses.remove(&entry);
                continue;
            }
            let diags = self.analyze(&entry);
            for (f, _) in &diags {
                touched.insert(f.clone());
                (self.reported_by.entry(f.clone()).or_default()).insert(entry.clone());
            }
            self.published.insert(entry, diags);
        }
        touched.retain(|f| !(self.pull && self.docs.contains_key(f)));
        (touched.iter())
            .map(|file| {
                let version = self.versions.get(file).copied();
                publish(file, self.diagnostics_of(file), version)
            })
            .collect()
    }

    /// The syntax tree of the document at `path`, whose text is `text`: parsed once per
    /// version of an open document (analysing it parses it), and once per text otherwise.
    fn module_of(&self, path: &Path, text: &str) -> std::rc::Rc<ast::Module> {
        let version = self.versions.get(path).copied();
        let mut asts = self.asts.borrow_mut();
        if let Some((v, t, module)) = asts.get(path)
            && *v == version
            && (version.is_some() || t.as_deref() == Some(text))
        {
            return std::rc::Rc::clone(module);
        }
        let module = std::rc::Rc::new(crate::parser::parse_module_recover(text, 0).0);
        // files that are not open are kept with their text: not too many of them
        if version.is_none() && asts.len() >= 64 {
            asts.retain(|_, (v, ..)| v.is_some());
        }
        let text = version.is_none().then(|| text.to_string());
        asts.insert(
            path.to_path_buf(),
            (version, text, std::rc::Rc::clone(&module)),
        );
        module
    }

    /// A file's text: the open document's, or what is on disk.
    fn text_of(&self, path: &Path) -> Option<std::borrow::Cow<'_, str>> {
        match self.docs.get(path) {
            Some(t) => Some(std::borrow::Cow::Borrowed(t)),
            None => std::fs::read_to_string(path)
                .ok()
                .map(std::borrow::Cow::Owned),
        }
    }

    /// A document's path and text.
    fn source(&self, uri: &Uri) -> Option<(PathBuf, std::borrow::Cow<'_, str>)> {
        let path = uri_to_path(uri)?;
        let text = self.text_of(&path)?;
        Some((path, text))
    }

    // ------------------------------------------------------------------ notifications

    /// Handle notifications that arrived together, analysing what they change once.
    fn notifications(&mut self, ns: Vec<Notification>) -> Vec<Notification> {
        let mut changed = Vec::new();
        for n in ns {
            if n.method == "workspace/didChangeWatchedFiles" {
                // files changed outside the editor
                let Ok(p) = serde_json::from_value::<DidChangeWatchedFilesParams>(n.params) else {
                    continue;
                };
                changed.extend(
                    (p.changes.iter().filter_map(|c| uri_to_path(&c.uri)))
                        .filter(|p| !self.docs.contains_key(p)),
                );
            } else if n.method == DidSaveTextDocument::METHOD {
                // the document was analysed as it is in the editor already
            } else if let Some(path) = self.sync_document(n) {
                changed.push(path);
            }
        }
        if changed.is_empty() {
            return vec![];
        }
        // the changed files, what imports them, and open documents with an import that did not
        // resolve (it may resolve now)
        let mut entries = changed.clone();
        for path in &changed {
            entries.extend(self.dependents(path));
        }
        entries.extend(
            (self.docs.keys())
                .filter(|d| {
                    self.published.get(*d).is_some_and(|ds| {
                        ds.iter()
                            .any(|(_, d)| diag_code(d) == codes::MISSING_MODULE)
                    })
                })
                .cloned(),
        );
        self.refresh(entries)
    }

    /// Apply a document open, change, save or close; returns the document's path.
    fn sync_document(&mut self, n: Notification) -> Option<PathBuf> {
        fn params<T: serde::de::DeserializeOwned>(n: Notification) -> Option<T> {
            serde_json::from_value(n.params).ok()
        }
        Some(match n.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let p: DidOpenTextDocumentParams = params(n)?;
                let path = uri_to_path(&p.text_document.uri)?;
                self.docs.insert(path.clone(), p.text_document.text);
                self.versions.insert(path.clone(), p.text_document.version);
                path
            }
            DidChangeTextDocument::METHOD => {
                let p: DidChangeTextDocumentParams = params(n)?;
                let path = uri_to_path(&p.text_document.uri)?;
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
            DidCloseTextDocument::METHOD => {
                let p: DidCloseTextDocumentParams = params(n)?;
                let path = uri_to_path(&p.text_document.uri)?;
                self.docs.remove(&path);
                self.versions.remove(&path);
                self.asts.borrow_mut().remove(&path);
                // the file is now analysed as it is on disk (or forgotten when it is not)
                path
            }
            _ => return None,
        })
    }

    /// Analyse an entry file; returns diagnostics per file.
    fn analyze(&mut self, entry: &Path) -> Vec<(PathBuf, Diagnostic)> {
        let mut sm = SourceMap::default();
        let mut warnings = Vec::new();
        let mut index = None;
        let mut imports = HashMap::new();
        let mut clean = false;
        let mut entry_ast = None;
        let mut types = crate::types::Types::default();
        let mut loops = crate::lint::Loops::default();
        let errors =
            match crate::modules::load_with(&mut sm, entry, &self.docs, Some(&mut self.parses)) {
                Err(errs) => errs,
                Ok((graph, mut errors)) => {
                    let loaded = errors.is_empty();
                    entry_ast = Some(std::rc::Rc::clone(&graph.modules[0].ast));
                    for (alias, &m) in &graph.modules[0].imports {
                        imports.insert(alias.clone(), graph.modules[m].path.clone());
                    }
                    let (mut p, errs, ix) = crate::check::check(graph, true);
                    clean = loaded && errs.is_empty();
                    errors.extend(errs);
                    if let Err(errs) = crate::eval::Evaluator::run(&mut p, &sm, false) {
                        errors.extend(errs);
                    }
                    if clean {
                        warnings = crate::lint::race_warnings(&p);
                        loops = crate::lint::loops(&p, &sm);
                    }
                    index = Some(ix);
                    types = std::mem::take(&mut p.types);
                    errors
                }
            };
        use DiagnosticSeverity as Sev;
        let tag = |sev, code| move |d| (d, sev, code);
        // loops that never end are information; the code after one is faded: unreachable
        // code is a warning in debug builds and an error in release builds (lints, and so
        // `loops`, only run on programs that loaded)
        let diags = (errors.into_iter().map(tag(Sev::ERROR, None)))
            .chain(warnings.into_iter().map(tag(Sev::WARNING, None)))
            .chain(
                loops
                    .infinite
                    .into_iter()
                    .map(tag(Sev::INFORMATION, Some("infinite-loop"))),
            )
            .chain(
                (loops.unreachable.into_iter()).map(tag(Sev::WARNING, Some("unreachable-code"))),
            );
        let mut out: Vec<_> = (diags.filter(|_| !sm.files.is_empty()))
            .map(|(d, sev, code)| {
                let faded = code == Some("unreachable-code");
                let tags = faded.then(|| vec![DiagnosticTag::UNNECESSARY]);
                to_diagnostic(&sm, d, sev, code, tags)
            })
            .collect();
        // keep the previous index when the file did not load
        if let Some(index) = index {
            let decl_at = index
                .decls
                .iter()
                .enumerate()
                .map(|(i, d)| (d.span, i))
                .collect();
            let a = Analysis {
                sm,
                index,
                types,
                imports,
                decl_at,
                clean,
            };
            // the entry's syntax tree, as loaded: also what requests on this version use
            if let (Some(ast), Some(f)) = (entry_ast, a.sm.files.first()) {
                let version = self.versions.get(entry).copied();
                let text = version.is_none().then(|| f.text.clone());
                out.extend(actions::warnings(&a, entry, &ast));
                (self.asts.borrow_mut()).insert(entry.to_path_buf(), (version, text, ast));
            }
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
            self.index_some(usize::MAX);
        }
        let params = req.params;
        let result = match req.method.as_str() {
            HoverRequest::METHOD => handle(params, |p| self.hover(p)),
            GotoDefinition::METHOD | "textDocument/declaration" => {
                handle(params, |p| self.definition(p))
            }
            DocumentSymbolRequest::METHOD => handle(params, |p| self.symbols(p)),
            Completion::METHOD => handle(params, |p| self.completion(p)),
            "textDocument/references" => handle(params, |p| self.references(p)),
            "textDocument/documentHighlight" => handle(params, |p| self.highlights(p)),
            "textDocument/prepareRename" => handle(params, |p| self.prepare_rename(p)),
            "textDocument/rename" => handle_fallible(params, |p| self.rename(p)),
            "textDocument/signatureHelp" => handle(params, |p| self.signature_help(p)),
            "textDocument/foldingRange" => handle(params, |p| self.folding(p)),
            "workspace/symbol" => handle(params, |p| self.workspace_symbols(p)),
            "textDocument/formatting" => handle(params, |p: DocumentFormattingParams| {
                self.formatting(&p.text_document.uri, None)
            }),
            "textDocument/codeAction" => handle(params, |p| self.code_actions(p)),
            "textDocument/typeDefinition" => handle(params, |p| self.type_definition(p)),
            "textDocument/inlayHint" => handle(params, |p| self.inlay_hints(p)),
            "textDocument/semanticTokens/full" => handle(params, |p: SemanticTokensParams| {
                self.semantic_tokens(&p.text_document.uri, None)
            }),
            "textDocument/semanticTokens/range" => {
                handle(params, |p: SemanticTokensRangeParams| {
                    self.semantic_tokens(&p.text_document.uri, Some(p.range))
                })
            }
            "textDocument/prepareCallHierarchy" => {
                handle(params, |p| self.prepare_call_hierarchy(p))
            }
            "callHierarchy/incomingCalls" => handle(params, |p| self.incoming_calls(p)),
            "callHierarchy/outgoingCalls" => handle(params, |p| self.outgoing_calls(p)),
            "textDocument/selectionRange" => handle(params, |p| self.selection_ranges(p)),
            "textDocument/documentLink" => handle(params, |p| self.document_links(p)),
            "workspace/willRenameFiles" => handle(params, |p| self.will_rename_files(p)),
            "textDocument/onTypeFormatting" => handle(params, |p| self.on_type_formatting(p)),
            // the parsed syntax tree, for debugging (like rust-analyzer's "view syntax tree")
            "ncc/syntaxTree" => Ok(serde_json::from_value::<TextDocumentIdentifier>(
                params.get("textDocument").cloned().unwrap_or_default(),
            )
            .ok()
            .and_then(|d| {
                let (path, text) = self.source(&d.uri)?;
                let module = self.module_of(&path, &text);
                Some(serde_json::Value::String(format!("{module:#?}")))
            })),
            "textDocument/codeLens" => handle(params, |p| self.code_lenses(p)),
            "textDocument/diagnostic" => handle(params, |p| self.pull_diagnostics(p)),
            "experimental/joinLines" => handle(params, |p| self.join_lines(p)),
            "experimental/ssr" => handle_fallible(params, |p| self.ssr(p)),
            "textDocument/rangeFormatting" => handle(params, |p: DocumentRangeFormattingParams| {
                self.formatting(&p.text_document.uri, Some(p.range))
            }),
            method => {
                let msg = format!("unsupported request `{method}`");
                return Response::new_err(id, lsp_server::ErrorCode::MethodNotFound as i32, msg);
            }
        };
        let msg = match result {
            Ok(Some(v)) => return Response::new_ok(id, v),
            Ok(None) => "invalid parameters".into(),
            Err(msg) => msg,
        };
        Response::new_err(id, lsp_server::ErrorCode::InvalidParams as i32, msg)
    }

    /// Find the innermost indexed reference at a position.
    fn lookup(
        &self,
        uri: &Uri,
        pos: Position,
    ) -> Option<(&Analysis, &crate::check::index::SymRef)> {
        let (a, fid, text) = self.analysis(uri)?;
        let off = pos_to_offset(text, pos) as u32;
        a.ref_at(fid, off).map(|r| (a, r))
    }

    fn hover(&self, p: HoverParams) -> Option<Hover> {
        let pp = p.text_document_position_params;
        if let Some(h) = self.builtin_hover(&pp) {
            return Some(h);
        }
        if let Some(alias) = self.alias_at(&pp.text_document.uri, pp.position)
            && let Some(target) = alias.target
        {
            let module = alias.module_path;
            return md_hover(
                format!("```nc\nmodule {module}\n```\n\n`{}`", target.display()),
                alias.here,
            );
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
            return md_hover(value, span_range(text, r.span));
        }
        // keywords
        if let Some((_, text)) = self.source(&pp.text_document.uri)
            && let Some((s, e)) = word_at(&text, pos_to_offset(&text, pp.position))
            && let Some((_, doc)) = docs::KEYWORD_DOCS.iter().find(|(k, _)| *k == &text[s..e])
            && !in_string_or_comment(&text, s)
        {
            return md_hover(
                format!("```nc\n{}\n```\n\n---\n\n{doc}", &text[s..e]),
                range(&text, s, e),
            );
        }
        // the type of the innermost expression under the cursor
        let (a, fid, text) = self.fresh(&pp.text_document.uri)?;
        let off = pos_to_offset(text, pp.position) as u32;
        let e = a.expr_around(fid, off, |_| true)?;
        md_hover(
            format!("```nc\n{}\n```", a.show(e.ty)),
            span_range(text, e.span),
        )
    }

    /// The analysis of a document, when it was made from the document's current text: its
    /// file id and that text.
    fn fresh(&self, uri: &Uri) -> Option<(&Analysis, u32, &str)> {
        let (a, fid, text) = self.analysis(uri)?;
        if let Some(cur) = self.docs.get(&uri_to_path(uri)?)
            && cur != text
        {
            return None;
        }
        Some((a, fid, text))
    }

    /// The last analysis of a document: its file id and the text it analysed.
    fn analysis(&self, uri: &Uri) -> Option<(&Analysis, u32, &str)> {
        let path = uri_to_path(uri)?;
        let a = self.analyses.get(&path)?;
        let fid = a.file_id(&path)?;
        Some((a, fid, &a.sm.file(fid).text))
    }

    fn definition(&self, p: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let pp = p.text_document_position_params;
        if let Some(target) = self
            .alias_at(&pp.text_document.uri, pp.position)
            .and_then(|a| a.target)
        {
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

    fn symbols(&self, p: DocumentSymbolParams) -> Option<Vec<DocSymbol>> {
        let (path, text) = self.source(&p.text_document.uri)?;
        let module = self.module_of(&path, &text);
        // `span` is the whole declaration, `name_span` its name
        let sym = |name: &str, kind, span, name_span, detail| DocSymbol {
            name: if name.is_empty() { "_" } else { name }.to_string(),
            detail,
            kind,
            range: span_range(&text, span),
            selection_range: span_range(&text, name_span),
            children: None,
        };
        let src = |sp: Span| -> String {
            text.get(sp.start as usize..sp.end as usize)
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        // a function, with `(int a, int b) int` from its signature (whose span covers the
        // name; the parameters and return type run up to the body or `=`)
        let fn_sym = |sig: &ast::FnSig, span: Span| {
            let start = sig.span.end as usize;
            let detail = text.get(start..).and_then(|rest| {
                let end = rest.find(['{', '=', '\n']).unwrap_or(rest.len());
                let t = src(Span::new(sig.span.file, start, start + end));
                t.starts_with('(').then_some(t)
            });
            sym(&sig.name, SymbolKind::FUNCTION, span, sig.span, detail)
        };
        let fn_symbol = |f: &ast::FnDecl| {
            // functions declared directly in the body
            let nested: Vec<DocSymbol> = (f.body.stmts.iter())
                .filter_map(|st| match &st.kind {
                    ast::StmtKind::Fn(g) => Some(fn_sym(&g.sig, g.sig.span.to(g.body.span))),
                    _ => None,
                })
                .collect();
            DocSymbol {
                children: (!nested.is_empty()).then_some(nested),
                ..fn_sym(&f.sig, f.sig.span.to(f.body.span))
            }
        };
        let mut out = Vec::new();
        for it in &module.items {
            match it {
                ast::Item::Fn(f) => out.push(fn_symbol(f)),
                ast::Item::Struct(s) => {
                    let fields = (s.fields.iter())
                        .map(|(t, n, sp)| sym(n, SymbolKind::FIELD, *sp, *sp, Some(src(t.span()))))
                        .collect();
                    let s = sym(&s.name, SymbolKind::STRUCT, s.span, s.name_span, None);
                    out.push(DocSymbol {
                        children: Some(fields),
                        ..s
                    });
                }
                ast::Item::Enum(e) => {
                    let vs = (e.variants.iter())
                        .map(|(n, ts, sp)| {
                            let detail = (!ts.is_empty()).then(|| {
                                let ts: Vec<String> = ts.iter().map(|t| src(t.span())).collect();
                                format!("({})", ts.join(", "))
                            });
                            sym(n, SymbolKind::ENUM_MEMBER, *sp, *sp, detail)
                        })
                        .collect();
                    let e = sym(&e.name, SymbolKind::ENUM, e.span, e.name_span, None);
                    out.push(DocSymbol {
                        children: Some(vs),
                        ..e
                    });
                }
                ast::Item::Type(t) => {
                    let detail = Some(src(t.ty.span()));
                    out.push(sym(
                        &t.name,
                        SymbolKind::TYPE_PARAMETER,
                        t.span,
                        t.name_span,
                        detail,
                    ));
                }
                ast::Item::Test(name, _, span, name_span) => {
                    let detail = Some("test".into());
                    out.push(sym(name, SymbolKind::METHOD, *span, *name_span, detail));
                }
                ast::Item::Extern(e) => {
                    let fs = e.fns.iter().map(|f| fn_sym(&f.sig, f.sig.span)).collect();
                    let x = sym(
                        &e.alias,
                        SymbolKind::MODULE,
                        e.span,
                        e.alias_span,
                        Some(e.path.clone()),
                    );
                    out.push(DocSymbol {
                        children: Some(fs),
                        ..x
                    });
                }
                ast::Item::Import(im) => {
                    let detail = Some(im.path.clone());
                    out.push(sym(
                        &im.alias,
                        SymbolKind::MODULE,
                        im.span,
                        im.alias_span,
                        detail,
                    ));
                }
                ast::Item::Stmt(s) => match &s.kind {
                    ast::StmtKind::VarDecl {
                        targets, mutable, ..
                    } => {
                        let kind = if *mutable {
                            SymbolKind::VARIABLE
                        } else {
                            SymbolKind::CONSTANT
                        };
                        for t in targets {
                            let detail = Some(src(t.ty.span()));
                            out.push(sym(&t.name, kind, t.span, t.name_span, detail));
                        }
                    }
                    ast::StmtKind::FnVar {
                        name, name_span, ..
                    } => out.push(sym(name, SymbolKind::FUNCTION, s.span, *name_span, None)),
                    ast::StmtKind::Fn(f) => out.push(fn_symbol(f)),
                    _ => {}
                },
            }
        }
        Some(out)
    }
}

// ------------------------------------------------------------------ navigation and editing

/// A symbol's identity across analyses: its definition's file and offset.
type SymKey = (PathBuf, u32);

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

    /// The symbol at a position: its key and name.
    fn symbol_at(&self, uri: &Uri, pos: Position) -> Option<(SymKey, String)> {
        let (a, r) = self.lookup(uri, pos)?;
        let def = r.def?;
        let decl = a.decl(def)?;
        Some((Self::key_of(a, def), decl.name.clone()))
    }

    /// Every occurrence of a symbol named `name` (declaration and uses) in all analysed files.
    fn occurrences(&self, key: &SymKey, name: &str) -> Vec<(PathBuf, Range)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for a in self.analyses.values() {
            for r in &a.index.refs {
                let Some(def) = r.def else { continue };
                if def.start != key.1 || a.sm.file(def.file).path != key.0 {
                    continue;
                }
                let f = a.sm.file(r.span.file);
                let ns = r.name_span;
                if f.text.get(ns.start as usize..ns.end as usize) != Some(name) {
                    continue;
                }
                if seen.insert((f.path.clone(), ns.start)) {
                    out.push((f.path.clone(), span_range(&f.text, ns)));
                }
            }
        }
        out.sort_by(|a, b| (&a.0, a.1.start).cmp(&(&b.0, b.1.start)));
        out
    }

    /// The import alias or symbol at a position: its name and its occurrences in the document.
    fn ranges_here(&self, uri: &Uri, pos: Position) -> Option<(String, Vec<Range>)> {
        if let Some(alias) = self.alias_at(uri, pos) {
            return Some((alias.name, alias.ranges));
        }
        let path = uri_to_path(uri)?;
        let (key, name) = self.symbol_at(uri, pos)?;
        let ranges = (self.occurrences(&key, &name).into_iter())
            .filter(|(p, _)| *p == path)
            .map(|(_, r)| r)
            .collect();
        Some((name, ranges))
    }

    fn references(&self, p: ReferenceParams) -> Option<Vec<Location>> {
        let pp = p.text_document_position;
        if let Some(alias) = self.alias_at(&pp.text_document.uri, pp.position) {
            let skip = usize::from(!p.context.include_declaration);
            return Some(
                (alias.ranges.into_iter())
                    .skip(skip)
                    .map(|r| Location::new(pp.text_document.uri.clone(), r))
                    .collect(),
            );
        }
        let (key, name) = self.symbol_at(&pp.text_document.uri, pp.position)?;
        let occ = self.occurrences(&key, &name);
        let decl = if p.context.include_declaration {
            None
        } else {
            occ.iter().find(|(path, _)| *path == key.0).cloned()
        };
        Some(
            occ.into_iter()
                .filter(|o| Some(o) != decl.as_ref())
                .map(|(path, range)| Location::new(path_to_uri(&path), range))
                .collect(),
        )
    }

    fn highlights(&self, p: DocumentHighlightParams) -> Option<Vec<DocumentHighlight>> {
        let pp = p.text_document_position_params;
        // `fn`/`return`/`throw`/`try`: the function and its exit points
        let exits = self.source(&pp.text_document.uri).and_then(|(path, text)| {
            let off = pos_to_offset(&text, pp.position);
            if in_string_or_comment(&text, off) {
                return None;
            }
            refactor::exit_points(&self.module_of(&path, &text), &text, off)
        });
        let ranges = match exits {
            Some(r) => r,
            None => self.ranges_here(&pp.text_document.uri, pp.position)?.1,
        };
        let kind = Some(DocumentHighlightKind::TEXT);
        Some(
            ranges
                .into_iter()
                .map(|range| DocumentHighlight { range, kind })
                .collect(),
        )
    }

    fn prepare_rename(&self, p: TextDocumentPositionParams) -> Option<PrepareRenameResponse> {
        let (name, ranges) = self.ranges_here(&p.text_document.uri, p.position)?;
        let (_, text) = self.source(&p.text_document.uri)?;
        let off = pos_to_offset(&text, p.position);
        // the cursor must be on the name itself
        ranges
            .into_iter()
            .find(|r| range_contains(&text, *r, off))
            .map(|range| PrepareRenameResponse::RangeWithPlaceholder {
                range,
                placeholder: name,
            })
    }

    fn rename(&self, p: RenameParams) -> Result<Option<WorkspaceEdit>, String> {
        let pp = p.text_document_position;
        let new = p.new_name.trim();
        if !valid_name(new) || new == "_" {
            return Err(format!("`{new}` is not a valid name"));
        }
        let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
        let mut rename = |uri: Uri, range: Range| {
            changes
                .entry(uri)
                .or_default()
                .push(TextEdit::new(range, new.to_string()));
        };
        if let Some(alias) = self.alias_at(&pp.text_document.uri, pp.position) {
            for r in alias.ranges {
                rename(pp.text_document.uri.clone(), r);
            }
        } else {
            let Some((key, name)) = self.symbol_at(&pp.text_document.uri, pp.position) else {
                return Ok(None);
            };
            self.check_clash(&key, new)?;
            for (path, range) in self.occurrences(&key, &name) {
                rename(path_to_uri(&path), range);
            }
        }
        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }))
    }

    /// Fails when renaming the symbol `key` to `new` would clash with a declaration.
    fn check_clash(&self, key: &SymKey, new: &str) -> Result<(), String> {
        // a name that is already declared where the symbol is declared would clash
        if let Some(a) = self.analyses.get(&key.0)
            && let Some(fid) = a.file_id(&key.0)
            && let Some(d) = a.decl_at.keys().find(|s| s.file == fid && s.start == key.1)
            && let Some(decl) = a.decl(*d)
        {
            let member = |k| matches!(k, DeclKind::Field | DeclKind::Variant);
            let clash = a.index.decls.iter().any(|o| {
                o.name == new
                    && o.span.file == fid
                    && if member(decl.kind) {
                        // fields and variants clash only with their siblings
                        o.kind == decl.kind
                            && a.index.decls.iter().any(|p| {
                                matches!(p.kind, DeclKind::Struct | DeclKind::Enum)
                                    && p.span.start <= o.span.start.min(decl.span.start)
                                    && o.span.end.max(decl.span.end) <= p.span.end
                            })
                    } else {
                        o.scope == decl.scope && !member(o.kind)
                    }
            });
            if clash {
                return Err(format!("`{new}` is already declared here"));
            }
        }
        Ok(())
    }

    fn builtin_hover(&self, pp: &TextDocumentPositionParams) -> Option<Hover> {
        let (_, text) = self.source(&pp.text_document.uri)?;
        let off = pos_to_offset(&text, pp.position);
        let (s, e) = word_at(&text, off)?;
        let word = text[s..e].strip_prefix('@')?;
        let b = BUILTINS.iter().find(|b| b.name == word)?;
        md_hover(
            format!("```nc\n{}\n```\n\n---\n\n{}", b.label(), b.doc),
            range(&text, s, e),
        )
    }

    fn folding(&self, p: FoldingRangeParams) -> Option<Vec<FoldingRange>> {
        let (_, text) = self.source(&p.text_document.uri)?;
        let (toks, _) = crate::lexer::lex_all(&text, 0, 0);
        let line = |off: u32| offset_to_pos(&text, off as usize).line;
        let mut out = Vec::new();
        // open brackets: (offset, whether it is an import block)
        let mut stack: Vec<(u32, bool)> = Vec::new();
        for (i, t) in toks.iter().enumerate() {
            match t.tok {
                Tok::LBrace | Tok::LBracket | Tok::LParen => {
                    let import =
                        i > 0 && matches!(&toks[i - 1].tok, Tok::Ident(k) if k == "import");
                    stack.push((t.span.start, import));
                }
                Tok::RBrace | Tok::RBracket | Tok::RParen => {
                    if let Some((open, import)) = stack.pop() {
                        let (a, b) = (line(open), line(t.span.start));
                        if b > a {
                            out.push(FoldingRange {
                                start_line: a,
                                end_line: b - 1,
                                kind: import.then_some(FoldingRangeKind::Imports),
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
                let span = d.name_span;
                out.push(WorkspaceSymbol {
                    name: d.name.clone(),
                    kind,
                    tags: None,
                    container_name: f.path.file_name().map(|n| n.to_string_lossy().into_owned()),
                    location: OneOf::Left(Location::new(
                        path_to_uri(&f.path),
                        span_range(&f.text, span),
                    )),
                    data: None,
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        WorkspaceSymbolResponse::Nested(out)
    }
}

impl Server {
    /// Formatting with `ncc fmt`'s rules (nothing when the document has syntax errors). With a
    /// range, the whole document is formatted and only the changes to its lines are kept.
    fn formatting(&self, uri: &Uri, range: Option<Range>) -> Option<Vec<TextEdit>> {
        let (_, text) = self.source(uri)?;
        let formatted = crate::fmt::format(&text).ok()?;
        Some(minimal_edits(&text, &formatted, range))
    }

    /// After a `}` that starts its line, indent it like the line of its `{`.
    fn on_type_formatting(&self, p: DocumentOnTypeFormattingParams) -> Option<Vec<TextEdit>> {
        let (_, text) = self.source(&p.text_document_position.text_document.uri)?;
        let off = pos_to_offset(&text, p.text_document_position.position);
        let (toks, _) = crate::lexer::lex_all(&text, 0, 0);
        let close = toks
            .iter()
            .position(|t| t.tok == Tok::RBrace && t.span.end as usize == off)?;
        let ls = line_start(&text, off - 1);
        if !text[ls..off - 1].trim().is_empty() {
            return Some(vec![]);
        }
        let mut depth = 0;
        let open = toks[..close].iter().rev().find(|t| match t.tok {
            Tok::RBrace => {
                depth += 1;
                false
            }
            Tok::LBrace if depth > 0 => {
                depth -= 1;
                false
            }
            Tok::LBrace => true,
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
}

/// The extents of the string and character literals and comments of `text`.
fn literals_and_comments(text: &str) -> Vec<(usize, usize)> {
    let (toks, _) = crate::lexer::lex_all(text, 0, 0);
    (toks.iter())
        .filter(|t| matches!(t.tok, Tok::Str(_) | Tok::Char(_)))
        .map(|t| (t.span.start as usize, t.span.end as usize))
        .chain(crate::lexer::comments(text))
        .collect()
}

/// Whether `n` can name a declaration: an identifier that is not a keyword.
fn valid_name(n: &str) -> bool {
    n.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && n.bytes().all(is_ident_byte)
        && !crate::parser::is_keyword(n)
}

/// Whether an offset is inside a string literal or a comment.
fn in_string_or_comment(text: &str, off: usize) -> bool {
    literals_and_comments(text)
        .iter()
        .any(|&(s, e)| s <= off && off < e)
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
    // where the changed lines being collected start in `a` and `b`
    let mut changed: Option<(usize, usize)> = None;
    loop {
        let same = i < n && j < m && a[i] == b[j];
        if same || (i == n && j == m) {
            if let Some((os, ns)) = changed.take() {
                edits.push(TextEdit::new(
                    Range::new(Position::new(os as u32, 0), Position::new(i as u32, 0)),
                    b[ns..j].concat(),
                ));
            }
            if !same {
                break;
            }
            i += 1;
            j += 1;
        } else {
            changed.get_or_insert((i, j));
            if j < m && (i == n || dp[i * (m + 1) + j + 1] >= dp[(i + 1) * (m + 1) + j]) {
                j += 1;
            } else {
                i += 1;
            }
        }
    }
    // an old text without a final newline: positions past its last line are clamped by clients
    if let Some(r) = range {
        edits.retain(|e| e.range.start.line <= r.end.line && e.range.end.line >= r.start.line);
    }
    edits
}

/// `.nc` files under a directory, skipping hidden directories and build output unless `all`
/// is set (dependencies live under `.nc/`).
fn collect_nc_files(
    dir: &Path,
    all: bool,
    out: &mut Vec<PathBuf>,
    dirs: &mut Vec<PathBuf>,
    depth: usize,
) {
    if depth > 12 {
        return;
    }
    dirs.push(dir.to_path_buf());
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
            Ok(t) if t.is_dir() => collect_nc_files(&p, all, out, dirs, depth + 1),
            Ok(t) if t.is_file() && name.ends_with(".nc") => out.push(p),
            _ => {}
        }
    }
}

/// Whether `range` of `text` contains the offset `off`.
fn range_contains(text: &str, range: Range, off: usize) -> bool {
    pos_to_offset(text, range.start) <= off && off <= pos_to_offset(text, range.end)
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

/// How long the server waits for the next message before analysing: an edit for the next
/// edit, and workspace indexing for the client to go quiet.
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(20);

/// A document's syntax tree: the version it was parsed at when open, its text when not.
type DocAst = (Option<i32>, Option<String>, std::rc::Rc<ast::Module>);

/// Modules that can be imported, as (name, import path).
type Importable = std::rc::Rc<[(String, String)]>;

/// Importable modules found by scanning directories, valid while none of them changes.
struct ImportableScan {
    modules: Importable,
    /// the directories scanned, with their modification times when scanned
    dirs: Vec<(PathBuf, Option<std::time::SystemTime>)>,
}

fn modified(dir: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}

/// Diagnostics in order of position, without repeats.
fn sort_diagnostics(mut diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    diags.sort_by(|a, b| (a.range.start, &a.message).cmp(&(b.range.start, &b.message)));
    diags.dedup_by(|a, b| a.range == b.range && a.message == b.message);
    diags
}

/// A compiler diagnostic as an LSP diagnostic, with the path of its file.
fn to_diagnostic(
    sm: &SourceMap,
    d: crate::diag::Diag,
    severity: DiagnosticSeverity,
    code: Option<&str>,
    tags: Option<Vec<DiagnosticTag>>,
) -> (PathBuf, Diagnostic) {
    let f = sm.file(d.span.file);
    (
        f.path.clone(),
        Diagnostic {
            range: span_range(&f.text, d.span),
            severity: Some(severity),
            code: code.or(d.code).map(|c| NumberOrString::String(c.into())),
            source: Some("ncc".into()),
            message: d.msg,
            tags,
            data: (!d.data.is_empty()).then(|| serde_json::json!(d.data)),
            ..Default::default()
        },
    )
}

/// The code of a diagnostic, or "".
fn diag_code(d: &Diagnostic) -> &str {
    match &d.code {
        Some(NumberOrString::String(c)) => c,
        _ => "",
    }
}

/// The `i`th value a compiler diagnostic is about (see `Diag::data`).
fn diag_data(d: &Diagnostic, i: usize) -> Option<&str> {
    d.data.as_ref()?.get(i)?.as_str()
}

/// What a request's handler gives: its serialized result, `None` for parameters that do not
/// deserialize, or an error message.
type Handled = Result<Option<serde_json::Value>, String>;

/// Deserialize a request's parameters, run its handler and serialize the result.
fn handle<P: serde::de::DeserializeOwned, R: serde::Serialize>(
    params: serde_json::Value,
    f: impl FnOnce(P) -> R,
) -> Handled {
    handle_fallible(params, |p| Ok(f(p)))
}

/// Like `handle`, for a handler that can reject its parameters with a message.
fn handle_fallible<P: serde::de::DeserializeOwned, R: serde::Serialize>(
    params: serde_json::Value,
    f: impl FnOnce(P) -> Result<R, String>,
) -> Handled {
    let Ok(p) = serde_json::from_value(params) else {
        return Ok(None);
    };
    Ok(Some(serde_json::to_value(f(p)?).unwrap()))
}
