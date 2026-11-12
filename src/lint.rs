//! Warnings that do not prevent compilation.

use std::collections::{HashMap, HashSet};

use crate::check::flow;
use crate::diag::{Diag, Span};
use crate::tir::*;

/// The first shared mutable variable a function reads or assigns: a mutable global or a
/// captured reference.
fn shared_access(p: &Program, f: FuncId) -> Option<&str> {
    let func = &p.funcs[f];
    let mut vars = Vec::new();
    for s in &func.body.stmts {
        flow::visit_stmt(s, &mut |e| {
            if let EK::Var(v) = e.kind {
                vars.push(v);
            }
        });
    }
    for s in &func.body.stmts {
        flow::visit_stmts(s, &mut |s| {
            if let SK::Assign(pl, _) = &s.kind {
                vars.extend(flow::place_root(pl));
            }
        });
    }
    vars.into_iter().find_map(|v| match v {
        VarRef::Global(g) if p.globals[g].mutable => Some(p.globals[g].name.as_str()),
        VarRef::Local(l) if matches!(func.locals[l].kind, LocalKind::CapRef(_)) => {
            Some(func.locals[l].name.as_str())
        }
        _ => None,
    })
}

fn callees(p: &Program, f: FuncId) -> Vec<FuncId> {
    let mut out = Vec::new();
    for s in &p.funcs[f].body.stmts {
        flow::visit_stmt(s, &mut |e: &Expr| match &e.kind {
            EK::Call(Callee::Direct(g), _) | EK::FnRef(g) | EK::Closure(g) => out.push(*g),
            _ => {}
        });
    }
    out
}

/// Warn about `async` calls whose code touches shared mutable state outside a mutex.
pub fn race_warnings(p: &Program) -> Vec<Diag> {
    let mut warnings = Vec::new();
    let mut seen: HashSet<Span> = HashSet::new();
    for f in &p.funcs {
        // locals initialised with a closure: lets us resolve `async local_fn()`
        let mut closures: HashMap<LocalId, FuncId> = HashMap::new();
        let mut asyncs: Vec<(Option<FuncId>, Span)> = Vec::new();
        for s in &f.body.stmts {
            collect(s, &mut closures, &mut asyncs);
        }
        for (target, span) in asyncs {
            let Some(target) = target else { continue };
            // transitive closure of reachable functions
            let mut stack = vec![target];
            let mut visited = HashSet::new();
            let mut hit = None;
            while hit.is_none()
                && let Some(g) = stack.pop()
            {
                if visited.insert(g) {
                    hit = shared_access(p, g);
                    stack.extend(callees(p, g));
                }
            }
            if let Some(name) = hit
                && seen.insert(span)
            {
                warnings.push(Diag::new(
                        span,
                        format!(
                            "possible data race: `{}` runs asynchronously and accesses the mutable variable `{name}` without a mutex",
                            p.funcs[target].name
                        ),
                    ));
            }
        }
    }
    warnings
}

fn collect(
    s: &Stmt,
    closures: &mut HashMap<LocalId, FuncId>,
    asyncs: &mut Vec<(Option<FuncId>, Span)>,
) {
    if let SK::Let(
        Some(VarRef::Local(l)),
        Expr {
            kind: EK::Closure(f),
            ..
        },
    ) = &s.kind
    {
        closures.insert(*l, *f);
    }
    let mut found = Vec::new();
    flow::visit_stmt(s, &mut |e: &Expr| {
        if let EK::Async(c, _) = &e.kind {
            let target = match c {
                Callee::Direct(f) => Some(*f),
                Callee::Value(v) => match &v.kind {
                    EK::Closure(f) | EK::FnRef(f) => Some(*f),
                    EK::Var(VarRef::Local(l)) => closures.get(l).copied(),
                    _ => None,
                },
                Callee::Extern(_) => None,
            };
            found.push((target, e.span));
        }
    });
    asyncs.extend(found);
}

// ------------------------------------------------------------------ unused code

/// What an unused-code diagnostic is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnusedKind {
    Variable,
    Parameter,
    Function,
    Type,
    Import,
}

impl UnusedKind {
    /// The diagnostic code the language server attaches (and its quick fixes look for).
    pub fn code(self) -> &'static str {
        match self {
            Self::Variable => "unused-variable",
            Self::Parameter => "unused-parameter",
            Self::Function => "unused-function",
            Self::Type => "unused-type",
            Self::Import => "unused-import",
        }
    }
}

