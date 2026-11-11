//! Peephole clean-up of a generated function body.
//!
//! Code generation emits statements without tracking reachability: it writes `unreachable();`
//! after every function body and every exhaustive `if`, releases scope variables after a
//! `return`, and places a label wherever a jump *might* land. This pass walks the body once,
//! tracking whether the current line can be reached, and drops what cannot:
//!
//! - lines after an unconditional `return`, `goto`, `break`, `continue` or `unreachable()`,
//!   up to the next label that some reachable `goto` targets;
//! - labels that no reachable `goto` targets;
//! - `(void)x;` markers for variables that are used anyway.
//!
//! It relies on the shape of generated code: one statement per line, `{` at the end of a line
//! opening a block, `}` at the start of a line closing one, and only forward `goto`s.

use std::collections::HashSet;

/// A label definition line (`name:;`), returning the label name.
fn label(t: &str) -> Option<&str> {
    let name = t.strip_suffix(":;")?;
    is_ident(name).then_some(name)
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric())
        && !s.as_bytes()[0].is_ascii_digit()
}

/// The target of every `goto` in the line.
fn gotos(t: &str) -> impl Iterator<Item = &str> {
    t.match_indices("goto ").filter_map(|(i, _)| {
        let rest = &t[i + 5..];
        let end = rest.find(';')?;
        let name = &rest[..end];
        is_ident(name).then_some(name)
    })
}

