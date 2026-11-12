//! Source files, spans and diagnostics.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub file: u32,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(file: u32, start: usize, end: usize) -> Self {
        Self {
            file,
            start: start as u32,
            end: end as u32,
        }
    }
    pub fn to(self, other: Self) -> Self {
        Self {
            file: self.file,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}

pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    pub fn new(path: PathBuf, text: String) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self {
            path,
            text,
            line_starts,
        }
    }

    /// 1-based line and column of a byte offset.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let col = self.text[self.line_starts[line]..offset.min(self.text.len())]
            .chars()
            .count();
        (line + 1, col + 1)
    }

    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_starts[line - 1];
        let end = self
            .line_starts
            .get(line)
            .map_or(self.text.len(), |e| e - 1);
        &self.text[start..end.max(start)]
    }
}

#[derive(Default)]
pub struct SourceMap {
    pub files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn add(&mut self, path: PathBuf, text: String) -> u32 {
        self.files.push(SourceFile::new(path, text));
        (self.files.len() - 1) as u32
    }
    pub fn file(&self, id: u32) -> &SourceFile {
        &self.files[id as usize]
    }

    /// `path:line` of a span, the path relative to the current directory when inside it.
    pub fn location(&self, span: Span) -> String {
        if self.files.is_empty() {
            return "?".into();
        }
        let f = self.file(span.file);
        let (line, _) = f.line_col(span.start as usize);
        let cwd = std::env::current_dir().unwrap_or_default();
        let path = f.path.strip_prefix(&cwd).unwrap_or(&f.path);
        format!("{}:{line}", path.display())
    }
}

#[derive(Debug, Clone)]
pub struct Diag {
    pub span: Span,
    pub msg: String,
    /// A stable name for the kind of error, for tools that act on it (such as quick fixes).
    pub code: Option<&'static str>,
    /// Values the error is about (names, paths), in an order fixed by `code`.
    pub data: Vec<String>,
}

impl Diag {
    pub fn new(span: Span, msg: impl Into<String>) -> Self {
        Self {
            span,
            msg: msg.into(),
            code: None,
            data: vec![],
        }
    }

    /// Tag the error with a code and the values it is about.
    pub fn with_code(mut self, code: &'static str, data: &[&str]) -> Self {
        self.code = Some(code);
        self.data = data.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn render(&self, sm: &SourceMap) -> String {
        self.render_as("error", sm)
    }

    pub fn render_as(&self, kind: &str, sm: &SourceMap) -> String {
        if sm.files.is_empty() {
            return format!("{kind}: {}", self.msg);
        }
        let f = sm.file(self.span.file);
        let (line, col) = f.line_col(self.span.start as usize);
        let text = f.line_text(line);
        let width = (self.span.end.saturating_sub(self.span.start) as usize).max(1);
        let width = width.min(text.chars().count().saturating_sub(col - 1).max(1));
        format!(
            "{kind}: {}\n  --> {}:{}:{}\n   |\n{:>3}| {}\n   | {}{}",
            self.msg,
            f.path.display(),
            line,
            col,
            line,
            text,
            " ".repeat(col - 1),
            "^".repeat(width)
        )
    }
}

/// Codes of the errors tools act on. `data` lists what each error carries.
pub mod codes {
    /// assignment to an immutable variable
    pub const IMMUTABLE: &str = "immutable";
    /// a value computed and dropped
    pub const UNUSED_VALUE: &str = "unused-value";
    /// `[name]`: an unknown name or variable
    pub const UNKNOWN_NAME: &str = "unknown-name";
    /// `[name]`: an unknown type
    pub const UNKNOWN_TYPE: &str = "unknown-type";
    /// `try` or `throw` outside a throwing function
    pub const NEEDS_THROWING: &str = "needs-throwing";
    /// `[field]`: a struct literal leaves out a field
    pub const MISSING_FIELD: &str = "missing-field";
    /// `[field]`: a struct pattern leaves out a field
    pub const MISSING_PATTERN_FIELD: &str = "missing-pattern-field";
    /// `if` branches that do not cover every value
    pub const NOT_EXHAUSTIVE: &str = "not-exhaustive";
    /// `[import path, file looked for]`: an import whose file does not exist
    pub const MISSING_MODULE: &str = "missing-module";
    /// `[name, module alias]`: a use of a private declaration of another module
    pub const NOT_PUBLIC: &str = "not-public";
}

/// Sort diagnostics by position and drop exact repeats.
pub fn sort_dedup(v: &mut Vec<Diag>) {
    v.sort_by_key(|d| (d.span.file, d.span.start));
    v.dedup_by(|a, b| a.span == b.span && a.msg == b.msg);
}

pub type Result<T> = std::result::Result<T, Diag>;

/// Result carrying every error found.
pub type Results<T> = std::result::Result<T, Vec<Diag>>;

pub fn err<T>(span: Span, msg: impl Into<String>) -> Result<T> {
    Err(Diag::new(span, msg))
}

/// `err` with a code and the values the error is about (see `Diag::with_code`).
pub fn err_code<T>(
    span: Span,
    msg: impl Into<String>,
    code: &'static str,
    data: &[&str],
) -> Result<T> {
    Err(Diag::new(span, msg).with_code(code, data))
}
