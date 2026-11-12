//! `ncc fmt`: prints a parsed module back in a canonical style.
//!
//! - 2-space indentation, one statement per line, single spaces around binary operators
//! - parentheses only where precedence requires them
//! - at most one blank line between statements, as in the original
//! - comments are kept: own-line comments before the statement that follows them,
//!   trailing comments at the end of their line
//! - literals are printed exactly as written (`0xFF`, `1_000`, `"""` strings, escapes)
//! - array, map and struct literals that spanned several lines stay one entry per line;
//!   one-line `{ ... }` bodies of `if` arms and function literals stay on one line
//! - lines stay within 100 columns where possible: the outermost bracketed list on a longer
//!   line (call arguments, array, map and struct literals, parameters) is put one entry per line

use std::collections::HashSet;

use crate::ast::*;
use crate::diag::{Diag, Span};

/// Lines longer than this are wrapped where a bracketed list allows it.
pub const MAX_WIDTH: usize = 100;

/// Markers around the id of a bracketed list printed on one line (private-use characters,
/// removed from the final output): `\u{E000}id\u{E001}`.
const MARK_START: char = '\u{E000}';
const MARK_END: char = '\u{E001}';
/// Ids of parameter lists are offset so they never collide with expression ids.
const SIG_ID: usize = 1 << 40;