pub struct Unused {
    pub diag: Diag,
    pub kind: UnusedKind,
}

/// Unused local variables, parameters, private items and imports of the project's modules
/// (not dependencies). Names starting with `_` and `pub` items are never reported. Uses
/// anywhere, including tests, count.
pub fn unused(ix: &crate::check::index::Index, types: &crate::types::Types) -> Vec<Unused> {
    use crate::check::index::DeclKind;
    use crate::types::TyKind;
    let facts: HashMap<u32, &crate::check::index::ModFacts> =
        ix.modules.iter().map(|m| (m.file, m)).collect();
    let mut used: HashSet<Span> = HashSet::new();
    // declaration spans that are also a use of something else (a `lock` alias)
    let mut shadowing: HashSet<Span> = HashSet::new();
    for r in &ix.refs {
        if let Some(def) = r.def
            && r.span != def
        {
            used.insert(def);
            shadowing.insert(r.span);
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for d in &ix.decls {
        let Some(f) = facts.get(&d.span.file) else {
            continue;
        };
        if !f.project
            || ix
                .editor_only
                .iter()
                .any(|b| b.file == d.span.file && b.start <= d.span.start && d.span.end <= b.end)
            || d.name.is_empty()
            || d.name.starts_with('_')
            || matches!(d.kind, DeclKind::Field | DeclKind::Variant)
            || used.contains(&d.span)
            || shadowing.contains(&d.span)
            || !seen.insert(d.span)
            || (d.scope.is_none() && d.extern_alias.is_none() && f.public.contains(&d.name))
        {
            continue;
        }
        let (kind, what) = match d.kind {
            DeclKind::Variable if d.param => (UnusedKind::Parameter, "parameter"),
            // a function declared inside another one (its span is just the name)
            DeclKind::Variable
                if d.span == d.name_span
                    && d.ty
                        .is_some_and(|t| matches!(types.kind(t), TyKind::Fn(..))) =>
            {
                (UnusedKind::Function, "function")
            }
            DeclKind::Variable => (UnusedKind::Variable, "variable"),
            DeclKind::Function if d.extern_alias.is_some() => {
                (UnusedKind::Function, "extern function")
            }
            DeclKind::Function => (UnusedKind::Function, "function"),
            DeclKind::Struct => (UnusedKind::Type, "struct"),
            DeclKind::Enum => (UnusedKind::Type, "enum"),
            DeclKind::Type => (UnusedKind::Type, "type"),
            DeclKind::Field | DeclKind::Variant => continue,
        };
        out.push(Unused {
            diag: Diag::new(
                d.name_span,
                format!(
                    "unused {what} `{}`; prefix the name with `_` if this is intended",
                    d.name
                ),
            ),
            kind,
        });
    }
    for f in ix.modules.iter().filter(|f| f.project) {
        for (alias, span, is_used) in &f.imports {
            if *is_used || alias.starts_with('_') {
                continue;
            }
            out.push(Unused {
                diag: Diag::new(*span, format!("unused import `{alias}`")),
                kind: UnusedKind::Import,
            });
        }
    }
    out.sort_by_key(|u| (u.diag.span.file, u.diag.span.start));
    out
}

/// Loops that never finish, and the code they make unreachable.
#[derive(Default)]
pub struct Loops {
    /// Loops nothing can leave (except a panic): the head of each loop.
    pub infinite: Vec<Diag>,
    /// Code after a loop that never finishes.
    pub unreachable: Vec<Diag>,
}

/// Find `while` loops whose condition is always true (a constant, or true from values known
/// when the loop starts that nothing in the loop changes) and that no `break` ends. Code after such a
/// loop never runs; when nothing else leaves the loop either (`return`, `throw`, a `break` or
/// `continue` of an outer loop, or an error propagated with `try`), the loop runs forever.
pub fn loops(p: &Program, sm: &crate::diag::SourceMap) -> Loops {
    let mut consts = crate::eval::Consts::new(p, sm);
    let known = consts.endless_loops();
    let mut out = Loops::default();
    let mut seen = HashSet::new();
    for (fi, f) in p.funcs.iter().enumerate() {
        let mut breaks = HashSet::new();
        for s in &f.body.stmts {
            flow::visit_stmts(s, &mut |s| {
                if let SK::Break(id, _) = s.kind {
                    breaks.insert(id);
                }
            });
        }
        let mut cx = LoopCx {
            sm,
            consts: &mut consts,
            known: &known,
            func: fi,
            breaks: &breaks,
            out: &mut out,
            seen: &mut seen,
        };
        cx.block(&f.body);
    }
    out
}

struct LoopCx<'a, 'p> {
    sm: &'a crate::diag::SourceMap,
    consts: &'a mut crate::eval::Consts<'p>,
    /// loops whose condition stays true by the values known on entry
    known: &'a HashSet<(FuncId, JumpId)>,
    func: FuncId,
    /// loops and blocks that some `break` leaves
    breaks: &'a HashSet<JumpId>,
    out: &'a mut Loops,
    /// spans already reported (generic functions are checked once per instantiation)
    seen: &'a mut HashSet<(bool, Span)>,
}

impl LoopCx<'_, '_> {
    /// Dependencies are not checked, only the project's own files.
    fn project(&self, span: Span) -> bool {
        !crate::modules::is_dependency(&self.sm.file(span.file).path)
    }

    /// Analyse a block; whether it never finishes because of a loop that never ends.
    fn block(&mut self, b: &Block) -> bool {
        let mut endless = None;
        for (i, s) in b.stmts.iter().enumerate() {
            if self.stmt(s) {
                endless = Some(i);
                break;
            }
        }
        let Some(i) = endless else {
            return false;
        };
        let at = b.stmts[i].span;
        let rest: Vec<Span> = b.stmts[i + 1..]
            .iter()
            .map(|s| s.span)
            .filter(|sp| sp.file == at.file && sp.end > sp.start && sp.start >= at.end)
            .collect();
        if let (Some(first), Some(last)) = (rest.first(), rest.last()) {
            let span = first.to(*last);
            if self.project(span) && self.seen.insert((false, span)) {
                self.out.unreachable.push(Diag::new(
                    span,
                    "unreachable code: the loop before it never finishes",
                ));
            }
        }
        true
    }

    /// Analyse a statement; whether it never finishes because of a loop that never ends.
    fn stmt(&mut self, s: &Stmt) -> bool {
        // blocks inside expressions (those in loop bodies are seen again: reports are deduplicated)
        let mut nested = vec![];
        flow::visit_stmt(s, &mut |e| nested.extend(flow::nested_blocks(e)));
        for b in nested {
            self.block(b);
        }
        match &s.kind {
            SK::While { id, cond, body } => {
                self.block(body);
                let endless = !self.breaks.contains(id)
                    && (self.consts.always_true(cond) || self.known.contains(&(self.func, *id)));
                if endless && !leaves(body, *id) {
                    let head = loop_head(self.sm, s.span);
                    if self.project(head) && self.seen.insert((true, head)) {
                        self.out.infinite.push(Diag::new(
                            head,
                            "this loop never ends: its condition stays true and nothing leaves it",
                        ));
                    }
                }
                endless
            }
            SK::For { body, .. } | SK::Lock { body, .. } => {
                self.block(body);
                false
            }
            SK::Block(b) => self.block(b),
            _ => false,
        }
    }
}

/// Whether anything in the body of loop `id` leaves it other than a `break` of it.
fn leaves(body: &Block, id: JumpId) -> bool {
    // jump targets inside the body: jumps to them stay inside
    let mut inner = HashSet::from([id]);
    let mut found = false;
    for s in &body.stmts {
        flow::visit_stmts(s, &mut |s| match &s.kind {
            SK::While { id, .. } | SK::For { id, .. } | SK::Lock { id, .. } => {
                inner.insert(*id);
            }
            SK::Return(_) | SK::Throw(_) => found = true,
            _ => {}
        });
        flow::visit_stmt(s, &mut |e| match &e.kind {
            EK::If(i) => {
                inner.insert(i.id);
            }
            EK::Catch(_, _, _, j) | EK::Else(_, Fallback::Block(_, j)) => {
                inner.insert(*j);
            }
            EK::Try(_) => found = true,
            _ => {}
        });
    }
    for s in &body.stmts {
        flow::visit_stmts(s, &mut |s| match &s.kind {
            SK::Break(j, _) | SK::Continue(j) if !inner.contains(j) => found = true,
            _ => {}
        });
    }
    found
}

/// The first line of a loop, without its opening brace.
fn loop_head(sm: &crate::diag::SourceMap, span: Span) -> Span {
    let text = &sm.file(span.file).text;
    let (s, e) = (span.start as usize, (span.end as usize).min(text.len()));
    let line = text[s..e].split('\n').next().unwrap_or("");
    let head = line.trim_end().trim_end_matches('{').trim_end();
    Span::new(span.file, span.start as usize, s + head.len().max(1))
}
