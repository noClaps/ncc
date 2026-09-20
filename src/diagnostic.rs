use std::{fmt, ops::Range};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Diagnostics(pub Vec<Diagnostic>);

impl Diagnostics {
    pub fn render(&self, source: &str, path: &std::path::Path) -> String {
        let mut out = String::new();
        for diagnostic in &self.0 {
            let mut start = diagnostic.span.start.min(source.len());
            while !source.is_char_boundary(start) {
                start -= 1;
            }
            let line_start = source[..start].rfind('\n').map_or(0, |x| x + 1);
            let line_end = source[start..]
                .find('\n')
                .map_or(source.len(), |x| start + x);
            let line = source[..start].bytes().filter(|b| *b == b'\n').count() + 1;
            let column = source[line_start..start].chars().count() + 1;
            out.push_str(&format!(
                "{}:{line}:{column}: error: {}\n  |\n{line:>2} | {}\n  | {}^\n",
                path.display(),
                diagnostic.message,
                &source[line_start..line_end],
                " ".repeat(column - 1)
            ));
        }
        out
    }
    pub fn one(message: impl Into<String>, span: Range<usize>) -> Self {
        Self(vec![Diagnostic {
            message: message.into(),
            span,
        }])
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for diagnostic in &self.0 {
            writeln!(
                f,
                "error at {}..{}: {}",
                diagnostic.span.start, diagnostic.span.end, diagnostic.message
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostics {}
