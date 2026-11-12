//! Text clean-up of a generated function body, after `body` has left out unreachable code:
//!
//! - braces of bare blocks that scope nothing;
//! - empty bare blocks;
//! - `(void)x;` markers for variables that are used anyway;
//! - temporaries holding the value of an expression statement that nothing reads.
//!
//! It relies on the shape of generated code: one statement per line, `{` at the end of a line
//! opening a block and `}` at the start of a line closing one.

use std::collections::{HashMap, HashSet};

use super::is_ident;

fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

pub fn tidy(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let all = decls_unique(&lines);
    let lines = flatten_blocks(&lines, all);
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let lines = drop_unused_temps(&strip_empty_blocks(strip_void_markers(&refs)));
    let mut out = String::with_capacity(body.len());
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// The variable a declaration line declares (`T name = ...;`, `T *name = ...;`).
fn declared(t: &str) -> Option<&str> {
    let (lhs, _) = t.split_once(" = ")?;
    if lhs.starts_with(['{', '*', '(']) || lhs.contains(['(', '[', '.', '-']) {
        return None;
    }
    let (_, name) = lhs.rsplit_once(' ')?;
    let name = name.trim_start_matches('*');
    is_ident(name).then_some(name)
}

/// Whether no variable is declared twice in the body, so blocks can be merged freely.
fn decls_unique(lines: &[&str]) -> bool {
    let mut seen = HashSet::new();
    lines
        .iter()
        .filter_map(|l| declared(l.trim()))
        .all(|n| seen.insert(n))
}

/// Remove the braces of bare blocks: those that end their enclosing block, or all of them when
/// `all` (no name is declared twice, so the wider scopes change nothing; generated code only
/// jumps forward and never reads a variable whose declaration it jumped over).
fn flatten_blocks(lines: &[&str], all: bool) -> Vec<String> {
    let mut dedent = vec![0isize; lines.len() + 1];
    let mut drop = vec![false; lines.len()];
    let mut stack: Vec<(usize, bool)> = Vec::new();
    for (j, l) in lines.iter().enumerate() {
        let t = l.trim();
        if t.starts_with('}') {
            let Some((i, plain)) = stack.pop() else {
                return lines.iter().map(|l| l.to_string()).collect();
            };
            let last = lines.get(j + 1).is_none_or(|n| n.trim().starts_with('}'));
            if plain && t == "}" && (all || last) {
                drop[i] = true;
                drop[j] = true;
                dedent[i + 1] += 1;
                dedent[j] -= 1;
            }
        }
        if t.ends_with('{') {
            stack.push((j, t == "{"));
        }
    }
    let mut d = 0isize;
    let mut out = Vec::with_capacity(lines.len());
    for (k, l) in lines.iter().enumerate() {
        d += dedent[k];
        if !drop[k] {
            out.push(l[(2 * d as usize).min(indent(l))..].to_string());
        }
    }
    out
}

/// Drop `(void)x;` lines for variables that are mentioned elsewhere besides their declaration.
fn strip_void_markers<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    fn marker(l: &str) -> Option<&str> {
        let name = l.trim().strip_prefix("(void)")?.strip_suffix(';')?;
        is_ident(name).then_some(name)
    }
    let uses = mentions(lines.iter().filter(|l| marker(l).is_none()).copied());
    // one mention is the declaration; more means the variable is used
    (lines.iter())
        .filter(|l| marker(l).is_none_or(|n| uses.get(n).is_none_or(|&u| u <= 1)))
        .copied()
        .collect()
}

/// How many times each identifier is mentioned in the lines.
fn mentions<'a>(lines: impl Iterator<Item = &'a str>) -> HashMap<&'a str, usize> {
    let mut uses = HashMap::new();
    for w in lines.flat_map(super::idents) {
        *uses.entry(w).or_default() += 1;
    }
    uses
}

/// Drop bare `{` `}` pairs with nothing between them.
fn strip_empty_blocks(lines: Vec<&str>) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    for l in lines {
        if l.trim() == "}"
            && let Some(prev) = out.last()
            && prev.trim() == "{"
            && indent(prev) == indent(l)
        {
            out.pop();
            continue;
        }
        out.push(l);
    }
    out
}

/// Turn `T tN = expr;` into `expr;` when the temporary `tN` is never read (the value of an
/// expression statement that needs no release).
fn drop_unused_temps(lines: &[&str]) -> Vec<String> {
    fn temp(t: &str) -> Option<(usize, &str)> {
        let (decl, _) = t.split_once(" = ")?;
        let name = decl.rsplit(' ').next()?;
        let digits = name.strip_prefix('t')?;
        (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && t.ends_with(';'))
            .then_some((decl.len() + 3, name))
    }
    let uses = mentions(lines.iter().copied());
    lines
        .iter()
        .map(|l| {
            let t = l.trim_start();
            match temp(t) {
                Some((start, name)) if uses.get(name) == Some(&1) => {
                    format!("{}{}", &l[..indent(l)], &t[start..])
                }
                _ => l.to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::tidy;

    #[test]
    fn flattens_blocks_and_drops_empty_ones() {
        let body = "  {\n    if (c) goto arm1;\n    {\n      x = 1;\n    }\n  }\n  arm1:;\n  {\n    {\n    }\n  }\n  return x;\n";
        assert_eq!(
            tidy(body),
            "  if (c) goto arm1;\n  x = 1;\n  arm1:;\n  return x;\n"
        );
    }

    #[test]
    fn keeps_blocks_when_names_repeat() {
        let body = "  {\n    int t1 = 1;\n    g(t1);\n  }\n  {\n    int t1 = 2;\n    g(t1);\n  }\n";
        assert_eq!(
            tidy(body),
            "  {\n    int t1 = 1;\n    g(t1);\n  }\n  int t1 = 2;\n  g(t1);\n"
        );
    }
}
