//! Structural search and replace, join lines (both as rust-analyzer's `experimental/` requests)
//! and pulled diagnostics (`textDocument/diagnostic`).
//!
//! SSR rules look like `add($a, $b) ==>> $a + $b`. A `$name` placeholder matches any
//! expression; a placeholder used twice must match the same text. Matches are whole
//! expressions, so `$a + $b` against `x + y + z` binds `x + y` and `z`. A match inside another
//! match is left as it is (run the rule again to rewrite it).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use lsp_types::*;
use serde::Deserialize;

use super::nav::{Node, walk};
use super::{Server, line_end, offset_to_pos, path_to_uri, pos_to_offset, range, uri_to_path};
use crate::ast::{self, ExprKind as E};
use crate::lexer::Tok;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinLinesParams {
    pub text_document: TextDocumentIdentifier,
    pub ranges: Vec<Range>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SsrParams {
    pub query: String,
    #[serde(default)]
    pub parse_only: bool,
}

/// A token of an SSR pattern: a placeholder or literal source text.
#[derive(Debug, PartialEq)]
enum PTok {
    Hole(String),
    Text(String),
}

/// Split `a ==>> b` and tokenize the pattern.
fn parse_rule(query: &str) -> Result<(Vec<PTok>, String), String> {
    let (pat, tmpl) = query
        .split_once("==>>")
        .ok_or("expected a rule like `pattern ==>> replacement`")?;
    let (pat, tmpl) = (pat.trim(), tmpl.trim());
    let (toks, errs) = crate::lexer::lex_all(pat, 0, 0);
    if let Some(e) = errs.first() {
        return Err(format!("invalid pattern: {}", e.msg));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        match &t.tok {
            Tok::Newline | Tok::Eof => {}
            Tok::Dollar => match toks.get(i + 1).map(|t| &t.tok) {
                Some(Tok::Ident(n)) => {
                    out.push(PTok::Hole(n.clone()));
                    i += 1;
                }
                _ => return Err("`$` must be followed by a placeholder name".into()),
            },
            _ => out.push(PTok::Text(
                pat[t.span.start as usize..t.span.end as usize].to_string(),
            )),
        }
        i += 1;
    }
    if out.is_empty() {
        return Err("the pattern is empty".into());
    }
    let holes: HashSet<&str> = out
        .iter()
        .filter_map(|t| match t {
            PTok::Hole(n) => Some(n.as_str()),
            _ => None,
        })
        .collect();
    // every placeholder of the replacement must be bound by the pattern
    let mut rest = tmpl;
    while let Some(i) = rest.find('$') {
        let name: String = rest[i + 1..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !holes.contains(name.as_str()) {
            return Err(format!("`${name}` is not a placeholder of the pattern"));
        }
        rest = &rest[i + 1..];
    }
    Ok((out, tmpl.to_string()))
}

/// Tokens of a source range: (start, end, text).
fn tokens(text: &str, s: usize, e: usize) -> Vec<(usize, usize)> {
    let (toks, _) = crate::lexer::lex_all(&text[s..e], 0, s);
    toks.into_iter()
        .filter(|t| !matches!(t.tok, Tok::Newline | Tok::Eof))
        .map(|t| (t.span.start as usize, t.span.end as usize))
        .collect()
}

struct Matcher<'a> {
    text: &'a str,
    pat: &'a [PTok],
    toks: &'a [(usize, usize)],
    /// spans of the file's expressions: a placeholder binds exactly one
    exprs: &'a HashSet<(usize, usize)>,
}

impl Matcher<'_> {
    fn go(&self, pi: usize, ti: usize, binds: &mut HashMap<String, (usize, usize)>) -> bool {
        if pi == self.pat.len() {
            return ti == self.toks.len();
        }
        if ti == self.toks.len() {
            return false;
        }
        match &self.pat[pi] {
            PTok::Text(t) => {
                let (s, e) = self.toks[ti];
                &self.text[s..e] == t && self.go(pi + 1, ti + 1, binds)
            }
            PTok::Hole(name) => {
                for end in ti + 1..=self.toks.len() {
                    let span = (self.toks[ti].0, self.toks[end - 1].1);
                    if !self.exprs.contains(&span) {
                        continue;
                    }
                    if let Some(prev) = binds.get(name) {
                        if self.text[prev.0..prev.1] != self.text[span.0..span.1] {
                            continue;
                        }
                        if self.go(pi + 1, end, binds) {
                            return true;
                        }
                        continue;
                    }
                    binds.insert(name.clone(), span);
                    if self.go(pi + 1, end, binds) {
                        return true;
                    }
                    binds.remove(name);
                }
                false
            }
        }
    }
}

