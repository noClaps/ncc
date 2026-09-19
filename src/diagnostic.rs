use std::{fmt, ops::Range};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Diagnostics(pub Vec<Diagnostic>);

impl Diagnostics {
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
