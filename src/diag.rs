//! Source files, spans and diagnostics.

use std::fmt;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub file: u32,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(file: u32, start: usize, end: usize) -> Span {
        Span { file, start: start as u32, end: end as u32 }
    }
    pub fn to(self, other: Span) -> Span {
        Span { file: self.file, start: self.start.min(other.start), end: self.end.max(other.end) }
    }
}

pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    pub fn new(path: PathBuf, text: String) -> SourceFile {
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        SourceFile { path, text, line_starts }
    }

    /// 1-based line and column of a byte offset.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let col = self.text[self.line_starts[line]..offset.min(self.text.len())].chars().count();
        (line + 1, col + 1)
    }

    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_starts[line - 1];
        let end = self.line_starts.get(line).map(|e| e - 1).unwrap_or(self.text.len());
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
    /// `path:line` for runtime panic locations.
    pub fn location(&self, span: Span) -> String {
        let f = self.file(span.file);
        let (l, _) = f.line_col(span.start as usize);
        format!("{}:{}", f.path.display(), l)
    }
}

#[derive(Debug, Clone)]
pub struct Diag {
    pub span: Span,
    pub msg: String,
}

impl Diag {
    pub fn new(span: Span, msg: impl Into<String>) -> Diag {
        Diag { span, msg: msg.into() }
    }
    pub fn render(&self, sm: &SourceMap) -> String {
        if sm.files.is_empty() {
            return format!("error: {}", self.msg);
        }
        let f = sm.file(self.span.file);
        let (line, col) = f.line_col(self.span.start as usize);
        let text = f.line_text(line);
        let width = (self.span.end.saturating_sub(self.span.start) as usize).max(1);
        let width = width.min(text.chars().count().saturating_sub(col - 1).max(1));
        format!(
            "error: {}\n  --> {}:{}:{}\n   |\n{:>3}| {}\n   | {}{}",
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

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

pub type Result<T> = std::result::Result<T, Diag>;

pub fn err<T>(span: Span, msg: impl Into<String>) -> Result<T> {
    Err(Diag::new(span, msg))
}