/// Format NC source. Fails with the syntax errors if the source does not parse.
pub fn format(src: &str) -> Result<String, Vec<Diag>> {
    let (module, errors) = crate::parser::parse_module_recover(src, 0);
    if !errors.is_empty() {
        return Err(errors);
    }
    let comments = crate::lexer::comments(src);
    // print, then break the outermost one-line list of each line that is too long, until
    // every line fits or nothing more can be broken
    let mut force: HashSet<usize> = HashSet::new();
    let mut out = String::new();
    for _ in 0..200 {
        let mut p = Printer {
            src,
            comments: &comments,
            next: 0,
            out: String::new(),
            cursor: 0,
            force: &force,
        };
        p.module(&module);
        out = p.out;
        let mut added = false;
        for line in out.lines() {
            if strip_marks(line).chars().count() <= MAX_WIDTH {
                continue;
            }
            if let Some(id) = marks(line).into_iter().find(|id| !force.contains(id)) {
                force.insert(id);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    Ok(strip_marks(&out))
}

/// A single argument that is itself bracketed (`f([...])`, `f(T{...})`, `f(fn() {...})`):
/// when the line is too long, that argument breaks instead of the call.
fn hugs(args: &[Expr]) -> bool {
    matches!(
        args,
        [Expr {
            kind: ExprKind::Array(_)
                | ExprKind::Map(_)
                | ExprKind::Struct(..)
                | ExprKind::FnLit(..),
            ..
        }]
    )
}

/// Ids of the one-line lists in a line, outermost (leftmost) first.
fn marks(line: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(i) = rest.find(MARK_START) {
        let after = &rest[i + MARK_START.len_utf8()..];
        let Some(j) = after.find(MARK_END) else { break };
        if let Ok(id) = after[..j].parse() {
            out.push(id);
        }
        rest = &after[j + MARK_END.len_utf8()..];
    }
    out
}

fn strip_marks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_mark = false;
    for c in s.chars() {
        match c {
            MARK_START => in_mark = true,
            MARK_END => in_mark = false,
            _ if !in_mark => out.push(c),
            _ => {}
        }
    }
    out
}

struct Printer<'a> {
    src: &'a str,
    comments: &'a [(usize, usize)],
    /// next comment to print
    next: usize,
    out: String,
    /// end of the last printed source construct (for blank line detection)
    cursor: usize,
    /// ids of bracketed lists to print one entry per line because a line was too long
    force: &'a HashSet<usize>,
}

const INDENT: &str = "  ";

/// A list entry: its source range and a printer for it at a given indentation.
type EntryFn<'e, 'a> = Box<dyn Fn(&mut Printer<'a>, usize) -> String + 'e>;
struct Entry<'e, 'a> {
    start: usize,
    end: usize,
    show: EntryFn<'e, 'a>,
}

fn entry<'e, 'a>(span: Span, f: impl Fn(&mut Printer<'a>, usize) -> String + 'e) -> Entry<'e, 'a> {
    Entry {
        start: span.start as usize,
        end: span.end as usize,
        show: Box::new(f),
    }
}

/// One entry per expression.
fn expr_entries<'a>(xs: &[Expr]) -> Vec<Entry<'_, 'a>> {
    xs.iter()
        .map(|x| entry(x.span, move |p, i| p.expr(x, i)))
        .collect()
}

fn vis(public: bool) -> &'static str {
    if public { "pub " } else { "" }
}

fn ind(n: usize) -> String {
    INDENT.repeat(n)
}

/// Escape a plain string for a `"..."` literal (test names, import paths, extern symbols).
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{1b}' => out.push_str("\\e"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:X}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl<'a> Printer<'a> {
    fn text(&self, s: Span) -> &'a str {
        &self.src[s.start as usize..(s.end as usize).min(self.src.len())]
    }

    fn multiline(&self, s: Span) -> bool {
        self.text(s).contains('\n')
    }

    fn has_comment_in(&self, start: usize, end: usize) -> bool {
        self.comments.iter().any(|&(s, _)| s >= start && s < end)
    }

    /// The first `c` at or after `from` that is not inside a comment.
    fn find_outside_comments(&self, from: usize, c: char) -> usize {
        let mut at = from;
        while let Some(k) = self.src[at..].find(c) {
            let pos = at + k;
            match self.comments.iter().find(|&&(s, e)| s <= pos && pos < e) {
                Some(&(_, e)) => at = e,
                None => return pos,
            }
        }
        self.src.len()
    }

    /// Whether the bracketed list from `open` to `end` holds comments of its own: comments
    /// between its entries (given by their ranges) rather than inside one of them.
    fn list_has_comments(&self, open: usize, end: usize, entries: &[(usize, usize)]) -> bool {
        self.comments
            .iter()
            .any(|&(s, _)| s > open && s < end && !entries.iter().any(|&(a, b)| s >= a && s < b))
    }

    /// Is there an empty line in the source between two offsets?
    fn blank_between(&self, a: usize, b: usize) -> bool {
        if a >= b || b > self.src.len() {
            return false;
        }
        let gap = &self.src[a..b];
        gap.matches('\n').count() >= 2 && gap.split('\n').skip(1).any(|l| l.trim().is_empty())
    }

    /// Print comments that start before `before`, each on its own line.
    fn flush_comments(&mut self, before: usize, indent: usize, at_block_start: bool) {
        let mut first = at_block_start;
        while self.next < self.comments.len() && self.comments[self.next].0 < before {
            let (s, e) = self.comments[self.next];
            if !first && self.blank_between(self.cursor, s) {
                self.out.push('\n');
            }
            first = false;
            self.out.push_str(&ind(indent));
            self.out.push_str(self.src[s..e].trim_end());
            self.out.push('\n');
            self.cursor = e;
            self.next += 1;
        }
    }

    /// Append a comment that follows `end` on the same source line to the last printed line,
    /// unless code other than a separator comes between them (`fn f() { a } // c`).
    fn trailing_comment(&mut self, end: usize) {
        if self.next < self.comments.len() {
            let (s, e) = self.comments[self.next];
            let gap = |c: char| c == ',' || c == ';' || (c.is_whitespace() && c != '\n');
            if s >= end && self.src[end..s].chars().all(gap) {
                if self.out.ends_with('\n') {
                    self.out.pop();
                }
                self.out.push(' ');
                self.out.push_str(self.src[s..e].trim_end());
                self.out.push('\n');
                self.next += 1;
                self.cursor = e;
            }
        }
    }

    /// Before an entry of a block or module at `start`: the comments before it, and a blank
    /// line kept from the source.
    fn entry_start(&mut self, start: usize, indent: usize, first: bool) {
        self.flush_comments(start, indent, first && self.cursor_is_block_start());
        // keep a blank line from the source, except right after an opening brace
        if self.blank_between(self.cursor, start) && !self.cursor_is_block_start() {
            self.out.push('\n');
        }
    }

    /// The end of the source line containing `off`.
    fn line_end(&self, off: usize) -> usize {
        self.src[off..]
            .find('\n')
            .map_or(self.src.len(), |k| off + k)
    }

    fn cursor_is_block_start(&self) -> bool {
        self.out.is_empty() || self.out.ends_with("{\n")
    }

    // ------------------------------------------------------------------ items

    fn module(&mut self, m: &Module) {
        let mut i = 0;
        let mut first = true;
        while i < m.items.len() {
            let (start, end) = m.items[i].range();
            let (start, end) = (start as usize, end as usize);
            self.entry_start(start, 0, first);
            first = false;
            if let Item::Import(_) = &m.items[i] {
                // consecutive imports form one block
                let mut j = i;
                let mut group = Vec::new();
                while let Some(Item::Import(im)) = m.items.get(j) {
                    group.push(im);
                    j += 1;
                }
                if group.len() == 1 {
                    let im = group[0];
                    self.out.push_str(&format!(
                        "import {{ {} as {} }}\n",
                        quote(&im.path),
                        im.alias
                    ));
                } else {
                    self.out.push_str("import {\n");
                    for im in &group {
                        self.flush_comments(im.span.start as usize, 1, false);
                        self.out.push_str(&format!(
                            "{INDENT}{} as {}\n",
                            quote(&im.path),
                            im.alias
                        ));
                        self.cursor = im.span.end as usize;
                        self.trailing_comment(im.span.end as usize);
                    }
                    self.out.push_str("}\n");
                }
                let last_end = group.last().unwrap().span.end as usize;
                // skip the closing brace of the import block
                let close = self.src[last_end..]
                    .find('}')
                    .map_or(last_end, |k| last_end + k + 1);
                self.cursor = close;
                self.trailing_comment(close);
                i = j;
                continue;
            }
            self.item(&m.items[i]);
            self.cursor = end;
            self.trailing_comment(end);
            i += 1;
        }
        self.flush_comments(usize::MAX, 0, false);
    }

    fn item(&mut self, it: &Item) {
        match it {
            Item::Import(_) => unreachable!(),
            Item::Extern(e) => {
                let head = format!("extern {} as {}", quote(&e.path), e.alias);
                let fns = e.fns.iter().map(|f| (f.sig.span, f));
                self.members(head, fns, e.span, |p, f| {
                    let sig = p.sig(&f.sig, true, 1);
                    format!("{sig} = {}", quote(&f.symbol))
                });
            }
            Item::Struct(s) => {
                let params = GenericParam::list(&s.type_params);
                let head = format!("{}struct {}{params}", vis(s.public), s.name);
                let fields = s.fields.iter().map(|f| (f.span, f));
                self.members(head, fields, s.span, |_, f| f.show());
            }
            Item::Enum(e) => {
                let params = GenericParam::list(&e.type_params);
                let head = format!("{}enum {}{params}", vis(e.public), e.name);
                let variants = e.variants.iter().map(|v| (v.span, v));
                self.members(head, variants, e.span, |_, v| match &v.payload[..] {
                    [] => v.name.clone(),
                    ts => format!("{}({})", v.name, TypeExpr::show_list(ts)),
                });
            }
            Item::Type(t) => {
                let ty = t.ty.show();
                self.out
                    .push_str(&format!("{}type {} = {ty}\n", vis(t.public), t.name));
            }
            Item::Fn(f) => {
                let text = self.fn_decl(f, 0);
                self.out.push_str(&text);
                self.out.push('\n');
            }
            Item::Test(t) => {
                let b = self.block(&t.body, 0, false);
                self.out.push_str(&format!("test {} {b}\n", quote(&t.name)));
            }
            Item::Stmt(s) => {
                let text = self.stmt(s, 0);
                self.out.push_str(&text);
                self.out.push('\n');
            }
        }
    }

    /// `head { ... }` (an extern block, struct or enum ending at `span`), one member per line,
    /// or `head {}` with neither members nor comments;
    /// `show` prints a member after the comments before it. Blank lines between members are
    /// kept as between statements; comments before the first member never get one above them.
    fn members<'m, T: 'm>(
        &mut self,
        head: String,
        members: impl Iterator<Item = (Span, &'m T)>,
        span: Span,
        show: impl Fn(&mut Self, &T) -> String,
    ) {
        self.out.push_str(&head);
        let open = self.find_outside_comments(span.start as usize, '{');
        let mut members = members.peekable();
        if members.peek().is_none() && !self.has_comment_in(open, span.end as usize) {
            self.out.push_str(" {}\n");
            return;
        }
        self.out.push_str(" {\n");
        self.cursor = open + 1;
        for (i, (sp, m)) in members.enumerate() {
            self.entry_start(sp.start as usize, 1, i == 0);
            let text = show(self, m);
            self.out.push_str(&format!("{INDENT}{text}\n"));
            self.cursor = self.line_end(sp.end as usize);
            self.trailing_comment(sp.end as usize);
        }
        self.flush_comments(span.end as usize, 1, false);
        self.out.push_str("}\n");
    }

    fn fn_decl(&mut self, f: &FnDecl, indent: usize) -> String {
        let sig = self.sig(&f.sig, true, indent);
        let body = self.block(&f.body, indent, false);
        format!("{}{sig} {body}", vis(f.public))
    }

    /// `fn name<type T>(T a) R` (or `fn(T a) R` for literals)
    fn sig(&mut self, s: &FnSig, named: bool, indent: usize) -> String {
        let entries = s
            .params
            .iter()
            .map(|p| entry(p.span, move |_, _| p.show()))
            .collect();
        // the parameter list runs from the first `(` to the `)` after the last parameter
        let open = self.find_outside_comments(s.span.start as usize, '(');
        let last = s.params.last().map_or(open + 1, |p| p.span.end as usize);
        let end = self.find_outside_comments(last, ')');
        let id = SIG_ID + s.span.start as usize;
        let params = self.bracketed("()", entries, false, indent, open, end, Some(id));
        let mut out = if named {
            format!(
                "fn {}{}{params}",
                s.name,
                GenericParam::list(&s.type_params)
            )
        } else {
            format!("fn{params}")
        };
        if let Some(r) = &s.ret {
            out.push(' ');
            out.push_str(&r.show());
        }
        out
    }

    // ------------------------------------------------------------------ blocks and statements

    /// A `{ ... }` block. With `inline_ok`, a block that was on one line and holds at most one
    /// simple statement stays on one line.
    fn block(&mut self, b: &Block, indent: usize, inline_ok: bool) -> String {
        let (bs, be) = (b.span.start as usize, b.span.end as usize);
        let comments_inside = self.has_comment_in(bs, be);
        if b.stmts.is_empty() && !comments_inside {
            self.cursor = be;
            return "{}".into();
        }
        if inline_ok && b.stmts.len() == 1 && !self.multiline(b.span) && !comments_inside {
            let s = self.stmt(&b.stmts[0], indent + 1);
            if !s.contains('\n') {
                self.cursor = be;
                return format!("{{ {s} }}");
            }
        }
        let saved = std::mem::take(&mut self.out);
        self.out.push_str("{\n");
        self.cursor = bs + 1;
        for (i, s) in b.stmts.iter().enumerate() {
            self.entry_start(s.span.start as usize, indent + 1, i == 0);
            let text = self.stmt(s, indent + 1);
            self.out.push_str(&ind(indent + 1));
            self.out.push_str(&text);
            self.out.push('\n');
            self.cursor = s.span.end as usize;
            self.trailing_comment(s.span.end as usize);
        }
        self.flush_comments(be, indent + 1, b.stmts.is_empty());
        self.out.push_str(&ind(indent));
        self.out.push('}');
        self.cursor = be;
        std::mem::replace(&mut self.out, saved)
    }

    fn label(l: &Option<String>) -> String {
        l.as_ref().map(|l| format!("{l}: ")).unwrap_or_default()
    }

    fn stmt(&mut self, s: &Stmt, indent: usize) -> String {
        match &s.kind {
            StmtKind::VarDecl {
                public,
                mutable,
                mutex,
                targets,
                value,
            } => {
                let kw = if *mutable {
                    "mut "
                } else if *mutex {
                    "mutex "
                } else {
                    ""
                };
                let ts: Vec<String> = (targets.iter())
                    .map(|t| format!("{} {}", t.ty.show(), t.name))
                    .collect();
                let value = self.expr(value, indent);
                format!("{}{kw}{} = {value}", vis(*public), ts.join(", "))
            }
            StmtKind::FnVar { name, value, .. } => {
                format!("fn {name} = {}", self.expr(value, indent))
            }
            StmtKind::Assign { target, value } => {
                format!(
                    "{} = {}",
                    self.expr(target, indent),
                    self.expr(value, indent)
                )
            }
            StmtKind::Discard(e) => format!("_ = {}", self.expr(e, indent)),
            StmtKind::Expr(e) => self.expr(e, indent),
            StmtKind::Return(None) => "return".into(),
            StmtKind::Return(Some(e)) => {
                // `return a, b` (a tuple written without parentheses)
                if let ExprKind::Tuple(items) = &e.kind
                    && !self.text(e.span).starts_with('(')
                {
                    return format!("return {}", self.list(items, indent));
                }
                format!("return {}", self.expr(e, indent))
            }
            StmtKind::Throw(e) => format!("throw {}", self.expr(e, indent)),
            StmtKind::Break { label, value } => match (label, value) {
                (Some(l), _) => format!("break :{l}"),
                (None, Some(v)) => format!("break {}", self.expr(v, indent)),
                (None, None) => "break".into(),
            },
            StmtKind::Continue(Some(l)) => format!("continue :{l}"),
            StmtKind::Continue(None) => "continue".into(),
            StmtKind::While { label, cond, body } => {
                let c = self.expr(cond, indent);
                let b = self.block(body, indent, false);
                format!("{}while {c} {b}", Self::label(label))
            }
            StmtKind::For {
                label,
                var,
                iter,
                body,
                ..
            } => {
                let it = self.expr(iter, indent);
                let b = self.block(body, indent, false);
                format!("{}for {var} in {it} {b}", Self::label(label))
            }
            StmtKind::Lock {
                label, name, body, ..
            } => {
                let b = self.block(body, indent, false);
                format!("{}lock {name} {b}", Self::label(label))
            }
            StmtKind::Block(b) => self.block(b, indent, false),
            StmtKind::Fn(f) => self.fn_decl(f, indent),
            StmtKind::Assert(e) => format!("assert {}", self.expr(e, indent)),
        }
    }

    // ------------------------------------------------------------------ expressions

    /// Literal source text without any surrounding parentheses.
    fn literal(&self, e: &Expr) -> String {
        let mut t = self.text(e.span).trim();
        while t.starts_with('(') && t.ends_with(')') && !t.starts_with("(\"") {
            t = t[1..t.len() - 1].trim();
        }
        t.to_string()
    }

    fn paren(&mut self, e: &Expr, indent: usize, needed: bool) -> String {
        let s = self.expr(e, indent);
        if needed { format!("({s})") } else { s }
    }

    /// Expressions separated by commas.
    fn list(&mut self, items: &[Expr], indent: usize) -> String {
        let items: Vec<String> = items.iter().map(|x| self.expr(x, indent)).collect();
        items.join(", ")
    }

    /// Bracketed list (`brackets` is the opening and closing character, the opening one at
    /// source offset `start`, the closing one at `end`), one entry per line if the original
    /// spanned several lines or the list holds comments of its own. Line wrapping knows it by
    /// `id`, by default `end`.
    #[allow(clippy::too_many_arguments)]
    fn bracketed(
        &mut self,
        brackets: &str,
        entries: Vec<Entry<'_, 'a>>,
        multi: bool,
        indent: usize,
        start: usize,
        end: usize,
        id: Option<usize>,
    ) -> String {
        let (open, close) = brackets.split_at(1);
        let id = id.unwrap_or(end);
        let ranges: Vec<(usize, usize)> = entries.iter().map(|e| (e.start, e.end)).collect();
        let comments = self.list_has_comments(start, end, &ranges);
        let multi = ((multi || self.force.contains(&id)) && !entries.is_empty()) || comments;
        if !multi {
            let items: Vec<String> = entries.iter().map(|e| (e.show)(self, indent)).collect();
            let mark = if entries.is_empty() {
                String::new()
            } else {
                format!("{MARK_START}{id}{MARK_END}")
            };
            return format!("{mark}{open}{}{close}", items.join(", "));
        }
        let saved = std::mem::take(&mut self.out);
        self.out.push_str(open);
        self.out.push('\n');
        self.cursor = start + 1;
        for (i, e) in entries.iter().enumerate() {
            self.flush_comments(e.start, indent + 1, i == 0);
            let text = (e.show)(self, indent + 1);
            self.out.push_str(&ind(indent + 1));
            self.out.push_str(&text);
            self.out.push_str(",\n");
            // a comment after the entry (and its comma) on the same line stays at its end
            let after = e.end + usize::from(self.src[e.end..].starts_with(','));
            self.cursor = after;
            let next = entries.get(i + 1).map_or(end, |n| n.start);
            if self.comments.get(self.next).is_some_and(|c| c.0 < next) {
                self.trailing_comment(after);
            }
        }
        self.flush_comments(end, indent + 1, entries.is_empty());
        self.out.push_str(&ind(indent));
        self.out.push_str(close);
        std::mem::replace(&mut self.out, saved)
    }

    fn expr(&mut self, e: &Expr, indent: usize) -> String {
        match &e.kind {
            ExprKind::Int(..) | ExprKind::Float(_) | ExprKind::Char(_) | ExprKind::Str(_) => {
                self.literal(e)
            }
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::None => "none".into(),
            ExprKind::Ident(n) => n.clone(),
            ExprKind::Wildcard => "_".into(),
            ExprKind::Dollar => "$".into(),
            ExprKind::Error => self.literal(e),
            ExprKind::Unary(op, x) => {
                let space = if *op == UnOp::Not { " " } else { "" };
                let inner = self.paren(x, indent, prec(x) < UNARY);
                format!("{}{space}{inner}", op.symbol())
            }
            ExprKind::Binary(op, l, r) => {
                let p = op.prec();
                let lp = prec(l);
                let rp = prec(r);
                let left_parens =
                    lp < p || (p == COMPARE && lp == COMPARE) || (*op == BinOp::Pow && lp == p);
                let right_parens = rp < p || (rp == p && *op != BinOp::Pow);
                let a = self.paren(l, indent, left_parens);
                let b = self.paren(r, indent, right_parens);
                format!("{a} {} {b}", op.symbol())
            }
            ExprKind::Call(f, args) => {
                let callee = self.paren(f, indent, prec(f) < POSTFIX);
                let multi = (self.multiline(e.span) && args.len() > 1 && self.args_multiline(e))
                    || (self.force.contains(&(e.span.end as usize)) && !args.is_empty());
                let open = self.find_outside_comments(f.span.end as usize, '(');
                let end = e.span.end as usize;
                let entries = expr_entries(args);
                let ranges: Vec<_> = entries.iter().map(|e| (e.start, e.end)).collect();
                if !multi && hugs(args) && !self.list_has_comments(open, end, &ranges) {
                    // `f([...])`: the inner list breaks, not the call
                    return format!("{callee}({})", self.list(args, indent));
                }
                let list = self.bracketed("()", entries, multi, indent, open, end, None);
                format!("{callee}{list}")
            }
            ExprKind::Generic(inner, args) => {
                let base = self.paren(inner, indent, prec(inner) < POSTFIX);
                format!("{base}<{}>", TypeExpr::show_list(args))
            }
            ExprKind::Builtin(name, ty, args) => {
                let mut entries = Vec::new();
                if let Some(t) = ty {
                    entries.push(entry(t.span(), move |_, _| t.show()));
                }
                entries.extend(expr_entries(args));
                let open = self.find_outside_comments(e.span.start as usize, '(');
                let end = e.span.end as usize;
                let ranges: Vec<_> = entries.iter().map(|e| (e.start, e.end)).collect();
                if ty.is_none() && hugs(args) && !self.list_has_comments(open, end, &ranges) {
                    return format!("@{name}({})", self.list(args, indent));
                }
                let list = self.bracketed("()", entries, false, indent, open, end, None);
                format!("@{name}{list}")
            }
            ExprKind::Field(b, name) => {
                let base = self.paren(b, indent, prec(b) < POSTFIX);
                format!("{base}.{name}")
            }
            ExprKind::Index(b, i) => {
                let base = self.paren(b, indent, prec(b) < POSTFIX);
                format!("{base}[{}]", self.expr(i, indent))
            }
            ExprKind::Slice(b, lo, hi) => {
                let base = self.paren(b, indent, prec(b) < POSTFIX);
                format!(
                    "{base}[{}:{}]",
                    self.expr(lo, indent),
                    self.expr(hi, indent)
                )
            }
            ExprKind::Tuple(items) => format!("({})", self.list(items, indent)),
            ExprKind::Array(items) => {
                let multi = self.multiline(e.span);
                let entries = expr_entries(items);
                let open = self.find_outside_comments(e.span.start as usize, '[');
                let end = e.span.end as usize;
                self.bracketed("[]", entries, multi, indent, open, end, None)
            }
            ExprKind::Map(entries) => {
                let multi = self.multiline(e.span);
                let entries = entries
                    .iter()
                    .map(|(k, v)| {
                        entry(k.span.to(v.span), move |p, i| {
                            format!("{}: {}", p.expr(k, i), p.expr(v, i))
                        })
                    })
                    .collect();
                let open = self.find_outside_comments(e.span.start as usize, '[');
                let end = e.span.end as usize;
                self.bracketed("[]", entries, multi, indent, open, end, None)
            }
            ExprKind::Struct(ty, fields) => {
                let t = self.expr(ty, indent);
                let multi = self.multiline(e.span);
                let entries = fields
                    .iter()
                    .map(|(name, v, sp)| {
                        entry(sp.to(v.span), move |p, i| {
                            format!(".{name} = {}", p.expr(v, i))
                        })
                    })
                    .collect();
                let open = self.find_outside_comments(ty.span.end as usize, '{');
                let end = e.span.end as usize;
                let body = self.bracketed("{}", entries, multi, indent, open, end, None);
                format!("{t}{body}")
            }
            ExprKind::FnLit(sig, body) => {
                let s = self.sig(sig, false, indent);
                let b = self.block(body, indent, true);
                format!("{s} {b}")
            }
            ExprKind::If(i) => self.if_expr(i, indent),
            ExprKind::Else(v, fb) => {
                // `a else b else c` chains to the left; only a nested fallback needs parentheses
                let value = self.expr(v, indent);
                let fallback = match fb {
                    ElseFallback::Expr(x) => {
                        let nested = matches!(x.kind, ExprKind::Else(..) | ExprKind::Catch(..));
                        self.paren(x, indent, nested)
                    }
                    ElseFallback::Block(b) => self.block(b, indent, true),
                };
                format!("{value} else {fallback}")
            }
            ExprKind::Catch(v, name, _, b) => {
                let value = self.expr(v, indent);
                let block = self.block(b, indent, true);
                format!("{value} catch {name} {block}")
            }
            ExprKind::Try(x) | ExprKind::Async(x) | ExprKind::Await(x) => {
                let kw = match e.kind {
                    ExprKind::Try(_) => "try",
                    ExprKind::Async(_) => "async",
                    _ => "await",
                };
                format!("{kw} {}", self.paren(x, indent, prec(x) < UNARY))
            }
        }
    }

    /// Whether a call's arguments started on separate lines in the source.
    fn args_multiline(&self, e: &Expr) -> bool {
        let ExprKind::Call(_, args) = &e.kind else {
            return false;
        };
        args.windows(2)
            .any(|w| self.src[w[0].span.end as usize..w[1].span.start as usize].contains('\n'))
    }

    fn if_expr(&mut self, i: &IfExpr, indent: usize) -> String {
        let mut out = Self::label(&i.label);
        out.push_str("if ");
        if let Some(s) = &i.scrutinee {
            out.push_str(&self.expr(s, indent));
            out.push(' ');
        }
        out.push_str("{\n");
        let saved = std::mem::take(&mut self.out);
        self.cursor = i
            .arms
            .first()
            .map_or(i.span.end as usize, |a| a.span.start as usize);
        for (k, arm) in i.arms.iter().enumerate() {
            self.entry_start(arm.span.start as usize, indent + 1, k == 0);
            let pats = self.list(&arm.patterns, indent + 1);
            let body = self.block(&arm.body, indent + 1, true);
            self.out.push_str(&ind(indent + 1));
            self.out.push_str(&format!("{pats} -> {body}\n"));
            self.cursor = arm.body.span.end as usize;
            self.trailing_comment(arm.body.span.end as usize);
        }
        self.flush_comments(i.span.end as usize, indent + 1, false);
        let arms = std::mem::replace(&mut self.out, saved);
        out.push_str(&arms);
        out.push_str(&ind(indent));
        out.push('}');
        out
    }
}

