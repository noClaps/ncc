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

/// Short documentation for keywords, shown on hover.
pub const KEYWORD_DOCS: &[(&str, &str)] = &[
    (
        "fn",
        "Declares a function: `fn name(int a) int { ... }`. Functions are also values.",
    ),
    (
        "return",
        "Returns from the enclosing function, with a value unless it returns `void`.",
    ),
    (
        "if",
        "Branches on conditions (`if { cond -> {...} _ -> {...} }`) or matches a value against patterns (`if value { pattern -> {...} }`). The branches must be exhaustive.",
    ),
    (
        "else",
        "`value else fallback` unwraps an optional, using the fallback (a value or a block) when it is `none`.",
    ),
    ("while", "Loops while a condition holds."),
    (
        "for",
        "`for i in items { ... }` loops over the indices of an array or string (`uint`), or the keys of a map.",
    ),
    (
        "in",
        "Separates the variable from the collection in `for`; as an operator, tests membership.",
    ),
    (
        "break",
        "Leaves the innermost loop or `if`, optionally with a value (`break value`) or to a label (`break :label`).",
    ),
    (
        "continue",
        "Starts the next iteration of the innermost loop (or the labelled one).",
    ),
    (
        "mut",
        "Declares a mutable variable: `mut int n = 0`. Variables are immutable by default.",
    ),
    (
        "mutex",
        "Declares a variable shared between futures; it can only be used inside `lock`.",
    ),
    (
        "lock",
        "`lock name { ... }` locks a `mutex` variable for the block, where it can be read and changed.",
    ),
    (
        "struct",
        "Declares a struct type with named fields: `struct Point { int x int y }`.",
    ),
    (
        "enum",
        "Declares an enum type; its variants can hold values: `enum Shape { Circle(float) Square }`.",
    ),
    ("type", "Declares a named type: `type Meters = float`."),
    (
        "import",
        "Imports modules: `import { \"./path\" as alias }`. Their public items are used as `alias.name`.",
    ),
    ("as", "Names an import or extern block."),
    (
        "extern",
        "Declares functions implemented in a C file: `extern \"file.c\" as c { fn f(int a) int = \"c_f\" }`.",
    ),
    (
        "pub",
        "Makes a module-level declaration visible to modules that import this one.",
    ),
    (
        "test",
        "Declares a test: `test \"name\" { ... }`, run with `ncc test`.",
    ),
    (
        "assert",
        "Fails the current test when the condition is false. Only allowed in `test` blocks.",
    ),
    ("true", "The `bool` value true."),
    ("false", "The `bool` value false."),
    ("none", "The empty value of an optional type (`T?`)."),
    ("and", "Logical and, evaluated lazily."),
    ("or", "Logical or, evaluated lazily."),
    ("not", "Logical negation."),
    (
        "try",
        "`try value` unwraps an error union, returning the error from the enclosing throwing function.",
    ),
    (
        "catch",
        "`value catch err { ... }` handles the error of an error union; the block can `break` with a fallback value.",
    ),
    (
        "throw",
        "Throws an error from a throwing function (one whose return type ends with `!`).",
    ),
    (
        "async",
        "`async f()` starts a function call in the background and gives a `fut` value.",
    ),
    ("await", "Waits for a future and gives its value."),
    (
        "fut",
        "The type of a value computed in the background: `fut int`.",
    ),
];

/// The raw doc comment lines (without `///`) above the line that contains `span.start`.
fn doc_lines(text: &str, span: Span) -> Vec<&str> {
    let start = (span.start as usize).min(text.len());
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let mut lines = Vec::new();
    let mut end = line_start;
    while end > 0 {
        let prev_start = text[..end - 1].rfind('\n').map_or(0, |i| i + 1);
        let line = text[prev_start..end - 1].trim();
        let Some(rest) = line.strip_prefix("///") else {
            break;
        };
        let rest = rest.trim_start_matches('/');
        lines.push(rest.strip_prefix(' ').unwrap_or(rest));
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
            if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
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
        .map(ToString::to_string)
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