/// Whether an expression can be put anywhere without parentheses.
pub(super) fn atomic(e: &ast::Expr) -> bool {
    !matches!(
        e.kind,
        E::Binary(..)
            | E::Unary(..)
            | E::Else(..)
            | E::Catch(..)
            | E::Try(_)
            | E::Async(_)
            | E::Await(_)
    )
}

/// The replacements of a rule in one file: (start, end, new text).
fn ssr_file(text: &str, pat: &[PTok], tmpl: &str) -> Vec<(usize, usize, String)> {
    let (module, errors) = crate::parser::parse_module_recover(text, 0);
    if !errors.is_empty() {
        return vec![];
    }
    let mut exprs: Vec<&ast::Expr> = Vec::new();
    walk(&module, &mut |n| {
        if let Node::Expr(e) = n {
            exprs.push(e);
        }
    });
    let spans: HashSet<(usize, usize)> = exprs
        .iter()
        .map(|e| (e.span.start as usize, e.span.end as usize))
        .collect();
    let by_span: HashMap<(usize, usize), &ast::Expr> = exprs
        .iter()
        .map(|e| ((e.span.start as usize, e.span.end as usize), *e))
        .collect();
    let mut out: Vec<(usize, usize, String)> = Vec::new();
    // outer expressions come first; matches inside a match are left alone
    for e in &exprs {
        let (s, en) = (e.span.start as usize, e.span.end as usize);
        if out.iter().any(|(a, b, _)| *a <= s && en <= *b) {
            continue;
        }
        let toks = tokens(text, s, en);
        let m = Matcher {
            text,
            pat,
            toks: &toks,
            exprs: &spans,
        };
        let mut binds = HashMap::new();
        if !m.go(0, 0, &mut binds) {
            continue;
        }
        // fill the template, parenthesizing compound expressions inside a larger replacement
        let mut result = String::new();
        let mut rest = tmpl;
        while let Some(i) = rest.find('$') {
            result.push_str(&rest[..i]);
            let name: String = rest[i + 1..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let span = binds[&name];
            let sub = &text[span.0..span.1];
            // brackets, commas and `=` around the placeholder keep its expression whole
            let left = tmpl[..tmpl.len() - rest.len() + i]
                .trim_end()
                .chars()
                .last();
            let right = rest[i + 1 + name.len()..].trim_start().chars().next();
            let delimited = matches!(left, None | Some('(' | '[' | '{' | ',' | '='))
                && matches!(right, None | Some(')' | ']' | '}' | ','));
            if !delimited && by_span.get(&span).is_some_and(|x| !atomic(x)) {
                result.push_str(&format!("({sub})"));
            } else {
                result.push_str(sub);
            }
            rest = &rest[i + 1 + name.len()..];
        }
        result.push_str(rest);
        out.push((s, en, result));
    }
    out
}

impl Server {
    // `WorkspaceEdit::changes` is keyed by `Uri`, as defined by lsp-types
    #[allow(clippy::mutable_key_type)]
    pub(super) fn ssr(&self, p: SsrParams) -> Result<WorkspaceEdit, String> {
        let (pat, tmpl) = parse_rule(&p.query)?;
        let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
        if !p.parse_only {
            let mut files: Vec<PathBuf> = self
                .analyses
                .keys()
                .chain(self.docs.keys())
                .cloned()
                .collect();
            files.sort();
            files.dedup();
            for f in files {
                let Some(text) = self.text_of(&f) else {
                    continue;
                };
                let edits: Vec<TextEdit> = ssr_file(&text, &pat, &tmpl)
                    .into_iter()
                    .map(|(s, e, t)| TextEdit::new(range(&text, s, e), t))
                    .collect();
                if !edits.is_empty() {
                    changes.insert(path_to_uri(&f), edits);
                }
            }
        }
        Ok(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        })
    }

    /// Join each range's lines (or the line with the next one, for an empty range): the
    /// indentation goes, a space stays between words, none after an opening bracket or
    /// before a closing one, and a trailing comma before a closing bracket goes too. Comment
    /// lines lose the `//` of the joined line.
    pub(super) fn join_lines(&self, p: JoinLinesParams) -> Option<Vec<TextEdit>> {
        let path = uri_to_path(&p.text_document.uri)?;
        let text = self.text_of(&path)?;
        let mut edits = Vec::new();
        for r in p.ranges {
            let s = pos_to_offset(&text, r.start);
            let e = if r.end.line == r.start.line {
                // through the end of the next line break
                line_end(&text, s)
            } else {
                pos_to_offset(&text, r.end)
            };
            let mut i = s;
            while let Some(k) = text[i..e.min(text.len())].find('\n') {
                let nl = i + k;
                let before = text[..nl].trim_end_matches([' ', '\t']);
                let after_start = nl + 1 + text[nl + 1..].len()
                    - text[nl + 1..].trim_start_matches([' ', '\t']).len();
                let mut next = &text[after_start..];
                let mut del_end = after_start;
                let prev_line = &text[before.rfind('\n').map_or(0, |x| x + 1)..before.len()];
                if prev_line.trim_start().starts_with("//") && next.starts_with("//") {
                    let n = next.len() - next.trim_start_matches('/').len();
                    next = next[n..].trim_start();
                    del_end = text.len() - next.len();
                }
                let mut del_start = before.len();
                let first = next.chars().next();
                let closes = matches!(first, Some(')' | ']' | '}'));
                if closes && before.ends_with(',') {
                    del_start -= 1;
                }
                let last = text[..del_start].chars().last();
                let sep = if matches!(last, Some('(' | '['))
                    || closes
                    || first == Some('.')
                    || first.is_none()
                {
                    ""
                } else {
                    " "
                };
                edits.push(TextEdit::new(
                    Range::new(
                        offset_to_pos(&text, del_start),
                        offset_to_pos(&text, del_end),
                    ),
                    sep.to_string(),
                ));
                i = nl + 1;
            }
        }
        Some(edits)
    }

    /// Diagnostics of a document, for clients that pull them.
    pub(super) fn pull_diagnostics(
        &self,
        p: DocumentDiagnosticParams,
    ) -> DocumentDiagnosticReportResult {
        let items = uri_to_path(&p.text_document.uri)
            .map(|f| self.diagnostics_of(&f))
            .unwrap_or_default();
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(
            RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items,
                },
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str, rule: &str) -> String {
        let (pat, tmpl) = parse_rule(rule).unwrap();
        let mut out = src.to_string();
        let mut edits = ssr_file(src, &pat, &tmpl);
        edits.sort_by_key(|e| std::cmp::Reverse(e.0));
        for (s, e, t) in edits {
            out.replace_range(s..e, &t);
        }
        out
    }

    #[test]
    fn structural_replace() {
        let src = "int a = add(1, 2)\nint b = add(a * 2, add(3, 4))\n";
        assert_eq!(
            run(src, "add($x, $y) ==>> $x + $y"),
            "int a = 1 + 2\nint b = (a * 2) + add(3, 4)\n"
        );
        // placeholders bind whole expressions, respecting precedence
        assert_eq!(
            run("int c = x + y + z\n", "$a + $b ==>> f($a, $b)"),
            "int c = f(x + y, z)\n"
        );
        // a placeholder used twice matches the same text only
        assert_eq!(
            run("int d = g(x, x) + g(x, y)\n", "g($a, $a) ==>> h($a)"),
            "int d = h(x) + g(x, y)\n"
        );
        assert!(parse_rule("add($x) ==>> $y").is_err());
        assert!(parse_rule("no arrow").is_err());
    }
}