// precedence levels around the binary operators' (`BinOp::prec`; higher binds tighter)
const ELSE: u8 = 0;
const COMPARE: u8 = 3;
const UNARY: u8 = 12;
const POSTFIX: u8 = 14;

fn prec(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Binary(op, ..) => op.prec(),
        ExprKind::Unary(..) | ExprKind::Try(_) | ExprKind::Async(_) | ExprKind::Await(_) => UNARY,
        ExprKind::Else(..) | ExprKind::Catch(..) | ExprKind::If(_) | ExprKind::FnLit(..) => ELSE,
        _ => POSTFIX,
    }
}

#[cfg(test)]
mod tests {
    use super::format;

    fn fmt(s: &str) -> String {
        format(s).unwrap_or_else(|e| panic!("{e:?}"))
    }

    #[test]
    fn normalises_spacing_and_parens() {
        assert_eq!(fmt("int  x=( 1+2 )*3\n"), "int x = (1 + 2) * 3\n");
        assert_eq!(fmt("int y = (1 * 2) + 3\n"), "int y = 1 * 2 + 3\n");
        assert_eq!(fmt("int z = -(2 ** 2)\n"), "int z = -(2 ** 2)\n");
        assert_eq!(fmt("int w = (a - b) - c\n"), "int w = a - b - c\n");
        assert_eq!(fmt("int v = a - (b - c)\n"), "int v = a - (b - c)\n");
        assert_eq!(fmt("int p = 2 ** (3 ** 2)\n"), "int p = 2 ** 3 ** 2\n");
    }

