//! A function body as it is generated, keeping out code that can never run.
//!
//! Code generation writes statements without deciding reachability itself: it writes
//! `unreachable();` after every function body and every exhaustive `if`, releases scope
//! variables after a `return`, and places a label wherever a jump *might* land. `Body` tracks,
//! line by line, whether the current line can be reached, and leaves out:
//!
//! - lines after an unconditional `return`, `goto`, `break`, `continue` or `unreachable()`,
//!   up to the next label that some kept `goto` targets;
//! - labels that no kept `goto` targets, and a `goto` to the label right after it;
//! - `unreachable();` right after a call to a `[[noreturn]]` function, where the C compiler
//!   already knows control stops.
//!
//! Adjacent labels are merged into the last of them. This relies on the shape of generated
//! code: one statement per line, `{` at the end of a line opening a block, `}` at the start of
//! a line closing one, and only forward `goto`s.

use std::collections::{HashMap, HashSet};

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

/// A block being generated.
struct Open {
    /// The opener was a bare `{`, so the block is entered only from the top and left only by
    /// falling off its end: reachability after it is reachability at its end.
    plain: bool,
    /// The opener itself was unreachable.
    dead: bool,
    /// The lines opening the block, held back while it is unreachable (an `if` and its
    /// `} else {`), until a kept jump lands inside it. Empty once written.
    held: Vec<String>,
}

#[derive(Default)]
pub struct Body {
    /// the lines written so far; `None` for lines taken back
    lines: Vec<Option<String>>,
    dead: bool,
    open: Vec<Open>,
    /// for each label some kept jump targets, the lines of those jumps
    jumps: HashMap<String, Vec<usize>>,
    /// lines opening and closing bare `{` blocks, which run nothing
    braces: HashSet<usize>,
    /// a statement was given (kept or not) since the last label
    gap: bool,
    /// functions declared `[[noreturn]]`, by C name
    noreturn: HashSet<String>,
}

impl Body {
    pub fn new(noreturn: HashSet<String>) -> Self {
        Self {
            noreturn,
            ..Default::default()
        }
    }

