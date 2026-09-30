//! Conservative, non-fatal diagnostics for control flow and async shared state.
use crate::{
    ast::{Block, Expr, Function, Item},
    diagnostic::{Diagnostic, Diagnostics},
    sema::{CheckedModule, TypeInfo},
    visit,
};
use std::collections::{HashMap, HashSet};

pub fn data_races(checked: &CheckedModule) -> Diagnostics {
    let mut expressions = HashMap::new();
    let mut functions = HashMap::new();
    for item in &checked.module.items {
        visit::item(item, &mut |e| {
            expressions.insert(e.id(), e);
        });
        if let Item::Function(f) = item {
            functions.insert(f.name.as_str(), f);
        }
    }
    let analysis = Analysis {
        checked,
        expressions,
        functions,
    };
    let mut sites: Vec<_> = checked.async_sites.iter().collect();
    sites.sort_by_key(|(_, location)| (&location.path, location.span.start));
    let mut warnings = Vec::new();
    for (id, location) in sites {
        let Some(Expr::Async(call)) = analysis.expressions.get(id).copied() else {
            continue;
        };
        let Expr::Call { callee, .. } = call.unlocated() else {
            continue;
        };
        if analysis.callable(callee, &mut HashSet::new()) {
            warnings.push(Diagnostic {
                message: "potential data race: async call may access shared mutable state without a mutex; execution is allowed, but concurrent accesses can race".into(),
                span: location.span.clone(), path: Some(location.path.clone()),
            });
        }
    }
    warnings.extend(control_flow(checked).0);
    warnings.sort_by(|a, b| {
        (&a.path, a.span.start, a.span.end, &a.message).cmp(&(
            &b.path,
            b.span.start,
            b.span.end,
            &b.message,
        ))
    });
    warnings.dedup_by(|a, b| a.path == b.path && a.span == b.span && a.message == b.message);
    Diagnostics(warnings)
}

/// Flow-only diagnostics, available independently of async analysis.
pub fn control_flow(checked: &CheckedModule) -> Diagnostics {
    crate::flow::warnings(&checked.module, &checked.expression_types)
}

struct Analysis<'a> {
    checked: &'a CheckedModule,
    expressions: HashMap<usize, &'a Expr>,
    functions: HashMap<&'a str, &'a Function>,
}
impl Analysis<'_> {
    fn callable(&self, expr: &Expr, visited: &mut HashSet<usize>) -> bool {
        let expr = expr.unlocated();
        // Bound analysis cost and visit each callable expression once, including
        // recursive call graphs. Exceeding the budget is conservatively unsafe.
        if visited.len() >= 128 {
            return true;
        }
        if !visited.insert(expr.id()) {
            return false;
        }
        match expr {
            Expr::Name(name) if name.starts_with('@') => false,
            Expr::Name(name) => {
                if let Some(source) = self.checked.constant_sources.get(&expr.id()) {
                    source
                        .and_then(|id| self.expressions.get(&id))
                        .is_none_or(|value| self.callable(value, visited))
                } else if let Some(function) = self.functions.get(name.as_str()) {
                    self.body(&function.body, visited)
                } else {
                    // Opaque external/indirect calls cannot establish safety.
                    true
                }
            }
            Expr::Lambda(function) => {
                self.checked
                    .captures
                    .get(&expr.id())
                    .is_some_and(|captures| {
                        captures
                            .iter()
                            .any(|capture| capture.mutable && !capture.mutex)
                    })
                    || self.body(&function.body, visited)
            }
            Expr::Member { object, .. } => {
                if matches!(object.unlocated(), Expr::Name(name) if matches!(self.checked.types.get(name), Some(TypeInfo::Enum(_))))
                {
                    false
                } else {
                    self.callable(object, visited)
                }
            }
            Expr::Index { object, .. } => self.callable(object, visited),
            Expr::Array(values) | Expr::Tuple(values) => {
                values.iter().any(|value| self.callable(value, visited))
            }
            Expr::StructInit { fields, .. } => fields
                .iter()
                .any(|(_, value)| self.callable(value, visited)),
            Expr::Cast { value, .. } => self.callable(value, visited),
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::String(_)
            | Expr::Char(_)
            | Expr::None
            | Expr::Bytes(_) => false,
            // A factory may return a closure; inspect its body conservatively.
            Expr::Call { callee, .. } => self.callable(callee, visited),
            _ => true,
        }
    }
    fn body(&self, body: &Block, visited: &mut HashSet<usize>) -> bool {
        let mut risk = false;
        visit::block(body, &mut |expr| {
            if risk {
                return;
            }
            risk = self.checked.shared_accesses.contains(&expr.id());
            if let Expr::Call { callee, .. } = expr {
                risk |= self.callable(callee, visited);
            }
            if let Expr::Lambda(_) = expr {
                risk |= self
                    .checked
                    .captures
                    .get(&expr.id())
                    .is_some_and(|captures| {
                        captures
                            .iter()
                            .any(|capture| capture.mutable && !capture.mutex)
                    });
            }
        });
        risk
    }
}