    #[test]
    fn keeps_comments_and_blank_lines() {
        let src = "// header\n\n\n// about a\nint a = 1 // trailing\nfn f() {\n    // inside\n  return\n}\n";
        assert_eq!(
            fmt(src),
            "// header\n\n// about a\nint a = 1 // trailing\nfn f() {\n  // inside\n  return\n}\n"
        );
    }

    #[test]
    fn keeps_literals_and_layouts() {
        let src = "[str]int m = [\n    \"a\": 0xFF,\n  \"b\": 1_000\n]\nif x {\n  1 -> {   break 'a'   }\n  _ -> {\n  break 'b'\n  }\n}\n";
        assert_eq!(
            fmt(src),
            "[str]int m = [\n  \"a\": 0xFF,\n  \"b\": 1_000,\n]\nif x {\n  1 -> { break 'a' }\n  _ -> {\n    break 'b'\n  }\n}\n"
        );
    }

    #[test]
    fn wraps_long_lines() {
        let long = "fn combine(int first_argument_name, int second_argument_name, str third_argument, bool fourth_flag) int {\n  return combine(first_argument_name + second_argument_name, second_argument_name * 2, third_argument, fourth_flag)\n}\n";
        let out = fmt(long);
        assert_eq!(
            out,
            "fn combine(\n  int first_argument_name,\n  int second_argument_name,\n  str third_argument,\n  bool fourth_flag,\n) int {\n  return combine(\n    first_argument_name + second_argument_name,\n    second_argument_name * 2,\n    third_argument,\n    fourth_flag,\n  )\n}\n"
        );
        assert_eq!(fmt(&out), out);
        // a lone list argument breaks instead of the call
        let src = format!(
            "@println([{}])\n",
            (0..30)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let out = fmt(&src);
        assert!(out.starts_with("@println([\n  0,\n"), "{out}");
        assert!(out.ends_with("  29,\n])\n"), "{out}");
        // short lines are left alone
        assert_eq!(fmt("int x = f(1, 2)\n"), "int x = f(1, 2)\n");
        assert!(out.lines().all(|l| l.chars().count() <= super::MAX_WIDTH));
    }

    #[test]
    fn comment_on_first_member() {
        let src = "int a = 1\n\nstruct P {\n  /// x\n  int x\n}\n\nenum E {\n  // c\n  A\n}\n";
        assert_eq!(fmt(src), src);
        let src = "int a = 1\n\nextern \"m.h\" as m {\n  // g\n  fn g() = \"g\"\n}\n";
        assert_eq!(fmt(src), src);
    }

    #[test]
    fn blank_lines_between_members() {
        let src = "struct P {\n  int x\n\n\n  int y\n  // c\n\n  int z\n}\n\nenum E {\n\n  A\n\n  B\n}\n\nextern \"m.h\" as m {\n  fn f() = \"f\"\n\n  fn g() = \"g\"\n}\n";
        let out = "struct P {\n  int x\n\n  int y\n  // c\n\n  int z\n}\n\nenum E {\n  A\n\n  B\n}\n\nextern \"m.h\" as m {\n  fn f() = \"f\"\n\n  fn g() = \"g\"\n}\n";
        assert_eq!(fmt(src), out);
        assert_eq!(fmt(out), out);
    }

    #[test]
    fn empty_members() {
        let src =
            "struct Q {\n}\nenum E {}\nextern \"m.h\" as m {\n\n}\nstruct C {\n  // only\n}\n";
        let out = "struct Q {}\nenum E {}\nextern \"m.h\" as m {}\nstruct C {\n  // only\n}\n";
        assert_eq!(fmt(src), out);
        assert_eq!(fmt(out), out);
    }

    #[test]
    fn comment_after_one_line_body() {
        let src = "fn r() int { return 0 } // tail\nstruct P { int x } // p\n";
        let out = "fn r() int {\n  return 0\n} // tail\nstruct P {\n  int x\n} // p\n";
        assert_eq!(fmt(src), out);
        assert_eq!(fmt(out), out);
    }

    #[test]
    fn comments_in_parameter_lists() {
        let src = "fn f(int a, // first\n  int b) int {\n  return a + b\n}\n";
        let out = "fn f(\n  int a, // first\n  int b,\n) int {\n  return a + b\n}\n";
        assert_eq!(fmt(src), out);
        assert_eq!(fmt(out), out);
        let src = "fn g(\n  // lead\n  int a, int b) {}\n";
        let out = "fn g(\n  // lead\n  int a,\n  int b,\n) {}\n";
        assert_eq!(fmt(src), out);
        assert_eq!(fmt(out), out);
        let src = "extern \"m.h\" as m {\n  fn h(int a, // c\n    int b) = \"h\"\n}\n";
        let out = "extern \"m.h\" as m {\n  fn h(\n    int a, // c\n    int b,\n  ) = \"h\"\n}\n";
        assert_eq!(fmt(src), out);
        assert_eq!(fmt(out), out);
        // call arguments: a comment after the last one stays there
        let src = "int y = f(1, 2, // two\n)\n";
        let out = "int y = f(\n  1,\n  2, // two\n)\n";
        assert_eq!(fmt(src), out);
        assert_eq!(fmt(out), out);
    }

    #[test]
    fn blank_line_after_leading_comment() {
        assert_eq!(fmt("// file\n\nint a = 1\n"), "// file\n\nint a = 1\n");
        assert_eq!(fmt("fn f() {\n\n  return\n}\n"), "fn f() {\n  return\n}\n");
    }

    #[test]
    fn idempotent() {
        let src = "import {\n  \"./a\" as a\n  \"./b\" as b\n}\n\npub struct P<type T> {\n  T x // the x\n}\n\nfn f(int a) int! {\n  int y = g() catch e {\n    throw e\n  }\n  return y\n}\n";
        let once = fmt(src);
        assert_eq!(once, src);
        assert_eq!(fmt(&once), once);
    }
}