/// Whether control never continues past this (complete) statement line.
fn terminates(t: &str) -> bool {
    t == "break;"
        || t == "continue;"
        || t == "unreachable();"
        || t == "return;"
        || t.starts_with("return ")
        || (t.starts_with("goto ") && t.ends_with(';'))
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

struct Open {
    /// The opener was a bare `{`, so the block is entered only from the top and left only by
    /// falling off its end: reachability after it is reachability at its end.
    plain: bool,
    /// The opener itself was unreachable.
    dead: bool,
}

/// `noreturn` names the functions declared `[[noreturn]]`.
pub fn tidy(body: &str, noreturn: &HashSet<String>) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let all = decls_unique(&lines);
    let mut lines = flatten_blocks(&lines, all);
    // removing a dead line can bring a `goto` next to its label, which frees more: repeat
    // until nothing changes
    loop {
        lines = merge_labels(lines);
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let removed = strip_unreachable(&refs);
        if !removed.contains(&true) {
            break;
        }
        lines = (lines.into_iter().zip(removed))
            .filter_map(|(l, r)| (!r).then_some(l))
            .collect();
    }
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let lines = drop_unused_temps(&strip_empty_blocks(strip_void_markers(&refs)));
    let lines = strip_noreturn_hints(lines, noreturn);
    let mut out = String::with_capacity(body.len());
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// Drop `unreachable();` right after a call to a `[[noreturn]]` function, where the C
/// compiler already knows control stops. Until now it served the reachability analysis above.
fn strip_noreturn_hints(lines: Vec<String>, noreturn: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for l in lines {
        if l.trim() == "unreachable();"
            && let Some(prev) = out.last()
            && let Some((name, _)) = prev.trim().split_once('(')
            && noreturn.contains(name)
            && prev.trim().ends_with(");")
        {
            continue;
        }
        out.push(l);
    }
    out
}

/// Point every jump to a run of adjacent labels at the last of them, so the others go unused.
fn merge_labels(mut lines: Vec<String>) -> Vec<String> {
    let mut alias: std::collections::HashMap<String, String> = Default::default();
    let mut k = 0;
    while k < lines.len() {
        let run = lines[k..]
            .iter()
            .take_while(|l| label(l.trim()).is_some())
            .count();
        if run > 1 {
            let last = label(lines[k + run - 1].trim()).unwrap().to_string();
            for l in &lines[k..k + run - 1] {
                alias.insert(label(l.trim()).unwrap().to_string(), last.clone());
            }
        }
        k += run.max(1);
    }
    if alias.is_empty() {
        return lines;
    }
    for l in &mut lines {
        if label(l.trim()).is_some() || !l.contains("goto ") {
            continue;
        }
        let targets: Vec<String> = gotos(l).map(String::from).collect();
        for t in targets {
            if let Some(to) = alias.get(&t) {
                *l = l.replace(&format!("goto {t};"), &format!("goto {to};"));
            }
        }
    }
    lines
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

/// Mark unreachable lines and unused labels for removal.
fn strip_unreachable(lines: &[&str]) -> Vec<bool> {
    let mut removed = vec![false; lines.len()];
    let mut targets: HashSet<&str> = HashSet::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut dead = false;
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if let Some(name) = label(t) {
            if targets.contains(name) {
                dead = false;
            } else {
                removed[i] = true;
            }
            i += 1;
            continue;
        }
        let closes = t.starts_with('}');
        let opens = t.ends_with('{');
        if closes {
            let Some(o) = stack.pop() else {
                // unbalanced: leave the rest alone
                return removed;
            };
            if !o.plain {
                dead = o.dead;
            }
            if opens {
                // `} else {`: the else branch is reachable when the `if` was
                stack.push(Open {
                    plain: false,
                    dead: o.dead,
                });
            }
            i += 1;
            continue;
        }
        if dead {
            if opens {
                // skip the whole block unless a reachable jump lands inside it
                if let Some(end) = block_end(lines, i)
                    && !(i..=end)
                        .any(|k| label(lines[k].trim()).is_some_and(|n| targets.contains(n)))
                {
                    removed[i..=end].fill(true);
                    i = end + 1;
                    continue;
                }
                stack.push(Open {
                    plain: t == "{",
                    dead: true,
                });
            } else {
                removed[i] = true;
            }
            i += 1;
            continue;
        }
        // a jump to the very next line (past nothing but labels)
        if let Some(name) = t.strip_prefix("goto ").and_then(|r| r.strip_suffix(';'))
            && (lines[i + 1..].iter().map_while(|l| label(l.trim()))).any(|l| l == name)
        {
            removed[i] = true;
            i += 1;
            continue;
        }
        targets.extend(gotos(t));
        if opens {
            stack.push(Open {
                plain: t == "{",
                dead: false,
            });
        } else if terminates(t) {
            dead = true;
        }
        i += 1;
    }
    removed
}

/// The index of the line closing the block opened at `start` (following `} else {` chains).
fn block_end(lines: &[&str], start: usize) -> Option<usize> {
    let ind = indent(lines[start]);
    for (k, l) in lines.iter().enumerate().skip(start + 1) {
        let t = l.trim();
        if indent(l) < ind && !t.is_empty() {
            return None;
        }
        if indent(l) == ind && t.starts_with('}') && !t.ends_with('{') {
            return Some(k);
        }
    }
    None
}

/// Drop `(void)x;` lines for variables that are mentioned elsewhere besides their declaration.
fn strip_void_markers<'a>(lines: &[&'a str]) -> Vec<&'a str> {
    let marker = |t: &str| -> Option<String> {
        let name = t.strip_prefix("(void)")?.strip_suffix(';')?;
        is_ident(name).then(|| name.to_string())
    };
    let names: HashSet<String> = lines.iter().filter_map(|l| marker(l.trim())).collect();
    if names.is_empty() {
        return lines.to_vec();
    }
    let mut uses: std::collections::HashMap<&str, usize> = Default::default();
    for l in lines {
        if marker(l.trim()).is_some() {
            continue;
        }
        for w in l.split(|c: char| !(c == '_' || c.is_ascii_alphanumeric())) {
            if names.contains(w) {
                *uses.entry(w).or_default() += 1;
            }
        }
    }
    // one mention is the declaration; more means the variable is used
    lines
        .iter()
        .filter(|l| {
            marker(l.trim()).is_none_or(|n| uses.get(n.as_str()).copied().unwrap_or(0) <= 1)
        })
        .copied()
        .collect()
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
    let mut uses: std::collections::HashMap<&str, usize> = Default::default();
    for l in lines {
        for w in l.split(|c: char| !(c == '_' || c.is_ascii_alphanumeric())) {
            if w.starts_with('t') {
                *uses.entry(w).or_default() += 1;
            }
        }
    }
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
    use super::tidy as tidy_with;
    use std::collections::HashSet;

    fn tidy(body: &str) -> String {
        tidy_with(body, &HashSet::new())
    }

    #[test]
    fn merges_labels() {
        let body = "  if (c) goto arm1;\n  x = 1;\n  goto end_1;\n  arm1:;\n  goto end_1;\n  end_1:;\n  return;\n";
        assert_eq!(
            tidy(body),
            "  if (c) goto end_1;\n  x = 1;\n  end_1:;\n  return;\n"
        );
    }

    #[test]
    fn strips_dead_code_and_unused_labels() {
        let body = "  {\n    if (c) goto arm1;\n    {\n      x = 1;\n    }\n    goto end_1;\n  }\n  arm1:;\n  {\n    {\n    }\n    goto end_1;\n  }\n  arm3:;\n  unreachable();\n  end_1:;\n  return x;\n  rl(y);\n  unreachable();\n";
        assert_eq!(
            tidy(body),
            "  if (c) goto end_1;\n  x = 1;\n  end_1:;\n  return x;\n"
        );
    }

    #[test]
    fn keeps_reachable_unreachable() {
        // after a call that never returns, the hint stays
        let body = "  if (c) goto arm1;\n  f();\n  unreachable();\n  arm1:;\n  return;\n";
        assert_eq!(tidy(body), body);
    }

    #[test]
    fn drops_hint_after_noreturn_call() {
        let body = "  if (c) goto arm1;\n  f0_die();\n  unreachable();\n  arm1:;\n  return 1;\n  unreachable();\n";
        let noreturn = HashSet::from(["f0_die".to_string()]);
        assert_eq!(
            tidy_with(body, &noreturn),
            "  if (c) goto arm1;\n  f0_die();\n  arm1:;\n  return 1;\n"
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
