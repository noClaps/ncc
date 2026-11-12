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

/// A list entry: its source offset and a printer for it at a given indentation.
type EntryFn<'e, 'a> = Box<dyn Fn(&mut Printer<'a>, usize) -> String + 'e>;
type Entry<'e, 'a> = (usize, EntryFn<'e, 'a>);

fn entry<'e, 'a>(
    start: usize,
    f: impl Fn(&mut Printer<'a>, usize) -> String + 'e,
) -> Entry<'e, 'a> {
    (start, Box::new(f))
}

/// One entry per expression.
fn expr_entries<'a>(xs: &[Expr]) -> Vec<Entry<'_, 'a>> {
    xs.iter()
        .map(|x| entry(x.span.start as usize, move |p, i| p.expr(x, i)))
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

    /// Append a comment that follows `end` on the same source line to the last printed line.
    fn trailing_comment(&mut self, end: usize) {
        if self.next < self.comments.len() {
            let (s, e) = self.comments[self.next];
            if s >= end && !self.src[end..s].contains('\n') {
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
                self.members(head, fns, e.span, false, |p, f| {
                    let sig = p.sig(&f.sig, true, 1);
                    format!("{sig} = {}", quote(&f.symbol))
                });
            }
            Item::Struct(s) => {
                let params = GenericParam::list(&s.type_params);
                let head = format!("{}struct {}{params}", vis(s.public), s.name);
                let fields = s.fields.iter().map(|f| (f.2, f));
                self.members(head, fields, s.span, true, |_, (ty, name, _)| {
                    format!("{} {name}", ty.show())
                });
            }
            Item::Enum(e) => {
                let params = GenericParam::list(&e.type_params);
                let head = format!("{}enum {}{params}", vis(e.public), e.name);
                let variants = e.variants.iter().map(|v| (v.2, v));
                self.members(head, variants, e.span, true, |_, (name, tys, _)| {
                    if tys.is_empty() {
                        name.clone()
                    } else {
                        format!("{name}({})", TypeExpr::show_list(tys))
                    }
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
            Item::Test(name, body, ..) => {
                let b = self.block(body, 0, false);
                self.out.push_str(&format!("test {} {b}\n", quote(name)));
            }
            Item::Stmt(s) => {
                let text = self.stmt(s, 0);
                self.out.push_str(&text);
                self.out.push('\n');
            }
        }
    }

    /// `head { ... }` (an extern block, struct or enum ending at `span`), one member per line;
    /// `show` prints a member after the comments before it. With `fresh`, comments before the
    /// first member never get a blank line above them.
    fn members<'m, T: 'm>(
        &mut self,
        head: String,
        members: impl Iterator<Item = (Span, &'m T)>,
        span: Span,
        fresh: bool,
        show: impl Fn(&mut Self, &T) -> String,
    ) {
        self.out.push_str(&head);
        self.out.push_str(" {\n");
        for (i, (sp, m)) in members.enumerate() {
            self.flush_comments(sp.start as usize, 1, fresh && i == 0);
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
            .map(|p| {
                entry(p.span.start as usize, move |_, _| {
                    format!("{} {}", p.ty.show(), p.name)
                })
            })
            .collect();
        // the parameter list ends at the `)` after the last parameter
        let end = s
            .params
            .last()
            .map(|p| p.span.end as usize)
            .and_then(|e| self.src[e..].find(')').map(|i| e + i))
            .unwrap_or(s.span.end as usize);
        let id = SIG_ID + s.span.start as usize;
        let params = self.bracketed("()", entries, false, indent, end, Some(id));
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

    /// Bracketed list (`brackets` is the opening and closing character), one entry per line if
    /// the original spanned several lines. Line wrapping knows it by `id`, by default the source
    /// end offset.
    fn bracketed(
        &mut self,
        brackets: &str,
        entries: Vec<Entry<'_, 'a>>,
        multi: bool,
        indent: usize,
        end: usize,
        id: Option<usize>,
    ) -> String {
        let (open, close) = brackets.split_at(1);
        let id = id.unwrap_or(end);
        let multi = multi || self.force.contains(&id);
        if !multi || entries.is_empty() {
            let items: Vec<String> = entries.iter().map(|(_, f)| f(self, indent)).collect();
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
        for (start, f) in &entries {
            self.flush_comments(*start, indent + 1, false);
            let text = f(self, indent + 1);
            self.out.push_str(&ind(indent + 1));
            self.out.push_str(&text);
            self.out.push_str(",\n");
        }
        self.flush_comments(end, indent + 1, false);
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
                if !multi && hugs(args) {
                    // `f([...])`: the inner list breaks, not the call
                    return format!("{callee}({})", self.list(args, indent));
                }
                let entries = expr_entries(args);
                let list = self.bracketed("()", entries, multi, indent, e.span.end as usize, None);
                format!("{callee}{list}")
            }
            ExprKind::Generic(inner, args) => {
                let base = self.paren(inner, indent, prec(inner) < POSTFIX);
                format!("{base}<{}>", TypeExpr::show_list(args))
            }
            ExprKind::Builtin(name, ty, args) => {
                let mut entries = Vec::new();
                if let Some(t) = ty {
                    entries.push(entry(t.span().start as usize, move |_, _| t.show()));
                }
                entries.extend(expr_entries(args));
                if ty.is_none() && hugs(args) {
                    return format!("@{name}({})", self.list(args, indent));
                }
                let list = self.bracketed("()", entries, false, indent, e.span.end as usize, None);
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
                self.bracketed("[]", entries, multi, indent, e.span.end as usize, None)
            }
            ExprKind::Map(entries) => {
                let multi = self.multiline(e.span);
                let entries = entries
                    .iter()
                    .map(|(k, v)| {
                        entry(k.span.start as usize, move |p, i| {
                            format!("{}: {}", p.expr(k, i), p.expr(v, i))
                        })
                    })
                    .collect();
                self.bracketed("[]", entries, multi, indent, e.span.end as usize, None)
            }
            ExprKind::Struct(ty, fields) => {
                let t = self.expr(ty, indent);
                let multi = self.multiline(e.span);
                let entries = fields
                    .iter()
                    .map(|(name, v, sp)| {
                        entry(sp.start as usize, move |p, i| {
                            format!(".{name} = {}", p.expr(v, i))
                        })
                    })
                    .collect();
                let body = self.bracketed("{}", entries, multi, indent, e.span.end as usize, None);
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