    /// Start a new function body.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.dead = false;
        self.open.clear();
        self.jumps.clear();
        self.braces.clear();
        self.gap = false;
    }

    /// The number of lines written, for checking that nothing was.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// The body's text.
    pub fn take(&mut self) -> String {
        let mut out = String::new();
        for l in self.lines.drain(..).flatten() {
            out.push_str(&l);
            out.push('\n');
        }
        self.clear();
        out
    }

    /// The last line kept, and its index.
    fn last(&self) -> Option<(usize, &str)> {
        let i = self.lines.iter().rposition(Option::is_some)?;
        Some((i, self.lines[i].as_deref()?.trim()))
    }

    /// The last line kept that runs or labels something (not a brace of a bare block).
    fn last_solid(&self) -> Option<(usize, &str)> {
        let i = (self.lines.iter().enumerate())
            .rposition(|(i, l)| l.is_some() && !self.braces.contains(&i))?;
        Some((i, self.lines[i].as_deref()?.trim()))
    }

    fn push(&mut self, line: String) {
        if line.trim() == "{" {
            self.braces.insert(self.lines.len());
        }
        self.lines.push(Some(line));
    }

    /// Write the openers of the unreachable blocks around the current line.
    fn release_held(&mut self) {
        let held: Vec<String> = (self.open.iter_mut())
            .flat_map(|o| std::mem::take(&mut o.held))
            .collect();
        for l in held {
            self.push(l);
        }
    }

    /// Add a line (with its indentation).
    pub fn line(&mut self, line: String) {
        let t = line.trim();
        if let Some(name) = label(t) {
            self.label(name.to_string(), line);
            return;
        }
        self.gap |= t != "{" && t != "}";
        let closes = t.starts_with('}');
        let opens = t.ends_with('{');
        if closes {
            let Some(o) = self.open.pop() else {
                // unbalanced: keep it
                return self.push(line);
            };
            let written = o.held.is_empty();
            if !o.plain {
                // as before the block: an `if` may be skipped, a loop is left by its jumps
                self.dead = o.dead;
            }
            if opens {
                // `} else {`: the else branch is reachable when the `if` was
                let mut held = o.held;
                if !written {
                    held.push(line.clone());
                }
                self.open.push(Open {
                    plain: false,
                    dead: o.dead,
                    held,
                });
            }
            if written {
                if o.plain {
                    self.braces.insert(self.lines.len());
                }
                self.push(line);
            }
            return;
        }
        if self.dead {
            if opens {
                self.open.push(Open {
                    plain: t == "{",
                    dead: true,
                    held: vec![line],
                });
            }
            return;
        }
        if opens {
            self.open.push(Open {
                plain: t == "{",
                dead: false,
                held: vec![],
            });
            return self.push(line);
        }
        if t == "unreachable();"
            && let Some((_, prev)) = self.last()
            && let Some((name, _)) = prev.split_once('(')
            && prev.ends_with(");")
            && self.noreturn.contains(name)
        {
            self.dead = true;
            return;
        }
        let at = self.lines.len();
        for target in gotos(t) {
            self.jumps.entry(target.to_string()).or_default().push(at);
        }
        self.dead |= terminates(t);
        self.push(line);
    }

    fn label(&mut self, name: String, line: String) {
        loop {
            // a label after another one: jumps to that one come here, unless statements
            // separated them and nothing jumps here (then this label is the one to go)
            if let Some((i, prev)) = self.last_solid()
                && let Some(prev) = label(prev).map(String::from)
                && !(self.gap && !self.jumps.contains_key(&name))
            {
                self.lines[i] = None;
                for at in self.jumps.remove(&prev).unwrap_or_default() {
                    let l = self.lines[at].as_mut().unwrap();
                    *l = l.replace(&format!("goto {prev};"), &format!("goto {name};"));
                    self.jumps.entry(name.clone()).or_default().push(at);
                }
                continue;
            }
            // a jump to right here
            let Some((i, prev)) = self.last_solid() else {
                break;
            };
            if prev != format!("goto {name};") {
                break;
            }
            self.lines[i] = None;
            self.dead = false;
            if let Some(js) = self.jumps.get_mut(&name) {
                js.retain(|&at| at != i);
                if js.is_empty() {
                    self.jumps.remove(&name);
                }
            }
        }
        if self.jumps.contains_key(&name) {
            self.release_held();
            self.dead = false;
            self.gap = false;
            self.push(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Body;

    fn run(body: &str, noreturn: &[&str]) -> String {
        let mut b = Body::new(noreturn.iter().map(|s| s.to_string()).collect());
        for l in body.lines() {
            b.line(l.to_string());
        }
        b.take()
    }

    #[test]
    fn merges_labels() {
        let body = "  if (c) goto arm1;\n  x = 1;\n  goto end_1;\n  arm1:;\n  goto end_1;\n  end_1:;\n  return;\n";
        assert_eq!(
            run(body, &[]),
            "  if (c) goto end_1;\n  x = 1;\n  end_1:;\n  return;\n"
        );
    }

    #[test]
    fn strips_dead_code_and_unused_labels() {
        let body = "  {\n    if (c) goto arm1;\n    {\n      x = 1;\n    }\n    goto end_1;\n  }\n  arm1:;\n  {\n    {\n    }\n    goto end_1;\n  }\n  arm3:;\n  unreachable();\n  end_1:;\n  return x;\n  rl(y);\n  unreachable();\n";
        assert_eq!(
            run(body, &[]),
            "  {\n    if (c) goto end_1;\n    {\n      x = 1;\n    }\n  }\n  {\n    {\n    }\n  }\n  end_1:;\n  return x;\n"
        );
    }

    #[test]
    fn keeps_reachable_unreachable() {
        // after a call that never returns, the hint stays
        let body = "  if (c) goto arm1;\n  f();\n  unreachable();\n  arm1:;\n  return;\n";
        assert_eq!(run(body, &[]), body);
    }

    #[test]
    fn drops_hint_after_noreturn_call() {
        let body = "  if (c) goto arm1;\n  f0_die();\n  unreachable();\n  arm1:;\n  return 1;\n  unreachable();\n";
        assert_eq!(
            run(body, &["f0_die"]),
            "  if (c) goto arm1;\n  f0_die();\n  arm1:;\n  return 1;\n"
        );
    }

    #[test]
    fn keeps_unreachable_blocks_a_jump_lands_in() {
        let body = "  if (c) goto inner;\n  return;\n  if (d) {\n    x = 1;\n  } else {\n    inner:;\n    y = 2;\n  }\n  return;\n";
        assert_eq!(
            run(body, &[]),
            "  if (c) goto inner;\n  return;\n  if (d) {\n  } else {\n    inner:;\n    y = 2;\n  }\n"
        );
    }

    /// A random function body in the shape code generation writes: one statement per line,
    /// bare and `if`/`else`/loop blocks, and forward jumps to labels of the same or an
    /// enclosing block.
    fn random_body(rand: &mut dyn FnMut(u64) -> u64) -> String {
        fn block(
            rand: &mut dyn FnMut(u64) -> u64,
            depth: usize,
            next: &mut usize,
            ahead: &mut Vec<String>,
            out: &mut Vec<String>,
        ) {
            let ind = "  ".repeat(depth + 1);
            // the labels this block will place, which jumps before them may target
            let own: Vec<String> = (0..rand(3))
                .map(|_| {
                    *next += 1;
                    format!("l{next}")
                })
                .collect();
            let base = ahead.len();
            ahead.extend(own.iter().cloned());
            let mut placed = 0;
            for _ in 0..rand(7) {
                match rand(14) {
                    0..=2 => out.push(format!("{ind}x = {};", rand(9))),
                    3 | 4 if !ahead.is_empty() => {
                        let l = &ahead[rand(ahead.len() as u64) as usize];
                        out.push(format!("{ind}if (c) goto {l};"));
                    }
                    5 if !ahead.is_empty() => {
                        let l = &ahead[rand(ahead.len() as u64) as usize];
                        out.push(format!("{ind}goto {l};"));
                    }
                    6 => out.push(format!("{ind}return;")),
                    7 => out.push(format!("{ind}unreachable();")),
                    8 => {
                        out.push(format!("{ind}f0_die();"));
                        out.push(format!("{ind}unreachable();"));
                    }
                    9 if depth < 3 => {
                        out.push(format!("{ind}{{"));
                        block(rand, depth + 1, next, ahead, out);
                        out.push(format!("{ind}}}"));
                    }
                    10 if depth < 3 => {
                        out.push(format!("{ind}if (d) {{"));
                        block(rand, depth + 1, next, ahead, out);
                        if rand(2) == 0 {
                            out.push(format!("{ind}}} else {{"));
                            block(rand, depth + 1, next, ahead, out);
                        }
                        out.push(format!("{ind}}}"));
                    }
                    11 if depth < 3 => {
                        out.push(format!("{ind}for (;;) {{"));
                        block(rand, depth + 1, next, ahead, out);
                        out.push(format!("{ind}}}"));
                    }
                    12 | 13 if placed < own.len() => {
                        ahead.retain(|l| *l != own[placed]);
                        out.push(format!("{ind}{}:;", own[placed]));
                        placed += 1;
                    }
                    _ => {}
                }
            }
            for l in &own[placed..] {
                ahead.retain(|x| x != l);
                out.push(format!("{ind}{l}:;"));
            }
            debug_assert_eq!(ahead.len(), base);
        }
        let mut out = Vec::new();
        block(rand, 0, &mut 0, &mut Vec::new(), &mut out);
        out.join("\n") + "\n"
    }

    /// Leaving unreachable code out while generating, then tidying, gives what the old text
    /// pass over the whole body gave.
    #[test]
    fn matches_the_text_pass() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut rand = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let noreturn: std::collections::HashSet<String> = ["f0_die".to_string()].into();
        for _ in 0..20_000 {
            let body = random_body(&mut rand);
            let old = super::super::tidy_old::tidy(&body, &noreturn);
            let new = super::super::tidy::tidy(&run(&body, &["f0_die"]));
            assert_eq!(new, old, "body:\n{body}");
        }
    }
}
