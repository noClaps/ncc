use crate::{
    diagnostic::Diagnostics,
    lexer::{self, TokenKind},
    parser,
};
/// Canonical two-space indentation, preserving literal contents and comments.
pub fn format(source: &str) -> Result<String, Diagnostics> {
    let tokens = lexer::lex(source)?;
    parser::parse(tokens.clone())?;
    let mut output = String::new();
    let mut depth = 0usize;
    let mut offset = 0;
    let mut blank = false;
    for line in source.split_inclusive('\n') {
        let end = offset + line.len();
        let literal_continuation = tokens
            .iter()
            .any(|t| t.span.start < offset && t.span.end > offset);
        if literal_continuation {
            output.push_str(line);
        } else {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if !blank && !output.is_empty() {
                    output.push('\n');
                }
                blank = true;
            } else {
                let leading_close = tokens
                    .iter()
                    .find(|t| t.span.start >= offset && t.span.start < end)
                    .is_some_and(|t| matches!(t.kind, TokenKind::RBrace | TokenKind::RBracket));
                output.push_str(&"  ".repeat(depth.saturating_sub(usize::from(leading_close))));
                let literal_on_line = tokens
                    .iter()
                    .any(|t| t.span.start >= offset && t.span.start < end && t.span.end >= end);
                output.push_str(if literal_on_line {
                    line.trim_start().trim_end_matches('\n')
                } else {
                    trimmed
                });
                output.push('\n');
                blank = false;
            }
        }
        for token in tokens
            .iter()
            .filter(|t| t.span.start >= offset && t.span.start < end)
        {
            match token.kind {
                TokenKind::LBrace | TokenKind::LBracket => depth += 1,
                TokenKind::RBrace | TokenKind::RBracket => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        offset = end;
    }
    while output.ends_with("\n\n") {
        output.pop();
    }
    Ok(output)
}
