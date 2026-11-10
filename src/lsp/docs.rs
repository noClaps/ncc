//! Doc comments: the `///` lines directly above a declaration, rendered as Markdown.
//!
//! `@param` and `@returns` lines become a parameter list and a returns line:
//!
//! ```nc
//! /// Squares every number
//! /// @param nums The numbers to square
//! /// @returns The squares
//! fn square_all(int[] nums) int[] { ... }
//! ```

use crate::diag::Span;

/// The raw doc comment lines (without `///`) above the line that contains `span.start`.
fn doc_lines(text: &str, span: Span) -> Vec<&str> {
    let start = (span.start as usize).min(text.len());
    let line_start = text[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let mut lines = Vec::new();
    let mut end = line_start;
    while end > 0 {
        let prev_start = text[..end - 1].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line = text[prev_start..end - 1].trim();
        let Some(rest) = line.strip_prefix("///") else {
            break;
        };
        lines.push(
            rest.trim_start_matches('/')
                .strip_prefix(' ')
                .unwrap_or(rest),
        );
        end = prev_start;
    }
    lines.reverse();
    lines
}

/// The rendered doc comment of the declaration at `span`, if it has one.
pub fn doc_comment(text: &str, span: Span) -> Option<String> {
    let lines = doc_lines(text, span);
    if lines.is_empty() {
        return None;
    }
    let mut body = Vec::new();
    let mut params = Vec::new();
    let mut returns = Vec::new();
    for l in lines {
        let t = l.trim_start();
        if let Some(p) = t.strip_prefix("@param") {
            params.push(p.trim().to_string());
        } else if let Some(r) = t
            .strip_prefix("@returns")
            .or_else(|| t.strip_prefix("@return"))
        {
            returns.push(r.trim().to_string());
        } else {
            body.push(l.to_string());
        }
    }
    let mut out = body.join("\n").trim().to_string();
    if !params.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("**Parameters**\n");
        for p in &params {
            out.push_str(&format!("\n- {}", render_param(p)));
        }
    }
    if !returns.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!("**Returns** {}", returns.join(" ")));
    }
    (!out.is_empty()).then_some(out)
}

/// `nums The numbers` -> "`nums`: The numbers" when the first word looks like a parameter name.
fn render_param(p: &str) -> String {
    match p.split_once(' ') {
        Some((name, rest))
            if !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase() || c == '_') =>
        {
            format!("`{name}`: {}", rest.trim())
        }
        _ => p.to_string(),
    }
}

/// The parameter documentation from a rendered doc comment, in order.
pub fn param_docs(doc: &str) -> Vec<String> {
    let Some((_, list)) = doc.split_once("**Parameters**\n") else {
        return vec![];
    };
    list.lines()
        .take_while(|l| !l.starts_with("**Returns**"))
        .filter_map(|l| l.strip_prefix("- "))
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_doc_comments() {
        let src = "int a = 1\n\n/// Adds two numbers\n/// @param x the first\n/// @param y the second\n/// @returns their sum\nfn add(int x, int y) int {\n  return x + y\n}\n";
        let at = src.find("fn add").unwrap();
        let d = doc_comment(src, Span::new(0, at, at + 3)).unwrap();
        assert_eq!(
            d,
            "Adds two numbers\n\n**Parameters**\n\n- `x`: the first\n- `y`: the second\n\n**Returns** their sum"
        );
        assert_eq!(param_docs(&d), vec!["`x`: the first", "`y`: the second"]);
        // a plain comment is not documentation
        let src = "// note\nint a = 1\n";
        assert_eq!(doc_comment(src, Span::new(0, 8, 9)), None);
    }
}
