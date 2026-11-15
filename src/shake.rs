//! Tree shaking of imports.
//!
//! An imported module's top-level statements only run when they initialise a global that the
//! program uses. Starting from the entry module's code and its tests, this pass follows the
//! functions and globals that kept code references, keeps the imported initialisers writing a
//! reached global, and drops the rest. The checker runs it once on the program it builds; release
//! builds run it again after folding and dead code elimination, which can remove the only uses of
//! an imported global or function. Extern functions are reached the same way, so only extern
//! files whose functions are called are compiled in.

use std::collections::HashSet;

use crate::check::flow;
use crate::tir::*;

/// What kept code references, transitively.
pub struct Reach {
    pub funcs: HashSet<FuncId>,
    pub globals: HashSet<GlobalId>,
    pub externs: HashSet<ExternId>,
}

/// Drop the imported initialisers in `main` whose globals nothing reaches. `entry_file` is the
/// source file of the entry module: its statements are always kept. Returns whether anything
/// was removed.
pub fn run(p: &mut Program, entry_file: u32) -> bool {
    let (_, keep) = analyse(p, entry_file);
    let main = &mut p.funcs[p.main];
    let mut k = keep.iter();
    main.body.stmts.retain(|_| *k.next().unwrap());
    keep.contains(&false)
}

/// What the program reaches from its entry module's code and tests.
pub fn reach(p: &Program, entry_file: u32) -> Reach {
    analyse(p, entry_file).0
}

/// Reachability, and for each statement of `main` whether it is kept.
fn analyse(p: &Program, entry_file: u32) -> (Reach, Vec<bool>) {
    let stmts = &p.funcs[p.main].body.stmts;
    let mut r = Reach {
        funcs: HashSet::new(),
        globals: HashSet::new(),
        externs: HashSet::new(),
    };
    let mut work: Vec<FuncId> = p.tests.iter().map(|t| t.func).collect();
    let mut keep: Vec<bool> = stmts.iter().map(|s| s.span.file == entry_file).collect();
    let writes: Vec<HashSet<GlobalId>> = stmts
        .iter()
        .map(|s| {
            let mut w = HashSet::new();
            flow::writes_stmt(s, &mut w);
            w
        })
        .collect();
    for (s, _) in stmts.iter().zip(&keep).filter(|(_, k)| **k) {
        refs(s, &mut r, &mut work);
    }
    loop {
        while let Some(f) = work.pop() {
            if r.funcs.insert(f) {
                for s in &p.funcs[f].body.stmts {
                    refs(s, &mut r, &mut work);
                }
            }
        }
        // imported initialisers of reached globals, which may reach more
        let mut changed = false;
        for ((s, k), w) in stmts.iter().zip(keep.iter_mut()).zip(&writes) {
            if !*k && w.iter().any(|g| r.globals.contains(g)) {
                *k = true;
                changed = true;
                refs(s, &mut r, &mut work);
            }
        }
        if !changed && work.is_empty() {
            return (r, keep);
        }
    }
}

/// Record the functions, globals and externs `s` references; new functions go on `work`.
fn refs(s: &Stmt, r: &mut Reach, work: &mut Vec<FuncId>) {
    // nested lets declare globals only at module level, covered by `writes_stmt`
    if let SK::Assign(pl, _) = &s.kind
        && let Some(VarRef::Global(g)) = flow::place_root(pl)
    {
        r.globals.insert(g);
    }
    flow::visit_stmt(s, &mut |e| match &e.kind {
        EK::Var(VarRef::Global(g)) => {
            r.globals.insert(*g);
        }
        EK::Call(Callee::Extern(x), _) | EK::Async(Callee::Extern(x), _) => {
            r.externs.insert(*x);
        }
        _ => work.extend(e.fn_ref().filter(|f| !r.funcs.contains(f))),
    });
}
