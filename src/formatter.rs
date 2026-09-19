use crate::{diagnostic::Diagnostics, lexer};
/// Validates source now; canonical pretty-printing is intentionally token based.
pub fn format(source: &str) -> Result<String, Diagnostics> {
    lexer::lex(source)?;
    Ok(source.trim_end().to_owned() + "\n")
}
